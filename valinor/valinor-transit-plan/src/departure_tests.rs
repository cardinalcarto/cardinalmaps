//! Departure ranges on ride legs, on synthetic feeds: one test per state a departure can be in.
//!
//! The main feed runs route 1 north from `A` through `B` to `C` every half hour in the morning,
//! late in the evening, and across midnight; route 2 east from `B` to `D`; and route 3 from `A`
//! to `C` as a frequency trip. Stops are farther apart than the access radius,
//! so each journey boards where the test expects.

use core::fmt::Write as _;

use insta::assert_json_snapshot;

use valinor_gtfs::{FeedId as CompileFeedId, FeedInput, InMemorySource, LatLon, LoadedTimetable};
use valinor_gtfs::{TimetableSet, compile};
use valinor_gtfs_rt::{FeedKind, proto};
use valinor_transit::Router;

use crate::departures::{DeparturesRequest, departures};
use crate::error::{Missing, NotRunning, PlanError, StateError};
use crate::plan::{DepartureRange, TransitOptions, TransitRequest, TravelMode, plan};
use crate::response::{
    Accessibility, AlertResponse, DepartureStatus, DeparturesResponse, JourneyLeg, PlanResponse,
    RideDeparture, TransitLeg,
};
use crate::ride::{RideRequest, ride};
use crate::state::{FeedUpdate, StateConfig, TransitState};
use crate::testing::{
    StraightLineAccess, StraightLineTransfers, alert_entity, delay_update, descriptor,
    encoded_feed, entity, june_15, route_selector,
};

const FEED: &str = "metro";
const FEED_TIME: i64 = 1_781_535_000; // 2026-06-15 08:10 PDT.
const JUNE_15: &str = "20260615";
const SCHEDULED: DepartureStatus = DepartureStatus::Scheduled;

const STOPS: &str = "stop_id,stop_name,stop_lat,stop_lon,wheelchair_boarding\n\
A,Alpha,47.6000,-122.3300,1\n\
B,Bravo,47.6200,-122.3300,1\n\
C,Charlie,47.6400,-122.3300,1\n\
D,Delta,47.6200,-122.3000,1\n";

/// Route 1's trips as `(start at A, wheelchair_accessible)`; ten minutes to each next stop.
const ROUTE_1: [(&str, u8); 10] = [
    ("07:00", 1),
    ("07:30", 1),
    ("08:00", 1),
    ("08:30", 1),
    ("09:00", 1),
    ("09:30", 2),
    ("23:00", 1),
    ("23:50", 1),
    ("24:20", 1),
    ("00:50", 1),
];

/// Route 2's departures from `B`, ten minutes to `D`.
const ROUTE_2: [&str; 3] = ["08:15", "08:25", "08:45"];

/// The walk from [`near_a`] to `A`.
const ACCESS_SECONDS: i64 = 84;

fn plus_minutes(time: &str, minutes: u32) -> String {
    let (hours, rest) = time.split_once(':').expect("HH:MM");
    let total = hours.parse::<u32>().expect("hours") * 60 + rest.parse::<u32>().expect("minutes");
    let total = total + minutes;
    format!("{:02}:{:02}:00", total / 60, total % 60)
}

fn trip_id(route: &str, start: &str) -> String {
    format!("{route}-{}", start.replace(':', ""))
}

fn main_source() -> InMemorySource {
    let mut trips = String::from(
        "route_id,service_id,trip_id,trip_headsign,wheelchair_accessible,bikes_allowed\n",
    );
    let mut times =
        String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence,stop_headsign\n");
    for (start, wheelchair) in ROUTE_1 {
        let id = trip_id("R1", start);
        writeln!(trips, "R1,WK,{id},North,{wheelchair},1").expect("to a string");
        for (sequence, (stop, offset)) in [("A", 0), ("B", 10), ("C", 20)].iter().enumerate() {
            let at = plus_minutes(start, *offset);
            // The 08:30 reads differently at `A` than its trip does.
            let headsign = if id == "R1-0830" && *stop == "A" {
                "Charlie Express"
            } else {
                ""
            };
            writeln!(times, "{id},{at},{at},{stop},{},{headsign}", sequence + 1)
                .expect("to a string");
        }
    }
    for start in ROUTE_2 {
        let id = trip_id("R2", start);
        writeln!(trips, "R2,WK,{id},East,1,").expect("to a string");
        for (sequence, (stop, offset)) in [("B", 0), ("D", 10)].iter().enumerate() {
            let at = plus_minutes(start, *offset);
            writeln!(times, "{id},{at},{at},{stop},{},", sequence + 1).expect("to a string");
        }
    }
    trips.push_str("R3,WK,F1,Express,1,\n");
    times.push_str("F1,10:00:00,10:00:00,A,1,\nF1,10:20:00,10:20:00,C,2,\n");
    InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\n\
             MT,Metro,America/Los_Angeles,https://m.test\n",
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_type\n\
             R1,MT,1,3\nR2,MT,2,3\nR3,MT,3,3\n",
        )
        .with("stops.txt", STOPS)
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with("trips.txt", &trips)
        .with("stop_times.txt", &times)
        .with(
            "frequencies.txt",
            "trip_id,start_time,end_time,headway_secs,exact_times\n\
             F1,10:00:00,11:00:00,1200,1\n",
        )
}

fn loaded(id: &str, source: InMemorySource) -> LoadedTimetable {
    let compiled = compile(vec![FeedInput {
        id: CompileFeedId::try_new(id.to_owned()).expect("valid id"),
        source: Box::new(source),
    }])
    .expect("compiles");
    LoadedTimetable::from_bytes(compiled.bytes).expect("loads")
}

fn main_state() -> TransitState {
    TransitState::new(loaded(FEED, main_source()), StateConfig::default()).expect("state")
}

fn at(lat: f64) -> LatLon {
    LatLon {
        lat,
        lon: -122.3300,
    }
}

/// 111 m south of `A`.
fn near_a() -> LatLon {
    at(47.5990)
}

fn near_c() -> LatLon {
    at(47.6410)
}

fn near_d() -> LatLon {
    LatLon {
        lat: 47.6200,
        lon: -122.2990,
    }
}

fn request(origin: LatLon, destination: LatLon, departure: i64, window: u32) -> TransitRequest {
    TransitRequest {
        window_seconds: window,
        options: TransitOptions {
            max_trips: 2,
            ..TransitOptions::default()
        },
        ..TransitRequest::new(origin, destination, departure, FEED_TIME + 15)
    }
}

fn try_run(
    state: &TransitState,
    request: &TransitRequest,
) -> Result<PlanResponse, PlanError<core::convert::Infallible, core::convert::Infallible>> {
    let snapshot = state.pin(request.now).expect("pin");
    let mut router = Router::new(StraightLineTransfers::new(
        snapshot.stop_locations(),
        500.0,
        1.33,
    ));
    plan(
        state,
        &mut router,
        &mut StraightLineAccess::default(),
        request,
        &|| false,
    )
}

fn run(state: &TransitState, request: &TransitRequest) -> PlanResponse {
    try_run(state, request).expect("plan succeeds")
}

fn ingest(state: &TransitState, feed: &str, kind: FeedKind, entities: Vec<proto::FeedEntity>) {
    let bytes = encoded_feed(FEED_TIME, entities);
    state
        .ingest(FeedUpdate {
            feed,
            kind,
            bytes: Some(&bytes),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
}

fn canceled(trip: &str) -> proto::FeedEntity {
    let mut item = entity(&format!("tu-{trip}"));
    item.trip_update = Some(proto::TripUpdate {
        trip: proto::TripDescriptor {
            schedule_relationship: Some(
                proto::trip_descriptor::ScheduleRelationship::Canceled as i32,
            ),
            ..descriptor(trip, JUNE_15, None)
        },
        ..proto::TripUpdate::default()
    });
    item
}

fn transit_legs(response: &PlanResponse) -> impl Iterator<Item = &TransitLeg> {
    response
        .journeys
        .iter()
        .flat_map(|journey| &journey.legs)
        .filter_map(|leg| match leg {
            JourneyLeg::Transit(leg) => Some(leg.as_ref()),
            _ => None,
        })
}

/// The leg riding `trip`, which the test expects the plan to have.
fn leg_on<'r>(response: &'r PlanResponse, trip: &str) -> &'r TransitLeg {
    transit_legs(response)
        .find(|leg| leg.trip.trip_id == trip)
        .unwrap_or_else(|| {
            let found: Vec<&str> = transit_legs(response)
                .map(|leg| leg.trip.trip_id.as_str())
                .collect();
            panic!("no leg rides {trip}; legs ride {found:?}")
        })
}

/// A departure as the tests compare it: `S` suggested, `M` missed, and `L` last of its day.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    trip: String,
    departure: i64,
    status: DepartureStatus,
    flags: String,
}

fn row(departure: &RideDeparture) -> Row {
    let mut flags = String::new();
    for (set, flag) in [
        (departure.suggested, 'S'),
        (departure.missed, 'M'),
        (departure.last_of_day, 'L'),
    ] {
        if set {
            flags.push(flag);
        }
    }
    Row {
        trip: departure.trip_id.clone(),
        departure: departure.departure,
        status: departure.status,
        flags,
    }
}

fn rows(leg: &TransitLeg) -> Vec<Row> {
    leg.departures.iter().map(row).collect()
}

fn expected(trip: &str, departure: i64, status: DepartureStatus, flags: &str) -> Row {
    Row {
        trip: trip.to_owned(),
        departure,
        status,
        flags: flags.to_owned(),
    }
}

fn leg_kind(leg: &JourneyLeg) -> &'static str {
    match leg {
        JourneyLeg::Access(_) => "access",
        JourneyLeg::Transit(_) => "transit",
        JourneyLeg::Transfer(_) => "transfer",
        JourneyLeg::Egress(_) => "egress",
    }
}

fn trips_of(leg: &TransitLeg) -> Vec<&str> {
    leg.departures.iter().map(|d| d.trip_id.as_str()).collect()
}

/// Each departure's route as its short name, where it differs from the leg's.
fn routes_of(leg: &TransitLeg) -> Vec<Option<&str>> {
    leg.departures
        .iter()
        .map(|d| {
            d.route
                .as_ref()
                .and_then(|route| route.short_name.as_deref())
        })
        .collect()
}

