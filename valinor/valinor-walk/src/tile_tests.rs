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
    TransferSeed, TransferSink, WalkRules,
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
pub(super) fn costing_named(name: &str) -> AnyCosting {
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
    // A new run each time: these searches are independent of one another.
    search.begin_run(direction);
    let mut sink = Sink::default();
    let request = TransferRequest {
        direction,
        seeds,
        upper_bound: Clock::new(upper_bound),
        change_margin: TransferRequest::REPORT_ALL,
        rules: WalkRules::none(),
        interrupt: &|| false,
    };
    let outcome = search.search(&request, &mut sink).expect("search");
    assert_eq!(outcome, TransferOutcome::Completed);
    sink.best
}

/// Snaps for stops placed along random pedestrian edges of a tile,
/// from a deterministic generator (no Loki): stops on edges directly.
pub(crate) fn random_stops<P: GraphTileProvider>(
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
                let (from, to) = if dir == 0 { (u, *v) } else { (*v, u) };
                let cost = direct_between(from, to).cost;
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
        change_margin: TransferRequest::REPORT_ALL,
        rules: WalkRules::none(),
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
    let other = search.stops[1].expect("stop");
    let expected = direct_between(attach, other).cost;
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

pub(crate) fn pedestrian_options() -> valhalla_proto::Options {
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
    // Some seeds lie on an edge whose cost alone exceeds the search's bound, so look for one that
    // reaches further.
    let leg = points[3..]
        .iter()
        .find_map(|&seed| {
            let _ = access_table_for(&mut tables, AccessSide::Egress, seed, snaps.stop_count());
            (0..snaps.stop_count()).find_map(|i| {
                tables
                    .leg(StopId::new(i as u32))
                    .filter(|l| l.edges.len() >= 3)
            })
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

// ------------------------------------------------------------------------------------------
// Real data: King County Metro stops on Washington tiles. Run with `-- --ignored --nocapture`
// in a release build, at low priority.
// ------------------------------------------------------------------------------------------

const KCM_ZIP: &str = "/mnt/maps/cardinal/transit/kcm/static/kcm-FAL26-161.2.zip";

/// Splits a line of comma-separated values, honoring double quotes.
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

/// The boardable stops of King County Metro's feed, as `(lon, lat)`.
fn kcm_stops() -> Vec<(f64, f64)> {
    let output = std::process::Command::new("unzip")
        .args(["-p", KCM_ZIP, "stops.txt"])
        .output()
        .expect("unzip");
    assert!(output.status.success(), "unzip failed");
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

/// Washington tiles (as `TileSets`, the provider production uses) with King County Metro's
/// stops snapped.
fn wa_fixture(
    costing: &AnyCosting,
) -> (
    &'static valinor_router::tiles::TileSets,
    Arc<StopSnaps>,
    Vec<(f64, f64)>,
) {
    let tile_sets = valinor_router::tiles::TileSets::open(
        &[WA_TILES],
        NonZeroUsize::new(1024).expect("nonzero"),
    )
    .expect("open tiles");
    // The tests live for the whole process, and a provider is `Copy` through a reference.
    let provider: &'static valinor_router::tiles::TileSets = Box::leak(Box::new(tile_sets));
    let stops = kcm_stops();
    let started = std::time::Instant::now();
    let snaps = crate::snap_stops(
        &provider,
        costing,
        &crate::SnapConfig::default(),
        provider.build_id(),
        &stops,
    )
    .expect("snap");
    let unsnapped = snaps
        .reports()
        .iter()
        .filter(|r| matches!(r, crate::SnapReport::Unsnapped(_)))
        .count();
    let disconnected = snaps
        .reports()
        .iter()
        .filter(|r| matches!(r, crate::SnapReport::Disconnected(..)))
        .count();
    println!(
        "snapped {} KCM stops in {:.2?}: {unsnapped} unsnapped, {disconnected} disconnected",
        stops.len(),
        started.elapsed()
    );
    (provider, Arc::new(snaps), stops)
}

#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn king_county_forward_backward_agree_on_real_tiles() {
    let costing = costing_named("pedestrian");
    let (provider, snaps, _stops) = wa_fixture(&costing);
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
        let seeds = [TransferSeed {
            stop: StopId::new(u),
            clock: Clock::new(0),
        }];
        let forward = run_search(&mut search, &seeds, Direction::Forward, 3600);
        for (&v, &(clock, ..)) in forward.iter().filter(|(v, _)| *v % 13 == i as u32 % 13) {
            let back = [TransferSeed {
                stop: StopId::new(v),
                clock: Clock::new(0),
            }];
            let backward = run_search(&mut search, &back, Direction::Backward, 3600 + 1);
            checked += 1;
            if let Some(&(from_v, _, _)) = backward.get(&u) {
                if from_v != clock {
                    mismatches += 1;
                    if mismatches <= 20 {
                        println!("mismatch {u} -> {v}: forward {clock}, backward {from_v}");
                    }
                }
            } else {
                missing += 1;
                if missing <= 20 {
                    println!("missing mirror arrival {u} -> {v}: forward {clock}");
                }
            }
        }
    }
    println!(
        "checked {checked} forward/backward pairs, {mismatches} mismatches, {missing} missing"
    );
    assert_eq!((mismatches, missing), (0, 0));
    assert!(checked > 100);
}

/// Thor's pedestrian time for a route between two points, in seconds.
fn thor_seconds(router: &valinor_router::Router, a: (f64, f64), b: (f64, f64)) -> Option<f64> {
    let body = format!(
        r#"{{"locations":[{{"lat":{},"lon":{}}},{{"lat":{},"lon":{}}}],"costing":"pedestrian"}}"#,
        a.1, a.0, b.1, b.0
    );
    let response = router.route(body.as_bytes());
    if response.status != 200 {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&response.body).ok()?;
    json.get("trip")?.get("summary")?.get("time")?.as_f64()
}

/// Compares our integer transfer seconds with Thor's pedestrian routes on a sample of stop pairs.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn walking_times_compared_with_thor() {
    let costing = costing_named("pedestrian");
    let (provider, snaps, stops) = wa_fixture(&costing);
    let mut search = WalkSearch::new(provider, costing);
    search.bind(snaps.clone()).expect("bind");
    let config = valinor_router::RouterConfig::default();
    let router = valinor_router::Router::open(config, &[WA_TILES]).expect("router");
    let stop_count = snaps.stop_count();
    let mut diffs = Vec::new();
    let mut ratios = Vec::new();
    let mut failed = 0u32;
    let mut sampled = 0u32;
    let mut i = 0usize;
    while diffs.len() < 400 && i < stop_count {
        let u = ((i * 7919) % stop_count) as u32;
        i += 37;
        if search.stops[u as usize].is_none() {
            continue;
        }
        let seeds = [TransferSeed {
            stop: StopId::new(u),
            clock: Clock::new(0),
        }];
        search.begin_run(Direction::Forward);
        // Stops within a 25-minute walk, sampled to spread over distances.
        let found = run_search(&mut search, &seeds, Direction::Forward, 1500);
        let mut pairs: Vec<(u32, i32)> = found.iter().map(|(&v, &(c, ..))| (v, c)).collect();
        pairs.sort_unstable();
        for &(v, clock) in pairs.iter().step_by(pairs.len().max(1).div_ceil(6)) {
            if clock < 60 {
                continue;
            }
            sampled += 1;
            let Some(thor) = thor_seconds(&router, stops[u as usize], stops[v as usize]) else {
                failed += 1;
                continue;
            };
            let ours = f64::from(clock);
            diffs.push(ours - thor);
            ratios.push(ours / thor);
        }
    }
    diffs.sort_by(f64::total_cmp);
    ratios.sort_by(f64::total_cmp);
    let pct = |v: &[f64], p: f64| v[((v.len() - 1) as f64 * p) as usize];
    println!(
        "sampled {sampled} stop pairs (15 s to 25 min walks); Thor failed on {failed}; {} compared",
        diffs.len()
    );
    println!(
        "ours - Thor seconds: min {:.0}, p5 {:.1}, p25 {:.1}, median {:.1}, p75 {:.1}, p95 {:.1}, max {:.0}, mean {:.1}",
        diffs[0],
        pct(&diffs, 0.05),
        pct(&diffs, 0.25),
        pct(&diffs, 0.5),
        pct(&diffs, 0.75),
        pct(&diffs, 0.95),
        diffs[diffs.len() - 1],
        diffs.iter().sum::<f64>() / diffs.len() as f64,
    );
    println!(
        "ours / Thor ratio: p5 {:.3}, p25 {:.3}, median {:.3}, p75 {:.3}, p95 {:.3}",
        pct(&ratios, 0.05),
        pct(&ratios, 0.25),
        pct(&ratios, 0.5),
        pct(&ratios, 0.75),
        pct(&ratios, 0.95),
    );
}

// ------------------------------------------------------------------------------------------
// The benchmark for the 1 s p95 profile target. Noisy on a busy machine: see the output's
// conditions line, and repeat it.
// ------------------------------------------------------------------------------------------

/// Median and p95 of a sample (destroys the order).
fn median_p95(sample: &mut [f64]) -> (f64, f64) {
    sample.sort_by(f64::total_cmp);
    let n = sample.len();
    (sample[n / 2], sample[((n as f64 - 1.0) * 0.95) as usize])
}

/// Deterministic pseudo-random subsets of stops (`seed` varies between repetitions).
fn seed_sample(stop_count: usize, count: usize, seed: u64) -> Vec<u32> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut seen = vec![false; stop_count];
    let mut ids = Vec::with_capacity(count);
    while ids.len() < count.min(stop_count) {
        let i = (next() as usize) % stop_count;
        if !seen[i] {
            seen[i] = true;
            ids.push(i as u32);
        }
    }
    ids
}

/// One benchmark row: the same search set `repeats` times, in both directions.
#[expect(
    clippy::too_many_arguments,
    reason = "the row's parameters, for printing"
)]
fn bench_row<P: GraphTileProvider, C: Costing>(
    search: &mut WalkSearch<P, C>,
    stop_count: usize,
    seed_count: usize,
    bound: Option<i32>,
    spread_secs: i32,
    repeats: u32,
    label: &str,
    results: &mut String,
) {
    use std::fmt::Write as _;
    let mut times_ms = Vec::with_capacity(repeats as usize);
    let (mut settled, mut relaxed, mut labels, mut pops, mut arrivals, mut arena) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let lookups_before = search.graph_stats().tile_lookups;
    for repeat in 0..repeats {
        let dir = if repeat % 2 == 0 {
            Direction::Forward
        } else {
            Direction::Backward
        };
        let ids = seed_sample(
            stop_count,
            seed_count,
            u64::from(repeat) * 1009 + seed_count as u64,
        );
        let seeds: Vec<TransferSeed> = ids
            .iter()
            .enumerate()
            .map(|(i, &stop)| TransferSeed {
                stop: StopId::new(stop),
                clock: Clock::new(((i as i64 * 7919) % i64::from(spread_secs.max(1))) as i32),
            })
            .collect();
        let min = seeds.iter().map(|s| s.clock.seconds()).min().unwrap_or(0);
        let upper_bound = bound.map_or(i32::MAX / 2, |b| min + b);
        let mut sink = Sink::default();
        let request = TransferRequest {
            direction: dir,
            seeds: &seeds,
            upper_bound: Clock::new(upper_bound),
            change_margin: TransferRequest::REPORT_ALL,
            rules: WalkRules::none(),
            interrupt: &|| false,
        };
        search.begin_run(dir);
        let started = std::time::Instant::now();
        search.search(&request, &mut sink).expect("search");
        times_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        let s = search.last_search_stats();
        settled += s.nodes_settled as f64;
        labels += s.labels_settled as f64;
        relaxed += s.arcs_relaxed as f64;
        pops += s.pops as f64;
        arrivals += s.arrivals as f64;
        arena += s.arena_entries as f64;
    }
    let (median, p95) = median_p95(&mut times_ms);
    let n = f64::from(repeats);
    let lookups = search.graph_stats().tile_lookups - lookups_before;
    let _ = writeln!(
        results,
        "| {label} | {seed_count} | {median:.2} | {p95:.2} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {lookups} |",
        settled / n,
        labels / n,
        relaxed / n,
        arrivals / n,
        arena / n,
    );
    let _ = pops;
}

