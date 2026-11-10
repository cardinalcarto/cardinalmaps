//! Regression tests from the independent review: each reproduces, or rules out, one finding.

use std::cell::Cell;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::{
    DirectoryGraphTileProvider, GraphTileProvider, TarballTileProvider,
};
use valinor_sif::{
    AnyCosting, Cost, Costing, CostingError, DisallowMask, DynamicCost, EdgeAccess, EdgeCostResult,
    InternalTurn, Label, TimeContext,
};
use valinor_transit::{
    Clock, Direction, StopId, TransferArrival, TransferOutcome, TransferRequest, TransferSearch,
    TransferSeed, TransferSink, WalkRules,
};

use super::*;
use crate::snap::{SnapPoint, SnapQuality, SnappedStop, StopSnaps};
use crate::table::{AccessSide, StopTables, TableRequest, TableSeed};

const TILES: [u64; 5] = [762_485, 762_486, 763_925, 763_926, 763_927];
const WA_TILES: &str = "/mnt/maps/cardinal/routing-elevation-i2b/washington/tiles.tar";
const KCM_ZIP: &str = "/mnt/maps/cardinal/transit/kcm/static/kcm-FAL26-161.2.zip";

fn andorra() -> DirectoryGraphTileProvider {
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles");
    DirectoryGraphTileProvider::new(tiles, NonZeroUsize::new(64).expect("nonzero"))
}

fn ped() -> AnyCosting {
    super::tile_tests::costing_named("pedestrian")
}

#[derive(Default)]
struct Sink {
    best: HashMap<u32, (i32, u32, WalkHandle)>,
}
impl TransferSink<WalkHandle> for Sink {
    fn reach(&mut self, a: TransferArrival<WalkHandle>) {
        let (s, c) = (a.stop.into_inner(), a.clock.seconds());
        if self.best.get(&s).is_none_or(|b| c < b.0) {
            self.best.insert(s, (c, a.source.into_inner(), a.parent));
        }
    }
}

fn seed(stop: u32, clock: i32) -> TransferSeed {
    TransferSeed {
        stop: StopId::new(stop),
        clock: Clock::new(clock),
    }
}

fn run<P: GraphTileProvider, C: Costing>(
    s: &mut WalkSearch<P, C>,
    seeds: &[TransferSeed],
    dir: Direction,
    ub: i32,
) -> HashMap<u32, (i32, u32, WalkHandle)> {
    s.begin_run(dir);
    round(s, seeds, dir, ub)
}

fn round<P: GraphTileProvider, C: Costing>(
    s: &mut WalkSearch<P, C>,
    seeds: &[TransferSeed],
    dir: Direction,
    ub: i32,
) -> HashMap<u32, (i32, u32, WalkHandle)> {
    let mut sink = Sink::default();
    let req = TransferRequest {
        direction: dir,
        seeds,
        upper_bound: Clock::new(ub),
        change_margin: TransferRequest::REPORT_ALL,
        rules: WalkRules::none(),
        interrupt: &|| false,
    };
    assert_eq!(
        s.search(&req, &mut sink).expect("search"),
        TransferOutcome::Completed
    );
    sink.best
}

fn point(edge: GraphId, percent_along: f32) -> SnapPoint {
    SnapPoint {
        edge,
        percent_along,
        distance_m: 0.0,
        point: (0.0, 0.0),
    }
}

pub(super) fn snaps_of<P: GraphTileProvider>(provider: &P, points: &[SnapPoint]) -> Arc<StopSnaps> {
    let build = provider
        .with_tile_containing(points[0].edge, |t| t.header().build_id())
        .expect("tile");
    let mut snaps = StopSnaps::new(build, points.len());
    for (i, &p) in points.iter().enumerate() {
        snaps
            .insert(
                StopId::new(i as u32),
                SnappedStop {
                    result: Ok(p),
                    quality: SnapQuality::Connected,
                    build_id: Some(build),
                },
            )
            .expect("insert");
    }
    Arc::new(snaps)
}

/// A costing wrapper with fault and barrier injection.
#[derive(Clone)]
struct Wrapped {
    inner: AnyCosting,
    /// A node's `edge_index`, made into a barrier.
    barrier: Option<u32>,
    /// An edge denied (made one-way against it).
    deny: Option<GraphId>,
    /// Fail the n-th `edge_cost` call from now, when set.
    fail_in: Rc<Cell<i64>>,
}

