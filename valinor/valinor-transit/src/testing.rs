//! Test support: a synthetic street graph, an exhaustive oracle, and a journey checker.
//!
//! The oracle is deliberately simple and shares no code with the router:
//! no lanes, no pruning, no sentinels, every trip examined at every position.

#![allow(clippy::needless_range_loop, clippy::too_many_lines)]

use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::convert::Infallible;
use std::sync::Arc;

use crate::TIME_LIMIT;
use crate::ids::{DaySlot, Direction, RouteId, Seconds, ServiceDate, ServiceDay, StopId, Time};
use crate::journey::{Journey, Leg};
use crate::raptor::{Router, TransitQuery};
use crate::snapshot::Snapshot;
use crate::stop_table::{RouteFilter, StopTable};
use crate::timetable::{
    BaseWindow, ResolvedBlock, ResolvedTrip, RouteAttributes, StopRules, StopTime, Timetable,
    TimetableBuilder,
};
use crate::transfer::{
    Clock, TransferArrival, TransferOutcome, TransferRequest, TransferSearch, TransferSink,
};

/// A time.
pub(crate) fn t(seconds: i32) -> Time {
    Time::try_new(seconds).expect("time in range")
}

/// A duration.
pub(crate) fn secs(seconds: u32) -> Seconds {
    Seconds::try_new(seconds).expect("duration in range")
}

/// An unreachable clock in the oracle, above every time near the limits and clear of overflow.
const INF: i32 = TIME_LIMIT * 2;

/// A walking distance between unconnected nodes, small enough to add two of them.
pub(crate) const NO_WALK: i32 = TIME_LIMIT / 2;

/// A directed street graph with stops on some of its nodes.
#[derive(Debug, Clone)]
pub(crate) struct Graph {
    pub(crate) edges_out: Vec<Vec<(u32, i32)>>,
    pub(crate) edges_in: Vec<Vec<(u32, i32)>>,
    /// The node of each stop.
    pub(crate) stop_node: Vec<u32>,
    /// The stops at each node.
    pub(crate) node_stops: Vec<Vec<u32>>,
}

impl Graph {
    pub(crate) fn new(nodes: usize, edges: &[(u32, u32, i32)], stop_node: Vec<u32>) -> Self {
        let mut edges_out = vec![Vec::new(); nodes];
        let mut edges_in = vec![Vec::new(); nodes];
        for &(from, to, cost) in edges {
            edges_out[from as usize].push((to, cost));
            edges_in[to as usize].push((from, cost));
        }
        let mut node_stops = vec![Vec::new(); nodes];
        for (stop, &node) in stop_node.iter().enumerate() {
            node_stops[node as usize].push(u32::try_from(stop).expect("few stops"));
        }
        Self {
            edges_out,
            edges_in,
            stop_node,
            node_stops,
        }
    }

    /// Exhaustive `stop → stop` walking distances, following edges as directed.
    pub(crate) fn distances(&self) -> Vec<Vec<i32>> {
        let n = self.edges_out.len();
        let mut d = vec![vec![NO_WALK; n]; n];
        for (node, row) in d.iter_mut().enumerate() {
            row[node] = 0;
        }
        for (from, edges) in self.edges_out.iter().enumerate() {
            for &(to, cost) in edges {
                let cell = &mut d[from][to as usize];
                *cell = (*cell).min(cost);
            }
        }
        for k in 0..n {
            for i in 0..n {
                for j in 0..n {
                    if d[i][k] + d[k][j] < d[i][j] {
                        d[i][j] = (d[i][k] + d[k][j]).min(NO_WALK);
                    }
                }
            }
        }
        let stops = self.stop_node.len();
        let mut out = vec![vec![NO_WALK; stops]; stops];
        for (u, row) in out.iter_mut().enumerate() {
            for (v, cell) in row.iter_mut().enumerate() {
                *cell = d[self.stop_node[u] as usize][self.stop_node[v] as usize];
            }
        }
        out
    }
}

/// A parent handle: an index into the search's trace of settled labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TraceId(u32);

#[derive(Debug, Clone, Copy)]
struct Trace {
    node: u32,
    previous: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct NodeLabel {
    clock: i32,
    source: u32,
    trace: u32,
}

/// Dijkstra over the two best labels with distinct sources, per the trait's contract.
#[derive(Debug, Clone)]
pub(crate) struct SyntheticSearch {
    pub(crate) graph: Arc<Graph>,
    trace: Vec<Trace>,
    /// How many walks have been resolved from handles.
    pub(crate) walks_resolved: Cell<usize>,
}

impl SyntheticSearch {
    pub(crate) fn new(graph: Arc<Graph>) -> Self {
        Self {
            graph,
            trace: Vec::new(),
            walks_resolved: Cell::new(0),
        }
    }

