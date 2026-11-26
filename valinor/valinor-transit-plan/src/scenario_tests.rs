//! Scenarios: the snapshot lifecycle, one test per transition, and response building.

use core::convert::Infallible;

use insta::assert_json_snapshot;
use proptest::prelude::*;
use valinor_gtfs::{
    FeedId as CompileFeedId, FeedInput, InMemorySource, LatLon, LoadedTimetable, compile,
};
use valinor_gtfs_rt::proto::alert::Effect;
use valinor_gtfs_rt::{FeedId, FeedKind, StaticLookup, TripQuery};
use valinor_transit::{RouteAttributes, RouteFilter, Router, StopId};

use crate::error::{PlanError, StateError};
use crate::lookup::CompiledLookup;
use crate::plan::{
    AccessEgressRequest, AccessEgressSearch, AccessEgressTables, TransitOptions, TransitRequest,
    TravelMode, plan,
};
use crate::response::{FeedState, JourneyLeg, PlanResponse, RealtimeLevel, TransitLeg};
use crate::state::{FeedUpdate, IngestOutcome, StateConfig, TransitSnapshot, TransitState};
use crate::testing::{
    FEED, JUNE_15, JUNE_16, StraightLineAccess, StraightLineTransfers, StraightWalk, alert_entity,
    delay_update, encoded_feed, june_15, route_selector, route_stop_selector, stop_selector,
    synthetic_state, vehicle_entity,
};
use valinor_gtfs_rt::proto;

const FEED_TIME: i64 = 1_781_535_000; // 2026-06-15 08:10 PDT.

fn origin() -> LatLon {
    LatLon {
        lat: 47.5995,
        lon: -122.3300,
    }
}

fn destination() -> LatLon {
    LatLon {
        lat: 47.6204,
        lon: -122.3098,
    }
}

/// A request around the morning trips, asked just before they run.
fn morning_request() -> TransitRequest {
    TransitRequest::new(origin(), destination(), june_15(7, 55), june_15(7, 54))
}

fn trip_feed(
    state: &TransitState,
    timestamp: i64,
    entities: Vec<proto::FeedEntity>,
) -> IngestOutcome {
    let bytes = encoded_feed(timestamp, entities);
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::TripUpdates,
            bytes: Some(&bytes),
            received_at: timestamp,
            stale_after: None,
        })
        .expect("known feed")
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

fn transit_legs(journey: &crate::response::Journey) -> Vec<&TransitLeg> {
    journey
        .legs
        .iter()
        .filter_map(|leg| match leg {
            JourneyLeg::Transit(leg) => Some(leg.as_ref()),
            _ => None,
        })
        .collect()
}

/// The `S1`-to-`S6` journey with both rides.
fn two_trip_journey(response: &PlanResponse) -> &crate::response::Journey {
    response
        .journeys
        .iter()
        .find(|journey| {
            transit_legs(journey)
                .iter()
                .map(|leg| leg.trip.trip_id.as_str())
                .eq(["T1", "T3"])
        })
        .expect("the T1-T3 journey exists")
}

fn lookup_of(snapshot: &TransitSnapshot) -> CompiledLookup<'_> {
    CompiledLookup::of(snapshot)
}

fn feed_id() -> FeedId {
    FeedId::new(FEED).expect("non-empty")
}

// --- StaticLookup ---

#[test]
fn lookup_puts_trips_on_the_window_clock() {
    let state = synthetic_state();
    let snapshot = state.pin(june_15(8, 0)).expect("pin");
    let found = lookup_of(&snapshot)
        .trip(
            &feed_id(),
            &TripQuery {
                trip_id: "T1",
                date: crate::window::calendar_date(crate::testing::date(JUNE_15)).expect("date"),
                start_time: None,
            },
        )
        .expect("T1 runs on a weekday");
    // 08:00:30 on the second day of a window whose clock starts a full day before.
    assert_eq!(found.first_departure.into_inner(), 86_400 + 8 * 3600 + 30);
    assert_eq!(found.last_arrival.into_inner(), 86_400 + 8 * 3600 + 30 * 60);
    assert_eq!(found.day.into_inner(), 1);
}

#[test]
fn lookup_rejects_trips_on_days_they_do_not_run() {
    let state = synthetic_state();
    let snapshot = state.pin(june_15(8, 0)).expect("pin");
    let sunday = crate::window::calendar_date(crate::testing::date("20260614")).expect("date");
    let found = lookup_of(&snapshot).trip(
        &feed_id(),
        &TripQuery {
            trip_id: "T1",
            date: sunday,
            start_time: None,
        },
    );
    assert_eq!(found, None);
}

fn resolved_first_departure(snapshot: &TransitSnapshot, trip: &str, start: u32) -> Option<i32> {
    let date = crate::window::calendar_date(crate::testing::date(JUNE_15)).expect("date");
    lookup_of(snapshot)
        .trip(
            &feed_id(),
            &TripQuery {
                trip_id: trip,
                date,
                start_time: Some(start),
            },
        )
        .map(|found| found.first_departure.into_inner() - 86_400)
}

#[test]
fn headway_trips_resolve_to_the_nearest_grid_run() {
    let state = synthetic_state();
    let snapshot = state.pin(june_15(8, 0)).expect("pin");
    // Grid runs at 10:00 and 10:30. A vehicle that started at 10:12 is closest to 10:00.
    assert_eq!(
        resolved_first_departure(&snapshot, "F1", 10 * 3600 + 12 * 60),
        Some(36_000)
    );
    // One that started at 10:20 is closest to 10:30.
    assert_eq!(
        resolved_first_departure(&snapshot, "F1", 10 * 3600 + 20 * 60),
        Some(37_800)
    );
}

#[test]
fn exact_frequency_trips_need_an_exact_start_time() {
    let state = synthetic_state();
    let snapshot = state.pin(june_15(8, 0)).expect("pin");
    assert_eq!(
        resolved_first_departure(&snapshot, "F2", 11 * 3600 + 1800),
        Some(41_400)
    );
    assert_eq!(
        resolved_first_departure(&snapshot, "F2", 11 * 3600 + 600),
        None
    );
}

#[test]
fn stations_list_their_platforms_but_not_themselves() {
    let state = synthetic_state();
    let snapshot = state.pin(june_15(8, 0)).expect("pin");
    let lookup = lookup_of(&snapshot);
    let platforms: Vec<&str> = lookup
        .child_stops(&feed_id(), "STN")
        .into_iter()
        .filter_map(|stop| lookup.stop_gtfs_id(&feed_id(), stop))
        .collect();
    assert_eq!(platforms.len(), 2);
    assert!(platforms.contains(&"P1") && platforms.contains(&"P2"));
    assert_eq!(lookup.child_stops(&feed_id(), "S1"), Vec::<StopId>::new());
}