impl Wrapped {
    fn new(inner: AnyCosting) -> Self {
        Self {
            inner,
            barrier: None,
            deny: None,
            fail_in: Rc::new(Cell::new(-1)),
        }
    }
}

impl Costing for Wrapped {
    fn base(&self) -> &DynamicCost {
        self.inner.base()
    }
    fn base_mut(&mut self) -> &mut DynamicCost {
        self.inner.base_mut()
    }
    fn allowed<T: GraphTile, L: Label>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        pred: &L,
        time: TimeContext,
        access: &mut EdgeAccess,
    ) -> bool {
        self.deny != Some(edge_id) && self.inner.allowed(edge, edge_id, tile, pred, time, access)
    }
    fn allowed_reverse<T: GraphTile, L: Label>(
        &self,
        edge: &DirectedEdge,
        pred: &L,
        opp_edge: &DirectedEdge,
        opp_edge_id: GraphId,
        tile: &T,
        time: TimeContext,
        access: &mut EdgeAccess,
    ) -> bool {
        self.deny != Some(opp_edge_id)
            && self
                .inner
                .allowed_reverse(edge, pred, opp_edge, opp_edge_id, tile, time, access)
    }
    fn edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        s: Option<u32>,
    ) -> Result<EdgeCostResult, CostingError> {
        let left = self.fail_in.get();
        if left >= 0 {
            self.fail_in.set(left - 1);
            if left == 0 {
                return Err(CostingError::MissingEdge(edge_id));
            }
        }
        self.inner.edge_cost(edge, edge_id, tile, s)
    }
    fn transition_cost<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        edge: &DirectedEdge,
        tile: &T,
        node: &NodeInfo,
        pred: &L,
        provider: &P,
        d: bool,
    ) -> Result<Cost, CostingError> {
        self.inner
            .transition_cost(edge, tile, node, pred, provider, d)
    }
    fn transition_cost_reverse<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        idx: u8,
        node: &NodeInfo,
        opp_edge: &DirectedEdge,
        opp_pred_edge: &DirectedEdge,
        pred: &L,
        tile: &T,
        provider: &P,
        m: bool,
        i: InternalTurn,
        d: bool,
    ) -> Result<Cost, CostingError> {
        self.inner.transition_cost_reverse(
            idx,
            node,
            opp_edge,
            opp_pred_edge,
            pred,
            tile,
            provider,
            m,
            i,
            d,
        )
    }
    fn edge_filter<T: GraphTile>(&self, e: &DirectedEdge, t: &T, d: DisallowMask) -> bool {
        self.inner.edge_filter(e, t, d)
    }
    fn node_allowed(&self, node: &NodeInfo) -> bool {
        self.barrier != Some(node.edge_index()) && self.inner.node_allowed(node)
    }
    fn astar_cost_factor(&self) -> f32 {
        self.inner.astar_cost_factor()
    }
    fn partial_edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        f: Option<GraphId>,
        tile: &T,
        s: Option<u32>,
        a: f32,
        b: f32,
    ) -> Result<EdgeCostResult, CostingError> {
        self.inner
            .partial_edge_cost(edge, edge_id, f, tile, s, a, b)
    }
}

/// Finds a node in a tile with two distinct walkable outbound edges, each both ways, in-tile.
fn node_with_two_edges<P: GraphTileProvider>(_p: &P) -> (u32, GraphId, GraphId) {
    let base = GraphId::try_from_components(2, TILES[2], 0).expect("id");
    let mut graph: crate::graph::LazyGraph<_, AnyCosting> =
        crate::graph::LazyGraph::new(andorra(), ped());
    let tile = graph.tile(base).expect("t").expect("present");
    for node in tile.nodes() {
        if !graph.costing.node_allowed(node) {
            continue;
        }
        let first = node.edge_index();
        let outs = tile.get_outbound_edges_from_node(node);
        let mut chosen = Vec::new();
        for (i, e) in outs.iter().enumerate() {
            if e.is_shortcut() || e.length() < 40 {
                continue;
            }
            let id = base
                .with_feature_index(u64::from(first) + i as u64)
                .expect("id");
            let Ok(Some(d)) = graph.describe_edge(id) else {
                continue;
            };
            if d.weights.forward != INF && d.weights.backward != INF && d.tail != d.head {
                chosen.push(id);
            }
        }
        if chosen.len() >= 2 {
            return (first, chosen[0], chosen[1]);
        }
    }
    panic!("no node found");
}