#[test]
fn a_ride_lists_every_route_between_its_stops_around_its_own_departure() {
    let state = main_state();
    let response = run(&state, &request(near_a(), near_c(), june_15(7, 55), 1800));
    let leg = leg_on(&response, "R1-0800");
    assert_eq!(
        rows(leg),
        vec![
            // Both leave before the rider, out at 07:55, can walk to `A`.
            expected("R1-0700", june_15(7, 0), SCHEDULED, "M"),
            expected("R1-0730", june_15(7, 30), SCHEDULED, "M"),
            expected("R1-0800", june_15(8, 0), SCHEDULED, "S"),
            expected("R1-0830", june_15(8, 30), SCHEDULED, ""),
            expected("R1-0900", june_15(9, 0), SCHEDULED, ""),
            expected("R1-0930", june_15(9, 30), SCHEDULED, ""),
            // Route 3 also runs from `A` to `C`, two hours after the leg's own at the most.
            expected("F1", june_15(10, 0), SCHEDULED, ""),
        ],
    );
    assert_eq!(
        routes_of(leg),
        vec![None, None, None, None, None, None, Some("3")],
        "only another route names its route"
    );
    let other = leg.departures[6].route.as_ref().expect("route 3");
    assert_eq!(other.route_id, "R3");
    assert_eq!(other.route_type, 3);
    assert_eq!(leg.departures[6].headsign.as_deref(), Some("Express"));
    let own = &leg.departures[2];
    assert_eq!(own.departure, leg.departure);
    assert_eq!(own.arrival, leg.arrival);
    assert_eq!(own.accessibility, leg.accessibility);
    let next = &leg.departures[3];
    assert_eq!(next.arrival, june_15(8, 50));
    assert_eq!(next.scheduled_departure, Some(june_15(8, 30)));
    assert_eq!(next.scheduled_arrival, Some(june_15(8, 50)));
    assert_eq!(next.delay_seconds, None);
    assert!(!next.realtime);
    assert_eq!(next.vehicle_id, None);
    assert_eq!(next.accessibility, Accessibility::Accessible);
    assert_eq!(next.bikes_allowed, Some("yes"));
    // What the leg already says isn't repeated, but a headsign of its own at the stop is shown.
    assert_eq!(next.headsign.as_deref(), Some("Charlie Express"));
    assert!(
        leg.departures
            .iter()
            .filter(|d| d.trip_id != "R1-0830" && d.trip_id != "F1")
            .all(|d| d.headsign.is_none())
    );
    assert_eq!(next.start_date, None);
    // The trip marked inaccessible says so.
    assert_eq!(leg.departures[5].accessibility, Accessibility::Inaccessible);
}

#[test]
fn the_first_ride_misses_what_leaves_before_the_walk_from_the_origin() {
    let state = main_state();
    // Out at 07:58:40 and at `A` at 08:00:04: the 08:00 is gone, so the plan takes the 08:30.
    let departure = june_15(8, 0) - ACCESS_SECONDS + 4;
    let response = run(&state, &request(near_a(), near_c(), departure, 1800));
    let leg = leg_on(&response, "R1-0830");
    // Seen from a leg whose own reads "Charlie Express" at `A`, the others read their trip's.
    assert_eq!(leg.departures[0].headsign.as_deref(), Some("North"));
    let missed: Vec<(&str, bool)> = leg
        .departures
        .iter()
        .map(|d| (d.trip_id.as_str(), d.missed))
        .collect();
    assert_eq!(
        missed,
        vec![
            ("R1-0730", true),
            ("R1-0800", true),
            ("R1-0830", false),
            ("R1-0900", false),
            ("R1-0930", false),
            ("F1", false),
            ("F1", false),
        ]
    );
}

#[test]
fn a_range_is_bounded_by_count_and_by_time_and_can_be_turned_off() {
    let state = main_state();
    let mut narrow = request(near_a(), near_c(), june_15(7, 55), 1800);
    narrow.options.departures = DepartureRange {
        before: 1,
        after: 5,
        before_seconds: 3600,
        after_seconds: 3600,
    };
    let response = run(&state, &narrow);
    // One before; five after are asked for, but only two leave within the hour.
    assert_eq!(
        trips_of(leg_on(&response, "R1-0800")),
        vec!["R1-0730", "R1-0800", "R1-0830", "R1-0900"]
    );

    let mut none = narrow;
    none.options.departures = DepartureRange::NONE;
    let response = run(&state, &none);
    let leg = leg_on(&response, "R1-0800");
    assert!(leg.departures.is_empty());
    let json = serde_json::to_value(leg).expect("serializes");
    assert!(
        json.get("departures").is_none(),
        "an empty range isn't serialized"
    );
    assert!(json.get("departure_alerts").is_none());
}

#[test]
fn a_range_too_wide_is_refused() {
    let state = main_state();
    let mut wide = request(near_a(), near_c(), june_15(7, 55), 1800);
    wide.options.departures.after = 11;
    let error = try_run(&state, &wide).expect_err("eleven is more than ten");
    assert!(matches!(error, PlanError::InvalidRequest(_)), "{error:?}");
}

#[test]
fn the_last_trip_of_a_service_day_is_marked_even_after_midnight() {
    let state = main_state();
    let response = run(&state, &request(near_a(), near_c(), june_15(23, 40), 3600));
    let leg = leg_on(&response, "R1-2350");
    assert_eq!(
        rows(leg),
        vec![
            expected("R1-2300", june_15(23, 0), SCHEDULED, "M"),
            expected("R1-2350", june_15(23, 50), SCHEDULED, "S"),
            // 00:20 on June 16, but on June 15's service day, whose last trip it is.
            expected("R1-2420", june_15(24, 20), SCHEDULED, "L"),
            // June 16's first trip, which is not its day's last.
            expected("R1-0050", june_15(24, 50), SCHEDULED, ""),
        ]
    );
    assert_eq!(leg.departures[2].start_date, None, "June 15, as the leg");
    assert_eq!(leg.departures[3].start_date.as_deref(), Some("20260616"));

    let late = leg_on(&response, "R1-2420");
    let own = late
        .departures
        .iter()
        .find(|d| d.suggested)
        .expect("its own");
    assert!(
        own.last_of_day,
        "the journey's own connection can be the last"
    );
}

#[test]
fn a_canceled_departure_is_listed_and_isnt_the_last() {
    let state = main_state();
    ingest(
        &state,
        FEED,
        FeedKind::TripUpdates,
        vec![canceled("R1-2420")],
    );
    let response = run(&state, &request(near_a(), near_c(), june_15(23, 40), 3600));
    let leg = leg_on(&response, "R1-2350");
    assert_eq!(
        rows(leg),
        vec![
            expected("R1-2300", june_15(23, 0), SCHEDULED, "M"),
            // With 00:20 canceled, 23:50 is the day's last.
            expected("R1-2350", june_15(23, 50), SCHEDULED, "SL"),
            expected("R1-2420", june_15(24, 20), DepartureStatus::Canceled, ""),
            expected("R1-0050", june_15(24, 50), SCHEDULED, ""),
        ]
    );
    let canceled = &leg.departures[2];
    assert_eq!(canceled.scheduled_departure, Some(june_15(24, 20)));
    assert!(!canceled.realtime);
    assert_eq!(canceled.delay_seconds, None);
}

#[test]
fn a_skipped_boarding_stop_is_listed_as_skipped() {
    let state = main_state();
    let mut skip = entity("tu-skip");
    skip.trip_update = Some(proto::TripUpdate {
        trip: descriptor("R1-0900", JUNE_15, None),
        stop_time_update: vec![proto::trip_update::StopTimeUpdate {
            stop_sequence: Some(1),
            schedule_relationship: Some(
                proto::trip_update::stop_time_update::ScheduleRelationship::Skipped as i32,
            ),
            ..proto::trip_update::StopTimeUpdate::default()
        }],
        ..proto::TripUpdate::default()
    });
    ingest(&state, FEED, FeedKind::TripUpdates, vec![skip]);
    let response = run(&state, &request(near_a(), near_c(), june_15(7, 55), 1800));
    let leg = leg_on(&response, "R1-0800");
    let statuses: Vec<(&str, DepartureStatus)> = leg
        .departures
        .iter()
        .map(|d| (d.trip_id.as_str(), d.status))
        .collect();
    assert_eq!(
        statuses,
        vec![
            ("R1-0700", SCHEDULED),
            ("R1-0730", SCHEDULED),
            ("R1-0800", SCHEDULED),
            ("R1-0830", SCHEDULED),
            ("R1-0900", DepartureStatus::Skipped),
            ("R1-0930", SCHEDULED),
            ("F1", SCHEDULED),
        ]
    );
    assert_eq!(
        leg.departures[4].departure,
        june_15(9, 0),
        "at its schedule"
    );
}

#[test]
fn a_late_first_ride_makes_the_scheduled_connection_one_the_rider_misses() {
    let state = main_state();
    let trip_to_d = request(near_a(), near_d(), june_15(7, 55), 1800);

    // On schedule: at `B` at 08:10, two minutes to change, and route 2 at 08:15.
    let response = run(&state, &trip_to_d);
    let journey = response
        .journeys
        .iter()
        .find(|journey| {
            journey
                .legs
                .iter()
                .any(|leg| matches!(leg, JourneyLeg::Transit(leg) if leg.trip.trip_id == "R2-0815"))
        })
        .expect("a journey changing to the 08:15");
    let kinds: Vec<&str> = journey.legs.iter().map(leg_kind).collect();
    assert_eq!(
        kinds,
        vec!["access", "transit", "transit", "egress"],
        "a change at one stop, without a walk"
    );
    let (JourneyLeg::Transit(first), JourneyLeg::Transit(second)) =
        (&journey.legs[1], &journey.legs[2])
    else {
        unreachable!("checked above");
    };
    assert_eq!(first.transfer_seconds, None, "the first ride has none");
    assert_eq!(
        second.transfer_seconds,
        Some(120),
        "the stop's change time after a ride to it"
    );
    assert_eq!(second.departures[0].departure, june_15(8, 15));
    assert!(
        !second.departures[0].missed,
        "08:10 plus two minutes is before 08:15"
    );
    assert_eq!(
        rows(leg_on(&response, "R2-0815")),
        vec![
            expected("R2-0815", june_15(8, 15), SCHEDULED, "S"),
            expected("R2-0825", june_15(8, 25), SCHEDULED, ""),
            expected("R2-0845", june_15(8, 45), SCHEDULED, "L"),
        ]
    );

    // Eight minutes late: at `B` at 08:18 and ready at 08:20, so the 08:15 is gone.
    ingest(
        &state,
        FEED,
        FeedKind::TripUpdates,
        vec![delay_update("R1-0800", JUNE_15, 1, 480)],
    );
    let response = run(&state, &trip_to_d);
    let first = leg_on(&response, "R1-0800");
    let own = first.departures.iter().find(|d| d.suggested).expect("own");
    assert_eq!(own.status, DepartureStatus::Live);
    assert!(own.realtime);
    assert_eq!(own.delay_seconds, Some(480));
    assert_eq!(own.departure, june_15(8, 8));
    assert_eq!(own.scheduled_departure, Some(june_15(8, 0)));
    assert_eq!(
        rows(leg_on(&response, "R2-0825")),
        vec![
            expected("R2-0815", june_15(8, 15), SCHEDULED, "M"),
            expected("R2-0825", june_15(8, 25), SCHEDULED, "S"),
            expected("R2-0845", june_15(8, 45), SCHEDULED, "L"),
        ]
    );
}