#[test]
fn scheduled_blocks_agree_with_the_materialized_window() {
    let state = synthetic_state();
    let snapshot = state.pin(june_15(8, 0)).expect("pin");
    let lookup = lookup_of(&snapshot);
    let route = lookup.route_patterns(&feed_id(), "R1")[0];
    let day = valinor_transit::DaySlot::new(1);
    let trips = lookup.scheduled_block(route, day);
    assert_eq!(trips.len(), 2);
    for trip in &trips {
        assert_eq!(trip.events.len(), 4);
        assert_eq!(trip.stop_sequences, [10, 20, 30, 40]);
        assert_eq!(trip.direction_id, Some(0));
    }
    let lanes = snapshot.snapshot().block_lanes(route, day).expect("block");
    let window_events: Vec<_> = lanes
        .iter()
        .flat_map(|lane| {
            let count = snapshot.snapshot().lane_trip_count(*lane).unwrap_or(0);
            (0..count).filter_map(|row| snapshot.snapshot().lane_trip_events(*lane, row))
        })
        .collect();
    assert_eq!(window_events.len(), 2);
    for trip in &trips {
        assert!(window_events.contains(&trip.events));
    }
}

// --- The snapshot lifecycle: one test per transition. ---

#[test]
fn transition_none_to_scheduled_routes_without_any_feed() {
    let state = synthetic_state();
    let response = run(&state, &morning_request());
    assert_eq!(response.realtime.level, RealtimeLevel::Schedule);
    assert!(response.realtime.feeds.is_empty());
    let journey = two_trip_journey(&response);
    let leg = transit_legs(journey)[0];
    assert!(!leg.realtime);
    assert_eq!(leg.delay_seconds, None);
}

#[test]
fn transition_scheduled_to_updated_applies_delays() {
    let state = synthetic_state();
    trip_feed(
        &state,
        FEED_TIME,
        vec![delay_update("T1", JUNE_15, 10, 120)],
    );
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    assert_eq!(response.realtime.level, RealtimeLevel::Realtime);
    assert_eq!(response.realtime.feeds[0].state, FeedState::Applied);
    let journey = two_trip_journey(&response);
    let leg = transit_legs(journey)[0];
    assert!(leg.realtime);
    assert_eq!(leg.delay_seconds, Some(120));
    assert_eq!(
        leg.stops[0].predicted_departure,
        Some(leg.stops[0].scheduled_departure.unwrap() + 120)
    );
    assert_eq!(leg.departure, june_15(8, 0) + 30 + 120);
}

#[test]
fn transition_updated_to_updated_uses_the_newer_message() {
    let state = synthetic_state();
    trip_feed(
        &state,
        FEED_TIME - 30,
        vec![delay_update("T1", JUNE_15, 10, 60)],
    );
    assert_eq!(
        transit_legs(two_trip_journey(&run(
            &state,
            &TransitRequest {
                now: FEED_TIME - 30,
                ..morning_request()
            },
        )))[0]
            .delay_seconds,
        Some(60)
    );
    trip_feed(
        &state,
        FEED_TIME,
        vec![delay_update("T1", JUNE_15, 10, 180)],
    );
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    assert_eq!(
        transit_legs(two_trip_journey(&response))[0].delay_seconds,
        Some(180)
    );
}

#[test]
fn transition_updated_to_stale_falls_back_to_the_schedule() {
    let state = synthetic_state();
    trip_feed(
        &state,
        FEED_TIME,
        vec![delay_update("T1", JUNE_15, 10, 120)],
    );
    let fresh = run(
        &state,
        &TransitRequest {
            now: FEED_TIME + 8 * 60,
            ..morning_request()
        },
    );
    assert_eq!(fresh.realtime.level, RealtimeLevel::Realtime);
    assert_eq!(
        transit_legs(two_trip_journey(&fresh))[0].delay_seconds,
        Some(120)
    );

    let later = FEED_TIME + valinor_gtfs_rt::DEFAULT_REALTIME_STALE_AFTER + 1;
    let response = run(
        &state,
        &TransitRequest {
            now: later,
            ..morning_request()
        },
    );
    assert_eq!(response.realtime.level, RealtimeLevel::Schedule);
    let feed = &response.realtime.feeds[0];
    assert_eq!(feed.state, FeedState::Stale);
    assert_eq!(
        feed.age_seconds,
        Some(valinor_gtfs_rt::DEFAULT_REALTIME_STALE_AFTER + 1)
    );
    let journey = two_trip_journey(&response);
    assert_eq!(transit_legs(journey)[0].delay_seconds, None);
}

#[test]
fn transition_stale_to_updated_applies_fresh_bytes() {
    let state = synthetic_state();
    trip_feed(
        &state,
        FEED_TIME,
        vec![delay_update("T1", JUNE_15, 10, 120)],
    );
    let later = FEED_TIME + valinor_gtfs_rt::DEFAULT_REALTIME_STALE_AFTER + 10;
    assert_eq!(
        run(
            &state,
            &TransitRequest {
                now: later,
                ..morning_request()
            }
        )
        .realtime
        .level,
        RealtimeLevel::Schedule
    );
    trip_feed(&state, later, vec![delay_update("T1", JUNE_15, 10, 90)]);
    let response = run(
        &state,
        &TransitRequest {
            now: later,
            ..morning_request()
        },
    );
    assert_eq!(response.realtime.level, RealtimeLevel::Realtime);
    assert_eq!(
        transit_legs(two_trip_journey(&response))[0].delay_seconds,
        Some(90)
    );
}

#[test]
fn transition_updated_to_undecodable_degrades_and_reports() {
    let state = synthetic_state();
    trip_feed(
        &state,
        FEED_TIME,
        vec![delay_update("T1", JUNE_15, 10, 120)],
    );
    run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );

    let outcome = state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::TripUpdates,
            bytes: Some(&[0xff, 0xff, 0xff]),
            received_at: FEED_TIME + 30,
            stale_after: None,
        })
        .expect("known feed");
    assert!(matches!(outcome, IngestOutcome::Undecodable(_)));
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME + 30,
            ..morning_request()
        },
    );
    assert_eq!(response.realtime.level, RealtimeLevel::Schedule);
    assert_eq!(response.realtime.feeds[0].state, FeedState::Undecodable);
    assert!(response.realtime.feeds[0].error.is_some());
    assert_eq!(
        transit_legs(two_trip_journey(&response))[0].delay_seconds,
        None
    );
}

#[test]
fn transition_undecodable_to_updated_recovers_with_good_bytes() {
    let state = synthetic_state();
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::TripUpdates,
            bytes: Some(&[0xff, 0xff, 0xff]),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    assert_eq!(
        run(
            &state,
            &TransitRequest {
                now: FEED_TIME,
                ..morning_request()
            }
        )
        .realtime
        .level,
        RealtimeLevel::Schedule
    );
    trip_feed(&state, FEED_TIME, vec![delay_update("T1", JUNE_15, 10, 75)]);
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    assert_eq!(response.realtime.level, RealtimeLevel::Realtime);
    assert_eq!(
        transit_legs(two_trip_journey(&response))[0].delay_seconds,
        Some(75)
    );
}

