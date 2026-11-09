//! Transfer tables on real Washington tiles and stops.
//!
//! Needs `/mnt/maps/cardinal` data; run with `-- --ignored` in a release build.
#![allow(clippy::all, clippy::pedantic)]

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use valinor_router::tiles::TileSets;
use valinor_sif::AnyCosting;
use valinor_sif::costing_options::parse_costing;
use valinor_sif::json::JsonValue;
use valinor_transit::StopId;
use valinor_walk::{
    AccessSide, SnapConfig, StopAttachments, StopSnaps, StopTables, TableRequest, TableSeed,
    TransferTable, snap_stop, snap_stops,
};

const TAR: &str = "/mnt/maps/cardinal/routing-elevation-i2b/washington/tiles.tar";
const METRO: &str =
    "/mnt/maps/cardinal/transit/global/us-wa-2026-11-09/timetables/us-wa-Metro-Transit.timetable";

fn costing(name: &str) -> AnyCosting {
    use valhalla_proto::costing::Type;
    let kind = if name == "bicycle" {
        Type::Bicycle
    } else {
        Type::Pedestrian
    };
    let (json, key): (&[u8], &str) = if name == "wheelchair" {
        (br#"{"o":{"type":"wheelchair"}}"#, "/o")
    } else {
        (b"{}", "/none")
    };
    let doc = JsonValue::parse(json).expect("doc");
    let mut proto = valhalla_proto::Costing::default();
    parse_costing(&doc, key, &mut proto, &mut Vec::new(), Some(kind)).expect("costing");
    AnyCosting::new(&proto).expect("any")
}

const TIMETABLES: &str = "/mnt/maps/cardinal/transit/global/us-wa-2026-11-09/timetables";

fn tiles() -> TileSets {
    TileSets::open(&[PathBuf::from(TAR)], NonZeroUsize::new(64).unwrap()).expect("tiles")
}

/// Metro stops in greater Seattle, in an order that mixes the neighborhoods.
fn points(limit: usize) -> Vec<(f64, f64)> {
    let bytes = std::fs::read(METRO).expect("timetable");
    let timetable = valinor_gtfs::CompiledTimetable::parse(&bytes).expect("parse");
    let mut out: Vec<(f64, f64)> = (0..timetable.stop_count() as u32)
        .filter_map(|s| timetable.stop(StopId::new(s)).and_then(|i| i.coordinate))
        .map(|c| (c.lon, c.lat))
        .filter(|&(lon, lat)| (47.55..47.68).contains(&lat) && (-122.40..-122.27).contains(&lon))
        .collect();
    let mut state = 0x1234_5678_u64;
    for i in (1..out.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        out.swap(i, (state >> 33) as usize % (i + 1));
    }
    out.truncate(limit);
    out
}

fn snapped(p: &&TileSets, c: &AnyCosting, pts: &[(f64, f64)]) -> Arc<StopSnaps> {
    Arc::new(snap_stops(p, c, &SnapConfig::default(), p.build_id(), pts).expect("snap"))
}

fn triples(table: &TransferTable) -> Vec<(u32, u32, u32)> {
    let mut all = Vec::new();
    for from in 0..table.stop_count() as u32 {
        for (to, s) in table.walks_from(StopId::new(from)) {
            all.push((from, to.into_inner(), s));
        }
    }
    all.sort_unstable();
    all
}

fn built(p: &&TileSets, c: &AnyCosting, s: &Arc<StopSnaps>) -> TransferTable {
    TransferTable::build(p, c, s, 900, NonZeroUsize::new(16).unwrap(), &|| false)
        .expect("build")
        .expect("done")
}

/// One side of a walk between two stops, as far as this file compares them.
#[derive(Clone, Debug, PartialEq)]
struct Found {
    secs: f32,
    cost: f32,
    edges: Vec<u64>,
    /// Each edge's `(transition cost, traversal cost)`, in travel order.
    parts: Vec<(f32, f32)>,
}

fn found(leg: &valinor_walk::AccessLeg) -> Found {
    Found {
        secs: leg.secs,
        cost: leg
            .edges
            .iter()
            .map(|e| e.transition.cost + e.traversal.cost)
            .sum(),
        edges: leg.edges.iter().map(|e| e.edge.value()).collect(),
        parts: leg
            .edges
            .iter()
            .map(|e| (e.transition.cost, e.traversal.cost))
            .collect(),
    }
}

/// How a forward and a backward walk between the same two stops relate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Agreement {
    /// The same route and the same seconds, to the rounding of the table.
    Same,
    /// The same route and costs; float noise in the seconds, within or across a whole second.
    Rounding,
    /// As `Rounding`, but the noise moved the seconds across a whole-second rounding edge.
    RoundsDifferently,
    /// Different routes of equal cost: a tie, broken differently by each direction.
    EqualCost,
    /// Different routes whose costs differ by less than a tenth of a second of cost: a tie
    /// but for accumulated float noise.
    NearlyEqualCost,
    /// The same route with a different cost, so Sif's forward and reverse charge it differently.
    CostOfSameRoute,
    /// Different routes of different cost.
    DifferentRoute,
}

