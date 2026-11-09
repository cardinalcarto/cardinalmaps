//! Tests on real tiles, using the committed Andorra fixture.
//!
//! Tests needing large local data (`/mnt/maps/cardinal/...`) are `#[ignore]`d;
//! run them with `-- --ignored`, in a release build.

use std::collections::{BinaryHeap, HashMap};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::GraphTile;
use valhalla_graphtile::tile_provider::{
    DirectoryGraphTileProvider, GraphTileProvider, TarballTileProvider,
};
use valinor_sif::{AnyCosting, Costing};
use valinor_transit::{
    Clock, Direction, StopId, TransferArrival, TransferOutcome, TransferRequest, TransferSearch,
    TransferSeed, TransferSink,
};

use super::*;
use crate::snap::{SnapPoint, StopSnaps};

/// Andorra, committed in `valhalla-graphtile/fixtures`.
fn andorra() -> DirectoryGraphTileProvider {
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles");
    DirectoryGraphTileProvider::new(tiles, NonZeroUsize::new(64).expect("nonzero"))
}

/// Costing with default options, as a request naming only the costing gets.
pub(crate) fn costing_named(name: &str) -> AnyCosting {
    use valinor_sif::costing_options::{parse_costing, parse_costing_name};
    use valinor_sif::json::JsonValue;
    let doc = JsonValue::parse(b"{}").expect("empty document");
    let costing_type = parse_costing_name(name).expect("costing name");
    let mut proto = valhalla_proto::Costing::default();
    parse_costing(
        &doc,
        "/none",
        &mut proto,
        &mut Vec::new(),
        Some(costing_type),
    )
    .expect("parse");
    AnyCosting::new(&proto).expect("costing")
}

fn pedestrian() -> AnyCosting {
    costing_named("pedestrian")
}

fn bicycle() -> AnyCosting {
    costing_named("bicycle")
}

#[derive(Default)]
struct Sink {
    best: HashMap<u32, (i32, u32, WalkHandle)>,
}

impl TransferSink<WalkHandle> for Sink {
    fn reach(&mut self, arrival: TransferArrival<WalkHandle>) {
        let stop = arrival.stop.into_inner();
        assert_ne!(
            stop,
            arrival.source.into_inner(),
            "a stop walked from itself"
        );
        let c = arrival.clock.seconds();
        match self.best.get(&stop) {
            Some(&(best, _, _)) if best <= c => {}
            _ => {
                self.best
                    .insert(stop, (c, arrival.source.into_inner(), arrival.parent));
            }
        }
    }
}

fn run_search<P: GraphTileProvider, C: Costing>(
    search: &mut WalkSearch<P, C>,
    seeds: &[TransferSeed],
    direction: Direction,
    upper_bound: i32,
) -> HashMap<u32, (i32, u32, WalkHandle)> {
    let mut sink = Sink::default();
    let request = TransferRequest {
        direction,
        seeds,
        upper_bound: Clock::new(upper_bound),
        interrupt: &|| false,
    };
    let outcome = search.search(&request, &mut sink).expect("search");
    assert_eq!(outcome, TransferOutcome::Completed);
    sink.best
}

/// Snaps for stops placed along random pedestrian edges of a tile,
/// from a deterministic generator (no Loki): stops on edges directly.
fn random_stops<P: GraphTileProvider>(
    provider: &P,
    tiles: &[u64],
    count: usize,
    seed: u64,
) -> Vec<SnapPoint> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut stops = Vec::new();
    for i in 0..count {
        let base = GraphId::try_from_components(2, tiles[i % tiles.len()], 0).expect("id");
        stops.push(
            provider
                .with_tile_containing(base, |tile| {
                    let edges = tile.directed_edges();
                    assert!(!edges.is_empty(), "empty tile {base:?}");
                    loop {
                        let index = (next() % edges.len() as u64) as usize;
                        if edges[index].is_shortcut() {
                            continue;
                        }
                        let percent = (next() % 1001) as f32 / 1000.0;
                        return SnapPoint {
                            edge: base.with_feature_index(index as u64).expect("edge id"),
                            percent_along: percent,
                            distance_m: 0.0,
                            point: (0.0, 0.0),
                        };
                    }
                })
                .expect("tile"),
        );
    }
    stops
}