#[test]
fn transition_updated_to_scheduled_when_a_feed_is_cleared() {
    let state = synthetic_state();
    trip_feed(
        &state,
        FEED_TIME,
        vec![delay_update("T1", JUNE_15, 10, 120)],
    );
    run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    let outcome = state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::TripUpdates,
            bytes: None,
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    assert_eq!(outcome, IngestOutcome::Cleared);
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    assert_eq!(response.realtime.level, RealtimeLevel::Schedule);
    assert!(response.realtime.feeds.is_empty());
}

#[test]
fn transition_rollover_keeps_feeds_and_moves_the_window() {
    let state = synthetic_state();
    trip_feed(
        &state,
        FEED_TIME,
        vec![
            delay_update("T1", JUNE_15, 10, 120),
            delay_update("T1", JUNE_16, 10, 240),
        ],
    );
    let monday = state.pin(FEED_TIME).expect("pin");
    assert_eq!(
        monday.window.dates,
        [
            crate::testing::date("20260614"),
            crate::testing::date(JUNE_15),
            crate::testing::date(JUNE_16),
        ]
    );

    // Just after the service day rolls over, with the feed still within its age limit.
    let after_midnight = june_15(24, 1);
    let tuesday_feed = encoded_feed(after_midnight, vec![delay_update("T1", JUNE_16, 10, 240)]);
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::TripUpdates,
            bytes: Some(&tuesday_feed),
            received_at: after_midnight,
            stale_after: None,
        })
        .expect("known feed");
    let tuesday = state.pin(after_midnight).expect("pin");
    assert_ne!(monday.window.dates, tuesday.window.dates);
    assert_eq!(tuesday.window.dates[1], crate::testing::date(JUNE_16));

    let request = TransitRequest {
        departure: june_15(24 + 7, 55),
        now: after_midnight,
        ..morning_request()
    };
    let response = run(&state, &request);
    assert_eq!(
        transit_legs(two_trip_journey(&response))[0].delay_seconds,
        Some(240)
    );
}

#[test]
fn transition_rollover_without_feeds_still_routes() {
    let state = synthetic_state();
    let monday = state.pin(june_15(23, 59)).expect("pin");
    let tuesday = state.pin(june_15(24, 0)).expect("pin");
    assert_ne!(monday.window.dates, tuesday.window.dates);
    let request = TransitRequest {
        departure: june_15(24 + 7, 55),
        now: june_15(24, 0),
        ..morning_request()
    };
    assert!(
        run(&state, &request)
            .journeys
            .iter()
            .any(|j| !j.legs.is_empty())
    );
}

#[test]
fn a_pinned_snapshot_is_unchanged_by_later_refreshes() {
    let state = synthetic_state();
    let earlier = state.pin(FEED_TIME).expect("pin");
    assert_eq!(earlier.stats().trip_updates, 0);
    trip_feed(
        &state,
        FEED_TIME,
        vec![delay_update("T1", JUNE_15, 10, 300)],
    );
    let later = state.refresh(FEED_TIME).expect("refresh");
    assert_eq!(later.stats().trip_updates, 1);
    assert_eq!(earlier.stats().trip_updates, 0);
}

#[test]
fn a_pin_serves_the_current_snapshot_while_a_rebuild_is_running() {
    let state = std::sync::Arc::new(synthetic_state());
    let before = state.pin(FEED_TIME).expect("pin");
    trip_feed(
        &state,
        FEED_TIME,
        vec![delay_update("T1", JUNE_15, 10, 120)],
    );

    // A rebuild in flight holds the writer.
    let writer = state.writer.lock().expect("lock");
    let (done, finished) = std::sync::mpsc::channel();
    let shared = std::sync::Arc::clone(&state);
    let query = std::thread::spawn(move || {
        let pinned = shared.pin(FEED_TIME);
        done.send(()).expect("receiver");
        pinned
    });
    finished
        .recv_timeout(core::time::Duration::from_secs(5))
        .expect("the query didn't wait for the rebuild");
    let pinned = query.join().expect("join").expect("pin");
    assert!(std::sync::Arc::ptr_eq(&pinned, &before));
    drop(writer);
    assert_eq!(state.pin(FEED_TIME).expect("pin").stats().trip_updates, 1);
}

#[test]
fn a_pin_waits_when_the_service_date_has_rolled_over() {
    let state = synthetic_state();
    let monday = state.pin(june_15(23, 59)).expect("pin");
    let tuesday = state.pin(june_15(24, 1)).expect("pin");
    assert_ne!(monday.window.dates, tuesday.window.dates);
}

#[test]
fn ingest_and_refresh_publishes_in_one_step() {
    let state = synthetic_state();
    let bytes = encoded_feed(FEED_TIME, vec![delay_update("T1", JUNE_15, 10, 60)]);
    let result = state
        .ingest_and_refresh(
            FeedUpdate {
                feed: FEED,
                kind: FeedKind::TripUpdates,
                bytes: Some(&bytes),
                received_at: FEED_TIME,
                stale_after: None,
            },
            FEED_TIME,
        )
        .expect("ingest");
    assert_eq!(result.outcome, IngestOutcome::Accepted);
    assert_eq!(result.snapshot.expect("built").stats().trip_updates, 1);
}

#[test]
fn a_query_outside_the_timetable_is_an_error() {
    let state = synthetic_state();
    let far_future = june_15(24 * 400, 0);
    let error = state
        .pin_for_departure(far_future, far_future, far_future + 3600)
        .expect_err("not covered");
    assert!(matches!(error, StateError::DateNotCovered(_)));
    let request = TransitRequest {
        departure: far_future,
        now: far_future,
        ..morning_request()
    };
    let snapshot = state.pin(june_15(8, 0)).expect("pin");
    let mut router = Router::new(StraightLineTransfers::new(
        snapshot.stop_locations(),
        500.0,
        1.33,
    ));
    let error = plan(
        &state,
        &mut router,
        &mut StraightLineAccess::default(),
        &request,
        &|| false,
    );
    assert!(matches!(
        error,
        Err(PlanError::State(StateError::DateNotCovered(_)))
    ));
}

#[test]
fn unknown_feeds_are_rejected_rather_than_ignored() {
    let state = synthetic_state();
    let error = state.ingest(FeedUpdate {
        feed: "nope",
        kind: FeedKind::Alerts,
        bytes: Some(&[]),
        received_at: FEED_TIME,
        stale_after: None,
    });
    assert!(matches!(error, Err(StateError::UnknownFeed(name)) if name == "nope"));
}

