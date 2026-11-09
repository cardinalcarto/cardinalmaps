//! Property tests on randomized networks. See the module's assertions for what they settle.

#![allow(clippy::too_many_lines)]

use std::sync::Arc;

use proptest::prelude::*;

use crate::ids::{Direction, Seconds, ServiceDate, ServiceDay, StopId};
use crate::overlay::OverlayBuilder;
use crate::raptor::TransitQuery;
use crate::snapshot::Snapshot;
use crate::stop_table::{RouteFilter, StopTable};
use crate::testing::{
    Graph, Results, SyntheticSearch, TestNet, TestRoute, Triples, block_of, brute_force_profile,
    check_journey, day_slot, oracle, oracle_every_second, routes_from_timetable, secs, t,
};
use crate::timetable::{
    ResolvedBlock, ResolvedLane, ResolvedTrip, RouteAttributes, StopEvent, StopRules, StopTime,
    TimetableBuilder,
};
use crate::{RouteId, Router, TripId};

const WINDOW: (i32, i32) = (0, 60);

/// The largest and smallest clock shifts keep every generated time within a few hundred seconds
/// of [`crate::TIME_LIMIT`], close enough to exercise it but not to cross it.
const SHIFT_HIGH: i32 = crate::TIME_LIMIT - 700;
const SHIFT_LOW: i32 = 500 - crate::TIME_LIMIT;

/// The offsets a day may have: past and future days, around a window at zero.
const DAY_OFFSETS: [i32; 5] = [-60, -30, 0, 20, 70];

#[derive(Debug, Clone)]
struct RawTrip {
    start: u8,
    /// `(hop, dwell)`, cycled along the route.
    hops: Vec<(u8, u8)>,
    /// `(pickup, drop_off)`, cycled along the route.
    rules: Vec<(bool, bool)>,
    /// A bitmask of the day slots this trip runs on.
    days: u8,
}

#[derive(Debug, Clone)]
struct RawRoute {
    stops: Vec<u8>,
    bikes: bool,
    trips: Vec<RawTrip>,
}

#[derive(Debug, Clone)]
struct RawNet {
    stop_count: u8,
    min_change: Vec<u8>,
    stop_node: Vec<u8>,
    edges: Vec<(u8, u8, u8)>,
    access: Vec<Option<u8>>,
    egress: Vec<Option<u8>>,
    routes: Vec<RawRoute>,
    /// One offset per service day.
    day_offsets: Vec<i32>,
    /// Added to every clock, to run a network near the limits of time.
    shift: i32,
    max_trips: u8,
}

fn raw_trip() -> impl Strategy<Value = RawTrip> {
    (
        0_u8..90,
        prop::collection::vec((0_u8..10, 0_u8..4), 1..5),
        prop::collection::vec(
            (prop::bool::weighted(0.85), prop::bool::weighted(0.85)),
            1..5,
        ),
        1_u8..8,
    )
        .prop_map(|(start, hops, rules, days)| RawTrip {
            start,
            hops,
            rules,
            days,
        })
}

fn raw_route() -> impl Strategy<Value = RawRoute> {
    (
        prop::collection::vec(any::<u8>(), 2..10),
        prop::bool::weighted(0.8),
        // Up to 20 trips in a lane, so the binary search over rows has rows to search.
        prop::collection::vec(raw_trip(), 1..20),
    )
        .prop_map(|(stops, bikes, trips)| RawRoute {
            stops,
            bikes,
            trips,
        })
}

fn raw_net() -> impl Strategy<Value = RawNet> {
    let opt = || prop::collection::vec(prop::option::weighted(0.5, 0_u8..15), 8);
    (
        3_u8..7,
        prop::collection::vec(0_u8..8, 8),
        prop::collection::vec(0_u8..4, 8),
        prop::collection::vec((any::<u8>(), any::<u8>(), 0_u8..10), 2..24),
        opt(),
        opt(),
        prop::collection::vec(raw_route(), 2..9),
        prop::collection::vec(prop::sample::select(DAY_OFFSETS.to_vec()), 1..4),
        prop_oneof![6 => Just(0), 1 => Just(SHIFT_HIGH), 1 => Just(SHIFT_LOW)],
        1_u8..7,
    )
        .prop_map(
            |(
                stop_count,
                min_change,
                stop_node,
                edges,
                access,
                egress,
                routes,
                day_offsets,
                shift,
                max_trips,
            )| RawNet {
                stop_count,
                min_change,
                stop_node,
                edges,
                access,
                egress,
                routes,
                day_offsets,
                shift,
                max_trips,
            },
        )
}

