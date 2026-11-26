//! One test per trip state transition (see [`crate::states`]), plus the scenarios that matter:
//! overtaking, propagation, day boundaries, staleness, and partial updates.

#![allow(deprecated)]

use std::convert::Infallible;
use std::sync::Arc;

use proptest::prelude::*;
use valinor_transit::{
    DaySlot, Direction, RouteFilter, RouteId, Router, Seconds, Snapshot, StopRules, StopTable,
    Time, TransferOutcome, TransferRequest, TransferSearch, TransferSink, TransitQuery, TripId,
    TripKey,
};

use crate::feed::{DEFAULT_REALTIME_STALE_AFTER, FeedInput, FeedKind, ScopedId};
use crate::overlay::{OverlayOptions, RealtimeOverlay, build_overlay};
use crate::proto;
use crate::proto::trip_descriptor::ScheduleRelationship as Trip;
use crate::proto::trip_update::{StopTimeEvent, StopTimeUpdate};
use crate::states::TripState;
use crate::testing::*;

const KCM: &str = "kcm";

fn three_stop_net() -> Net {
    NetBuilder::new()
        .route(
            "R1",
            &["A", "B", "C"],
            &[("t1", &[100, 200, 300]), ("t2", &[1100, 1200, 1300])],
        )
        .build()
}

fn run(net: &Net, feeds: Vec<FeedInput>) -> RealtimeOverlay {
    build_overlay(
        &net.lookup,
        Arc::clone(&net.base),
        feeds,
        NOW,
        OverlayOptions::default(),
    )
    .expect("overlay builds")
}

/// `(trip, lane index in block, arrivals, rules)`, read through the snapshot's public view.
fn lanes_of(
    snapshot: &Snapshot,
    route: u32,
    day: u8,
) -> Vec<(u32, usize, Vec<i32>, Vec<StopRules>)> {
    let lanes = snapshot
        .block_lanes(RouteId::new(route), DaySlot::new(day))
        .expect("known route and day");
    let mut out = Vec::new();
    for (lane_index, lane) in lanes.into_iter().enumerate() {
        for row in 0..snapshot.lane_trip_count(lane).expect("lane") {
            let key = snapshot.trip_key(lane, row).expect("key");
            let events = snapshot.lane_trip_events(lane, row).expect("events");
            out.push((
                key.trip.into_inner(),
                lane_index,
                events.iter().map(|e| e.arrival.into_inner()).collect(),
                events.iter().map(|e| e.rules).collect(),
            ));
        }
    }
    out
}

fn arrivals_of(snapshot: &Snapshot, route: u32, day: u8, trip: u32) -> Option<Vec<i32>> {
    lanes_of(snapshot, route, day)
        .into_iter()
        .find(|(t, _, _, _)| *t == trip)
        .map(|(_, _, arrivals, _)| arrivals)
}

/// The middle day of a three-day test window.
const TODAY: u8 = 1;

fn key(trip: u32) -> TripKey {
    TripKey {
        trip: TripId::new(trip),
        date: valinor_transit::ServiceDate::new(u32::from(TODAY)),
    }
}

// Scheduled.

#[test]
fn scheduled_is_base_times_with_no_replacement_block() {
    let net = three_stop_net();
    let overlay = run(&net, vec![]);
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 100),
            window_seconds(TODAY, 200),
            window_seconds(TODAY, 300)
        ])
    );
    assert_eq!(overlay.stats.blocks_replaced, 0);
    assert_eq!(overlay.stats.arena.blocks, 0);
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::Scheduled);
}

// Delayed.

#[test]
fn scheduled_to_delayed_replaces_times() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![delay_at(2, 30)])])],
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 100),
            window_seconds(TODAY, 230),
            window_seconds(TODAY, 330)
        ])
    );
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::Delayed);
    assert_eq!(overlay.stats.blocks_replaced, 1);
}

#[test]
fn two_vehicles_on_one_headway_grid_run_keep_both_updates() {
    let mut net = three_stop_net();
    net.lookup.headway_trips.insert("t1".to_owned());
    let vehicle = |start: &str, delay: i32, timestamp: u64| {
        let mut one = update("t1", DATE_1, vec![delay_at(2, delay)]);
        one.trip.start_time = Some(start.to_owned());
        one.timestamp = Some(timestamp);
        one
    };
    let overlay = run(
        &net,
        vec![fresh(vec![
            vehicle("00:01:40", 30, 1_000),
            vehicle("00:03:00", 90, 2_000),
        ])],
    );

    // Neither update is a duplicate of the other, and riders can reach each one.
    assert_eq!(overlay.stats.duplicate_trip_updates, 0);
    assert_eq!(overlay.stats.trip_updates, 2);
    assert_eq!(overlay.rider.trips_of_run(key(0)).len(), 2);
    let feed = feed_id("kcm");
    let scoped = ScopedId::new(&feed, "t1");
    let early = overlay
        .rider
        .trip(&scoped, Some(date(DATE_1)), Some(100))
        .expect("the 00:01:40 vehicle");
    assert_eq!(early.delay, None);
    assert_eq!(early.stops[1].arrival_delay, Some(30));
    let late = overlay
        .rider
        .trip(&scoped, Some(date(DATE_1)), Some(180))
        .expect("the 00:03:00 vehicle");
    assert_eq!(late.stops[1].arrival_delay, Some(90));

    // The one grid run routes on the newest of them.
    assert_eq!(
        overlay.rider.trip_by_key(key(0)).map(|t| t.start_time),
        Some(Some(180))
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 100),
            window_seconds(TODAY, 290),
            window_seconds(TODAY, 390)
        ])
    );
}

#[test]
fn delayed_to_scheduled_when_the_update_vanishes() {
    let net = three_stop_net();
    let delayed = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![delay_at(1, 30)])])],
    );
    assert_eq!(delayed.rider.trip_state(key(0)), TripState::Delayed);
    let overlay = run(&net, vec![fresh(vec![])]);
    assert_eq!(overlay.stats.blocks_replaced, 0);
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::Scheduled);
}

#[test]
fn delayed_to_scheduled_when_the_delay_returns_to_zero() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![delay_at(1, 0)])])],
    );
    assert_eq!(overlay.stats.blocks_replaced, 0, "no change, no block");
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::Scheduled);
}

#[test]
fn propagation_stops_at_the_next_update() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update(
            "t1",
            DATE_1,
            vec![delay_at(1, 60), delay_at(3, 10)],
        )])],
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 160),
            window_seconds(TODAY, 260),
            window_seconds(TODAY, 310)
        ])
    );
}

#[test]
fn only_the_named_trip_of_a_block_changes() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![delay_at(1, 30)])])],
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 1),
        Some(vec![
            window_seconds(TODAY, 1100),
            window_seconds(TODAY, 1200),
            window_seconds(TODAY, 1300)
        ])
    );
}

// Stop skipped.

#[test]
fn scheduled_to_stop_skipped_forbids_boarding_there() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![skip_at(2)])])],
    );
    let lanes = lanes_of(&overlay.snapshot, 0, TODAY);
    let (_, _, _, rules) = lanes.iter().find(|(t, ..)| *t == 0).expect("trip");
    assert_eq!(rules[1], StopRules::SKIPPED);
    assert_eq!(rules[0], StopRules::REGULAR);
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::StopSkipped);
}

#[test]
fn stop_skipped_to_scheduled_when_the_skip_vanishes() {
    let net = three_stop_net();
    let skipped = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![skip_at(2)])])],
    );
    assert_eq!(skipped.rider.trip_state(key(0)), TripState::StopSkipped);
    let overlay = run(&net, vec![]);
    let lanes = lanes_of(&overlay.snapshot, 0, TODAY);
    assert!(lanes.iter().all(|(_, _, _, r)| r[1] == StopRules::REGULAR));
}

#[test]
fn delayed_to_stop_skipped_carries_the_delay_across() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update(
            "t1",
            DATE_1,
            vec![delay_at(1, 40), skip_at(2)],
        )])],
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 140),
            window_seconds(TODAY, 240),
            window_seconds(TODAY, 340)
        ])
    );
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::StopSkipped);
}

// Canceled.

#[test]
fn scheduled_to_canceled_removes_the_trip() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![with_relationship(
            update("t1", DATE_1, vec![]),
            Trip::Canceled,
        )])],
    );
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
    assert!(arrivals_of(&overlay.snapshot, 0, TODAY, 1).is_some());
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::Canceled);
}

