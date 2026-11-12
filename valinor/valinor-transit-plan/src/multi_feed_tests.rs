//! Several feeds loaded at once: numbering, dispatch, clocks, and per-feed overlays.
//!
//! All feeds use the same stop, route, and trip IDs, so any dispatch across files shows up.

use core::fmt::Write as _;
use std::sync::Arc;

use serde_json::Value;
use valinor_gtfs::{FeedId as CompileId, FeedInput, InMemorySource, LatLon, LoadedTimetable};
use valinor_gtfs::{TimetableSet, compile};
use valinor_gtfs_rt::proto::alert::Effect;
use valinor_gtfs_rt::{FeedKind, proto};
use valinor_transit::{Router, StopId};

use crate::plan::{TransitRequest, plan};
use crate::response::{FeedState, Journey, JourneyLeg, PlanResponse, RealtimeLevel, TransitLeg};
use crate::state::{FeedUpdate, StateConfig, TransitState};
use crate::testing::{
    JUNE_15, StraightLineAccess, StraightLineTransfers, alert_entity, delay_update, encoded_feed,
    entity, june_15, route_selector,
};

const FEED_A: &str = "alpha";
const FEED_B: &str = "bravo";
const FEED_C: &str = "charlie";
const PACIFIC: &str = "America/Los_Angeles";
const EASTERN: &str = "America/New_York";
const FEED_TIME: i64 = 1_781_535_000; // 2026-06-15 08:10 PDT.