    /// The nodes of a walk from its source to where it was reported.
    pub(crate) fn path(&self, parent: TraceId) -> Vec<u32> {
        let mut nodes = Vec::new();
        let mut cursor = Some(parent.0);
        while let Some(index) = cursor {
            let entry = self.trace[index as usize];
            nodes.push(entry.node);
            cursor = entry.previous;
        }
        nodes.reverse();
        nodes
    }
}

impl TransferSearch for SyntheticSearch {
    type Parent = TraceId;
    type Walk = Vec<u32>;
    type Error = Infallible;

    fn begin_run(&mut self, _direction: Direction) {
        self.trace.clear();
    }

    fn walk(&self, parent: TraceId) -> Vec<u32> {
        self.walks_resolved.set(self.walks_resolved.get() + 1);
        self.path(parent)
    }

    fn search<K: TransferSink<TraceId>>(
        &mut self,
        request: &TransferRequest<'_>,
        sink: &mut K,
    ) -> Result<TransferOutcome, Infallible> {
        if (request.interrupt)() {
            return Ok(TransferOutcome::Interrupted);
        }
        let node_count = self.graph.edges_out.len();
        let mut slots: Vec<[Option<NodeLabel>; 2]> = vec![[None; 2]; node_count];
        let mut reported = vec![false; self.graph.stop_node.len()];
        let mut queue = BinaryHeap::new();

        let consider = |slots: &mut Vec<[Option<NodeLabel>; 2]>,
                        queue: &mut BinaryHeap<Reverse<(i32, u32, u32)>>,
                        trace: &mut Vec<Trace>,
                        node: u32,
                        clock: i32,
                        source: u32,
                        previous: Option<u32>| {
            let entry = &mut slots[node as usize];
            let new_label = |trace: &mut Vec<Trace>| {
                let id = u32::try_from(trace.len()).expect("trace fits u32");
                trace.push(Trace { node, previous });
                NodeLabel {
                    clock,
                    source,
                    trace: id,
                }
            };
            let replaced = match *entry {
                [Some(first), _] if first.source == source => {
                    if clock < first.clock {
                        entry[0] = Some(new_label(trace));
                        true
                    } else {
                        false
                    }
                }
                [_, Some(second)] if second.source == source => {
                    if clock < second.clock {
                        entry[1] = Some(new_label(trace));
                        if entry[0].is_some_and(|f| f.clock > clock) {
                            entry.swap(0, 1);
                        }
                        true
                    } else {
                        false
                    }
                }
                [Some(first), _] if clock < first.clock => {
                    entry[1] = entry[0];
                    entry[0] = Some(new_label(trace));
                    true
                }
                [Some(_), None] => {
                    entry[1] = Some(new_label(trace));
                    true
                }
                [Some(_), Some(second)] if clock < second.clock => {
                    entry[1] = Some(new_label(trace));
                    true
                }
                [None, _] => {
                    entry[0] = Some(new_label(trace));
                    true
                }
                _ => false,
            };
            if replaced {
                queue.push(Reverse((clock, node, source)));
            }
        };

        for seed in request.seeds {
            let node = self.graph.stop_node[seed.stop.index()];
            consider(
                &mut slots,
                &mut queue,
                &mut self.trace,
                node,
                seed.clock.seconds(),
                seed.stop.into_inner(),
                None,
            );
        }

        while let Some(Reverse((clock, node, source))) = queue.pop() {
            if clock >= request.upper_bound.seconds() {
                break;
            }
            let label = slots[node as usize]
                .iter()
                .flatten()
                .copied()
                .find(|l| l.source == source && l.clock == clock);
            let Some(label) = label else { continue };
            for &stop in &self.graph.node_stops[node as usize] {
                if stop != source && !reported[stop as usize] {
                    reported[stop as usize] = true;
                    sink.reach(TransferArrival {
                        stop: StopId::new(stop),
                        source: StopId::new(source),
                        clock: Clock(clock),
                        parent: TraceId(label.trace),
                    });
                }
            }
            let edges = match request.direction {
                Direction::Forward => &self.graph.edges_out[node as usize],
                Direction::Backward => &self.graph.edges_in[node as usize],
            };
            for &(next, cost) in edges {
                consider(
                    &mut slots,
                    &mut queue,
                    &mut self.trace,
                    next,
                    clock + cost,
                    source,
                    Some(label.trace),
                );
            }
        }
        Ok(TransferOutcome::Completed)
    }
}

/// A route in test form: every trip of every day, resolved.
#[derive(Debug, Clone)]
pub(crate) struct TestRoute {
    pub(crate) stops: Vec<StopId>,
    pub(crate) attributes: RouteAttributes,
    /// By day slot.
    pub(crate) days: Vec<Vec<ResolvedTrip>>,
}

/// A network with everything needed to check a search against its definition.
#[derive(Debug, Clone)]
pub(crate) struct TestNet {
    pub(crate) min_change: Vec<i32>,
    pub(crate) routes: Vec<TestRoute>,
    pub(crate) graph: Arc<Graph>,
    /// Stop-to-stop walking distances, following edges as directed.
    pub(crate) dist: Vec<Vec<i32>>,
    pub(crate) access: StopTable,
    pub(crate) egress: StopTable,
    pub(crate) snapshot: Arc<Snapshot>,
}

impl TestNet {
    pub(crate) fn query(&self, max_trips: u8, filter: RouteFilter) -> TransitQuery<'_> {
        TransitQuery {
            snapshot: Arc::clone(&self.snapshot),
            access: &self.access,
            egress: &self.egress,
            filter,
            max_trips,
        }
    }