fn classify(forward: &Found, backward: &Found) -> Agreement {
    let cost_off = (forward.cost - backward.cost).abs();
    if forward.edges == backward.edges {
        if cost_off < 0.01 {
            if forward.secs.round() == backward.secs.round() {
                return if forward.secs == backward.secs {
                    Agreement::Same
                } else {
                    Agreement::Rounding
                };
            }
            return Agreement::RoundsDifferently;
        }
        return Agreement::CostOfSameRoute;
    }
    if cost_off < 1e-3 {
        Agreement::EqualCost
    } else if cost_off < 0.1 {
        Agreement::NearlyEqualCost
    } else {
        Agreement::DifferentRoute
    }
}

/// `with_new_stops` adds what a full build would, with each base stop's walk into a new stop
/// from that new stop's backward search: any difference from a full forward build must be one
/// of those walks, and must be what that backward search finds.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn new_stops_match_a_full_build_on_real_streets() {
    use std::collections::BTreeMap;
    let sets = tiles();
    let p = &sets;
    let mut failures = Vec::new();
    for name in ["pedestrian", "bicycle"] {
        let c = costing(name);
        let pts = points(1400);
        let (n, k) = (pts.len(), 200);
        let all = snapped(&p, &c, &pts);
        let base = snapped(&p, &c, &pts[..n - k]);
        let new: Vec<StopId> = ((n - k) as u32..n as u32).map(StopId::new).collect();
        let full = triples(&built(&p, &c, &all));
        let base_table = built(&p, &c, &base);
        let started = Instant::now();
        let grown = base_table
            .with_new_stops(&p, &c, &all, &new, &|| false)
            .expect("grow")
            .expect("done");
        let added_in = started.elapsed().as_secs_f64();
        let grown = triples(&grown);
        let full_map: BTreeMap<(u32, u32), u32> =
            full.iter().map(|&(a, b, s)| ((a, b), s)).collect();
        let grown_map: BTreeMap<(u32, u32), u32> =
            grown.iter().map(|&(a, b, s)| ((a, b), s)).collect();
        // What the backward search from each new stop finds, to explain any difference.
        let mut backward = StopTables::new(p, c.clone());
        backward.bind(Arc::clone(&all)).expect("bind");
        let mut backward_found: BTreeMap<(u32, u32), Found> = BTreeMap::new();
        for &stop in &new {
            let seeds = backward.stop_seeds(stop).expect("seeds");
            if seeds.is_empty() {
                continue;
            }
            assert!(
                backward
                    .search(&TableRequest {
                        side: AccessSide::Egress,
                        seeds: &seeds,
                        max_seconds: 900,
                        interrupt: &|| false,
                    })
                    .expect("search")
            );
            for (from, _) in backward.reached_stops().collect::<Vec<_>>() {
                if from == stop {
                    continue;
                }
                let leg = backward.leg(from).expect("leg");
                backward_found.insert((from.into_inner(), stop.into_inner()), found(&leg));
            }
        }
        let is_new = |s: u32| s as usize >= n - k;
        let mut unexplained: Vec<((u32, u32), Option<u32>, Option<u32>)> = Vec::new();
        let (mut base_to_new_diff, mut base_to_new_total) = (0_usize, 0_usize);
        let (mut new_origin_diff, mut new_origin_total) = (0_usize, 0_usize);
        let mut keys: Vec<_> = full_map.keys().chain(grown_map.keys()).copied().collect();
        keys.sort_unstable();
        keys.dedup();
        for key in keys {
            let (f, g) = (full_map.get(&key).copied(), grown_map.get(&key).copied());
            if !is_new(key.0) && is_new(key.1) {
                base_to_new_total += 1;
                let backward_secs = backward_found.get(&key).map(|b| b.secs.round() as u32);
                if f != g {
                    base_to_new_diff += 1;
                }
                if g != backward_secs {
                    unexplained.push((key, f, g));
                }
            } else {
                if is_new(key.0) {
                    new_origin_total += 1;
                    if f != g {
                        new_origin_diff += 1;
                    }
                }
                if f != g {
                    unexplained.push((key, f, g));
                }
            }
        }
        eprintln!(
            "{name}: added {k} new stops in {added_in:.2} s; new-stop origins {new_origin_total} \
             walks, {new_origin_diff} differ; base-to-new {base_to_new_total} walks, \
             {base_to_new_diff} differ from a full forward build (all from the backward search); \
             unexplained {}: {:?}",
            unexplained.len(),
            &unexplained[..unexplained.len().min(12)]
        );
        failures.push((name, unexplained.len()));
    }
    assert!(failures.iter().all(|f| f.1 == 0), "{failures:?}");
}