struct StopRow(&'static str, f64, f64);

fn source(tz: &str, stops: &[StopRow], trips: &[(&str, &str, &str)]) -> InMemorySource {
    let mut stops_csv = String::from("stop_id,stop_name,stop_lat,stop_lon\n");
    for StopRow(id, lat, lon) in stops {
        stops_csv.push_str(id);
        stops_csv.push(',');
        writeln!(stops_csv, "{id} stop,{lat},{lon}").expect("to a string");
    }
    let mut trips_csv = String::from("route_id,service_id,trip_id,direction_id\n");
    let mut times = String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence\n");
    for (id, depart, arrive) in trips {
        writeln!(trips_csv, "R1,WK,{id},0").expect("to a string");
        write!(
            times,
            "{id},{depart},{depart},S1,1\n{id},{arrive},{arrive},S2,2\n"
        )
        .expect("to a string");
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
        .with("stops.txt", &stops_csv)
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,\
             start_date,end_date\nWK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with("trips.txt", &trips_csv)
        .with("stop_times.txt", &times)
}

fn feed_a_source() -> InMemorySource {
    source(
        PACIFIC,
        &[StopRow("S1", 47.60, -122.33), StopRow("S2", 47.62, -122.33)],
        &[
            ("T1", "08:00:00", "08:20:00"),
            ("T2", "08:30:00", "08:50:00"),
        ],
    )
}

fn feed_b_source() -> InMemorySource {
    source(
        PACIFIC,
        &[
            StopRow("S1", 47.6204, -122.33),
            StopRow("S2", 47.6204, -122.31),
        ],
        &[
            ("T1", "08:40:00", "09:00:00"),
            ("T2", "09:10:00", "09:30:00"),
        ],
    )
}

/// A feed whose agency is in another time zone: its `S1` is steps from the end of alpha's line.
fn feed_c_source() -> InMemorySource {
    source(
        EASTERN,
        &[
            StopRow("S1", 47.6204, -122.33),
            StopRow("S2", 47.65, -122.33),
        ],
        // 09:40 Pacific is 12:40 Eastern.
        &[("T1", "12:40:00", "13:00:00")],
    )
}

fn loaded(id: &str, source: InMemorySource) -> LoadedTimetable {
    let compiled = compile(vec![FeedInput {
        id: CompileId::try_new(id.to_owned()).expect("valid id"),
        source: Box::new(source),
    }])
    .expect("compiles");
    LoadedTimetable::from_bytes(compiled.bytes).expect("loads")
}

fn state_of(files: Vec<LoadedTimetable>) -> TransitState {
    TransitState::from_set(
        TimetableSet::new(files).expect("set"),
        StateConfig::default(),
    )
    .expect("state")
}

fn origin() -> LatLon {
    LatLon {
        lat: 47.5995,
        lon: -122.33,
    }
}

fn destination() -> LatLon {
    LatLon {
        lat: 47.6204,
        lon: -122.3098,
    }
}

fn request(to: LatLon) -> TransitRequest {
    TransitRequest::new(origin(), to, june_15(7, 55), june_15(7, 54))
}

fn run(state: &TransitState, request: &TransitRequest) -> PlanResponse {
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
    .expect("plan succeeds")
}

fn transit_legs(journey: &Journey) -> Vec<&TransitLeg> {
    journey
        .legs
        .iter()
        .filter_map(|leg| match leg {
            JourneyLeg::Transit(leg) => Some(leg.as_ref()),
            _ => None,
        })
        .collect()
}

fn trips_of(journey: &Journey) -> Vec<(String, String)> {
    transit_legs(journey)
        .iter()
        .map(|leg| {
            (
                leg.trip.feed.clone().unwrap_or_default(),
                leg.trip.trip_id.clone(),
            )
        })
        .collect()
}

fn ingest(state: &TransitState, feed: &str, entities: Vec<proto::FeedEntity>) {
    let bytes = encoded_feed(FEED_TIME, entities);
    state
        .ingest(FeedUpdate {
            feed,
            kind: FeedKind::TripUpdates,
            bytes: Some(&bytes),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
}

#[test]
fn a_journey_can_board_two_feeds_with_a_walk_between_them() {
    let state = state_of(vec![
        loaded(FEED_A, feed_a_source()),
        loaded(FEED_B, feed_b_source()),
    ]);
    let response = run(&state, &request(destination()));
    let journey = response
        .journeys
        .iter()
        .find(|journey| journey.trips == 2)
        .expect("a two-trip journey");
    assert_eq!(
        trips_of(journey),
        vec![
            (FEED_A.to_owned(), "T1".to_owned()),
            (FEED_B.to_owned(), "T1".to_owned())
        ]
    );
    let legs = transit_legs(journey);
    assert_eq!(
        legs[0].stops.last().map(|s| s.stop.stop_id.as_str()),
        Some("S2")
    );
    assert_eq!(
        legs[1].stops.first().map(|s| s.stop.stop_id.as_str()),
        Some("S1")
    );
    assert!(
        journey
            .legs
            .iter()
            .any(|leg| matches!(leg, JourneyLeg::Transfer(leg) if leg.from.stop_id == "S2" && leg.to.stop_id == "S1")),
        "a transfer joins the end of one feed's line to the start of the other's"
    );
}

/// What a set of files answered, without the things a different loading can legitimately change.
fn comparable(response: &PlanResponse) -> Value {
    let mut value = serde_json::to_value(response).expect("serializable");
    value.as_object_mut().expect("an object").remove("realtime");
    value
}

#[test]
fn a_subset_of_feeds_answers_like_a_file_of_only_those_feeds() {
    let both = compile(vec![
        FeedInput {
            id: CompileId::try_new(FEED_A.to_owned()).unwrap(),
            source: Box::new(feed_a_source()),
        },
        FeedInput {
            id: CompileId::try_new(FEED_B.to_owned()).unwrap(),
            source: Box::new(feed_b_source()),
        },
    ])
    .expect("compiles")
    .bytes;
    let combined = state_of(vec![LoadedTimetable::from_bytes(both).unwrap()]);
    let separate = state_of(vec![
        loaded(FEED_A, feed_a_source()),
        loaded(FEED_B, feed_b_source()),
    ]);
    let with_extra = state_of(vec![
        loaded(FEED_C, feed_c_source()),
        loaded(FEED_A, feed_a_source()),
        loaded(FEED_B, feed_b_source()),
    ]);
    let request = request(destination());
    let answer = comparable(&run(&combined, &request));
    assert_eq!(comparable(&run(&separate, &request)), answer);
    // A third feed elsewhere changes numbering but no answer.
    assert_eq!(comparable(&run(&with_extra, &request)), answer);

    // Loading a feed whose stops are elsewhere doesn't change one feed's own answers.
    let beside_another = state_of(vec![
        loaded(FEED_A, feed_a_source()),
        loaded(FEED_C, feed_c_source()),
    ]);
    let alone = state_of(vec![loaded(FEED_A, feed_a_source())]);
    let to_a = request_to_a();
    assert_eq!(
        comparable(&run(&beside_another, &to_a)),
        comparable(&run(&alone, &to_a))
    );
}

fn request_to_a() -> TransitRequest {
    TransitRequest::new(
        origin(),
        LatLon {
            lat: 47.62,
            lon: -122.3302,
        },
        june_15(7, 55),
        june_15(7, 54),
    )
}

#[test]
fn a_feed_in_another_time_zone_keeps_its_own_clock() {
    let state = state_of(vec![
        loaded(FEED_A, feed_a_source()),
        loaded(FEED_C, feed_c_source()),
    ]);
    let to = LatLon {
        lat: 47.65,
        lon: -122.33,
    };
    let response = run(
        &state,
        &TransitRequest::new(origin(), to, june_15(7, 55), june_15(7, 54)),
    );
    let journey = response
        .journeys
        .iter()
        .find(|journey| journey.trips == 2)
        .expect("a journey across zones");
    let legs = transit_legs(journey);
    assert_eq!(legs[1].trip.feed.as_deref(), Some(FEED_C));
    // 12:40 Eastern is 09:40 Pacific, at the same instant.
    assert_eq!(legs[1].departure, june_15(9, 40));
    assert_eq!(legs[1].arrival, june_15(10, 0));
    let first = legs[1].stops.first().expect("a stop");
    assert_eq!(first.scheduled_departure, Some(june_15(9, 40)));
    assert_eq!(first.utc_offset_seconds, Some(-4 * 3600));
    assert_eq!(
        transit_legs(journey)[0]
            .stops
            .first()
            .unwrap()
            .utc_offset_seconds,
        Some(-7 * 3600)
    );

    // A delay on the other feed moves that feed's own trip on the right clock.
    ingest(&state, FEED_C, vec![delay_update("T1", JUNE_15, 1, 300)]);
    let response = run(
        &state,
        &TransitRequest::new(origin(), to, june_15(7, 55), june_15(7, 54)),
    );
    let journey = response
        .journeys
        .iter()
        .find(|journey| journey.trips == 2)
        .expect("the journey still exists");
    let legs = transit_legs(journey);
    assert_eq!(legs[1].departure, june_15(9, 45));
    assert_eq!(legs[1].delay_seconds, Some(300));
}

fn three_feed_state() -> TransitState {
    state_of(vec![
        loaded(FEED_A, feed_a_source()),
        loaded(FEED_B, feed_b_source()),
        loaded(FEED_C, feed_c_source()),
    ])
}

fn canceled(trip: &str) -> proto::FeedEntity {
    let mut item = entity(&format!("cancel-{trip}"));
    item.trip_update = Some(proto::TripUpdate {
        trip: proto::TripDescriptor {
            trip_id: Some(trip.to_owned()),
            start_date: Some(JUNE_15.to_owned()),
            schedule_relationship: Some(
                proto::trip_descriptor::ScheduleRelationship::Canceled as i32,
            ),
            ..proto::TripDescriptor::default()
        },
        ..proto::TripUpdate::default()
    });
    item
}

#[test]
fn a_delay_applies_only_to_its_feeds_trip() {
    let state = three_feed_state();
    // `T1` exists in every feed; only bravo's should move.
    ingest(&state, FEED_B, vec![delay_update("T1", JUNE_15, 1, 240)]);
    let response = run(&state, &request(destination()));
    let journey = response
        .journeys
        .iter()
        .find(|journey| trips_of(journey).first().is_some_and(|(_, t)| t == "T1"))
        .expect("the first departure");
    let legs = transit_legs(journey);
    assert!(legs[0].delay_seconds.is_none(), "alpha's T1 is unaffected");
    assert_eq!(legs[0].departure, june_15(8, 0));
    assert_eq!(legs[1].delay_seconds, Some(240));
    assert_eq!(legs[1].departure, june_15(8, 44));
}

#[test]
fn a_cancellation_removes_only_its_feeds_trip() {
    let state = three_feed_state();
    ingest(&state, FEED_B, vec![canceled("T1")]);
    let response = run(&state, &request(destination()));
    for journey in &response.journeys {
        let trips = trips_of(journey);
        assert!(
            !trips
                .iter()
                .any(|(feed, trip)| feed == FEED_B && trip == "T1")
        );
    }
    let to_a = run(&state, &request_to_a());
    assert!(
        to_a.journeys
            .iter()
            .any(|journey| trips_of(journey) == [(FEED_A.to_owned(), "T1".to_owned())]),
        "alpha's T1 still runs"
    );
}

#[test]
fn a_no_service_alert_affects_only_its_feed() {
    let state = three_feed_state();
    let bytes = encoded_feed(
        FEED_TIME,
        vec![alert_entity(
            "gone",
            Effect::NoService,
            vec![route_selector("R1")],
        )],
    );
    state
        .ingest(FeedUpdate {
            feed: FEED_B,
            kind: FeedKind::Alerts,
            bytes: Some(&bytes),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let response = run(&state, &request(destination()));
    assert!(
        response
            .journeys
            .iter()
            .all(|j| !trips_of(j).iter().any(|(f, _)| f == FEED_B)),
        "bravo's route R1 has no service"
    );
    let to_a = run(&state, &request_to_a());
    assert!(
        to_a.journeys
            .iter()
            .any(|j| trips_of(j).iter().any(|(f, _)| f == FEED_A)),
        "alpha's route R1, a different route, still runs"
    );
}

fn feed_state(response: &PlanResponse, feed: &str) -> FeedState {
    response
        .realtime
        .feeds
        .iter()
        .find(|report| report.feed == feed)
        .map(|report| report.state)
        .expect("a report for this feed")
}

#[test]
fn a_stale_feed_is_confined_to_itself() {
    let state = three_feed_state();
    ingest(&state, FEED_A, vec![delay_update("T1", JUNE_15, 1, 120)]);
    // Bravo's message is from long ago; its delay must not apply.
    let old = encoded_feed(june_15(2, 0), vec![delay_update("T1", JUNE_15, 1, 999)]);
    state
        .ingest(FeedUpdate {
            feed: FEED_B,
            kind: FeedKind::TripUpdates,
            bytes: Some(&old),
            received_at: june_15(2, 0),
            stale_after: None,
        })
        .expect("known feed");
    let response = run(&state, &request(destination()));
    assert_eq!(feed_state(&response, FEED_A), FeedState::Applied);
    assert_eq!(feed_state(&response, FEED_B), FeedState::Stale);
    assert_eq!(response.realtime.level, RealtimeLevel::Degraded);
    let journey = response
        .journeys
        .iter()
        .find(|journey| trips_of(journey).first().is_some_and(|(_, t)| t == "T1"))
        .expect("a journey");
    assert_eq!(transit_legs(journey)[0].delay_seconds, Some(120));
    assert!(transit_legs(journey)[1].delay_seconds.is_none());
}

#[test]
fn an_undecodable_feed_is_confined_to_itself() {
    let state = three_feed_state();
    ingest(&state, FEED_A, vec![delay_update("T1", JUNE_15, 1, 120)]);
    state
        .ingest(FeedUpdate {
            feed: FEED_B,
            kind: FeedKind::TripUpdates,
            bytes: Some(&[0xff, 0xff, 0xff]),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let response = run(&state, &request(destination()));
    assert_eq!(feed_state(&response, FEED_B), FeedState::Undecodable);
    assert_eq!(feed_state(&response, FEED_A), FeedState::Applied);
    let journey = response
        .journeys
        .iter()
        .find(|journey| trips_of(journey).first().is_some_and(|(_, t)| t == "T1"))
        .expect("a journey");
    assert_eq!(transit_legs(journey)[0].delay_seconds, Some(120));
}

#[test]
fn an_unknown_feed_is_refused_at_ingest() {
    let state = three_feed_state();
    let error = state
        .ingest(FeedUpdate {
            feed: "delta",
            kind: FeedKind::Alerts,
            bytes: None,
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect_err("unknown");
    assert!(matches!(error, crate::error::StateError::UnknownFeed(_)));
}

#[test]
fn new_overlay_stops_number_after_all_base_stops() {
    let state = three_feed_state();
    let mut item = entity("new-stop");
    item.stop = Some(proto::Stop {
        stop_id: Some("RTONLY".to_owned()),
        stop_lat: Some(47.61),
        stop_lon: Some(-122.33),
        ..proto::Stop::default()
    });
    ingest(&state, FEED_B, vec![item]);
    let snapshot = state.pin(june_15(7, 54)).expect("pin");
    let base = state.feeds().len();
    assert_eq!(base, 3);
    let new_stops = snapshot.new_stops();
    assert_eq!(new_stops.len(), 1);
    assert_eq!(
        new_stops[0].stop,
        StopId::new(6),
        "two stops in each of three files"
    );
    assert_eq!(snapshot.snapshot().stop_count(), 7);
    // Numbering of static stops is unchanged, and locations include the new one.
    assert_eq!(snapshot.stop_locations().count(), 7);
    // The new stop belongs to no file.
    assert!(snapshot.data.set.stop_part(new_stops[0].stop).is_none());
}

/// A snapshot is shared: its files stay usable from another thread.
#[test]
fn a_snapshot_is_shared_across_threads() {
    let state = Arc::new(three_feed_state());
    let snapshot = state.pin(june_15(7, 54)).expect("pin");
    let handle = std::thread::spawn({
        let state = Arc::clone(&state);
        move || run(&state, &request(destination())).journeys.len()
    });
    assert!(handle.join().expect("no panic") > 0);
    drop(snapshot);
}