#[test]
fn an_on_demand_window_serves_another_date_with_current_alerts() {
    let state = synthetic_state();
    let alert = alert_entity("a1", Effect::StopMoved, vec![stop_selector("S1")]);
    let bytes = encoded_feed(FEED_TIME, vec![alert]);
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::Alerts,
            bytes: Some(&bytes),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let current = state.pin(FEED_TIME).expect("pin");
    // A week later is outside the current window and needs its own.
    let next_monday = june_15(24 * 7 + 7, 55);
    let on_demand = state
        .pin_for_departure(FEED_TIME, next_monday, next_monday + 3600)
        .expect("on-demand");
    assert!(!on_demand.window.covers(&[crate::testing::date(JUNE_15)]));
    assert!(on_demand.window.covers(&[crate::testing::date("20260622")]));
    assert_eq!(
        state.pin(FEED_TIME).expect("pin").window.dates,
        current.window.dates
    );

    let request = TransitRequest {
        departure: next_monday,
        now: FEED_TIME,
        ..morning_request()
    };
    let response = run(&state, &request);
    let leg = transit_legs(two_trip_journey(&response))[0];
    assert!(
        leg.alerts
            .iter()
            .any(|a| a.alert.effect.as_deref() == Some("stop_moved"))
    );
}

/// Tables that are already empty must still surface an interrupt to the caller.
#[test]
fn an_interrupt_during_tables_is_reported_even_with_empty_tables() {
    struct Empty;
    impl crate::plan::AccessEgressSearch for Empty {
        type Error = core::convert::Infallible;
        type Walk = StraightWalk;
        fn tables(
            &mut self,
            request: &crate::plan::AccessEgressRequest<'_>,
            _: &dyn Fn() -> bool,
        ) -> Result<crate::plan::AccessEgressTables, Self::Error> {
            let count = request.snapshot.snapshot().stop_count();
            Ok(crate::plan::AccessEgressTables {
                access: valinor_transit::StopTable::new(count),
                egress: valinor_transit::StopTable::new(count),
                direct: crate::plan::DirectSearch::NotSearched,
            })
        }
    }
    let state = synthetic_state();
    let snapshot = state.pin(june_15(8, 0)).expect("pin");
    let mut router = Router::new(StraightLineTransfers::new(
        snapshot.stop_locations(),
        500.0,
        1.33,
    ));
    let result = plan(&state, &mut router, &mut Empty, &morning_request(), &|| {
        true
    });
    assert!(matches!(result, Err(PlanError::Interrupted)));
}

#[test]
fn an_interrupted_query_reports_interrupted() {
    let state = synthetic_state();
    let snapshot = state.pin(june_15(8, 0)).expect("pin");
    let mut router = Router::new(StraightLineTransfers::new(
        snapshot.stop_locations(),
        500.0,
        1.33,
    ));
    let result = plan(
        &state,
        &mut router,
        &mut StraightLineAccess::default(),
        &morning_request(),
        &|| true,
    );
    assert!(matches!(result, Err(PlanError::Interrupted)));
}

// --- Response building ---

#[allow(clippy::too_many_lines)]
#[test]
fn the_response_has_everything_a_rider_might_want() {
    let state = synthetic_state();
    let trips = encoded_feed(FEED_TIME, vec![delay_update("T1", JUNE_15, 10, 120)]);
    let vehicles = encoded_feed(
        FEED_TIME,
        vec![vehicle_entity("7033", "T1", JUNE_15, None, 47.605, -122.33)],
    );
    let alerts = encoded_feed(
        FEED_TIME,
        vec![
            alert_entity(
                "a1",
                Effect::StopMoved,
                vec![route_stop_selector("R1", "S1")],
            ),
            alert_entity("a2", Effect::Detour, vec![route_selector("R1")]),
            alert_entity("a3", Effect::NoService, vec![stop_selector("S4")]),
        ],
    );
    for (kind, bytes) in [
        (FeedKind::TripUpdates, trips.as_slice()),
        (FeedKind::VehiclePositions, vehicles.as_slice()),
        (FeedKind::Alerts, alerts.as_slice()),
    ] {
        state
            .ingest(FeedUpdate {
                feed: FEED,
                kind,
                bytes: Some(bytes),
                received_at: FEED_TIME,
                stale_after: None,
            })
            .expect("known feed");
    }
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME + 15,
            ..morning_request()
        },
    );
    // The first journey has both rides, and shows the moved stop, the vehicle, and the delay.
    assert_json_snapshot!("full_response", &response);
}

/// Tells the response layer a transfer's real walk was short.
#[derive(Default)]
struct ShortWalks(StraightLineAccess);

impl AccessEgressSearch for ShortWalks {
    type Error = Infallible;
    type Walk = StraightWalk;

    fn tables(
        &mut self,
        request: &AccessEgressRequest<'_>,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<AccessEgressTables, Infallible> {
        self.0.tables(request, interrupt)
    }

    fn transfer_seconds(&self, _walk: &StraightWalk) -> Option<u32> {
        Some(17)
    }
}

/// A transfer leg shows the walk's own time; a longer transfer time is a wait to board.
#[test]
fn a_transfer_leg_is_the_walk_and_the_rest_is_a_wait() {
    let state = synthetic_state();
    let request = morning_request();
    let snapshot = state.pin(request.now).expect("pin");
    let mut router = Router::new(StraightLineTransfers::new(
        snapshot.stop_locations(),
        500.0,
        1.33,
    ));
    let response = plan(
        &state,
        &mut router,
        &mut ShortWalks::default(),
        &request,
        &|| false,
    )
    .expect("plan");
    let journey = two_trip_journey(&response);
    let (index, leg) = journey
        .legs
        .iter()
        .enumerate()
        .find_map(|(index, leg)| match leg {
            JourneyLeg::Transfer(leg) => Some((index, leg)),
            _ => None,
        })
        .expect("a transfer");
    assert_eq!(leg.duration_seconds, 17);
    assert_eq!(leg.arrival - leg.departure, 17);
    assert_eq!(journey.walking.transfer_seconds, 17);
    let JourneyLeg::Transit(next) = &journey.legs[index + 1] else {
        panic!("a ride follows");
    };
    assert_eq!(next.wait_seconds, next.departure - leg.arrival);
}

#[test]
fn stop_moved_marks_the_boarding_stop() {
    let state = synthetic_state();
    let alerts = encoded_feed(
        FEED_TIME,
        vec![alert_entity(
            "a1",
            Effect::StopMoved,
            vec![route_stop_selector("R1", "S1")],
        )],
    );
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::Alerts,
            bytes: Some(&alerts),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    let leg = transit_legs(two_trip_journey(&response))[0];
    let alert = leg
        .alerts
        .iter()
        .find(|a| a.alert.effect.as_deref() == Some("stop_moved"))
        .expect("attached");
    let moved = alert.moved_stops.as_ref().expect("marked");
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].role, "boarding");
    assert_eq!(moved[0].stop_id, "S1");
    // And the rider can still board there: the alert changes nothing about routing.
    assert_eq!(leg.stops[0].stop.stop_id, "S1");
}

#[test]
fn stop_moved_marks_the_alighting_stop() {
    let state = synthetic_state();
    let alerts = encoded_feed(
        FEED_TIME,
        vec![alert_entity(
            "a1",
            Effect::StopMoved,
            vec![stop_selector("S6")],
        )],
    );
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::Alerts,
            bytes: Some(&alerts),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    let leg = transit_legs(two_trip_journey(&response))[1];
    let alert = leg
        .alerts
        .iter()
        .find(|a| a.alert.effect.as_deref() == Some("stop_moved"))
        .expect("attached");
    let moved = alert.moved_stops.as_ref().expect("marked");
    assert_eq!(
        (moved[0].role, moved[0].stop_id.as_str()),
        ("alighting", "S6")
    );
}