/// A forward and a backward search between each pair of stops: how they differ, and why.
///
/// `PROOF_OUT` writes each differing pair to a file.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn forward_and_backward_agree_on_real_streets() {
    use std::collections::{BTreeMap, HashMap};
    use std::io::Write as _;
    let sets = tiles();
    let p = &sets;
    let mut out = std::env::var("PROOF_OUT")
        .ok()
        .map(|path| std::fs::File::create(path).expect("output"));
    for name in ["pedestrian", "bicycle"] {
        let c = costing(name);
        let pts = points(700);
        let snaps = snapped(&p, &c, &pts);
        let mut tables = StopTables::new(p, c.clone());
        tables.bind(Arc::clone(&snaps)).expect("bind");
        let mut forward: HashMap<(u32, u32), Found> = HashMap::new();
        for from in 0..snaps.stop_count() as u32 {
            let id = StopId::new(from);
            let seeds = tables.stop_seeds(id).expect("seeds");
            if seeds.is_empty() {
                continue;
            }
            assert!(
                tables
                    .search(&TableRequest {
                        side: AccessSide::Access,
                        seeds: &seeds,
                        max_seconds: 900,
                        interrupt: &|| false,
                    })
                    .expect("forward")
            );
            for (to, _) in tables.reached_stops().collect::<Vec<_>>() {
                if to != id {
                    let leg = tables.leg(to).expect("leg");
                    forward.insert((from, to.into_inner()), found(&leg));
                }
            }
        }
        let mut counts: BTreeMap<Agreement, u64> = BTreeMap::new();
        let (mut only_forward, mut only_backward, mut compared) = (0_u64, 0_u64, 0_u64);
        let mut seen = HashMap::new();
        for to in 0..snaps.stop_count() as u32 {
            let id = StopId::new(to);
            let seeds = tables.stop_seeds(id).expect("seeds");
            if seeds.is_empty() {
                continue;
            }
            assert!(
                tables
                    .search(&TableRequest {
                        side: AccessSide::Egress,
                        seeds: &seeds,
                        max_seconds: 900,
                        interrupt: &|| false,
                    })
                    .expect("backward")
            );
            for (from, _) in tables.reached_stops().collect::<Vec<_>>() {
                if from == id {
                    continue;
                }
                let key = (from.into_inner(), to);
                let leg = tables.leg(from).expect("leg");
                let backward = found(&leg);
                seen.insert(key, ());
                match forward.get(&key) {
                    None => {
                        only_backward += 1;
                        if let Some(out) = out.as_mut() {
                            writeln!(out, "{name}\t{key:?}\tonly_backward\t{}", backward.secs)
                                .expect("write");
                        }
                    }
                    Some(forward) => {
                        compared += 1;
                        let verdict = classify(forward, &backward);
                        *counts.entry(verdict).or_default() += 1;
                        if verdict != Agreement::Same
                            && let Some(out) = out.as_mut()
                        {
                            writeln!(
                                out,
                                "{name}\t{key:?}\t{verdict:?}\tf_secs={} b_secs={} f_cost={} \
                                 b_cost={} f_edges={} b_edges={} f={:?} b={:?} fp={:?} bp={:?}",
                                forward.secs,
                                backward.secs,
                                forward.cost,
                                backward.cost,
                                forward.edges.len(),
                                backward.edges.len(),
                                forward.edges,
                                backward.edges,
                                forward.parts,
                                backward.parts,
                            )
                            .expect("write");
                        }
                    }
                }
            }
        }
        for key in forward.keys() {
            if !seen.contains_key(key) {
                only_forward += 1;
                if let Some(out) = out.as_mut() {
                    writeln!(out, "{name}\t{key:?}\tonly_forward\t{}", forward[key].secs)
                        .expect("write");
                }
            }
        }
        eprintln!(
            "{name}: compared {compared}, only forward {only_forward}, only backward \
             {only_backward}; {counts:?}"
        );
        let unexplained = counts.get(&Agreement::DifferentRoute).copied().unwrap_or(0)
            + counts
                .get(&Agreement::CostOfSameRoute)
                .copied()
                .unwrap_or(0)
            + only_forward
            + only_backward;
        assert_eq!(unexplained, 0, "{name}: unexplained differences");
    }
}

