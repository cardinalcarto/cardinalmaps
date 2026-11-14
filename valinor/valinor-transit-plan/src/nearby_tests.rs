//! Nearby departures and route detail on synthetic feeds.
//!
//! The rider stands at [`HOME`]. Within the straight-line access radius are `N1` (42 s away),
//! `N2` (335 s), `E1` (57 s), `N3` (42 s, marked inaccessible), and the ferry's `DOCK`; every
//! other stop is too far to walk to. Times are on Monday 2026-06-15, in Seattle's time zone.
//!
//! - Route 1 runs north (`direction_id` 0) through `N1` then `N2`, and south (1) through `N2`
//!   then `N1`, every ten minutes.
//! - Route 2 has no `direction_id`: east from `N1` through `E1` to `EX`, west from `EX` through
//!   `E1` and `N1` to `WX`, and one short trip from `N1` that ends at `E1`.
//! - Route 3 is a frequency trip from `N2`; route 4's trip is marked inaccessible; route 5 only
//!   serves `N3`; route 6 runs late in the evening and past midnight; route 7 runs at 10:30.
//! - The ferry feed's one route leaves `DOCK` at 08:15 and 09:15.

use core::convert::Infallible;
use core::fmt::Write as _;

use insta::assert_json_snapshot;
use valinor_gtfs::{FeedId as CompileFeedId, FeedInput, InMemorySource, LatLon, LoadedTimetable};
use valinor_gtfs::{TimetableSet, compile};
use valinor_gtfs_rt::{FeedKind, proto};

use crate::error::{Missing, PlanError};
use crate::nearby::{DirectionKey, NearbyRequest, RouteDetailRequest, TripChoice, nearby};
use crate::plan::TravelMode;
use crate::response::{
    Accessibility, DepartureStatus, NearbyDirection, NearbyResponse, NearbyRoute, NoRoutesReason,
    RouteDetailResponse,
};
use crate::state::{FeedUpdate, StateConfig, TransitState};
use crate::testing::{
    StraightLineAccess, delay_update, descriptor, encoded_feed, entity, june_15, vehicle_entity,
};

const FEED: &str = "metro";
const FERRY: &str = "ferry";
/// 2026-06-15 07:50 PDT.
const FEED_TIME: i64 = 1_781_535_000;
const JUNE_15: &str = "20260615";

const HOME: LatLon = LatLon {
    lat: 47.6000,
    lon: -122.3300,
};

/// The walk from [`HOME`] to `N1` or `N3`, 55.6 m at 1.33 m/s.
const WALK_N1: u32 = 42;
/// The walk to `N2`, 444.8 m.
const WALK_N2: u32 = 335;
/// The walk to `E1`, 75.0 m.
const WALK_E1: u32 = 57;

const STOPS: &str = "stop_id,stop_name,stop_lat,stop_lon,wheelchair_boarding\n\
N1,North 1st,47.6005,-122.3300,1\n\
N2,North 2nd,47.6040,-122.3300,1\n\
N3,Steps,47.5995,-122.3300,2\n\
NF,North Far,47.6300,-122.3300,1\n\
SF,South Far,47.5700,-122.3300,1\n\
E1,East 1st,47.6000,-122.3290,\n\
EX,East End,47.6000,-122.3000,\n\
WX,West End,47.6000,-122.3600,\n";

/// `HH:MM:SS` for seconds from midnight.
fn clock(seconds: u32) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn hm(hours: u32, minutes: u32) -> u32 {
    hours * 3600 + minutes * 60
}

/// One trip: `(route, trip_id, headsign, direction_id, wheelchair_accessible, shape_id)`
/// and its stops as `(stop, seconds from midnight, stop_headsign)`.
type Trip = (
    &'static str,
    String,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Vec<(&'static str, u32, &'static str)>,
);

