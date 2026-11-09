//! Scenario tests: each pins down one rule on a hand-made network.

use crate::ids::{Direction, TripId};
use crate::overlay::OverlayBuilder;
use crate::stop_table::RouteFilter;
use crate::testing::{
    Scenario, ScenarioTrip, TestNet, block_of, check_journey, day_slot, oracle, t,
};
use crate::timetable::{
    ResolvedBlock, ResolvedLane, ResolvedTrip, RouteAttributes, StopEvent, StopRules,
};
use crate::{Journey, Leg, MAX_TRIPS, QueryError, Router, SearchError, StopTable};

fn arrivals(net: &TestNet, departure: i32) -> Vec<(u8, i32)> {
    let query = net.query(4, RouteFilter::ALL);
    let mut router = net.router();
    let run = router
        .forward(&query, t(departure), &|| false)
        .expect("search");
    let results: Vec<_> = run
        .results()
        .iter()
        .map(|r| (r.trips, r.time.into_inner()))
        .collect();
    let journeys: Vec<_> = results
        .iter()
        .map(|&(trips, _)| run.journey(trips).expect("journey"))
        .collect();
    for journey in &journeys {
        check_journey(net, journey);
    }
    results
}

fn departures(net: &TestNet, arrival: i32) -> Vec<(u8, i32)> {
    let query = net.query(4, RouteFilter::ALL);
    let mut router = net.router();
    let run = router
        .backward(&query, t(arrival), &|| false)
        .expect("search");
    run.results()
        .iter()
        .map(|r| (r.trips, r.time.into_inner()))
        .collect()
}

fn agrees_with_oracle(net: &TestNet, departure: i32) {
    assert_eq!(
        arrivals(net, departure),
        oracle(net, RouteFilter::ALL, Direction::Forward, departure, 4),
        "forward search at {departure}"
    );
}

#[test]
fn single_ride() {
    let (net, _) = Scenario::new(2)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[100, 200])])
        .access(0, 10)
        .egress(1, 5)
        .build();
    assert_eq!(arrivals(&net, 0), vec![(1, 205)]);
    assert_eq!(departures(&net, 205), vec![(1, 90)]);
    assert_eq!(departures(&net, 204), vec![]);
}

#[test]
fn a_journey_needs_a_trip() {
    // The origin can walk straight to a stop with an egress entry; that is not a journey.
    let (net, _) = Scenario::new(2)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[100, 200])])
        .access(1, 10)
        .egress(1, 5)
        .build();
    assert_eq!(arrivals(&net, 0), vec![]);
}

#[test]
fn earliest_arrival_trip_is_found_with_multiple_trips() {
    let (net, _) = Scenario::new(3)
        .route(
            &[0, 1],
            &[
                ScenarioTrip::simple(1, &[100, 200]),
                ScenarioTrip::simple(2, &[150, 250]),
            ],
        )
        .route(&[1, 2], &[ScenarioTrip::simple(3, &[220, 300])])
        .access(0, 0)
        .egress(2, 0)
        .build();
    assert_eq!(arrivals(&net, 0), vec![(2, 300)]);
    // The second bus reaches stop 1 too late for the connection.
    assert_eq!(arrivals(&net, 120), vec![]);
    agrees_with_oracle(&net, 0);
    agrees_with_oracle(&net, 120);
}

#[test]
fn an_earlier_trip_further_along_replaces_the_current_one() {
    // Boarding the late trip at stop 0, the rider could switch to an earlier
    // trip at stop 1, where a walk arrived in time.
    let (net, _) = Scenario::new(4)
        .route(
            &[0, 1, 2],
            &[
                ScenarioTrip::simple(1, &[50, 150, 250]),
                ScenarioTrip::simple(2, &[100, 200, 300]),
            ],
        )
        .walk(3, 1, 10)
        .route(&[3, 1], &[ScenarioTrip::simple(3, &[60, 90])])
        .access(0, 80)
        .access(3, 0)
        .egress(2, 0)
        .build();
    agrees_with_oracle(&net, 0);
    assert_eq!(arrivals(&net, 0), vec![(1, 300), (2, 250)]);
}