/// The table entry's route, drawn again by a search stopped at its target.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn a_draw_is_the_tables_route_on_real_streets() {
    let sets = tiles();
    let p = &sets;
    for name in ["pedestrian", "bicycle"] {
        let c = costing(name);
        let pts = points(700);
        let snaps = snapped(&p, &c, &pts);
        let mut table_search = StopTables::new(p, c.clone());
        let mut draw = StopTables::new(p, c.clone());
        table_search.bind(Arc::clone(&snaps)).expect("bind");
        draw.bind(Arc::clone(&snaps)).expect("bind");
        let (mut checked, mut missing, mut secs_off, mut edges_off) = (0, 0, 0, 0);
        let mut drawing = 0.0_f64;
        for from in 0..snaps.stop_count() as u32 {
            let from_id = StopId::new(from);
            let seeds = table_search.stop_seeds(from_id).expect("seeds");
            if seeds.is_empty() {
                continue;
            }
            let request = TableRequest {
                side: AccessSide::Access,
                seeds: &seeds,
                max_seconds: 900,
                interrupt: &|| false,
            };
            assert!(table_search.search(&request).expect("search"));
            let reached: Vec<(StopId, u32)> = table_search
                .reached_stops()
                .filter(|&(to, _)| to != from_id)
                .map(|(to, s)| (to, s.into_inner()))
                .collect();
            for (i, (to, seconds)) in reached.into_iter().enumerate() {
                // Bicycles reach many more stops.
                if i % (if name == "bicycle" { 27 } else { 9 }) != 0 {
                    continue;
                }
                let leg = table_search.leg(to).expect("table leg");
                let started = Instant::now();
                assert!(draw.search_to_stop(&request, to).expect("draw"));
                drawing += started.elapsed().as_secs_f64();
                checked += 1;
                let Some(drawn) = draw.leg(to) else {
                    missing += 1;
                    continue;
                };
                if drawn.secs.round() as u32 != seconds {
                    secs_off += 1;
                }
                let a: Vec<_> = leg.edges.iter().map(|e| e.edge).collect();
                let b: Vec<_> = drawn.edges.iter().map(|e| e.edge).collect();
                if a != b {
                    edges_off += 1;
                }
            }
        }
        eprintln!(
            "{name}: draws {checked}, not drawn {missing}, seconds differ {secs_off}, \
             route differs {edges_off}; per draw {:.2} ms",
            1000.0 * drawing / checked.max(1) as f64
        );
        assert_eq!((missing, secs_off, edges_off), (0, 0, 0), "{name}");
    }
}

/// The build snaps in batches, the planner in one list, and an overlay one by one.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn snapping_does_not_depend_on_the_batch() {
    let sets = tiles();
    let p = &sets;
    let c = costing("pedestrian");
    let pts = points(2000);
    let whole = snapped(&p, &c, &pts);
    let mut differ = 0;
    for (i, chunk) in pts.chunks(137).enumerate() {
        let part = snapped(&p, &c, chunk);
        for j in 0..chunk.len() {
            let global = StopId::new((i * 137 + j) as u32);
            if whole.point(global) != part.point(StopId::new(j as u32)) {
                differ += 1;
            }
        }
    }
    let mut one_differ = 0;
    for (i, &ll) in pts.iter().enumerate().step_by(5) {
        let single = snap_stop(&p, &c, &SnapConfig::default(), ll).expect("snap one");
        if single.result.ok() != whole.point(StopId::new(i as u32)) {
            one_differ += 1;
        }
    }
    eprintln!("batch differ {differ}, single differ {one_differ}");
    assert_eq!((differ, one_differ), (0, 0));
}

