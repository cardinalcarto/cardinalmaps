//! Seeded stress tests over wider networks than `proptests.rs`, and an exhaustive enumerator
//! of journeys that shares neither labels nor the round recurrence with the router or oracle.

#![allow(clippy::too_many_lines, clippy::too_many_arguments)]

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

use crate::ids::{DaySlot, Direction, ServiceDate, ServiceDay, StopId};
use crate::overlay::OverlayBuilder;
use crate::raptor::Router;
use crate::snapshot::Snapshot;
use crate::stop_table::{RouteFilter, StopTable};
use crate::testing::{
    Graph, SyntheticSearch, TestNet, block_of, brute_force_profile, check_journey, oracle,
    oracle_every_second, routes_from_timetable, secs, t,
};
use crate::timetable::{
    ResolvedTrip, RouteAttributes, StopEvent, StopRules, StopTime, TimetableBuilder,
};
use crate::{RouteId, TripId};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + i32::try_from(self.below(u64::try_from(hi - lo).unwrap())).unwrap()
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}

fn rules(rng: &mut Rng) -> StopRules {
    StopRules {
        pickup: rng.chance(85),
        drop_off: rng.chance(85),
    }
}

/// Times and rules for a random trip.
fn random_events(
    rng: &mut Rng,
    len: usize,
    start_lo: i32,
    start_hi: i32,
) -> Vec<(i32, i32, StopRules)> {
    let mut clock = rng.range(start_lo, start_hi);
    let fixed_rules = rng.chance(60);
    let base_rule = rules(rng);
    (0..len)
        .map(|_| {
            let arrival = clock;
            let departure = arrival + rng.range(0, 6);
            clock = departure + rng.range(0, 25);
            let rule = if fixed_rules { base_rule } else { rules(rng) };
            (arrival, departure, rule)
        })
        .collect()
}

const OFFSETS: [i32; 3] = [-60, 0, 70];

fn random_net(rng: &mut Rng) -> TestNet {
    let stops = usize::try_from(rng.range(3, 12)).unwrap();
    let mut builder = TimetableBuilder::new();
    let keys: Vec<_> = (0..stops)
        .map(|_| {
            let change = if rng.chance(50) { 0 } else { rng.range(0, 30) };
            builder
                .add_stop(secs(u32::try_from(change).unwrap()))
                .unwrap()
        })
        .collect();
    let dates = [1u32, 2, 3].map(ServiceDate::new);
    let services: Vec<_> = (1u32..8)
        .map(|mask| {
            let set: Vec<_> = (0..3)
                .filter(|i| mask & (1 << i) != 0)
                .map(|i: usize| dates[i])
                .collect();
            builder.add_service(set).unwrap()
        })
        .collect();
    let routes = rng.range(2, 12);
    let mut next_trip = 0u32;
    for _ in 0..routes {
        let len = usize::try_from(rng.range(2, 9)).unwrap();
        let seq: Vec<_> = (0..len)
            .map(|_| keys[usize::try_from(rng.below(stops as u64)).unwrap()])
            .collect();
        let pattern = builder.add_pattern(&seq, RouteAttributes::NONE).unwrap();
        let trips = rng.range(1, 30);
        for _ in 0..trips {
            let times = random_events(rng, len, 0, 120)
                .into_iter()
                .map(|(a, d, r)| StopTime {
                    arrival: t(a),
                    departure: t(d),
                    rules: r,
                })
                .collect();
            let service = services[usize::try_from(rng.below(7)).unwrap()];
            builder
                .add_trip(pattern, TripId::new(next_trip), service, times)
                .unwrap();
            next_trip += 1;
        }
    }
    let timetable = builder.build().unwrap();
    let days: Vec<_> = dates
        .iter()
        .zip(OFFSETS)
        .map(|(&date, offset)| ServiceDay {
            date,
            offset: t(offset),
        })
        .collect();
    let window = Arc::new(timetable.materialize(&days).unwrap());
    let snapshot = Snapshot::schedule(Arc::clone(&window));
    let nodes = stops + 2;
    let mut stop_node = vec![0u32; stops];
    let mut min_change = vec![0; stops];
    let mut access = StopTable::new(stops);
    let mut egress = StopTable::new(stops);
    for (&key, _) in keys.iter().zip(0..) {
        let id = timetable.stop_id(key);
        stop_node[id.index()] = u32::try_from(rng.below(nodes as u64)).unwrap();
        min_change[id.index()] = window.min_change[id.index()];
        if rng.chance(50) {
            access
                .set(id, secs(u32::try_from(rng.range(0, 20)).unwrap()))
                .unwrap();
        }
        if rng.chance(50) {
            egress
                .set(id, secs(u32::try_from(rng.range(0, 20)).unwrap()))
                .unwrap();
        }
    }
    let edge_count = rng.range(0, 30);
    let edges: Vec<_> = (0..edge_count)
        .map(|_| {
            (
                u32::try_from(rng.below(nodes as u64)).unwrap(),
                u32::try_from(rng.below(nodes as u64)).unwrap(),
                rng.range(0, 25),
            )
        })
        .collect();
    let graph = Arc::new(Graph::new(nodes, &edges, stop_node));
    let dist = graph.distances();
    let routes = routes_from_timetable(&timetable, &window);
    TestNet {
        no_change: vec![false; stops],
        pair_floors: HashMap::new(),
        min_change,
        routes,
        graph,
        dist,
        access,
        egress,
        snapshot,
    }
}