#[test]
fn loops_visit_a_stop_twice() {
    // The route starts and ends at stop 0.
    // A rider starting at stop 1 can only reach stop 0 as the loop's last stop.
    let (net, _) = Scenario::new(3)
        .route(&[0, 1, 2, 0], &[ScenarioTrip::simple(1, &[10, 20, 30, 40])])
        .access(1, 0)
        .egress(0, 0)
        .build();
    assert_eq!(arrivals(&net, 0), vec![(1, 40)]);
    assert_eq!(departures(&net, 40), vec![(1, 20)]);
    agrees_with_oracle(&net, 0);
}

#[test]
fn a_loop_can_board_at_the_second_visit() {
    // The first visit to stop 0 is before the rider gets there, so only the second can board.
    let (net, _) = Scenario::new(3)
        .route(&[0, 1, 0, 2], &[ScenarioTrip::simple(1, &[10, 20, 30, 40])])
        .access(0, 25)
        .egress(2, 0)
        .build();
    assert_eq!(arrivals(&net, 0), vec![(1, 40)]);
}

#[test]
fn trips_past_midnight_run_on_their_service_day() {
    // A trip of yesterday (slot 0) still running after midnight, seen from today's clock.
    let mut late = ScenarioTrip::simple(1, &[90_000, 90_600]);
    late.day_slots = vec![0];
    let mut today = ScenarioTrip::simple(2, &[3_700, 3_800]);
    today.day_slots = vec![1];
    let (net, ids) = Scenario::new(2)
        .days(&[(1, -86_400), (2, 0)])
        .route(&[0, 1], &[late, today])
        .access(0, 0)
        .egress(1, 0)
        .build();
    // 90_000 seconds into yesterday is 3_600 after today's midnight.
    assert_eq!(arrivals(&net, 0), vec![(1, 3_800)]);
    let query = net.query(1, RouteFilter::ALL);
    let mut router = net.router();
    let journey = {
        let run = router.forward(&query, t(0), &|| false).expect("search");
        run.journey(1).expect("journey")
    };
    let Some(Leg::Ride(ride)) = journey.legs.get(1) else {
        panic!("expected a ride: {journey:?}");
    };
    // The rider takes today's service-day run, even though the block rows are shared by time.
    assert_eq!(ride.board_stop, ids[0]);
    assert_eq!(ride.trip.trip, TripId::new(2));
    assert_eq!(arrivals(&net, 3_601), vec![(1, 3_800)]);
}

#[test]
fn minimum_change_time_applies_at_the_same_stop() {
    let net = |min_change: u32| {
        Scenario::new(3)
            .min_change(1, min_change)
            .route(&[0, 1], &[ScenarioTrip::simple(1, &[10, 20])])
            .route(&[1, 2], &[ScenarioTrip::simple(2, &[25, 40])])
            .access(0, 0)
            .egress(2, 0)
            .build()
            .0
    };
    // Leaves 5 seconds to change.
    assert_eq!(arrivals(&net(5), 0), vec![(2, 40)]);
    assert_eq!(arrivals(&net(6), 0), vec![]);
    assert_eq!(departures(&net(5), 40), vec![(2, 10)]);
    assert_eq!(departures(&net(6), 40), vec![]);
}

#[test]
fn a_zero_length_walk_cannot_bypass_the_change_time() {
    // Stops 1 and 3 are one node; walking between them costs nothing.
    // Boarding at 3 (a different stop) is free of the change time.
    // Boarding back at stop 1 itself must still wait for it.
    let scenario = |bus_stop: u32, bus_departs: i32| {
        Scenario::new(4)
            .min_change(1, 300)
            .route(&[0, 1], &[ScenarioTrip::simple(1, &[0, 100])])
            .route(
                &[bus_stop, 2],
                &[ScenarioTrip::simple(2, &[bus_departs, 500])],
            )
            .walk(1, 3, 0)
            .walk(3, 1, 0)
            .access(0, 0)
            .egress(2, 0)
            .build()
            .0
    };
    // Walking to the other stop for nothing is a real walk.
    assert_eq!(arrivals(&scenario(3, 100), 0), vec![(2, 500)]);
    assert_eq!(departures(&scenario(3, 100), 500), vec![(2, 0)]);
    // At the same stop, 100 seconds to change isn't enough, whatever the walking graph allows.
    assert_eq!(arrivals(&scenario(1, 100), 0), vec![]);
    assert_eq!(departures(&scenario(1, 100), 500), vec![]);
    assert_eq!(arrivals(&scenario(1, 400), 0), vec![(2, 500)]);
}