    pub(crate) fn router(&self) -> Router<SyntheticSearch> {
        Router::new(SyntheticSearch::new(Arc::clone(&self.graph)))
    }
}

/// `(trips, time)`: the improving results of a search, in real time.
pub(crate) type Results = Vec<(u8, i32)>;

/// The results of a search, by exhaustively applying the rules.
pub(crate) fn oracle(
    net: &TestNet,
    filter: RouteFilter,
    direction: Direction,
    time: i32,
    max_trips: u8,
) -> Results {
    let back = direction == Direction::Backward;
    let start = if back { -time } else { time };
    let (seeding, target) = if back {
        (&net.egress, &net.access)
    } else {
        (&net.access, &net.egress)
    };
    let n = net.min_change.len();
    let mut trip = vec![INF; n];
    let mut foot = vec![INF; n];
    for s in 0..n {
        if let Some(cost) = seeding.cost(StopId::new(u32::try_from(s).expect("few stops"))) {
            foot[s] = start + cost.as_i32();
        }
    }
    let walk = |u: usize, v: usize| {
        if back { net.dist[v][u] } else { net.dist[u][v] }
    };

    let mut best = INF;
    let mut results = Vec::new();
    for round in 1..=max_trips {
        let board: Vec<i32> = (0..n)
            .map(|s| (trip[s] + net.min_change[s]).min(foot[s]))
            .collect();
        let mut improved_trip = trip.clone();
        for route in &net.routes {
            if !filter.allows(route.attributes) {
                continue;
            }
            for day in &route.days {
                for ridden in day {
                    let order: Vec<usize> = if back {
                        (0..route.stops.len()).rev().collect()
                    } else {
                        (0..route.stops.len()).collect()
                    };
                    for (i, &from) in order.iter().enumerate() {
                        let event = ridden.events[from];
                        let (can_board, board_clock) = if back {
                            (event.rules.drop_off, -event.arrival.into_inner())
                        } else {
                            (event.rules.pickup, event.departure.into_inner())
                        };
                        let from_stop = route.stops[from].index();
                        if !can_board || board_clock < board[from_stop] {
                            continue;
                        }
                        for &to in &order[i + 1..] {
                            let event = ridden.events[to];
                            let (can_alight, alight_clock) = if back {
                                (event.rules.pickup, -event.departure.into_inner())
                            } else {
                                (event.rules.drop_off, event.arrival.into_inner())
                            };
                            if can_alight {
                                let to_stop = route.stops[to].index();
                                improved_trip[to_stop] = improved_trip[to_stop].min(alight_clock);
                            }
                        }
                    }
                }
            }
        }
        trip = improved_trip;
        for u in 0..n {
            if trip[u] >= INF {
                continue;
            }
            for v in 0..n {
                if u != v && walk(u, v) < NO_WALK {
                    foot[v] = foot[v].min(trip[u] + walk(u, v));
                }
            }
        }
        let mut reached = INF;
        for s in 0..n {
            let id = StopId::new(u32::try_from(s).expect("few stops"));
            if trip[s] < INF
                && let Some(cost) = target.cost(id)
            {
                reached = reached.min(trip[s] + cost.as_i32());
            }
        }
        if reached < best {
            best = reached;
            results.push((round, if back { -reached } else { reached }));
        }
    }
    results
}