/// An offline area holds only part of the network: a snap whose tile isn't on the device acts
/// as an unsnapped stop, and every other stop works.
#[test]
fn a_snap_in_a_tile_not_on_the_device_acts_as_unsnapped() {
    let provider = andorra();
    let (_, e1, e2) = node_with_two_edges(&provider);
    let elsewhere = GraphId::try_from_components(2, 100, 0).expect("valid");
    let build = provider
        .with_tile_containing(e1, |t| t.header().build_id())
        .expect("tile");
    let mut snaps = StopSnaps::new(build, 3);
    for (i, edge) in [e1, elsewhere, e2].into_iter().enumerate() {
        snaps
            .insert(
                StopId::new(i as u32),
                SnappedStop {
                    result: Ok(point(edge, 0.5)),
                    quality: SnapQuality::Connected,
                    build_id: Some(build),
                },
            )
            .expect("insert");
    }
    let snaps = Arc::new(snaps);

    let mut tables = StopTables::new(andorra(), ped());
    tables
        .bind(Arc::clone(&snaps))
        .expect("binds with a missing tile");
    let seeds = tables.stop_seeds(StopId::new(0)).expect("seeds");
    assert!(
        tables
            .search(&TableRequest {
                side: AccessSide::Access,
                seeds: &seeds,
                max_seconds: 60,
                interrupt: &|| false,
            })
            .expect("searches")
    );
    assert!(
        tables
            .reached_stops()
            .all(|(stop, _)| stop != StopId::new(1))
    );

    let mut search = WalkSearch::new(provider, ped());
    search.bind(snaps).expect("binds with a missing tile");
}

/// A barrier node stops walks from passing through it, in either direction.
#[test]
fn barrier_is_not_walked_through_into_a_stop_on_the_far_side() {
    let provider = andorra();
    let (edge_index, e1, e2) = node_with_two_edges(&provider);
    let snaps = snaps_of(&provider, &[point(e1, 0.5), point(e2, 0.5)]);
    let mut costing = Wrapped::new(ped());
    costing.barrier = Some(edge_index);
    let mut search = WalkSearch::new(provider, costing);
    search.bind(snaps).expect("bind");
    let found = run(&mut search, &[seed(0, 0)], Direction::Forward, i32::MAX / 2);
    if let Some(&(clock, source, handle)) = found.get(&1) {
        let walk = search.walk(handle);
        // A walk of the two connectors through the barrier has exactly two edges.
        assert!(
            walk.edges.len() != 2,
            "walked through the barrier in {clock}s from {source}: {walk:?}"
        );
    }
}

#[test]
fn the_queue_is_usable_after_a_search_error() {
    let provider = andorra();
    let points = super::tile_tests::random_stops(&provider, &TILES, 50, 3);
    let snaps = snaps_of(&provider, &points);
    let costing = Wrapped::new(ped());
    let fail_in = costing.fail_in.clone();
    let mut search = WalkSearch::new(provider, costing);
    search.bind(snaps).expect("bind");
    let seeds: Vec<_> = (0..20).map(|i| seed(i, i as i32 * 3)).collect();
    fail_in.set(400);
    search.begin_run(Direction::Forward);
    let mut sink = Sink::default();
    let req = TransferRequest {
        direction: Direction::Forward,
        seeds: &seeds,
        upper_bound: Clock::new(i32::MAX / 2),
        change_margin: TransferRequest::REPORT_ALL,
        rules: WalkRules::none(),
        interrupt: &|| false,
    };
    let result = search.search(&req, &mut sink);
    assert!(result.is_err(), "the fault wasn't reached");
    // A costing failure is a real outcome (corrupt tile); the worker is reused afterwards.
    fail_in.set(-1);
    let found = run(&mut search, &seeds, Direction::Forward, i32::MAX / 2);
    let mut fresh = WalkSearch::new(andorra(), Wrapped::new(ped()));
    fresh.bind(snaps_of(&andorra(), &points)).expect("bind");
    let expected = run(&mut fresh, &seeds, Direction::Forward, i32::MAX / 2);
    let c = |m: &HashMap<u32, (i32, u32, WalkHandle)>| {
        let mut v: Vec<_> = m.iter().map(|(k, v)| (*k, v.0)).collect();
        v.sort();
        v
    };
    assert_eq!(c(&found), c(&expected));
}