/// The local tiles of the Andorra fixture.
const ANDORRA_TILES: [u64; 5] = [762_485, 762_486, 763_925, 763_926, 763_927];

fn snaps_for<P: GraphTileProvider>(provider: &P, points: &[SnapPoint]) -> Arc<StopSnaps> {
    let build_id = provider
        .with_tile_containing(points[0].edge, |t| t.header().build_id())
        .expect("tile");
    let mut snaps = StopSnaps::new(build_id, points.len());
    for (i, &p) in points.iter().enumerate() {
        snaps
            .insert(
                StopId::new(u32::try_from(i).expect("index")),
                crate::snap::SnappedStop {
                    result: Ok(p),
                    quality: crate::snap::SnapQuality::Connected,
                    build_id: Some(build_id),
                },
            )
            .expect("insert");
    }
    Arc::new(snaps)
}

fn seed_clock(i: usize) -> Clock {
    // Seeds with varied clocks, as RAPTOR gives them, in a range wider than an edge.
    Clock::new(((i * 37) % 1000) as i32)
}

fn make_seeds(ids: &[u32]) -> Vec<TransferSeed> {
    ids.iter()
        .enumerate()
        .map(|(i, &s)| TransferSeed {
            stop: StopId::new(s),
            clock: seed_clock(i),
        })
        .collect()
}

/// A plain single-source Dijkstra over the same memoized arcs, in the given search direction,
/// as a reference for the multi-source search: it only uses the costs, not the label logic.
fn reference_from<P: GraphTileProvider, C: Costing>(
    search: &mut WalkSearch<P, C>,
    seed: u32,
    dir: usize,
) -> HashMap<u32, u32> {
    let mut dist: Vec<u32> = vec![u32::MAX; search.graph.node_count()];
    let mut heap = BinaryHeap::new();
    let attach = search.stops[seed as usize].expect("snapped seed");
    let leave = if dir == 0 { attach.from } else { attach.into };
    let start_at =
        |side: usize| -> Option<u32> { (leave[side] != INF).then(|| u32::from(leave[side])) };
    for side in 0..2 {
        if let Some(cost) = start_at(side) {
            let n = attach.node[side];
            dist.resize((n as usize + 1).max(dist.len()), u32::MAX);
            if cost < dist[n as usize] {
                dist[n as usize] = cost;
                heap.push(core::cmp::Reverse((cost, n)));
            }
        }
    }
    while let Some(core::cmp::Reverse((d, node))) = heap.pop() {
        if dist.get(node as usize).copied().unwrap_or(u32::MAX) < d {
            continue;
        }
        if search.graph.first_arc[node as usize] == u32::MAX {
            search.graph.materialize(node).expect("materialize");
        }
        search.ensure_node_arrays();
        let first = search.graph.first_arc[node as usize];
        if first == NO_ARCS {
            continue;
        }
        let mut index = first as usize;
        loop {
            let arc = search.graph.arcs[index];
            let w = arc.secs[dir];
            if w != INF {
                let next = d + u32::from(w);
                let head = arc.head() as usize;
                if head >= dist.len() {
                    dist.resize(search.graph.node_count().max(head + 1), u32::MAX);
                }
                if next < dist[head] {
                    dist[head] = next;
                    heap.push(core::cmp::Reverse((next, arc.head())));
                }
            }
            if arc.is_last() {
                break;
            }
            index += 1;
        }
    }
    let mut result = HashMap::new();
    for (stop, attach_v) in search.stops.iter().enumerate() {
        let Some(v) = attach_v else { continue };
        let stop = stop as u32;
        if stop == seed {
            continue;
        }
        let cost_into = if dir == 0 { v.into } else { v.from };
        let mut best = u32::MAX;
        for side in 0..2 {
            let node_dist = dist.get(v.node[side] as usize).copied().unwrap_or(u32::MAX);
            if node_dist != u32::MAX && cost_into[side] != INF {
                best = best.min(node_dist + u32::from(cost_into[side]));
            }
        }
        // Same-edge direct walk.
        if v.group.1 > 0 {
            let u = search.stops[seed as usize].expect("seed");
            if u.edges[0] == v.edges[0] || u.edges[0] == v.edges[1] {
                let q = if v.edges[0] == u.edges[0] {
                    v.percent
                } else {
                    1.0 - v.percent
                };
                let (a, b) = if dir == 0 {
                    (u.percent, q)
                } else {
                    (q, u.percent)
                };
                let cost = direct_cost(u.weights, a, b);
                if cost != INF {
                    best = best.min(u32::from(cost));
                }
            }
        }
        if best != u32::MAX {
            result.insert(stop, best);
        }
    }
    result
}