/// Every Washington stop, in one list.
fn washington_points() -> Vec<(f64, f64)> {
    let mut files: Vec<_> = std::fs::read_dir(TIMETABLES)
        .expect("timetables")
        .map(|entry| entry.expect("entry").path())
        .collect();
    files.sort();
    let mut out = Vec::new();
    for file in files {
        let bytes = std::fs::read(&file).expect("timetable");
        let timetable = valinor_gtfs::CompiledTimetable::parse(&bytes).expect("parse");
        out.extend(
            (0..timetable.stop_count() as u32)
                .filter_map(|s| timetable.stop(StopId::new(s)).and_then(|i| i.coordinate))
                .map(|c| (c.lon, c.lat)),
        );
    }
    out
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * fraction) as usize]
}

/// A mode's whole-route cost over time, over real routes in Washington, for its table bound.
///
/// `max_cost = max_seconds * factor(mode)` should cover the 95th percentile of cost over time,
/// so that a stop's cheapest route usually isn't cut off by the bound. The search here runs with
/// a very large factor, so a costly route is measured instead of censored, and every route of
/// at least a minute is counted.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn route_cost_over_time_on_real_streets() {
    let sets = tiles();
    let p = &sets;
    let all = washington_points();
    let mut sample: Vec<u32> = (0..all.len() as u32).collect();
    let mut state = 0x1234_5678_u64;
    for i in (1..sample.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        sample.swap(i, (state >> 33) as usize % (i + 1));
    }
    sample.truncate(300);
    for name in ["pedestrian", "bicycle", "wheelchair"] {
        let c = costing(name);
        let snaps = snapped(&p, &c, &all);
        let mut tables = StopTables::new(p, c.clone());
        tables.bind(Arc::clone(&snaps)).expect("bind");
        tables.set_cost_factor_override(Some(100.0));
        let mut ratios: Vec<f64> = Vec::new();
        for &stop in &sample {
            let seeds = tables.stop_seeds(StopId::new(stop)).expect("seeds");
            if seeds.is_empty() {
                continue;
            }
            tables
                .search(&TableRequest {
                    side: AccessSide::Access,
                    seeds: &seeds,
                    max_seconds: 900,
                    interrupt: &|| false,
                })
                .expect("search");
            let reached: Vec<StopId> = tables.reached_stops().map(|(to, _)| to).collect();
            for to in reached {
                let leg = tables.leg(to).expect("leg");
                if leg.secs < 60.0 {
                    continue;
                }
                let cost: f32 = leg
                    .edges
                    .iter()
                    .map(|e| e.transition.cost + e.traversal.cost)
                    .sum();
                ratios.push(f64::from(cost / leg.secs));
            }
        }
        ratios.sort_unstable_by(f64::total_cmp);
        eprintln!(
            "{name}: routes {}, cost over time p50 {:.3}, p95 {:.3}, p99 {:.3}, max {:.3}",
            ratios.len(),
            percentile(&ratios, 0.5),
            percentile(&ratios, 0.95),
            percentile(&ratios, 0.99),
            ratios.last().copied().unwrap_or(0.0),
        );
    }
}