#[test]
fn a_failed_bind_does_not_leave_the_previous_snaps_half_replaced() {
    let provider = andorra();
    let points = super::tile_tests::random_stops(&provider, &TILES, 40, 5);
    let good = snaps_of(&provider, &points);
    let build = good.build_id();
    let mut bad = StopSnaps::new(
        valhalla_graphtile::graph_tile::BuildId::new(build.value() ^ 1),
        40,
    );
    for (i, &p) in points.iter().enumerate() {
        bad.insert(
            StopId::new(i as u32),
            SnappedStop {
                result: Ok(p),
                quality: SnapQuality::Connected,
                build_id: None,
            },
        )
        .expect("insert");
    }
    let mut search = WalkSearch::new(provider, ped());
    search.bind(good.clone()).expect("bind");
    let before = run(
        &mut search,
        &[seed(0, 0), seed(1, 5)],
        Direction::Forward,
        i32::MAX / 2,
    );
    assert!(!before.is_empty());
    assert!(search.bind(Arc::new(bad)).is_err());
    // The worker retries with the snaps it had: `ptr_eq` short-circuits.
    search.bind(good).expect("rebind");
    let after = run(
        &mut search,
        &[seed(0, 0), seed(1, 5)],
        Direction::Forward,
        i32::MAX / 2,
    );
    assert_eq!(
        before.len(),
        after.len(),
        "a failed bind wiped the attachments"
    );
}

#[test]
fn two_stops_at_the_same_point_have_a_walk_of_no_edges() {
    let provider = andorra();
    let (_, e1, _) = node_with_two_edges(&provider);
    let snaps = snaps_of(&provider, &[point(e1, 0.5), point(e1, 0.5)]);
    let mut search = WalkSearch::new(provider, ped());
    search.bind(snaps).expect("bind");
    let found = run(
        &mut search,
        &[seed(0, 100)],
        Direction::Forward,
        i32::MAX / 2,
    );
    let (clock, _, handle) = found[&1];
    assert_eq!(clock, 100, "co-located stops cost nothing");
    let walk = search.walk(handle);
    assert!(
        walk.edges.is_empty(),
        "a zero-length walk carries an edge: {walk:?}"
    );
}

/// Co-located stops on the disallowed direction of a one-way: `<=` picks `weights[0]`.
#[test]
fn zero_length_walk_on_a_one_way_against_its_direction() {
    assert_eq!(direct_cost([INF, 50], 0.5, 0.5), 0);
}