/// The rule for a position, cycling a trip's raw rules.
fn rules_at(trip: &RawTrip, position: usize) -> StopRules {
    let (pickup, drop_off) = trip.rules[position % trip.rules.len()];
    StopRules { pickup, drop_off }
}

/// Times along a trip from its raw hops: each stop dwells, then hops to the next.
fn times_of(trip: &RawTrip, stop_count: usize) -> Vec<(i32, i32)> {
    let mut clock = i32::from(trip.start);
    (0..stop_count)
        .map(|position| {
            let (hop, dwell) = trip.hops[position % trip.hops.len()];
            let arrival = clock;
            let departure = arrival + i32::from(dwell);
            clock = departure + i32::from(hop);
            (arrival, departure)
        })
        .collect()
}

fn stop_of(raw: u8, stop_count: u8) -> usize {
    usize::from(raw % stop_count)
}

/// In an added route's raw stops: resolves to the overlay's first added stop.
const FIRST_ADDED_STOP: u8 = 255;

/// The date of a day slot.
fn date_of(slot: usize) -> ServiceDate {
    ServiceDate::new(u32::try_from(slot).expect("few days") + 1)
}

/// Builds a network from raw parts: stops `0..stop_count`, and the two service days.
fn build_net(raw: &RawNet) -> TestNet {
    let mut builder = TimetableBuilder::new();
    let keys: Vec<_> = (0..raw.stop_count)
        .map(|s| {
            builder
                .add_stop(secs(u32::from(raw.min_change[usize::from(s)])))
                .expect("stop")
        })
        .collect();
    // One calendar per subset of the (up to three) day slots.
    let services: Vec<_> = (1_u8..8)
        .map(|mask| {
            let dates = (0..3)
                .filter(move |slot| mask & (1 << slot) != 0)
                .map(date_of);
            builder.add_service(dates).expect("service")
        })
        .collect();

    let mut next_trip = 0;
    for route in &raw.routes {
        let stops: Vec<_> = route
            .stops
            .iter()
            .map(|&s| keys[stop_of(s, raw.stop_count)])
            .collect();
        let attributes = if route.bikes {
            RouteAttributes::BIKES_ALLOWED
        } else {
            RouteAttributes::NONE
        };
        let pattern = builder.add_pattern(&stops, attributes).expect("pattern");
        for trip in &route.trips {
            let service = services[usize::from(trip.days) - 1];
            let times = times_of(trip, stops.len())
                .iter()
                .enumerate()
                .map(|(position, &(arrival, departure))| StopTime {
                    arrival: t(arrival),
                    departure: t(departure),
                    rules: rules_at(trip, position),
                })
                .collect();
            builder
                .add_trip(pattern, TripId::new(next_trip), service, times)
                .expect("valid trip");
            next_trip += 1;
        }
    }
    let timetable = builder.build().expect("build");
    let days: Vec<_> = raw
        .day_offsets
        .iter()
        .enumerate()
        .map(|(slot, &offset)| ServiceDay {
            date: date_of(slot),
            offset: t(offset + raw.shift),
        })
        .collect();
    let window = Arc::new(timetable.materialize(&days).expect("materialize"));
    let snapshot = Snapshot::schedule(Arc::clone(&window));

    let count = usize::from(raw.stop_count);
    let mut stop_node = vec![0; count];
    let mut min_change = vec![0; count];
    let mut access = StopTable::new(count);
    let mut egress = StopTable::new(count);
    let node_count = count + 2;
    for (index, &key) in keys.iter().enumerate() {
        let id = timetable.stop_id(key);
        // Stops 0 to 3 of the node table may collapse together: zero-length walks.
        stop_node[id.index()] = u32::from(raw.stop_node[index]) % u32::from(raw.stop_count + 2);
        min_change[id.index()] = i32::from(raw.min_change[index]);
        if let Some(cost) = raw.access[index] {
            access.set(id, secs(u32::from(cost))).expect("in range");
        }
        if let Some(cost) = raw.egress[index] {
            egress.set(id, secs(u32::from(cost))).expect("in range");
        }
    }
    let edges: Vec<_> = raw
        .edges
        .iter()
        .map(|&(a, b, c)| {
            (
                u32::from(a) % u32::from(raw.stop_count + 2),
                u32::from(b) % u32::from(raw.stop_count + 2),
                i32::from(c),
            )
        })
        .collect();
    let graph = Arc::new(Graph::new(node_count, &edges, stop_node));
    let routes = routes_from_timetable(&timetable, &window);
    let dist = graph.distances();
    TestNet {
        min_change,
        routes,
        graph,
        dist,
        access,
        egress,
        snapshot,
    }
}