#[test]
fn a_delayed_departure_is_live_and_shows_its_delay() {
    let state = main_state();
    ingest(
        &state,
        FEED,
        FeedKind::TripUpdates,
        vec![delay_update("R1-0830", JUNE_15, 1, 120)],
    );
    let response = run(&state, &request(near_a(), near_c(), june_15(7, 55), 1800));
    let leg = leg_on(&response, "R1-0800");
    let delayed = leg
        .departures
        .iter()
        .find(|d| d.trip_id == "R1-0830")
        .expect("listed");
    assert_eq!(delayed.status, DepartureStatus::Live);
    assert!(delayed.realtime);
    assert_eq!(delayed.departure, june_15(8, 32));
    assert_eq!(delayed.scheduled_departure, Some(june_15(8, 30)));
    assert_eq!(delayed.delay_seconds, Some(120));
}

#[test]
fn an_added_trip_on_the_route_is_listed_live() {
    let state = main_state();
    let stop = |stop: &str, sequence: u32, unix: i64| {
        let event = proto::trip_update::StopTimeEvent {
            time: Some(unix),
            ..proto::trip_update::StopTimeEvent::default()
        };
        proto::trip_update::StopTimeUpdate {
            stop_sequence: Some(sequence),
            stop_id: Some(stop.to_owned()),
            arrival: Some(event),
            departure: Some(event),
            ..proto::trip_update::StopTimeUpdate::default()
        }
    };
    let mut added = entity("tu-added");
    added.trip_update = Some(proto::TripUpdate {
        trip: proto::TripDescriptor {
            route_id: Some("R1".to_owned()),
            schedule_relationship: Some(proto::trip_descriptor::ScheduleRelationship::New as i32),
            ..descriptor("EXTRA", JUNE_15, None)
        },
        stop_time_update: vec![
            stop("A", 1, june_15(8, 10)),
            stop("B", 2, june_15(8, 20)),
            stop("C", 3, june_15(8, 30)),
        ],
        ..proto::TripUpdate::default()
    });
    ingest(&state, FEED, FeedKind::TripUpdates, vec![added]);
    let response = run(&state, &request(near_a(), near_c(), june_15(7, 55), 1800));
    let leg = leg_on(&response, "R1-0800");
    let trips: Vec<(&str, DepartureStatus)> = leg
        .departures
        .iter()
        .map(|d| (d.trip_id.as_str(), d.status))
        .collect();
    assert_eq!(
        trips,
        vec![
            ("R1-0700", SCHEDULED),
            ("R1-0730", SCHEDULED),
            ("R1-0800", SCHEDULED),
            ("EXTRA", DepartureStatus::Live),
            ("R1-0830", SCHEDULED),
            ("R1-0900", SCHEDULED),
            ("R1-0930", SCHEDULED),
            ("F1", SCHEDULED),
        ]
    );
    let extra = &leg.departures[3];
    assert_eq!(extra.route, None, "the update names the leg's route");
    assert_eq!(
        extra.scheduled_departure, None,
        "an added trip has no schedule"
    );
    assert_eq!(extra.arrival, june_15(8, 30));
}

#[test]
fn a_trip_no_route_names_lists_every_route_between_its_stops() {
    let state = main_state();
    let stop = |stop: &str, sequence: u32, unix: i64| {
        let event = proto::trip_update::StopTimeEvent {
            time: Some(unix),
            ..proto::trip_update::StopTimeEvent::default()
        };
        proto::trip_update::StopTimeUpdate {
            stop_sequence: Some(sequence),
            stop_id: Some(stop.to_owned()),
            arrival: Some(event),
            departure: Some(event),
            ..proto::trip_update::StopTimeUpdate::default()
        }
    };
    let mut added = entity("tu-loner");
    added.trip_update = Some(proto::TripUpdate {
        trip: proto::TripDescriptor {
            schedule_relationship: Some(proto::trip_descriptor::ScheduleRelationship::New as i32),
            ..descriptor("LONER", JUNE_15, None)
        },
        stop_time_update: vec![stop("A", 1, june_15(8, 10)), stop("C", 2, june_15(8, 30))],
        ..proto::TripUpdate::default()
    });
    ingest(&state, FEED, FeedKind::TripUpdates, vec![added]);
    let response = run(&state, &request(near_a(), near_c(), june_15(8, 5), 600));
    let leg = leg_on(&response, "LONER");
    assert_eq!(leg.route, None);
    assert_eq!(
        rows(leg),
        vec![
            expected("R1-0730", june_15(7, 30), SCHEDULED, "M"),
            expected("R1-0800", june_15(8, 0), SCHEDULED, "M"),
            expected("LONER", june_15(8, 10), DepartureStatus::Live, "S"),
            expected("R1-0830", june_15(8, 30), SCHEDULED, ""),
            expected("R1-0900", june_15(9, 0), SCHEDULED, ""),
            expected("R1-0930", june_15(9, 30), SCHEDULED, ""),
            expected("F1", june_15(10, 0), SCHEDULED, ""),
        ],
        "the stops' departures, on routes it isn't"
    );
    assert_eq!(
        routes_of(leg),
        vec![
            Some("1"),
            Some("1"),
            None,
            Some("1"),
            Some("1"),
            Some("1"),
            Some("3")
        ]
    );
}

#[test]
fn frequency_runs_are_each_a_departure() {
    let state = main_state();
    let response = run(&state, &request(near_a(), near_c(), june_15(9, 57), 600));
    let leg = leg_on(&response, "F1");
    let runs: Vec<(i64, bool)> = leg
        .departures
        .iter()
        .map(|d| (d.departure, d.suggested))
        .collect();
    assert_eq!(
        runs,
        vec![
            (june_15(9, 0), false),
            (june_15(9, 30), false),
            (june_15(10, 0), true),
            (june_15(10, 20), false),
            (june_15(10, 40), false),
        ],
    );
    assert_eq!(trips_of(leg)[2..], ["F1", "F1", "F1"]);
    assert_eq!(routes_of(leg), vec![Some("1"), Some("1"), None, None, None]);
    assert!(
        leg.departures.iter().all(|d| !d.last_of_day),
        "route 3's last run isn't the last between `A` and `C`: route 1 runs until after midnight"
    );
}

#[test]
fn a_wheelchair_rider_sees_only_departures_they_can_take() {
    let state = main_state();
    let response = run(
        &state,
        &TransitRequest {
            mode: TravelMode::Wheelchair,
            ..request(near_a(), near_c(), june_15(7, 55), 1800)
        },
    );
    let leg = leg_on(&response, "R1-0800");
    assert_eq!(
        trips_of(leg),
        vec!["R1-0700", "R1-0730", "R1-0800", "R1-0830", "R1-0900", "F1"],
        "the 09:30 is marked inaccessible"
    );
    assert!(
        leg.departures
            .iter()
            .all(|d| d.accessibility == Accessibility::Accessible)
    );
}

#[test]
fn alerts_on_departures_point_at_the_legs_or_their_own() {
    use proto::alert::Effect;
    let state = main_state();
    let trip_alert = alert_entity(
        "trip-0930",
        Effect::ReducedService,
        vec![proto::EntitySelector {
            trip: Some(descriptor("R1-0930", JUNE_15, None)),
            ..proto::EntitySelector::default()
        }],
    );
    ingest(
        &state,
        FEED,
        FeedKind::Alerts,
        vec![
            alert_entity("route-1", Effect::Detour, vec![route_selector("R1")]),
            trip_alert,
        ],
    );
    let response = run(&state, &request(near_a(), near_c(), june_15(7, 55), 1800));
    let leg = leg_on(&response, "R1-0800");
    assert_eq!(leg.alerts.len(), 1, "the route's alert");
    assert_eq!(leg.alerts[0].alert.id, "route-1");
    assert_eq!(leg.departure_alerts.len(), 1, "the 09:30's own");
    assert_eq!(leg.departure_alerts[0].id, "trip-0930");
    assert_eq!(trips_of(leg).last(), Some(&"F1"));
    for departure in &leg.departures {
        // Route 1's alert isn't about route 3.
        let leg_alerts: &[usize] = if departure.trip_id == "F1" { &[] } else { &[0] };
        assert_eq!(departure.alerts, leg_alerts, "{}", departure.trip_id);
        let own: &[usize] = if departure.trip_id == "R1-0930" {
            &[0]
        } else {
            &[]
        };
        assert_eq!(departure.departure_alerts, own, "{}", departure.trip_id);
    }
}

#[test]
fn json_requests_set_the_range() {
    let (request, _) = crate::json::parse_body_for_test(
        br#"{"origin":{"lat":47.599,"lon":-122.33},"destination":{"lat":47.641,"lon":-122.33},
            "departures_before":0,"departures_after":4}"#,
        FEED_TIME,
    );
    let defaults = DepartureRange::default();
    assert_eq!(
        request.options.departures,
        DepartureRange {
            before: 0,
            after: 4,
            ..defaults
        }
    );
    let (request, _) = crate::json::parse_body_for_test(
        br#"{"origin":{"lat":47.599,"lon":-122.33},"destination":{"lat":47.641,"lon":-122.33}}"#,
        FEED_TIME,
    );
    assert_eq!(request.options.departures, defaults);
}

// Stay-seated trips: `T1` (route 1, `ORG` to `MID`) continues as `T2` (route 2, `MID` to `DST`)
// on block `X`, and `T5` as `T6` on block `Y`; `T3` (route 1) and `T4` (route 2) run alone,
// and `T7` (route 3) runs from `ORG` to `DST` without stopping at `MID`.

const SEATED_STOPS: &str = "stop_id,stop_name,stop_lat,stop_lon\n\
ORG,Origin,47.6000,-122.3300\n\
MID,Middle,47.6200,-122.3300\n\
DST,Destination,47.6400,-122.3300\n";

const SEATED_TRIPS: &str = "route_id,service_id,trip_id,block_id,trip_headsign\n\
R1,WK,T1,X,To Middle\nR2,WK,T2,X,To Destination\n\
R1,WK,T3,,To Middle\nR2,WK,T4,,To Destination\n\
R1,WK,T5,Y,To Middle\nR2,WK,T6,Y,To Destination\n\
R3,WK,T7,,To Destination\n";

const SEATED_TIMES: &str = "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
T1,08:05:00,08:05:00,ORG,1\nT1,08:20:00,08:20:00,MID,2\n\
T2,08:25:00,08:25:00,MID,1\nT2,08:50:00,08:50:00,DST,2\n\
T3,08:35:00,08:35:00,ORG,1\nT3,08:50:00,08:50:00,MID,2\n\
T4,09:00:00,09:00:00,MID,1\nT4,09:25:00,09:25:00,DST,2\n\
T5,09:05:00,09:05:00,ORG,1\nT5,09:20:00,09:20:00,MID,2\n\
T6,09:25:00,09:25:00,MID,1\nT6,09:50:00,09:50:00,DST,2\n\
T7,08:40:00,08:40:00,ORG,1\nT7,09:15:00,09:15:00,DST,2\n";