/// An access table must cover a stop within the limit even if the end of its edge is beyond it.
#[test]
fn a_stop_within_the_limit_on_an_edge_ending_beyond_it_is_kept() {
    let provider = andorra();
    let mut points = super::tile_tests::random_stops(&provider, &TILES, 300, 21);
    points[0] = crate::snap_stop(
        &provider,
        &ped(),
        &crate::SnapConfig::default(),
        (1.5218, 42.5063),
    )
    .expect("snap")
    .result
    .expect("snap");
    let snaps = snaps_of(&provider, &points);
    let mut tables = StopTables::new(provider, ped());
    tables.bind(snaps.clone()).expect("bind");
    let seeds = [TableSeed {
        edge: points[0].edge,
        percent_along: points[0].percent_along,
        initial: 0.0,
    }];
    let full = tables
        .compute(
            snaps.stop_count(),
            &TableRequest {
                side: AccessSide::Access,
                seeds: &seeds,
                max_seconds: 7200,
                interrupt: &|| false,
            },
        )
        .expect("t")
        .expect("done");
    let mut lost = Vec::new();
    for i in 1..snaps.stop_count() {
        let stop = StopId::new(i as u32);
        if full.cost(stop).is_none() {
            continue;
        }
        let leg = tables.leg(stop).expect("leg");
        // Whole seconds left on the last edge past the stop: the probe needs a real gap.
        let last = leg.edges.last().expect("edges");
        if leg.end_percent >= 0.8 || last.traversal.secs < 1.0 || leg.edges.len() < 2 {
            continue;
        }
        let limit = leg.secs.ceil() as u32;
        // A route too costly for this limit's cost bound is rightly left out.
        let cost: f32 = leg
            .edges
            .iter()
            .map(|e| e.transition.cost + e.traversal.cost)
            .sum();
        if cost > limit as f32 * crate::table::WALKING_COST_FACTOR {
            continue;
        }
        let table = tables
            .compute(
                snaps.stop_count(),
                &TableRequest {
                    side: AccessSide::Access,
                    seeds: &seeds,
                    max_seconds: limit,
                    interrupt: &|| false,
                },
            )
            .expect("t")
            .expect("done");
        if table.cost(stop).is_none() {
            lost.push((i, leg.secs, limit, leg.end_percent));
        }
        let _ = tables
            .compute(
                snaps.stop_count(),
                &TableRequest {
                    side: AccessSide::Access,
                    seeds: &seeds,
                    max_seconds: 7200,
                    interrupt: &|| false,
                },
            )
            .expect("t");
    }
    assert!(
        lost.is_empty(),
        "lost {} stops, e.g. {:?}",
        lost.len(),
        &lost[..lost.len().min(5)]
    );
}

/// A stop at a node may leave through that node, even if its edge is one-way the other way.
#[test]
fn a_stop_at_a_node_leaves_through_it_when_its_edge_is_one_way() {
    let provider = andorra();
    let (edge_index, e1, e2) = node_with_two_edges(&provider);
    let _ = edge_index;
    // e1 leaves the node; a stop at its very start, on a walk one-way along e1.
    let mut graph: crate::graph::LazyGraph<_, AnyCosting> =
        crate::graph::LazyGraph::new(andorra(), ped());
    let d1 = graph.describe_edge(e1).expect("e").expect("present");
    let opp = graph.edge_id(d1.opposing_dense).expect("opp");
    let snaps = snaps_of(&provider, &[point(e1, 0.0), point(e2, 0.5)]);
    let mut costing = Wrapped::new(ped());
    costing.deny = Some(opp);
    let mut search = WalkSearch::new(provider, costing);
    search.bind(snaps).expect("bind");
    let found = run(&mut search, &[seed(0, 0)], Direction::Forward, i32::MAX / 2);
    let attach = search.stops[1].expect("stop");
    let (clock, ..) = found[&1];
    assert_eq!(
        clock,
        i32::from(attach.into[0]),
        "the stop can't leave its own node"
    );
}

#[test]
fn egress_legs_put_each_transition_on_the_edge_it_enters() {
    let provider = andorra();
    let mut points = super::tile_tests::random_stops(&provider, &TILES, 300, 21);
    points[0] = crate::snap_stop(
        &provider,
        &ped(),
        &crate::SnapConfig::default(),
        (1.5218, 42.5063),
    )
    .expect("snap")
    .result
    .expect("snap");
    let snaps = snaps_of(&provider, &points);
    let mut tables = StopTables::new(provider, ped());
    tables.bind(snaps.clone()).expect("bind");
    let seeds = [TableSeed {
        edge: points[0].edge,
        percent_along: points[0].percent_along,
        initial: 0.0,
    }];
    tables
        .compute(
            snaps.stop_count(),
            &TableRequest {
                side: AccessSide::Egress,
                seeds: &seeds,
                max_seconds: 3600,
                interrupt: &|| false,
            },
        )
        .expect("t");
    let mut nonzero_first = 0;
    let mut seen = 0;
    for i in 0..snaps.stop_count() {
        let Some(leg) = tables.leg(StopId::new(i as u32)) else {
            continue;
        };
        seen += 1;
        if leg.edges[0].transition.cost != 0.0 {
            nonzero_first += 1;
        }
    }
    assert!(seen > 0);
    assert_eq!(
        nonzero_first, 0,
        "transitions on the first edge of {nonzero_first}/{seen} legs"
    );
}