#[expect(clippy::too_many_lines, reason = "one feed's trips, listed in full")]
fn metro_trips() -> Vec<Trip> {
    let mut trips = Vec::new();
    for minute in [0, 10, 20, 30] {
        let n1 = hm(8, minute) + 30;
        // The 08:20's headsign at `N1` differs from its trip's.
        let at_n1 = if minute == 20 {
            "Northgate via 2nd"
        } else {
            ""
        };
        trips.push((
            "R1",
            format!("R1N-08{minute:02}"),
            "Northgate",
            "0",
            "1",
            "SH1",
            vec![
                ("SF", n1 - 300, ""),
                ("N1", n1, at_n1),
                ("N2", n1 + 90, ""),
                ("NF", n1 + 600, ""),
            ],
        ));
        let n2 = hm(8, minute + 6);
        trips.push((
            "R1",
            format!("R1S-08{minute:02}"),
            "Southcenter",
            "1",
            "1",
            "",
            vec![
                ("NF", n2 - 300, ""),
                ("N2", n2, ""),
                ("N1", n2 + 180, ""),
                ("SF", n2 + 540, ""),
            ],
        ));
    }
    for minute in [3, 13] {
        let n1 = hm(8, minute);
        trips.push((
            "R2",
            format!("R2E-08{minute:02}"),
            "East End",
            "",
            "",
            "",
            vec![("N1", n1, ""), ("E1", n1 + 120, ""), ("EX", n1 + 420, "")],
        ));
    }
    for minute in [5, 25] {
        let e1 = hm(8, minute);
        trips.push((
            "R2",
            format!("R2W-08{minute:02}"),
            "West End",
            "",
            "",
            "",
            vec![
                ("EX", e1 - 300, ""),
                ("E1", e1, ""),
                ("N1", e1 + 120, ""),
                ("WX", e1 + 600, ""),
            ],
        ));
    }
    trips.push((
        "R2",
        "R2S-0820".to_owned(),
        "East 1st",
        "",
        "",
        "",
        vec![("N1", hm(8, 20), ""), ("E1", hm(8, 22), "")],
    ));
    trips.push((
        "R3",
        "F1".to_owned(),
        "Frequent",
        "0",
        "",
        "",
        vec![("N2", hm(8, 0), ""), ("NF", hm(8, 10), "")],
    ));
    trips.push((
        "R4",
        "R4-0801".to_owned(),
        "No Steps",
        "0",
        "2",
        "",
        vec![("N1", hm(8, 1) + 30, ""), ("NF", hm(8, 10), "")],
    ));
    trips.push((
        "R5",
        "R5-0802".to_owned(),
        "Up the Steps",
        "0",
        "1",
        "",
        vec![("N3", hm(8, 2), ""), ("NF", hm(8, 12), "")],
    ));
    for (id, n1) in [
        ("R6-2350", hm(23, 50)),
        ("R6-2410", hm(24, 10)),
        ("R6-0040", hm(0, 40)),
    ] {
        trips.push((
            "R6",
            id.to_owned(),
            "Night",
            "0",
            "",
            "",
            vec![("N1", n1, ""), ("NF", n1 + 600, "")],
        ));
    }
    trips.push((
        "R7",
        "R7-1030".to_owned(),
        "Late Morning",
        "0",
        "",
        "",
        vec![("N1", hm(10, 30), ""), ("NF", hm(10, 40), "")],
    ));
    trips
}

fn metro_source() -> InMemorySource {
    let mut trips = String::from(
        "route_id,service_id,trip_id,trip_headsign,direction_id,wheelchair_accessible,shape_id\n",
    );
    let mut times =
        String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence,stop_headsign\n");
    for (route, id, headsign, direction, wheelchair, shape, stops) in metro_trips() {
        writeln!(
            trips,
            "{route},WK,{id},{headsign},{direction},{wheelchair},{shape}"
        )
        .expect("to a string");
        for (sequence, (stop, at, stop_headsign)) in stops.into_iter().enumerate() {
            let at = clock(at);
            writeln!(
                times,
                "{id},{at},{at},{stop},{},{stop_headsign}",
                (sequence + 1) * 10
            )
            .expect("to a string");
        }
    }
    InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\n\
             MT,Metro,America/Los_Angeles,https://m.test\n",
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_long_name,route_type,route_color,\
             route_text_color\n\
             R1,MT,1,First Avenue,3,FDB71A,000000\n\
             R2,MT,2,,3,,\n\
             R3,MT,3,,3,,\n\
             R4,MT,4,,3,,\n\
             R5,MT,5,,3,,\n\
             R6,MT,6,,3,,\n\
             R7,MT,7,,3,,\n",
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
             F1,08:00:00,10:00:00,1200,1\n",
        )
        .with(
            "shapes.txt",
            "shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\n\
             SH1,47.5700,-122.3300,1\n\
             SH1,47.6005,-122.3300,2\n\
             SH1,47.6020,-122.3302,3\n\
             SH1,47.6040,-122.3300,4\n\
             SH1,47.6300,-122.3300,5\n",
        )
}