/// The smallest arrival at each stop from every other seed, by brute force over `reference_from`.
fn reference_all<P: GraphTileProvider, C: Costing>(
    search: &mut WalkSearch<P, C>,
    seeds: &[TransferSeed],
    dir: usize,
) -> HashMap<u32, i32> {
    let mut best: HashMap<u32, i32> = HashMap::new();
    for seed in seeds {
        let s = seed.stop.into_inner();
        for (stop, d) in reference_from(search, s, dir) {
            let clock = seed.clock.seconds() + d as i32;
            best.entry(stop)
                .and_modify(|b| *b = (*b).min(clock))
                .or_insert(clock);
        }
    }
    best
}

fn assert_matches_reference<C: Costing>(costing: C, stops: usize, seed_counts: &[usize]) {
    let provider = andorra();
    let points = random_stops(&provider, &ANDORRA_TILES, stops, 42);
    let snaps = snaps_for(&provider, &points);
    let mut search = WalkSearch::new(provider, costing);
    search.bind(snaps).expect("bind");
    for dir in [Direction::Forward, Direction::Backward] {
        search.begin_run(dir);
        let d = usize::from(dir == Direction::Backward);
        for (attempt, &count) in seed_counts.iter().enumerate() {
            let ids: Vec<u32> = (0..u32::try_from(stops).expect("count"))
                .step_by(stops / count)
                .take(count)
                .map(|i| {
                    (i + u32::try_from(attempt).expect("index"))
                        % u32::try_from(stops).expect("count")
                })
                .collect();
            let seeds = make_seeds(&ids);
            let found = run_search(&mut search, &seeds, dir, i32::MAX / 2);
            let expected = reference_all(&mut search, &seeds, d);
            assert_eq!(
                found.len(),
                expected.len(),
                "reached different stop sets ({dir:?}, {count} seeds)"
            );
            for (stop, &clock) in &expected {
                assert_eq!(found[stop].0, clock, "stop {stop} ({dir:?}, {count} seeds)");
            }
        }
    }
}

#[test]
fn forward_and_backward_match_a_reference_dijkstra() {
    assert_matches_reference(pedestrian(), 300, &[1, 2, 5, 25, 150]);
}

#[test]
fn bicycles_match_a_reference_dijkstra_too() {
    assert_matches_reference(bicycle(), 300, &[1, 10, 100]);
}

// ------------------------------------------------------------------------------------------
// Large-data tests: run with `-- --ignored` in a release build.
// ------------------------------------------------------------------------------------------

const WA_TILES: &str = "/mnt/maps/cardinal/routing-elevation-i2b/washington/tiles.tar";

fn tar_provider(path: &str) -> TarballTileProvider<false, false> {
    TarballTileProvider::new_readonly(path).expect("tiles")
}

