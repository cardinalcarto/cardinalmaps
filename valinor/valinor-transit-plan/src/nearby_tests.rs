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
//! - The lines feed has routes for pages, from 11:00 on, at its own `P1`, `P2`, and `P3`
//!   (where `N1`, `N2`, and `E1` are); see [`lines_source`].

use core::convert::Infallible;
use core::fmt::Write as _;

use insta::assert_json_snapshot;
use valinor_gtfs::{FeedId as CompileFeedId, FeedInput, InMemorySource, LatLon, LoadedTimetable};
use valinor_gtfs::{TimetableSet, compile};
use valinor_gtfs_rt::{FeedKind, proto};

use crate::error::{Missing, PlanError};
use crate::nearby::{NearbyRequest, RouteDetailRequest, TripChoice, nearby};
use crate::nearby_seated_tests::segments;
use crate::plan::TravelMode;
use crate::response::{
    Accessibility, ContinuesAs, DepartureStatus, NearbyPage, NearbyResponse, NearbyRoute,
    NoRoutesReason, RouteDetailResponse,
};
use crate::state::{FeedUpdate, StateConfig, TransitState};
use crate::testing::{
    StraightLineAccess, delay_update, descriptor, encoded_feed, entity, june_15, vehicle_entity,
};

const FEED: &str = "metro";
const FERRY: &str = "ferry";
const LINES: &str = "lines";
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
    // Afternoon routes for the stop choice, apart from the morning's.
    // Route 8 rides from `N1` to `N2` for longer than the walk between them takes.
    for (id, n1) in [("R8-1335", hm(13, 35) + 40), ("R8-1344", hm(13, 44))] {
        trips.push((
            "R8",
            id.to_owned(),
            "Long Way",
            "0",
            "",
            "",
            vec![("N1", n1, ""), ("N2", n1 + 380, ""), ("NF", n1 + 900, "")],
        ));
    }
    // Route 9 calls at `N3` and then `N1`, both 42 s away.
    trips.push((
        "R9",
        "R9-1305".to_owned(),
        "Twin Stops",
        "0",
        "",
        "",
        vec![
            ("N3", hm(13, 5), ""),
            ("N1", hm(13, 6), ""),
            ("NF", hm(13, 15), ""),
        ],
    ));
    // Route 10 loops through `N1` twice.
    trips.push((
        "R10",
        "R10-1500".to_owned(),
        "Loop",
        "0",
        "",
        "",
        vec![
            ("N1", hm(15, 0), ""),
            ("NF", hm(15, 10), ""),
            ("N1", hm(15, 20), ""),
            ("WX", hm(15, 30), ""),
        ],
    ));
    // Route 11 is a frequency trip from `N1` to `N2`, seven minutes apart, every ten minutes.
    trips.push((
        "R11",
        "F2".to_owned(),
        "Frequent Long Way",
        "0",
        "",
        "",
        vec![
            ("N1", hm(17, 0), ""),
            ("N2", hm(17, 7), ""),
            ("NF", hm(17, 15), ""),
        ],
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
             R7,MT,7,,3,,\n\
             R8,MT,8,,3,,\n\
             R9,MT,9,,3,,\n\
             R10,MT,10,,3,,\n\
             R11,MT,11,,3,,\n",
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
             F1,08:00:00,10:00:00,1200,1\n\
             F2,17:00:00,18:00:00,600,1\n",
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

/// Routes whose headsigns share pages or don't, on weekdays from 11:00.
///
/// - Route 21's trips leave `P1` for `P2`: "Alder" (three trips) and "Cedar" (two) go on to
///   different places, and "Birch" (one) ends at `P2`.
/// - Route 22's "Gingko" leaves only `P2`, at 11:08, and "Hazel" calls at `P1`, then `P2`,
///   and on as "Gingko" does.
/// - Route 23's "Ivy" calls at `P1` and then `P2`, and "Juniper" at `P3` and then `P2`, both
///   going on alike.
/// - Route 44's "UW Medical Center" goes from `P1` and `P2` to `PZ` and `PW`; its 11:50
///   "University District" ends at `PZ`, where its vehicle goes on as route 43's "Capitol Hill"
///   (one block, a three-minute wait); "Ballard" goes the other way.
/// - Route 26 has one trip a page: by `direction_id` 1, none, and 0 twice, one without a
///   headsign, each leaving `P1` earlier than the page before it in the route's order.
/// - Route 62's trips read "Downtown via Fremont" at `P1` and "Downtown" from `P2` on.
/// - Route 71's 11:10 reads "Alpha" at `P1` and "Beta" at `P2`, and its 11:20 "Gamma" at `P3`
///   and "Beta" at `P2`, each going on somewhere else.
fn lines_source() -> InMemorySource {
    InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\n\
             LN,Lines,America/Los_Angeles,https://l.test\n",
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_type\n\
             L21,LN,21,3\n\
             L22,LN,22,3\n\
             L23,LN,23,3\n\
             L44,LN,44,3\n\
             L43,LN,43,3\n\
             L26,LN,26,3\n\
             L62,LN,62,3\n\
             L71,LN,71,3\n",
        )
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon\n\
             P1,Line 1st,47.6005,-122.3300\n\
             P2,Line 2nd,47.6040,-122.3300\n\
             P3,Line East,47.6000,-122.3290\n\
             PX,Far North,47.6300,-122.3300\n\
             PY,Far South,47.5700,-122.3300\n\
             PZ,Far East,47.6000,-122.3000\n\
             PW,Farther East,47.6000,-122.2800\n\
             PV,Hilltop,47.6500,-122.3000\n",
        )
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,trip_headsign,direction_id,block_id\n\
             L21,WK,L21-A1110,Alder,0,\n\
             L21,WK,L21-A1120,Alder,0,\n\
             L21,WK,L21-A1130,Alder,0,\n\
             L21,WK,L21-B1115,Birch,0,\n\
             L21,WK,L21-C1112,Cedar,0,\n\
             L21,WK,L21-C1125,Cedar,0,\n\
             L22,WK,L22-G1108,Gingko,0,\n\
             L22,WK,L22-H1114,Hazel,0,\n\
             L22,WK,L22-H1144,Hazel,0,\n\
             L23,WK,L23-I1106,Ivy,0,\n\
             L23,WK,L23-J1120,Juniper,0,\n\
             L44,WK,L44-1140,UW Medical Center,0,\n\
             L44,WK,L44-1200,UW Medical Center,0,\n\
             L44,WK,L44-1150,University District,0,B44\n\
             L43,WK,L43-1203,Capitol Hill,0,B44\n\
             L44,WK,L44-1135,Ballard,1,\n\
             L26,WK,L26-M1101,Maple,,\n\
             L26,WK,L26-A1102,Aspen,1,\n\
             L26,WK,L26-N1103,,0,\n\
             L26,WK,L26-Z1104,Zinnia,0,\n\
             L62,WK,L62-1110,Downtown,1,\n\
             L62,WK,L62-1150,Downtown,1,\n\
             L71,WK,L71-1110,Beta,0,\n\
             L71,WK,L71-1120,Beta,0,\n",
        )
        .with("stop_times.txt", &lines_stop_times())
}