/// Randomized rounds of a run, against an independent baseline with no pruning.
#[test]
fn run_pruning_matches_a_running_minimum_over_random_rounds() {
    let mut x = 0xdead_beef_cafe_f00du64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for trial in 0..60 {
        let dir = if trial % 2 == 0 {
            Direction::Forward
        } else {
            Direction::Backward
        };
        let provider = andorra();
        let stops = 120;
        let points = super::tile_tests::random_stops(&provider, &TILES, stops, 100 + trial);
        let snaps = snaps_of(&provider, &points);
        let mut pruned = WalkSearch::new(andorra(), ped());
        let mut plain = WalkSearch::new(andorra(), ped());
        pruned.bind(snaps.clone()).expect("bind");
        plain.bind(snaps).expect("bind");
        plain.set_run_pruning(false);
        pruned.begin_run(dir);
        plain.begin_run(dir);
        let (mut best_p, mut best_u) = (HashMap::<u32, i32>::new(), HashMap::<u32, i32>::new());
        // A non-increasing bound, as `best_target` is: the core never raises it.
        let mut ub = i32::MAX / 2;
        for r in 0..8 {
            let count = (next() % 25 + 1) as usize;
            let mut ids: Vec<u32> = Vec::new();
            for _ in 0..count {
                let s = (next() % stops as u64) as u32;
                if !ids.contains(&s) {
                    ids.push(s);
                }
            }
            let seeds: Vec<_> = ids
                .iter()
                .map(|&s| {
                    let c = (next() % 2400) as i32 - if r > 3 { 2000 } else { 0 };
                    seed(s, c)
                })
                .collect();
            if r >= 2 && next() % 2 == 0 {
                ub = ub.min((next() % 1200) as i32 + 1000);
            }
            for (out, s) in [(&mut best_p, &mut pruned), (&mut best_u, &mut plain)] {
                for (stop, (c, ..)) in round(s, &seeds, dir, ub) {
                    if c < ub {
                        let e = out.entry(stop).or_insert(c);
                        *e = (*e).min(c);
                    }
                }
            }
            // Only arrivals under the current bound matter.
            let live = |m: &HashMap<u32, i32>| {
                let mut v: Vec<(u32, i32)> = m
                    .iter()
                    .map(|(k, c)| (*k, *c))
                    .filter(|e| e.1 < ub)
                    .collect();
                v.sort();
                v
            };
            assert_eq!(
                live(&best_p),
                live(&best_u),
                "trial {trial} round {r} {dir:?}"
            );
        }
    }
}

/// The carry-over between rounds assumes the bound never rises; the trait doesn't promise that.
#[test]
fn run_pruning_with_a_rising_bound_loses_arrivals() {
    let provider = andorra();
    let points = super::tile_tests::random_stops(&provider, &TILES, 120, 9);
    let snaps = snaps_of(&provider, &points);
    let mut pruned = WalkSearch::new(andorra(), ped());
    let mut plain = WalkSearch::new(andorra(), ped());
    pruned.bind(snaps.clone()).expect("bind");
    plain.bind(snaps).expect("bind");
    plain.set_run_pruning(false);
    pruned.begin_run(Direction::Forward);
    plain.begin_run(Direction::Forward);
    let dir = Direction::Forward;
    let seeds = [seed(0, 0), seed(1, 5)];
    round(&mut pruned, &seeds, dir, 200);
    round(&mut plain, &seeds, dir, 200);
    let a = round(&mut pruned, &seeds, dir, i32::MAX / 2);
    let b = round(&mut plain, &seeds, dir, i32::MAX / 2);
    // Documented on `TransferRequest::upper_bound`: a raised bound would lose arrivals.
    assert!(a.len() < b.len(), "pruned {} vs plain {}", a.len(), b.len());
}

// ---- Real data.

fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(core::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    fields.push(field);
    fields
}

fn kcm_stops() -> Vec<(f64, f64)> {
    let output = std::process::Command::new("unzip")
        .args(["-p", KCM_ZIP, "stops.txt"])
        .output()
        .expect("unzip");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let header = csv_fields(lines.next().expect("header").trim_start_matches('\u{feff}'));
    let field = |name: &str| header.iter().position(|h| h == name).expect(name);
    let (lat, lon) = (field("stop_lat"), field("stop_lon"));
    let kind = header.iter().position(|h| h == "location_type");
    lines
        .filter_map(|line| {
            let f = csv_fields(line);
            let loc_type = kind.and_then(|k| f.get(k)).map_or("", String::as_str);
            if !matches!(loc_type, "" | "0") {
                return None;
            }
            Some((f[lon].parse().ok()?, f[lat].parse().ok()?))
        })
        .collect()
}