fn seated_state() -> TransitState {
    let source = InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\n\
             A,Metro,America/Los_Angeles,https://m.test\n",
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_type\nR1,A,1,3\nR2,A,2,3\nR3,A,3,3\n",
        )
        .with("stops.txt", SEATED_STOPS)
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with("trips.txt", SEATED_TRIPS)
        .with("stop_times.txt", SEATED_TIMES);
    TransitState::new(loaded("seated", source), StateConfig::default()).expect("state")
}

#[test]
fn a_stay_seated_ride_lists_trips_that_make_the_same_change() {
    let state = seated_state();
    let response = run(
        &state,
        &request(at(47.5990), at(47.6410), june_15(8, 0), 3600),
    );
    let seated = transit_legs(&response)
        .find(|leg| leg.trip.trip_id == "T1" && !leg.segments.is_empty())
        .expect("the ride stays seated from T1 into T2");
    // `T3` and `T4` don't make the ride without a change; `T5` into `T6` does, and so does
    // `T7` on route 3, without one.
    assert_eq!(trips_of(seated), vec!["T1", "T7", "T5"]);
    assert_eq!(
        routes_of(seated),
        vec![None, Some("3"), None],
        "`T5` boards route 1, as the leg"
    );
    assert_eq!(seated.departures[2].departure, june_15(9, 5));
    assert_eq!(seated.departures[2].arrival, june_15(9, 50));
    assert!(seated.departures[2].last_of_day);
    assert!(!seated.departures[1].last_of_day);

    // Seen from `T7`, the seated rides are on the route they board, route 1.
    let response = run(
        &state,
        &request(at(47.5990), at(47.6410), june_15(8, 35), 600),
    );
    let direct = leg_on(&response, "T7");
    assert_eq!(trips_of(direct), vec!["T1", "T7", "T5"]);
    assert_eq!(routes_of(direct), vec![Some("1"), None, Some("1")]);
    let seated = &direct.departures[0];
    assert_eq!(
        seated.route.as_ref().map(|r| r.route_id.as_str()),
        Some("R1")
    );
    assert_eq!(seated.headsign.as_deref(), Some("To Middle"));
    assert_eq!(seated.arrival, june_15(8, 50), "where `T2` reaches `DST`");
}

#[test]
fn a_ride_on_one_route_lists_combined_trips_on_that_route_once() {
    let state = seated_state();
    // From the origin to the middle: route 1 alone, which `T1` and `T5` ride before continuing.
    let response = run(
        &state,
        &request(at(47.5990), at(47.6210), june_15(8, 30), 900),
    );
    let leg = leg_on(&response, "T3");
    assert_eq!(trips_of(leg), vec!["T1", "T3", "T5"]);
    let times: Vec<(i64, i64)> = leg
        .departures
        .iter()
        .map(|d| (d.departure, d.arrival))
        .collect();
    assert_eq!(
        times,
        vec![
            (june_15(8, 5), june_15(8, 20)),
            (june_15(8, 35), june_15(8, 50)),
            (june_15(9, 5), june_15(9, 20)),
        ]
    );
    assert!(leg.departures[2].last_of_day);

    // From the middle to the destination: route 2, boarded where `T2` and `T6` begin.
    let response = run(
        &state,
        &request(at(47.6190), at(47.6410), june_15(8, 50), 900),
    );
    let leg = leg_on(&response, "T4");
    assert_eq!(trips_of(leg), vec!["T2", "T4", "T6"]);
    assert!(
        leg.departures.iter().all(|d| d.headsign.is_none()),
        "every one is To Destination, as the leg"
    );
}

// Two files: `alpha` in Pacific time and `bravo` in Eastern, with the same stop, route, and trip IDs.

fn line(tz: &str, lat: f64, starts: &[&str]) -> InMemorySource {
    let mut trips = String::from("route_id,service_id,trip_id\n");
    let mut times = String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence\n");
    for (index, start) in starts.iter().enumerate() {
        let id = format!("T{}", index + 1);
        writeln!(trips, "R1,WK,{id}").expect("to a string");
        let (begin, end) = (plus_minutes(start, 0), plus_minutes(start, 20));
        writeln!(times, "{id},{begin},{begin},S1,1\n{id},{end},{end},S2,2").expect("to a string");
    }
    InMemorySource::new()
        .with(
            "agency.txt",
            &format!(
                "agency_id,agency_name,agency_timezone,agency_url\nAG,Agency,{tz},https://a.test\n"
            ),
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_type\nR1,AG,1,3\n",
        )
        .with(
            "stops.txt",
            &format!(
                "stop_id,stop_name,stop_lat,stop_lon\nS1,One,{lat},-122.33\nS2,Two,{},-122.33\n",
                lat + 0.02
            ),
        )
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,\
             start_date,end_date\nWK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with("trips.txt", &trips)
        .with("stop_times.txt", &times)
}

#[test]
fn departures_come_from_the_rides_own_file_and_its_own_overlay() {
    let state = TransitState::from_set(
        TimetableSet::new(vec![
            loaded(
                "alpha",
                line("America/Los_Angeles", 47.50, &["08:00", "08:30", "09:00"]),
            ),
            // 08:00, 08:30, and 09:00 Pacific.
            loaded(
                "bravo",
                line("America/New_York", 47.70, &["11:00", "11:30", "12:00"]),
            ),
        ])
        .expect("set"),
        StateConfig::default(),
    )
    .expect("state");
    // The same trip ID in each feed: for a ride on bravo, only bravo's cancellation counts.
    ingest(&state, "alpha", FeedKind::TripUpdates, vec![canceled("T2")]);
    ingest(&state, "bravo", FeedKind::TripUpdates, vec![canceled("T3")]);
    let response = run(
        &state,
        &request(at(47.6990), at(47.7210), june_15(7, 55), 600),
    );
    let leg = leg_on(&response, "T1");
    assert_eq!(leg.trip.feed.as_deref(), Some("bravo"));
    let rows: Vec<(&str, Option<i64>, DepartureStatus)> = leg
        .departures
        .iter()
        .map(|d| (d.trip_id.as_str(), d.scheduled_departure, d.status))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("T1", Some(june_15(8, 0)), SCHEDULED),
            ("T2", Some(june_15(8, 30)), SCHEDULED),
            ("T3", Some(june_15(9, 0)), DepartureStatus::Canceled),
        ]
    );
    assert!(leg.departures[1].last_of_day, "T3 is canceled");
}

// Two routes between the same two stops: route 7 and route 14 from `P` to `Q`, alternating,
// and route 9 from `W`, a short walk from `Q`, to `R`.

const CONNECTION: &str = "conn";

const CONNECTION_STOPS: &str = "stop_id,stop_name,stop_lat,stop_lon,wheelchair_boarding\n\
P,Papa,47.6000,-122.3300,1\n\
Q,Quebec,47.6200,-122.3300,1\n\
W,Whiskey,47.6215,-122.3300,1\n\
R,Romeo,47.6400,-122.3300,1\n";

/// `(route, start, minutes to the next stop, wheelchair_accessible, from, to)`.
const CONNECTION_TRIPS: [(&str, &str, u32, u8, &str, &str); 10] = [
    ("R7", "21:00", 10, 1, "P", "Q"),
    ("R7", "21:30", 10, 1, "P", "Q"),
    ("R7", "22:00", 10, 1, "P", "Q"),
    ("R14", "21:10", 12, 1, "P", "Q"),
    ("R14", "21:40", 12, 2, "P", "Q"),
    ("R14", "22:30", 12, 1, "P", "Q"),
    ("R9", "21:11", 10, 1, "W", "R"),
    ("R9", "21:20", 10, 1, "W", "R"),
    ("R9", "21:35", 10, 1, "W", "R"),
    ("R9", "21:50", 10, 1, "W", "R"),
];

fn connection_state() -> TransitState {
    let mut trips =
        String::from("route_id,service_id,trip_id,trip_headsign,wheelchair_accessible\n");
    let mut times = String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence\n");
    for (route, start, minutes, wheelchair, from, to) in CONNECTION_TRIPS {
        let id = trip_id(route, start);
        let headsign = match route {
            "R14" => "Mount Baker",
            _ => "Downtown",
        };
        writeln!(trips, "{route},WK,{id},{headsign},{wheelchair}").expect("to a string");
        let (begin, end) = (plus_minutes(start, 0), plus_minutes(start, minutes));
        writeln!(
            times,
            "{id},{begin},{begin},{from},1\n{id},{end},{end},{to},2"
        )
        .expect("to a string");
    }
    let source = InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\n\
             MT,Metro,America/Los_Angeles,https://m.test\n",
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_type\n\
             R7,MT,7,3\nR14,MT,14,3\nR9,MT,9,3\n",
        )
        .with("stops.txt", CONNECTION_STOPS)
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with("trips.txt", &trips)
        .with("stop_times.txt", &times);
    TransitState::new(loaded(CONNECTION, source), StateConfig::default()).expect("state")
}

/// 111 m south of `P`, and of `Q`, which is 278 m from `W`.
fn near_p() -> LatLon {
    at(47.5990)
}

fn near_q() -> LatLon {
    at(47.6190)
}

fn near_r() -> LatLon {
    at(47.6410)
}

#[test]
fn a_ride_lists_every_route_between_its_two_stops() {
    let state = connection_state();
    let response = run(&state, &request(near_p(), near_q(), june_15(20, 55), 600));
    let leg = leg_on(&response, "R7-2100");
    assert_eq!(
        rows(leg),
        vec![
            expected("R7-2100", june_15(21, 0), SCHEDULED, "S"),
            expected("R14-2110", june_15(21, 10), SCHEDULED, ""),
            expected("R7-2130", june_15(21, 30), SCHEDULED, ""),
            expected("R14-2140", june_15(21, 40), SCHEDULED, ""),
            expected("R7-2200", june_15(22, 0), SCHEDULED, ""),
            // The last between `P` and `Q`, of either route.
            expected("R14-2230", june_15(22, 30), SCHEDULED, "L"),
        ]
    );
    assert_eq!(
        routes_of(leg),
        vec![None, Some("14"), None, Some("14"), None, Some("14")]
    );
    let headsigns: Vec<Option<&str>> = leg
        .departures
        .iter()
        .map(|d| d.headsign.as_deref())
        .collect();
    assert_eq!(
        headsigns,
        vec![
            None,
            Some("Mount Baker"),
            None,
            Some("Mount Baker"),
            None,
            Some("Mount Baker")
        ]
    );
    let other = &leg.departures[1];
    assert_eq!(
        other.arrival,
        june_15(21, 22),
        "route 14's own running time"
    );
    let json = serde_json::to_value(leg).expect("serializes");
    assert!(json["departures"][0].get("route").is_none());
    assert_eq!(json["departures"][1]["route"]["route_id"], "R14");
    assert_eq!(json["departures"][1]["route"]["short_name"], "14");
    assert!(
        json.get("transfer_seconds").is_none(),
        "the journey's first ride"
    );

    // Seen from route 14, route 7 names its route, and the one before the rider is out is missed.
    let response = run(&state, &request(near_p(), near_q(), june_15(21, 5), 300));
    let leg = leg_on(&response, "R14-2110");
    assert_eq!(
        rows(leg),
        vec![
            expected("R7-2100", june_15(21, 0), SCHEDULED, "M"),
            expected("R14-2110", june_15(21, 10), SCHEDULED, "S"),
            expected("R7-2130", june_15(21, 30), SCHEDULED, ""),
            expected("R14-2140", june_15(21, 40), SCHEDULED, ""),
            expected("R7-2200", june_15(22, 0), SCHEDULED, ""),
            expected("R14-2230", june_15(22, 30), SCHEDULED, "L"),
        ]
    );
    assert_eq!(
        routes_of(leg),
        vec![Some("7"), None, Some("7"), None, Some("7"), None]
    );
    assert_eq!(leg.departures[0].headsign.as_deref(), Some("Downtown"));
}