fn apply_random_overlay(rng: &mut Rng, net: &TestNet) -> TestNet {
    let mut routes = net.routes.clone();
    let mut overlay = OverlayBuilder::new(Arc::clone(net.snapshot.base()));
    let edits = rng.range(1, 10);
    for _ in 0..edits {
        let r = usize::try_from(rng.below(routes.len() as u64)).unwrap();
        let d = usize::try_from(rng.below(3)).unwrap();
        let slot = DaySlot::new(u8::try_from(d).unwrap());
        let route = RouteId::new(u32::try_from(r).unwrap());
        if rng.below(3) == 0 {
            overlay.remove_service(route, slot).unwrap();
            routes[r].days[d].clear();
        } else {
            // Replace the whole day: new trips, earlier or later, possibly on a day
            // that had no service, and possibly the same (route, day) twice.
            let len = routes[r].stops.len();
            let count = rng.range(0, 12);
            let offset = OFFSETS[d];
            let trips: Vec<_> = (0..count)
                .map(|i| ResolvedTrip {
                    trip: TripId::new(5_000_000 + u32::try_from(i).unwrap()),
                    events: random_events(rng, len, offset - 30, offset + 150)
                        .into_iter()
                        .map(|(a, dep, rule)| StopEvent {
                            arrival: t(a),
                            departure: t(dep),
                            rules: rule,
                        })
                        .collect(),
                })
                .collect();
            overlay
                .replace_block(route, slot, &block_of(trips.clone()))
                .unwrap();
            routes[r].days[d] = trips;
        }
    }
    let snapshot = overlay.build().unwrap();
    TestNet {
        no_change: net.no_change.clone(),
        pair_floors: HashMap::new(),
        min_change: net.min_change.clone(),
        routes,
        graph: Arc::clone(&net.graph),
        dist: net.dist.clone(),
        access: net.access.clone(),
        egress: net.egress.clone(),
        snapshot,
    }
}