#[test]
fn walking_from_another_stop_helps_even_after_a_trip_arrival() {
    // A rider arrives at stop 1 by trip at 100 and at stop 2 by trip at 90,
    // and the walk from 2 to 1 takes 15 seconds.
    // Boarding at stop 1 at 105 is possible only on foot; the same-stop rule would wait until 400.
    let (net, _) = Scenario::new(5)
        .min_change(1, 300)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[0, 100])])
        .route(&[0, 2], &[ScenarioTrip::simple(2, &[0, 90])])
        .route(&[1, 3], &[ScenarioTrip::simple(3, &[105, 200])])
        .walk(2, 1, 15)
        .access(0, 0)
        .egress(3, 0)
        .build();
    assert_eq!(arrivals(&net, 0), vec![(2, 200)]);
    agrees_with_oracle(&net, 0);
}

#[test]
fn pickup_and_drop_off_rules_are_honored() {
    let trip_with = |rules: Vec<StopRules>| {
        let mut trip = ScenarioTrip::simple(1, &[10, 20, 30]);
        trip.rules = rules;
        trip
    };
    let no_pickup = StopRules {
        pickup: false,
        drop_off: true,
    };
    let no_drop = StopRules {
        pickup: true,
        drop_off: false,
    };
    let reach = |rules: Vec<StopRules>, from: u32, to: u32| {
        let (net, _) = Scenario::new(3)
            .route(&[0, 1, 2], &[trip_with(rules)])
            .access(from, 0)
            .egress(to, 0)
            .build();
        (arrivals(&net, 0), departures(&net, 100))
    };
    assert_eq!(
        reach(vec![StopRules::REGULAR; 3], 0, 2),
        (vec![(1, 30)], vec![(1, 10)])
    );
    assert_eq!(
        reach(
            vec![no_pickup, StopRules::REGULAR, StopRules::REGULAR],
            0,
            2
        ),
        (vec![], vec![])
    );
    assert_eq!(
        reach(vec![StopRules::REGULAR, StopRules::REGULAR, no_drop], 0, 2),
        (vec![], vec![])
    );
    // Boarding at the middle stop works when only the first stop forbids it.
    assert_eq!(
        reach(
            vec![no_pickup, StopRules::REGULAR, StopRules::REGULAR],
            1,
            2
        ),
        (vec![(1, 30)], vec![(1, 20)])
    );
}

#[test]
fn rules_differing_across_trips_keep_their_own_lanes() {
    let no_pickup = StopRules {
        pickup: false,
        drop_off: true,
    };
    let mut no_board = ScenarioTrip::simple(1, &[10, 20]);
    no_board.rules = vec![no_pickup, StopRules::REGULAR];
    let (net, _) = Scenario::new(2)
        .route(&[0, 1], &[no_board, ScenarioTrip::simple(2, &[50, 60])])
        .access(0, 0)
        .egress(1, 0)
        .build();
    // The earlier trip has no pickup, so the rider waits for the second.
    assert_eq!(arrivals(&net, 0), vec![(1, 60)]);
    assert_eq!(departures(&net, 70), vec![(1, 50)]);
}

#[test]
fn bikes_allowed_filter_selects_routes() {
    let (net, _) = Scenario::new(2)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[10, 20])])
        .route_with(
            &[0, 1],
            RouteAttributes::BIKES_ALLOWED,
            &[ScenarioTrip::simple(2, &[30, 40])],
        )
        .access(0, 0)
        .egress(1, 0)
        .build();
    assert_eq!(arrivals(&net, 0), vec![(1, 20)]);
    let query = net.query(4, RouteFilter::requiring(RouteAttributes::BIKES_ALLOWED));
    let mut router = net.router();
    let run = router.forward(&query, t(0), &|| false).expect("search");
    assert_eq!(
        run.results()
            .iter()
            .map(|r| r.time.into_inner())
            .collect::<Vec<_>>(),
        vec![40]
    );
}