#[test]
fn scheduled_to_deleted_also_removes_the_trip() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![with_relationship(
            update("t2", DATE_1, vec![]),
            Trip::Deleted,
        )])],
    );
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 1), None);
    assert_eq!(overlay.rider.trip_state(key(1)), TripState::Canceled);
}

#[test]
fn canceled_only_touches_its_own_service_day() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![with_relationship(
            update("t1", DATE_1, vec![]),
            Trip::Canceled,
        )])],
    );
    assert!(arrivals_of(&overlay.snapshot, 0, 0, 0).is_some());
    assert!(arrivals_of(&overlay.snapshot, 0, 2, 0).is_some());
}

#[test]
fn canceled_to_scheduled_when_the_cancellation_vanishes() {
    let net = three_stop_net();
    let canceled = run(
        &net,
        vec![fresh(vec![with_relationship(
            update("t1", DATE_1, vec![]),
            Trip::Canceled,
        )])],
    );
    assert_eq!(arrivals_of(&canceled.snapshot, 0, TODAY, 0), None);
    let overlay = run(&net, vec![fresh(vec![])]);
    assert!(arrivals_of(&overlay.snapshot, 0, TODAY, 0).is_some());
}

#[test]
fn canceled_wins_over_delayed() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![with_relationship(
            update("t1", DATE_1, vec![delay_at(1, 30)]),
            Trip::Canceled,
        )])],
    );
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
}

#[test]
fn every_trip_canceled_empties_the_block() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![
            with_relationship(update("t1", DATE_1, vec![]), Trip::Canceled),
            with_relationship(update("t2", DATE_1, vec![]), Trip::Canceled),
        ])],
    );
    assert_eq!(lanes_of(&overlay.snapshot, 0, TODAY), vec![]);
    assert_eq!(overlay.stats.blocks_removed, 1);
}

// Added and replaced.

fn timed_new_trip(trip: &str, stops: &[(&str, i64)]) -> proto::TripUpdate {
    let updates = stops
        .iter()
        .map(|&(stop, unix)| time_at(stop, unix))
        .collect();
    update(trip, DATE_1, updates)
}

fn unix_on_today(relative: i64) -> i64 {
    DAY_0 + DAY_SECONDS + relative
}

#[test]
fn scheduled_to_replaced_by_replacement_moves_to_its_own_pattern() {
    let net = three_stop_net();
    let replacement = with_relationship(
        timed_new_trip(
            "t1",
            &[("A", unix_on_today(500)), ("C", unix_on_today(700))],
        ),
        Trip::Replacement,
    );
    let overlay = run(&net, vec![fresh(vec![replacement])]);
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
    assert_eq!(overlay.stats.added_routes, 1);
    assert_eq!(
        overlay
            .snapshot
            .route_stops(RouteId::new(1))
            .expect("added")
            .len(),
        2
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 1, TODAY, 0),
        Some(vec![window_seconds(TODAY, 500), window_seconds(TODAY, 700)])
    );
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::Replaced);
}

#[test]
fn replaced_to_scheduled_restores_the_original_pattern() {
    let net = three_stop_net();
    let replacement = with_relationship(
        timed_new_trip(
            "t1",
            &[("A", unix_on_today(500)), ("C", unix_on_today(700))],
        ),
        Trip::Replacement,
    );
    let replaced = run(&net, vec![fresh(vec![replacement])]);
    assert_eq!(replaced.stats.added_routes, 1);
    let overlay = run(&net, vec![fresh(vec![])]);
    assert!(arrivals_of(&overlay.snapshot, 0, TODAY, 0).is_some());
    assert_eq!(overlay.stats.added_routes, 0);
}

#[test]
fn assigned_stop_id_replaces_the_pattern() {
    let net = NetBuilder::new()
        .route("R1", &["A", "B", "C"], &[("t1", &[100, 200, 300])])
        .route("R2", &["D", "E"], &[("x", &[100, 200])])
        .build();
    let mut moved = delay_at(2, 0);
    moved.stop_time_properties = Some(proto::trip_update::stop_time_update::StopTimeProperties {
        assigned_stop_id: Some("D".into()),
        ..Default::default()
    });
    let overlay = run(&net, vec![fresh(vec![update("t1", DATE_1, vec![moved])])]);
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
    let stops = overlay
        .snapshot
        .route_stops(RouteId::new(2))
        .expect("added");
    assert_eq!(stops[1], net.lookup.stop_number(KCM, "D"));
}

/// An update naming its stop by `stop_id` names the original stop, so its times still apply.
#[test]
fn an_assigned_stop_named_by_stop_id_keeps_its_times() {
    let net = NetBuilder::new()
        .route("R1", &["A", "B", "C"], &[("t1", &[100, 200, 300])])
        .route("R2", &["D", "E"], &[("x", &[100, 200])])
        .build();
    let mut moved = time_at("B", unix_on_today(260));
    moved.stop_time_properties = Some(proto::trip_update::stop_time_update::StopTimeProperties {
        assigned_stop_id: Some("D".into()),
        ..Default::default()
    });
    let overlay = run(&net, vec![fresh(vec![update("t1", DATE_1, vec![moved])])]);
    assert_eq!(overlay.stats.stop_updates_unmatched, 0);
    let stops = overlay
        .snapshot
        .route_stops(RouteId::new(2))
        .expect("added");
    assert_eq!(stops[1], net.lookup.stop_number(KCM, "D"));
    assert_eq!(
        arrivals_of(&overlay.snapshot, 2, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 100),
            window_seconds(TODAY, 260),
            window_seconds(TODAY, 360),
        ])
    );
}

#[test]
fn an_added_trip_at_an_absurd_time_is_unusable() {
    let net = three_stop_net();
    let far = unix_on_today(1_000_000);
    let added = with_relationship(
        timed_new_trip("extra", &[("A", far), ("B", far + 100)]),
        Trip::New,
    );
    let overlay = run(&net, vec![fresh(vec![added])]);
    assert_eq!(overlay.stats.invalid_added_trips, 1);
}

#[test]
fn no_trip_to_added_lays_out_a_new_pattern() {
    let net = three_stop_net();
    let added = with_relationship(
        timed_new_trip(
            "extra",
            &[("A", unix_on_today(400)), ("B", unix_on_today(500))],
        ),
        Trip::New,
    );
    let overlay = run(&net, vec![fresh(vec![added])]);
    assert_eq!(overlay.stats.added_routes, 1);
    let trip_id = net.lookup.first_unused_trip_id_for_test();
    assert_eq!(
        arrivals_of(&overlay.snapshot, 1, TODAY, trip_id),
        Some(vec![window_seconds(TODAY, 400), window_seconds(TODAY, 500)])
    );
    assert_eq!(
        overlay.rider.trip_state(TripKey {
            trip: TripId::new(trip_id),
            date: valinor_transit::ServiceDate::new(1),
        }),
        TripState::Added
    );
}

#[test]
fn deprecated_added_counts_as_added() {
    let net = three_stop_net();
    let added = with_relationship(
        timed_new_trip(
            "extra",
            &[("A", unix_on_today(400)), ("B", unix_on_today(500))],
        ),
        Trip::Added,
    );
    let overlay = run(&net, vec![fresh(vec![added])]);
    assert_eq!(overlay.stats.added_routes, 1);
}

#[test]
fn added_to_gone_when_it_vanishes() {
    let net = three_stop_net();
    let added = with_relationship(
        timed_new_trip(
            "extra",
            &[("A", unix_on_today(400)), ("B", unix_on_today(500))],
        ),
        Trip::New,
    );
    let present = run(&net, vec![fresh(vec![added])]);
    assert_eq!(present.snapshot.route_count(), 2);
    let overlay = run(&net, vec![fresh(vec![])]);
    assert_eq!(overlay.snapshot.route_count(), 1);
}

#[test]
fn added_matching_a_static_trip_replaces_rather_than_doubles() {
    let net = three_stop_net();
    let added = with_relationship(
        timed_new_trip(
            "t1",
            &[("A", unix_on_today(400)), ("C", unix_on_today(500))],
        ),
        Trip::Added,
    );
    let overlay = run(&net, vec![fresh(vec![added])]);
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::Replaced);
}

#[test]
fn added_without_times_is_unusable_but_still_shown() {
    let net = three_stop_net();
    let added = with_relationship(
        update(
            "extra",
            DATE_1,
            vec![StopTimeUpdate {
                stop_id: Some("A".into()),
                ..Default::default()
            }],
        ),
        Trip::New,
    );
    let overlay = run(&net, vec![fresh(vec![added])]);
    assert_eq!(overlay.stats.invalid_added_trips, 1);
    assert_eq!(overlay.stats.added_routes, 0);
    assert!(
        overlay
            .rider
            .trip(
                &ScopedId::new(&feed_id(KCM), "extra"),
                Some(date(DATE_1)),
                None
            )
            .is_some()
    );
}