#[test]
fn the_last_of_the_day_is_the_connections_and_a_canceled_one_isnt_it() {
    let state = connection_state();
    ingest(
        &state,
        CONNECTION,
        FeedKind::TripUpdates,
        vec![canceled("R14-2230")],
    );
    let response = run(&state, &request(near_p(), near_q(), june_15(20, 55), 600));
    let leg = leg_on(&response, "R7-2100");
    assert_eq!(
        rows(leg),
        vec![
            expected("R7-2100", june_15(21, 0), SCHEDULED, "S"),
            expected("R14-2110", june_15(21, 10), SCHEDULED, ""),
            expected("R7-2130", june_15(21, 30), SCHEDULED, ""),
            // Route 14's last that runs, but route 7 runs later.
            expected("R14-2140", june_15(21, 40), SCHEDULED, ""),
            // With 22:30 canceled, the last of either route.
            expected("R7-2200", june_15(22, 0), SCHEDULED, "L"),
            expected("R14-2230", june_15(22, 30), DepartureStatus::Canceled, ""),
        ]
    );
    assert_eq!(
        leg.departures[5]
            .route
            .as_ref()
            .map(|r| r.route_id.as_str()),
        Some("R14")
    );
}

#[test]
fn a_wheelchair_rider_sees_only_the_routes_departures_they_can_take() {
    let state = connection_state();
    let response = run(
        &state,
        &TransitRequest {
            mode: TravelMode::Wheelchair,
            ..request(near_p(), near_q(), june_15(20, 55), 600)
        },
    );
    let leg = leg_on(&response, "R7-2100");
    assert_eq!(
        trips_of(leg),
        vec!["R7-2100", "R14-2110", "R7-2130", "R7-2200", "R14-2230"],
        "route 14's 21:40 is marked inaccessible"
    );
}

#[test]
fn a_ride_after_a_walk_counts_its_transfer_from_the_previous_arrival() {
    let state = connection_state();
    let response = run(&state, &request(near_p(), near_r(), june_15(20, 55), 600));
    let journey = response
        .journeys
        .iter()
        .find(|journey| {
            journey
                .legs
                .iter()
                .any(|leg| matches!(leg, JourneyLeg::Transit(leg) if leg.trip.trip_id == "R7-2100"))
        })
        .expect("a journey on route 7's 21:00");
    let kinds: Vec<&str> = journey.legs.iter().map(leg_kind).collect();
    assert_eq!(
        kinds,
        vec!["access", "transit", "transfer", "transit", "egress"]
    );
    let (JourneyLeg::Transit(first), JourneyLeg::Transfer(walk), JourneyLeg::Transit(second)) =
        (&journey.legs[1], &journey.legs[2], &journey.legs[3])
    else {
        unreachable!("checked above");
    };
    assert_eq!(first.transfer_seconds, None);
    let transfer = second.transfer_seconds.expect("a ride after the first");
    // 167 m at 1.33 m/s, and the walk is the whole of routing's cost here.
    assert_eq!(transfer, walk.duration_seconds);
    assert_eq!(transfer, 126);
    assert_eq!(second.trip.trip_id, "R9-2120");
    // At `Q` at 21:10 and at `W` 126 s later, after the 21:11 leaves.
    let ready = first.arrival + transfer;
    for departure in &second.departures {
        assert_eq!(
            departure.missed,
            departure.departure < ready,
            "{}",
            departure.trip_id
        );
    }
    assert_eq!(
        rows(second),
        vec![
            expected("R9-2111", june_15(21, 11), SCHEDULED, "M"),
            expected("R9-2120", june_15(21, 20), SCHEDULED, "S"),
            expected("R9-2135", june_15(21, 35), SCHEDULED, ""),
            expected("R9-2150", june_15(21, 50), SCHEDULED, "L"),
        ]
    );
}

// A ride lookup describes one departure of a ride as the plan's leg on it would be.

fn ride_request(feed: &str, (board, alight): (&str, &str), trip: &str) -> RideRequest {
    RideRequest {
        feed: feed.to_owned(),
        board_stop_id: board.to_owned(),
        alight_stop_id: alight.to_owned(),
        trip_id: trip.to_owned(),
        start_date: JUNE_15.to_owned(),
        departure: None,
        mode: TravelMode::Walking,
        now: FEED_TIME + 15,
    }
}

type LookupError = PlanError<core::convert::Infallible, core::convert::Infallible>;

fn try_lookup(state: &TransitState, request: &RideRequest) -> Result<TransitLeg, LookupError> {
    ride(state, request, &|| false).map(|response| response.ride)
}

fn lookup(state: &TransitState, request: &RideRequest) -> TransitLeg {
    try_lookup(state, request).expect("the lookup succeeds")
}

fn stop_ids(leg: &TransitLeg) -> Vec<&str> {
    leg.stops
        .iter()
        .map(|visit| visit.stop.stop_id.as_str())
        .collect()
}

/// A plan's leg less what only a plan knows: when the rider gets there, and the other departures.
fn as_looked_up(leg: &TransitLeg) -> TransitLeg {
    TransitLeg {
        wait_seconds: 0,
        transfer_seconds: None,
        departures: Vec::new(),
        departure_alerts: Vec::new(),
        ..leg.clone()
    }
}

#[test]
fn a_rides_own_departure_looks_up_as_the_plans_leg() {
    let state = main_state();
    // Live, with a delay and an alert on the route, so every part of the leg is compared.
    ingest(
        &state,
        FEED,
        FeedKind::TripUpdates,
        vec![delay_update("R1-0800", JUNE_15, 1, 120)],
    );
    ingest(
        &state,
        FEED,
        FeedKind::Alerts,
        vec![alert_entity(
            "detour",
            proto::alert::Effect::Detour,
            vec![route_selector("R1")],
        )],
    );
    let response = run(&state, &request(near_a(), near_c(), june_15(7, 55), 1800));
    let leg = leg_on(&response, "R1-0800");
    assert!(leg.realtime && !leg.alerts.is_empty(), "{leg:?}");
    let looked_up = lookup(&state, &ride_request(FEED, ("A", "C"), "R1-0800"));
    assert_eq!(looked_up, as_looked_up(leg));

    // A plan's ride after a change, which waits and has a transfer, looks up the same way.
    let response = run(&state, &request(near_a(), near_d(), june_15(7, 55), 1800));
    let second = transit_legs(&response)
        .find(|leg| leg.trip.trip_id.starts_with("R2"))
        .expect("a ride on route 2");
    assert!(second.transfer_seconds.is_some());
    let looked_up = lookup(
        &state,
        &ride_request(FEED, ("B", "D"), &second.trip.trip_id),
    );
    assert_eq!(looked_up, as_looked_up(second));
}

#[test]
fn another_routes_departure_looks_up_as_a_leg_on_its_route() {
    let state = connection_state();
    let response = run(&state, &request(near_p(), near_q(), june_15(20, 55), 600));
    let leg = leg_on(&response, "R7-2100");
    let other = &leg.departures[1];
    assert_eq!(other.trip_id, "R14-2110");
    let looked_up = lookup(&state, &ride_request(CONNECTION, ("P", "Q"), "R14-2110"));
    assert_eq!(looked_up.trip.trip_id, "R14-2110");
    assert_eq!(looked_up.trip.start_date.as_deref(), Some(JUNE_15));
    assert_eq!(looked_up.trip.headsign.as_deref(), Some("Mount Baker"));
    assert_eq!(
        looked_up
            .route
            .as_ref()
            .map(|route| route.route_id.as_str()),
        Some("R14")
    );
    assert_eq!(stop_ids(&looked_up), ["P", "Q"]);
    // It agrees with what the ride's list said of it.
    assert_eq!(
        (looked_up.departure, looked_up.arrival),
        (other.departure, other.arrival)
    );
    assert_eq!(looked_up.accessibility, other.accessibility);
    assert_eq!(looked_up.wait_seconds, 0);

    // As a plan would have it, had it ridden route 14.
    let response = run(&state, &request(near_p(), near_q(), june_15(21, 5), 300));
    assert_eq!(looked_up, as_looked_up(leg_on(&response, "R14-2110")));
}

#[test]
fn a_stay_seated_departure_looks_up_as_one_ride_in_segments() {
    let state = seated_state();
    let response = run(
        &state,
        &request(at(47.5990), at(47.6410), june_15(8, 35), 600),
    );
    let direct = leg_on(&response, "T7");
    let seated = &direct.departures[0];
    assert_eq!(seated.trip_id, "T1");
    let looked_up = lookup(&state, &ride_request("seated", ("ORG", "DST"), "T1"));
    assert_eq!(stop_ids(&looked_up), ["ORG", "MID", "DST"]);
    let segments: Vec<(&str, usize, usize)> = looked_up
        .segments
        .iter()
        .map(|s| (s.trip_id.as_str(), s.first_stop_index, s.last_stop_index))
        .collect();
    assert_eq!(segments, [("T1", 0, 1), ("T2", 1, 2)]);
    assert_eq!(looked_up.trip.trip_id, "T1");
    assert_eq!(
        looked_up
            .route
            .as_ref()
            .map(|route| route.route_id.as_str()),
        Some("R1"),
        "the route it boards"
    );
    assert_eq!(
        (looked_up.departure, looked_up.arrival),
        (seated.departure, seated.arrival)
    );

    // The plan that rides it has the same leg.
    let response = run(
        &state,
        &request(at(47.5990), at(47.6410), june_15(8, 0), 3600),
    );
    let planned = transit_legs(&response)
        .find(|leg| leg.trip.trip_id == "T1" && !leg.segments.is_empty())
        .expect("the ride stays seated from T1 into T2");
    assert_eq!(looked_up, as_looked_up(planned));
}

