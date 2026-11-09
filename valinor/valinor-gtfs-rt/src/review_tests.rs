//! Regression tests from the independent review of this crate. Each states the expected behavior.
#![allow(deprecated)]

use std::sync::Arc;
use std::time::Instant;

use proptest::prelude::*;
use valinor_transit::{DaySlot, RouteId, Snapshot, StopRules, TripId, TripKey};

use crate::decode::DecodedFeed;
use crate::feed::{FeedInput, FeedKind, ScopedId};
use crate::overlay::{OverlayOptions, RealtimeOverlay, build_overlay};
use crate::proto;
use crate::proto::trip_descriptor::ScheduleRelationship as Trip;
use crate::proto::trip_update::StopTimeUpdate;
use crate::proto::trip_update::stop_time_update::ScheduleRelationship as Stop;
use crate::states::TripState;
use crate::testing::*;

const TODAY: u8 = 1;
const TODAY_UNIX: i64 = DAY_0 + DAY_SECONDS;

fn net() -> Net {
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

fn trips_in(snapshot: &Snapshot, route: u32, day: u8) -> Vec<(u32, Vec<i32>, Vec<StopRules>)> {
    // An added pattern whose only trip is removed is never created, so it holds no trips.
    let lanes = snapshot
        .block_lanes(RouteId::new(route), DaySlot::new(day))
        .unwrap_or_default();
    let mut out = Vec::new();
    for lane in lanes {
        for row in 0..snapshot.lane_trip_count(lane).expect("lane") {
            let key = snapshot.trip_key(lane, row).expect("key");
            let events = snapshot.lane_trip_events(lane, row).expect("events");
            out.push((
                key.trip.into_inner(),
                events.iter().map(|e| e.arrival.into_inner()).collect(),
                events.iter().map(|e| e.rules).collect(),
            ));
        }
    }
    out
}

fn arrivals(snapshot: &Snapshot, route: u32, day: u8, trip: u32) -> Option<Vec<i32>> {
    trips_in(snapshot, route, day)
        .into_iter()
        .find(|(t, _, _)| *t == trip)
        .map(|(_, a, _)| a)
}

fn key(trip: u32) -> TripKey {
    TripKey {
        trip: TripId::new(trip),
        date: valinor_transit::ServiceDate::new(u32::from(TODAY)),
    }
}

fn duplicate_at(start_time: &str, new_id: &str) -> proto::TripUpdate {
    let mut d = with_relationship(update("t1", DATE_1, vec![]), Trip::Duplicated);
    d.trip_properties = Some(proto::trip_update::TripProperties {
        trip_id: Some(new_id.into()),
        start_date: Some(DATE_1.into()),
        start_time: Some(start_time.into()),
        ..Default::default()
    });
    d
}

/// A `REPLACEMENT` running only A then C, today.
fn replacement_a_c(start_time: Option<&str>) -> proto::TripUpdate {
    let mut replaced = with_relationship(
        update(
            "t1",
            DATE_1,
            vec![
                time_at("A", TODAY_UNIX + 100),
                time_at("C", TODAY_UNIX + 300),
            ],
        ),
        Trip::Replacement,
    );
    replaced.trip.start_time = start_time.map(str::to_owned);
    replaced
}

// Alerts and rebuilt service.

#[test]
fn no_service_removes_a_duplicated_trip() {
    let net = net();
    let copy = net.lookup.first_unused_trip_id_for_test();
    let overlay = run(
        &net,
        vec![
            fresh(vec![duplicate_at("00:30:00", "t1-dup")]),
            alerts_feed(vec![no_service(vec![route_selector("R1")])]),
        ],
    );
    assert_eq!(
        arrivals(&overlay.snapshot, 0, TODAY, copy),
        None,
        "docs: NoService wins over Added"
    );
}

#[test]
fn a_stop_scoped_no_service_applies_to_a_duplicated_trip() {
    let net = net();
    let copy = net.lookup.first_unused_trip_id_for_test();
    let overlay = run(
        &net,
        vec![
            fresh(vec![duplicate_at("00:30:00", "t1-dup")]),
            alerts_feed(vec![no_service(vec![stop_selector("B")])]),
        ],
    );
    let (_, _, rules) = trips_in(&overlay.snapshot, 0, TODAY)
        .into_iter()
        .find(|(t, _, _)| *t == copy)
        .expect("copy runs");
    assert_eq!(rules[1], StopRules::SKIPPED);
}

fn direction_selector(route: &str, direction: u32) -> proto::EntitySelector {
    proto::EntitySelector {
        route_id: Some(route.into()),
        direction_id: Some(direction),
        ..Default::default()
    }
}

#[test]
fn direction_scoped_no_service_reaches_a_replaced_trip() {
    let net = net();
    let overlay = run(
        &net,
        vec![
            fresh(vec![replacement_a_c(None)]),
            alerts_feed(vec![no_service(vec![direction_selector("R1", 0)])]),
        ],
    );
    assert_eq!(overlay.rider.trip_state(key(0)), TripState::NoService);
    assert_eq!(arrivals(&overlay.snapshot, 1, TODAY, 0), None);
}

// A feed with two descriptors for the same run.

#[test]
fn a_cancellation_under_another_identity_removes_a_replaced_trip() {
    let net = net();
    let canceled = with_relationship(
        proto::TripUpdate {
            trip: proto::TripDescriptor {
                start_time: Some("00:01:40".into()),
                ..descriptor("t1", DATE_1)
            },
            ..Default::default()
        },
        Trip::Canceled,
    );
    let overlay = run(&net, vec![fresh(vec![replacement_a_c(None), canceled])]);
    assert_eq!(
        arrivals(&overlay.snapshot, 1, TODAY, 0),
        None,
        "canceled trip still runs on its added pattern"
    );
}

#[test]
fn a_trip_is_never_on_both_its_base_block_and_an_added_pattern() {
    let net = net();
    let mut delayed = update("t1", DATE_1, vec![delay_at(1, 60)]);
    delayed.trip.start_time = Some("00:01:40".into());
    let overlay = run(&net, vec![fresh(vec![replacement_a_c(None), delayed])]);
    let base = arrivals(&overlay.snapshot, 0, TODAY, 0).is_some();
    let added =
        overlay.snapshot.route_count() > 1 && arrivals(&overlay.snapshot, 1, TODAY, 0).is_some();
    assert!(!(base && added), "t1 is in both blocks");
}

// Duplicates.

#[test]
fn two_duplicates_of_one_trip_both_run() {
    let net = net();
    let first_copy = net.lookup.first_unused_trip_id_for_test();
    let overlay = run(
        &net,
        vec![fresh(vec![
            duplicate_at("00:30:00", "t1-dup-a"),
            duplicate_at("01:00:00", "t1-dup-b"),
        ])],
    );
    let copies = trips_in(&overlay.snapshot, 0, TODAY)
        .iter()
        .filter(|(t, _, _)| *t >= first_copy)
        .count();
    assert_eq!(
        copies, 2,
        "stats: {:?}",
        overlay.stats.duplicate_trip_updates
    );
}

// Selectors.

#[test]
fn a_descriptor_without_trip_id_names_one_run() {
    let net = net();
    let selector = proto::EntitySelector {
        trip: Some(proto::TripDescriptor {
            route_id: Some("R1".into()),
            direction_id: Some(0),
            start_time: Some("00:01:40".into()),
            start_date: Some(DATE_1.into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let overlay = run(&net, vec![alerts_feed(vec![no_service(vec![selector])])]);
    assert!(
        arrivals(&overlay.snapshot, 0, TODAY, 1).is_some(),
        "t2 runs: the selector named t1's start time"
    );
    assert!(
        arrivals(&overlay.snapshot, 0, 0, 0).is_some(),
        "another day runs"
    );
}

#[test]
fn a_direction_only_selector_is_invalid_and_removes_nothing() {
    let net = net();
    let selector = proto::EntitySelector {
        direction_id: Some(0),
        ..Default::default()
    };
    let overlay = run(&net, vec![alerts_feed(vec![no_service(vec![selector])])]);
    assert_eq!(overlay.stats.trips_removed, 0);
}

#[test]
fn an_unparseable_trip_date_does_not_widen_a_selector() {
    let net = net();
    let overlay = run(
        &net,
        vec![alerts_feed(vec![no_service(vec![trip_selector(
            "t1",
            Some("2026-01-02"),
        )])])],
    );
    assert!(
        arrivals(&overlay.snapshot, 0, 0, 0).is_some(),
        "garbage date must not match every day"
    );
    assert!(arrivals(&overlay.snapshot, 0, 2, 0).is_some());
}

// Delays.

#[test]
fn trip_delay_propagates_until_the_first_stop_update() {
    let net = net();
    let mut u = update("t1", DATE_1, vec![delay_at(3, 5)]);
    u.delay = Some(60);
    let overlay = run(&net, vec![fresh(vec![u])]);
    assert_eq!(
        arrivals(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 160),
            window_seconds(TODAY, 260),
            window_seconds(TODAY, 305),
        ]),
        "proto: trip-level delay is propagated until the next stop with a delay"
    );
}

#[test]
fn a_stop_id_update_on_a_loop_finds_the_visit_it_predicts() {
    // Producers may drop past stops; here only the second visit of A is reported.
    let net = NetBuilder::new()
        .route("L", &["A", "B", "A"], &[("t1", &[100, 200, 300])])
        .build();
    let overlay = run(
        &net,
        vec![fresh(vec![update(
            "t1",
            DATE_1,
            vec![time_at("A", TODAY_UNIX + 310)],
        )])],
    );
    assert_eq!(
        arrivals(&overlay.snapshot, 0, TODAY, 0),
        Some(vec![
            window_seconds(TODAY, 100),
            window_seconds(TODAY, 200),
            window_seconds(TODAY, 310),
        ])
    );
}

#[test]
fn a_prediction_a_year_off_is_not_used() {
    let net = net();
    let overlay = run(
        &net,
        vec![fresh(vec![update(
            "t1",
            DATE_1,
            vec![time_at("B", TODAY_UNIX + 200 - 365 * 86_400)],
        )])],
    );
    let got = arrivals(&overlay.snapshot, 0, TODAY, 0).expect("runs");
    assert!(
        got.iter().all(|&a| a >= 0),
        "a year-old timestamp moved the trip to {got:?}"
    );
}

// Rider data against routing.

#[test]
fn a_platforms_alert_is_found_from_the_platform() {
    let mut net = net();
    net.lookup.add_station("kcm", "STATION", &["A"]);
    let overlay = run(
        &net,
        vec![alerts_feed(vec![no_service(vec![stop_selector(
            "STATION",
        )])])],
    );
    assert_eq!(
        overlay
            .stats
            .alert_impacts
            .values()
            .map(|i| i.stops_removed)
            .sum::<usize>(),
        6,
        "routing removed A on both trips, three days"
    );
    let id = ScopedId::new(&feed_id("kcm"), "A");
    assert_eq!(
        overlay.rider.alerts_for_stop(&id).len(),
        1,
        "a leg at the platform must be able to explain the removal"
    );
}

#[test]
fn rider_delay_matches_the_time_routing_used() {
    let net = net();
    let overlay = run(
        &net,
        vec![fresh(vec![update(
            "t1",
            DATE_1,
            vec![delay_at(1, 300), delay_at(3, -50)],
        )])],
    );
    let details = overlay.rider.trip_by_key(key(0)).expect("details");
    let stop = &details.stops[2];
    let predicted = stop.predicted_arrival.expect("prediction");
    let scheduled = stop.scheduled_arrival.expect("schedule");
    assert_eq!(
        stop.arrival_delay,
        Some(i32::try_from(predicted - scheduled).expect("small")),
        "delay and prediction disagree"
    );
}

#[test]
fn an_alert_removed_stop_is_marked_in_rider_details() {
    let net = net();
    let overlay = run(
        &net,
        vec![
            fresh(vec![update("t1", DATE_1, vec![delay_at(1, 60)])]),
            alerts_feed(vec![no_service(vec![stop_selector("B")])]),
        ],
    );
    let details = overlay.rider.trip_by_key(key(0)).expect("details");
    assert_eq!(details.stops[1].relationship, Some(Stop::Skipped));
}

// Hostile input.

#[test]
fn memory_is_proportionate_to_the_message() {
    let size = size_of::<proto::FeedEntity>();
    let header = proto::FeedHeader {
        gtfs_realtime_version: "2.0".into(),
        ..Default::default()
    };
    let mut bytes = prost::Message::encode_to_vec(&proto::FeedMessage {
        header,
        entity: vec![],
    });
    let count = 200_000;
    for _ in 0..count {
        bytes.extend_from_slice(&[0x12, 0x00]);
    }
    let decoded = DecodedFeed::decode(&bytes).expect("valid protobuf");
    let held = decoded.message().entity.len() * size;
    println!(
        "{} bytes -> {} entities of {size} bytes = {held} bytes",
        bytes.len(),
        decoded.message().entity.len()
    );
    assert!(
        held < bytes.len() * 32,
        "{} input bytes hold {held} bytes",
        bytes.len()
    );
}

#[test]
fn trip_modifications_cost_is_not_the_product_of_their_lists() {
    let net = net();
    let n = 300;
    let set = proto::TripModifications {
        selected_trips: vec![proto::trip_modifications::SelectedTrips {
            trip_ids: vec!["nope".into(); n],
            shape_id: None,
        }],
        start_times: vec!["00:00:00".into(); n],
        service_dates: vec![DATE_1.into(); n],
        modifications: vec![],
    };
    let feed = input(
        "kcm",
        FeedKind::TripUpdates,
        NOW,
        vec![proto::FeedEntity {
            id: "m".into(),
            trip_modifications: Some(set),
            ..Default::default()
        }],
    );
    let wire = prost::Message::encoded_len(feed.decoded.as_ref().expect("decoded").message());
    let started = Instant::now();
    let overlay = run(&net, vec![feed]);
    let elapsed = started.elapsed();
    println!(
        "{wire} wire bytes: {elapsed:?}, invalid {}",
        overlay.stats.invalid_modifications
    );
    assert!(elapsed.as_millis() < 100, "{elapsed:?} for {wire} bytes");
}

// Property: anything a feed says yields a valid overlay.

fn any_stu() -> impl Strategy<Value = StopTimeUpdate> {
    (
        prop::option::of(0_u32..6),
        prop::option::of(prop_oneof![Just("A"), Just("B"), Just("C"), Just("Z")]),
        prop::option::of((
            prop::option::of(any::<i32>()),
            prop::option::of(any::<i64>()),
        )),
        prop::option::of((
            prop::option::of(-90_000_i32..90_000),
            prop::option::of(TODAY_UNIX - 90_000..TODAY_UNIX + 90_000),
        )),
        0_i32..4,
    )
        .prop_map(|(sequence, stop, arrival, departure, relationship)| {
            let event =
                |(delay, time): (Option<i32>, Option<i64>)| proto::trip_update::StopTimeEvent {
                    delay,
                    time,
                    ..Default::default()
                };
            StopTimeUpdate {
                stop_sequence: sequence,
                stop_id: stop.map(str::to_owned),
                arrival: arrival.map(event),
                departure: departure.map(event),
                schedule_relationship: Some(relationship),
                ..Default::default()
            }
        })
}

proptest! {
    #[test]
    fn any_stop_updates_build_a_valid_overlay(
        updates in prop::collection::vec(any_stu(), 0..8),
        trip_delay in prop::option::of(any::<i32>()),
    ) {
        let net = net();
        let mut u = update("t1", DATE_1, updates);
        u.delay = trip_delay;
        let overlay = build_overlay(
            &net.lookup,
            Arc::clone(&net.base),
            vec![fresh(vec![u])],
            NOW,
            OverlayOptions::default(),
        );
        let overlay = overlay.expect("feed content never fails the build");
        for (trip, a, _) in trips_in(&overlay.snapshot, 0, TODAY) {
            prop_assert!(a.windows(2).all(|w| w[0] <= w[1]), "trip {} {:?}", trip, a);
        }
    }
}