#[test]
fn duplicated_copies_a_trip_onto_another_time_and_day() {
    let net = three_stop_net();
    let mut duplicate = with_relationship(update("t1", DATE_1, vec![]), Trip::Duplicated);
    duplicate.trip_properties = Some(proto::trip_update::TripProperties {
        trip_id: Some("t1-dup".into()),
        start_date: Some(DATE_2.into()),
        start_time: Some("00:10:00".into()),
        ..Default::default()
    });
    let overlay = run(&net, vec![fresh(vec![duplicate])]);
    let copy = net.lookup.first_unused_trip_id_for_test();
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, 2, copy),
        Some(vec![
            window_seconds(2, 600),
            window_seconds(2, 700),
            window_seconds(2, 800)
        ])
    );
    assert!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0).is_some(),
        "original stays"
    );
}

// New stops.

fn new_stop(id: &str, latitude: f32, longitude: f32) -> proto::FeedEntity {
    proto::FeedEntity {
        id: format!("stop-{id}"),
        stop: Some(proto::Stop {
            stop_id: Some(id.into()),
            stop_lat: Some(latitude),
            stop_lon: Some(longitude),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn updates_with_stops(updates: Vec<proto::TripUpdate>, stops: Vec<proto::FeedEntity>) -> FeedInput {
    let mut entity: Vec<proto::FeedEntity> = updates.into_iter().map(entity_of_update).collect();
    entity.extend(stops);
    input(KCM, FeedKind::Combined, NOW, entity)
}

#[test]
fn a_new_stop_is_appended_and_returned_with_its_coordinates() {
    let net = three_stop_net();
    let base_stops = net.base.stop_count();
    let overlay = run(
        &net,
        vec![updates_with_stops(
            vec![],
            vec![new_stop("N", 47.6, -122.3)],
        )],
    );
    assert_eq!(overlay.snapshot.stop_count(), base_stops + 1);
    let stop = overlay.new_stops.first().expect("returned");
    assert_eq!(stop.stop.into_inner() as usize, base_stops);
    assert_eq!(stop.id, ScopedId::new(&feed_id(KCM), "N"));
    assert!((stop.latitude - 47.6).abs() < 1e-3);
}

#[test]
fn a_stop_already_in_the_static_feed_is_not_new() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![updates_with_stops(vec![], vec![new_stop("A", 1.0, 1.0)])],
    );
    assert!(overlay.new_stops.is_empty());
}

#[test]
fn a_stop_without_usable_coordinates_is_dropped() {
    let net = three_stop_net();
    let bad = proto::FeedEntity {
        id: "x".into(),
        stop: Some(proto::Stop {
            stop_id: Some("N".into()),
            stop_lat: Some(f32::NAN),
            stop_lon: Some(0.0),
            ..Default::default()
        }),
        ..Default::default()
    };
    let overlay = run(
        &net,
        vec![updates_with_stops(
            vec![],
            vec![bad, new_stop("M", 95.0, 0.0)],
        )],
    );
    assert!(overlay.new_stops.is_empty());
    assert_eq!(overlay.stats.invalid_stops, 2);
}

#[test]
fn added_may_use_a_new_stop() {
    let net = three_stop_net();
    let trip = with_relationship(
        timed_new_trip(
            "extra",
            &[("A", unix_on_today(400)), ("N", unix_on_today(500))],
        ),
        Trip::New,
    );
    let overlay = run(
        &net,
        vec![updates_with_stops(
            vec![trip],
            vec![new_stop("N", 47.6, -122.3)],
        )],
    );
    let added = overlay
        .snapshot
        .route_stops(RouteId::new(1))
        .expect("added");
    assert_eq!(added[1], overlay.new_stops[0].stop);
}

#[test]
fn a_new_stop_disappears_when_its_feed_goes_stale() {
    let net = three_stop_net();
    let mut feed = updates_with_stops(vec![], vec![new_stop("N", 47.6, -122.3)]);
    feed.stale_after = 60;
    let stale = build_overlay(
        &net.lookup,
        Arc::clone(&net.base),
        vec![feed],
        NOW + 3600,
        OverlayOptions::default(),
    )
    .expect("builds");
    assert!(stale.new_stops.is_empty());
    assert_eq!(stale.snapshot.stop_count(), net.base.stop_count());
}

// `TripModifications`.

fn detour(trip: &str, service_date: &str) -> proto::FeedEntity {
    proto::FeedEntity {
        id: "mods-1".into(),
        trip_modifications: Some(proto::TripModifications {
            selected_trips: vec![proto::trip_modifications::SelectedTrips {
                trip_ids: vec![trip.into()],
                shape_id: None,
            }],
            start_times: vec![],
            service_dates: vec![service_date.into()],
            modifications: vec![proto::trip_modifications::Modification {
                start_stop_selector: Some(proto::StopSelector {
                    stop_sequence: Some(2),
                    stop_id: None,
                }),
                end_stop_selector: Some(proto::StopSelector {
                    stop_sequence: Some(2),
                    stop_id: None,
                }),
                propagated_modification_delay: Some(60),
                replacement_stops: vec![proto::ReplacementStop {
                    travel_time_to_stop: Some(50),
                    stop_id: Some("N".into()),
                }],
                service_alert_id: None,
                last_modified_time: None,
            }],
        }),
        ..Default::default()
    }
}

#[test]
fn trip_modifications_replace_the_pattern() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![updates_with_stops(
            vec![],
            vec![new_stop("N", 47.6, -122.3), detour("t1", DATE_1)],
        )],
    );
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
    let stops = overlay
        .snapshot
        .route_stops(RouteId::new(1))
        .expect("added");
    assert_eq!(stops[1], overlay.new_stops[0].stop);
    assert_eq!(
        arrivals_of(&overlay.snapshot, 1, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 100),
            window_seconds(TODAY, 150),
            window_seconds(TODAY, 360)
        ])
    );
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::Replaced);
}

/// A feed spending its whole modification budget doesn't stop another feed's modifications.
#[test]
fn a_flood_of_modifications_in_one_feed_leaves_another_alone() {
    let net = NetBuilder::new()
        .route(
            "R1",
            &["A", "B", "C"],
            &[("t1", &[100, 200, 300]), ("t2", &[1100, 1200, 1300])],
        )
        .in_feed("noisy")
        .route("R1", &["A", "B"], &[("t1", &[500, 600])])
        .build();
    let flood = proto::FeedEntity {
        id: "noisy-mods".into(),
        trip_modifications: Some(proto::TripModifications {
            selected_trips: vec![proto::trip_modifications::SelectedTrips {
                trip_ids: (0..60_000).map(|i| format!("gone-{i}")).collect(),
                shape_id: None,
            }],
            start_times: vec![],
            service_dates: vec![DATE_1.into()],
            modifications: vec![proto::trip_modifications::Modification::default()],
        }),
        ..Default::default()
    };
    let overlay = run(
        &net,
        vec![
            input("noisy", FeedKind::Combined, NOW, vec![flood]),
            input(
                KCM,
                FeedKind::Combined,
                NOW,
                vec![new_stop("N", 47.6, -122.3), detour("t1", DATE_1)],
            ),
        ],
    );
    assert_eq!(
        overlay.rider.trip_state(key(0)),
        TripState::Replaced,
        "the quiet feed's detour was dropped"
    );
    assert_eq!(overlay.stats.feeds_over_budget, vec![feed_id("noisy")]);
}

/// Each feed's limit is an equal share of the whole, and never more than one feed's limit.
#[test]
fn feed_limits_keep_a_whole_overlay_bounded() {
    let one = crate::overlay::tests_limits_for(1);
    assert_eq!(one.0, 4_096);
    let many = crate::overlay::tests_limits_for(26);
    assert_eq!(many.0, 65_536 / 26);
    assert!(many.0 * 26 <= 65_536);
    let huge = crate::overlay::tests_limits_for(100_000);
    assert!(huge.0 >= 1);
    assert!(many.1 * 26 <= 10_000);
}