#[expect(clippy::too_many_lines, reason = "one feed's trips, listed in full")]
fn lines_stop_times() -> String {
    let trips: [(&str, &[(&str, u32)]); 24] = [
        (
            "L21-A1110",
            &[("P1", hm(11, 10)), ("P2", hm(11, 12)), ("PX", hm(11, 20))],
        ),
        (
            "L21-A1120",
            &[("P1", hm(11, 20)), ("P2", hm(11, 22)), ("PX", hm(11, 30))],
        ),
        (
            "L21-A1130",
            &[("P1", hm(11, 30)), ("P2", hm(11, 32)), ("PX", hm(11, 40))],
        ),
        ("L21-B1115", &[("P1", hm(11, 15)), ("P2", hm(11, 17))]),
        (
            "L21-C1112",
            &[("P1", hm(11, 12)), ("P2", hm(11, 14)), ("PY", hm(11, 30))],
        ),
        (
            "L21-C1125",
            &[("P1", hm(11, 25)), ("P2", hm(11, 27)), ("PY", hm(11, 43))],
        ),
        ("L22-G1108", &[("P2", hm(11, 8)), ("PX", hm(11, 20))]),
        (
            "L22-H1114",
            &[("P1", hm(11, 14)), ("P2", hm(11, 16)), ("PX", hm(11, 28))],
        ),
        (
            "L22-H1144",
            &[("P1", hm(11, 44)), ("P2", hm(11, 46)), ("PX", hm(11, 58))],
        ),
        (
            "L23-I1106",
            &[("P1", hm(11, 6)), ("P2", hm(11, 8)), ("PX", hm(11, 20))],
        ),
        (
            "L23-J1120",
            &[("P3", hm(11, 20)), ("P2", hm(11, 23)), ("PX", hm(11, 35))],
        ),
        (
            "L44-1140",
            &[
                ("P1", hm(11, 40)),
                ("P2", hm(11, 42)),
                ("PZ", hm(11, 50)),
                ("PW", hm(11, 55)),
            ],
        ),
        (
            "L44-1200",
            &[
                ("P1", hm(12, 0)),
                ("P2", hm(12, 2)),
                ("PZ", hm(12, 10)),
                ("PW", hm(12, 15)),
            ],
        ),
        (
            "L44-1150",
            &[("P1", hm(11, 50)), ("P2", hm(11, 52)), ("PZ", hm(12, 0))],
        ),
        ("L43-1203", &[("PZ", hm(12, 3)), ("PV", hm(12, 20))]),
        (
            "L44-1135",
            &[
                ("PW", hm(11, 35)),
                ("PZ", hm(11, 40)),
                ("P2", hm(11, 50)),
                ("P1", hm(11, 55)),
                ("PX", hm(12, 5)),
            ],
        ),
        ("L26-M1101", &[("P1", hm(11, 1)), ("PX", hm(11, 10))]),
        ("L26-A1102", &[("P1", hm(11, 2)), ("PY", hm(11, 10))]),
        ("L26-N1103", &[("P1", hm(11, 3)), ("PZ", hm(11, 10))]),
        ("L26-Z1104", &[("P1", hm(11, 4)), ("PW", hm(11, 10))]),
        (
            "L62-1110",
            &[("P1", hm(11, 10)), ("P2", hm(11, 12)), ("PY", hm(11, 25))],
        ),
        (
            "L62-1150",
            &[("P1", hm(11, 50)), ("P2", hm(11, 52)), ("PY", hm(12, 5))],
        ),
        (
            "L71-1110",
            &[("P1", hm(11, 10)), ("P2", hm(11, 12)), ("PX", hm(11, 20))],
        ),
        (
            "L71-1120",
            &[("P3", hm(11, 20)), ("P2", hm(11, 23)), ("PY", hm(11, 35))],
        ),
    ];
    // Headsigns that differ from their trip's, by trip and stop.
    let stop_headsigns = [
        ("L62-1110", "P1", "Downtown via Fremont"),
        ("L62-1150", "P1", "Downtown via Fremont"),
        ("L71-1110", "P1", "Alpha"),
        ("L71-1120", "P3", "Gamma"),
    ];
    let mut times =
        String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence,stop_headsign\n");
    for (trip, stops) in trips {
        for (sequence, &(stop, at)) in stops.iter().enumerate() {
            let at = clock(at);
            let headsign = stop_headsigns
                .iter()
                .find(|&&(t, s, _)| t == trip && s == stop)
                .map_or("", |&(_, _, headsign)| headsign);
            writeln!(times, "{trip},{at},{at},{stop},{},{headsign}", sequence + 1)
                .expect("to a string");
        }
    }
    times
}

fn loaded(id: &str, source: InMemorySource) -> LoadedTimetable {
    let compiled = compile(vec![FeedInput {
        id: CompileFeedId::try_new(id.to_owned()).expect("valid id"),
        source: Box::new(source),
    }])
    .expect("compiles");
    LoadedTimetable::from_bytes(compiled.bytes).expect("loads")
}