#[test]
fn a_frequency_trips_run_is_the_one_leaving_nearest_the_departure() {
    let state = main_state();
    let mut request = ride_request(FEED, ("A", "C"), "F1");
    assert_eq!(
        lookup(&state, &request).departure,
        june_15(10, 0),
        "the first"
    );
    request.departure = Some(june_15(10, 20));
    assert_eq!(lookup(&state, &request).departure, june_15(10, 20));
    request.departure = Some(june_15(10, 31));
    assert_eq!(lookup(&state, &request).departure, june_15(10, 40));
    // Halfway between two runs, the earlier.
    request.departure = Some(june_15(10, 30));
    assert_eq!(lookup(&state, &request).departure, june_15(10, 20));
}

#[test]
fn a_departure_real_time_data_took_away_is_refused() {
    let state = main_state();
    ingest(
        &state,
        FEED,
        FeedKind::TripUpdates,
        vec![canceled("R1-2420")],
    );
    // Listed as canceled by a ride from `A` to `C`, but not one a rider can take.
    let error = try_lookup(&state, &ride_request(FEED, ("A", "C"), "R1-2420")).unwrap_err();
    assert!(
        matches!(error, PlanError::NotRunning(NotRunning::Canceled)),
        "{error:?}"
    );
    // Its trip still runs on the other days.
    let mut tomorrow = ride_request(FEED, ("A", "C"), "R1-2420");
    tomorrow.start_date = "20260616".to_owned();
    assert_eq!(lookup(&state, &tomorrow).departure, june_15(24 + 24, 20));

    let mut skip = entity("tu-skip");
    skip.trip_update = Some(proto::TripUpdate {
        trip: descriptor("R1-0900", JUNE_15, None),
        stop_time_update: vec![proto::trip_update::StopTimeUpdate {
            stop_sequence: Some(1),
            schedule_relationship: Some(
                proto::trip_update::stop_time_update::ScheduleRelationship::Skipped as i32,
            ),
            ..proto::trip_update::StopTimeUpdate::default()
        }],
        ..proto::TripUpdate::default()
    });
    ingest(&state, FEED, FeedKind::TripUpdates, vec![skip]);
    let error = try_lookup(&state, &ride_request(FEED, ("A", "C"), "R1-0900")).unwrap_err();
    assert!(
        matches!(error, PlanError::NotRunning(NotRunning::Skipped)),
        "{error:?}"
    );
    // From `B` it still runs.
    let from_b = lookup(&state, &ride_request(FEED, ("B", "C"), "R1-0900"));
    assert_eq!(stop_ids(&from_b), ["B", "C"]);
}

#[test]
fn a_lookup_names_what_isnt_there() {
    let state = main_state();
    let missing = |request: &RideRequest| match try_lookup(&state, request) {
        Err(PlanError::NotFound(missing)) => Some(missing),
        _ => None,
    };
    // Route 2 doesn't run from `A` to `C`, and no trip runs from `C` back to `A`.
    assert_eq!(
        missing(&ride_request(FEED, ("A", "C"), "R2-0815")),
        Some(Missing::Trip)
    );
    assert_eq!(
        missing(&ride_request(FEED, ("C", "A"), "R1-0800")),
        Some(Missing::Trip)
    );
    // A weekday trip on a Saturday.
    let mut saturday = ride_request(FEED, ("A", "C"), "R1-0800");
    saturday.start_date = "20260613".to_owned();
    assert_eq!(missing(&saturday), Some(Missing::Trip));
    assert_eq!(
        missing(&ride_request(FEED, ("A", "Z"), "R1-0800")),
        Some(Missing::Stop)
    );
    assert!(matches!(
        try_lookup(&state, &ride_request("other", ("A", "C"), "R1-0800")),
        Err(PlanError::State(StateError::UnknownFeed(_)))
    ));
    let mut undated = ride_request(FEED, ("A", "C"), "R1-0800");
    undated.start_date = "2026-06-15".to_owned();
    assert!(matches!(
        try_lookup(&state, &undated),
        Err(PlanError::InvalidRequest(_))
    ));
    // A date the timetable doesn't cover at all.
    undated.start_date = "20300101".to_owned();
    assert!(matches!(
        try_lookup(&state, &undated),
        Err(PlanError::State(StateError::ServiceDateNotCovered(_)))
    ));
}

#[test]
fn a_wheelchair_lookup_reads_the_rules_a_wheelchair_plan_does() {
    let state = main_state();
    // The 09:30 is marked inaccessible, so a wheelchair ride's list doesn't have it.
    let mut request = ride_request(FEED, ("A", "C"), "R1-0930");
    assert_eq!(lookup(&state, &request).departure, june_15(9, 30));
    request.mode = TravelMode::Wheelchair;
    assert!(matches!(
        try_lookup(&state, &request),
        Err(PlanError::NotFound(Missing::Trip))
    ));
}

#[test]
fn a_lookup_on_a_date_outside_the_current_window_pins_one_of_its_own() {
    let state = main_state();
    let current = state.pin(FEED_TIME + 15).expect("pin");
    let mut friday = ride_request(FEED, ("A", "C"), "R1-0800");
    friday.start_date = "20260619".to_owned();
    let leg = lookup(&state, &friday);
    assert_eq!(leg.departure, june_15(4 * 24 + 8, 0));
    assert_eq!(leg.trip.start_date.as_deref(), Some("20260619"));
    // The current snapshot stays as it was.
    let after = state.pin(FEED_TIME + 15).expect("pin");
    assert!(std::sync::Arc::ptr_eq(&current, &after));
}

// A departures lookup lists what a plan's ride would, around a time rather than a leg.

fn between(feed: &str, (board, alight): (&str, &str), around: i64) -> DeparturesRequest {
    DeparturesRequest {
        feed: feed.to_owned(),
        board_stop_id: board.to_owned(),
        alight_stop_id: alight.to_owned(),
        around,
        range: DepartureRange::default(),
        ready: None,
        mode: TravelMode::Walking,
        now: FEED_TIME + 15,
    }
}

fn counted(mut request: DeparturesRequest, before: u8, after: u8) -> DeparturesRequest {
    request.range.before = before;
    request.range.after = after;
    request
}

fn try_list(
    state: &TransitState,
    request: &DeparturesRequest,
) -> Result<DeparturesResponse, LookupError> {
    departures(state, request, &|| false)
}

fn list(state: &TransitState, request: &DeparturesRequest) -> DeparturesResponse {
    try_list(state, request).expect("the lookup succeeds")
}

fn listed_rows(response: &DeparturesResponse) -> Vec<Row> {
    response.departures.iter().map(row).collect()
}

fn listed_trips(response: &DeparturesResponse) -> Vec<&str> {
    response
        .departures
        .iter()
        .map(|d| d.trip_id.as_str())
        .collect()
}

/// A departure apart from any list: its alerts themselves, by ID, rather than indices.
fn with_alerts(
    mut departure: RideDeparture,
    mut alerts: Vec<AlertResponse>,
) -> (RideDeparture, Vec<AlertResponse>) {
    departure.alerts.clear();
    departure.departure_alerts.clear();
    alerts.sort_by(|a, b| a.id.cmp(&b.id));
    (departure, alerts)
}

/// A leg's departures as a lookup states them: with what they left for the leg to say,
/// and without what only a plan knows, which is the journey's own and what the rider misses.
fn stated_by_leg(leg: &TransitLeg) -> Vec<(RideDeparture, Vec<AlertResponse>)> {
    let leg_headsign = leg
        .stops
        .first()
        .and_then(|visit| visit.headsign.clone())
        .or_else(|| leg.trip.headsign.clone());
    leg.departures
        .iter()
        .map(|departure| {
            let alerts = departure
                .alerts
                .iter()
                .map(|&index| leg.alerts[index].alert.clone())
                .chain(
                    departure
                        .departure_alerts
                        .iter()
                        .map(|&index| leg.departure_alerts[index].clone()),
                )
                .collect();
            let stated = RideDeparture {
                start_date: departure
                    .start_date
                    .clone()
                    .or_else(|| leg.trip.start_date.clone()),
                headsign: departure.headsign.clone().or_else(|| leg_headsign.clone()),
                route: departure.route.clone().or_else(|| leg.route.clone()),
                suggested: false,
                missed: false,
                ..departure.clone()
            };
            with_alerts(stated, alerts)
        })
        .collect()
}

fn stated_by_lookup(response: &DeparturesResponse) -> Vec<(RideDeparture, Vec<AlertResponse>)> {
    response
        .departures
        .iter()
        .map(|departure| {
            assert!(
                departure.departure_alerts.is_empty(),
                "a lookup has one list of alerts"
            );
            let alerts = departure
                .alerts
                .iter()
                .map(|&index| response.alerts[index].clone())
                .collect();
            with_alerts(departure.clone(), alerts)
        })
        .collect()
}

/// Real-time data that touches every part of a departure: a delay, a cancellation,
/// and alerts on a route and on one trip.
fn ingest_everything(state: &TransitState) {
    ingest(
        state,
        FEED,
        FeedKind::TripUpdates,
        vec![
            delay_update("R1-0800", JUNE_15, 1, 120),
            canceled("R1-0900"),
        ],
    );
    ingest(
        state,
        FEED,
        FeedKind::Alerts,
        vec![
            alert_entity(
                "route-1",
                proto::alert::Effect::Detour,
                vec![route_selector("R1")],
            ),
            alert_entity(
                "trip-0930",
                proto::alert::Effect::ReducedService,
                vec![proto::EntitySelector {
                    trip: Some(descriptor("R1-0930", JUNE_15, None)),
                    ..proto::EntitySelector::default()
                }],
            ),
        ],
    );
}

#[test]
fn a_departures_lookup_around_a_legs_departure_agrees_with_the_legs_own_list() {
    let state = main_state();
    ingest_everything(&state);
    let response = run(&state, &request(near_a(), near_c(), june_15(7, 55), 1800));
    let leg = leg_on(&response, "R1-0800");
    // The 07:00 leaves more than an hour before the late 08:00 does.
    assert_eq!(
        trips_of(leg),
        vec!["R1-0730", "R1-0800", "R1-0830", "R1-0900", "R1-0930", "F1"],
    );
    // A leg lists two before its own, its own, and six after; a lookup counts the departure
    // at `around` among those after it, so one more is asked for.
    let looked_up = list(
        &state,
        &counted(between(FEED, ("A", "C"), leg.departure), 2, 7),
    );
    assert_eq!(stated_by_lookup(&looked_up), stated_by_leg(leg));
    assert_eq!(looked_up.generated_at, response.generated_at);
    assert_eq!(looked_up.realtime, response.realtime);
    // Every departure says what a leg's leaves to the leg, and none is anyone's own.
    for departure in &looked_up.departures {
        assert_eq!(departure.start_date.as_deref(), Some(JUNE_15));
        assert!(departure.headsign.is_some(), "{}", departure.trip_id);
        assert!(departure.route.is_some(), "{}", departure.trip_id);
        assert!(!departure.suggested && !departure.missed);
    }
    // Each alert is listed once, and the departures index the one list.
    let ids: Vec<&str> = looked_up.alerts.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["route-1", "trip-0930"]);
    let indices: Vec<&[usize]> = looked_up
        .departures
        .iter()
        .map(|d| d.alerts.as_slice())
        .collect();
    let (route, both, none): (&[usize], &[usize], &[usize]) = (&[0], &[0, 1], &[]);
    assert_eq!(indices, [route, route, route, route, both, none]);

    // The same holds of a ride after a change, on another route.
    let response = run(&state, &request(near_a(), near_d(), june_15(7, 55), 1800));
    let second = transit_legs(&response)
        .find(|leg| leg.trip.trip_id.starts_with("R2"))
        .expect("a ride on route 2");
    let looked_up = list(
        &state,
        &counted(between(FEED, ("B", "D"), second.departure), 2, 7),
    );
    assert_eq!(stated_by_lookup(&looked_up), stated_by_leg(second));
}