#[test]
fn a_modified_trip_update_applies_to_the_detoured_times() {
    let net = three_stop_net();
    let mut trip_update = update("", DATE_1, vec![delay_at(3, 30)]);
    trip_update.trip = proto::TripDescriptor {
        modified_trip: Some(proto::trip_descriptor::ModifiedTripSelector {
            modifications_id: Some("mods-1".into()),
            affected_trip_id: Some("t1".into()),
            start_date: Some(DATE_1.into()),
            start_time: None,
        }),
        ..Default::default()
    };
    let overlay = run(
        &net,
        vec![updates_with_stops(
            vec![trip_update],
            vec![new_stop("N", 47.6, -122.3), detour("t1", DATE_1)],
        )],
    );
    // 300 + 60 from the modification, + 30 from the update.
    assert_eq!(
        arrivals_of(&overlay.snapshot, 1, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 100),
            window_seconds(TODAY, 150),
            window_seconds(TODAY, 390)
        ])
    );
}

// No service: every scope.

fn removed_by(net: &Net, selectors: Vec<proto::EntitySelector>) -> RealtimeOverlay {
    run(net, vec![alerts_feed(vec![no_service(selectors)])])
}

fn multi_net() -> Net {
    NetBuilder::new()
        .route_with(
            "R1",
            Some("metro"),
            3,
            Some(0),
            1,
            &["A", "B"],
            &[("t1", &[100, 200])],
        )
        .route_with(
            "R2",
            Some("metro"),
            3,
            Some(1),
            1,
            &["B", "C"],
            &[("t2", &[300, 400])],
        )
        .route_with(
            "R3",
            Some("sound"),
            2,
            Some(0),
            1,
            &["C", "D"],
            &[("t3", &[500, 600])],
        )
        .build()
}

fn remaining(overlay: &RealtimeOverlay) -> Vec<u32> {
    let mut trips: Vec<u32> = (0..3)
        .flat_map(|route| lanes_of(&overlay.snapshot, route, TODAY))
        .map(|(t, ..)| t)
        .collect();
    trips.sort_unstable();
    trips
}

#[test]
fn no_service_at_agency_scope_removes_its_trips() {
    let net = multi_net();
    let overlay = removed_by(
        &net,
        vec![proto::EntitySelector {
            agency_id: Some("metro".into()),
            ..Default::default()
        }],
    );
    assert_eq!(remaining(&overlay), vec![2]);
}

#[test]
fn no_service_at_route_scope_removes_its_trips() {
    let net = multi_net();
    let overlay = removed_by(&net, vec![route_selector("R2")]);
    assert_eq!(remaining(&overlay), vec![0, 2]);
    assert_eq!(overlay.rider.trip_state(key(1)), TripState::NoService);
}

#[test]
fn no_service_at_route_type_scope_removes_its_trips() {
    let net = multi_net();
    let overlay = removed_by(
        &net,
        vec![proto::EntitySelector {
            route_type: Some(2),
            ..Default::default()
        }],
    );
    assert_eq!(remaining(&overlay), vec![0, 1]);
}

#[test]
fn no_service_at_direction_scope_removes_its_trips() {
    let net = multi_net();
    let overlay = removed_by(
        &net,
        vec![proto::EntitySelector {
            route_id: Some("R2".into()),
            direction_id: Some(1),
            ..Default::default()
        }],
    );
    assert_eq!(remaining(&overlay), vec![0, 2]);
}

#[test]
fn no_service_at_trip_scope_removes_that_trip_on_every_day_it_names() {
    let net = three_stop_net();
    let overlay = removed_by(&net, vec![trip_selector("t1", None)]);
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
    assert_eq!(arrivals_of(&overlay.snapshot, 0, 0, 0), None);
    assert!(arrivals_of(&overlay.snapshot, 0, TODAY, 1).is_some());
}

#[test]
fn no_service_with_a_trip_date_names_only_that_day() {
    let net = three_stop_net();
    let overlay = removed_by(&net, vec![trip_selector("t1", Some(DATE_1))]);
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
    assert!(arrivals_of(&overlay.snapshot, 0, 2, 0).is_some());
}

#[test]
fn no_service_at_stop_scope_removes_only_that_stop() {
    let net = three_stop_net();
    let overlay = removed_by(&net, vec![stop_selector("B")]);
    let lanes = lanes_of(&overlay.snapshot, 0, TODAY);
    assert_eq!(lanes.len(), 2, "the trips still run");
    assert!(lanes.iter().all(|(_, _, _, r)| r[1] == StopRules::SKIPPED));
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::StopSkipped);
}

#[test]
fn no_service_at_a_station_covers_its_platforms() {
    let mut net = three_stop_net();
    net.lookup.add_station(KCM, "STATION", &["B"]);
    let overlay = removed_by(&net, vec![stop_selector("STATION")]);
    let lanes = lanes_of(&overlay.snapshot, 0, TODAY);
    assert!(lanes.iter().all(|(_, _, _, r)| r[1] == StopRules::SKIPPED));
}

#[test]
fn selector_fields_are_combined_with_and() {
    let net = multi_net();
    // Route R1 and stop C: R1 doesn't serve C, so nothing is removed.
    let overlay = removed_by(
        &net,
        vec![proto::EntitySelector {
            route_id: Some("R1".into()),
            stop_id: Some("C".into()),
            ..Default::default()
        }],
    );
    assert_eq!(remaining(&overlay), vec![0, 1, 2]);
    assert_eq!(overlay.stats.blocks_replaced, 0);
}

#[test]
fn selectors_are_combined_with_or() {
    let net = multi_net();
    let overlay = removed_by(&net, vec![route_selector("R1"), route_selector("R3")]);
    assert_eq!(remaining(&overlay), vec![1]);
}

#[test]
fn an_empty_selector_informs_nothing() {
    let net = multi_net();
    let overlay = removed_by(&net, vec![proto::EntitySelector::default()]);
    assert_eq!(remaining(&overlay), vec![0, 1, 2]);
}

#[test]
fn a_direction_that_does_not_match_removes_nothing() {
    let net = multi_net();
    let overlay = removed_by(
        &net,
        vec![proto::EntitySelector {
            route_id: Some("R1".into()),
            direction_id: Some(1),
            ..Default::default()
        }],
    );
    assert_eq!(remaining(&overlay), vec![0, 1, 2]);
    assert_eq!(overlay.stats.invalid_selectors, 0);
}