fn results_of(
    router: &mut Router<SyntheticSearch>,
    query: &TransitQuery<'_>,
    d: Direction,
    time: i32,
) -> Results {
    let run = match d {
        Direction::Forward => router.forward(query, t(time), &|| false).expect("search"),
        Direction::Backward => router.backward(query, t(time), &|| false).expect("search"),
    };
    run.results()
        .iter()
        .map(|r| (r.trips, r.time.into_inner()))
        .collect()
}

/// `(departure, arrival, trips)` of a profile.
fn profile_triples(
    router: &mut Router<SyntheticSearch>,
    query: &TransitQuery<'_>,
    shift: i32,
) -> Triples {
    let profile = router
        .profile(query, t(WINDOW.0 + shift), t(WINDOW.1 + shift), &|| false)
        .expect("profile");
    let mut triples: Triples = profile
        .entries
        .iter()
        .map(|e| (e.departure.into_inner(), e.arrival.into_inner(), e.trips))
        .collect();
    triples.sort_unstable();
    triples
}

/// The checks every network must pass, base or overlaid.
fn check_network(net: &TestNet, filter: RouteFilter, max_trips: u8, shift: i32) {
    let query = net.query(max_trips, filter);
    let mut router = net.router();
    let (earliest, latest) = (WINDOW.0 + shift, WINDOW.1 + shift);

    // The router against the oracle at every second of the window, not just at sampled times.
    let runs = oracle_every_second(net, filter, earliest, latest, max_trips);
    for (offset, expected) in runs.iter().enumerate() {
        let time = earliest + i32::try_from(offset).expect("small window");
        assert_eq!(
            &results_of(&mut router, &query, Direction::Forward, time),
            expected,
            "forward search at {time}"
        );
    }

    // Every search time we test against the oracle.
    for time in [earliest, earliest + 7, latest, latest + 20] {
        let forward = results_of(&mut router, &query, Direction::Forward, time);
        // A backward search from every arrival must return the matching journey, mirrored.
        for &(trips, arrival) in &forward {
            let backward = results_of(&mut router, &query, Direction::Backward, arrival);
            assert_eq!(
                backward,
                oracle(net, filter, Direction::Backward, arrival, max_trips),
                "backward search at {arrival}"
            );
            // The latest departure with at most `trips` trips is no earlier than `time`.
            assert!(
                backward.iter().any(|&(k, d)| k <= trips && d >= time),
                "forward found {trips} trips at {arrival} but backward didn't"
            );
            // Each backward answer, run forward, arrives no later than it promised.
            for &(k, departure) in &backward {
                let replay = results_of(&mut router, &query, Direction::Forward, departure);
                assert!(
                    replay.iter().any(|&(k2, a)| k2 <= k && a <= arrival),
                    "backward said {k} trips leave at {departure} and arrive by {arrival}"
                );
            }
        }
        check_extraction(net, &query, &mut router, time);
    }

    // The profile against brute force over the oracle.
    let profile = profile_triples(&mut router, &query, shift);
    assert_eq!(
        profile,
        brute_force_profile(&runs, earliest),
        "profile against brute force"
    );

    let result = router
        .profile(&query, t(earliest), t(latest), &|| false)
        .expect("profile");
    for entry in &result.entries {
        assert!(entry.journey.trips >= 1);
        // A journey's times are its legs' times, whichever search produced it.
        assert_eq!(entry.departure, entry.journey.departure);
        assert_eq!(entry.arrival, entry.journey.arrival);
        assert_eq!(entry.trips, entry.journey.trips);
        // Walks were resolved when the entry was found, so later searches can't invalidate them.
        check_journey(net, &entry.journey);
    }
}