/// Whether opposing edges of a sample really have different times from their grades.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn tiles_carry_elevation() {
    let provider = tar_provider(WA_TILES);
    let center = geo::Point::new(-122.33f64, 47.61f64);
    let tiles = provider
        .tiles_within_radius(center, 20_000.0f64)
        .expect("tiles");
    let local: Vec<GraphId> = tiles.into_iter().filter(|t| t.level() == 2).collect();
    println!("tiles: {}", local.len());
    let mut search = WalkSearch::new(provider, costing_named("pedestrian"));
    let (mut pairs, mut differ, mut grades_differ) = (0u64, 0u64, 0u64);
    let (mut grade_counts, mut sum_abs) = ([0u64; 16], 0u64);
    for &tile in &local {
        let edges = search
            .graph
            .provider
            .with_tile_containing(tile, |t| t.directed_edges().len())
            .expect("tile");
        for index in 0..edges {
            let id = tile.with_feature_index(index as u64).expect("id");
            let Some(desc) = search.graph.describe_edge(id).expect("edge") else {
                continue;
            };
            if desc.weights.forward == INF || desc.weights.backward == INF {
                continue;
            }
            pairs += 1;
            let tile_h = search.graph.tile(id).expect("tile").expect("present");
            let edge = tile_h.get_directed_edge(id).expect("edge");
            grade_counts[usize::from(edge.weighted_grade())] += 1;
            if desc.weights.forward != desc.weights.backward {
                differ += 1;
                sum_abs += u64::from(desc.weights.forward.abs_diff(desc.weights.backward));
            }
            // Compare with the opposing edge's stored grade.
            let opp_id = search.graph.edge_id(desc.opposing_dense).expect("opp");
            let opp_handle = search.graph.tile(opp_id).expect("tile").expect("present");
            let opp = opp_handle.get_directed_edge(opp_id).expect("opp");
            if edge.weighted_grade() != opp.weighted_grade() {
                grades_differ += 1;
            }
        }
    }
    println!(
        "pedestrian-passable directed edge pairs: {pairs}; differing integer secs: {differ} \
         ({:.1}%), mean abs difference when differing: {:.2}s; weighted_grade differs between \
         opposing edges: {grades_differ} ({:.1}%)",
        100.0 * differ as f64 / pairs as f64,
        sum_abs as f64 / differ.max(1) as f64,
        100.0 * grades_differ as f64 / pairs as f64,
    );
    println!("weighted_grade histogram (0-15): {grade_counts:?}");
    assert!(grades_differ > 0, "tiles carry no grade information");
}

#[test]
fn forward_and_backward_agree_exactly() {
    let provider = andorra();
    let points = random_stops(&provider, &ANDORRA_TILES, 200, 7);
    let snaps = snaps_for(&provider, &points);
    let mut search = WalkSearch::new(provider, pedestrian());
    search.bind(snaps).expect("bind");
    for &u in &[0u32, 3, 57, 110, 199] {
        let seeds = [TransferSeed {
            stop: StopId::new(u),
            clock: Clock::new(0),
        }];
        search.begin_run(Direction::Forward);
        let forward = run_search(&mut search, &seeds, Direction::Forward, i32::MAX / 2);
        for (&v, &(clock, source, _)) in &forward {
            assert_eq!(source, u);
            let back = [TransferSeed {
                stop: StopId::new(v),
                clock: Clock::new(0),
            }];
            search.begin_run(Direction::Backward);
            let backward = run_search(&mut search, &back, Direction::Backward, i32::MAX / 2);
            let &(from_v, _, _) = backward.get(&u).expect("mirror arrival");
            assert_eq!(from_v, clock, "walk {u} -> {v}: forward vs backward");
        }
    }
}

#[test]
fn every_stop_reports_the_best_arrival_from_another_seed() {
    // Every stop is a seed, so every stop's label at its own edge's nodes is its own:
    // the second label per node is what makes its arrival from elsewhere correct.
    assert_matches_reference(pedestrian(), 120, &[120]);
}

#[test]
fn the_upper_bound_only_prunes_arrivals_at_or_beyond_it() {
    let provider = andorra();
    let points = random_stops(&provider, &ANDORRA_TILES, 200, 11);
    let snaps = snaps_for(&provider, &points);
    let mut search = WalkSearch::new(provider, pedestrian());
    search.bind(snaps).expect("bind");
    let seeds = make_seeds(&[1, 2, 3, 4, 5, 6]);
    search.begin_run(Direction::Forward);
    let unbounded = run_search(&mut search, &seeds, Direction::Forward, i32::MAX / 2);
    let bound = 1500;
    let bounded = run_search(&mut search, &seeds, Direction::Forward, bound);
    for (stop, &(clock, ..)) in &unbounded {
        if clock < bound {
            assert_eq!(
                bounded[stop].0, clock,
                "stop {stop} was pruned below the bound"
            );
        }
    }
    assert!(bounded.iter().all(|(_, &(clock, ..))| clock < bound));
}

