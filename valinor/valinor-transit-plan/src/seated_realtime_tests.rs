//! How GTFS-RT, leg fields, and wheelchair routing apply to stay-seated combined trips.

use valinor_gtfs::{
    FeedId as CompileFeedId, FeedInput, InMemorySource, LatLon, LoadedTimetable, compile,
};
use valinor_gtfs_rt::FeedKind;
use valinor_gtfs_rt::proto;
use valinor_transit::Router;

use crate::plan::{TransitOptions, TransitRequest, TravelMode, plan};
use crate::response::{Accessibility, JourneyLeg, PlanResponse, TransitLeg};
use crate::state::{FeedUpdate, StateConfig, TransitState};
use crate::testing::{StraightLineAccess, StraightLineTransfers, encoded_feed, june_15};

const FEED_TIME: i64 = 1_781_535_000; // 2026-06-15 08:10 PDT.
const JUNE_15: &str = "20260615";
const FEED: &str = "seated";

fn origin() -> LatLon {
    LatLon {
        lat: 47.5995,
        lon: -122.3300,
    }
}
fn mid_ll() -> LatLon {
    LatLon {
        lat: 47.6100,
        lon: -122.3200,
    }
}
fn destination() -> LatLon {
    LatLon {
        lat: 47.6204,
        lon: -122.3098,
    }
}

fn state_of(trips: &str, stop_times: &str) -> TransitState {
    state_with_routes(
        "route_id,agency_id,route_short_name,route_type\nR1,A,1,3\nR2,A,2,3\n",
        trips,
        stop_times,
    )
}

fn state_with_routes(routes: &str, trips: &str, stop_times: &str) -> TransitState {
    state_with(routes, STOPS, trips, stop_times)
}

const STOPS: &str = "stop_id,stop_name,stop_lat,stop_lon,wheelchair_boarding\n\
     ORG,Origin,47.5995,-122.3300,1\n\
     MID,Middle,47.6100,-122.3200,1\n\
     DST,Destination,47.6204,-122.3098,1\n\
     ALT,Alternate,47.6210,-122.3098,1\n";

fn state_with(routes: &str, stops: &str, trips: &str, stop_times: &str) -> TransitState {
    let source = InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\n\
             A,Metro,America/Los_Angeles,https://m.test\n",
        )
        .with("routes.txt", routes)
        .with("stops.txt", stops)
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with("trips.txt", trips)
        .with("stop_times.txt", stop_times);
    let compiled = compile(vec![FeedInput {
        id: CompileFeedId::try_new(FEED.to_owned()).expect("id"),
        source: Box::new(source),
    }])
    .expect("compiles");
    TransitState::new(
        LoadedTimetable::from_bytes(compiled.bytes).expect("loads"),
        StateConfig::default(),
    )
    .expect("state")
}

const STOP_TIMES: &str = "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
T1,08:05:00,08:05:00,ORG,1\nT1,08:20:00,08:20:00,MID,2\n\
T2,08:25:00,08:25:00,MID,1\nT2,08:50:00,08:50:00,DST,2\n";

fn plain_state() -> TransitState {
    state_of(
        "route_id,service_id,trip_id,block_id,trip_headsign,wheelchair_accessible\n\
         R1,WK,T1,X,To Middle,\nR2,WK,T2,X,To Destination,\n",
        STOP_TIMES,
    )
}