fn check_extraction(
    net: &TestNet,
    query: &TransitQuery<'_>,
    router: &mut Router<SyntheticSearch>,
    time: i32,
) {
    for d in [Direction::Forward, Direction::Backward] {
        let (results, journeys) = {
            let run = match d {
                Direction::Forward => router.forward(query, t(time), &|| false).expect("search"),
                Direction::Backward => router.backward(query, t(time), &|| false).expect("search"),
            };
            let results = run.results();
            let journeys: Vec<_> = results
                .iter()
                .map(|r| run.journey(r.trips).expect("every result has a journey"))
                .collect();
            (results, journeys)
        };
        for (result, journey) in results.iter().zip(&journeys) {
            check_journey(net, journey);
            match d {
                Direction::Forward => assert_eq!(result.time, journey.arrival),
                Direction::Backward => assert_eq!(result.time, journey.departure),
            }
        }
    }
}

/// An overlay's raw changes.
#[derive(Debug, Clone)]
enum RawChange {
    Delay {
        route: u8,
        day: u8,
        trip: u8,
        position: u8,
        /// Negative for an early trip.
        delay: i8,
    },
    Skip {
        route: u8,
        day: u8,
        trip: u8,
        position: u8,
    },
    Cancel {
        route: u8,
        day: u8,
        trip: u8,
    },
    RemoveService {
        route: u8,
        day: u8,
    },
    /// Replaces the day's whole service, possibly on a day that had none,
    /// and possibly with no service at all.
    ReplaceDay {
        route: u8,
        day: u8,
        trips: Vec<RawTrip>,
    },
}