#[test]
fn interrupts_stop_the_search() {
    let provider = andorra();
    let points = random_stops(&provider, &ANDORRA_TILES, 50, 3);
    let snaps = snaps_for(&provider, &points);
    let mut search = WalkSearch::new(provider, pedestrian());
    search.bind(snaps).expect("bind");
    let seeds = make_seeds(&(0..50).collect::<Vec<_>>());
    let mut sink = Sink::default();
    let request = TransferRequest {
        direction: Direction::Forward,
        seeds: &seeds,
        upper_bound: Clock::new(i32::MAX / 2),
        interrupt: &|| true,
    };
    let outcome = search.search(&request, &mut sink).expect("search");
    assert_eq!(outcome, TransferOutcome::Interrupted);
    // A later search still works: the queue was cleared.
    let found = run_search(&mut search, &seeds, Direction::Forward, i32::MAX / 2);
    assert!(!found.is_empty());
}

#[test]
fn snaps_are_refused_for_another_tile_build() {
    let provider = andorra();
    let points = random_stops(&provider, &ANDORRA_TILES, 5, 1);
    let good = snaps_for(&provider, &points);
    let build = good.build_id();
    let mut snaps = StopSnaps::new(
        valhalla_graphtile::graph_tile::BuildId::new(build.value() ^ 1),
        5,
    );
    for (i, &p) in points.iter().enumerate() {
        snaps
            .insert(
                StopId::new(i as u32),
                crate::snap::SnappedStop {
                    result: Ok(p),
                    quality: crate::snap::SnapQuality::Connected,
                    build_id: None,
                },
            )
            .expect("insert");
    }
    let mut search = WalkSearch::new(provider, pedestrian());
    assert!(matches!(
        search.bind(Arc::new(snaps)),
        Err(crate::WalkError::BuildMismatch { .. })
    ));
}

#[test]
fn a_mismatched_build_snap_is_refused_on_insert() {
    let mut snaps = StopSnaps::new(valhalla_graphtile::graph_tile::BuildId::new(1), 1);
    let snapped = crate::snap::SnappedStop {
        result: Err(crate::snap::SnapFailure::NothingNearby),
        quality: crate::snap::SnapQuality::Disconnected,
        build_id: Some(valhalla_graphtile::graph_tile::BuildId::new(2)),
    };
    assert!(snaps.insert(StopId::new(0), snapped).is_err());
}

#[test]
fn stops_on_the_same_edge_walk_directly() {
    let provider = andorra();
    // A level-2 edge a pedestrian can walk both ways, with the stops at 20% and 80% along it.
    let (edge, opp_edge_percent) = {
        let base = GraphId::try_from_components(2, ANDORRA_TILES[0], 0).expect("id");
        provider
            .with_tile_containing(base, |t| {
                let mut found = None;
                for (i, e) in t.directed_edges().iter().enumerate() {
                    if !e.is_shortcut() && e.length() > 60 {
                        found = Some(base.with_feature_index(i as u64).expect("id"));
                        break;
                    }
                }
                (found.expect("long edge"), 0.0f32)
            })
            .expect("tile")
    };
    let _ = opp_edge_percent;
    let points: Vec<SnapPoint> = [0.2, 0.8]
        .iter()
        .map(|&percent_along| SnapPoint {
            edge,
            percent_along,
            distance_m: 0.0,
            point: (0.0, 0.0),
        })
        .collect();
    let snaps = snaps_for(&provider, &points);
    let mut search = WalkSearch::new(provider, pedestrian());
    search.bind(snaps).expect("bind");
    let seeds = [TransferSeed {
        stop: StopId::new(0),
        clock: Clock::new(100),
    }];
    search.begin_run(Direction::Forward);
    let found = run_search(&mut search, &seeds, Direction::Forward, i32::MAX / 2);
    let &(clock, _, handle) = found.get(&1).expect("the other stop");
    let attach = search.stops[0].expect("stop");
    let expected = direct_cost(attach.weights, 0.2, 0.8);
    assert_eq!(clock - 100, i32::from(expected));
    assert_eq!(handle.chain, NONE, "a direct walk has no node chain");
    let walk = search.walk(handle);
    assert_eq!(walk.edges, vec![edge]);
    assert!((walk.begin_percent - 0.2).abs() < 1e-5 && (walk.end_percent - 0.8).abs() < 1e-5);
    assert_eq!(walk.secs(), u32::from(expected));
}