fn request(to: LatLon, mode: TravelMode) -> TransitRequest {
    TransitRequest {
        mode,
        options: TransitOptions {
            max_trips: 2,
            ..TransitOptions::default()
        },
        now: FEED_TIME + 15,
        ..TransitRequest::new(origin(), to, june_15(7, 55), june_15(7, 54))
    }
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

#[allow(deprecated)]
fn trip_update(
    trip: &str,
    relationship: proto::trip_descriptor::ScheduleRelationship,
    stop_updates: Vec<proto::trip_update::StopTimeUpdate>,
    timestamp: Option<u64>,
) -> proto::FeedEntity {
    proto::FeedEntity {
        id: format!("tu-{trip}"),
        trip_update: Some(proto::TripUpdate {
            trip: proto::TripDescriptor {
                trip_id: Some(trip.to_owned()),
                start_date: Some(JUNE_15.to_owned()),
                schedule_relationship: Some(relationship as i32),
                ..proto::TripDescriptor::default()
            },
            timestamp,
            stop_time_update: stop_updates,
            ..proto::TripUpdate::default()
        }),
        ..proto::FeedEntity::default()
    }
}

fn delay_at(sequence: u32, delay: i32) -> proto::trip_update::StopTimeUpdate {
    proto::trip_update::StopTimeUpdate {
        stop_sequence: Some(sequence),
        arrival: Some(proto::trip_update::StopTimeEvent {
            delay: Some(delay),
            ..Default::default()
        }),
        departure: Some(proto::trip_update::StopTimeEvent {
            delay: Some(delay),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn time_at(sequence: u32, unix: i64) -> proto::trip_update::StopTimeUpdate {
    proto::trip_update::StopTimeUpdate {
        stop_sequence: Some(sequence),
        arrival: Some(proto::trip_update::StopTimeEvent {
            time: Some(unix),
            ..Default::default()
        }),
        departure: Some(proto::trip_update::StopTimeEvent {
            time: Some(unix),
            ..Default::default()
        }),
        ..Default::default()
    }
}

use proto::trip_descriptor::ScheduleRelationship as Trip;

// 1. Wheelchair: a block of explicitly inaccessible trips.

#[test]
fn merged_inaccessible_trips_stay_excluded_from_wheelchair() {
    let state = state_of(
        "route_id,service_id,trip_id,block_id,trip_headsign,wheelchair_accessible\n\
         R1,WK,T1,X,To Middle,2\nR2,WK,T2,X,To Destination,2\n",
        STOP_TIMES,
    );
    let wheelchair = run(&state, &request(destination(), TravelMode::Wheelchair));
    let walking = run(&state, &request(destination(), TravelMode::Walking));
    let leg = transit_legs(&walking.journeys[0])[0];
    let report = format!(
        "wheelchair journeys: {}, walking leg accessibility: {:?}",
        wheelchair.journeys.len(),
        leg.accessibility
    );
    assert!(
        wheelchair.journeys.is_empty(),
        "a wheelchair rider was routed onto trips marked wheelchair_accessible=2 ({report})"
    );
    assert_eq!(leg.accessibility, Accessibility::Inaccessible, "({report})");
}

// 2. NO_SERVICE trip-scoped on one part.

#[allow(deprecated)]
fn no_service_alert(selector: proto::EntitySelector) -> proto::FeedEntity {
    proto::FeedEntity {
        id: "a1".to_owned(),
        alert: Some(proto::Alert {
            active_period: vec![proto::TimeRange {
                start: Some(1_700_000_000),
                end: Some(1_900_000_000),
            }],
            informed_entity: vec![selector],
            effect: Some(proto::alert::Effect::NoService as i32),
            ..proto::Alert::default()
        }),
        ..proto::FeedEntity::default()
    }
}

#[test]
fn trip_scoped_no_service_on_second_part_keeps_first_part() {
    let state = plain_state();
    ingest(
        &state,
        FeedKind::Alerts,
        vec![no_service_alert(proto::EntitySelector {
            trip: Some(proto::TripDescriptor {
                trip_id: Some("T2".to_owned()),
                start_date: Some(JUNE_15.to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        })],
    );
    let to_mid = run(&state, &request(mid_ll(), TravelMode::Walking));
    assert!(
        to_mid.journeys.iter().any(|j| transit_legs(j)
            .iter()
            .any(|l| l.trip.trip_id == "T1" && l.stops.last().unwrap().stop.stop_id == "MID")),
        "NO_SERVICE on T2 removed T1's part too: {:?}",
        to_mid.journeys.len()
    );
}

// 3. assigned_stop_id on a stretch.

#[test]
fn assigned_stop_on_a_stretch_changes_where_it_stops() {
    let state = plain_state();
    let mut update = delay_at(2, 0);
    update.stop_time_properties = Some(proto::trip_update::stop_time_update::StopTimeProperties {
        assigned_stop_id: Some("ALT".to_owned()),
        ..Default::default()
    });
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![trip_update("T2", Trip::Scheduled, vec![update], None)],
    );
    let response = run(&state, &request(destination(), TravelMode::Walking));
    let leg = transit_legs(&response.journeys[0])[0];
    let last = leg.stops.last().unwrap();
    assert_eq!(
        last.stop.stop_id, "ALT",
        "assigned_stop_id for a stretch is ignored; ride still ends at {}",
        last.stop.stop_id
    );
}

// 4. REPLACEMENT of the second part.

#[test]
fn replacement_of_second_part_keeps_first_part() {
    let state = plain_state();
    let mid = proto::trip_update::StopTimeUpdate {
        stop_id: Some("MID".to_owned()),
        arrival: Some(proto::trip_update::StopTimeEvent {
            time: Some(june_15(8, 26)),
            ..Default::default()
        }),
        departure: Some(proto::trip_update::StopTimeEvent {
            time: Some(june_15(8, 26)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let dst = proto::trip_update::StopTimeUpdate {
        stop_id: Some("DST".to_owned()),
        arrival: Some(proto::trip_update::StopTimeEvent {
            time: Some(june_15(8, 51)),
            ..Default::default()
        }),
        departure: Some(proto::trip_update::StopTimeEvent {
            time: Some(june_15(8, 51)),
            ..Default::default()
        }),
        ..Default::default()
    };
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![trip_update("T2", Trip::Replacement, vec![mid, dst], None)],
    );
    let to_mid = run(&state, &request(mid_ll(), TravelMode::Walking));
    assert!(
        to_mid
            .journeys
            .iter()
            .any(|j| transit_legs(j).iter().any(|l| l.trip.trip_id == "T1")),
        "REPLACEMENT of T2 erased T1's stretch too; legs: {:?}",
        to_mid
            .journeys
            .iter()
            .flat_map(transit_legs)
            .map(|l| l.trip.trip_id.clone())
            .collect::<Vec<_>>()
    );
}

// 5. DUPLICATED of the second part.

#[test]
fn duplicated_second_part_only_copies_that_part() {
    let state = plain_state();
    let mut entity = trip_update("T2", Trip::Duplicated, vec![], None);
    let update = entity.trip_update.as_mut().unwrap();
    update.trip_properties = Some(proto::trip_update::TripProperties {
        trip_id: Some("T2-copy".to_owned()),
        start_date: Some(JUNE_15.to_owned()),
        start_time: Some("10:00:00".to_owned()),
        ..Default::default()
    });
    ingest(&state, FeedKind::TripUpdates, vec![entity]);
    let response = run(
        &state,
        &TransitRequest {
            departure: june_15(9, 30),
            ..request(destination(), TravelMode::Walking)
        },
    );
    for journey in &response.journeys {
        for leg in transit_legs(journey) {
            assert_ne!(
                leg.stops.first().unwrap().stop.stop_id,
                "ORG",
                "the copy of T2 (MID->DST) has T1's stops too: {:?}",
                leg.stops
                    .iter()
                    .map(|s| (s.stop.stop_id.clone(), s.departure))
                    .collect::<Vec<_>>()
            );
        }
    }
}

// 6. Order dependence: repaired times used as the schedule of the second part.

#[test]
fn second_parts_prediction_is_not_read_against_repaired_times() {
    // T1 is 10 minutes late, pushing T2's start at MID to 08:30 by repair. T2's own update then
    // says it really leaves MID at 08:30, which is +5 min against its 08:25 schedule, and
    // has no later stops. The delay must carry to DST: 08:50 + 5 = 08:55.
    let state = plain_state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![
            trip_update("T1", Trip::Scheduled, vec![delay_at(2, 600)], Some(100)),
            trip_update(
                "T2",
                Trip::Scheduled,
                vec![time_at(1, june_15(8, 30))],
                Some(200),
            ),
        ],
    );
    let response = run(&state, &request(destination(), TravelMode::Walking));
    let leg = transit_legs(&response.journeys[0])[0];
    assert_eq!(
        leg.arrival,
        june_15(8, 55),
        "T2's +5 was measured against the repaired time; arrival {}",
        (leg.arrival - june_15(0, 0)) / 60
    );
}

// 6b. The same updates in the other order give the other answer.

#[test]
fn two_updates_are_order_independent() {
    let build = |t1_ts, t2_ts| {
        let state = plain_state();
        ingest(
            &state,
            FeedKind::TripUpdates,
            vec![
                trip_update("T1", Trip::Scheduled, vec![delay_at(2, 600)], Some(t1_ts)),
                trip_update(
                    "T2",
                    Trip::Scheduled,
                    vec![time_at(1, june_15(8, 30))],
                    Some(t2_ts),
                ),
            ],
        );
        let response = run(&state, &request(destination(), TravelMode::Walking));
        transit_legs(&response.journeys[0])[0].arrival
    };
    assert_eq!(
        build(100, 200),
        build(200, 100),
        "result depends on update timestamps' order"
    );
}

// 7. A late first part compresses the second part to zero travel time.

#[test]
fn a_late_first_part_does_not_make_the_second_part_instant() {
    let state = plain_state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![trip_update(
            "T1",
            Trip::Scheduled,
            vec![delay_at(2, 3600)],
            None,
        )],
    );
    let response = run(&state, &request(destination(), TravelMode::Walking));
    let leg = transit_legs(&response.journeys[0])[0];
    let middle = &leg.stops[1];
    assert!(
        leg.arrival - middle.departure >= 25 * 60,
        "T2 scheduled 25 min, but after a late T1 it takes {} s",
        leg.arrival - middle.departure
    );
}

// 8. A ride on the first part only must not take the second part's update.

#[test]
fn a_ride_on_the_first_part_is_not_reported_canceled_for_the_second() {
    let state = plain_state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![trip_update("T2", Trip::Canceled, vec![], None)],
    );
    let to_mid = run(&state, &request(mid_ll(), TravelMode::Walking));
    let leg = to_mid
        .journeys
        .iter()
        .flat_map(transit_legs)
        .find(|leg| leg.trip.trip_id == "T1")
        .expect("T1 ride");
    assert_ne!(
        leg.trip.schedule_relationship.as_deref(),
        Some("canceled"),
        "T1's rider is told their leg is canceled"
    );
}

// 9. Leg accessibility of a ride within one accessible stretch.

#[test]
fn leg_accessibility_covers_the_boarded_stretch() {
    let state = state_of(
        "route_id,service_id,trip_id,block_id,trip_headsign,wheelchair_accessible\n\
         R1,WK,T1,X,To Middle,1\nR2,WK,T2,X,To Destination,\n",
        STOP_TIMES,
    );
    let to_mid = run(&state, &request(mid_ll(), TravelMode::Walking));
    let leg = to_mid
        .journeys
        .iter()
        .flat_map(transit_legs)
        .find(|leg| leg.trip.trip_id == "T1" && leg.segments.is_empty())
        .expect("ride within T1");
    assert_eq!(leg.trip.wheelchair_accessible, Some("yes"));
    assert_eq!(
        leg.accessibility,
        Accessibility::Accessible,
        "accessible trip, accessible stops, but the leg reports {:?}",
        leg.accessibility
    );
}

// 10. A stop removed by an alert is marked only on the newest update of a combined trip.

#[test]
fn a_removed_stop_on_the_first_part_is_marked_skipped() {
    let state = state_of(
        "route_id,service_id,trip_id,block_id,trip_headsign,wheelchair_accessible\n\
         R1,WK,T1,X,To Middle,\nR2,WK,T2,X,To Destination,\n",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         T1,08:05:00,08:05:00,ORG,1\nT1,08:12:00,08:12:00,ALT,2\nT1,08:20:00,08:20:00,MID,3\n\
         T2,08:25:00,08:25:00,MID,1\nT2,08:50:00,08:50:00,DST,2\n",
    );
    let (t1, t2) = (
        trip_update("T1", Trip::Scheduled, vec![delay_at(1, 0)], Some(100)),
        trip_update("T2", Trip::Scheduled, vec![delay_at(1, 60)], Some(200)),
    );
    ingest(&state, FeedKind::TripUpdates, vec![t1, t2]);
    ingest(
        &state,
        FeedKind::Alerts,
        vec![no_service_alert(proto::EntitySelector {
            stop_id: Some("ALT".to_owned()),
            ..Default::default()
        })],
    );
    let response = run(&state, &request(destination(), TravelMode::Walking));
    let leg = transit_legs(&response.journeys[0])[0];
    let visit = leg
        .stops
        .iter()
        .find(|v| v.stop.stop_id == "ALT")
        .expect("ALT on the ride");
    assert_eq!(
        visit.schedule_relationship.as_deref(),
        Some("skipped"),
        "ALT was removed from routing by the alert but the leg doesn't say so: {:?}",
        visit.schedule_relationship
    );
}

// 11. A brand-new trip takes the wheelchair flag of the route's first stop pattern.

#[test]
fn a_new_trip_on_a_route_with_an_inaccessible_pattern_is_unknown_not_excluded() {
    let state = state_of(
        "route_id,service_id,trip_id,block_id,trip_headsign,wheelchair_accessible\n\
         R1,WK,T1,,Inaccessible,2\nR1,WK,T2,,Accessible,\n",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         T1,08:05:00,08:05:00,ORG,1\nT1,08:20:00,08:20:00,MID,2\n\
         T2,08:25:00,08:25:00,MID,1\nT2,08:50:00,08:50:00,DST,2\n",
    );
    let mut new_trip = trip_update(
        "ADDED1",
        Trip::New,
        vec![
            proto::trip_update::StopTimeUpdate {
                stop_id: Some("ORG".to_owned()),
                arrival: Some(proto::trip_update::StopTimeEvent {
                    time: Some(june_15(8, 30)),
                    ..Default::default()
                }),
                departure: Some(proto::trip_update::StopTimeEvent {
                    time: Some(june_15(8, 30)),
                    ..Default::default()
                }),
                ..Default::default()
            },
            proto::trip_update::StopTimeUpdate {
                stop_id: Some("DST".to_owned()),
                arrival: Some(proto::trip_update::StopTimeEvent {
                    time: Some(june_15(8, 55)),
                    ..Default::default()
                }),
                departure: Some(proto::trip_update::StopTimeEvent {
                    time: Some(june_15(8, 55)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ],
        None,
    );
    new_trip.trip_update.as_mut().unwrap().trip.route_id = Some("R1".to_owned());
    ingest(&state, FeedKind::TripUpdates, vec![new_trip]);
    let control = run(&state, &request(destination(), TravelMode::Walking));
    assert!(
        control
            .journeys
            .iter()
            .flat_map(transit_legs)
            .any(|l| l.trip.trip_id == "ADDED1"),
        "control: the walking search must see the NEW trip"
    );
    let response = run(&state, &request(destination(), TravelMode::Wheelchair));
    assert!(
        response
            .journeys
            .iter()
            .flat_map(transit_legs)
            .any(|l| l.trip.trip_id == "ADDED1"),
        "a NEW trip says nothing about wheelchairs, yet it's excluded: journeys {:?}",
        response
            .journeys
            .iter()
            .flat_map(transit_legs)
            .map(|l| l.trip.trip_id.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn control_with_only_the_first_parts_update_the_stop_is_marked() {
    let state = state_of(
        "route_id,service_id,trip_id,block_id,trip_headsign,wheelchair_accessible\n\
         R1,WK,T1,X,To Middle,\nR2,WK,T2,X,To Destination,\n",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         T1,08:05:00,08:05:00,ORG,1\nT1,08:12:00,08:12:00,ALT,2\nT1,08:20:00,08:20:00,MID,3\n\
         T2,08:25:00,08:25:00,MID,1\nT2,08:50:00,08:50:00,DST,2\n",
    );
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![trip_update(
            "T1",
            Trip::Scheduled,
            vec![delay_at(1, 60)],
            Some(100),
        )],
    );
    ingest(
        &state,
        FeedKind::Alerts,
        vec![no_service_alert(proto::EntitySelector {
            stop_id: Some("ALT".to_owned()),
            ..Default::default()
        })],
    );
    let response = run(&state, &request(destination(), TravelMode::Walking));
    let leg = transit_legs(&response.journeys[0])[0];
    let visit = leg
        .stops
        .iter()
        .find(|v| v.stop.stop_id == "ALT")
        .expect("ALT on the ride");
    assert_eq!(visit.schedule_relationship.as_deref(), Some("skipped"));
}

// Alert selectors reaching a later original trip of a combined run.

fn directed_state() -> TransitState {
    state_of(
        "route_id,service_id,trip_id,block_id,trip_headsign,wheelchair_accessible,direction_id\n\
         R1,WK,T1,X,To Middle,,0\nR2,WK,T2,X,To Destination,,1\n",
        STOP_TIMES,
    )
}

fn assert_second_part_gone_first_kept(state: &TransitState) {
    let to_destination = run(state, &request(destination(), TravelMode::Walking));
    assert!(
        !to_destination
            .journeys
            .iter()
            .flat_map(transit_legs)
            .any(|l| l.stops.last().is_some_and(|v| v.stop.stop_id == "DST")),
        "the alert should have taken T2's stretch out"
    );
    let to_mid = run(state, &request(mid_ll(), TravelMode::Walking));
    assert!(
        to_mid
            .journeys
            .iter()
            .flat_map(transit_legs)
            .any(|l| l.trip.trip_id == "T1"),
        "T1's stretch should still run"
    );
}

#[test]
fn a_route_direction_and_start_selector_can_name_a_second_part() {
    let state = directed_state();
    ingest(
        &state,
        FeedKind::Alerts,
        vec![no_service_alert(proto::EntitySelector {
            trip: Some(proto::TripDescriptor {
                route_id: Some("R2".to_owned()),
                direction_id: Some(1),
                start_time: Some("08:25:00".to_owned()),
                start_date: Some(JUNE_15.to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        })],
    );
    assert_second_part_gone_first_kept(&state);
}

#[test]
fn a_route_selector_reaches_a_second_part_of_another_route() {
    let state = plain_state();
    ingest(
        &state,
        FeedKind::Alerts,
        vec![no_service_alert(proto::EntitySelector {
            route_id: Some("R2".to_owned()),
            ..Default::default()
        })],
    );
    assert_second_part_gone_first_kept(&state);
}

#[test]
fn a_route_type_selector_reaches_a_second_part_of_another_type() {
    let state = state_with_routes(
        "route_id,agency_id,route_short_name,route_type\nR1,A,1,3\nR2,A,2,0\n",
        "route_id,service_id,trip_id,block_id,trip_headsign,wheelchair_accessible\n\
         R1,WK,T1,X,To Middle,\nR2,WK,T2,X,To Destination,\n",
        STOP_TIMES,
    );
    ingest(
        &state,
        FeedKind::Alerts,
        vec![no_service_alert(proto::EntitySelector {
            route_type: Some(0),
            ..Default::default()
        })],
    );
    assert_second_part_gone_first_kept(&state);
}

// A replacement of one part: its leg's fields come from the replacement, not from positions.

#[test]
fn a_replaced_part_rides_as_its_own_trip_with_its_own_fields() {
    let state = plain_state();
    let replacement_stop = |stop: &str, minute: i64| proto::trip_update::StopTimeUpdate {
        stop_id: Some(stop.to_owned()),
        arrival: Some(proto::trip_update::StopTimeEvent {
            time: Some(june_15(8, minute)),
            ..Default::default()
        }),
        departure: Some(proto::trip_update::StopTimeEvent {
            time: Some(june_15(8, minute)),
            ..Default::default()
        }),
        ..Default::default()
    };
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![trip_update(
            "T2",
            Trip::Replacement,
            vec![
                replacement_stop("MID", 30),
                replacement_stop("ALT", 45),
                replacement_stop("DST", 55),
            ],
            None,
        )],
    );
    let response = run(&state, &request(destination(), TravelMode::Walking));
    let leg = response
        .journeys
        .iter()
        .flat_map(transit_legs)
        .find(|leg| leg.trip.trip_id == "T2")
        .expect("a ride on the replacement");
    assert_eq!(leg.stops.first().unwrap().stop.stop_id, "MID");
    assert_eq!(leg.stops[1].stop.stop_id, "ALT");
    assert!(leg.segments.is_empty(), "a replacement is one plain ride");
    assert_eq!(leg.trip.headsign.as_deref(), Some("To Destination"));
    assert_eq!(leg.route.as_ref().unwrap().route_id, "R2");
    // Nobody rides on through: the seat is gone with the replacement.
    assert!(
        !response.journeys.iter().flat_map(transit_legs).any(|l| l
            .stops
            .first()
            .unwrap()
            .stop
            .stop_id
            == "ORG"
            && l.stops.last().unwrap().stop.stop_id == "DST")
    );
}

// How delays compose across parts.

#[test]
fn a_carried_delay_stops_at_a_later_update_and_early_arrivals_wait() {
    // T2's update at its last stop wins there, and the carried delay holds before it.
    let state = plain_state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![
            trip_update("T1", Trip::Scheduled, vec![delay_at(2, 600)], None),
            trip_update("T2", Trip::Scheduled, vec![delay_at(2, 0)], None),
        ],
    );
    let leg = run(&state, &request(destination(), TravelMode::Walking));
    let leg = transit_legs(&leg.journeys[0])[0];
    assert_eq!(
        leg.stops[1].departure,
        june_15(8, 35),
        "carried to T2's start"
    );
    assert_eq!(
        leg.arrival,
        june_15(8, 50),
        "T2's own update at its last stop wins"
    );

    // An early arrival doesn't let the vehicle leave the join early.
    let state = plain_state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![trip_update(
            "T1",
            Trip::Scheduled,
            vec![delay_at(2, -300)],
            None,
        )],
    );
    let leg = run(&state, &request(destination(), TravelMode::Walking));
    let leg = transit_legs(&leg.journeys[0])[0];
    assert_eq!(leg.stops[1].departure, june_15(8, 25));
    assert_eq!(leg.arrival, june_15(8, 50));
}

#[test]
fn a_delay_carries_through_several_parts_with_no_updates() {
    let state = state_with(
        "route_id,agency_id,route_short_name,route_type\nR1,A,1,3\nR2,A,2,3\n",
        &format!("{STOPS}FAR,Far,47.7000,-122.2500,1\n"),
        "route_id,service_id,trip_id,block_id,trip_headsign,wheelchair_accessible\n\
         R1,WK,T1,X,One,\nR2,WK,T2,X,Two,\nR1,WK,T3,X,Three,\n",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
         T1,08:05:00,08:05:00,ORG,1\nT1,08:20:00,08:20:00,MID,2\n\
         T2,08:25:00,08:25:00,MID,1\nT2,08:50:00,08:50:00,DST,2\n\
         T3,08:55:00,08:55:00,DST,1\nT3,09:10:00,09:10:00,FAR,2\n",
    );
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![trip_update(
            "T1",
            Trip::Scheduled,
            vec![delay_at(2, 300)],
            None,
        )],
    );
    let far = LatLon {
        lat: 47.7000,
        lon: -122.2500,
    };
    let response = run(
        &state,
        &TransitRequest {
            options: TransitOptions {
                max_trips: 1,
                ..TransitOptions::default()
            },
            ..request(far, TravelMode::Walking)
        },
    );
    let leg = transit_legs(&response.journeys[0])[0];
    assert_eq!(leg.arrival, june_15(9, 15), "through T2 and into T3");
    assert_eq!(leg.trip.realtime_state, Some("delayed"));
}

#[test]
fn a_second_parts_displayed_delay_is_against_its_own_schedule() {
    let state = plain_state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![
            trip_update("T1", Trip::Scheduled, vec![delay_at(2, 600)], Some(100)),
            trip_update(
                "T2",
                Trip::Scheduled,
                vec![time_at(1, june_15(8, 30))],
                Some(200),
            ),
        ],
    );
    let response = run(&state, &request(destination(), TravelMode::Walking));
    let leg = transit_legs(&response.journeys[0])[0];
    let middle = &leg.stops[1];
    assert_eq!(
        middle.departure_delay_seconds,
        Some(300),
        "08:30 against 08:25"
    );
    assert_eq!(middle.scheduled_departure, Some(june_15(8, 25)));
}

#[test]
fn a_rides_state_covers_only_the_stretches_it_rides() {
    let state = plain_state();
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![trip_update(
            "T2",
            Trip::Scheduled,
            vec![delay_at(1, 120)],
            None,
        )],
    );
    let to_mid = run(&state, &request(mid_ll(), TravelMode::Walking));
    let t1 = to_mid
        .journeys
        .iter()
        .flat_map(transit_legs)
        .find(|leg| leg.trip.trip_id == "T1")
        .expect("T1 ride");
    assert_eq!(t1.trip.realtime_state, None, "T1 is on time");
    let to_destination = run(&state, &request(destination(), TravelMode::Walking));
    let spanning = transit_legs(&to_destination.journeys[0])[0];
    assert_eq!(spanning.trip.realtime_state, Some("delayed"));
}

// A combined run moved to its own pattern still answers alerts by stretch.

#[test]
fn an_alert_on_a_part_still_applies_after_the_run_moves_to_a_new_pattern() {
    let state = plain_state();
    let mut update = delay_at(2, 0);
    update.stop_time_properties = Some(proto::trip_update::stop_time_update::StopTimeProperties {
        assigned_stop_id: Some("ALT".to_owned()),
        ..Default::default()
    });
    ingest(
        &state,
        FeedKind::TripUpdates,
        vec![trip_update("T2", Trip::Scheduled, vec![update], None)],
    );
    ingest(
        &state,
        FeedKind::Alerts,
        vec![no_service_alert(proto::EntitySelector {
            trip: Some(proto::TripDescriptor {
                trip_id: Some("T1".to_owned()),
                start_date: Some(JUNE_15.to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        })],
    );
    let response = run(&state, &request(mid_ll(), TravelMode::Walking));
    assert!(
        !response
            .journeys
            .iter()
            .flat_map(transit_legs)
            .any(|leg| leg.stops.first().unwrap().stop.stop_id == "ORG"),
        "NO_SERVICE on T1 must remove it from the moved run too"
    );
}