fn ferry_source() -> InMemorySource {
    InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\n\
             FC,Ferry Co,America/Los_Angeles,https://f.test\n",
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_long_name,route_type\n\
             FY,FC,,Dock Ferry,4\n",
        )
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon\n\
             DOCK,Dock,47.6001,-122.3305\n\
             ISL,Island,47.6500,-122.4000\n",
        )
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,trip_headsign,direction_id\n\
             FY,WK,FY-0815,Island,0\n\
             FY,WK,FY-0915,Island,0\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             FY-0815,08:15:00,08:15:00,DOCK,1\n\
             FY-0815,08:45:00,08:45:00,ISL,2\n\
             FY-0915,09:15:00,09:15:00,DOCK,1\n\
             FY-0915,09:45:00,09:45:00,ISL,2\n",
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

/// Both feeds, as separate files.
fn state() -> TransitState {
    let set = TimetableSet::new(vec![
        loaded(FEED, metro_source()),
        loaded(FERRY, ferry_source()),
    ])
    .expect("a set");
    TransitState::from_set(set, StateConfig::default()).expect("state")
}

fn request(time: i64) -> NearbyRequest {
    NearbyRequest::new(HOME, time, FEED_TIME + 15)
}

fn try_run(
    state: &TransitState,
    request: &NearbyRequest,
) -> Result<NearbyResponse, PlanError<Infallible, Infallible>> {
    nearby(state, &mut StraightLineAccess::default(), request, &|| {
        false
    })
}

fn run(state: &TransitState, request: &NearbyRequest) -> NearbyResponse {
    try_run(state, request).expect("nearby succeeds")
}

fn detail_request(
    route: &str,
    direction: DirectionKey,
    stop: &str,
    time: i64,
) -> RouteDetailRequest {
    RouteDetailRequest {
        feed: FEED.to_owned(),
        route_id: route.to_owned(),
        direction,
        stop_id: stop.to_owned(),
        time,
        now: FEED_TIME + 15,
        mode: TravelMode::Walking,
        walk_seconds: 0,
        horizon_seconds: 7200,
        max_departures: 3,
        trip: None,
    }
}

fn try_detail(
    state: &TransitState,
    request: &RouteDetailRequest,
) -> Result<RouteDetailResponse, PlanError<Infallible, Infallible>> {
    crate::nearby::route_detail(state, request, &|| false)
}

fn detail(state: &TransitState, request: &RouteDetailRequest) -> RouteDetailResponse {
    try_detail(state, request).expect("route detail succeeds")
}

fn ingest(state: &TransitState, kind: FeedKind, entities: Vec<proto::FeedEntity>) {
    let bytes = encoded_feed(FEED_TIME, entities);
    state
        .ingest(FeedUpdate {
            feed: FEED,
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

fn skipped_at(trip: &str, sequence: u32) -> proto::FeedEntity {
    let mut item = entity(&format!("tu-{trip}"));
    item.trip_update = Some(proto::TripUpdate {
        trip: descriptor(trip, JUNE_15, None),
        stop_time_update: vec![proto::trip_update::StopTimeUpdate {
            stop_sequence: Some(sequence),
            schedule_relationship: Some(
                proto::trip_update::stop_time_update::ScheduleRelationship::Skipped as i32,
            ),
            ..proto::trip_update::StopTimeUpdate::default()
        }],
        ..proto::TripUpdate::default()
    });
    item
}

/// The route of `feed` with `route_id`, which the test expects listed.
fn route<'r>(response: &'r NearbyResponse, feed: &str, route_id: &str) -> &'r NearbyRoute {
    response
        .routes
        .iter()
        .find(|route| route.feed == feed && route.route.route_id == route_id)
        .unwrap_or_else(|| panic!("{feed} {route_id} isn't listed: {:?}", listed(response)))
}

fn listed(response: &NearbyResponse) -> Vec<String> {
    response
        .routes
        .iter()
        .map(|route| format!("{}:{}", route.feed, route.route.route_id))
        .collect()
}

fn direction<'r>(route: &'r NearbyRoute, key: &DirectionKey) -> &'r NearbyDirection {
    route
        .directions
        .iter()
        .find(|d| match key {
            DirectionKey::Id(id) => d.direction.direction_id == Some(*id),
            DirectionKey::Terminus(stop) => {
                d.direction.terminus_stop_id.as_deref() == Some(stop.as_str())
            }
        })
        .unwrap_or_else(|| panic!("{} has no direction {key:?}", route.route.route_id))
}