fn check(net: &TestNet, router: &mut Router<SyntheticSearch>, rng: &mut Rng, seed: u64) {
    let max_trips = u8::try_from(rng.range(1, 9)).unwrap();
    router.transfers = SyntheticSearch::new(Arc::clone(&net.graph));
    let query = net.query(max_trips, RouteFilter::ALL);
    for _ in 0..6 {
        let time = rng.range(-120, 260);
        for d in [Direction::Forward, Direction::Backward] {
            let expected = oracle(net, RouteFilter::ALL, d, time, max_trips);
            let (results, journeys): (Vec<(u8, i32)>, Vec<_>) = {
                let run = match d {
                    Direction::Forward => router.forward(&query, t(time), &|| false).unwrap(),
                    Direction::Backward => router.backward(&query, t(time), &|| false).unwrap(),
                };
                let r = run.results();
                let js = r
                    .iter()
                    .map(|x| run.journey(x.trips).expect("journey"))
                    .collect();
                (
                    r.iter().map(|x| (x.trips, x.time.into_inner())).collect(),
                    js,
                )
            };
            assert_eq!(results, expected, "seed {seed}, {d:?} at {time}");
            for (&(trips, _), journey) in results.iter().zip(&journeys) {
                assert_eq!(journey.trips, trips, "seed {seed}: round vs ride count");
                check_journey(net, journey);
            }
        }
    }
    let (earliest, latest) = (rng.range(-90, 100), 0);
    let latest = earliest + latest + rng.range(0, 90);
    let profile = router
        .profile(&query, t(earliest), t(latest), &|| false)
        .unwrap();
    for entry in &profile.entries {
        check_journey(net, &entry.journey);
    }
    let mut triples: Vec<_> = profile
        .entries
        .iter()
        .map(|e| (e.departure.into_inner(), e.arrival.into_inner(), e.trips))
        .collect();
    triples.sort_unstable();
    let runs = oracle_every_second(net, RouteFilter::ALL, earliest, latest, max_trips);
    let brute = brute_force_profile(&runs, earliest);
    assert_eq!(triples, brute, "seed {seed}: profile vs brute force");
}