/// Checks a journey against the network's definition, leg by leg.
pub(crate) fn check_journey(net: &TestNet, journey: &Journey<Vec<u32>>) {
    let mut legs = journey.legs.iter();
    let Some(Leg::Access {
        stop: access_stop,
        duration: access,
    }) = legs.next()
    else {
        panic!("a journey starts with access: {journey:?}");
    };
    assert_eq!(net.access.cost(*access_stop), Some(*access));

    let mut position_stop = *access_stop;
    let mut previous_alight: Option<i32> = None;
    let mut walked = 0;
    let mut foot = true;
    let mut rides = 0_u8;
    let mut first_board = None;
    let mut last_alight = None;
    let mut egress = None;
    for leg in legs {
        match leg {
            Leg::Ride(ride) => {
                rides += 1;
                assert_eq!(ride.board_stop, position_stop, "ride starts where we are");
                let route = &net.routes[ride.route.index()];
                let (day_index, trip) = route
                    .days
                    .iter()
                    .enumerate()
                    .find_map(|(day, trips)| {
                        (net.snapshot.days()[day].date == ride.trip.date)
                            .then(|| {
                                trips
                                    .iter()
                                    .find(|t| t.trip == ride.trip.trip)
                                    .map(|t| (day, t))
                            })
                            .flatten()
                    })
                    .expect("the ridden trip exists");
                assert_eq!(
                    net.snapshot.lane_day(ride.lane).map(DaySlot::index),
                    Some(day_index)
                );
                let board = trip.events[ride.board_position as usize];
                let alight = trip.events[ride.alight_position as usize];
                assert!(ride.board_position < ride.alight_position);
                assert_eq!(route.stops[ride.board_position as usize], ride.board_stop);
                assert_eq!(route.stops[ride.alight_position as usize], ride.alight_stop);
                assert!(board.rules.pickup, "boarded where pickup is forbidden");
                assert!(
                    alight.rules.drop_off,
                    "alighted where drop-off is forbidden"
                );
                assert_eq!(board.departure, ride.board_time);
                assert_eq!(alight.arrival, ride.alight_time);
                let board_time = ride.board_time.into_inner();
                if let Some(arrived) = previous_alight {
                    if foot {
                        assert!(arrived + walked <= board_time, "walked too slowly");
                    } else {
                        assert!(
                            arrived + net.min_change[ride.board_stop.index()] <= board_time,
                            "change time ignored"
                        );
                    }
                } else {
                    first_board = Some(board_time);
                }
                previous_alight = Some(ride.alight_time.into_inner());
                last_alight = Some(ride.alight_time.into_inner());
                position_stop = ride.alight_stop;
                foot = false;
                walked = 0;
            }
            Leg::Transfer {
                from,
                to,
                duration,
                walk,
            } => {
                assert_ne!(from, to, "walks join different stops");
                assert_eq!(*from, position_stop);
                assert_eq!(
                    net.dist[from.index()][to.index()],
                    duration.as_i32(),
                    "walk costs the shortest path"
                );
                let first = *walk.first().expect("non-empty path");
                let last = *walk.last().expect("non-empty path");
                let (from_node, to_node) = (
                    net.graph.stop_node[from.index()],
                    net.graph.stop_node[to.index()],
                );
                assert!(
                    (first == from_node && last == to_node)
                        || (first == to_node && last == from_node),
                    "the parent chain joins the walk's ends"
                );
                position_stop = *to;
                walked = duration.as_i32();
                foot = true;
            }
            Leg::Egress { stop, duration } => {
                assert_eq!(*stop, position_stop, "egress leaves from where we are");
                assert!(!foot, "the last leg before egress is a ride");
                assert_eq!(net.egress.cost(*stop), Some(*duration));
                egress = Some(duration.as_i32());
            }
            Leg::Access { .. } => panic!("access only comes first: {journey:?}"),
        }
    }
    assert_eq!(rides, journey.trips);
    assert!(journey.trips >= 1, "a journey uses at least one trip");
    assert_eq!(
        first_board.expect("has a ride") - access.as_i32(),
        journey.departure.into_inner()
    );
    assert_eq!(
        last_alight.expect("has a ride") + egress.expect("has egress"),
        journey.arrival.into_inner()
    );
}