/// A direction's departures as `(trip, departure, status)`.
fn rows(direction: &NearbyDirection) -> Vec<(&str, i64, DepartureStatus)> {
    direction
        .departures
        .iter()
        .map(|d| (d.trip_id.as_str(), d.departure, d.status))
        .collect()
}

const SCHEDULED: DepartureStatus = DepartureStatus::Scheduled;

/// `june_15` with seconds.
fn at(hours: i64, minutes: i64, seconds: i64) -> i64 {
    june_15(hours, minutes) + seconds
}

#[test]
fn routes_are_listed_by_their_soonest_catchable_departure_across_feeds() {
    let state = state();
    let response = run(&state, &request(june_15(8, 0)));
    assert_eq!(
        listed(&response),
        vec![
            "metro:R4", // 08:01:30 at N1
            "metro:R5", // 08:02 at N3
            "metro:R2", // 08:03 at N1
            "metro:R1", // 08:06 south at N2
            "ferry:FY", // 08:15 at DOCK
            "metro:R3", // 08:20 at N2
        ],
        "route 6 runs only at night and route 7 after the horizon"
    );
    assert_eq!(response.no_routes_reason, None);
    assert_eq!(response.window.earliest, june_15(8, 0));
    assert_eq!(response.window.latest, june_15(10, 0));
    let ferry = route(&response, FERRY, "FY");
    assert_eq!(ferry.route.long_name.as_deref(), Some("Dock Ferry"));
    assert_eq!(ferry.route.route_type, 4);
    assert_eq!(
        ferry.agency.as_ref().map(|a| a.name.as_str()),
        Some("Ferry Co")
    );
    assert_eq!(ferry.directions[0].stop.stop_id, "DOCK");
    assert_eq!(ferry.directions[0].stop.feed.as_deref(), Some(FERRY));

    let max = NearbyRequest {
        max_routes: Some(2),
        ..request(june_15(8, 0))
    };
    assert_eq!(listed(&run(&state, &max)), vec!["metro:R4", "metro:R5"]);
}

#[test]
fn each_direction_uses_the_stop_with_its_earliest_catchable_departure() {
    let state = state();
    let response = run(&state, &request(june_15(8, 0)));
    let r1 = route(&response, FEED, "R1");
    assert_eq!(r1.route.short_name.as_deref(), Some("1"));
    assert_eq!(r1.route.color.as_deref(), Some("FDB71A"));
    assert_eq!(r1.route.text_color.as_deref(), Some("000000"));
    assert_eq!(r1.agency.as_ref().map(|a| a.name.as_str()), Some("Metro"));
    // Directions in order of `direction_id`, whatever leaves first.
    let ids: Vec<Option<u32>> = r1
        .directions
        .iter()
        .map(|d| d.direction.direction_id)
        .collect();
    assert_eq!(ids, vec![Some(0), Some(1)]);

    // North: the 08:00:30 leaves `N1` before the rider gets there and `N2` at 08:02, also too
    // soon, so the 08:10:30 from `N1` is the earliest that can be caught.
    let north = direction(r1, &DirectionKey::Id(0));
    assert_eq!(north.stop.stop_id, "N1");
    assert_eq!(north.walk_seconds, WALK_N1);
    assert_eq!(
        rows(north),
        vec![
            ("R1N-0810", at(8, 10, 30), SCHEDULED),
            ("R1N-0820", at(8, 20, 30), SCHEDULED),
            ("R1N-0830", at(8, 30, 30), SCHEDULED),
        ]
    );
    assert_eq!(north.headsigns, vec!["Northgate", "Northgate via 2nd"]);
    let first = &north.departures[0];
    assert_eq!(first.headsign.as_deref(), Some("Northgate"));
    assert_eq!(first.start_date.as_deref(), Some(JUNE_15));
    assert_eq!(first.scheduled_departure, Some(at(8, 10, 30)));
    assert_eq!(first.delay_seconds, None);
    assert!(!first.realtime);
    assert_eq!(first.accessibility, Accessibility::Accessible);
    assert_eq!(
        north.departures[1].headsign.as_deref(),
        Some("Northgate via 2nd")
    );

    // South: `N2` is farther, but its 08:06 leaves before the same trip reaches `N1` at 08:09.
    let south = direction(r1, &DirectionKey::Id(1));
    assert_eq!(south.stop.stop_id, "N2");
    assert_eq!(south.walk_seconds, WALK_N2);
    assert_eq!(rows(south)[0], ("R1S-0800", june_15(8, 6), SCHEDULED));
    assert_eq!(south.headsigns, vec!["Southcenter"]);
}