/// Wider networks than the proptests: long routes, many trips per lane, three days including a
/// negative offset, negative query times, labels reused across differently sized networks.
#[test]
fn stress_against_oracle() {
    let cases: u64 = std::env::var("TRANSIT_STRESS_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(150);
    let first = random_net(&mut Rng(1));
    let mut router = first.router();
    for seed in 1..=cases {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let net = random_net(&mut rng);
        check(&net, &mut router, &mut rng, seed);
        let overlaid = apply_random_overlay(&mut rng, &net);
        check(&overlaid, &mut router, &mut rng, seed);
    }
}

/// Every journey by exhaustive depth-first enumeration of trip/walk sequences.
/// This shares neither the labels nor the per-round recurrence of `oracle` or the router.
fn enumerate(net: &TestNet, max_trips: u8) -> BTreeSet<(i32, i32, u8)> {
    fn go(
        net: &TestNet,
        out: &mut BTreeSet<(i32, i32, u8)>,
        stop: usize,
        ready: i32,
        started: Option<i32>,
        access: i32,
        rides: u8,
        max_trips: u8,
    ) {
        if rides == max_trips {
            return;
        }
        for route in &net.routes {
            for day in &route.days {
                for trip in day {
                    for from in 0..route.stops.len() {
                        if route.stops[from].index() != stop {
                            continue;
                        }
                        let board = trip.events[from];
                        if !board.rules.pickup || board.departure.into_inner() < ready {
                            continue;
                        }
                        let departure = started.unwrap_or(board.departure.into_inner() - access);
                        for to in from + 1..route.stops.len() {
                            let alight = trip.events[to];
                            if !alight.rules.drop_off {
                                continue;
                            }
                            let at = route.stops[to];
                            let clock = alight.arrival.into_inner();
                            if let Some(cost) = net.egress.cost(at) {
                                out.insert((departure, clock + cost.as_i32(), rides + 1));
                            }
                            let i = at.index();
                            go(
                                net,
                                out,
                                i,
                                if net.no_change[i] {
                                    i32::MAX
                                } else {
                                    clock + net.min_change[i]
                                },
                                Some(departure),
                                access,
                                rides + 1,
                                max_trips,
                            );
                            for v in 0..net.min_change.len() {
                                if let Some(cost) = net.walk_cost(i, v) {
                                    go(
                                        net,
                                        out,
                                        v,
                                        clock + cost,
                                        Some(departure),
                                        access,
                                        rides + 1,
                                        max_trips,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut out = BTreeSet::new();
    for s in 0..net.min_change.len() {
        let id = StopId::new(u32::try_from(s).unwrap());
        if let Some(cost) = net.access.cost(id) {
            go(
                net,
                &mut out,
                s,
                i32::MIN / 2,
                None,
                cost.as_i32(),
                0,
                max_trips,
            );
        }
    }
    out
}

fn small_net(rng: &mut Rng) -> TestNet {
    loop {
        let net = random_net(rng);
        let trips: usize = net.routes.iter().flat_map(|r| &r.days).map(Vec::len).sum();
        if net.min_change.len() <= 7 && net.routes.len() <= 5 && trips <= 45 {
            return net;
        }
    }
}

fn improving(mut rows: Vec<(u8, i32)>) -> Vec<(u8, i32)> {
    rows.sort_unstable();
    let mut best = i32::MAX;
    rows.retain(|&(_, v)| {
        let keep = v < best;
        if keep {
            best = v;
        }
        keep
    });
    rows
}

/// The router and profile against journey enumeration, independent of the DP.
#[test]
fn router_matches_exhaustive_enumeration() {
    let cases: u64 = std::env::var("TRANSIT_ENUMERATION_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    let (mut journeys, mut multi) = (0_usize, 0_usize);
    for seed in 1..=cases {
        let mut rng = Rng(seed.wrapping_mul(0xD6E8_FEB8_6659_FD93) | 1);
        let net = small_net(&mut rng);
        let k = 3;
        let all = enumerate(&net, k);
        journeys += all.len();
        multi += all.iter().filter(|j| j.2 >= 2).count();
        let query = net.query(k, RouteFilter::ALL);
        let mut router = net.router();
        for _ in 0..8 {
            let time = rng.range(-100, 240);
            let run = router.forward(&query, t(time), &|| false).unwrap();
            let got: Vec<_> = run
                .results()
                .iter()
                .map(|r| (r.trips, r.time.into_inner()))
                .collect();
            let want = improving(
                all.iter()
                    .filter(|j| j.0 >= time)
                    .map(|j| (j.2, j.1))
                    .collect(),
            );
            assert_eq!(got, want, "seed {seed} forward at {time}");
            let run = router.backward(&query, t(time), &|| false).unwrap();
            let got: Vec<_> = run
                .results()
                .iter()
                .map(|r| (r.trips, -r.time.into_inner()))
                .collect();
            let want = improving(
                all.iter()
                    .filter(|j| j.1 <= time)
                    .map(|j| (j.2, -j.0))
                    .collect(),
            );
            assert_eq!(got, want, "seed {seed} backward at {time}");
        }
        let (a, b) = (rng.range(-80, 60), rng.range(60, 200));
        let profile = router.profile(&query, t(a), t(b), &|| false).unwrap();
        let mut got: Vec<_> = profile
            .entries
            .iter()
            .map(|e| (e.departure.into_inner(), e.arrival.into_inner(), e.trips))
            .collect();
        got.sort_unstable();
        // A journey departing after the window still dominates one inside it.
        let want: Vec<_> =
            crate::testing::pareto(all.iter().copied().filter(|j| j.0 >= a).collect())
                .into_iter()
                .filter(|j| j.0 <= b)
                .collect();
        assert_eq!(got, want, "seed {seed} profile [{a}, {b}]");
    }
    println!("enumerated {journeys} journeys, {multi} with 2+ trips");
    assert!(
        u64::try_from(journeys).unwrap() > cases,
        "enumeration is near-empty"
    );
}