fn raw_change() -> impl Strategy<Value = RawChange> {
    prop_oneof![
        (
            any::<u8>(),
            any::<u8>(),
            any::<u8>(),
            any::<u8>(),
            -25_i8..26
        )
            .prop_map(|(route, day, trip, position, delay)| RawChange::Delay {
                route,
                day,
                trip,
                position,
                delay
            }),
        (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(
            |(route, day, trip, position)| RawChange::Skip {
                route,
                day,
                trip,
                position
            }
        ),
        (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(route, day, trip)| RawChange::Cancel {
            route,
            day,
            trip
        }),
        (any::<u8>(), any::<u8>()).prop_map(|(route, day)| RawChange::RemoveService { route, day }),
        (
            any::<u8>(),
            any::<u8>(),
            prop::collection::vec(raw_trip(), 0..10)
        )
            .prop_map(|(route, day, trips)| RawChange::ReplaceDay { route, day, trips }),
    ]
}

/// A stop that an overlay adds, with its node in the street graph.
#[derive(Debug, Clone)]
struct RawOverlayStop {
    node: u8,
    min_change: u8,
    access: Option<u8>,
    egress: Option<u8>,
}

/// An overlay's raw changes: edits to base service, plus added stops and an added route.
#[derive(Debug, Clone)]
struct RawOverlay {
    changes: Vec<RawChange>,
    added_stops: Vec<RawOverlayStop>,
    added_route: Option<RawRoute>,
    /// Build replaced blocks through `ResolvedBlock::from_lanes` instead of `from_trips`.
    via_lanes: bool,
}

fn raw_overlay() -> impl Strategy<Value = RawOverlay> {
    let stop = (
        any::<u8>(),
        0_u8..8,
        prop::option::weighted(0.7, 0_u8..15),
        prop::option::weighted(0.7, 0_u8..15),
    )
        .prop_map(|(node, min_change, access, egress)| RawOverlayStop {
            node,
            min_change,
            access,
            egress,
        });
    (
        prop::collection::vec(raw_change(), 0..8),
        prop::collection::vec(stop, 0..3),
        prop::option::of(raw_route()),
        any::<bool>(),
    )
        .prop_map(
            |(changes, added_stops, added_route, via_lanes)| RawOverlay {
                changes,
                added_stops,
                added_route,
                via_lanes,
            },
        )
}

/// Applies an overlay's changes to a network, producing what a search must see.
fn apply_overlay(net: &TestNet, raw: &RawOverlay) -> TestNet {
    let base_stops = net.snapshot.stop_count();
    let day_count = net.snapshot.days().len();
    let total_stops = base_stops + raw.added_stops.len();

    // The effective service, starting from the base and edited below.
    let mut routes: Vec<TestRoute> = net.routes.clone();
    let mut touched = vec![vec![false; day_count]; routes.len()];
    let mut removed = vec![vec![false; day_count]; routes.len()];
    let mut next_replacement = 2_000_000_u32;
    for change in &raw.changes {
        match change.clone() {
            RawChange::Delay {
                route,
                day,
                trip,
                position,
                delay,
            } => {
                let (r, d) = index_of(route, day, routes.len(), day_count);
                let trips = &mut routes[r].days[d];
                if trips.is_empty() {
                    continue;
                }
                let trip_index = usize::from(trip) % trips.len();
                let events = &mut trips[trip_index].events;
                let from = usize::from(position) % events.len();
                // An early trip can't leave a stop before it reached the one before it.
                let earliest_delay = match from.checked_sub(1) {
                    Some(previous) => {
                        events[previous].departure.into_inner() - events[from].arrival.into_inner()
                    }
                    None => i32::MIN,
                };
                let delay = i32::from(delay).max(earliest_delay);
                for event in &mut events[from..] {
                    event.arrival = t(event.arrival.into_inner() + delay);
                    event.departure = t(event.departure.into_inner() + delay);
                }
                touched[r][d] = true;
            }
            RawChange::Skip {
                route,
                day,
                trip,
                position,
            } => {
                let (r, d) = index_of(route, day, routes.len(), day_count);
                let trips = &mut routes[r].days[d];
                if trips.is_empty() {
                    continue;
                }
                let trip_index = usize::from(trip) % trips.len();
                let events = &mut trips[trip_index].events;
                let position = usize::from(position) % events.len();
                events[position].rules = StopRules::SKIPPED;
                touched[r][d] = true;
            }
            RawChange::Cancel { route, day, trip } => {
                let (r, d) = index_of(route, day, routes.len(), day_count);
                let trips = &mut routes[r].days[d];
                if trips.is_empty() {
                    continue;
                }
                let trip_index = usize::from(trip) % trips.len();
                trips.remove(trip_index);
                touched[r][d] = true;
            }
            RawChange::RemoveService { route, day } => {
                let (r, d) = index_of(route, day, routes.len(), day_count);
                routes[r].days[d].clear();
                removed[r][d] = true;
                touched[r][d] = false;
            }
            RawChange::ReplaceDay { route, day, trips } => {
                let (r, d) = index_of(route, day, routes.len(), day_count);
                let offset = net.snapshot.days()[d].offset.into_inner();
                let stop_count = routes[r].stops.len();
                routes[r].days[d] = trips
                    .iter()
                    .map(|trip| {
                        next_replacement += 1;
                        ResolvedTrip {
                            trip: TripId::new(next_replacement),
                            events: events_of(trip, stop_count, offset),
                        }
                    })
                    .collect();
                removed[r][d] = false;
                touched[r][d] = true;
            }
        }
    }

    let mut overlay = OverlayBuilder::new(Arc::clone(net.snapshot.base()));
    let mut stop_node = net.graph.stop_node.clone();
    let mut min_change = net.min_change.clone();
    let mut access = StopTable::new(total_stops);
    let mut egress = StopTable::new(total_stops);
    for s in 0..base_stops {
        let id = StopId::new(u32::try_from(s).expect("few"));
        if let Some(cost) = net.access.cost(id) {
            access.set(id, cost).expect("in range");
        }
        if let Some(cost) = net.egress.cost(id) {
            egress.set(id, cost).expect("in range");
        }
    }
    for (i, stop) in raw.added_stops.iter().enumerate() {
        let id = overlay
            .add_stop(secs(u32::from(stop.min_change)))
            .expect("add stop");
        assert_eq!(
            id.index(),
            base_stops + i,
            "overlay stops follow base stops"
        );
        stop_node
            .push(u32::from(stop.node) % u32::try_from(net.graph.edges_out.len()).expect("few"));
        min_change.push(i32::from(stop.min_change));
        if let Some(cost) = stop.access {
            access.set(id, secs(u32::from(cost))).expect("in range");
        }
        if let Some(cost) = stop.egress {
            egress.set(id, secs(u32::from(cost))).expect("in range");
        }
    }

    for (r, per_day) in touched.iter().enumerate() {
        for (d, &changed) in per_day.iter().enumerate() {
            let route = RouteId::new(u32::try_from(r).expect("few"));
            let slot = day_slot(u8::try_from(d).expect("few"));
            if removed[r][d] {
                overlay.remove_service(route, slot).expect("remove");
            } else if changed {
                overlay
                    .replace_block(
                        route,
                        slot,
                        &block_of_raw(&routes[r].days[d], raw.via_lanes),
                    )
                    .expect("replace");
            }
        }
    }

    // A route that may serve overlay stops and base stops alike.
    if let Some(added) = &raw.added_route {
        let stops: Vec<StopId> = added
            .stops
            .iter()
            .map(|&s| {
                if s == FIRST_ADDED_STOP && !raw.added_stops.is_empty() {
                    StopId::new(u32::try_from(base_stops).expect("few"))
                } else {
                    StopId::new(u32::from(s) % u32::try_from(total_stops).expect("few"))
                }
            })
            .collect();
        let attributes = if added.bikes {
            RouteAttributes::BIKES_ALLOWED
        } else {
            RouteAttributes::NONE
        };
        let route = overlay.add_route(&stops, attributes).expect("add route");
        let mut days = vec![Vec::new(); day_count];
        let mut next_trip = 1_000_000_u32;
        for trip in &added.trips {
            for (d, service_day) in net.snapshot.days().iter().enumerate() {
                if trip.days & (1 << d) == 0 {
                    continue;
                }
                let events = events_of(trip, stops.len(), service_day.offset.into_inner());
                days[d].push(ResolvedTrip {
                    trip: TripId::new(next_trip),
                    events,
                });
                next_trip += 1;
            }
        }
        for (d, trips) in days.iter().enumerate() {
            if !trips.is_empty() {
                overlay
                    .replace_block(
                        route,
                        day_slot(u8::try_from(d).expect("few")),
                        &block_of(trips.clone()),
                    )
                    .expect("replace");
            }
        }
        routes.push(TestRoute {
            stops,
            attributes,
            days,
        });
    }

    let node_count = net.graph.edges_out.len();
    let edges: Vec<(u32, u32, i32)> = net
        .graph
        .edges_out
        .iter()
        .enumerate()
        .flat_map(|(from, list)| {
            let from = u32::try_from(from).expect("few");
            list.iter().map(move |&(to, cost)| (from, to, cost))
        })
        .collect();
    let graph = Arc::new(Graph::new(node_count, &edges, stop_node));
    let dist = graph.distances();
    let snapshot = overlay.build().expect("build overlay");
    assert_eq!(snapshot.stop_count(), total_stops);
    TestNet {
        min_change,
        routes,
        graph,
        dist,
        access,
        egress,
        snapshot,
    }
}

/// The events of a raw trip on a day whose window clock starts at `offset`.
fn events_of(trip: &RawTrip, stop_count: usize, offset: i32) -> Vec<StopEvent> {
    times_of(trip, stop_count)
        .iter()
        .enumerate()
        .map(|(position, &(arrival, departure))| StopEvent {
            arrival: t(arrival + offset),
            departure: t(departure + offset),
            rules: rules_at(trip, position),
        })
        .collect()
}

/// A block of these trips, through `from_trips` or else `from_lanes`,
/// the latter with the extra empty lane that canceling every trip of a lane can leave.
fn block_of_raw(trips: &[ResolvedTrip], via_lanes: bool) -> ResolvedBlock {
    let block = block_of(trips.to_vec());
    if !via_lanes {
        return block;
    }
    let mut lanes: Vec<_> = block
        .lanes()
        .iter()
        .map(|lane| ResolvedLane::new(lane.trips().to_vec()).expect("a lane is valid"))
        .collect();
    lanes.push(ResolvedLane::new(Vec::new()).expect("an emptied lane is accepted"));
    ResolvedBlock::from_lanes(lanes)
}

fn index_of(route: u8, day: u8, routes: usize, days: usize) -> (usize, usize) {
    (usize::from(route) % routes, usize::from(day) % days)
}

/// Runs a network through the filters.
fn check_both_filters(net: &TestNet, raw: &RawNet) {
    check_network(net, RouteFilter::ALL, raw.max_trips, raw.shift);
    check_network(
        net,
        RouteFilter::requiring(RouteAttributes::BIKES_ALLOWED),
        raw.max_trips,
        raw.shift,
    );
}

proptest! {
    // Case counts come from proptest's default, which `PROPTEST_CASES` overrides for longer runs.
    #![proptest_config(ProptestConfig {
        max_shrink_iters: 500,
        .. ProptestConfig::default()
    })]

    #[test]
    fn base_networks_match_oracle_and_brute_force(raw in raw_net()) {
        let net = build_net(&raw);
        check_both_filters(&net, &raw);
    }

    #[test]
    fn overlaid_networks_match_oracle_and_brute_force(
        raw in raw_net(),
        overlay in raw_overlay(),
    ) {
        let base = build_net(&raw);
        let net = apply_overlay(&base, &overlay);
        check_both_filters(&net, &raw);
    }

    /// An overlay adds stops, and a trip on an added route calls at them.
    #[test]
    fn added_stops_match_oracle_and_brute_force(
        raw in raw_net(),
        overlay in raw_overlay(),
        extra in (
            any::<u8>(),
            0_u8..8,
            prop::option::weighted(0.7, 0_u8..15),
            prop::option::weighted(0.7, 0_u8..15),
        ),
        route in raw_route(),
    ) {
        let base = build_net(&raw);
        let mut overlay = overlay;
        // Keep the overlay's edits, but guarantee a stop that the added route must use.
        overlay.added_stops.push(RawOverlayStop {
            node: extra.0,
            min_change: extra.1,
            access: extra.2,
            egress: extra.3,
        });
        let mut route = route;
        // The strategy cannot see how many stops the base has, so `apply_overlay` maps this
        // marker value onto the overlay's added stop.
        route.stops[0] = FIRST_ADDED_STOP;
        overlay.added_route = Some(route);
        let net = apply_overlay(&base, &overlay);
        check_both_filters(&net, &raw);
    }

    /// One router serves snapshots of different sizes in turn, as a worker does when
    /// overlays add stops and are replaced: its reused labels must not leak between them.
    #[test]
    fn reused_routers_match_fresh_ones_across_snapshots(
        raw in raw_net(),
        overlay in raw_overlay(),
    ) {
        let base = build_net(&raw);
        let overlaid = apply_overlay(&base, &overlay);
        let (earliest, latest) = (WINDOW.0 + raw.shift, WINDOW.1 + raw.shift);
        let mut shared = base.router();
        for net in [&overlaid, &base, &overlaid, &base] {
            shared.transfers = SyntheticSearch::new(Arc::clone(&net.graph));
            let query = net.query(raw.max_trips, RouteFilter::ALL);
            let mut fresh = net.router();
            prop_assert_eq!(
                results_of(&mut shared, &query, Direction::Forward, earliest),
                results_of(&mut fresh, &query, Direction::Forward, earliest)
            );
            prop_assert_eq!(
                results_of(&mut shared, &query, Direction::Backward, latest + 40),
                results_of(&mut fresh, &query, Direction::Backward, latest + 40)
            );
            prop_assert_eq!(
                profile_triples(&mut shared, &query, raw.shift),
                profile_triples(&mut fresh, &query, raw.shift)
            );
        }
    }
}