/// The location of a node, for checking walks across hierarchy levels.
fn node_coordinates<P: GraphTileProvider, C: Costing>(
    search: &WalkSearch<P, C>,
    node: u32,
) -> (f64, f64) {
    let id = search.graph.node_id(node).expect("node id");
    search
        .graph
        .provider
        .with_tile_containing(id, |t| {
            let c = t
                .get_node(id)
                .expect("node")
                .coordinate_f64(t.header().sw_corner());
            (c.x, c.y)
        })
        .expect("tile")
}

/// A walk's edges must connect head to tail, and its parts must sum to its arrival.
fn check_walk_consistent<P: GraphTileProvider, C: Costing>(
    search: &mut WalkSearch<P, C>,
    walk: &ResolvedWalk,
    clock_delta: i32,
) {
    assert_eq!(
        walk.secs() as i32,
        clock_delta,
        "the walk's secs are not its arrival"
    );
    for pair in walk.edges.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let da = search.graph.describe_edge(a).expect("edge").expect("edge");
        let db = search.graph.describe_edge(b).expect("edge").expect("edge");
        // The end of one edge is the start of the next, possibly across hierarchy levels.
        let end = node_coordinates(search, da.head);
        let start = node_coordinates(search, db.tail);
        assert!(
            (end.0 - start.0).abs() < 1e-5 && (end.1 - start.1).abs() < 1e-5,
            "edges {a:?} and {b:?} are not connected"
        );
    }
    assert!(!walk.begin_percent.is_nan() && !walk.end_percent.is_nan());
}

#[test]
fn resolved_walks_sum_to_their_arrivals() {
    let provider = andorra();
    let points = random_stops(&provider, &ANDORRA_TILES, 200, 5);
    let snaps = snaps_for(&provider, &points);
    let mut search = WalkSearch::new(provider, pedestrian());
    search.bind(snaps).expect("bind");
    for dir in [Direction::Forward, Direction::Backward] {
        search.begin_run(dir);
        let seeds = make_seeds(&[10, 20]);
        let found = run_search(&mut search, &seeds, dir, i32::MAX / 2);
        let clocks: HashMap<u32, i32> = seeds
            .iter()
            .map(|s| (s.stop.into_inner(), s.clock.seconds()))
            .collect();
        assert!(!found.is_empty());
        for (&stop, &(clock, source, handle)) in &found {
            let walk = search.walk(handle);
            assert_eq!(
                walk.to.into_inner(),
                if dir == Direction::Forward {
                    stop
                } else {
                    source
                }
            );
            check_walk_consistent(&mut search, &walk, clock - clocks[&source]);
        }
    }
}

#[test]
fn snapping_a_location_goes_through_loki() {
    let provider = andorra();
    let costing = pedestrian();
    let build = provider
        .with_tile_containing(
            GraphId::try_from_components(2, ANDORRA_TILES[0], 0).expect("id"),
            |t| t.header().build_id(),
        )
        .expect("tile");
    // In the middle of Andorra la Vella, and in the middle of the sea.
    let snapped = crate::snap_stops(
        &provider,
        &costing,
        &crate::SnapConfig::default(),
        build,
        &[(1.5218, 42.5063), (-20.0, 42.0)],
    )
    .expect("snap");
    assert!(snapped.point(StopId::new(0)).is_some());
    assert!(snapped.point(StopId::new(1)).is_none());
    assert_eq!(
        snapped.reports(),
        &[crate::SnapReport::Unsnapped(StopId::new(1))]
    );
    // Snapping a single stop later, as a GTFS-RT overlay does.
    let one = crate::snap_stop(
        &provider,
        &costing,
        &crate::SnapConfig::default(),
        (1.5300, 42.5100),
    )
    .expect("snap");
    assert!(one.result.is_ok());
    assert_eq!(one.build_id, Some(build));
}