#[test]
fn a_route_cycle_terminates() {
    // Two routes feed each other in a cycle; the search must stop when nothing improves.
    let (net, _) = Scenario::new(2)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[10, 20])])
        .route(&[1, 0], &[ScenarioTrip::simple(2, &[30, 40])])
        .access(0, 0)
        .egress(1, 0)
        .build();
    let query = net.query(MAX_TRIPS, RouteFilter::ALL);
    let mut router = net.router();
    let run = router.forward(&query, t(0), &|| false).expect("search");
    assert_eq!(run.results().len(), 1);
}

#[test]
fn an_interrupt_stops_the_search() {
    let (net, _) = Scenario::new(2)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[10, 20])])
        .access(0, 0)
        .egress(1, 0)
        .build();
    let query = net.query(4, RouteFilter::ALL);
    let mut router = net.router();
    let result = router.forward(&query, t(0), &|| true);
    assert!(matches!(result, Err(SearchError::Interrupted)));
}

#[test]
fn invalid_queries_are_rejected() {
    let (net, _) = Scenario::new(2)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[10, 20])])
        .access(0, 0)
        .egress(1, 0)
        .build();
    let mut router = net.router();
    let query = net.query(0, RouteFilter::ALL);
    assert!(matches!(
        router.forward(&query, t(0), &|| false),
        Err(SearchError::Query(QueryError::MaxTrips(0)))
    ));
    let query = net.query(MAX_TRIPS + 1, RouteFilter::ALL);
    assert!(matches!(
        router.forward(&query, t(0), &|| false),
        Err(SearchError::Query(QueryError::MaxTrips(_)))
    ));
    let short_table = StopTable::new(1);
    let mut query = net.query(1, RouteFilter::ALL);
    query.access = &short_table;
    assert!(matches!(
        router.forward(&query, t(0), &|| false),
        Err(SearchError::Query(QueryError::StopTableSize {
            expected: 2,
            got: 1
        }))
    ));
    let query = net.query(1, RouteFilter::ALL);
    assert!(matches!(
        router.profile(&query, t(10), t(0), &|| false),
        Err(SearchError::Query(QueryError::EmptyWindow))
    ));
}

#[test]
fn profile_has_one_entry_per_useful_departure() {
    let (net, _) = Scenario::new(2)
        .route(
            &[0, 1],
            &[
                ScenarioTrip::simple(1, &[100, 200]),
                ScenarioTrip::simple(2, &[300, 400]),
                ScenarioTrip::simple(3, &[500, 600]),
            ],
        )
        .access(0, 0)
        .egress(1, 0)
        .build();
    let query = net.query(2, RouteFilter::ALL);
    let mut router = net.router();
    let profile = router
        .profile(&query, t(0), t(350), &|| false)
        .expect("profile");
    let triples: Vec<_> = profile
        .entries
        .iter()
        .map(|e| (e.departure.into_inner(), e.arrival.into_inner(), e.trips))
        .collect();
    // The trip at 500 departs after the window and is left out.
    assert_eq!(triples, vec![(100, 200, 1), (300, 400, 1)]);
}

#[test]
fn a_two_day_window_includes_tomorrows_trips() {
    let mut tomorrow = ScenarioTrip::simple(1, &[100, 200]);
    tomorrow.day_slots = vec![1];
    let (net, _) = Scenario::new(2)
        .days(&[(1, 0), (2, 86_400)])
        .route(&[0, 1], &[tomorrow])
        .access(0, 0)
        .egress(1, 0)
        .build();
    assert_eq!(arrivals(&net, 0), vec![(1, 86_600)]);
}

#[test]
fn overlay_changes_are_seen_by_a_search() {
    let (mut net, _) = Scenario::new(2)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[100, 200])])
        .access(0, 0)
        .egress(1, 0)
        .build();
    assert_eq!(arrivals(&net, 0), vec![(1, 200)]);
    let event = |time| StopEvent {
        arrival: t(time),
        departure: t(time),
        rules: StopRules::REGULAR,
    };
    let delayed = ResolvedTrip {
        trip: TripId::new(1),
        events: vec![event(150), event(250)],
    };
    let mut overlay = OverlayBuilder::new(net.snapshot.base().clone());
    overlay
        .replace_block(
            crate::RouteId::new(0),
            day_slot(0),
            &block_of(vec![delayed]),
        )
        .expect("replace");
    net.snapshot = overlay.build().expect("build");
    net.routes[0].days[0][0].events = vec![event(150), event(250)];
    assert_eq!(arrivals(&net, 0), vec![(1, 250)]);
}