/// Guards against overlays that never overtake or never use an added stop.
#[test]
fn generated_overlays_are_not_trivial() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let (mut overlaid_journeys, mut uses_added_stop, mut several_lanes) = (0, 0, 0);
    let samples = 400;
    for _ in 0..samples {
        let raw = raw_net().new_tree(&mut runner).expect("sample").current();
        let overlay = raw_overlay()
            .new_tree(&mut runner)
            .expect("sample")
            .current();
        let base = build_net(&raw);
        let net = apply_overlay(&base, &overlay);
        let base_stops = base.snapshot.stop_count();
        let query = net.query(raw.max_trips, RouteFilter::ALL);
        let mut router = net.router();
        let profile = router
            .profile(
                &query,
                t(WINDOW.0 + raw.shift),
                t(WINDOW.1 + raw.shift),
                &|| false,
            )
            .expect("profile");
        let rides = || {
            profile.entries.iter().flat_map(|e| {
                e.journey.legs.iter().filter_map(|leg| match leg {
                    crate::Leg::Ride(ride) => Some(*ride),
                    _ => None,
                })
            })
        };
        if rides().any(|ride| ride.lane.is_overlay()) {
            overlaid_journeys += 1;
        }
        if rides().any(|ride| {
            ride.board_stop.index() >= base_stops || ride.alight_stop.index() >= base_stops
        }) {
            uses_added_stop += 1;
        }
        let arena = &net.snapshot.overlay_arena;
        if arena.blocks.iter().any(|block| block.lane_count >= 2) {
            several_lanes += 1;
        }
    }
    println!(
        "of {samples}: overlay rides {overlaid_journeys}, added stop used {uses_added_stop}, \
         several lanes {several_lanes}"
    );
    assert!(
        overlaid_journeys > 20,
        "only {overlaid_journeys} rode an overlay lane"
    );
    assert!(
        uses_added_stop > 5,
        "only {uses_added_stop} used an added stop"
    );
    assert!(
        several_lanes > 20,
        "only {several_lanes} had overtaking lanes in an overlay block"
    );
}