/// Small named networks for scenario tests.
#[derive(Debug, Default)]
pub(crate) struct Scenario {
    min_change: Vec<u32>,
    routes: Vec<(Vec<u32>, RouteAttributes, Vec<ScenarioTrip>)>,
    edges: Vec<(u32, u32, i32)>,
    access: Vec<(u32, u32)>,
    egress: Vec<(u32, u32)>,
    days: Vec<(ServiceDate, i32)>,
}

/// A scenario trip: times per position, rules per position, and the day slots it runs on.
#[derive(Debug, Clone)]
pub(crate) struct ScenarioTrip {
    pub(crate) id: u32,
    pub(crate) times: Vec<(i32, i32)>,
    pub(crate) rules: Vec<StopRules>,
    pub(crate) day_slots: Vec<u8>,
}

impl ScenarioTrip {
    /// A trip with regular rules, arriving and departing at the same second.
    pub(crate) fn simple(id: u32, times: &[i32]) -> Self {
        Self {
            id,
            times: times.iter().map(|&time| (time, time)).collect(),
            rules: vec![StopRules::REGULAR; times.len()],
            day_slots: vec![0],
        }
    }
}

impl Scenario {
    /// A scenario of `stops` stops on separate graph nodes, with no change time.
    pub(crate) fn new(stops: usize) -> Self {
        Self {
            min_change: vec![0; stops],
            days: vec![(ServiceDate::new(1), 0)],
            ..Self::default()
        }
    }

    pub(crate) fn days(mut self, days: &[(u32, i32)]) -> Self {
        self.days = days
            .iter()
            .map(|&(d, o)| (ServiceDate::new(d), o))
            .collect();
        self
    }

    pub(crate) fn min_change(mut self, stop: u32, seconds: u32) -> Self {
        self.min_change[stop as usize] = seconds;
        self
    }

    pub(crate) fn route(mut self, stops: &[u32], trips: &[ScenarioTrip]) -> Self {
        self.routes
            .push((stops.to_vec(), RouteAttributes::NONE, trips.to_vec()));
        self
    }

    pub(crate) fn route_with(
        mut self,
        stops: &[u32],
        attributes: RouteAttributes,
        trips: &[ScenarioTrip],
    ) -> Self {
        self.routes
            .push((stops.to_vec(), attributes, trips.to_vec()));
        self
    }

    pub(crate) fn walk(mut self, from: u32, to: u32, seconds: i32) -> Self {
        self.edges.push((from, to, seconds));
        self
    }

    pub(crate) fn access(mut self, stop: u32, seconds: u32) -> Self {
        self.access.push((stop, seconds));
        self
    }

    pub(crate) fn egress(mut self, stop: u32, seconds: u32) -> Self {
        self.egress.push((stop, seconds));
        self
    }

    /// Builds the scenario's network.
    ///
    /// Stops sit on their own graph nodes (the node number is the scenario's stop number),
    /// and the returned mapping gives each scenario stop's final number.
    pub(crate) fn build(self) -> (TestNet, Vec<StopId>) {
        let mut builder = TimetableBuilder::new();
        let keys: Vec<_> = self
            .min_change
            .iter()
            .map(|&m| builder.add_stop(secs(m)).expect("stop"))
            .collect();
        let services: Vec<_> = self
            .days
            .iter()
            .map(|&(date, _)| builder.add_service([date]).expect("service"))
            .collect();
        for (stops, attributes, trips) in &self.routes {
            let stop_keys: Vec<_> = stops.iter().map(|&s| keys[s as usize]).collect();
            let pattern = builder
                .add_pattern(&stop_keys, *attributes)
                .expect("pattern");
            for trip in trips {
                for &day in &trip.day_slots {
                    let times = trip
                        .times
                        .iter()
                        .zip(&trip.rules)
                        .map(|(&(arrival, departure), &rules)| StopTime {
                            arrival: t(arrival),
                            departure: t(departure),
                            rules,
                        })
                        .collect();
                    builder
                        .add_trip(
                            pattern,
                            crate::ids::TripId::new(trip.id),
                            services[usize::from(day)],
                            times,
                        )
                        .expect("trip");
                }
            }
        }
        let timetable = builder.build().expect("build");
        let ids: Vec<StopId> = keys.iter().map(|&k| timetable.stop_id(k)).collect();
        let days: Vec<ServiceDay> = self
            .days
            .iter()
            .map(|&(date, offset)| ServiceDay {
                date,
                offset: t(offset),
            })
            .collect();
        let window = Arc::new(timetable.materialize(&days).expect("materialize"));
        let snapshot = Snapshot::schedule(Arc::clone(&window));

        let stop_count = self.min_change.len();
        let mut stop_node = vec![0_u32; stop_count];
        for (scenario_stop, id) in ids.iter().enumerate() {
            stop_node[id.index()] = u32::try_from(scenario_stop).expect("few");
        }
        let graph = Arc::new(Graph::new(stop_count, &self.edges, stop_node));
        let mut access = StopTable::new(stop_count);
        for &(stop, cost) in &self.access {
            access
                .set(ids[stop as usize], secs(cost))
                .expect("in range");
        }
        let mut egress = StopTable::new(stop_count);
        for &(stop, cost) in &self.egress {
            egress
                .set(ids[stop as usize], secs(cost))
                .expect("in range");
        }
        let mut min_change = vec![0; stop_count];
        for (scenario_stop, id) in ids.iter().enumerate() {
            min_change[id.index()] = i32::try_from(self.min_change[scenario_stop]).expect("small");
        }
        let routes = routes_from_timetable(&timetable, &window);
        let dist = graph.distances();
        (
            TestNet {
                min_change,
                routes,
                graph,
                dist,
                access,
                egress,
                snapshot,
            },
            ids,
        )
    }
}