#[test]
fn invalid_selectors_are_counted() {
    let net = multi_net();
    let overlay = removed_by(
        &net,
        vec![
            proto::EntitySelector {
                direction_id: Some(1),
                ..Default::default()
            },
            trip_selector("t1", Some("not-a-date")),
            proto::EntitySelector {
                trip: Some(proto::TripDescriptor {
                    route_id: Some("R1".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ],
    );
    assert_eq!(overlay.stats.invalid_selectors, 3);
    assert_eq!(remaining(&overlay), vec![0, 1, 2]);
}

#[test]
fn a_descriptor_without_trip_id_removes_the_run_it_names() {
    let net = three_stop_net();
    let overlay = removed_by(
        &net,
        vec![proto::EntitySelector {
            trip: Some(proto::TripDescriptor {
                route_id: Some("R1".into()),
                direction_id: Some(0),
                start_time: Some("00:01:40".into()),
                start_date: Some(DATE_1.into()),
                ..Default::default()
            }),
            ..Default::default()
        }],
    );
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
    assert!(arrivals_of(&overlay.snapshot, 0, TODAY, 1).is_some());
    assert!(arrivals_of(&overlay.snapshot, 0, 2, 0).is_some());
}

#[test]
fn an_agency_selector_matches_a_feed_that_omits_agency_id() {
    let net = NetBuilder::new()
        .route_with(
            "R1",
            None,
            3,
            Some(0),
            1,
            &["A", "B"],
            &[("t1", &[100, 200])],
        )
        .build();
    let overlay = removed_by(
        &net,
        vec![proto::EntitySelector {
            agency_id: Some("anything".into()),
            ..Default::default()
        }],
    );
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
}

#[test]
fn other_effects_do_not_change_routing() {
    let net = three_stop_net();
    for effect in [
        proto::alert::Effect::Detour,
        proto::alert::Effect::StopMoved,
        proto::alert::Effect::SignificantDelays,
    ] {
        let overlay = run(
            &net,
            vec![alerts_feed(vec![alert(effect, vec![route_selector("R1")])])],
        );
        assert_eq!(overlay.stats.blocks_replaced, 0, "{effect:?}");
        assert_eq!(overlay.rider.alerts().len(), 1, "still carried for display");
    }
}

#[test]
fn any_to_no_service_and_back_when_the_alert_ends() {
    let net = three_stop_net();
    let during_trip = during(
        no_service(vec![trip_selector("t1", None)]),
        Some(DAY_0 + DAY_SECONDS + 50),
        Some(DAY_0 + DAY_SECONDS + 150),
    );
    let overlay = run(&net, vec![alerts_feed(vec![during_trip])]);
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);

    let after_trip = during(
        no_service(vec![trip_selector("t1", None)]),
        Some(DAY_0 + DAY_SECONDS + 400),
        Some(DAY_0 + DAY_SECONDS + 500),
    );
    let overlay = run(&net, vec![alerts_feed(vec![after_trip])]);
    assert!(arrivals_of(&overlay.snapshot, 0, TODAY, 0).is_some());
}

#[test]
fn active_periods_compare_against_the_trips_time_not_now() {
    // The alert isn't active now, but it will be when tomorrow's run departs.
    let net = three_stop_net();
    let tomorrow_trip_time = DAY_0 + 2 * DAY_SECONDS + 100;
    let alert = during(
        no_service(vec![route_selector("R1")]),
        Some(tomorrow_trip_time - 10),
        Some(tomorrow_trip_time + 10),
    );
    let overlay = run(&net, vec![alerts_feed(vec![alert])]);
    assert_eq!(arrivals_of(&overlay.snapshot, 0, 2, 0), None);
    assert!(arrivals_of(&overlay.snapshot, 0, TODAY, 0).is_some());
}

#[test]
fn stop_scope_uses_the_stops_own_time() {
    let net = three_stop_net();
    // Only stop B of t1 (window 100..=200 today) falls in the active period.
    let alert = during(
        no_service(vec![stop_selector("A")]),
        Some(DAY_0 + DAY_SECONDS + 500),
        Some(DAY_0 + DAY_SECONDS + 2000),
    );
    let overlay = run(&net, vec![alerts_feed(vec![alert])]);
    let lanes = lanes_of(&overlay.snapshot, 0, TODAY);
    let t1 = lanes.iter().find(|(t, ..)| *t == 0).expect("t1");
    assert_eq!(
        t1.3[0],
        StopRules::REGULAR,
        "stop A's visit is before the alert"
    );
    let t2 = lanes.iter().find(|(t, ..)| *t == 1).expect("t2");
    assert_eq!(
        t2.3[0],
        StopRules::SKIPPED,
        "stop A's visit is during the alert"
    );
}

#[test]
fn no_service_uses_the_delayed_time() {
    let net = three_stop_net();
    // Scheduled at 100, the alert ending at 150 wouldn't touch a run delayed past it.
    let delayed = update("t1", DATE_1, vec![delay_at(1, 500)]);
    let alert = during(
        no_service(vec![trip_selector("t1", None)]),
        Some(DAY_0 + DAY_SECONDS),
        Some(DAY_0 + DAY_SECONDS + 150),
    );
    let overlay = run(&net, vec![fresh(vec![delayed]), alerts_feed(vec![alert])]);
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 600),
            window_seconds(TODAY, 700),
            window_seconds(TODAY, 800)
        ])
    );
}

#[test]
fn no_service_wins_over_delayed() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![
            fresh(vec![update("t1", DATE_1, vec![delay_at(1, 30)])]),
            alerts_feed(vec![no_service(vec![trip_selector("t1", None)])]),
        ],
    );
    assert_eq!(arrivals_of(&overlay.snapshot, 0, TODAY, 0), None);
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::NoService);
}

#[test]
fn no_service_wins_over_replaced_and_added() {
    let net = three_stop_net();
    let replaced = with_relationship(
        timed_new_trip(
            "t1",
            &[("A", unix_on_today(500)), ("C", unix_on_today(700))],
        ),
        Trip::Replacement,
    );
    let mut added = with_relationship(
        timed_new_trip(
            "extra",
            &[("A", unix_on_today(800)), ("B", unix_on_today(900))],
        ),
        Trip::New,
    );
    added.trip.route_id = Some("R1".into());
    let overlay = run(
        &net,
        vec![
            fresh(vec![replaced, added]),
            alerts_feed(vec![no_service(vec![route_selector("R1")])]),
        ],
    );
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::NoService);
    assert_eq!(overlay.stats.added_routes, 0);
}

#[test]
fn no_service_can_name_a_trip_the_overlay_added() {
    let net = three_stop_net();
    let added = with_relationship(
        timed_new_trip(
            "extra",
            &[("A", unix_on_today(800)), ("B", unix_on_today(900))],
        ),
        Trip::New,
    );
    let overlay = run(
        &net,
        vec![
            fresh(vec![added]),
            alerts_feed(vec![no_service(vec![trip_selector("extra", None)])]),
        ],
    );
    assert_eq!(overlay.stats.added_routes, 0);
}

#[test]
fn alert_impacts_are_counted_per_alert() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![alerts_feed(vec![
            no_service(vec![route_selector("R1")]),
            no_service(vec![stop_selector("B")]),
        ])],
    );
    let impacts = &overlay.stats.alert_impacts;
    assert_eq!(
        impacts[&0].trips_removed, 6,
        "two trips on each of three days"
    );
    assert_eq!(impacts[&1].stops_removed, 6);
}

// Stale.

#[test]
fn a_feed_that_fails_to_decode_degrades_to_schedule_and_says_why() {
    let net = three_stop_net();
    let good = fresh(vec![update("t2", DATE_1, vec![delay_at(1, 30)])]);
    let bad = FeedInput::from_bytes(feed_id(KCM), FeedKind::Alerts, &[0xff, 0xff]);
    let overlay = run(&net, vec![good, bad]);
    assert_eq!(
        overlay.rider.trip_state(key(1)),
        TripState::Delayed,
        "the good feed still applies"
    );
    let statuses = overlay.rider.feed_status();
    assert_eq!(statuses.len(), 2);
    assert!(statuses[0].error.is_none());
    assert!(statuses[1].stale);
    assert!(statuses[1].error.is_some());
}

#[test]
fn delayed_to_scheduled_when_the_feed_goes_stale() {
    let net = three_stop_net();
    let feed = updates_at(
        NOW - DEFAULT_REALTIME_STALE_AFTER - 1,
        vec![update("t1", DATE_1, vec![delay_at(1, 30)])],
    );
    let overlay = run(&net, vec![feed]);
    assert_eq!(overlay.stats.blocks_replaced, 0);
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::Scheduled);
    let status = &overlay.rider.feed_status()[0];
    assert_eq!(status.age, Some(DEFAULT_REALTIME_STALE_AFTER + 1));
    assert!(status.stale);
}

#[test]
fn stop_skipped_to_scheduled_when_the_feed_goes_stale() {
    let net = three_stop_net();
    let feed = updates_at(
        NOW - DEFAULT_REALTIME_STALE_AFTER - 1,
        vec![update("t1", DATE_1, vec![skip_at(2)])],
    );
    let overlay = run(&net, vec![feed]);
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::Scheduled);
    let lanes = lanes_of(&overlay.snapshot, 0, TODAY);
    assert!(lanes.iter().all(|(_, _, _, r)| r[1] == StopRules::REGULAR));
}

#[test]
fn canceled_to_scheduled_when_the_feed_goes_stale() {
    let net = three_stop_net();
    let feed = updates_at(
        NOW - DEFAULT_REALTIME_STALE_AFTER - 1,
        vec![with_relationship(
            update("t1", DATE_1, vec![]),
            Trip::Canceled,
        )],
    );
    let overlay = run(&net, vec![feed]);
    assert!(arrivals_of(&overlay.snapshot, 0, TODAY, 0).is_some());
}

#[test]
fn replaced_and_added_to_scheduled_when_the_feed_goes_stale() {
    let net = three_stop_net();
    let replaced = with_relationship(
        timed_new_trip(
            "t1",
            &[("A", unix_on_today(500)), ("C", unix_on_today(700))],
        ),
        Trip::Replacement,
    );
    let added = with_relationship(
        timed_new_trip(
            "extra",
            &[("A", unix_on_today(800)), ("B", unix_on_today(900))],
        ),
        Trip::New,
    );
    let overlay = run(
        &net,
        vec![updates_at(
            NOW - DEFAULT_REALTIME_STALE_AFTER - 1,
            vec![replaced, added],
        )],
    );
    assert_eq!(overlay.snapshot.route_count(), 1);
    assert!(arrivals_of(&overlay.snapshot, 0, TODAY, 0).is_some());
}

#[test]
fn no_service_ends_when_the_alert_feed_goes_stale() {
    let net = three_stop_net();
    let mut feed = alerts_feed(vec![no_service(vec![route_selector("R1")])]);
    feed.stale_after = 60;
    let overlay = build_overlay(
        &net.lookup,
        Arc::clone(&net.base),
        vec![feed],
        NOW + 120,
        OverlayOptions::default(),
    )
    .expect("builds");
    assert_eq!(overlay.stats.blocks_replaced, 0);
    assert!(overlay.rider.alerts().is_empty());
}