#[test]
fn a_moved_stop_in_the_middle_of_a_leg_is_shown_without_a_mark() {
    let state = synthetic_state();
    let alerts = encoded_feed(
        FEED_TIME,
        vec![alert_entity(
            "a1",
            Effect::StopMoved,
            vec![stop_selector("S2")],
        )],
    );
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::Alerts,
            bytes: Some(&alerts),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    let leg = transit_legs(two_trip_journey(&response))[0];
    let alert = leg
        .alerts
        .iter()
        .find(|a| a.alert.effect.as_deref() == Some("stop_moved"))
        .expect("attached");
    assert_eq!(alert.moved_stops, None);
}

#[test]
fn route_and_stop_selectors_combine_with_and() {
    let state = synthetic_state();
    // Names route R2 together with stop S1, which R2 never serves.
    let alerts = encoded_feed(
        FEED_TIME,
        vec![alert_entity(
            "a1",
            Effect::StopMoved,
            vec![route_stop_selector("R2", "S1")],
        )],
    );
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::Alerts,
            bytes: Some(&alerts),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    let journey = two_trip_journey(&response);
    assert!(
        transit_legs(journey)
            .iter()
            .all(|leg| leg.alerts.is_empty())
    );
}

#[test]
fn an_alert_outside_the_ride_does_not_attach() {
    let state = synthetic_state();
    let mut alert = alert_entity("a1", Effect::Detour, vec![route_selector("R1")]);
    #[allow(deprecated)]
    {
        let body = alert.alert.as_mut().expect("alert");
        body.active_period = vec![proto::TimeRange {
            start: Some(u64::try_from(june_15(20, 0)).expect("positive")),
            end: Some(u64::try_from(june_15(22, 0)).expect("positive")),
        }];
    }
    let alerts = encoded_feed(FEED_TIME, vec![alert]);
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::Alerts,
            bytes: Some(&alerts),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    assert!(
        transit_legs(two_trip_journey(&response))[0]
            .alerts
            .is_empty()
    );
}

#[test]
fn removed_stops_carry_their_marks_without_a_trip_update() {
    let state = synthetic_state();
    // A stop-scoped NO_SERVICE alert removes S2 from T1, which has no trip update.
    let alerts = encoded_feed(
        FEED_TIME,
        vec![alert_entity(
            "a1",
            Effect::NoService,
            vec![stop_selector("S2")],
        )],
    );
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::Alerts,
            bytes: Some(&alerts),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    let leg = transit_legs(two_trip_journey(&response))[0];
    let beta = leg
        .stops
        .iter()
        .find(|s| s.stop.stop_id == "S2")
        .expect("S2 is on the leg");
    assert!(beta.skipped);
    assert!(beta.removed_by_alert);
    assert!(!leg.stops[0].skipped);
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME,
            ..morning_request()
        },
    );
    let leg = transit_legs(two_trip_journey(&response))[0];
    assert!(
        leg.alerts
            .iter()
            .any(|a| a.alert.effect.as_deref() == Some("no_service"))
    );
}

/// Stops with every wheelchair value, and trips with and without the accessible flag.
fn wheelchair_state() -> TransitState {
    let source = InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\n\
             A,Metro,America/Los_Angeles,https://m.test\n",
        )
        .with("routes.txt", "route_id,agency_id,route_type\nR,A,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon,wheelchair_boarding\n\
             ORG,Origin,47.5995,-122.3300,1\n\
             BAD,Stairs,47.6100,-122.3200,2\n\
             GOOD,Unknown,47.6100,-122.3100,\n\
             DST,Destination,47.6204,-122.3098,1\n",
        )
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,wheelchair_accessible\n\
             R,WK,T1,1\nR,WK,T2,1\nR,WK,T3,1\nR,WK,T4,1\nR,WK,T5,2\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             T1,08:05:00,08:05:00,ORG,1\nT1,08:15:00,08:15:00,BAD,2\n\
             T2,08:20:00,08:20:00,BAD,1\nT2,08:30:00,08:30:00,DST,2\n\
             T3,08:10:00,08:10:00,ORG,1\nT3,08:25:00,08:25:00,GOOD,2\n\
             T4,08:30:00,08:30:00,GOOD,1\nT4,08:45:00,08:45:00,DST,2\n\
             T5,08:20:00,08:20:00,ORG,1\nT5,08:36:00,08:36:00,DST,2\n",
        );
    let compiled = compile(vec![FeedInput {
        id: CompileFeedId::try_new("w".to_owned()).expect("id"),
        source: Box::new(source),
    }])
    .expect("compiles");
    TransitState::new(
        LoadedTimetable::from_bytes(compiled.bytes).expect("loads"),
        StateConfig::default(),
    )
    .expect("state")
}

/// A rider in a wheelchair can't change at an inaccessible stop, but unknown ones are fine,
/// and a trip marked inaccessible is out of the question.
#[test]
fn a_wheelchair_query_avoids_an_inaccessible_stop_and_uses_unknown_ones() {
    let state = wheelchair_state();
    let run_mode = |mode| {
        let request = TransitRequest {
            mode,
            options: TransitOptions {
                max_trips: 3,
                ..TransitOptions::default()
            },
            ..morning_request()
        };
        run(&state, &request)
    };
    let stops_of = |response: &PlanResponse| -> Vec<String> {
        response
            .journeys
            .iter()
            .flat_map(transit_legs)
            .flat_map(|leg| leg.stops.iter().map(|visit| visit.stop.stop_id.clone()))
            .collect()
    };
    let walking = run_mode(TravelMode::Walking);
    assert!(
        stops_of(&walking).contains(&"BAD".to_owned()),
        "a rider on foot may use it"
    );
    assert!(
        walking
            .journeys
            .iter()
            .flat_map(transit_legs)
            .any(|leg| leg.trip.trip_id == "T5")
    );
    let wheelchair = run_mode(TravelMode::Wheelchair);
    assert!(!wheelchair.journeys.is_empty());
    let trips: Vec<&str> = wheelchair
        .journeys
        .iter()
        .flat_map(transit_legs)
        .map(|leg| leg.trip.trip_id.as_str())
        .collect();
    assert!(
        !stops_of(&wheelchair).contains(&"BAD".to_owned()),
        "a rider in a wheelchair can't use it"
    );
    assert!(!trips.contains(&"T5"), "a trip marked inaccessible is out");
    assert!(trips.contains(&"T3"), "{trips:?}");
    assert!(
        stops_of(&wheelchair).contains(&"GOOD".to_owned()),
        "a stop with no accessibility information is allowed"
    );
}