#[test]
fn build_id_of_a_tile_set_is_read_from_a_tile() {
    let provider = andorra();
    let mut graph: crate::graph::LazyGraph<_, AnyCosting> =
        crate::graph::LazyGraph::new(provider, pedestrian());
    let id = GraphId::try_from_components(2, ANDORRA_TILES[0], 0).expect("id");
    let build = graph.build_id_of(id).expect("read").expect("present");
    assert_eq!(graph.build_id(), Some(build));
}

// ------------------------------------------------------------------------------------------
// Access and egress tables, and conversion to Thor's paths.
// ------------------------------------------------------------------------------------------

use crate::table::{AccessSide, StopTables, TableRequest, TableSeed};

fn access_table_for<P: GraphTileProvider, C: Costing>(
    tables: &mut StopTables<P, C>,
    side: AccessSide,
    seed: SnapPoint,
    stop_count: usize,
) -> valinor_transit::StopTable {
    let seeds = [TableSeed {
        edge: seed.edge,
        percent_along: seed.percent_along,
        initial: 0.0,
    }];
    let request = TableRequest {
        side,
        seeds: &seeds,
        max_seconds: 3600,
        interrupt: &|| false,
    };
    tables
        .compute(stop_count, &request)
        .expect("table")
        .expect("not interrupted")
}

/// Sets up a table over Andorra; the seed is the first random point.
fn tables_fixture() -> (
    StopTables<DirectoryGraphTileProvider, AnyCosting>,
    Arc<StopSnaps>,
    Vec<SnapPoint>,
) {
    let provider = andorra();
    let mut points = random_stops(&provider, &ANDORRA_TILES, 300, 21);
    // The first point is the query's location: somewhere Loki knows a person can walk.
    points[0] = crate::snap_stop(
        &provider,
        &pedestrian(),
        &crate::SnapConfig::default(),
        (1.5218, 42.5063),
    )
    .expect("snap")
    .result
    .expect("a snap");
    let snaps = snaps_for(&provider, &points);
    let mut tables = StopTables::new(provider, pedestrian());
    tables.bind(snaps.clone()).expect("bind");
    (tables, snaps, points)
}

#[test]
fn access_tables_reach_stops_and_legs_add_up() {
    let (mut tables, snaps, points) = tables_fixture();
    let table = access_table_for(
        &mut tables,
        AccessSide::Access,
        points[0],
        snaps.stop_count(),
    );
    let mut reached = 0;
    for i in 0..snaps.stop_count() {
        let stop = StopId::new(i as u32);
        let Some(secs) = table.cost(stop) else {
            continue;
        };
        reached += 1;
        let leg = tables.leg(stop).expect("leg of a reached stop");
        assert_eq!(leg.side, AccessSide::Access);
        let total: f32 = leg
            .edges
            .iter()
            .map(|e| e.transition.secs + e.traversal.secs)
            .sum();
        assert!(
            (total - leg.secs).abs() < 0.05,
            "edges sum {total} vs leg {}",
            leg.secs
        );
        assert_eq!(secs.into_inner(), leg.secs.round() as u32);
        // Access runs from the origin to the stop: the stop's edge comes last.
        let last = leg.edges.last().expect("edges");
        let point = points[i];
        assert!(last.edge == point.edge || is_opposing_of(last.edge, point.edge));
    }
    assert!(reached > 50, "only {reached} stops reached");
}

/// Whether two directed edges are each other's opposing edges, in the Andorra fixture.
fn is_opposing_of(a: GraphId, b: GraphId) -> bool {
    let provider = andorra();
    let mut graph: crate::graph::LazyGraph<_, AnyCosting> =
        crate::graph::LazyGraph::new(provider, pedestrian());
    let (Some(da), Some(db)) = (
        graph.describe_edge(a).expect("a"),
        graph.describe_edge(b).expect("b"),
    ) else {
        return false;
    };
    da.edge_dense == db.opposing_dense && da.opposing_dense == db.edge_dense
}