fn tar() -> TarballTileProvider<false, false> {
    TarballTileProvider::new_readonly(WA_TILES).expect("tiles")
}

fn kc_snaps(provider: &TarballTileProvider<false, false>, costing: &AnyCosting) -> Arc<StopSnaps> {
    let stops = kcm_stops();
    let probe =
        crate::snap_stop(provider, costing, &crate::SnapConfig::default(), stops[0]).expect("snap");
    let build = probe.build_id.expect("build");
    Arc::new(
        crate::snap_stops(
            provider,
            costing,
            &crate::SnapConfig::default(),
            build,
            &stops,
        )
        .expect("snap all"),
    )
}

/// The original test, with the raw tarball provider and the same seeds.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn tarball_king_county_forward_backward() {
    let costing = ped();
    let provider = tar();
    let snaps = kc_snaps(&provider, &costing);
    let mut search = WalkSearch::new(provider, costing);
    search.bind(snaps.clone()).expect("bind");
    let stop_count = snaps.stop_count() as u32;
    let (mut checked, mut mismatches, mut missing) = (0u64, 0u64, 0u64);
    let sources = [0u32, 500, 1000, 2000, 3000, 4000, 5000, 6000]
        .into_iter()
        .chain((0..stop_count).step_by(301));
    for (i, u) in sources.enumerate() {
        let u = u % stop_count;
        if search.stops[u as usize].is_none() {
            continue;
        }
        let forward = run(&mut search, &[seed(u, 0)], Direction::Forward, 3600);
        for (&v, &(clock, ..)) in forward.iter().filter(|(v, _)| *v % 13 == i as u32 % 13) {
            let backward = run(&mut search, &[seed(v, 0)], Direction::Backward, 3601);
            checked += 1;
            match backward.get(&u) {
                Some(&(b, ..)) if b == clock => {}
                Some(&(b, ..)) => {
                    mismatches += 1;
                    if mismatches <= 20 {
                        println!("mismatch {u}->{v}: forward {clock}, backward {b}");
                    }
                }
                None => {
                    missing += 1;
                    if missing <= 20 {
                        println!("missing mirror {u}->{v}: forward {clock}");
                    }
                }
            }
        }
    }
    println!("checked {checked}, {mismatches} mismatches, {missing} missing");
    assert_eq!((mismatches, missing), (0, 0));
}

/// Counts the snap situations the review asks about, on real stops.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn king_county_snap_census() {
    let costing = ped();
    let provider = tar();
    let snaps = kc_snaps(&provider, &costing);
    let mut search = WalkSearch::new(provider, costing);
    search.bind(snaps.clone()).expect("bind");
    let (mut at_node, mut one_way, mut both_inf, mut no_attach, mut at_node_one_way) =
        (0, 0, 0, 0, 0);
    let mut frames: HashMap<u32, (u32, u32)> = HashMap::new();
    for i in 0..snaps.stop_count() {
        let Some(a) = search.stops[i] else {
            no_attach += 1;
            continue;
        };
        let w = a.canon_weights;
        let edge_end = a.percent <= 0.0 || a.percent >= 1.0;
        if edge_end {
            at_node += 1;
        }
        if (w[0] == INF) != (w[1] == INF) {
            one_way += 1;
            if edge_end {
                at_node_one_way += 1;
            }
        }
        if w[0] == INF && w[1] == INF {
            both_inf += 1;
        }
        let canon = a.edges[0].min(a.edges[1]);
        let e = frames.entry(canon).or_default();
        if a.edges[0] == canon {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }
    let mixed_frames = frames.values().filter(|(a, b)| *a > 0 && *b > 0).count();
    println!(
        "stops {} unsnapped {no_attach}; at edge end {at_node}; one-way {one_way}; \
         at edge end and one-way {at_node_one_way}; both directions impassable {both_inf}; \
         edges with stops in both frames {mixed_frames}",
        snaps.stop_count()
    );
}