#[test]
fn every_stop_and_leg_reports_accessibility() {
    use crate::response::Accessibility;
    let state = wheelchair_state();
    let request = |mode| TransitRequest {
        mode,
        options: TransitOptions {
            max_trips: 3,
            ..TransitOptions::default()
        },
        ..morning_request()
    };
    let wheelchair = run(&state, &request(TravelMode::Wheelchair));
    let journey = wheelchair
        .journeys
        .iter()
        .find(|journey| {
            transit_legs(journey)
                .iter()
                .any(|leg| leg.trip.trip_id == "T3")
        })
        .expect("T3 journey");
    // T3 runs ORG (accessible) to GOOD (unknown), so the leg is unknown; the access is on streets
    // routed for a wheelchair, so it's accessible.
    let JourneyLeg::Access(access) = &journey.legs[0] else {
        panic!("access first");
    };
    assert_eq!(access.accessibility, Accessibility::Accessible);
    let leg = transit_legs(journey)[0];
    assert_eq!(leg.accessibility, Accessibility::Unknown);
    let visits: Vec<_> = leg
        .stops
        .iter()
        .map(|visit| (visit.stop.stop_id.as_str(), visit.stop.accessibility))
        .collect();
    assert_eq!(
        visits,
        vec![
            ("ORG", Accessibility::Accessible),
            ("GOOD", Accessibility::Unknown)
        ]
    );

    let walking = run(&state, &request(TravelMode::Walking));
    let journey = walking
        .journeys
        .iter()
        .find(|journey| {
            transit_legs(journey)
                .iter()
                .any(|leg| leg.trip.trip_id == "T5")
        })
        .expect("T5 journey");
    // T5 is marked inaccessible, so the leg is, whatever its stops are; a walk is unknown.
    let leg = transit_legs(journey)
        .into_iter()
        .find(|leg| leg.trip.trip_id == "T5")
        .expect("T5");
    assert_eq!(leg.accessibility, Accessibility::Inaccessible);
    let JourneyLeg::Access(access) = &journey.legs[0] else {
        panic!("access first");
    };
    assert_eq!(access.accessibility, Accessibility::Unknown);
    let body = serde_json::to_value(&walking.journeys).expect("json");
    assert!(body[0]["legs"][0]["accessibility"].is_string());
}

/// `T1` (route R1) from the origin to a middle stop, staying seated into `T2` (route R2) on
/// to the destination.
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
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon\n\
             ORG,Origin,47.5995,-122.3300\n\
             MID,Middle,47.6100,-122.3200\n\
             DST,Destination,47.6204,-122.3098\n",
        )
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,block_id,trip_headsign\n\
             R1,WK,T1,X,To Middle\nR2,WK,T2,X,To Destination\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             T1,08:05:00,08:05:00,ORG,1\nT1,08:20:00,08:20:00,MID,2\n\
             T2,08:25:00,08:25:00,MID,1\nT2,08:50:00,08:50:00,DST,2\n",
        );
    let compiled = compile(vec![FeedInput {
        id: CompileFeedId::try_new("seated".to_owned()).expect("id"),
        source: Box::new(source),
    }])
    .expect("compiles");
    TransitState::new(
        LoadedTimetable::from_bytes(compiled.bytes).expect("loads"),
        StateConfig::default(),
    )
    .expect("state")
}

/// The rider's one ride, and how it's reported.
fn seated_leg(response: &PlanResponse) -> &TransitLeg {
    let journey = response
        .journeys
        .iter()
        .find(|journey| journey.trips == 1)
        .expect("a journey with one ride");
    transit_legs(journey)[0]
}

#[test]
fn a_stay_seated_ride_is_one_leg_with_its_segments() {
    let state = seated_state();
    let response = run(&state, &morning_request());
    let leg = seated_leg(&response);
    assert_eq!(leg.trip.trip_id, "T1", "the segment the rider boards");
    assert_eq!(leg.route.as_ref().expect("route").route_id, "R1");
    assert_eq!(leg.segments.len(), 2);
    assert_eq!(leg.segments[0].trip_id, "T1");
    assert_eq!(
        leg.segments[0].route.as_ref().expect("route").route_id,
        "R1"
    );
    assert_eq!(leg.segments[0].headsign.as_deref(), Some("To Middle"));
    assert_eq!(leg.segments[1].trip_id, "T2");
    assert_eq!(
        leg.segments[1].route.as_ref().expect("route").route_id,
        "R2"
    );
    // The join shows once, with the first trip's arrival and the next one's departure.
    let names: Vec<_> = leg.stops.iter().map(|s| s.stop.stop_id.as_str()).collect();
    assert_eq!(names, vec!["ORG", "MID", "DST"]);
    let middle = &leg.stops[1];
    assert_eq!(middle.arrival, june_15(8, 20));
    assert_eq!(middle.departure, june_15(8, 25));
    assert_eq!(leg.segments[0].first_stop_index, 0);
    assert_eq!(leg.segments[0].last_stop_index, 1);
    assert_eq!(leg.segments[1].first_stop_index, 1);
    assert_eq!(leg.segments[1].last_stop_index, 2);
}

fn seated_trip_update(
    trip: &str,
    relationship: proto::trip_descriptor::ScheduleRelationship,
    stop_sequence: u32,
    delay: i32,
) -> proto::FeedEntity {
    proto::FeedEntity {
        trip_update: Some(proto::TripUpdate {
            trip: proto::TripDescriptor {
                trip_id: Some(trip.to_owned()),
                start_date: Some(JUNE_15.to_owned()),
                schedule_relationship: Some(relationship as i32),
                ..proto::TripDescriptor::default()
            },
            stop_time_update: if delay == 0 {
                Vec::new()
            } else {
                vec![proto::trip_update::StopTimeUpdate {
                    stop_sequence: Some(stop_sequence),
                    arrival: Some(proto::trip_update::StopTimeEvent {
                        delay: Some(delay),
                        ..proto::trip_update::StopTimeEvent::default()
                    }),
                    departure: Some(proto::trip_update::StopTimeEvent {
                        delay: Some(delay),
                        ..proto::trip_update::StopTimeEvent::default()
                    }),
                    ..proto::trip_update::StopTimeUpdate::default()
                }]
            },
            ..proto::TripUpdate::default()
        }),
        ..entity_for(trip)
    }
}

fn entity_for(id: &str) -> proto::FeedEntity {
    proto::FeedEntity {
        id: id.to_owned(),
        ..proto::FeedEntity::default()
    }
}

