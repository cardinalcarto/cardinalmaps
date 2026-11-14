//! A route detail's ride through a vehicle's stay-seated continuations, on a synthetic feed.
//!
//! One vehicle runs block `K` on Monday 2026-06-15, in Seattle's time zone, north from `C1`:
//!
//! - route 7's `K7-0900` "Alpha", `C1` 09:00 to `C2` 09:05, on shape `S7`, which bends between;
//! - route 8's `K8-0907` "Beta", `C2` 09:07 through `C3` 09:12 to `C4` 09:15, on shape `S8`,
//!   which bends between `C3` and `C4`;
//! - route 9's `K9-0918` "Gamma", `C4` 09:18 through `C5` 09:25 to `C6` 09:30, with no shape.
//!
//! Each wait is within the compiler's ten minutes, so the three trips are one combined trip.
//! Route 7's `K7-1000` "Alpha" runs alone, at 10:00, from `C1` to `C2`.

use core::convert::Infallible;

use insta::assert_json_snapshot;
use valinor_gtfs::{FeedId as CompileFeedId, FeedInput, InMemorySource, LoadedTimetable};
use valinor_gtfs::{TimetableSet, compile};
use valinor_gtfs_rt::{FeedKind, proto};

use crate::error::PlanError;
use crate::nearby::{RouteDetailRequest, TripChoice};
use crate::plan::TravelMode;
use crate::response::{ContinuesAs, RouteDetailResponse, TransitLeg};
use crate::state::{FeedUpdate, StateConfig, TransitState};
use crate::testing::{
    alert_entity, delay_update, descriptor, encoded_feed, entity, june_15, vehicle_entity,
};

const CHAIN: &str = "chain";
/// 2026-06-15 07:50 PDT.
const FEED_TIME: i64 = 1_781_535_000;
const JUNE_15: &str = "20260615";

fn chain_source() -> InMemorySource {
    InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\n\
             CH,Chain,America/Los_Angeles,https://c.test\n",
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_type,route_color,route_text_color\n\
             K7,CH,7,3,0000FF,FFFFFF\n\
             K8,CH,8,3,FF0000,FFFFFF\n\
             K9,CH,9,3,,\n",
        )
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon\n\
             C1,Chain 1st,47.6000,-122.3300\n\
             C2,Chain 2nd,47.6100,-122.3300\n\
             C3,Chain 3rd,47.6200,-122.3300\n\
             C4,Chain 4th,47.6300,-122.3300\n\
             C5,Chain 5th,47.6400,-122.3300\n\
             C6,Chain 6th,47.6500,-122.3300\n",
        )
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,trip_headsign,direction_id,block_id,shape_id\n\
             K7,WK,K7-0900,Alpha,0,K,S7\n\
             K8,WK,K8-0907,Beta,0,K,S8\n\
             K9,WK,K9-0918,Gamma,0,K,\n\
             K7,WK,K7-1000,Alpha,0,,S7\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             K7-0900,09:00:00,09:00:00,C1,1\n\
             K7-0900,09:05:00,09:05:00,C2,2\n\
             K8-0907,09:07:00,09:07:00,C2,1\n\
             K8-0907,09:12:00,09:12:00,C3,2\n\
             K8-0907,09:15:00,09:15:00,C4,3\n\
             K9-0918,09:18:00,09:18:00,C4,1\n\
             K9-0918,09:25:00,09:25:00,C5,2\n\
             K9-0918,09:30:00,09:30:00,C6,3\n\
             K7-1000,10:00:00,10:00:00,C1,1\n\
             K7-1000,10:05:00,10:05:00,C2,2\n",
        )
        .with(
            "shapes.txt",
            "shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\n\
             S7,47.6000,-122.3300,1\n\
             S7,47.6050,-122.3310,2\n\
             S7,47.6100,-122.3300,3\n\
             S8,47.6100,-122.3300,1\n\
             S8,47.6200,-122.3300,2\n\
             S8,47.6250,-122.3290,3\n\
             S8,47.6300,-122.3300,4\n",
        )
}

fn state() -> TransitState {
    let compiled = compile(vec![FeedInput {
        id: CompileFeedId::try_new(CHAIN.to_owned()).expect("valid id"),
        source: Box::new(chain_source()),
    }])
    .expect("compiles");
    let set = TimetableSet::new(vec![
        LoadedTimetable::from_bytes(compiled.bytes).expect("loads"),
    ])
    .expect("a set");
    TransitState::from_set(set, StateConfig::default()).expect("state")
}