// Overtaking and lanes.

#[test]
fn delays_that_overtake_split_the_block_into_lanes() {
    let net = NetBuilder::new()
        .route(
            "R1",
            &["A", "B"],
            &[
                ("t1", &[100, 200]),
                ("t2", &[150, 250]),
                ("t3", &[300, 400]),
            ],
        )
        .build();
    // t1 falls so far behind that t2 overtakes it between A and B.
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![delay_at(2, 100)])])],
    );
    let lanes = lanes_of(&overlay.snapshot, 0, TODAY);
    let lane_of = |trip: u32| {
        lanes
            .iter()
            .find(|(t, ..)| *t == trip)
            .map(|(_, lane, ..)| *lane)
    };
    assert_ne!(
        lane_of(0),
        lane_of(1),
        "overtaking trips can't share a lane"
    );
    assert_eq!(overlay.stats.overtaking_blocks, 1);
    assert_eq!(overlay.stats.overtaking_blocks_new, 1);
    assert_eq!(overlay.stats.overtaking_lanes, 1);
    assert_eq!(overlay.stats.max_lanes, 2);
}

#[test]
fn delays_without_overtaking_keep_one_lane() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![delay_at(1, 30)])])],
    );
    assert_eq!(overlay.stats.max_lanes, 1);
    assert_eq!(overlay.stats.overtaking_blocks, 0);
}

#[test]
fn skipped_trips_share_a_lane_only_with_trips_skipping_the_same_stops() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![skip_at(2)])])],
    );
    assert_eq!(overlay.stats.max_lanes, 2, "one per set of rules");
    assert_eq!(overlay.stats.overtaking_blocks, 0, "rules, not overtaking");
}

#[test]
fn replacement_blocks_are_contiguous_in_route_order() {
    let net = NetBuilder::new()
        .route("R1", &["A", "B"], &[("t1", &[100, 200])])
        .route("R2", &["C", "D"], &[("t2", &[100, 200])])
        .route("R3", &["E", "F"], &[("t3", &[100, 200])])
        .build();
    let overlay = run(
        &net,
        vec![fresh(vec![
            // Deliberately out of order in the feed.
            update("t3", DATE_0, vec![delay_at(1, 10)]),
            update("t1", DATE_2, vec![delay_at(1, 10)]),
            update("t2", DATE_1, vec![delay_at(1, 10)]),
            update("t1", DATE_0, vec![delay_at(1, 10)]),
        ])],
    );
    let order = [(0, 0), (0, 2), (1, 1), (2, 0)];
    let mut lane_indices = Vec::new();
    for (route, day) in order {
        let lanes = overlay
            .snapshot
            .block_lanes(RouteId::new(route), DaySlot::new(day))
            .expect("known");
        assert!(lanes.iter().all(|l| l.is_overlay()));
        lane_indices.extend(lanes.iter().map(|lane| lane.index()));
    }
    let expected: Vec<u32> = (0..u32::try_from(lane_indices.len()).expect("few")).collect();
    assert_eq!(
        lane_indices, expected,
        "laid out contiguously in route order"
    );
}

#[test]
fn added_routes_come_after_base_routes_in_the_arena() {
    let net = three_stop_net();
    let added = with_relationship(
        timed_new_trip(
            "extra",
            &[("A", unix_on_today(400)), ("B", unix_on_today(500))],
        ),
        Trip::New,
    );
    let overlay = run(
        &net,
        vec![fresh(vec![
            added,
            update("t1", DATE_1, vec![delay_at(1, 10)]),
        ])],
    );
    let base = overlay
        .snapshot
        .block_lanes(RouteId::new(0), DaySlot::new(TODAY))
        .expect("known");
    let extra = overlay
        .snapshot
        .block_lanes(RouteId::new(1), DaySlot::new(TODAY))
        .expect("known");
    assert!(base[0].index() < extra[0].index());
}

// Day boundaries.

#[test]
fn a_trip_after_midnight_with_start_date_yesterday_resolves_to_yesterday() {
    // A 25:00 trip is at 01:00 of the next calendar day but belongs to yesterday's service day.
    let net = NetBuilder::new()
        .route("R1", &["A", "B"], &[("late", &[90_000, 90_600])])
        .build();
    let delayed_time = DAY_0 + DAY_SECONDS + 90_000 + 120;
    let overlay = run(
        &net,
        vec![fresh(vec![update(
            "late",
            DATE_1,
            vec![StopTimeUpdate {
                stop_sequence: Some(1),
                arrival: Some(StopTimeEvent {
                    time: Some(delayed_time),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        )])],
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 90_000 + 120),
            window_seconds(TODAY, 90_600 + 120)
        ])
    );
    // The same run on the following day isn't touched.
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, 2, 0),
        Some(vec![window_seconds(2, 90_000), window_seconds(2, 90_600)])
    );
}

#[test]
fn a_start_date_outside_the_window_is_counted_and_ignored() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", "20250101", vec![delay_at(1, 30)])])],
    );
    assert_eq!(overlay.stats.trip_updates_outside_window, 1);
    assert_eq!(overlay.stats.blocks_replaced, 0);
}

#[test]
fn a_missing_start_date_picks_the_run_nearest_now() {
    // NOW is 15:00 on day 1, while this trip is running.
    let net = NetBuilder::new()
        .route("R1", &["A", "B"], &[("t1", &[53_000, 56_000])])
        .build();
    let mut trip = update("t1", DATE_1, vec![delay_at(1, 30)]);
    trip.trip.start_date = None;
    let overlay = run(&net, vec![fresh(vec![trip])]);
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 53_030),
            window_seconds(TODAY, 56_030)
        ])
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, 2, 0),
        Some(vec![window_seconds(2, 53_000), window_seconds(2, 56_000)])
    );
}

// Partial updates.

#[test]
fn a_partial_update_leaves_earlier_stops_on_schedule() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![delay_at(3, 90)])])],
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 100),
            window_seconds(TODAY, 200),
            window_seconds(TODAY, 390)
        ])
    );
}

#[test]
fn an_update_for_one_trip_leaves_the_others_untouched() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t2", DATE_1, vec![delay_at(1, 30)])])],
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 100),
            window_seconds(TODAY, 200),
            window_seconds(TODAY, 300)
        ])
    );
}

#[test]
fn unknown_trips_are_counted_and_still_shown() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("ghost", DATE_1, vec![delay_at(1, 30)])])],
    );
    assert_eq!(overlay.stats.trip_updates_unmatched, 1);
    assert!(
        overlay
            .rider
            .trip(
                &ScopedId::new(&feed_id(KCM), "ghost"),
                Some(date(DATE_1)),
                None
            )
            .is_some()
    );
}

#[test]
fn the_newest_update_of_a_trip_wins() {
    let net = three_stop_net();
    let mut older = update("t1", DATE_1, vec![delay_at(1, 10)]);
    older.timestamp = Some(5);
    let mut newer = update("t1", DATE_1, vec![delay_at(1, 40)]);
    newer.timestamp = Some(9);
    let overlay = run(&net, vec![fresh(vec![newer, older])]);
    assert_eq!(overlay.stats.duplicate_trip_updates, 1);
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0).map(|a| a[0]),
        Some(window_seconds(TODAY, 140))
    );
}

#[test]
fn the_newest_update_of_a_run_wins_under_any_spelling() {
    let net = three_stop_net();
    let mut older = update("t1", DATE_1, vec![delay_at(1, 10)]);
    older.timestamp = Some(5);
    older.trip.start_time = Some("00:01:40".into());
    let mut newer = update("t1", DATE_1, vec![delay_at(1, 40)]);
    newer.timestamp = Some(9);
    let overlay = run(&net, vec![fresh(vec![older, newer])]);
    assert_eq!(overlay.stats.duplicate_trip_updates, 1);
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0).map(|a| a[0]),
        Some(window_seconds(TODAY, 140))
    );
}

#[test]
fn stop_sequences_need_not_be_consecutive() {
    let net = NetBuilder::new()
        .route_with(
            "R1",
            Some("a"),
            3,
            None,
            10,
            &["A", "B"],
            &[("t1", &[100, 200])],
        )
        .build();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![delay_at(20, 30)])])],
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0).map(|a| a[1]),
        Some(window_seconds(TODAY, 230))
    );
}

// Bounds on what a feed can make a build do.