#[test]
fn zero_second_seconds_constant_is_zero() {
    assert_eq!(Seconds::ZERO, secs(0));
}

/// Guards against the generators drifting into trivial networks,
/// where the comparisons above would pass while proving nothing.
#[test]
fn generated_networks_are_not_trivial() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = raw_net();
    let (mut with_results, mut with_transfer, mut with_multi_trip, mut profiles_with_several) =
        (0, 0, 0, 0);
    let samples = 400;
    for _ in 0..samples {
        let raw = strategy.new_tree(&mut runner).expect("sample").current();
        let net = build_net(&raw);
        let query = net.query(raw.max_trips, RouteFilter::ALL);
        let mut router = net.router();
        let profile = router
            .profile(
                &query,
                t(WINDOW.0 + raw.shift),
                t(WINDOW.1 + raw.shift),
                &|| false,
            )
            .expect("profile");
        if !profile.entries.is_empty() {
            with_results += 1;
        }
        if profile.entries.len() >= 2 {
            profiles_with_several += 1;
        }
        if profile.entries.iter().any(|e| e.trips >= 2) {
            with_multi_trip += 1;
        }
        if profile.entries.iter().any(|e| {
            e.journey
                .legs
                .iter()
                .any(|leg| matches!(leg, crate::Leg::Transfer { .. }))
        }) {
            with_transfer += 1;
        }
    }
    println!(
        "of {samples}: results {with_results}, several {profiles_with_several}, \
         multi-trip {with_multi_trip}, transfers {with_transfer}"
    );
    assert!(
        with_results > samples / 4,
        "only {with_results} of {samples} had any journey"
    );
    assert!(
        with_multi_trip > 20,
        "only {with_multi_trip} of {samples} used two trips"
    );
    assert!(
        with_transfer > 5,
        "only {with_transfer} of {samples} walked between trips"
    );
    assert!(
        profiles_with_several > 20,
        "only {profiles_with_several} had two or more entries"
    );
}