#[test]
#[ignore = "needs /mnt/maps/cardinal data; run in release"]
fn benchmark_king_county_multi_source_searches() {
    let costing = costing_named("pedestrian");
    let (provider, snaps, _stops) = wa_fixture(&costing);
    let stop_count = snaps.stop_count();
    let mut search = WalkSearch::new(provider, costing);
    let bind_started = std::time::Instant::now();
    search.bind(snaps.clone()).expect("bind");
    println!(
        "bind (attach {} stops): {:.2?}",
        stop_count,
        bind_started.elapsed()
    );

    // Cold: the very first search, which reads and costs every arc it reaches.
    let cold_seeds: Vec<TransferSeed> = (0..stop_count as u32)
        .step_by(100)
        .map(|i| TransferSeed {
            stop: StopId::new(i),
            clock: Clock::new(0),
        })
        .collect();
    let mut sink = Sink::default();
    let request = TransferRequest {
        direction: Direction::Forward,
        seeds: &cold_seeds,
        upper_bound: Clock::new(3600),
        change_margin: TransferRequest::REPORT_ALL,
        rules: WalkRules::none(),
        interrupt: &|| false,
    };
    search.begin_run(Direction::Forward);
    let cold_started = std::time::Instant::now();
    search.search(&request, &mut sink).expect("search");
    println!(
        "cold first search ({} seeds, 1 hour bound): {:.2?} ({:?})",
        cold_seeds.len(),
        cold_started.elapsed(),
        search.last_search_stats()
    );
    println!("graph after warm-up: {:?}", search.graph_stats());

    let mut table = String::from(
        "| workload | seeds | median ms | p95 ms | nodes settled | labels settled | arcs relaxed | arrivals | arena entries | tile lookups |\n|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for &(seeds, spread) in &[
        (1usize, 0),
        (10, 900),
        (100, 1800),
        (1000, 1800),
        (stop_count, 3600),
    ] {
        for (label, bound) in [
            ("no bound", None),
            ("bound +30 min", Some(1800)),
            ("bound +15 min", Some(900)),
            ("bound +5 min", Some(300)),
        ] {
            bench_row(
                &mut search,
                stop_count,
                seeds,
                bound,
                spread,
                21,
                label,
                &mut table,
            );
        }
    }
    println!("{table}");
    println!(
        "working memory: {} MB labels/arena/attachments, graph memo: {:?}",
        search.working_memory_bytes() / (1024 * 1024),
        search.graph_stats()
    );
}