/// A plan with the stay-seated feed's one update applied.
fn seated_plan_with(entity: proto::FeedEntity) -> PlanResponse {
    let state = seated_state();
    let bytes = encoded_feed(FEED_TIME, vec![entity]);
    state
        .ingest(FeedUpdate {
            feed: "seated",
            kind: FeedKind::TripUpdates,
            bytes: Some(&bytes),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    run(
        &state,
        &TransitRequest {
            now: FEED_TIME + 15,
            ..morning_request()
        },
    )
}

#[test]
fn a_delay_moves_its_own_part_and_carries_into_a_later_part_with_no_update() {
    use proto::trip_descriptor::ScheduleRelationship;
    // T2 (the second part) five minutes late: T1's part keeps its times.
    let response = seated_plan_with(seated_trip_update(
        "T2",
        ScheduleRelationship::Scheduled,
        1,
        300,
    ));
    let leg = seated_leg(&response);
    assert_eq!(leg.departure, june_15(8, 5), "the first part is on time");
    assert_eq!(leg.arrival, june_15(8, 55), "the second part is late");
    let middle = &leg.stops[1];
    assert_eq!(middle.arrival, june_15(8, 20), "T1's arrival is unchanged");
    assert_eq!(middle.departure, june_15(8, 30));

    // Updates for both original trips of one run in the same feed both apply.
    let state = seated_state();
    let bytes = encoded_feed(
        FEED_TIME,
        vec![
            seated_trip_update("T1", ScheduleRelationship::Scheduled, 2, 60),
            seated_trip_update("T2", ScheduleRelationship::Scheduled, 1, 300),
        ],
    );
    state
        .ingest(FeedUpdate {
            feed: "seated",
            kind: FeedKind::TripUpdates,
            bytes: Some(&bytes),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let response = run(
        &state,
        &TransitRequest {
            now: FEED_TIME + 15,
            ..morning_request()
        },
    );
    let leg = seated_leg(&response);
    assert_eq!(leg.stops[1].arrival, june_15(8, 21), "T1's own delay");
    assert_eq!(leg.arrival, june_15(8, 55), "T2's own delay");

    // T1 ten minutes late with no word on T2: it's the same vehicle, so T2's run is ten minutes
    // late too, and keeps its scheduled running time.
    let response = seated_plan_with(seated_trip_update(
        "T1",
        ScheduleRelationship::Scheduled,
        2,
        600,
    ));
    let leg = seated_leg(&response);
    assert_eq!(leg.stops[1].arrival, june_15(8, 30));
    assert_eq!(leg.stops[1].departure, june_15(8, 35));
    assert_eq!(leg.arrival, june_15(9, 0));
}

#[test]
fn a_cancelled_second_part_ends_the_ride_at_the_join() {
    use proto::trip_descriptor::ScheduleRelationship;
    let response = seated_plan_with(seated_trip_update(
        "T2",
        ScheduleRelationship::Canceled,
        1,
        0,
    ));
    // Nothing reaches the destination by staying on.
    for journey in &response.journeys {
        for leg in transit_legs(journey) {
            assert_ne!(
                leg.stops.last().map(|s| s.stop.stop_id.as_str()),
                Some("DST"),
                "the ride can't reach the end of the cancelled part"
            );
        }
    }
    // A cancelled first part leaves the second part running, boarded at the join.
    let response = seated_plan_with(seated_trip_update(
        "T1",
        ScheduleRelationship::Canceled,
        1,
        0,
    ));
    let boarded_at_origin = response
        .journeys
        .iter()
        .flat_map(transit_legs)
        .any(|leg| leg.stops.first().map(|s| s.stop.stop_id.as_str()) == Some("ORG"));
    assert!(!boarded_at_origin, "the first part is gone");
}

#[test]
fn a_cycling_query_only_rides_routes_that_allow_bikes() {
    let state = synthetic_state();
    let request = TransitRequest {
        destination: LatLon {
            lat: 47.6200,
            lon: -122.3285,
        },
        mode: TravelMode::Cycling,
        options: TransitOptions {
            max_trips: 2,
            ..TransitOptions::default()
        },
        ..morning_request()
    };
    let response = run(&state, &request);
    assert!(!response.journeys.is_empty());
    for journey in &response.journeys {
        for leg in transit_legs(journey) {
            assert_eq!(leg.trip.trip_id, "T5", "only T5 allows bikes");
            assert_eq!(leg.trip.bikes_allowed, Some("yes"));
        }
    }
    assert_eq!(
        TravelMode::Cycling.route_filter(),
        RouteFilter::requiring(RouteAttributes::BIKES_ALLOWED)
    );
}

#[test]
fn journeys_slower_than_the_direct_trip_are_dropped() {
    let state = synthetic_state();
    let snapshot = state.pin(june_15(8, 0)).expect("pin");
    let mut router = Router::new(StraightLineTransfers::new(
        snapshot.stop_locations(),
        500.0,
        1.33,
    ));
    let mut access = StraightLineAccess {
        include_direct: true,
        ..StraightLineAccess::default()
    };
    // About 2.5 km as the crow flies: far faster than any bus here.
    let response = plan(
        &state,
        &mut router,
        &mut access,
        &morning_request(),
        &|| false,
    )
    .expect("plan");
    assert!(response.journeys.is_empty());
    assert_eq!(
        response.no_journeys_reason,
        Some(crate::response::NoJourneysReason::SlowerThanDirect)
    );
    assert_eq!(response.direct.status, "found");
}

#[test]
fn empty_results_say_which_end_has_no_stops() {
    let state = synthetic_state();
    let far = LatLon {
        lat: 47.0,
        lon: -122.0,
    };
    let no_access = run(
        &state,
        &TransitRequest {
            origin: far,
            ..morning_request()
        },
    );
    assert_eq!(
        no_access.no_journeys_reason,
        Some(crate::response::NoJourneysReason::NoAccess)
    );
    let no_egress = run(
        &state,
        &TransitRequest {
            destination: far,
            ..morning_request()
        },
    );
    assert_eq!(
        no_egress.no_journeys_reason,
        Some(crate::response::NoJourneysReason::NoEgress)
    );
}

#[test]
fn a_window_with_no_service_says_so() {
    let state = synthetic_state();
    // Saturday: the calendar has no trips at all.
    let saturday = june_15(24 * 5 + 8, 0);
    let request = TransitRequest {
        departure: saturday,
        now: saturday - 60,
        ..morning_request()
    };
    let response = run(&state, &request);
    assert!(response.journeys.is_empty());
    assert_eq!(
        response.no_journeys_reason,
        Some(crate::response::NoJourneysReason::NoService)
    );
}

// --- Headway-based runs ---

#[test]
fn headway_vehicle_is_matched_by_start_time_not_grid_run() {
    let state = synthetic_state();
    // Two real vehicles of F1, both nearest the 10:30 grid run.
    let vehicles = encoded_feed(
        june_15(10, 30),
        vec![
            vehicle_entity("V1", "F1", JUNE_15, Some("10:25:00"), 47.60, -122.33),
            vehicle_entity("V2", "F1", JUNE_15, Some("10:40:00"), 47.61, -122.33),
        ],
    );
    let trips = encoded_feed(june_15(10, 30), {
        let mut item = crate::testing::entity("tu-1");
        item.trip_update = Some(proto::TripUpdate {
            trip: crate::testing::descriptor("F1", JUNE_15, Some("10:40:00")),
            ..proto::TripUpdate::default()
        });
        vec![item]
    });
    for (kind, bytes) in [
        (FeedKind::VehiclePositions, vehicles.as_slice()),
        (FeedKind::TripUpdates, trips.as_slice()),
    ] {
        state
            .ingest(FeedUpdate {
                feed: FEED,
                kind,
                bytes: Some(bytes),
                received_at: june_15(10, 30),
                stale_after: None,
            })
            .expect("known feed");
    }
    let request = TransitRequest {
        origin: LatLon {
            lat: 47.5995,
            lon: -122.33,
        },
        destination: LatLon {
            lat: 47.6105,
            lon: -122.33,
        },
        departure: june_15(10, 25),
        now: june_15(10, 30),
        ..morning_request()
    };
    let response = run(&state, &request);
    let leg = response
        .journeys
        .iter()
        .flat_map(transit_legs)
        .find(|leg| leg.trip.headway_based == Some(true))
        .expect("the frequent line is ridden");
    assert_eq!(leg.trip.start_time, Some(10 * 3600 + 40 * 60));
    assert_eq!(
        leg.vehicle.as_ref().and_then(|v| v.id.as_deref()),
        Some("V2")
    );
    // Its times are not promised.
    assert_eq!(leg.stops[0].approximate, Some(true));
}

#[test]
fn a_vehicle_can_be_looked_up_by_id() {
    let state = synthetic_state();
    let vehicles = encoded_feed(
        FEED_TIME,
        vec![vehicle_entity("7033", "T1", JUNE_15, None, 47.605, -122.33)],
    );
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::VehiclePositions,
            bytes: Some(&vehicles),
            received_at: FEED_TIME,
            stale_after: None,
        })
        .expect("known feed");
    let found = state.vehicle(FEED_TIME + 5, FEED, "7033").expect("lookup");
    let vehicle = found.vehicle.expect("present");
    assert_eq!(vehicle.label.as_deref(), Some("Bus 7033"));
    assert_eq!(
        vehicle.current_stop.as_ref().map(|s| s.stop_id.as_str()),
        Some("S2")
    );
    assert_eq!(
        vehicle.current_stop.and_then(|s| s.name).as_deref(),
        Some("Beta")
    );

    // Stale feeds don't supply positions: the rider shouldn't see an old location as current.
    let outdated = state
        .vehicle(
            FEED_TIME + valinor_gtfs_rt::DEFAULT_REALTIME_STALE_AFTER + 1,
            FEED,
            "7033",
        )
        .expect("lookup");
    assert!(outdated.vehicle.is_none());
    assert_eq!(outdated.feeds[0].state, FeedState::Stale);
}

#[allow(deprecated)]
fn timed_stop_update(stop: &str, sequence: u32, unix: i64) -> proto::trip_update::StopTimeUpdate {
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
}

#[test]
fn an_added_trip_gets_a_leg_with_predictions_and_no_schedule() {
    let state = synthetic_state();
    let mut item = crate::testing::entity("tu-added");
    item.trip_update = Some(proto::TripUpdate {
        trip: proto::TripDescriptor {
            trip_id: Some("EXTRA".to_owned()),
            start_date: Some(JUNE_15.to_owned()),
            route_id: Some("R1".to_owned()),
            schedule_relationship: Some(proto::trip_descriptor::ScheduleRelationship::New as i32),
            ..proto::TripDescriptor::default()
        },
        stop_time_update: vec![
            timed_stop_update("S2", 1, june_15(7, 58)),
            timed_stop_update("S4", 2, june_15(8, 2)),
        ],
        ..proto::TripUpdate::default()
    });
    trip_feed(&state, FEED_TIME, vec![item]);
    let request = TransitRequest {
        origin: LatLon {
            lat: 47.6101,
            lon: -122.3300,
        },
        destination: LatLon {
            lat: 47.6301,
            lon: -122.3300,
        },
        departure: june_15(7, 50),
        now: FEED_TIME,
        options: TransitOptions {
            max_trips: 1,
            ..TransitOptions::default()
        },
        ..morning_request()
    };
    let response = run(&state, &request);
    let leg = response
        .journeys
        .iter()
        .flat_map(transit_legs)
        .find(|leg| leg.trip.trip_id == "EXTRA")
        .expect("the added trip is ridden");
    assert_eq!(leg.trip.realtime_state, Some("added"));
    assert_eq!(leg.route.as_ref().map(|r| r.route_id.as_str()), Some("R1"));
    assert_eq!(leg.stops.len(), 2);
    assert_eq!(leg.stops[0].scheduled_departure, None);
    assert_eq!(leg.stops[0].predicted_departure, Some(june_15(7, 58)));
    assert_eq!(leg.shape, None);
}

#[test]
fn a_replaced_trip_uses_its_own_stops_and_no_static_shape() {
    let state = synthetic_state();
    let mut item = crate::testing::entity("tu-replaced");
    item.trip_update = Some(proto::TripUpdate {
        trip: proto::TripDescriptor {
            trip_id: Some("T1".to_owned()),
            start_date: Some(JUNE_15.to_owned()),
            schedule_relationship: Some(
                proto::trip_descriptor::ScheduleRelationship::Replacement as i32,
            ),
            ..proto::TripDescriptor::default()
        },
        stop_time_update: vec![
            timed_stop_update("S1", 1, june_15(8, 0)),
            timed_stop_update("S4", 2, june_15(8, 20)),
        ],
        ..proto::TripUpdate::default()
    });
    trip_feed(&state, FEED_TIME, vec![item]);
    let request = TransitRequest {
        destination: LatLon {
            lat: 47.6301,
            lon: -122.3300,
        },
        now: FEED_TIME,
        options: TransitOptions {
            max_trips: 1,
            ..TransitOptions::default()
        },
        ..morning_request()
    };
    let response = run(&state, &request);
    let leg = response
        .journeys
        .iter()
        .flat_map(transit_legs)
        .find(|leg| leg.trip.trip_id == "T1")
        .expect("the replaced trip is ridden");
    assert_eq!(leg.trip.realtime_state, Some("replaced"));
    let stops: Vec<&str> = leg.stops.iter().map(|s| s.stop.stop_id.as_str()).collect();
    assert_eq!(stops, ["S1", "S4"]);
    assert_eq!(leg.stops[1].predicted_arrival, Some(june_15(8, 20)));
    // The schedule's shape describes a different stop pattern than this one.
    assert_eq!(leg.shape, None);
}

#[test]
fn state_is_shared_between_threads() {
    fn assert_shareable<T: Send + Sync>() {}
    assert_shareable::<TransitState>();
    assert_shareable::<TransitSnapshot>();
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Whatever the bytes, ingesting never fails on them, routing survives, and the feed is reported.
    #[test]
    fn arbitrary_bytes_degrade_to_the_schedule(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let state = synthetic_state();
        let outcome = state.ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::TripUpdates,
            bytes: Some(&bytes),
            received_at: FEED_TIME,
            stale_after: None,
        }).expect("known feed");
        let response = run(&state, &TransitRequest { now: FEED_TIME, ..morning_request() });
        prop_assert!(two_trip_journey(&response).trips == 2);
        prop_assert_eq!(response.realtime.feeds.len(), 1);
        if matches!(outcome, IngestOutcome::Undecodable(_)) {
            prop_assert_eq!(response.realtime.level, RealtimeLevel::Schedule);
        }
    }
}