/// Search cost on real stops: access, egress, and transfer, warm and cold, walking and cycling.
///
/// Prints one line per case; `TABLE_BENCH_OUT` also writes them to a file.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn table_search_costs_on_real_streets() {
    use std::io::Write as _;
    let sets = tiles();
    let p = &sets;
    let mut out = std::env::var("TABLE_BENCH_OUT")
        .ok()
        .map(|path| std::fs::File::create(path).expect("output"));
    let mut report = |line: String| {
        eprintln!("{line}");
        if let Some(out) = out.as_mut() {
            writeln!(out, "{line}").expect("write");
        }
    };
    report("mode,side,limit,phase,n,p50_ms,p95_ms,mean_labels,mean_stops,terminated_early".into());
    let all = washington_points();
    // Query stops in greater Seattle, shuffled, the same on every run.
    let mut seattle: Vec<u32> = (0..all.len() as u32)
        .filter(|&i| {
            let (lon, lat) = all[i as usize];
            (47.55..47.68).contains(&lat) && (-122.40..-122.27).contains(&lon)
        })
        .collect();
    let mut state = 0x1234_5678_u64;
    for i in (1..seattle.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        seattle.swap(i, (state >> 33) as usize % (i + 1));
    }
    seattle.truncate(230);
    // Places a little off the stops, for access and egress, snapped as a query's would be.
    let offset: Vec<(f64, f64)> = seattle
        .iter()
        .map(|&i| {
            let (lon, lat) = all[i as usize];
            (lon + 0.0004, lat - 0.0003)
        })
        .collect();
    for name in ["pedestrian", "bicycle"] {
        let c = costing(name);
        let snaps = snapped(&p, &c, &all);
        let attachments =
            Arc::new(StopAttachments::new(p, c.clone(), Arc::clone(&snaps)).expect("attach"));
        let place_snaps = snapped(&p, &c, &offset);
        let mut places = StopTables::new(p, c.clone());
        places.bind(place_snaps).expect("bind");
        let mut stops = StopTables::new(p, c.clone());
        stops.bind_shared(Arc::clone(&attachments));
        let mut queries: Vec<(Vec<TableSeed>, Vec<TableSeed>)> = Vec::new();
        for (index, &stop) in seattle.iter().enumerate() {
            let at_place = places
                .stop_seeds(StopId::new(index as u32))
                .expect("place seeds");
            let at_stop = stops.stop_seeds(StopId::new(stop)).expect("stop seeds");
            if !at_place.is_empty() && !at_stop.is_empty() {
                queries.push((at_place, at_stop));
            }
        }
        let sides = [
            ("access", AccessSide::Access, false),
            ("egress", AccessSide::Egress, false),
            ("transfer", AccessSide::Access, true),
            ("draw", AccessSide::Access, true),
        ];
        for limit in [900_u32, 1800] {
            for &(kind, side, at_stop) in &sides {
                let seeds_of = |q: &(Vec<TableSeed>, Vec<TableSeed>)| -> Vec<TableSeed> {
                    if at_stop { q.1.clone() } else { q.0.clone() }
                };
                let run = |tables: &mut StopTables<&TileSets, AnyCosting>,
                           q: &(Vec<TableSeed>, Vec<TableSeed>)|
                 -> (f64, u64, usize, bool) {
                    let seeds = seeds_of(q);
                    let request = TableRequest {
                        side,
                        seeds: &seeds,
                        max_seconds: limit,
                        interrupt: &|| false,
                    };
                    let target = (kind == "draw").then(|| {
                        assert!(tables.search(&request).expect("table"));
                        let mut reached: Vec<_> = tables.reached_stops().collect();
                        reached.sort_by_key(|&(_, secs)| secs.into_inner());
                        reached.get(reached.len() / 2).map(|&(stop, _)| stop)
                    });
                    if target == Some(None) {
                        return (0.0, 0, 0, false);
                    }
                    let started = Instant::now();
                    match target.flatten() {
                        Some(stop) => {
                            assert!(tables.search_to_stop(&request, stop).expect("draw"));
                        }
                        None => assert!(tables.search(&request).expect("search")),
                    }
                    let ms = started.elapsed().as_secs_f64() * 1000.0;
                    (
                        ms,
                        tables.stats().labels_settled,
                        tables.reached_stops().count(),
                        tables.stats().terminated_early,
                    )
                };
                let mut warm = StopTables::new(p, c.clone());
                warm.bind_shared(Arc::clone(&attachments));
                for q in queries.iter().take(30) {
                    run(&mut warm, q);
                }
                let warm_runs: Vec<_> = queries.iter().map(|q| run(&mut warm, q)).collect();
                let cold_runs: Vec<_> = queries
                    .iter()
                    .take(40)
                    .map(|q| {
                        let mut fresh = StopTables::new(p, c.clone());
                        fresh.bind_shared(Arc::clone(&attachments));
                        run(&mut fresh, q)
                    })
                    .collect();
                for (phase, runs) in [("warm", &warm_runs), ("cold", &cold_runs)] {
                    let mut ms: Vec<f64> = runs.iter().map(|r| r.0).collect();
                    ms.sort_by(f64::total_cmp);
                    let n = runs.len() as f64;
                    report(format!(
                        "{name},{kind},{limit},{phase},{},{:.3},{:.3},{:.0},{:.0},{}",
                        runs.len(),
                        percentile(&ms, 0.5),
                        percentile(&ms, 0.95),
                        runs.iter().map(|r| r.1 as f64).sum::<f64>() / n,
                        runs.iter().map(|r| r.2 as f64).sum::<f64>() / n,
                        runs.iter().filter(|r| r.3).count(),
                    ));
                }
            }
        }
    }
}