#[test]
fn egress_tables_mirror_access_on_a_small_scale() {
    let (mut tables, snaps, points) = tables_fixture();
    let forward = access_table_for(
        &mut tables,
        AccessSide::Access,
        points[0],
        snaps.stop_count(),
    );
    let backward = access_table_for(
        &mut tables,
        AccessSide::Egress,
        points[0],
        snaps.stop_count(),
    );
    let mut both = 0;
    for i in 0..snaps.stop_count() {
        let stop = StopId::new(i as u32);
        if let (Some(a), Some(b)) = (forward.cost(stop), backward.cost(stop)) {
            both += 1;
            // The two directions differ by hills and turns, but not by much.
            let (a, b) = (f64::from(a.into_inner()), f64::from(b.into_inner()));
            assert!(
                (a - b).abs() <= 0.5 * a.max(b).max(60.0),
                "access {a}s vs egress {b}s"
            );
        }
    }
    assert!(both > 50, "only {both} stops reached both ways");
}

fn pedestrian_options() -> valhalla_proto::Options {
    use valhalla_proto::costing::Type as CostingType;
    use valinor_sif::costing_options::parse_costing;
    use valinor_sif::json::JsonValue;
    let mut options = valhalla_proto::Options {
        costing_type: CostingType::Pedestrian as i32,
        ..Default::default()
    };
    let doc = JsonValue::parse(b"{}").expect("empty");
    let mut proto = valhalla_proto::Costing::default();
    parse_costing(
        &doc,
        "/none",
        &mut proto,
        &mut Vec::new(),
        Some(CostingType::Pedestrian),
    )
    .expect("costing");
    options
        .costings
        .insert(CostingType::Pedestrian as i32, proto);
    options
}

#[test]
fn walks_convert_to_thor_legs_with_a_shape() {
    use crate::paths::{route_paths, transfer_leg};
    let provider = andorra();
    let points = random_stops(&provider, &ANDORRA_TILES, 100, 9);
    let snaps = snaps_for(&provider, &points);
    let mut search = WalkSearch::new(provider, pedestrian());
    search.bind(snaps).expect("bind");
    let seeds = make_seeds(&[0, 1, 2]);
    search.begin_run(Direction::Forward);
    let found = run_search(&mut search, &seeds, Direction::Forward, i32::MAX / 2);
    let &(_, _, handle) = found
        .values()
        .find(|(_, _, h)| h.chain != NONE)
        .expect("an arrival through the graph");
    let walk = search.walk(handle);
    assert!(walk.edges.len() >= 2);
    let leg = transfer_leg(
        &walk,
        valinor_sif::TravelMode::Pedestrian,
        0,
        1,
        (1.5, 42.5),
        (1.51, 42.51),
    )
    .expect("a leg");
    let paths = route_paths(vec![leg]);
    let thor = valinor_thor::Thor::new(valinor_thor::ThorConfig::default(), andorra());
    let mut api = valhalla_proto::Api {
        options: Some(pedestrian_options()),
        ..Default::default()
    };
    thor.build_trip(&mut api, &paths).expect("build trip");
    let trip = api.trip.expect("trip");
    assert_eq!(trip.routes[0].legs.len(), 1);
    let trip_leg = &trip.routes[0].legs[0];
    assert!(!trip_leg.shape.is_empty(), "no shape");
    assert_eq!(trip_leg.node.len(), walk.edges.len() + 1);
}

#[test]
fn access_legs_convert_to_thor_legs_with_a_shape() {
    use crate::paths::{access_leg, route_paths};
    let (mut tables, snaps, points) = tables_fixture();
    let _ = access_table_for(
        &mut tables,
        AccessSide::Egress,
        points[3],
        snaps.stop_count(),
    );
    let leg = (0..snaps.stop_count())
        .find_map(|i| {
            tables
                .leg(StopId::new(i as u32))
                .filter(|l| l.edges.len() >= 3)
        })
        .expect("a long enough leg");
    let thor_leg = access_leg(
        &leg,
        valinor_sif::TravelMode::Pedestrian,
        0,
        1,
        (1.5, 42.5),
        (1.51, 42.51),
    )
    .expect("a leg");
    let thor = valinor_thor::Thor::new(valinor_thor::ThorConfig::default(), andorra());
    let mut api = valhalla_proto::Api {
        options: Some(pedestrian_options()),
        ..Default::default()
    };
    thor.build_trip(&mut api, &route_paths(vec![thor_leg]))
        .expect("build trip");
    let trip = api.trip.expect("trip");
    assert!(!trip.routes[0].legs[0].shape.is_empty());
}