#[test]
fn a_departure_is_catchable_from_the_time_plus_the_walk() {
    let state = state();
    // At `N1` exactly as the 08:00:30 northbound leaves: catchable, as routing boards it.
    let exactly = run(&state, &request(at(8, 0, 30) - i64::from(WALK_N1)));
    let north = direction(route(&exactly, FEED, "R1"), &DirectionKey::Id(0));
    assert_eq!(
        (north.stop.stop_id.as_str(), rows(north)[0].0),
        ("N1", "R1N-0800")
    );

    // A second later it is gone from `N1`, and from `N2` too, a walk of 335 s for 08:02.
    let late = run(&state, &request(at(8, 0, 31) - i64::from(WALK_N1)));
    let north = direction(route(&late, FEED, "R1"), &DirectionKey::Id(0));
    assert_eq!(
        (north.stop.stop_id.as_str(), rows(north)[0].0),
        ("N1", "R1N-0810")
    );
    assert!(north.departures.iter().all(|d| d.departure >= at(8, 0, 31)));
}

#[test]
fn trips_without_a_direction_id_are_grouped_by_their_last_stop() {
    let state = state();
    let response = run(&state, &request(june_15(8, 0)));
    let r2 = route(&response, FEED, "R2");
    let termini: Vec<Option<&str>> = r2
        .directions
        .iter()
        .map(|d| {
            assert_eq!(d.direction.direction_id, None);
            d.direction.terminus_stop_id.as_deref()
        })
        .collect();
    // By `stop_id`, so a row's pages keep their order.
    assert_eq!(termini, vec![Some("E1"), Some("EX"), Some("WX")]);

    let east = direction(r2, &DirectionKey::Terminus("EX".to_owned()));
    assert_eq!(east.stop.stop_id, "N1");
    assert_eq!(
        rows(east),
        vec![
            ("R2E-0803", june_15(8, 3), SCHEDULED),
            ("R2E-0813", june_15(8, 13), SCHEDULED),
        ]
    );
    // West: `E1` at 08:05 is sooner than the same trip at `N1` at 08:07.
    let west = direction(r2, &DirectionKey::Terminus("WX".to_owned()));
    assert_eq!(west.stop.stop_id, "E1");
    assert_eq!(west.walk_seconds, WALK_E1);
    assert_eq!(west.stop.accessibility, Accessibility::Unknown);
    assert_eq!(rows(west)[0], ("R2W-0805", june_15(8, 5), SCHEDULED));
    // The short trip ends at `E1`, so it is a direction of its own; and a trip's last stop is
    // never a departure, so it's listed from `N1`, not from `E1`.
    let short = direction(r2, &DirectionKey::Terminus("E1".to_owned()));
    assert_eq!(short.stop.stop_id, "N1");
    assert_eq!(rows(short), vec![("R2S-0820", june_15(8, 20), SCHEDULED)]);
    assert_eq!(short.headsigns, vec!["East 1st"]);
}

#[test]
fn the_horizon_bounds_departures_and_a_direction_without_any_is_dropped() {
    let state = state();
    let short = NearbyRequest {
        horizon_seconds: 15 * 60,
        ..request(june_15(8, 0))
    };
    let response = run(&state, &short);
    // The ferry leaves at 08:15, which is in; route 3's 08:20 isn't.
    assert_eq!(
        listed(&response),
        vec!["metro:R4", "metro:R5", "metro:R2", "metro:R1", "ferry:FY"]
    );
    let r1 = route(&response, FEED, "R1");
    let north = direction(r1, &DirectionKey::Id(0));
    assert_eq!(rows(north), vec![("R1N-0810", at(8, 10, 30), SCHEDULED)]);
    let r2 = route(&response, FEED, "R2");
    assert!(
        r2.directions
            .iter()
            .all(|d| d.direction.terminus_stop_id.as_deref() != Some("E1")),
        "the short trip leaves at 08:20"
    );

    let long = NearbyRequest {
        horizon_seconds: 3 * 3600,
        ..request(june_15(8, 0))
    };
    let response = run(&state, &long);
    assert_eq!(
        rows(&route(&response, FEED, "R7").directions[0]),
        vec![("R7-1030", june_15(10, 30), SCHEDULED)]
    );
}