/// Every feed, each its own file.
fn state() -> TransitState {
    let set = TimetableSet::new(vec![
        loaded(FEED, metro_source()),
        loaded(FERRY, ferry_source()),
        loaded(LINES, lines_source()),
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
    let response = try_run(state, request).expect("nearby succeeds");
    assert_no_trip_on_two_pages(&response);
    response
}

/// A run, by `trip_id` and `start_date`, is one vehicle, so it is on one page of its route.
fn assert_no_trip_on_two_pages(response: &NearbyResponse) {
    for route in &response.routes {
        let mut seen: Vec<(&str, Option<&str>, usize)> = Vec::new();
        for (index, page) in route.pages.iter().enumerate() {
            for departure in &page.departures {
                let run = (departure.trip_id.as_str(), departure.start_date.as_deref());
                if let Some(&(_, _, other)) = seen.iter().find(|&&(t, d, _)| (t, d) == run) {
                    assert_eq!(
                        other, index,
                        "{run:?} is on two pages of {}",
                        route.route.route_id
                    );
                }
                seen.push((run.0, run.1, index));
            }
        }
    }
}

fn detail_request(route: &str, headsigns: &[&str], stop: &str, time: i64) -> RouteDetailRequest {
    RouteDetailRequest {
        feed: FEED.to_owned(),
        route_id: route.to_owned(),
        headsigns: headsigns.iter().map(|&h| h.to_owned()).collect(),
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
    ingest_into(state, FEED, kind, entities);
}

fn ingest_into(state: &TransitState, feed: &str, kind: FeedKind, entities: Vec<proto::FeedEntity>) {
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

/// The page of `route` that lists `headsign`.
fn page<'r>(route: &'r NearbyRoute, headsign: &str) -> &'r NearbyPage {
    route
        .pages
        .iter()
        .find(|page| page.headsigns.iter().any(|h| h == headsign))
        .unwrap_or_else(|| panic!("{} has no page for {headsign}", route.route.route_id))
}

/// Each page's headsigns, in the route's order.
fn pages(route: &NearbyRoute) -> Vec<Vec<&str>> {
    route
        .pages
        .iter()
        .map(|page| page.headsigns.iter().map(String::as_str).collect())
        .collect()
}

/// A page's departures as `(trip, departure, status)`.
fn rows(page: &NearbyPage) -> Vec<(&str, i64, DepartureStatus)> {
    page.departures
        .iter()
        .map(|d| (d.trip_id.as_str(), d.departure, d.status))
        .collect()
}

const SCHEDULED: DepartureStatus = DepartureStatus::Scheduled;

/// Route 1's northbound page: both its headsigns at `N1`.
const NORTH: &[&str] = &["Northgate", "Northgate via 2nd"];

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
    assert_eq!(ferry.pages[0].stop.stop_id, "DOCK");
    assert_eq!(ferry.pages[0].stop.feed.as_deref(), Some(FERRY));

    let max = NearbyRequest {
        max_routes: Some(2),
        ..request(june_15(8, 0))
    };
    assert_eq!(listed(&run(&state, &max)), vec!["metro:R4", "metro:R5"]);
}

#[test]
fn each_page_uses_the_stop_with_its_earliest_catchable_departure() {
    let state = state();
    let response = run(&state, &request(june_15(8, 0)));
    let r1 = route(&response, FEED, "R1");
    assert_eq!(r1.route.short_name.as_deref(), Some("1"));
    assert_eq!(r1.route.color.as_deref(), Some("FDB71A"));
    assert_eq!(r1.route.text_color.as_deref(), Some("000000"));
    assert_eq!(r1.agency.as_ref().map(|a| a.name.as_str()), Some("Metro"));
    // Pages in order of `direction_id`, whatever leaves first. The 08:20's stop headsign is a
    // headsign of its own, but its trip calls at the same stops as the others, so they share
    // a page.
    let ids: Vec<Option<u32>> = r1.pages.iter().map(|p| p.direction_id).collect();
    assert_eq!(ids, vec![Some(0), Some(1)]);
    assert_eq!(
        pages(r1),
        vec![vec!["Northgate", "Northgate via 2nd"], vec!["Southcenter"]]
    );

    // North: the 08:00:30 leaves `N1` before the rider gets there and `N2` at 08:02, also too
    // soon, so the 08:10:30 from `N1` is the earliest that can be caught.
    let north = page(r1, "Northgate");
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

    // South: the earliest trip leaves `N2` at 08:06, but the rider can still catch it at `N1`,
    // a far shorter walk, at 08:09.
    let south = page(r1, "Southcenter");
    assert_eq!(south.stop.stop_id, "N1");
    assert_eq!(south.walk_seconds, WALK_N1);
    assert_eq!(rows(south)[0], ("R1S-0800", june_15(8, 9), SCHEDULED));
    assert_eq!(south.headsigns, vec!["Southcenter"]);
}

#[test]
fn a_departure_is_catchable_from_the_time_plus_the_walk() {
    let state = state();
    // At `N1` exactly as the 08:00:30 northbound leaves: catchable, as routing boards it.
    let exactly = run(&state, &request(at(8, 0, 30) - i64::from(WALK_N1)));
    let north = page(route(&exactly, FEED, "R1"), "Northgate");
    assert_eq!(
        (north.stop.stop_id.as_str(), rows(north)[0].0),
        ("N1", "R1N-0800")
    );

    // A second later it is gone from `N1`, and from `N2` too, a walk of 335 s for 08:02.
    let late = run(&state, &request(at(8, 0, 31) - i64::from(WALK_N1)));
    let north = page(route(&late, FEED, "R1"), "Northgate");
    assert_eq!(
        (north.stop.stop_id.as_str(), rows(north)[0].0),
        ("N1", "R1N-0810")
    );
    assert!(north.departures.iter().all(|d| d.departure >= at(8, 0, 31)));
}

#[test]
fn a_short_turn_shares_the_page_of_the_trips_it_runs_with() {
    let state = state();
    let response = run(&state, &request(june_15(8, 0)));
    let r2 = route(&response, FEED, "R2");
    // From `N1`, the short trip to `E1` calls where the trips to `EX` do until it ends, so
    // they share a page, the more frequent headsign first. The trips to `WX` go the other
    // way. Without a `direction_id`, pages are in order of headsign.
    assert_eq!(
        pages(r2),
        vec![vec!["East End", "East 1st"], vec!["West End"]]
    );
    assert!(r2.pages.iter().all(|p| p.direction_id.is_none()));

    let east = page(r2, "East End");
    assert_eq!(east.stop.stop_id, "N1");
    // A trip's last stop is never a departure, so the short trip is listed from `N1`.
    assert_eq!(
        rows(east),
        vec![
            ("R2E-0803", june_15(8, 3), SCHEDULED),
            ("R2E-0813", june_15(8, 13), SCHEDULED),
            ("R2S-0820", june_15(8, 20), SCHEDULED),
        ]
    );
    let headsigns: Vec<Option<&str>> = east
        .departures
        .iter()
        .map(|d| d.headsign.as_deref())
        .collect();
    assert_eq!(
        headsigns,
        vec![Some("East End"), Some("East End"), Some("East 1st")]
    );
    assert_eq!(east.soonest_departure, june_15(8, 3));
    // West: the trip leaves `E1` at 08:05, but `N1`, where it calls at 08:07, is nearer.
    let west = page(r2, "West End");
    assert_eq!(west.stop.stop_id, "N1");
    assert_eq!(west.walk_seconds, WALK_N1);
    assert_eq!(rows(west)[0], ("R2W-0805", june_15(8, 7), SCHEDULED));
    assert_eq!(west.soonest_departure, june_15(8, 7));
}

#[test]
fn the_horizon_bounds_departures_and_a_headsign_without_any_has_no_page() {
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
    let north = page(r1, "Northgate");
    assert_eq!(rows(north), vec![("R1N-0810", at(8, 10, 30), SCHEDULED)]);
    let r2 = route(&response, FEED, "R2");
    assert_eq!(
        pages(r2),
        vec![vec!["East End"], vec!["West End"]],
        "the short trip leaves at 08:20"
    );

    let long = NearbyRequest {
        horizon_seconds: 3 * 3600,
        ..request(june_15(8, 0))
    };
    let response = run(&state, &long);
    assert_eq!(
        rows(&route(&response, FEED, "R7").pages[0]),
        vec![("R7-1030", june_15(10, 30), SCHEDULED)]
    );
}

#[test]
fn the_earliest_trip_is_caught_at_the_nearest_stop_where_it_still_can_be() {
    let state = state();
    let long_way = |time: i64| {
        let response = run(&state, &request(time));
        route(&response, FEED, "R8").pages[0].clone()
    };
    // At `N1` in time for the 13:35:40: both stops catch it, and `N1` is nearer.
    let near = long_way(at(13, 35, 40) - i64::from(WALK_N1));
    assert_eq!(
        (near.stop.stop_id.as_str(), near.walk_seconds),
        ("N1", WALK_N1)
    );
    assert_eq!(rows(&near)[0], ("R8-1335", at(13, 35, 40), SCHEDULED));

    // A second later it is gone from `N1`, but still catchable at `N2`, at 13:42. The later trip
    // from the nearer `N1`, at 13:44, doesn't win over it.
    let far = long_way(at(13, 35, 41) - i64::from(WALK_N1));
    assert_eq!(
        (far.stop.stop_id.as_str(), far.walk_seconds),
        ("N2", WALK_N2)
    );
    assert_eq!(
        rows(&far),
        vec![
            ("R8-1335", at(13, 42, 0), SCHEDULED),
            ("R8-1344", at(13, 50, 20), SCHEDULED),
        ]
    );
}

#[test]
fn equal_walks_take_the_earlier_departure() {
    let state = state();
    // `N3` and `N1` are both 42 s away; the trip leaves `N3` first.
    let response = run(&state, &request(june_15(13, 0)));
    let twins = &route(&response, FEED, "R9").pages[0];
    assert_eq!(twins.stop.stop_id, "N3");
    assert_eq!(rows(twins), vec![("R9-1305", june_15(13, 5), SCHEDULED)]);
}

#[test]
fn a_loop_through_the_stop_twice_lists_both_visits_from_one_stop() {
    let state = state();
    let response = run(&state, &request(june_15(14, 55)));
    let looped = &route(&response, FEED, "R10").pages[0];
    assert_eq!(looped.stop.stop_id, "N1");
    assert_eq!(
        rows(looped),
        vec![
            ("R10-1500", june_15(15, 0), SCHEDULED),
            ("R10-1500", june_15(15, 20), SCHEDULED),
        ]
    );
    // Once the first visit is gone, the second is still the same trip at the same stop.
    let later = run(&state, &request(june_15(15, 5)));
    let looped = &route(&later, FEED, "R10").pages[0];
    assert_eq!(
        (looped.stop.stop_id.as_str(), rows(looped)),
        ("N1", vec![("R10-1500", june_15(15, 20), SCHEDULED)])
    );
}

#[test]
fn each_run_of_a_frequency_trip_is_a_trip_of_its_own() {
    let state = state();
    // The 17:00 run leaves `N1` before the rider is there, but is caught at `N2` at 17:07.
    // The 17:10 run shares its `trip_id` and calls at the nearer `N1`, but is a later trip.
    let response = run(&state, &request(june_15(17, 0)));
    let frequent = &route(&response, FEED, "R11").pages[0];
    assert_eq!(frequent.stop.stop_id, "N2");
    assert_eq!(
        rows(frequent),
        vec![
            ("F2", june_15(17, 7), SCHEDULED),
            ("F2", june_15(17, 17), SCHEDULED),
            ("F2", june_15(17, 27), SCHEDULED),
        ]
    );
}

#[test]
fn a_frequency_trip_lists_each_run() {
    let state = state();
    let response = run(&state, &request(june_15(8, 0)));
    let r3 = route(&response, FEED, "R3");
    assert_eq!(r3.pages[0].stop.stop_id, "N2");
    assert_eq!(
        rows(&r3.pages[0]),
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
    let night = &route(&response, FEED, "R6").pages[0];
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
    let inaccessible = &route(&walking, FEED, "R4").pages[0].departures[0];
    assert_eq!(inaccessible.accessibility, Accessibility::Inaccessible);
    let steps = &route(&walking, FEED, "R5").pages[0];
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
    let north = page(route(&response, FEED, "R1"), "Northgate");
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
    let north = page(r1, "Northgate");
    let late = &north.departures[0];
    assert_eq!(
        (late.trip_id.as_str(), late.status, late.departure),
        ("R1N-0810", DepartureStatus::Live, at(8, 12, 30))
    );
    assert_eq!(late.scheduled_departure, Some(at(8, 10, 30)));
    assert_eq!(late.delay_seconds, Some(120));
    assert!(late.realtime);

    // The trip skips `N2`, so only its 08:09 at `N1` can be caught.
    let south = page(r1, "Southcenter");
    assert_eq!(south.stop.stop_id, "N1");
    assert_eq!(rows(south)[0], ("R1S-0800", june_15(8, 9), SCHEDULED));

    // A canceled departure is listed at its schedule, but the next that runs chooses the stop.
    let east = page(route(&response, FEED, "R2"), "East End");
    assert_eq!(east.stop.stop_id, "N1");
    assert_eq!(
        rows(east),
        vec![
            ("R2E-0803", june_15(8, 3), DepartureStatus::Canceled),
            ("R2E-0813", june_15(8, 13), SCHEDULED),
            ("R2S-0820", june_15(8, 20), SCHEDULED),
        ]
    );
    assert!(!east.departures[0].realtime);
    assert_eq!(
        response.realtime.feeds[0].state,
        crate::response::FeedState::Applied
    );
}

#[test]
fn canceled_departures_give_a_page_no_headsign() {
    let state = state();
    ingest(&state, FeedKind::TripUpdates, vec![canceled("R2S-0820")]);
    let response = run(&state, &request(june_15(8, 0)));
    let r2 = route(&response, FEED, "R2");
    // The short trip's only departure is canceled, so its headsign is on no page, and the
    // departure isn't listed: it belongs to that headsign's page.
    assert_eq!(pages(r2), vec![vec!["East End"], vec!["West End"]]);
    assert_eq!(
        rows(page(r2, "East End")),
        vec![
            ("R2E-0803", june_15(8, 3), SCHEDULED),
            ("R2E-0813", june_15(8, 13), SCHEDULED),
        ]
    );

    // A canceled departure of a headsign that has a page is listed there, and a page's
    // headsigns are only those of departures that run.
    ingest(&state, FeedKind::TripUpdates, vec![canceled("R1N-0820")]);
    let response = run(&state, &request(june_15(8, 0)));
    let north = page(route(&response, FEED, "R1"), "Northgate");
    assert_eq!(north.headsigns, vec!["Northgate"]);
    assert_eq!(
        rows(north),
        vec![
            ("R1N-0810", at(8, 10, 30), SCHEDULED),
            ("R1N-0830", at(8, 30, 30), SCHEDULED),
        ],
        "the canceled 08:20 is under its own headsign, which has no page"
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

    let quiet = run(&state, &request(june_15(19, 0)));
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
        ..detail_request("R1", NORTH, "N1", june_15(8, 0))
    };
    let response = detail(&state, &request);
    assert_eq!(response.route.route_id, "R1");
    assert_eq!(response.headsigns, NORTH);
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

/// Decodes a polyline6 to `(lat, lon)` pairs in millionths of a degree, as it encodes them.
fn polyline6(encoded: &str) -> Vec<(i64, i64)> {
    let mut values = Vec::new();
    let (mut value, mut shift) = (0_i64, 0_u32);
    for byte in encoded.bytes() {
        let chunk = i64::from(byte) - 63;
        value |= (chunk & 0x1f) << shift;
        shift += 5;
        if chunk < 0x20 {
            values.push(if value & 1 == 1 {
                !(value >> 1)
            } else {
                value >> 1
            });
            (value, shift) = (0, 0);
        }
    }
    let (mut lat, mut lon) = (0, 0);
    values
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[d_lat, d_lon]| {
            lat += d_lat;
            lon += d_lon;
            (lat, lon)
        })
        .collect()
}

/// `(lat, lon)` in millionths of a degree.
fn micro(lat: f64, lon: f64) -> (i64, i64) {
    #[expect(clippy::cast_possible_truncation, reason = "test coordinates")]
    ((lat * 1e6).round() as i64, (lon * 1e6).round() as i64)
}

fn shape_to_stop(response: &RouteDetailResponse) -> Option<Vec<(i64, i64)>> {
    response.shape_to_stop.as_deref().map(polyline6)
}

#[test]
fn a_route_detail_draws_its_trip_before_the_stop_on_the_trip_shape() {
    let state = state();
    // From `N1`: the shape from `SF`, the trip's first stop, to `N1`, where the ride's begins.
    let at_n1 = detail(&state, &detail_request("R1", NORTH, "N1", june_15(8, 0)));
    let ride = at_n1.ride.as_ref().expect("a ride");
    assert_eq!(ride.trip.trip_id, "R1N-0800");
    let before = shape_to_stop(&at_n1).expect("a shape before the stop");
    assert_eq!(before, vec![micro(47.57, -122.33), micro(47.6005, -122.33)]);
    let after = polyline6(ride.shape.as_deref().expect("the ride's shape"));
    assert_eq!(
        before.last(),
        after.first(),
        "the two lines meet at the stop"
    );

    // From `N2`: on along the shape, by its bend between `N1` and `N2`.
    let at_n2 = detail(&state, &detail_request("R1", NORTH, "N2", june_15(8, 0)));
    assert_eq!(
        shape_to_stop(&at_n2).expect("a shape before the stop"),
        vec![
            micro(47.57, -122.33),
            micro(47.6005, -122.33),
            micro(47.602, -122.3302),
            micro(47.604, -122.33),
        ]
    );

    // A chosen trip's, not the first's.
    let chosen = detail(
        &state,
        &RouteDetailRequest {
            trip: Some(TripChoice {
                trip_id: "R1N-0830".to_owned(),
                start_date: Some(JUNE_15.to_owned()),
            }),
            ..detail_request("R1", NORTH, "N2", june_15(8, 0))
        },
    );
    assert_eq!(shape_to_stop(&chosen), shape_to_stop(&at_n2));
}

#[test]
fn a_route_detail_draws_a_trip_without_a_shape_before_the_stop_straight() {
    let state = state();
    // The southbound trips have no shape: straight from `NF` through `N2` to `N1`.
    let response = detail(
        &state,
        &detail_request("R1", &["Southcenter"], "N1", june_15(8, 0)),
    );
    assert_eq!(
        response
            .ride
            .as_ref()
            .map(|ride| ride.trip.trip_id.as_str()),
        Some("R1S-0800")
    );
    assert!(
        response
            .ride
            .as_ref()
            .is_some_and(|ride| ride.shape.is_none()),
        "the ride has no shape either"
    );
    assert_eq!(
        shape_to_stop(&response).expect("a line before the stop"),
        vec![
            micro(47.63, -122.33),
            micro(47.604, -122.33),
            micro(47.6005, -122.33),
        ]
    );
}

#[test]
fn a_route_detail_has_nothing_before_a_trip_first_stop() {
    let state = state();
    // Route 2's eastbound trips start at `N1`.
    let response = detail(
        &state,
        &detail_request("R2", &["East End"], "N1", june_15(8, 0)),
    );
    assert!(response.ride.is_some());
    assert_eq!(response.shape_to_stop, None);
    let json = serde_json::to_value(&response).expect("serializes");
    assert!(json.get("shape_to_stop").is_none(), "left out of the JSON");

    // Nor without a ride.
    let none = detail(
        &state,
        &lines_detail("L44", &["Capitol Hill"], "P1", june_15(11, 30)),
    );
    assert!(none.ride.is_none() && none.shape_to_stop.is_none());
}

#[test]
fn a_stay_seated_trip_is_drawn_before_the_stop_from_its_own_first_stop() {
    let state = state();
    // The 43 begins at `PZ`, where the 44 it continues ends: nothing of the 44 is drawn.
    let r43 = detail(
        &state,
        &lines_detail("L43", &["Capitol Hill"], "PZ", june_15(11, 30)),
    );
    assert_eq!(
        r43.ride.as_ref().map(|ride| ride.trip.trip_id.as_str()),
        Some("L43-1203")
    );
    assert_eq!(r43.shape_to_stop, None);

    // And the 44 from `P2` is drawn from its own first stop, `P1`.
    let r44 = detail(
        &state,
        &lines_detail("L44", &["University District"], "P2", june_15(11, 45)),
    );
    assert_eq!(
        r44.ride.as_ref().map(|ride| ride.trip.trip_id.as_str()),
        Some("L44-1150")
    );
    assert_eq!(
        shape_to_stop(&r44).expect("a line before the stop"),
        vec![micro(47.6005, -122.33), micro(47.604, -122.33)]
    );
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
        &detail_request("R1", &["Southcenter"], "N2", june_15(8, 0)),
    );
    let statuses: Vec<(&str, DepartureStatus)> = response
        .departures
        .iter()
        .map(|d| (d.trip_id.as_str(), d.status))
        .collect();
    assert_eq!(
        statuses,
        vec![
            // Skipped, so beside the three that run rather than one of them.
            ("R1S-0800", DepartureStatus::Skipped),
            ("R1S-0810", SCHEDULED),
            ("R1S-0820", SCHEDULED),
            ("R1S-0830", SCHEDULED),
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
        &detail_request("R2", &["West End"], "E1", june_15(8, 0)),
    );
    assert_eq!(west.headsigns, vec!["West End"]);
    let trips: Vec<&str> = west.departures.iter().map(|d| d.trip_id.as_str()).collect();
    assert_eq!(trips, vec!["R2W-0805", "R2W-0825"]);
    let ride = west.ride.as_ref().expect("a ride");
    let stops: Vec<&str> = ride.stops.iter().map(|s| s.stop.stop_id.as_str()).collect();
    assert_eq!(stops, vec!["E1", "N1", "WX"]);
}

#[test]
fn a_route_detail_names_what_it_cant_find() {
    let state = state();
    let base = detail_request("R1", NORTH, "N1", june_15(8, 0));
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
            headsigns: vec!["Nowhere".to_owned()],
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

#[test]
fn only_departures_that_run_count_toward_max_departures() {
    let state = state();
    ingest(&state, FeedKind::TripUpdates, vec![canceled("R1N-0810")]);
    let two = NearbyRequest {
        max_departures: 2,
        ..request(june_15(8, 0))
    };
    let response = run(&state, &two);
    let north = page(route(&response, FEED, "R1"), "Northgate");
    // The canceled 08:10 is listed beside the two that run, within their span.
    assert_eq!(
        rows(north),
        vec![
            ("R1N-0810", at(8, 10, 30), DepartureStatus::Canceled),
            ("R1N-0820", at(8, 20, 30), SCHEDULED),
            ("R1N-0830", at(8, 30, 30), SCHEDULED),
        ]
    );
    // A route detail counts the same way.
    let detail = detail(
        &state,
        &RouteDetailRequest {
            walk_seconds: WALK_N1,
            max_departures: 2,
            ..detail_request("R1", NORTH, "N1", june_15(8, 0))
        },
    );
    let statuses: Vec<(&str, DepartureStatus)> = detail
        .departures
        .iter()
        .map(|d| (d.trip_id.as_str(), d.status))
        .collect();
    assert_eq!(
        statuses,
        vec![
            ("R1N-0810", DepartureStatus::Canceled),
            ("R1N-0820", SCHEDULED),
            ("R1N-0830", SCHEDULED),
        ]
    );

    // A canceled departure after the span isn't listed.
    ingest(&state, FeedKind::TripUpdates, vec![canceled("R1N-0830")]);
    let response = run(&state, &two);
    let north = page(route(&response, FEED, "R1"), "Northgate");
    assert_eq!(
        rows(north),
        vec![
            ("R1N-0810", at(8, 10, 30), SCHEDULED),
            ("R1N-0820", at(8, 20, 30), SCHEDULED),
        ]
    );
}

#[test]
fn the_route_detail_json_has_everything_a_detail_needs() {
    let state = state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![
            delay_update("R1N-0810", JUNE_15, 20, 120),
            canceled("R1N-0820"),
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
    ingest(
        &state,
        FeedKind::VehiclePositions,
        vec![vehicle_entity(
            "7033", "R1N-0810", JUNE_15, None, 47.585, -122.33,
        )],
    );
    let request = RouteDetailRequest {
        walk_seconds: WALK_N1,
        max_departures: 2,
        ..detail_request("R1", NORTH, "N1", june_15(8, 0))
    };
    assert_json_snapshot!("route_detail_response", &detail(&state, &request));
}

/// A route-detail request for one page of a lines route.
fn lines_detail(route: &str, headsigns: &[&str], stop: &str, time: i64) -> RouteDetailRequest {
    RouteDetailRequest {
        feed: LINES.to_owned(),
        ..detail_request(route, headsigns, stop, time)
    }
}

fn canceled_in(feed: &str, state: &TransitState, trips: &[&str]) {
    ingest_into(
        state,
        feed,
        FeedKind::TripUpdates,
        trips.iter().map(|trip| canceled(trip)).collect(),
    );
}

#[test]
fn headsigns_share_a_page_only_when_every_pair_goes_the_same_way() {
    let state = state();
    let response = run(&state, &request(june_15(11, 0)));
    let r21 = route(&response, LINES, "L21");
    // "Birch" ends at `P2`, where "Alder" and "Cedar" both go on, so it goes the same way as
    // each; but they go on to different places, so they don't share a page, and "Birch" is on
    // the first page it can join, the more frequent headsign's.
    assert_eq!(pages(r21), vec![vec!["Alder", "Birch"], vec!["Cedar"]]);
    let alder = page(r21, "Alder");
    assert_eq!(alder.stop.stop_id, "P1");
    assert_eq!(alder.direction_id, Some(0));
    assert_eq!(
        rows(alder),
        vec![
            ("L21-A1110", june_15(11, 10), SCHEDULED),
            ("L21-B1115", june_15(11, 15), SCHEDULED),
            ("L21-A1120", june_15(11, 20), SCHEDULED),
        ]
    );
    assert_eq!(
        rows(page(r21, "Cedar")),
        vec![
            ("L21-C1112", june_15(11, 12), SCHEDULED),
            ("L21-C1125", june_15(11, 25), SCHEDULED),
        ]
    );

    // With "Alder" canceled, "Birch" and "Cedar" share a page, and "Alder" is on none.
    canceled_in(LINES, &state, &["L21-A1110", "L21-A1120", "L21-A1130"]);
    let response = run(&state, &request(june_15(11, 0)));
    let r21 = route(&response, LINES, "L21");
    assert_eq!(pages(r21), vec![vec!["Cedar", "Birch"]]);
    assert_eq!(
        rows(&r21.pages[0]),
        vec![
            ("L21-C1112", june_15(11, 12), SCHEDULED),
            ("L21-B1115", june_15(11, 15), SCHEDULED),
            ("L21-C1125", june_15(11, 25), SCHEDULED),
        ]
    );
}

#[test]
fn a_shared_page_is_caught_where_its_headsigns_together_are_caught_soonest() {
    let state = state();
    let response = run(&state, &request(june_15(11, 0)));
    // The earliest trip of either is "Gingko"'s 11:08, caught only at `P2`, where "Hazel"'s
    // trips go the same way: one page there, though "Hazel" alone would be caught at `P1`.
    let r22 = route(&response, LINES, "L22");
    assert_eq!(pages(r22), vec![vec!["Hazel", "Gingko"]]);
    let shared = &r22.pages[0];
    assert_eq!(
        (shared.stop.stop_id.as_str(), shared.walk_seconds),
        ("P2", WALK_N2)
    );
    assert_eq!(shared.soonest_departure, june_15(11, 8));
    assert_eq!(
        rows(shared),
        vec![
            ("L22-G1108", june_15(11, 8), SCHEDULED),
            ("L22-H1114", june_15(11, 16), SCHEDULED),
            ("L22-H1144", june_15(11, 46), SCHEDULED),
        ]
    );
    // Once "Gingko" has left, "Hazel" is on its own and caught at the nearer `P1`.
    let later = run(&state, &request(june_15(11, 5)));
    let r22 = route(&later, LINES, "L22");
    assert_eq!(pages(r22), vec![vec!["Hazel"]]);
    assert_eq!(r22.pages[0].stop.stop_id, "P1");

    // "Ivy"'s 11:06 is the earliest of route 23, caught at `P1`, which "Juniper" doesn't
    // serve: they don't share a page, though from `P2` they go the same way.
    let r23 = route(&response, LINES, "L23");
    assert_eq!(pages(r23), vec![vec!["Ivy"], vec!["Juniper"]]);
    assert_eq!(page(r23, "Ivy").stop.stop_id, "P1");
    let juniper = page(r23, "Juniper");
    assert_eq!(
        (juniper.stop.stop_id.as_str(), juniper.walk_seconds),
        ("P3", WALK_E1)
    );
}

#[test]
fn a_trip_whose_vehicle_goes_on_as_another_route_says_so() {
    let state = state();
    let response = run(&state, &request(june_15(11, 30)));
    let r44 = route(&response, LINES, "L44");
    // "University District" ends at `PZ`, calling where "UW Medical Center" does until then,
    // so they share a page; its vehicle goes on as route 43, which isn't listed from here.
    assert_eq!(
        pages(r44),
        vec![
            vec!["UW Medical Center", "University District"],
            vec!["Ballard"]
        ]
    );
    let ids: Vec<Option<u32>> = r44.pages.iter().map(|p| p.direction_id).collect();
    assert_eq!(ids, vec![Some(0), Some(1)]);
    assert!(
        response.routes.iter().all(|r| r.route.route_id != "L43"),
        "route 43 runs only far away"
    );
    let east = page(r44, "UW Medical Center");
    assert_eq!(east.stop.stop_id, "P1");
    let rows: Vec<(&str, Option<&str>, Option<&ContinuesAs>)> = east
        .departures
        .iter()
        .map(|d| {
            (
                d.trip_id.as_str(),
                d.headsign.as_deref(),
                d.continues_as.as_ref(),
            )
        })
        .collect();
    let forty_three = ContinuesAs {
        route_id: "L43".to_owned(),
        short_name: Some("43".to_owned()),
        headsign: Some("Capitol Hill".to_owned()),
    };
    assert_eq!(
        rows,
        vec![
            ("L44-1140", Some("UW Medical Center"), None),
            ("L44-1150", Some("University District"), Some(&forty_three)),
            ("L44-1200", Some("UW Medical Center"), None),
        ]
    );
    let json = serde_json::to_value(&east.departures[1]).expect("serializes");
    assert_eq!(
        json["continues_as"],
        serde_json::json!({"route_id": "L43", "short_name": "43", "headsign": "Capitol Hill"})
    );
    assert!(
        serde_json::to_value(&east.departures[0])
            .expect("serializes")
            .get("continues_as")
            .is_none()
    );

    // When the next trip is canceled, the vehicle doesn't go on.
    canceled_in(LINES, &state, &["L43-1203"]);
    let response = run(&state, &request(june_15(11, 30)));
    let east = page(route(&response, LINES, "L44"), "University District");
    assert!(east.departures.iter().all(|d| d.continues_as.is_none()));
}

#[test]
fn a_route_detail_rides_on_as_the_route_its_vehicle_continues_as() {
    let state = state();
    let forty_three = ContinuesAs {
        route_id: "L43".to_owned(),
        short_name: Some("43".to_owned()),
        headsign: Some("Capitol Hill".to_owned()),
    };
    // A route detail's ride goes on with the vehicle, as route 43, to its end.
    let university_district = RouteDetailRequest {
        trip: Some(TripChoice {
            trip_id: "L44-1150".to_owned(),
            start_date: None,
        }),
        ..lines_detail("L44", &["University District"], "P1", june_15(11, 30))
    };
    let detail_44 = detail(&state, &university_district);
    let ride = detail_44.ride.as_ref().expect("a ride");
    let stops: Vec<&str> = ride.stops.iter().map(|s| s.stop.stop_id.as_str()).collect();
    assert_eq!(stops, vec!["P1", "P2", "PZ", "PV"]);
    assert_eq!(
        segments(ride),
        vec![
            ("L44-1150", Some("L44"), Some("University District"), 0, 2),
            ("L43-1203", Some("L43"), Some("Capitol Hill"), 2, 3),
        ]
    );
    // `PZ` arrives as the 44 and leaves as the 43, three minutes later.
    assert_eq!(
        (ride.stops[2].arrival, ride.stops[2].departure),
        (june_15(12, 0), june_15(12, 3))
    );
    assert_eq!(ride.arrival, june_15(12, 20));
    assert_eq!(
        detail_44.departures[0].continues_as.as_ref(),
        Some(&forty_three)
    );

    // The row is route 44's, whatever the vehicle goes on as: its ride is the 44's trip,
    // and its departures are the 44's.
    assert_eq!(detail_44.route.route_id, "L44");
    assert_eq!(ride.trip.trip_id, "L44-1150");
    assert_eq!(
        ride.route.as_ref().map(|r| r.route_id.as_str()),
        Some("L44")
    );
    assert!(
        detail_44
            .departures
            .iter()
            .all(|d| d.trip_id.starts_with("L44-")),
        "{:?}",
        detail_44.departures
    );
    // From where route 43's trip begins, its own row rides it alone.
    let detail_43 = detail(
        &state,
        &lines_detail("L43", &["Capitol Hill"], "PZ", june_15(11, 30)),
    );
    let ride_43 = detail_43.ride.as_ref().expect("a ride");
    assert_eq!(ride_43.stops.len(), 2);
    assert!(ride_43.segments.is_empty());

    // When the next trip is canceled, the ride ends with the 44.
    canceled_in(LINES, &state, &["L43-1203"]);
    let detail_44 = detail(&state, &university_district);
    let ride = detail_44.ride.as_ref().expect("a ride");
    let stops: Vec<&str> = ride.stops.iter().map(|s| s.stop.stop_id.as_str()).collect();
    assert_eq!(stops, vec!["P1", "P2", "PZ"]);
    assert!(ride.segments.is_empty());
}

#[test]
fn pages_keep_their_order_whatever_leaves_first() {
    let state = state();
    let response = run(&state, &request(june_15(11, 0)));
    let r26 = route(&response, LINES, "L26");
    // By `direction_id`, none last, then by headsign, none last: the reverse of the order they
    // leave in.
    assert_eq!(
        pages(r26),
        vec![vec!["Zinnia"], vec![], vec!["Aspen"], vec!["Maple"]]
    );
    let ids: Vec<Option<u32>> = r26.pages.iter().map(|p| p.direction_id).collect();
    assert_eq!(ids, vec![Some(0), Some(0), Some(1), None]);
    let soonest: Vec<i64> = r26.pages.iter().map(|p| p.soonest_departure).collect();
    assert_eq!(
        soonest,
        vec![
            june_15(11, 4),
            june_15(11, 3),
            june_15(11, 2),
            june_15(11, 1)
        ]
    );
    // The route is listed by its soonest page.
    let before: Vec<String> = listed(&response)
        .into_iter()
        .take_while(|id| id != "lines:L26")
        .collect();
    assert!(
        before.is_empty(),
        "{before:?} are listed before route 26's 11:01"
    );
    let unnamed = serde_json::to_value(&r26.pages[1]).expect("serializes");
    assert!(unnamed.get("headsigns").is_none());

    // A page without headsigns is asked for by none.
    let detail = detail(&state, &lines_detail("L26", &[], "P1", june_15(11, 0)));
    let trips: Vec<&str> = detail
        .departures
        .iter()
        .map(|d| d.trip_id.as_str())
        .collect();
    assert_eq!(trips, vec!["L26-N1103"]);
}

#[test]
fn a_route_detail_shows_one_page_and_its_vehicles() {
    let state = state();
    ingest_into(
        &state,
        LINES,
        FeedKind::VehiclePositions,
        vec![
            vehicle_entity("401", "L44-1140", JUNE_15, None, 47.6, -122.34),
            vehicle_entity("402", "L44-1150", JUNE_15, None, 47.6, -122.35),
            vehicle_entity("403", "L44-1135", JUNE_15, None, 47.6, -122.29),
        ],
    );
    let east = detail(
        &state,
        &RouteDetailRequest {
            walk_seconds: WALK_N1,
            ..lines_detail(
                "L44",
                &["UW Medical Center", "University District"],
                "P1",
                june_15(11, 30),
            )
        },
    );
    assert_eq!(
        east.headsigns,
        vec!["UW Medical Center", "University District"]
    );
    let trips: Vec<&str> = east.departures.iter().map(|d| d.trip_id.as_str()).collect();
    assert_eq!(trips, vec!["L44-1140", "L44-1150", "L44-1200"]);
    assert_eq!(
        east.ride.as_ref().map(|ride| ride.trip.trip_id.as_str()),
        Some("L44-1140")
    );
    let vehicles: Vec<Option<&str>> = east.vehicles.iter().map(|v| v.id.as_deref()).collect();
    assert_eq!(vehicles, vec![Some("401"), Some("402")]);

    // One headsign of the page lists only its own departures and vehicles.
    let short = detail(
        &state,
        &lines_detail("L44", &["University District"], "P1", june_15(11, 30)),
    );
    let trips: Vec<&str> = short
        .departures
        .iter()
        .map(|d| d.trip_id.as_str())
        .collect();
    assert_eq!(trips, vec!["L44-1150"]);
    let vehicles: Vec<Option<&str>> = short.vehicles.iter().map(|v| v.id.as_deref()).collect();
    assert_eq!(vehicles, vec![Some("402")]);

    let west = detail(
        &state,
        &lines_detail("L44", &["Ballard"], "P1", june_15(11, 30)),
    );
    let trips: Vec<&str> = west.departures.iter().map(|d| d.trip_id.as_str()).collect();
    assert_eq!(trips, vec!["L44-1135"]);
    let vehicles: Vec<Option<&str>> = west.vehicles.iter().map(|v| v.id.as_deref()).collect();
    assert_eq!(vehicles, vec![Some("403")]);

    // A headsign the stop has no departures under is an empty answer, not an error.
    let none = detail(
        &state,
        &lines_detail("L44", &["Capitol Hill"], "P1", june_15(11, 30)),
    );
    assert!(none.departures.is_empty() && none.ride.is_none() && none.vehicles.is_empty());

    // A headsign is never empty, and a page names each of its own once.
    for (headsigns, why) in [
        (vec![""], "an empty headsign"),
        (vec!["Ballard", "Ballard"], "a headsign twice"),
    ] {
        let error = try_detail(
            &state,
            &lines_detail("L44", &headsigns, "P1", june_15(11, 30)),
        )
        .expect_err(why);
        assert!(
            matches!(error, PlanError::InvalidRequest(_)),
            "{why}: {error:?}"
        );
    }
}

#[test]
fn the_json_of_a_shared_page_has_what_a_row_needs() {
    let state = state();
    let request = NearbyRequest {
        max_routes: Some(3),
        ..request(june_15(11, 30))
    };
    let response = run(&state, &request);
    assert_eq!(
        listed(&response),
        vec!["lines:L44", "lines:L22", "lines:L62"],
        "route 44's 11:40 from P1, then route 22's 11:44, then route 62's 11:50"
    );
    assert_json_snapshot!("nearby_shared_page", &response);
}

#[test]
fn a_trip_whose_headsign_changes_along_its_route_is_on_one_page() {
    let state = state();
    let response = run(&state, &request(june_15(11, 0)));
    let r62 = route(&response, LINES, "L62");
    // Its trips read "Downtown via Fremont" at `P1` and "Downtown" at `P2`, so the headsigns
    // never share a stop to go the same way from; but they're the same trips, so one page, at
    // `P1`, under what a rider reads there.
    assert_eq!(pages(r62), vec![vec!["Downtown via Fremont"]]);
    let only = &r62.pages[0];
    assert_eq!(
        (only.stop.stop_id.as_str(), only.walk_seconds),
        ("P1", WALK_N1)
    );
    assert_eq!(
        rows(only),
        vec![
            ("L62-1110", june_15(11, 10), SCHEDULED),
            ("L62-1150", june_15(11, 50), SCHEDULED),
        ]
    );
    assert!(
        only.departures
            .iter()
            .all(|d| d.headsign.as_deref() == Some("Downtown via Fremont"))
    );
    // The page's detail is asked for by its own headsigns.
    let detail = detail(
        &state,
        &lines_detail("L62", &["Downtown via Fremont"], "P1", june_15(11, 0)),
    );
    let trips: Vec<&str> = detail
        .departures
        .iter()
        .map(|d| d.trip_id.as_str())
        .collect();
    assert_eq!(trips, vec!["L62-1110", "L62-1150"]);
}

#[test]
fn pages_that_share_trips_are_one_page_transitively() {
    let state = state();
    let response = run(&state, &request(june_15(11, 0)));
    let r71 = route(&response, LINES, "L71");
    // "Alpha" and "Beta" share the 11:10, and "Beta" and "Gamma" the 11:20; "Alpha" and "Gamma"
    // share none, and no two go the same way from a stop all of them serve. Still one page:
    // at `P1`, where the earliest trip is caught, listing what leaves there.
    assert_eq!(pages(r71), vec![vec!["Alpha"]]);
    let only = &r71.pages[0];
    assert_eq!(only.stop.stop_id, "P1");
    assert_eq!(rows(only), vec![("L71-1110", june_15(11, 10), SCHEDULED)]);
}