#[test]
fn a_departures_lookups_json_is_a_legs_departures_with_everything_stated() {
    let state = main_state();
    ingest_everything(&state);
    let mut request = counted(between(FEED, ("A", "C"), june_15(8, 30)), 1, 3);
    request.ready = Some(june_15(8, 5));
    let response = list(&state, &request);
    assert_json_snapshot!("departures_between_response", &response);

    // Nothing in the range is still a list, and no alerts are no field.
    let empty = list(&state, &between(FEED, ("A", "C"), june_15(15, 0)));
    let json = serde_json::to_value(&empty).expect("serializes");
    assert_eq!(json["departures"], serde_json::json!([]));
    assert!(json.get("alerts").is_none());
}

#[test]
fn a_departures_lookup_counts_before_around_and_from_it_on() {
    let state = main_state();
    let around = |hour, minute, before, after| {
        list(
            &state,
            &counted(
                between(FEED, ("A", "C"), june_15(hour, minute)),
                before,
                after,
            ),
        )
    };
    // A departure leaving exactly at `around` is the first of those after it.
    assert_eq!(
        listed_trips(&around(8, 0, 1, 2)),
        ["R1-0730", "R1-0800", "R1-0830"]
    );
    assert_eq!(listed_trips(&around(8, 0, 0, 1)), ["R1-0800"]);
    assert_eq!(listed_trips(&around(8, 0, 1, 0)), ["R1-0730"]);
    // A second later, it is the last of those before.
    let mut late = counted(between(FEED, ("A", "C"), june_15(8, 0) + 1), 1, 2);
    assert_eq!(
        listed_trips(&list(&state, &late)),
        ["R1-0800", "R1-0830", "R1-0900"]
    );
    late.range.after = 0;
    assert_eq!(listed_trips(&list(&state, &late)), ["R1-0800"]);
    // Between departures, those nearest on each side, in order of departure.
    assert_eq!(
        listed_trips(&around(8, 10, 2, 2)),
        ["R1-0730", "R1-0800", "R1-0830", "R1-0900"]
    );
    // The defaults are a plan's: two and six.
    assert_eq!(
        listed_trips(&list(&state, &between(FEED, ("A", "C"), june_15(8, 0)))),
        [
            "R1-0700", "R1-0730", "R1-0800", "R1-0830", "R1-0900", "R1-0930", "F1"
        ],
        "the sixth after would leave more than two hours on"
    );
}

#[test]
fn a_departures_lookup_is_bounded_in_time_as_a_plans_range_is() {
    let state = main_state();
    let all = |hour, minute| {
        list(
            &state,
            &counted(between(FEED, ("A", "C"), june_15(hour, minute)), 10, 10),
        )
    };
    // An hour before and two hours after, to the second: the 08:00 leaves an hour before
    // 09:00, and route 3's last run, at 10:40, two hours after 08:40.
    assert_eq!(
        listed_trips(&all(9, 0)),
        ["R1-0800", "R1-0830", "R1-0900", "R1-0930", "F1", "F1", "F1"]
    );
    let a_second_later = list(
        &state,
        &counted(between(FEED, ("A", "C"), june_15(9, 0) + 1), 10, 10),
    );
    assert_eq!(
        listed_trips(&a_second_later),
        ["R1-0830", "R1-0900", "R1-0930", "F1", "F1", "F1"]
    );
    assert_eq!(
        listed_trips(&all(8, 40)),
        ["R1-0800", "R1-0830", "R1-0900", "R1-0930", "F1", "F1", "F1"]
    );
    let a_second_earlier = list(
        &state,
        &counted(between(FEED, ("A", "C"), june_15(8, 40) - 1), 10, 10),
    );
    assert_eq!(
        listed_trips(&a_second_earlier),
        ["R1-0800", "R1-0830", "R1-0900", "R1-0930", "F1", "F1"]
    );
    // The bounds a request's range carries are the ones read.
    let mut narrow = counted(between(FEED, ("A", "C"), june_15(8, 45)), 10, 10);
    narrow.range.before_seconds = 900;
    narrow.range.after_seconds = 900;
    assert_eq!(listed_trips(&list(&state, &narrow)), ["R1-0830", "R1-0900"]);
}

#[test]
fn a_departures_lookup_that_can_list_nothing_or_too_much_is_refused() {
    let state = main_state();
    let refused = |before, after| {
        matches!(
            try_list(
                &state,
                &counted(between(FEED, ("A", "C"), june_15(8, 0)), before, after)
            ),
            Err(PlanError::InvalidRequest(_))
        )
    };
    assert!(refused(0, 0), "nothing on either side");
    assert!(refused(11, 1));
    assert!(refused(1, 11));
    assert!(!refused(10, 10));
    let mut far = between(FEED, ("A", "C"), june_15(8, 0));
    far.range.after_seconds = 24 * 3600 + 1;
    assert!(matches!(
        try_list(&state, &far),
        Err(PlanError::InvalidRequest(_))
    ));
}

#[test]
fn a_departures_lookup_marks_what_leaves_before_the_rider_is_ready() {
    let state = main_state();
    ingest(
        &state,
        FEED,
        FeedKind::TripUpdates,
        vec![
            delay_update("R1-0800", JUNE_15, 1, 120),
            canceled("R1-0730"),
        ],
    );
    let mut request = counted(between(FEED, ("A", "C"), june_15(8, 0)), 2, 2);
    let unmarked = [
        expected("R1-0700", june_15(7, 0), SCHEDULED, ""),
        expected("R1-0730", june_15(7, 30), DepartureStatus::Canceled, ""),
        expected("R1-0800", june_15(8, 2), DepartureStatus::Live, ""),
        expected("R1-0830", june_15(8, 30), SCHEDULED, ""),
    ];
    assert_eq!(
        listed_rows(&list(&state, &request)),
        unmarked,
        "nothing says when the rider is ready"
    );
    // Ready at 08:02, when the late 08:00 leaves: it isn't missed, as routing would board it.
    request.ready = Some(june_15(8, 2));
    assert_eq!(
        listed_rows(&list(&state, &request)),
        [
            expected("R1-0700", june_15(7, 0), SCHEDULED, "M"),
            expected("R1-0730", june_15(7, 30), DepartureStatus::Canceled, "M"),
            expected("R1-0800", june_15(8, 2), DepartureStatus::Live, ""),
            expected("R1-0830", june_15(8, 30), SCHEDULED, ""),
        ]
    );
    // A second later it is; `ready` needn't be near `around`.
    request.ready = Some(june_15(8, 2) + 1);
    let flags: Vec<String> = listed_rows(&list(&state, &request))
        .into_iter()
        .map(|row| row.flags)
        .collect();
    assert_eq!(flags, ["M", "M", "M", ""]);
    request.ready = Some(june_15(6, 0));
    assert_eq!(listed_rows(&list(&state, &request)), unmarked);
}

#[test]
fn a_departures_lookup_marks_the_last_of_a_service_day_as_a_plan_does() {
    let state = main_state();
    let request = between(FEED, ("A", "C"), june_15(23, 50));
    let response = list(&state, &request);
    assert_eq!(
        listed_rows(&response),
        [
            expected("R1-2300", june_15(23, 0), SCHEDULED, ""),
            expected("R1-2350", june_15(23, 50), SCHEDULED, ""),
            // 00:20 on June 16, but on June 15's service day, whose last trip it is.
            expected("R1-2420", june_15(24, 20), SCHEDULED, "L"),
            expected("R1-0050", june_15(24, 50), SCHEDULED, ""),
        ]
    );
    let dates: Vec<Option<&str>> = response
        .departures
        .iter()
        .map(|d| d.start_date.as_deref())
        .collect();
    assert_eq!(
        dates,
        [
            Some(JUNE_15),
            Some(JUNE_15),
            Some(JUNE_15),
            Some("20260616")
        ],
        "each states its own service date"
    );
    // The day's last is known even when it is the only departure listed.
    let only = list(
        &state,
        &counted(between(FEED, ("A", "C"), june_15(24, 20)), 0, 1),
    );
    assert_eq!(
        listed_rows(&only),
        [expected("R1-2420", june_15(24, 20), SCHEDULED, "L")]
    );

    // With 00:20 canceled, 23:50 is the day's last, and the canceled one isn't.
    ingest(
        &state,
        FEED,
        FeedKind::TripUpdates,
        vec![canceled("R1-2420")],
    );
    let response = list(&state, &request);
    assert_eq!(
        listed_rows(&response),
        [
            expected("R1-2300", june_15(23, 0), SCHEDULED, ""),
            expected("R1-2350", june_15(23, 50), SCHEDULED, "L"),
            expected("R1-2420", june_15(24, 20), DepartureStatus::Canceled, ""),
            expected("R1-0050", june_15(24, 50), SCHEDULED, ""),
        ]
    );
    let canceled = &response.departures[2];
    assert_eq!(canceled.scheduled_departure, Some(june_15(24, 20)));
    assert!(!canceled.realtime);
    assert_eq!(canceled.delay_seconds, None);
}