#[test]
fn a_frequency_trip_lists_each_run() {
    let state = state();
    let response = run(&state, &request(june_15(8, 0)));
    let r3 = route(&response, FEED, "R3");
    assert_eq!(r3.directions[0].stop.stop_id, "N2");
    assert_eq!(
        rows(&r3.directions[0]),
        vec![
            ("F1", june_15(8, 20), SCHEDULED),
            ("F1", june_15(8, 40), SCHEDULED),
            ("F1", june_15(9, 0), SCHEDULED),
        ],
        "the 08:00 run leaves before the rider reaches N2"
    );
}

#[test]
fn departures_run_across_midnight_and_service_days() {
    let state = state();
    let response = run(&state, &request(june_15(23, 45)));
    assert_eq!(listed(&response), vec!["metro:R6"]);
    let night = &route(&response, FEED, "R6").directions[0];
    assert_eq!(
        rows(night),
        vec![
            ("R6-2350", june_15(23, 50), SCHEDULED),
            // 00:10 on June 16, on June 15's service day.
            ("R6-2410", june_15(24, 10), SCHEDULED),
            // June 16's own 00:40.
            ("R6-0040", june_15(24, 40), SCHEDULED),
        ]
    );
    let dates: Vec<Option<&str>> = night
        .departures
        .iter()
        .map(|d| d.start_date.as_deref())
        .collect();
    assert_eq!(dates, vec![Some(JUNE_15), Some(JUNE_15), Some("20260616")]);
}

#[test]
fn a_wheelchair_rider_sees_only_what_they_can_board() {
    let state = state();
    let walking = run(&state, &request(june_15(8, 0)));
    let inaccessible = &route(&walking, FEED, "R4").directions[0].departures[0];
    assert_eq!(inaccessible.accessibility, Accessibility::Inaccessible);
    let steps = &route(&walking, FEED, "R5").directions[0];
    assert_eq!(steps.stop.accessibility, Accessibility::Inaccessible);
    assert_eq!(
        steps.departures[0].accessibility,
        Accessibility::Inaccessible
    );

    let wheelchair = NearbyRequest {
        mode: TravelMode::Wheelchair,
        ..request(june_15(8, 0))
    };
    let response = run(&state, &wheelchair);
    // Route 4's trip is marked inaccessible, and route 5 only serves `N3`, marked so too.
    assert_eq!(
        listed(&response),
        vec!["metro:R2", "metro:R1", "ferry:FY", "metro:R3"]
    );
    let north = direction(route(&response, FEED, "R1"), &DirectionKey::Id(0));
    assert_eq!(north.stop.accessibility, Accessibility::Accessible);
    assert!(
        north
            .departures
            .iter()
            .all(|d| d.accessibility == Accessibility::Accessible)
    );
}

#[test]
fn real_time_data_is_shown_and_only_departures_that_run_choose_the_stop() {
    let state = state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![
            // Two minutes late from `N1`, its second stop.
            delay_update("R1N-0810", JUNE_15, 20, 120),
            canceled("R2E-0803"),
            // The 08:00 south skips `N2`.
            skipped_at("R1S-0800", 20),
        ],
    );
    let response = run(&state, &request(june_15(8, 0)));
    let r1 = route(&response, FEED, "R1");
    let north = direction(r1, &DirectionKey::Id(0));
    let late = &north.departures[0];
    assert_eq!(
        (late.trip_id.as_str(), late.status, late.departure),
        ("R1N-0810", DepartureStatus::Live, at(8, 12, 30))
    );
    assert_eq!(late.scheduled_departure, Some(at(8, 10, 30)));
    assert_eq!(late.delay_seconds, Some(120));
    assert!(late.realtime);

    // The skipped departure can't be caught at `N2`, so its 08:09 at `N1` is the soonest.
    let south = direction(r1, &DirectionKey::Id(1));
    assert_eq!(south.stop.stop_id, "N1");
    assert_eq!(rows(south)[0], ("R1S-0800", june_15(8, 9), SCHEDULED));

    // A canceled departure is listed at its schedule, but the next that runs chooses the stop.
    let east = direction(
        route(&response, FEED, "R2"),
        &DirectionKey::Terminus("EX".to_owned()),
    );
    assert_eq!(east.stop.stop_id, "N1");
    assert_eq!(
        rows(east),
        vec![
            ("R2E-0803", june_15(8, 3), DepartureStatus::Canceled),
            ("R2E-0813", june_15(8, 13), SCHEDULED),
        ]
    );
    assert!(!east.departures[0].realtime);
    assert_eq!(
        response.realtime.feeds[0].state,
        crate::response::FeedState::Applied
    );
}

