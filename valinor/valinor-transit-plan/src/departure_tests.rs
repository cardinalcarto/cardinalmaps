//! Departure ranges on ride legs, on synthetic feeds: one test per state a departure can be in.
//!
//! The main feed runs route 1 north from `A` through `B` to `C` every half hour in the morning,
//! late in the evening, and across midnight; route 2 east from `B` to `D`; and route 3 from `A`
//! to `C` as a frequency trip. Stops are farther apart than the access radius,
//! so each journey boards where the test expects.

use core::fmt::Write as _;

use valinor_gtfs::{FeedId as CompileFeedId, FeedInput, InMemorySource, LatLon, LoadedTimetable};
use valinor_gtfs::{TimetableSet, compile};
use valinor_gtfs_rt::{FeedKind, proto};
use valinor_transit::Router;

use crate::error::PlanError;
use crate::plan::{DepartureRange, TransitOptions, TransitRequest, TravelMode, plan};
use crate::response::{
    Accessibility, DepartureStatus, JourneyLeg, PlanResponse, RideDeparture, TransitLeg,
};
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

#[test]
fn a_ride_lists_its_route_around_its_own_departure() {
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
        ],
        "route 3's frequency trip from A to C is another route, and isn't listed"
    );
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
            .filter(|d| d.trip_id != "R1-0830")
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
        ]
    );
    let extra = &leg.departures[3];
    assert_eq!(
        extra.scheduled_departure, None,
        "an added trip has no schedule"
    );
    assert_eq!(extra.arrival, june_15(8, 30));
}

#[test]
fn a_trip_no_route_names_lists_only_itself() {
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
        vec![expected(
            "LONER",
            june_15(8, 10),
            DepartureStatus::Live,
            "S"
        )],
        "no route to list others of, or to call this the last of"
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
            (june_15(10, 0), true),
            (june_15(10, 20), false),
            (june_15(10, 40), false),
        ],
        "route 1's 09:30 is another route"
    );
    assert!(leg.departures.iter().all(|d| d.trip_id == "F1"));
    assert!(leg.departures[2].last_of_day);
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
        vec!["R1-0700", "R1-0730", "R1-0800", "R1-0830", "R1-0900"],
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
    for departure in &leg.departures {
        assert_eq!(departure.alerts, vec![0], "{}", departure.trip_id);
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
// on block `X`, and `T5` as `T6` on block `Y`; `T3` (route 1) and `T4` (route 2) run alone.

const SEATED_STOPS: &str = "stop_id,stop_name,stop_lat,stop_lon\n\
ORG,Origin,47.6000,-122.3300\n\
MID,Middle,47.6200,-122.3300\n\
DST,Destination,47.6400,-122.3300\n";

const SEATED_TRIPS: &str = "route_id,service_id,trip_id,block_id,trip_headsign\n\
R1,WK,T1,X,To Middle\nR2,WK,T2,X,To Destination\n\
R1,WK,T3,,To Middle\nR2,WK,T4,,To Destination\n\
R1,WK,T5,Y,To Middle\nR2,WK,T6,Y,To Destination\n";

const SEATED_TIMES: &str = "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
T1,08:05:00,08:05:00,ORG,1\nT1,08:20:00,08:20:00,MID,2\n\
T2,08:25:00,08:25:00,MID,1\nT2,08:50:00,08:50:00,DST,2\n\
T3,08:35:00,08:35:00,ORG,1\nT3,08:50:00,08:50:00,MID,2\n\
T4,09:00:00,09:00:00,MID,1\nT4,09:25:00,09:25:00,DST,2\n\
T5,09:05:00,09:05:00,ORG,1\nT5,09:20:00,09:20:00,MID,2\n\
T6,09:25:00,09:25:00,MID,1\nT6,09:50:00,09:50:00,DST,2\n";

fn seated_state() -> TransitState {
    let source = InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\n\
             A,Metro,America/Los_Angeles,https://m.test\n",
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_type\nR1,A,1,3\nR2,A,2,3\n",
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
    // `T3` and `T4` don't make the ride without a change; `T5` into `T6` does.
    assert_eq!(trips_of(seated), vec!["T1", "T5"]);
    assert_eq!(seated.departures[1].departure, june_15(9, 5));
    assert_eq!(seated.departures[1].arrival, june_15(9, 50));
    assert!(seated.departures[1].last_of_day);
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