/// What adding one GTFS-RT new stop costs, for real stops in a statewide table.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn new_stop_cost_on_real_streets() {
    let sets = tiles();
    let p = &sets;
    let all = washington_points();
    let picks: Vec<u32> = (0..all.len() as u32)
        .filter(|&i| {
            let (lon, lat) = all[i as usize];
            (47.55..47.68).contains(&lat) && (-122.40..-122.27).contains(&lon)
        })
        .step_by(400)
        .take(4)
        .collect();
    let base = TransferTable::from_sorted_walks(all.len(), 900, &[]).expect("empty table");
    for name in ["pedestrian", "bicycle"] {
        let c = costing(name);
        let snaps = snapped(&p, &c, &all);
        for &stop in &picks {
            let started = Instant::now();
            let grown = base
                .with_new_stops(&p, &c, &snaps, &[StopId::new(stop)], &|| false)
                .expect("grow")
                .expect("done");
            eprintln!(
                "new-stop,{name},{stop},{:.1} ms,{} walks",
                started.elapsed().as_secs_f64() * 1000.0,
                grown.walk_count()
            );
        }
    }
}

/// How two transfer builds differ, walk by walk: which walks, and by how much, in each direction.
///
/// `TRANSFER_DIFF_BEFORE` and `TRANSFER_DIFF_AFTER` are `transfers/` directories.
#[test]
#[ignore = "needs TRANSFER_DIFF_BEFORE and TRANSFER_DIFF_AFTER"]
fn transfer_build_differences() {
    use std::collections::HashMap;
    use valinor_walk::transfer_tile::{DecodeLimits, TransferMode, decode_tile};
    let before = PathBuf::from(std::env::var("TRANSFER_DIFF_BEFORE").expect("before"));
    let after = PathBuf::from(std::env::var("TRANSFER_DIFF_AFTER").expect("after"));
    type Walks = HashMap<(u32, u32, u32, u32), u32>;
    fn load(root: &std::path::Path, mode: TransferMode) -> Walks {
        let mut walks = Walks::new();
        let mode_root = root.join(mode.as_str()).join("12");
        for x in std::fs::read_dir(&mode_root).expect("x") {
            let x = x.expect("x").path();
            for tile in std::fs::read_dir(&x).expect("y") {
                let tile = tile.expect("tile").path();
                let bytes = std::fs::read(&tile).expect("read");
                let tile = decode_tile(&bytes, DecodeLimits::default()).expect("decode");
                for (feed, stop, row) in tile.rows() {
                    for walk in row.iter() {
                        walks.insert((feed, stop, walk.feed, walk.stop), walk.seconds);
                    }
                }
            }
        }
        walks
    }
    fn band(seconds: u32) -> usize {
        match seconds {
            0..=299 => 0,
            300..=599 => 1,
            600..=889 => 2,
            _ => 3,
        }
    }
    const BANDS: [&str; 4] = ["<5min", "5-10min", "10-14.8min", ">=14.8min"];
    for mode in [TransferMode::Walking, TransferMode::Cycling] {
        let b = load(&before, mode);
        let a = load(&after, mode);
        let (mut same, mut lost, mut gained, mut down, mut up) =
            ([0_u64; 4], [0_u64; 4], [0_u64; 4], [0_u64; 4], [0_u64; 4]);
        let mut short_lost = Vec::new();
        for (key, &seconds) in &b {
            let at = band(seconds);
            match a.get(key) {
                None => {
                    lost[at] += 1;
                    if at == 0 {
                        short_lost.push((*key, seconds));
                    }
                }
                Some(&now) if now == seconds => same[at] += 1,
                Some(&now) if now < seconds => down[at] += 1,
                Some(_) => up[at] += 1,
            }
        }
        for (key, &seconds) in &a {
            if !b.contains_key(key) {
                gained[band(seconds)] += 1;
            }
        }
        eprintln!("{}: before {} after {}", mode.as_str(), b.len(), a.len());
        for (i, name) in BANDS.iter().enumerate() {
            eprintln!(
                "  {name}: same {} lost {} gained {} seconds down {} up {}",
                same[i], lost[i], gained[i], down[i], up[i]
            );
        }
        short_lost.sort_unstable();
        eprintln!(
            "  lost under 5 minutes: {:?}",
            &short_lost[..short_lost.len().min(20)]
        );
    }
}