fn ingest(state: &TransitState, kind: FeedKind, entities: Vec<proto::FeedEntity>) {
    let bytes = encoded_feed(FEED_TIME, entities);
    state
        .ingest(FeedUpdate {
            feed: CHAIN,
            kind,
            bytes: Some(&bytes),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
}

fn with_relationship(
    trip: &str,
    relationship: proto::trip_descriptor::ScheduleRelationship,
    stop_time_update: Vec<proto::trip_update::StopTimeUpdate>,
) -> proto::FeedEntity {
    let mut item = entity(&format!("tu-{trip}"));
    item.trip_update = Some(proto::TripUpdate {
        trip: proto::TripDescriptor {
            schedule_relationship: Some(relationship as i32),
            ..descriptor(trip, JUNE_15, None)
        },
        stop_time_update,
        ..proto::TripUpdate::default()
    });
    item
}

/// `trip` canceled.
fn canceled(trip: &str) -> proto::FeedEntity {
    with_relationship(
        trip,
        proto::trip_descriptor::ScheduleRelationship::Canceled,
        Vec::new(),
    )
}

/// `K8-0907` replaced by a trip of its own over the same stops, a minute late.
fn k8_replaced() -> proto::FeedEntity {
    let at = |stop: &str, unix: i64| proto::trip_update::StopTimeUpdate {
        stop_id: Some(stop.to_owned()),
        arrival: Some(proto::trip_update::StopTimeEvent {
            time: Some(unix),
            ..proto::trip_update::StopTimeEvent::default()
        }),
        departure: Some(proto::trip_update::StopTimeEvent {
            time: Some(unix),
            ..proto::trip_update::StopTimeEvent::default()
        }),
        ..proto::trip_update::StopTimeUpdate::default()
    };
    with_relationship(
        "K8-0907",
        proto::trip_descriptor::ScheduleRelationship::Replacement,
        vec![
            at("C2", june_15(9, 8)),
            at("C3", june_15(9, 13)),
            at("C4", june_15(9, 16)),
        ],
    )
}

fn request(route: &str, headsigns: &[&str], stop: &str) -> RouteDetailRequest {
    RouteDetailRequest {
        feed: CHAIN.to_owned(),
        route_id: route.to_owned(),
        headsigns: headsigns.iter().map(|&h| h.to_owned()).collect(),
        stop_id: stop.to_owned(),
        time: june_15(8, 55),
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

/// Route 7's detail from `C1`, on the 09:00.
fn alpha(state: &TransitState) -> RouteDetailResponse {
    detail(state, &request("K7", &["Alpha"], "C1"))
}

fn ride(response: &RouteDetailResponse) -> &TransitLeg {
    response.ride.as_ref().expect("a ride")
}

fn stops(ride: &TransitLeg) -> Vec<&str> {
    ride.stops.iter().map(|s| s.stop.stop_id.as_str()).collect()
}

/// A ride's segment as `(trip_id, route_id, headsign, first stop, last stop)`.
pub(crate) type SegmentRow<'a> = (&'a str, Option<&'a str>, Option<&'a str>, usize, usize);

/// Each of a ride's segments.
pub(crate) fn segments(ride: &TransitLeg) -> Vec<SegmentRow<'_>> {
    ride.segments
        .iter()
        .map(|s| {
            (
                s.trip_id.as_str(),
                s.route.as_ref().map(|r| r.route_id.as_str()),
                s.headsign.as_deref(),
                s.first_stop_index,
                s.last_stop_index,
            )
        })
        .collect()
}

/// Each stop's `(arrival, departure)`.
fn times(ride: &TransitLeg) -> Vec<(i64, i64)> {
    ride.stops
        .iter()
        .map(|s| (s.arrival, s.departure))
        .collect()
}

/// Decodes a polyline6 to `(lat, lon)` pairs in millionths of a degree.
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

/// The shape's points, with a point repeated where two stretches meet left once.
fn shape(ride: &TransitLeg) -> Vec<(i64, i64)> {
    let mut points = polyline6(ride.shape.as_deref().expect("a shape"));
    points.dedup();
    points
}

#[test]
fn a_ride_goes_on_through_a_three_trip_chain_in_segments() {
    let state = state();
    let response = alpha(&state);
    let ride = ride(&response);
    // The trip boarded, its route, and its departures are route 7's.
    assert_eq!(ride.trip.trip_id, "K7-0900");
    assert_eq!(ride.route.as_ref().map(|r| r.route_id.as_str()), Some("K7"));
    let departures: Vec<&str> = response
        .departures
        .iter()
        .map(|d| d.trip_id.as_str())
        .collect();
    assert_eq!(departures, vec!["K7-0900", "K7-1000"]);

    // On to `C6`, each join one stop.
    assert_eq!(stops(ride), vec!["C1", "C2", "C3", "C4", "C5", "C6"]);
    assert_eq!(
        segments(ride),
        vec![
            ("K7-0900", Some("K7"), Some("Alpha"), 0, 1),
            ("K8-0907", Some("K8"), Some("Beta"), 1, 3),
            ("K9-0918", Some("K9"), Some("Gamma"), 3, 5),
        ]
    );
    // A join arrives as the first trip and leaves as the next.
    assert_eq!(
        times(ride),
        vec![
            (june_15(9, 0), june_15(9, 0)),
            (june_15(9, 5), june_15(9, 7)),
            (june_15(9, 12), june_15(9, 12)),
            (june_15(9, 15), june_15(9, 18)),
            (june_15(9, 25), june_15(9, 25)),
            (june_15(9, 30), june_15(9, 30)),
        ]
    );
    assert_eq!(
        (ride.departure, ride.arrival),
        (june_15(9, 0), june_15(9, 30))
    );
    assert!(!ride.realtime);

    // Each trip's own shape, and straight from stop to stop where it has none.
    assert_eq!(
        shape(ride),
        vec![
            micro(47.60, -122.33),
            micro(47.605, -122.331),
            micro(47.61, -122.33),
            micro(47.62, -122.33),
            micro(47.625, -122.329),
            micro(47.63, -122.33),
            micro(47.64, -122.33),
            micro(47.65, -122.33),
        ]
    );
    // Nothing before the trip's first stop; the departure still says what it goes on as.
    assert_eq!(response.shape_to_stop, None);
    assert_eq!(
        response.departures[0].continues_as,
        Some(ContinuesAs {
            route_id: "K8".to_owned(),
            short_name: Some("8".to_owned()),
            headsign: Some("Beta".to_owned()),
        })
    );

    // A trip that runs alone ends where it does.
    let alone = detail(
        &state,
        &RouteDetailRequest {
            trip: Some(TripChoice {
                trip_id: "K7-1000".to_owned(),
                start_date: Some(JUNE_15.to_owned()),
            }),
            ..request("K7", &["Alpha"], "C1")
        },
    );
    let alone = alone.ride.as_ref().expect("a ride");
    assert_eq!(stops(alone), vec!["C1", "C2"]);
    assert!(alone.segments.is_empty());
}

#[test]
fn a_ride_boarded_on_a_later_trip_of_a_chain_goes_on_from_there() {
    let state = state();
    let response = detail(&state, &request("K8", &["Beta"], "C3"));
    let ride = ride(&response);
    assert_eq!(ride.trip.trip_id, "K8-0907");
    assert_eq!(stops(ride), vec!["C3", "C4", "C5", "C6"]);
    assert_eq!(
        segments(ride),
        vec![
            ("K8-0907", Some("K8"), Some("Beta"), 0, 1),
            ("K9-0918", Some("K9"), Some("Gamma"), 1, 3),
        ]
    );
    // Before the stop, only route 8's own trip, from `C2`: not the route 7 trip before it.
    assert_eq!(
        response.shape_to_stop.as_deref().map(polyline6),
        Some(vec![micro(47.61, -122.33), micro(47.62, -122.33)])
    );

    // On the chain's last trip, a ride on one trip.
    let last = detail(&state, &request("K9", &["Gamma"], "C4"));
    let last = self::ride(&last);
    assert_eq!(stops(last), vec!["C4", "C5", "C6"]);
    assert!(last.segments.is_empty());
}

#[test]
fn a_ride_ends_where_its_vehicle_stops_carrying_riders() {
    // The middle trip canceled: the vehicle goes no further than route 7's end, though
    // route 9's trip runs.
    let state = state();
    ingest(&state, FeedKind::TripUpdates, vec![canceled("K8-0907")]);
    let response = alpha(&state);
    let ride = ride(&response);
    assert_eq!(stops(ride), vec!["C1", "C2"]);
    assert!(ride.segments.is_empty());
    assert_eq!(ride.arrival, june_15(9, 5));
    assert_eq!(response.departures[0].continues_as, None);

    // The last canceled: on as route 8, to its end.
    let state = self::state();
    ingest(&state, FeedKind::TripUpdates, vec![canceled("K9-0918")]);
    let response = alpha(&state);
    let ride = self::ride(&response);
    assert_eq!(stops(ride), vec!["C1", "C2", "C3", "C4"]);
    assert_eq!(
        segments(ride),
        vec![
            ("K7-0900", Some("K7"), Some("Alpha"), 0, 1),
            ("K8-0907", Some("K8"), Some("Beta"), 1, 3),
        ]
    );
    assert_eq!(ride.arrival, june_15(9, 15));

    // The middle replaced by a trip of its own: nothing says the vehicle stays the same.
    let state = self::state();
    ingest(&state, FeedKind::TripUpdates, vec![k8_replaced()]);
    let response = alpha(&state);
    let ride = self::ride(&response);
    assert_eq!(stops(ride), vec!["C1", "C2"]);
    assert_eq!(response.departures[0].continues_as, None);

    // A `NO_SERVICE` alert on the last trip removes its stretch as a cancellation does.
    let state = self::state();
    ingest(
        &state,
        FeedKind::Alerts,
        vec![alert_entity(
            "k9-gone",
            proto::alert::Effect::NoService,
            vec![proto::EntitySelector {
                trip: Some(descriptor("K9-0918", JUNE_15, None)),
                ..proto::EntitySelector::default()
            }],
        )],
    );
    let response = alpha(&state);
    assert_eq!(stops(self::ride(&response)), vec!["C1", "C2", "C3", "C4"]);
}

#[test]
fn a_later_trips_real_time_data_applies_to_its_own_segment() {
    let state = state();
    // Route 8's trip two minutes late at `C3`.
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![delay_update("K8-0907", JUNE_15, 2, 120)],
    );
    let response = alpha(&state);
    let ride = ride(&response);
    assert_eq!(stops(ride), vec!["C1", "C2", "C3", "C4", "C5", "C6"]);
    assert!(ride.realtime, "the ride has real-time times");
    let predicted: Vec<(Option<i64>, Option<i64>)> = ride
        .stops
        .iter()
        .map(|s| (s.predicted_arrival, s.predicted_departure))
        .collect();
    assert_eq!(
        predicted,
        vec![
            // Route 7's trip has no update.
            (None, None),
            (None, None),
            // Route 8's from `C3` on.
            (Some(june_15(9, 14)), Some(june_15(9, 14))),
            // The join: route 8's arrival, and route 9's departure, which has no update of
            // its own and takes the vehicle's delay on.
            (Some(june_15(9, 17)), Some(june_15(9, 20))),
            // Route 9's, its scheduled running times kept.
            (Some(june_15(9, 27)), Some(june_15(9, 27))),
            (Some(june_15(9, 32)), Some(june_15(9, 32))),
        ]
    );
    // Boarding on time, arriving late.
    assert_eq!(ride.departure, june_15(9, 0));
    assert_eq!(ride.delay_seconds, None);
    assert_eq!(ride.arrival, june_15(9, 32));
}

#[test]
fn a_later_trips_vehicle_is_the_rides() {
    let state = state();
    // The vehicle already reports route 8's trip.
    ingest(
        &state,
        FeedKind::VehiclePositions,
        vec![vehicle_entity(
            "4401", "K8-0907", JUNE_15, None, 47.605, -122.33,
        )],
    );
    let response = alpha(&state);
    assert_eq!(
        ride(&response)
            .vehicle
            .as_ref()
            .and_then(|v| v.id.as_deref()),
        Some("4401")
    );
    // Only route 7's vehicles are the page's.
    assert!(response.vehicles.is_empty());
}

#[test]
fn the_json_of_a_ride_through_a_chain() {
    let state = state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![delay_update("K8-0907", JUNE_15, 2, 120)],
    );
    let response = detail(
        &state,
        &RouteDetailRequest {
            max_departures: 1,
            ..request("K7", &["Alpha"], "C1")
        },
    );
    assert_json_snapshot!("route_detail_stay_seated_ride", &response.ride);
}