#[test]
fn a_lane_that_lost_all_its_trips_does_not_reach_the_scan() {
    let (mut net, _) = Scenario::new(2)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[100, 200])])
        .access(0, 0)
        .egress(1, 0)
        .build();
    let event = |time| StopEvent {
        arrival: t(time),
        departure: t(time),
        rules: StopRules::REGULAR,
    };
    let kept = ResolvedLane::new(vec![ResolvedTrip {
        trip: TripId::new(1),
        events: vec![event(100), event(200)],
    }])
    .expect("valid");
    let emptied = ResolvedLane::new(Vec::new()).expect("cancellation can empty a lane");
    let mut overlay = OverlayBuilder::new(net.snapshot.base().clone());
    overlay
        .replace_block(
            crate::RouteId::new(0),
            day_slot(0),
            &ResolvedBlock::from_lanes(vec![kept, emptied]),
        )
        .expect("replace");
    net.snapshot = overlay.build().expect("build");
    assert_eq!(arrivals(&net, 0), vec![(1, 200)]);
}

#[test]
fn removed_service_cannot_be_ridden() {
    let (mut net, _) = Scenario::new(2)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[100, 200])])
        .access(0, 0)
        .egress(1, 0)
        .build();
    let mut overlay = OverlayBuilder::new(net.snapshot.base().clone());
    overlay
        .remove_service(crate::RouteId::new(0), day_slot(0))
        .expect("remove");
    net.snapshot = overlay.build().expect("build");
    net.routes[0].days[0].clear();
    assert_eq!(arrivals(&net, 0), vec![]);
}

#[test]
fn journey_is_recorded_leg_by_leg() {
    let (net, ids) = Scenario::new(4)
        .min_change(2, 10)
        .route(&[0, 1], &[ScenarioTrip::simple(1, &[100, 200])])
        .route(&[2, 3], &[ScenarioTrip::simple(2, &[300, 400])])
        .walk(1, 2, 50)
        .access(0, 5)
        .egress(3, 7)
        .build();
    let query = net.query(4, RouteFilter::ALL);
    let mut router: Router<_> = net.router();
    let run = router.forward(&query, t(0), &|| false).expect("search");
    let journey = run.journey(2).expect("journey");
    assert_eq!(journey.departure, t(95));
    assert_eq!(journey.arrival, t(407));
    check_journey(&net, &journey);
    let backward = {
        let run = router.backward(&query, t(407), &|| false).expect("search");
        run.journey(2).expect("journey")
    };
    check_journey(&net, &backward);
    let compact = |j: &Journey<_>| {
        j.legs
            .iter()
            .map(|leg| match leg {
                Leg::Access { stop, duration } => {
                    format!("access {} {}", stop.into_inner(), duration.into_inner())
                }
                Leg::Ride(r) => format!(
                    "ride route {} trip {} from {} at {} to {} at {}",
                    r.route.into_inner(),
                    r.trip.trip.into_inner(),
                    r.board_stop.into_inner(),
                    r.board_time.into_inner(),
                    r.alight_stop.into_inner(),
                    r.alight_time.into_inner(),
                ),
                Leg::Transfer {
                    from, to, duration, ..
                } => format!(
                    "walk {} to {} ({}s)",
                    from.into_inner(),
                    to.into_inner(),
                    duration.into_inner()
                ),
                Leg::Egress { stop, duration } => {
                    format!("egress {} {}", stop.into_inner(), duration.into_inner())
                }
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(compact(&journey), compact(&backward));
    insta::assert_yaml_snapshot!(compact(&journey), @r"
    - access 0 5
    - ride route 0 trip 1 from 0 at 100 to 1 at 200
    - walk 1 to 2 (50s)
    - ride route 1 trip 2 from 2 at 300 to 3 at 400
    - egress 3 7
    ");
    let _ = ids;
}