#[test]
fn modifications_stop_when_their_budget_runs_out() {
    let net = three_stop_net();
    let set = proto::TripModifications {
        selected_trips: vec![proto::trip_modifications::SelectedTrips {
            trip_ids: (0..60_000).map(|i| format!("nope-{i}")).collect(),
            shape_id: None,
        }],
        service_dates: vec![DATE_1.into()],
        ..Default::default()
    };
    let feed = input(
        KCM,
        FeedKind::TripUpdates,
        NOW,
        vec![proto::FeedEntity {
            id: "m".into(),
            trip_modifications: Some(set),
            ..Default::default()
        }],
    );
    let overlay = run(&net, vec![feed]);
    // 50,000 unknown trips, then one more count for the set cut short.
    assert_eq!(overlay.stats.invalid_modifications, 50_001);
}

#[test]
fn added_trips_past_the_limit_are_dropped_and_counted() {
    let net = three_stop_net();
    let updates = (0..4_100)
        .map(|i| {
            with_relationship(
                timed_new_trip(
                    &format!("extra-{i}"),
                    &[("A", unix_on_today(400 + i)), ("B", unix_on_today(500))],
                ),
                Trip::New,
            )
        })
        .collect();
    let overlay = run(&net, vec![fresh(updates)]);
    assert_eq!(overlay.stats.trips_added, 4_096);
    assert_eq!(overlay.stats.invalid_added_trips, 4);
}

#[test]
fn new_stops_past_the_limit_are_dropped_and_counted() {
    let net = three_stop_net();
    let stops = (0..10_002)
        .map(|i| new_stop(&format!("n{i}"), 47.0, -122.0))
        .collect();
    let overlay = run(&net, vec![updates_with_stops(vec![], stops)]);
    assert_eq!(overlay.new_stops.len(), 10_000);
    assert_eq!(overlay.stats.invalid_stops, 2);
}

// Several feeds.

#[test]
fn identifiers_of_different_feeds_never_collide() {
    let net = NetBuilder::new()
        .route("R1", &["A", "B"], &[("t1", &[100, 200])])
        .in_feed("other")
        .route("R1", &["A", "B"], &[("t1", &[500, 600])])
        .build();
    let overlay = run(
        &net,
        vec![
            input(
                "other",
                FeedKind::TripUpdates,
                NOW,
                vec![entity_of_update(update(
                    "t1",
                    DATE_1,
                    vec![delay_at(1, 30)],
                ))],
            ),
            alerts_feed(vec![no_service(vec![route_selector("R1")])]),
        ],
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 0, TODAY, 0),
        None,
        "kcm's route is out"
    );
    assert_eq!(
        arrivals_of(&overlay.snapshot, 1, TODAY, 1).map(|a| a[0]),
        Some(window_seconds(TODAY, 530)),
        "the other feed's route is delayed, not removed"
    );
}

// Rider data.

#[test]
fn a_removal_keeps_rider_details_for_the_other_stops() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![
            fresh(vec![update("t1", DATE_1, vec![delay_at(1, 60)])]),
            alerts_feed(vec![no_service(vec![stop_selector("B")])]),
        ],
    );
    let details = overlay.rider.trip_by_key(key(0)).expect("details");
    assert!(details.stops[1].removed_by_alert);
    assert!(!details.stops[0].removed_by_alert);
}

#[test]
fn rider_data_is_kept_for_trips_stops_routes_and_vehicles() {
    let net = three_stop_net();
    let mut trip = update("t1", DATE_1, vec![delay_at(1, 30)]);
    trip.trip.route_id = Some("R1".into());
    trip.timestamp = Some(123);
    trip.vehicle = Some(proto::VehicleDescriptor {
        id: Some("7033".into()),
        label: Some("7033".into()),
        ..Default::default()
    });
    let vehicle = proto::FeedEntity {
        id: "vehicle".into(),
        vehicle: Some(proto::VehiclePosition {
            trip: Some(descriptor("t1", DATE_1)),
            vehicle: Some(proto::VehicleDescriptor {
                id: Some("7033".into()),
                ..Default::default()
            }),
            position: Some(proto::Position {
                latitude: 47.6,
                longitude: -122.3,
                ..Default::default()
            }),
            current_stop_sequence: Some(1),
            ..Default::default()
        }),
        ..Default::default()
    };
    let overlay = run(
        &net,
        vec![
            fresh(vec![trip]),
            input(KCM, FeedKind::VehiclePositions, NOW, vec![vehicle]),
            alerts_feed(vec![{
                let mut a = alert(proto::alert::Effect::Detour, vec![route_selector("R1")]);
                a.header_text = Some(proto::TranslatedString {
                    translation: vec![proto::translated_string::Translation {
                        text: "Detour".into(),
                        language: Some("en".into()),
                    }],
                });
                a
            }]),
        ],
    );
    let feed = feed_id(KCM);
    let rider = &overlay.rider;
    let details = rider
        .trip(&ScopedId::new(&feed, "t1"), Some(date(DATE_1)), None)
        .expect("trip");
    assert_eq!(details.state, Some(TripState::Delayed));
    assert_eq!(details.stops[0].arrival_delay, Some(30));
    assert_eq!(
        details.stops[1].scheduled_arrival,
        Some(DAY_0 + DAY_SECONDS + 200)
    );
    assert_eq!(
        details.stops[1].predicted_arrival,
        Some(DAY_0 + DAY_SECONDS + 230),
        "propagated"
    );
    assert_eq!(
        details.stops[1].stop_id.as_deref(),
        Some("B"),
        "named from the timetable"
    );
    assert_eq!(
        rider.trip_by_key(key(0)).map(|t| t.trip_id.as_str()),
        Some("t1")
    );
    assert_eq!(rider.trips_on_route(&ScopedId::new(&feed, "R1")).len(), 1);
    assert!(!rider.stop_times_at(&ScopedId::new(&feed, "B")).is_empty());

    let bus = rider
        .vehicle(&ScopedId::new(&feed, "7033"))
        .expect("vehicle");
    assert_eq!(bus.bearing, None, "absent fields stay absent");
    assert_eq!(bus.trip_key, Some(key(0)));
    assert_eq!(rider.vehicles_on_trip(&ScopedId::new(&feed, "t1")).len(), 1);

    assert_eq!(rider.alerts_for_route(&ScopedId::new(&feed, "R1")).len(), 1);
    let shown = &rider.alerts()[0];
    assert_eq!(
        shown
            .header
            .as_ref()
            .map(|h| h.translation[0].text.as_str()),
        Some("Detour")
    );
    assert_eq!(rider.feed_status().len(), 3);
    assert_eq!(rider.feed_status()[0].age, Some(0));
}

#[test]
fn the_alert_scope_indexes_cover_every_field() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![alerts_feed(vec![alert(
            proto::alert::Effect::StopMoved,
            vec![proto::EntitySelector {
                agency_id: Some("agency".into()),
                route_type: Some(3),
                stop_id: Some("B".into()),
                ..Default::default()
            }],
        )])],
    );
    let feed = feed_id(KCM);
    let rider = &overlay.rider;
    assert_eq!(rider.alerts_for_stop(&ScopedId::new(&feed, "B")).len(), 1);
    assert_eq!(
        rider
            .alerts_for_agency(&ScopedId::new(&feed, "agency"))
            .len(),
        1
    );
    assert_eq!(rider.alerts_for_route_type(&feed, 3).len(), 1);
    assert!(rider.alerts_for_stop(&ScopedId::new(&feed, "C")).is_empty());
}

// Routing through the overlay.

/// A transfer search for networks with no walking between stops.
struct NoWalks;

impl TransferSearch for NoWalks {
    type Parent = ();
    type Walk = ();
    type Error = Infallible;

    fn begin_run(&mut self, _direction: Direction) {}

    fn walk(&self, _parent: ()) {}

    fn search<K: TransferSink<()>>(
        &mut self,
        _request: &TransferRequest<'_>,
        _sink: &mut K,
    ) -> Result<TransferOutcome, Infallible> {
        Ok(TransferOutcome::Completed)
    }
}

/// The arrival of the fastest journey from `from` to `to` departing at or after `depart`.
fn route(snapshot: &Arc<Snapshot>, net: &Net, from: &str, to: &str, depart: i32) -> Option<i32> {
    let count = snapshot.stop_count();
    let mut access = StopTable::new(count);
    access
        .set(net.lookup.stop_number(KCM, from), Seconds::ZERO)
        .expect("in table");
    let mut egress = StopTable::new(count);
    egress
        .set(net.lookup.stop_number(KCM, to), Seconds::ZERO)
        .expect("in table");
    let query = TransitQuery {
        snapshot: Arc::clone(snapshot),
        access: &access,
        egress: &egress,
        filter: RouteFilter::ALL,
        max_trips: 2,
    };
    let mut router = Router::new(NoWalks);
    let run = router
        .forward(&query, Time::try_new(depart).expect("in range"), &|| false)
        .expect("search");
    run.journey(1).map(|journey| journey.arrival.into_inner())
}