#[test]
fn a_direction_with_only_canceled_departures_is_dropped() {
    let state = state();
    ingest(&state, FeedKind::TripUpdates, vec![canceled("R2S-0820")]);
    let response = run(&state, &request(june_15(8, 0)));
    let r2 = route(&response, FEED, "R2");
    assert!(
        r2.directions
            .iter()
            .all(|d| d.direction.terminus_stop_id.as_deref() != Some("E1"))
    );
}

#[test]
fn nothing_in_reach_says_why() {
    let state = state();
    let far = NearbyRequest {
        location: LatLon {
            lat: 47.7000,
            lon: -122.3300,
        },
        ..request(june_15(8, 0))
    };
    let response = run(&state, &far);
    assert!(response.routes.is_empty());
    assert_eq!(response.no_routes_reason, Some(NoRoutesReason::NoAccess));

    let quiet = run(&state, &request(june_15(14, 0)));
    assert!(quiet.routes.is_empty());
    assert_eq!(quiet.no_routes_reason, Some(NoRoutesReason::NoService));
    let json = serde_json::to_value(&quiet).expect("serializes");
    assert!(
        json.get("alerts").is_none(),
        "an empty list isn't serialized"
    );
}

#[test]
fn invalid_requests_are_refused() {
    let state = state();
    for (bad, why) in [
        (
            NearbyRequest {
                mode: TravelMode::Cycling,
                ..request(june_15(8, 0))
            },
            "on foot or in a wheelchair",
        ),
        (
            NearbyRequest {
                max_departures: 0,
                ..request(june_15(8, 0))
            },
            "at least one departure",
        ),
        (
            NearbyRequest {
                max_departures: 11,
                ..request(june_15(8, 0))
            },
            "at most ten",
        ),
        (
            NearbyRequest {
                horizon_seconds: 24 * 3600 + 1,
                ..request(june_15(8, 0))
            },
            "at most a day",
        ),
        (
            NearbyRequest {
                max_routes: Some(0),
                ..request(june_15(8, 0))
            },
            "at least one route",
        ),
        (
            NearbyRequest {
                location: LatLon {
                    lat: 91.0,
                    lon: 0.0,
                },
                ..request(june_15(8, 0))
            },
            "a real place",
        ),
    ] {
        let error = try_run(&state, &bad).expect_err(why);
        assert!(
            matches!(error, PlanError::InvalidRequest(_)),
            "{why}: {error:?}"
        );
    }
}

#[test]
fn a_route_detail_lists_departures_a_trip_and_vehicles() {
    let state = state();
    ingest(
        &state,
        FeedKind::VehiclePositions,
        vec![
            vehicle_entity("7033", "R1N-0810", JUNE_15, None, 47.585, -122.33),
            vehicle_entity("8001", "R1S-0800", JUNE_15, None, 47.62, -122.33),
        ],
    );
    let request = RouteDetailRequest {
        walk_seconds: WALK_N1,
        ..detail_request("R1", DirectionKey::Id(0), "N1", june_15(8, 0))
    };
    let response = detail(&state, &request);
    assert_eq!(response.route.route_id, "R1");
    assert_eq!(response.direction.direction_id, Some(0));
    assert_eq!(response.stop.stop_id, "N1");
    let trips: Vec<&str> = response
        .departures
        .iter()
        .map(|d| d.trip_id.as_str())
        .collect();
    assert_eq!(trips, vec!["R1N-0810", "R1N-0820", "R1N-0830"]);

    // The next trip, from `N1` to the end of its trip, with its shape.
    let ride = response.ride.as_ref().expect("a ride");
    assert_eq!(ride.trip.trip_id, "R1N-0810");
    let stops: Vec<&str> = ride.stops.iter().map(|s| s.stop.stop_id.as_str()).collect();
    assert_eq!(stops, vec!["N1", "N2", "NF"]);
    assert_eq!(ride.departure, at(8, 10, 30));
    assert_eq!(ride.arrival, at(8, 20, 30));
    assert_eq!(
        ride.wait_seconds,
        at(8, 10, 30) - june_15(8, 0) - i64::from(WALK_N1)
    );
    assert!(ride.shape.is_some(), "the trip has a shape");
    assert_eq!(
        ride.vehicle.as_ref().and_then(|v| v.id.as_deref()),
        Some("7033")
    );

    // Only the northbound vehicle runs in this direction.
    let ids: Vec<Option<&str>> = response.vehicles.iter().map(|v| v.id.as_deref()).collect();
    assert_eq!(ids, vec![Some("7033")]);
    assert_eq!(response.vehicles[0].latitude, Some(47.585));

    // A chosen trip, by the IDs a departure gave.
    let chosen = RouteDetailRequest {
        trip: Some(TripChoice {
            trip_id: "R1N-0830".to_owned(),
            start_date: Some(JUNE_15.to_owned()),
        }),
        ..request.clone()
    };
    let response = detail(&state, &chosen);
    assert_eq!(
        response
            .ride
            .as_ref()
            .map(|ride| ride.trip.trip_id.as_str()),
        Some("R1N-0830")
    );

    let missing = RouteDetailRequest {
        trip: Some(TripChoice {
            trip_id: "R1S-0830".to_owned(),
            start_date: None,
        }),
        ..request
    };
    assert!(matches!(
        try_detail(&state, &missing),
        Err(PlanError::NotFound(Missing::Trip))
    ));
}