/// Every route of a timetable, resolved for every day of a window.
pub(crate) fn routes_from_timetable(timetable: &Timetable, window: &BaseWindow) -> Vec<TestRoute> {
    (0..timetable.route_count())
        .map(|r| {
            let route = RouteId::new(u32::try_from(r).expect("few routes"));
            TestRoute {
                stops: timetable.route_stops(route).expect("route").to_vec(),
                attributes: window.attributes[r],
                days: window
                    .days()
                    .iter()
                    .map(|day| {
                        timetable
                            .resolved_trips(route, day.date, day.offset)
                            .expect("resolved")
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Sorted `(departure, arrival, trips)` triples.
pub(crate) type Triples = Vec<(i32, i32, u8)>;

/// Reduces triples to Pareto-optimal ones on later departure, earlier arrival, and fewer trips.
pub(crate) fn pareto(mut entries: Triples) -> Triples {
    entries.sort_unstable();
    entries.dedup();
    let reduced: Triples = entries
        .iter()
        .copied()
        .filter(|&(d, a, k)| {
            !entries
                .iter()
                .any(|&(od, oa, ok)| (od, oa, ok) != (d, a, k) && od >= d && oa <= a && ok <= k)
        })
        .collect();
    reduced
}

/// The oracle's forward results at every second from `earliest` to one past `latest`.
pub(crate) fn oracle_every_second(
    net: &TestNet,
    filter: RouteFilter,
    earliest: i32,
    latest: i32,
    max_trips: u8,
) -> Vec<Results> {
    (earliest..=latest + 1)
        .map(|departure| oracle(net, filter, Direction::Forward, departure, max_trips))
        .collect()
}

/// The profile by brute force, from a forward search at every second of the window.
///
/// A journey departing later than a given second is reported only by the later search,
/// so a second's result counts only if the next second's search can't also make it.
/// The run one past the window decides this for the window's last second.
pub(crate) fn brute_force_profile(runs: &[Results], earliest: i32) -> Triples {
    let arrival_with = |results: &Results, k: u8| {
        results
            .iter()
            .filter(|&&(trips, _)| trips <= k)
            .map(|&(_, a)| a)
            .min()
    };
    let mut entries = Vec::new();
    for (offset, results) in runs[..runs.len() - 1].iter().enumerate() {
        let departure = earliest + i32::try_from(offset).expect("small window");
        let next = &runs[offset + 1];
        for &(trips, arrival) in results {
            if arrival_with(next, trips) != Some(arrival) {
                entries.push((departure, arrival, trips));
            }
        }
    }
    pareto(entries)
}

/// The day slot of a route's trip list that `slot` names, as a [`DaySlot`].
pub(crate) fn day_slot(slot: u8) -> DaySlot {
    DaySlot::new(slot)
}

/// Lays out a block of trips.
pub(crate) fn block_of(trips: Vec<ResolvedTrip>) -> ResolvedBlock {
    ResolvedBlock::from_trips(trips).expect("valid trips")
}