#[test]
fn routing_sees_a_delayed_trip() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![delay_at(1, 500)])])],
    );
    assert_eq!(
        route(&overlay.snapshot, &net, "A", "C", window_seconds(TODAY, 0)),
        Some(window_seconds(TODAY, 800))
    );
}

#[test]
fn routing_cannot_board_a_canceled_trip() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![with_relationship(
            update("t1", DATE_1, vec![]),
            Trip::Canceled,
        )])],
    );
    assert_eq!(
        route(&overlay.snapshot, &net, "A", "C", window_seconds(TODAY, 0)),
        Some(window_seconds(TODAY, 1300)),
        "the next trip takes over"
    );
}

#[test]
fn routing_cannot_alight_at_a_skipped_stop() {
    let net = three_stop_net();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![skip_at(3)])])],
    );
    assert_eq!(
        route(&overlay.snapshot, &net, "A", "C", window_seconds(TODAY, 0)),
        Some(window_seconds(TODAY, 1300))
    );
}

#[test]
fn routing_can_use_an_overtaking_trip() {
    let net = NetBuilder::new()
        .route(
            "R1",
            &["A", "B"],
            &[("t1", &[100, 200]), ("t2", &[150, 250])],
        )
        .build();
    let overlay = run(
        &net,
        vec![fresh(vec![update("t1", DATE_1, vec![delay_at(2, 200)])])],
    );
    assert_eq!(
        route(&overlay.snapshot, &net, "A", "B", window_seconds(TODAY, 0)),
        Some(window_seconds(TODAY, 250)),
        "t2 now arrives first even though t1 departed first"
    );
}

// Arbitrary feeds never break the build.

fn arbitrary_update() -> impl Strategy<Value = proto::TripUpdate> {
    let trip = prop_oneof![
        Just("t1".to_owned()),
        Just("t2".to_owned()),
        Just("new".to_owned()),
    ];
    let start_date = prop_oneof![
        Just(Some(DATE_0.to_owned())),
        Just(Some(DATE_1.to_owned())),
        Just(Some(DATE_2.to_owned())),
        Just(Some("bogus".to_owned())),
        Just(None),
    ];
    let relationship = prop_oneof![
        Just(None),
        Just(Some(0)),
        Just(Some(1)),
        Just(Some(2)),
        Just(Some(3)),
        Just(Some(5)),
        Just(Some(6)),
        Just(Some(7)),
        Just(Some(8)),
        Just(Some(99)),
    ];
    let stop_update = (
        prop::option::of(0_u32..5),
        prop::option::of(prop_oneof![Just("A"), Just("B"), Just("C"), Just("zz")]),
        prop::option::of(any::<i32>()),
        prop::option::of(any::<i64>()),
        prop::option::of(0_i32..4),
    )
        .prop_map(
            |(sequence, stop, delay, time, relationship)| StopTimeUpdate {
                stop_sequence: sequence,
                stop_id: stop.map(str::to_owned),
                arrival: delay.map(|delay| StopTimeEvent {
                    delay: Some(delay),
                    time,
                    ..Default::default()
                }),
                schedule_relationship: relationship,
                ..Default::default()
            },
        );
    (
        trip,
        start_date,
        relationship,
        prop::collection::vec(stop_update, 0..6),
        prop::option::of(any::<i32>()),
    )
        .prop_map(
            |(trip, start_date, relationship, stops, delay)| proto::TripUpdate {
                trip: proto::TripDescriptor {
                    trip_id: Some(trip),
                    start_date,
                    schedule_relationship: relationship,
                    ..Default::default()
                },
                stop_time_update: stops,
                delay,
                ..Default::default()
            },
        )
}

fn arbitrary_alert() -> impl Strategy<Value = proto::Alert> {
    let selector = (
        prop::option::of(prop_oneof![Just("R1"), Just("nope")]),
        prop::option::of(prop_oneof![Just("A"), Just("nope")]),
        prop::option::of(0_u32..2),
        prop::option::of(any::<i32>()),
    )
        .prop_map(
            |(route, stop, direction, route_type)| proto::EntitySelector {
                route_id: route.map(str::to_owned),
                stop_id: stop.map(str::to_owned),
                direction_id: direction,
                route_type,
                ..Default::default()
            },
        );
    (
        prop::collection::vec(selector, 0..4),
        prop::option::of(any::<u64>()),
        prop::option::of(any::<u64>()),
    )
        .prop_map(|(selectors, start, end)| {
            let mut alert = no_service(selectors);
            #[allow(deprecated)]
            alert.active_period.push(proto::TimeRange { start, end });
            alert
        })
}

fn arbitrary_stop() -> impl Strategy<Value = proto::FeedEntity> {
    (
        prop::option::of(prop_oneof![Just("N"), Just("A")]),
        prop::option::of(any::<f32>()),
        prop::option::of(any::<f32>()),
    )
        .prop_map(|(id, latitude, longitude)| proto::FeedEntity {
            id: "s".into(),
            stop: Some(proto::Stop {
                stop_id: id.map(str::to_owned),
                stop_lat: latitude,
                stop_lon: longitude,
                ..Default::default()
            }),
            ..Default::default()
        })
}

fn every_lane_is_ordered(snapshot: &Snapshot) -> bool {
    for route in 0..u32::try_from(snapshot.route_count()).expect("few") {
        for day in 0..3 {
            let lanes = snapshot
                .block_lanes(RouteId::new(route), DaySlot::new(day))
                .expect("known");
            for lane in lanes {
                let mut previous: Option<Vec<valinor_transit::StopEvent>> = None;
                for row in 0..snapshot.lane_trip_count(lane).expect("lane") {
                    let events = snapshot.lane_trip_events(lane, row).expect("events");
                    if let Some(earlier) = previous.as_ref()
                        && earlier.iter().zip(&events).any(|(a, b)| {
                            a.arrival > b.arrival || a.departure > b.departure || a.rules != b.rules
                        })
                    {
                        return false;
                    }
                    previous = Some(events.clone());
                    for pair in events.windows(2) {
                        if pair[0].departure > pair[1].arrival
                            || pair[0].arrival > pair[0].departure
                        {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn arbitrary_feeds_never_fail_the_build(
        updates in prop::collection::vec(arbitrary_update(), 0..12),
        alerts in prop::collection::vec(arbitrary_alert(), 0..6),
        stops in prop::collection::vec(arbitrary_stop(), 0..4),
    ) {
        let net = three_stop_net();
        let mut entity: Vec<proto::FeedEntity> =
            updates.into_iter().map(entity_of_update).collect();
        entity.extend(stops);
        let overlay = build_overlay(
            &net.lookup,
            Arc::clone(&net.base),
            vec![
                input(KCM, FeedKind::Combined, NOW, entity),
                alerts_feed(alerts),
            ],
            NOW,
            OverlayOptions::default(),
        ).expect("feed content can't fail a build");
        prop_assert!(every_lane_is_ordered(&overlay.snapshot));
    }

    #[test]
    fn arbitrary_bytes_never_reach_a_panic(bytes in prop::collection::vec(any::<u8>(), 0..1024)) {
        if let Ok(decoded) = crate::decode::DecodedFeed::decode(&bytes) {
            let net = three_stop_net();
            let feed = FeedInput {
                feed: feed_id(KCM),
                kind: FeedKind::Combined,
                decoded: Ok(decoded),
                received_at: Some(NOW),
                stale_after: i64::MAX,
                expires_at: None,
            };
            build_overlay(
                &net.lookup,
                Arc::clone(&net.base),
                vec![feed],
                NOW,
                OverlayOptions::default(),
            ).expect("feed content can't fail a build");
        }
    }
}

#[test]
fn a_build_is_deterministic() {
    let net = three_stop_net();
    let updates = || {
        vec![
            update("t2", DATE_1, vec![delay_at(1, 30)]),
            update("t1", DATE_1, vec![delay_at(1, 50)]),
        ]
    };
    let first = run(&net, vec![fresh(updates())]);
    let second = run(&net, vec![fresh(updates())]);
    assert_eq!(first.stats, second.stats);
    assert_eq!(
        lanes_of(&first.snapshot, 0, TODAY),
        lanes_of(&second.snapshot, 0, TODAY)
    );
}