/// A simulated MCR run: `rounds` successive multi-source searches over changing seed sets,
/// with and without labels carrying over between rounds.
fn bench_run_rounds<P: GraphTileProvider, C: Costing>(
    search: &mut WalkSearch<P, C>,
    stop_count: usize,
    run_pruning: bool,
    bound: Option<i32>,
    repeats: u32,
) -> (f64, f64, f64) {
    let sizes = [200usize, 1000, 2000, 1500, 800, 300];
    search.set_run_pruning(run_pruning);
    let mut times = Vec::new();
    let (mut settled, mut arena) = (0.0, 0.0);
    for repeat in 0..repeats {
        let dir = if repeat % 2 == 0 {
            Direction::Forward
        } else {
            Direction::Backward
        };
        search.begin_run(dir);
        let started = std::time::Instant::now();
        let mut upper = bound.unwrap_or(i32::MAX / 2);
        for (r, &size) in sizes.iter().enumerate() {
            let size = size.min(stop_count);
            let ids = seed_sample(stop_count, size, u64::from(repeat) * 91 + r as u64);
            let seeds: Vec<TransferSeed> = ids
                .iter()
                .enumerate()
                .map(|(i, &stop)| TransferSeed {
                    stop: StopId::new(stop),
                    clock: Clock::new(300 * r as i32 + ((i as i32 * 7919) % 1800)),
                })
                .collect();
            let mut sink = Sink::default();
            let request = TransferRequest {
                direction: dir,
                seeds: &seeds,
                upper_bound: Clock::new(upper),
                change_margin: TransferRequest::REPORT_ALL,
                rules: WalkRules::none(),
                interrupt: &|| false,
            };
            search.search(&request, &mut sink).expect("search");
            settled += search.last_search_stats().nodes_settled as f64;
            arena = search.last_search_stats().arena_entries as f64;
            // A route is found by the middle rounds: pretend the target tightens.
            if bound.is_some() && r >= 2 {
                upper = upper.min(300 * (r as i32 + 1) + 1800);
            }
        }
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let (median, _) = median_p95(&mut times);
    search.set_run_pruning(true);
    (median, settled / f64::from(repeats), arena)
}

#[test]
#[ignore = "needs /mnt/maps/cardinal data; run in release"]
fn benchmark_king_county_run_of_rounds() {
    let costing = costing_named("pedestrian");
    let (provider, snaps, _stops) = wa_fixture(&costing);
    let stop_count = snaps.stop_count();
    let mut search = WalkSearch::new(provider, costing);
    search.bind(snaps).expect("bind");
    for (name, bound) in [
        ("no bound", None),
        ("bound 1 h", Some(3600)),
        ("bound 30 min", Some(1800)),
    ] {
        for run_pruning in [false, true] {
            let (ms, settled, arena_last) =
                bench_run_rounds(&mut search, stop_count, run_pruning, bound, 11);
            println!(
                "6-round run, {name}, run_pruning={run_pruning}: median {ms:.1} ms per run, \
                 {settled:.0} nodes settled per run, {arena_last:.0} arena entries at the last round"
            );
        }
    }
    println!(
        "working memory after runs: {} MB",
        search.working_memory_bytes() / (1024 * 1024)
    );
}

#[test]
#[ignore = "needs /mnt/maps/cardinal data; run in release"]
fn benchmark_king_county_access_tables() {
    let costing = costing_named("pedestrian");
    let (provider, snaps, stops) = wa_fixture(&costing);
    let stop_count = snaps.stop_count();
    let mut tables = StopTables::new(provider, costing.clone());
    tables.bind(snaps.clone()).expect("bind");
    let mut search_snaps = Vec::new();
    for i in (0..stop_count).step_by(47) {
        if snaps.point(StopId::new(i as u32)).is_some() {
            search_snaps.push(snaps.point(StopId::new(i as u32)).expect("point"));
        }
    }
    for (side, label) in [
        (AccessSide::Access, "access"),
        (AccessSide::Egress, "egress"),
    ] {
        let mut times = Vec::new();
        let (mut reached, mut settled) = (0.0, 0.0);
        for point in search_snaps.iter().take(60) {
            let seeds = [TableSeed {
                edge: point.edge,
                percent_along: point.percent_along,
                initial: 0.0,
            }];
            let request = TableRequest {
                side,
                seeds: &seeds,
                max_seconds: 1800,
                interrupt: &|| false,
            };
            let started = std::time::Instant::now();
            tables.compute(stop_count, &request).expect("table");
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            reached += tables.stats().stops_reached as f64;
            settled += tables.stats().labels_settled as f64;
        }
        let n = times.len() as f64;
        let (median, p95) = median_p95(&mut times);
        println!(
            "{label} table (30 min walk, {n:.0} origins): median {median:.1} ms, p95 {p95:.1} ms, mean labels settled {:.0}, mean stops reached {:.0}, memory {} MB",
            settled / n,
            reached / n,
            tables.stats().bytes / (1024 * 1024)
        );
    }
    let _ = stops;
}

#[test]
fn run_pruning_keeps_every_improving_arrival_across_rounds() {
    // Rounds of a run: each round's seeds are later than the last, so a walk
    // reaching a stop no earlier than a previous round did can be dropped.
    for dir in [Direction::Forward, Direction::Backward] {
        let provider = andorra();
        let points = random_stops(&provider, &ANDORRA_TILES, 250, 17);
        let snaps = snaps_for(&provider, &points);
        let mut pruned = WalkSearch::new(provider, pedestrian());
        pruned.bind(snaps.clone()).expect("bind");
        let provider = andorra();
        let mut unpruned = WalkSearch::new(provider, pedestrian());
        unpruned.set_run_pruning(false);
        unpruned.bind(snaps).expect("bind");
        pruned.begin_run(dir);
        unpruned.begin_run(dir);

        let (mut best_p, mut best_u) = (HashMap::<u32, i32>::new(), HashMap::<u32, i32>::new());
        let rounds: [(&[u32], i32); 4] = [
            (&[1, 2, 3, 4, 5], 1000),
            (&[10, 11, 12], 400),
            (&[20, 21, 22, 23], 2100),
            (&[1, 30, 31], 0),
        ];
        for (ids, base) in rounds {
            let seeds: Vec<TransferSeed> = ids
                .iter()
                .enumerate()
                .map(|(i, &s)| TransferSeed {
                    stop: StopId::new(s),
                    clock: Clock::new(base + (i as i32) * 50),
                })
                .collect();
            let request = TransferRequest {
                direction: dir,
                seeds: &seeds,
                upper_bound: Clock::new(i32::MAX / 2),
                change_margin: TransferRequest::REPORT_ALL,
                rules: WalkRules::none(),
                interrupt: &|| false,
            };
            let mut sink_p = Sink::default();
            let mut sink_u = Sink::default();
            pruned.search(&request, &mut sink_p).expect("search");
            unpruned.search(&request, &mut sink_u).expect("search");
            // Keep the best seen so far for each stop, as a router's `board` does.
            for (stop, (clock, ..)) in sink_p.best {
                let e = best_p.entry(stop).or_insert(clock);
                *e = (*e).min(clock);
            }
            for (stop, (clock, ..)) in sink_u.best {
                let e = best_u.entry(stop).or_insert(clock);
                *e = (*e).min(clock);
            }
            assert_eq!(best_p, best_u, "round at base {base}, {dir:?}");
        }
        assert!(!best_p.is_empty());
    }
}

/// Replays rounds of a run, keeping each stop's boarding clock as a router's `board` does,
/// with seeds able to board again after their own minimum change time.
fn board_clocks_of_rounds<P: GraphTileProvider, C: Costing>(
    search: &mut WalkSearch<P, C>,
    dir: Direction,
    margin: i32,
    change: i32,
    rounds: &[(Vec<u32>, i32)],
) -> HashMap<u32, i32> {
    search.begin_run(dir);
    let mut board = HashMap::<u32, i32>::new();
    for (ids, base) in rounds {
        let seeds: Vec<TransferSeed> = ids
            .iter()
            .enumerate()
            .map(|(i, &s)| TransferSeed {
                stop: StopId::new(s),
                clock: Clock::new(base + (i as i32) * 37),
            })
            .collect();
        for seed in &seeds {
            let at = seed.clock.seconds() + change;
            let entry = board.entry(seed.stop.into_inner()).or_insert(at);
            *entry = (*entry).min(at);
        }
        let request = TransferRequest {
            direction: dir,
            seeds: &seeds,
            upper_bound: Clock::new(i32::MAX / 2),
            change_margin: margin,
            rules: WalkRules::none(),
            interrupt: &|| false,
        };
        let mut sink = Sink::default();
        search.search(&request, &mut sink).expect("search");
        for (stop, (clock, ..)) in sink.best {
            let entry = board.entry(stop).or_insert(clock);
            *entry = (*entry).min(clock);
        }
    }
    board
}

#[test]
fn the_change_margin_never_changes_what_can_be_boarded() {
    let rounds: Vec<(Vec<u32>, i32)> = vec![
        ((0..80).step_by(3).collect(), 1000),
        ((1..120).step_by(5).collect(), 700),
        ((2..200).step_by(7).collect(), 1900),
        ((40..60).collect(), 300),
    ];
    for dir in [Direction::Forward, Direction::Backward] {
        for change in [0, 45, 300] {
            let provider = andorra();
            let points = random_stops(&provider, &ANDORRA_TILES, 250, 29);
            let snaps = snaps_for(&provider, &points);
            let mut pruned = WalkSearch::new(andorra(), pedestrian());
            pruned.bind(snaps.clone()).expect("bind");
            let mut reference = WalkSearch::new(andorra(), pedestrian());
            reference.set_own_seed_pruning(false);
            reference.bind(snaps).expect("bind");
            let got = board_clocks_of_rounds(&mut pruned, dir, change, change, &rounds);
            let expected = board_clocks_of_rounds(
                &mut reference,
                dir,
                TransferRequest::REPORT_ALL,
                change,
                &rounds,
            );
            assert_eq!(got, expected, "{dir:?}, change {change}");
        }
    }
}

/// Compares a search that ends once every stop is final with a full flood, on every entry.
fn check_ending_early_changes_no_entry(costing: AnyCosting) -> u32 {
    let provider = andorra();
    let mut points = random_stops(&provider, &ANDORRA_TILES, 300, 21);
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
    let stop_count = snaps.stop_count();
    let mut probe = StopTables::new(andorra(), costing.clone());
    probe.bind(snaps).expect("bind");
    let from_origin = access_table_for(&mut probe, AccessSide::Access, points[0], stop_count);
    let to_origin = access_table_for(&mut probe, AccessSide::Egress, points[0], stop_count);
    let reached: Vec<SnapPoint> = (0..stop_count)
        .filter(|&i| {
            let stop = StopId::new(i as u32);
            from_origin.cost(stop).is_some() && to_origin.cost(stop).is_some()
        })
        .map(|i| points[i])
        .collect();
    // Stops on each origin's own edge, in front of it and behind it. A seed's partial label
    // settles first, and on a one-way edge only a walk coming back around reaches those behind.
    let origins: Vec<SnapPoint> = reached
        .iter()
        .step_by((reached.len() / 40).max(1))
        .copied()
        .collect();
    let mut kept = reached;
    for origin in &origins {
        for percent_along in [0.02, 0.2, 0.5, 0.8, 0.98] {
            kept.push(SnapPoint {
                percent_along,
                ..*origin
            });
        }
    }
    let snaps = snaps_for(&provider, &kept);
    let count = snaps.stop_count();
    let mut early = StopTables::new(andorra(), costing.clone());
    early.bind(snaps.clone()).expect("bind");
    let mut full = StopTables::new(andorra(), costing);
    full.set_terminate_when_stops_settled(false);
    full.bind(snaps).expect("bind");

    let mut ended_early = 0;
    for side in [AccessSide::Access, AccessSide::Egress] {
        for origin in &origins {
            let seeds = [TableSeed {
                edge: origin.edge,
                percent_along: origin.percent_along,
                initial: 0.0,
            }];
            let request = TableRequest {
                side,
                seeds: &seeds,
                max_seconds: 86_400,
                interrupt: &|| false,
            };
            let got = early
                .compute(count, &request)
                .expect("table")
                .expect("finished");
            let expected = full
                .compute(count, &request)
                .expect("table")
                .expect("finished");
            ended_early += u32::from(early.stats().terminated_early);
            for i in 0..count {
                let stop = StopId::new(i as u32);
                assert_eq!(got.cost(stop), expected.cost(stop), "{side:?}, stop {i}");
            }
        }
    }
    ended_early
}

#[test]
fn ending_a_table_search_early_changes_no_entry() {
    let walking = check_ending_early_changes_no_entry(pedestrian());
    let cycling = check_ending_early_changes_no_entry(bicycle());
    println!("ended early: {walking} walking, {cycling} cycling");
    assert!(
        walking > 0,
        "no walking search ended early, so this tested nothing"
    );
}

/// Stops a bicycle reaches from SeaTac/Airport station's southbound platform (stop 99904),
/// as a table of walks from it, with the stops snapped by `config`.
///
/// The second stop is on Air Cargo Road at the main terminal, where Cardinal's geocoder puts it.
fn seatac_rides_from_the_platform(config: crate::SnapConfig) -> (SnapPoint, Option<u32>) {
    let provider = tar_provider(WA_TILES);
    let costing = bicycle();
    let stops = [(-122.297_03, 47.444_97), (-122.301_326_5, 47.442_500_6)];
    let probe = crate::snap_stop(&provider, &costing, &config, stops[0]).expect("snap");
    let build = probe.build_id.expect("build");
    let snaps =
        Arc::new(crate::snap_stops(&provider, &costing, &config, build, &stops).expect("snap"));
    let mut tables = StopTables::new(provider, costing);
    tables.bind(snaps.clone()).expect("bind");
    let platform = StopId::new(0);
    let seeds = tables.stop_seeds(platform).expect("seeds");
    let table = tables
        .compute(
            snaps.stop_count(),
            &crate::TableRequest {
                side: AccessSide::Access,
                seeds: &seeds,
                max_seconds: 1800,
                interrupt: &|| false,
            },
        )
        .expect("search")
        .expect("not interrupted");
    (
        snaps.point(platform).expect("snapped"),
        table
            .cost(StopId::new(1))
            .map(valinor_transit::Seconds::into_inner),
    )
}

/// By bicycle, the platform's nearest edge that reaches 50 nodes each way is in the airport
/// garage, which a bicycle can enter but not leave: no ride could end there, so Link riders
/// to the airport were sent to Tukwila International Boulevard to cycle 4.5 km instead.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn a_bicycle_can_ride_away_from_seatac_airport_station() {
    let old = crate::SnapConfig {
        min_reachability: 50,
        ..crate::SnapConfig::default()
    };
    let (garage, stranded) = seatac_rides_from_the_platform(old);
    println!("reach 50: {garage:?}, terminal {stranded:?}");
    assert_eq!(
        stranded, None,
        "the garage has a way out: the fixture moved"
    );

    let (snap, terminal) = seatac_rides_from_the_platform(crate::SnapConfig::default());
    println!("default: {snap:?}, terminal {terminal:?}");
    assert_ne!(snap.edge, garage.edge);
    // International Boulevard, then S 170th St and Air Cargo Road: about 2.4 km.
    let seconds = terminal.expect("the terminal is reachable from the platform");
    assert!(
        (300..=700).contains(&seconds),
        "{seconds} s to the terminal"
    );
}