#[test]
fn a_departures_lookup_lists_skipped_and_delayed_departures_as_real_time_data_has_them() {
    let state = main_state();
    let mut skip = entity("tu-skip");
    skip.trip_update = Some(proto::TripUpdate {
        trip: descriptor("R1-0900", JUNE_15, None),
        stop_time_update: vec![proto::trip_update::StopTimeUpdate {
            stop_sequence: Some(1),
            schedule_relationship: Some(
                proto::trip_update::stop_time_update::ScheduleRelationship::Skipped as i32,
            ),
            ..proto::trip_update::StopTimeUpdate::default()
        }],
        ..proto::TripUpdate::default()
    });
    ingest(
        &state,
        FEED,
        FeedKind::TripUpdates,
        vec![skip, delay_update("R1-0830", JUNE_15, 1, 300)],
    );
    // 08:32 is after the 08:30's schedule and before its prediction, which is what counts.
    let response = list(
        &state,
        &counted(between(FEED, ("A", "C"), june_15(8, 32)), 1, 3),
    );
    assert_eq!(
        listed_rows(&response),
        [
            expected("R1-0800", june_15(8, 0), SCHEDULED, ""),
            expected("R1-0830", june_15(8, 35), DepartureStatus::Live, ""),
            // Shown at its schedule.
            expected("R1-0900", june_15(9, 0), DepartureStatus::Skipped, ""),
            expected("R1-0930", june_15(9, 30), SCHEDULED, ""),
        ]
    );
    let live = &response.departures[1];
    assert!(live.realtime);
    assert_eq!(live.delay_seconds, Some(300));
    assert_eq!(live.scheduled_departure, Some(june_15(8, 30)));
    assert_eq!(live.headsign.as_deref(), Some("Charlie Express"));
    // From `B`, where it still stops, the 09:00 runs.
    let from_b = list(
        &state,
        &counted(between(FEED, ("B", "C"), june_15(9, 10)), 0, 1),
    );
    assert_eq!(
        listed_rows(&from_b),
        [expected("R1-0900", june_15(9, 10), SCHEDULED, "")]
    );
}

#[test]
fn a_departures_lookup_reads_the_rules_of_its_mode() {
    let state = main_state();
    let mut request = between(FEED, ("A", "C"), june_15(9, 0));
    assert_eq!(
        listed_trips(&list(&state, &request)),
        ["R1-0800", "R1-0830", "R1-0900", "R1-0930", "F1", "F1", "F1"]
    );
    // The 09:30 is marked inaccessible.
    request.mode = TravelMode::Wheelchair;
    let wheelchair = list(&state, &request);
    assert_eq!(
        listed_trips(&wheelchair),
        ["R1-0800", "R1-0830", "R1-0900", "F1", "F1", "F1"]
    );
    assert!(
        wheelchair
            .departures
            .iter()
            .all(|d| d.accessibility == Accessibility::Accessible)
    );
    // Route 3's trip doesn't say it takes bikes.
    request.mode = TravelMode::Cycling;
    let cycling = list(&state, &request);
    assert_eq!(
        listed_trips(&cycling),
        ["R1-0800", "R1-0830", "R1-0900", "R1-0930"]
    );
    assert!(
        cycling
            .departures
            .iter()
            .all(|d| d.bikes_allowed == Some("yes"))
    );
}

#[test]
fn a_departures_lookup_lists_each_run_of_a_frequency_trip() {
    let state = main_state();
    let response = list(&state, &between(FEED, ("A", "C"), june_15(10, 0)));
    let runs: Vec<(&str, i64, Option<&str>)> = response
        .departures
        .iter()
        .map(|d| {
            (
                d.trip_id.as_str(),
                d.departure,
                d.route.as_ref().and_then(|r| r.short_name.as_deref()),
            )
        })
        .collect();
    assert_eq!(
        runs,
        [
            ("R1-0900", june_15(9, 0), Some("1")),
            ("R1-0930", june_15(9, 30), Some("1")),
            ("F1", june_15(10, 0), Some("3")),
            ("F1", june_15(10, 20), Some("3")),
            ("F1", june_15(10, 40), Some("3")),
        ]
    );
    assert!(
        response.departures.iter().all(|d| !d.last_of_day),
        "route 1 runs until after midnight"
    );
    // Around a run, the runs on each side of it.
    let middle = list(
        &state,
        &counted(between(FEED, ("A", "C"), june_15(10, 30)), 1, 1),
    );
    let times: Vec<i64> = middle.departures.iter().map(|d| d.departure).collect();
    assert_eq!(times, [june_15(10, 20), june_15(10, 40)]);
}

#[test]
fn a_departures_lookup_lists_stay_seated_trips_by_the_route_they_board() {
    let state = seated_state();
    let response = list(&state, &between("seated", ("ORG", "DST"), june_15(8, 30)));
    // `T3` and `T4` don't make the ride without a change; `T1` into `T2` and `T5` into `T6`
    // do, and so does `T7` on route 3.
    let boarded: Vec<(&str, Option<&str>, Option<&str>)> = response
        .departures
        .iter()
        .map(|d| {
            (
                d.trip_id.as_str(),
                d.route.as_ref().map(|r| r.route_id.as_str()),
                d.headsign.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        boarded,
        [
            ("T1", Some("R1"), Some("To Middle")),
            ("T7", Some("R3"), Some("To Destination")),
            ("T5", Some("R1"), Some("To Middle")),
        ],
        "a seated ride is on the route it boards, under the headsign it boards with"
    );
    let times: Vec<(i64, i64, bool)> = response
        .departures
        .iter()
        .map(|d| (d.departure, d.arrival, d.last_of_day))
        .collect();
    assert_eq!(
        times,
        [
            // Where `T2` reaches the destination.
            (june_15(8, 5), june_15(8, 50), false),
            (june_15(8, 40), june_15(9, 15), false),
            (june_15(9, 5), june_15(9, 50), true),
        ]
    );
    // It is what the plan's seated ride lists.
    let plan = run(
        &state,
        &request(at(47.5990), at(47.6410), june_15(8, 0), 3600),
    );
    let seated = transit_legs(&plan)
        .find(|leg| leg.trip.trip_id == "T1" && !leg.segments.is_empty())
        .expect("the ride stays seated from T1 into T2");
    let around_it = list(
        &state,
        &counted(between("seated", ("ORG", "DST"), seated.departure), 2, 7),
    );
    assert_eq!(stated_by_lookup(&around_it), stated_by_leg(seated));
    // Combined trips are listed once on a route they ride part of.
    let to_middle = list(&state, &between("seated", ("ORG", "MID"), june_15(8, 30)));
    assert_eq!(listed_trips(&to_middle), ["T1", "T3", "T5"]);
}

#[test]
fn a_departures_lookup_lists_every_route_between_the_two_stops() {
    let state = connection_state();
    let plan = run(&state, &request(near_p(), near_q(), june_15(20, 55), 600));
    let leg = leg_on(&plan, "R7-2100");
    let looked_up = list(
        &state,
        &counted(between(CONNECTION, ("P", "Q"), leg.departure), 2, 7),
    );
    assert_eq!(stated_by_lookup(&looked_up), stated_by_leg(leg));
    let routes: Vec<Option<&str>> = looked_up
        .departures
        .iter()
        .map(|d| d.route.as_ref().and_then(|r| r.short_name.as_deref()))
        .collect();
    assert!(
        routes.contains(&Some("7")) && routes.contains(&Some("14")),
        "{routes:?}"
    );
}

#[test]
fn a_departures_lookup_names_what_isnt_there_and_lists_nothing_where_nothing_runs() {
    let state = main_state();
    let around = june_15(8, 0);
    assert!(matches!(
        try_list(&state, &between("other", ("A", "C"), around)),
        Err(PlanError::State(StateError::UnknownFeed(feed))) if feed == "other"
    ));
    for stops in [("A", "Z"), ("Z", "C"), ("Z", "Z")] {
        assert!(
            matches!(
                try_list(&state, &between(FEED, stops, around)),
                Err(PlanError::NotFound(Missing::Stop))
            ),
            "{stops:?}"
        );
    }
    // A moment no timetable covers at all.
    assert!(matches!(
        try_list(&state, &between(FEED, ("A", "C"), 1_893_456_000)),
        Err(PlanError::State(StateError::DateNotCovered(_)))
    ));
    assert!(matches!(
        try_list(&state, &between(FEED, ("A", "C"), i64::MAX)),
        Err(PlanError::State(StateError::DateNotCovered(_)))
    ));
    // Two stops nothing runs between, in that order, are a list of nothing: no trip runs from
    // `C` back to `A`, none from `A` to `D` without a change, and none from a stop to itself.
    for stops in [("C", "A"), ("A", "D"), ("A", "A")] {
        let response = list(&state, &between(FEED, stops, around));
        assert!(response.departures.is_empty(), "{stops:?}");
        assert!(response.alerts.is_empty(), "{stops:?}");
    }
    assert!(matches!(
        departures::<core::convert::Infallible, core::convert::Infallible>(
            &state,
            &between(FEED, ("A", "C"), around),
            &|| true
        ),
        Err(PlanError::Interrupted)
    ));
}

#[test]
fn a_departures_lookup_on_a_date_outside_the_current_window_pins_one_of_its_own() {
    let state = main_state();
    let current = state.pin(FEED_TIME + 15).expect("pin");
    let friday = june_15(4 * 24 + 8, 0);
    let response = list(&state, &counted(between(FEED, ("A", "C"), friday), 1, 1));
    assert_eq!(
        listed_rows(&response),
        [
            expected("R1-0730", friday - 1800, SCHEDULED, ""),
            expected("R1-0800", friday, SCHEDULED, ""),
        ]
    );
    assert_eq!(
        response.departures[1].start_date.as_deref(),
        Some("20260619")
    );
    // The current snapshot stays as it was.
    let after = state.pin(FEED_TIME + 15).expect("pin");
    assert!(std::sync::Arc::ptr_eq(&current, &after));
}

#[test]
fn json_departures_lookups_set_the_range_and_when_the_rider_is_ready() {
    let parsed = crate::json::parse_departures(
        br#"{"feed":"metro","board_stop_id":"A","alight_stop_id":"C","around":1781535600}"#,
        FEED_TIME,
    )
    .expect("parses");
    assert_eq!(parsed.range, DepartureRange::default());
    assert_eq!(
        (parsed.around, parsed.ready, parsed.mode, parsed.now),
        (1_781_535_600, None, TravelMode::Walking, FEED_TIME)
    );
    let parsed = crate::json::parse_departures(
        br#"{"feed":"metro","board_stop_id":"A","alight_stop_id":"C","around":1781535600,
            "departures_before":0,"departures_after":10,"ready":1781535700,"mode":"wheelchair"}"#,
        FEED_TIME,
    )
    .expect("parses");
    assert_eq!(
        parsed.range,
        DepartureRange {
            before: 0,
            after: 10,
            ..DepartureRange::default()
        }
    );
    assert_eq!(
        (parsed.ready, parsed.mode),
        (Some(1_781_535_700), TravelMode::Wheelchair)
    );
    // `around` is required, and a mode must be one there is.
    for body in [
        r#"{"feed":"metro","board_stop_id":"A","alight_stop_id":"C"}"#,
        r#"{"feed":"metro","board_stop_id":"A","alight_stop_id":"C","around":"now"}"#,
        r#"{"feed":"metro","board_stop_id":"A","alight_stop_id":"C","around":1,"mode":"flying"}"#,
        r#"{"feed":"metro","board_stop_id":"A","alight_stop_id":"C","around":1,"departures_after":-1}"#,
        "not json",
    ] {
        let refused =
            crate::json::parse_departures(body.as_bytes(), FEED_TIME).expect_err("doesn't parse");
        assert_eq!(refused.status, 400, "{body}");
    }
}