#[test]
fn a_route_detail_shows_skipped_departures_and_a_direction_by_terminus() {
    let state = state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![skipped_at("R1S-0800", 20)],
    );
    let response = detail(
        &state,
        &detail_request("R1", DirectionKey::Id(1), "N2", june_15(8, 0)),
    );
    let statuses: Vec<(&str, DepartureStatus)> = response
        .departures
        .iter()
        .map(|d| (d.trip_id.as_str(), d.status))
        .collect();
    assert_eq!(
        statuses,
        vec![
            ("R1S-0800", DepartureStatus::Skipped),
            ("R1S-0810", SCHEDULED),
            ("R1S-0820", SCHEDULED),
        ]
    );
    // The ride is the first that runs.
    assert_eq!(
        response
            .ride
            .as_ref()
            .map(|ride| ride.trip.trip_id.as_str()),
        Some("R1S-0810")
    );

    let west = detail(
        &state,
        &detail_request(
            "R2",
            DirectionKey::Terminus("WX".to_owned()),
            "E1",
            june_15(8, 0),
        ),
    );
    assert_eq!(west.direction.terminus_stop_id.as_deref(), Some("WX"));
    let trips: Vec<&str> = west.departures.iter().map(|d| d.trip_id.as_str()).collect();
    assert_eq!(trips, vec!["R2W-0805", "R2W-0825"]);
    let ride = west.ride.as_ref().expect("a ride");
    let stops: Vec<&str> = ride.stops.iter().map(|s| s.stop.stop_id.as_str()).collect();
    assert_eq!(stops, vec!["E1", "N1", "WX"]);
}

#[test]
fn a_route_detail_names_what_it_cant_find() {
    let state = state();
    let base = detail_request("R1", DirectionKey::Id(0), "N1", june_15(8, 0));
    let route = RouteDetailRequest {
        route_id: "R99".to_owned(),
        ..base.clone()
    };
    assert!(matches!(
        try_detail(&state, &route),
        Err(PlanError::NotFound(Missing::Route))
    ));
    let stop = RouteDetailRequest {
        stop_id: "ZZ".to_owned(),
        ..base.clone()
    };
    assert!(matches!(
        try_detail(&state, &stop),
        Err(PlanError::NotFound(Missing::Stop))
    ));
    let feed = RouteDetailRequest {
        feed: "nope".to_owned(),
        ..base.clone()
    };
    assert!(matches!(
        try_detail(&state, &feed),
        Err(PlanError::State(crate::error::StateError::UnknownFeed(_)))
    ));
    // A direction the stop has no departures in is an empty answer, not an error.
    let none = detail(
        &state,
        &RouteDetailRequest {
            direction: DirectionKey::Id(7),
            ..base
        },
    );
    assert!(none.departures.is_empty());
    assert!(none.ride.is_none());
}

#[test]
fn the_nearby_json_has_everything_a_row_needs() {
    let state = state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![
            delay_update("R1N-0810", JUNE_15, 20, 120),
            canceled("R2E-0803"),
        ],
    );
    ingest(
        &state,
        FeedKind::Alerts,
        vec![crate::testing::alert_entity(
            "a1",
            proto::alert::Effect::Detour,
            vec![crate::testing::route_selector("R1")],
        )],
    );
    let request = NearbyRequest {
        max_routes: Some(4),
        max_departures: 2,
        ..request(june_15(8, 0))
    };
    assert_json_snapshot!("nearby_response", &run(&state, &request));
}