/// End to end: a forward and a backward search charging one walk differently.
#[test]
fn stops_on_opposite_frames_of_an_edge_agree_forward_and_backward() {
    let provider = andorra();
    let base = GraphId::try_from_components(2, TILES[2], 0).expect("id");
    let mut graph: crate::graph::LazyGraph<_, AnyCosting> =
        crate::graph::LazyGraph::new(andorra(), ped());
    let edges = provider
        .with_tile_containing(base, |t| t.directed_edges().len())
        .expect("t");
    let mut found = None;
    for index in 0..edges {
        let id = base.with_feature_index(index as u64).expect("id");
        let Ok(Some(d)) = graph.describe_edge(id) else {
            continue;
        };
        let (a, b) = (d.weights.forward, d.weights.backward);
        if a == INF || b == INF || a < 200 || a > 60_000 || b > 60_000 {
            continue;
        }
        let mut x = 0x0123_4567_89ab_cdefu64 ^ index as u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..4_000_000 {
            let p = (next() % 1_000_000) as f32 / 1_000_003.7;
            let r = (next() % 1_000_000) as f32 / 999_991.3;
            let f = direct_cost([a, b], p, 1.0 - r);
            let g = direct_cost([b, a], 1.0 - p, r);
            if f != g {
                found = Some((
                    id,
                    graph.edge_id(d.opposing_dense).expect("opp"),
                    p,
                    r,
                    a,
                    b,
                    f,
                    g,
                ));
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let (e, opp, p, r, a, b, f, g) = found.expect("a mismatching split");
    let snaps = snaps_of(&provider, &[point(e, p), point(opp, r)]);
    let mut search = WalkSearch::new(provider, ped());
    search.bind(snaps).expect("bind");
    let forward = run(&mut search, &[seed(0, 0)], Direction::Forward, i32::MAX / 2);
    let backward = run(
        &mut search,
        &[seed(1, 0)],
        Direction::Backward,
        i32::MAX / 2,
    );
    assert_eq!(
        forward[&1].0, backward[&0].0,
        "edge weights {a}/{b}, p {p} r {r}: direct costs {f} and {g}"
    );
}

/// What Thor makes of the zero-length one-edge walk of co-located stops.
#[test]
fn colocated_stops_through_thor() {
    use crate::paths::transfer_leg;
    let provider = andorra();
    let (_, e1, _) = node_with_two_edges(&provider);
    let snaps = snaps_of(&provider, &[point(e1, 0.5), point(e1, 0.5)]);
    let mut search = WalkSearch::new(provider, ped());
    search.bind(snaps).expect("bind");
    let found = run(
        &mut search,
        &[seed(0, 100)],
        Direction::Forward,
        i32::MAX / 2,
    );
    let walk = search.walk(found[&1].2);
    // A walk of no length has no shape to build.
    assert!(
        transfer_leg(
            &walk,
            valinor_sif::TravelMode::Pedestrian,
            0,
            1,
            (1.5, 42.5),
            (1.5, 42.5)
        )
        .is_none()
    );
}

/// The likely shape of the lost failure: forward and backward searches sharing one run.
#[test]
#[ignore = "demonstrates misuse of the API: no begin_run between independent searches"]
fn forward_and_backward_sharing_a_run_disagree() {
    let provider = andorra();
    let points = super::tile_tests::random_stops(&provider, &TILES, 200, 7);
    let snaps = snaps_of(&provider, &points);
    let mut search = WalkSearch::new(provider, ped());
    search.bind(snaps).expect("bind");
    search.begin_run(Direction::Forward);
    let (mut checked, mut mismatches, mut missing) = (0, 0, 0);
    let forward = round(&mut search, &[seed(3, 0)], Direction::Forward, i32::MAX / 2);
    for (&v, &(clock, ..)) in &forward {
        let backward = round(
            &mut search,
            &[seed(v, 0)],
            Direction::Backward,
            i32::MAX / 2,
        );
        checked += 1;
        match backward.get(&3) {
            Some(&(b, ..)) if b == clock => {}
            Some(_) => mismatches += 1,
            None => missing += 1,
        }
    }
    println!("checked {checked}, {mismatches} mismatches, {missing} missing");
    assert_eq!((mismatches, missing), (0, 0));
}
