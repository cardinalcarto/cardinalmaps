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
    AccessSide, SnapConfig, StopSnaps, StopTables, TableRequest, TransferTable, snap_stop,
    snap_stops,
};

const TAR: &str = "/mnt/maps/cardinal/routing-elevation-i2b/washington/tiles.tar";
const METRO: &str =
    "/mnt/maps/cardinal/transit/global/us-wa-2026-11-09/timetables/us-wa-Metro-Transit.timetable";

fn costing(name: &str) -> AnyCosting {
    use valhalla_proto::costing::Type;
    let kind = if name == "pedestrian" {
        Type::Pedestrian
    } else {
        Type::Bicycle
    };
    let doc = JsonValue::parse(b"{}").expect("doc");
    let mut proto = valhalla_proto::Costing::default();
    parse_costing(&doc, "/none", &mut proto, &mut Vec::new(), Some(kind)).expect("costing");
    AnyCosting::new(&proto).expect("any")
}

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

/// `with_new_stops` promises what building with those stops gives, and draws must agree with it.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn new_stops_match_a_full_build_on_real_streets() {
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
        eprintln!(
            "{name}: added {k} new stops to {} base stops in {:.2} s on one thread",
            n - k,
            started.elapsed().as_secs_f64()
        );
        let grown = triples(&grown);
        let full_set: std::collections::BTreeMap<(u32, u32), u32> =
            full.iter().map(|&(a, b, s)| ((a, b), s)).collect();
        let grown_set: std::collections::BTreeMap<(u32, u32), u32> =
            grown.iter().map(|&(a, b, s)| ((a, b), s)).collect();
        let mut differ = Vec::new();
        for (key, s) in &full_set {
            match grown_set.get(key) {
                Some(t) if t == s => {}
                other => differ.push((*key, Some(*s), other.copied())),
            }
        }
        for (key, s) in &grown_set {
            if !full_set.contains_key(key) {
                differ.push((*key, None, Some(*s)));
            }
        }
        let touching_new = |&((a, b), _, _): &((u32, u32), Option<u32>, Option<u32>)| {
            a as usize >= n - k || b as usize >= n - k
        };
        let from_new = differ.iter().filter(|d| d.0.0 as usize >= n - k).count();
        let base_to_new_total = full_set
            .keys()
            .filter(|k2| (k2.0 as usize) < n - k && (k2.1 as usize) >= n - k)
            .count();
        let lost = differ
            .iter()
            .filter(|d| d.1.is_some() && d.2.is_none())
            .count();
        let invented = differ.iter().filter(|d| d.1.is_none()).count();
        let time_off: Vec<i64> = differ
            .iter()
            .filter_map(|d| Some(d.2? as i64 - d.1? as i64))
            .collect();
        eprintln!(
            "{name}: differ with a new origin {from_new}; base-to-new walks in the full build {base_to_new_total}; lost {lost}, invented {invented}, wrong seconds {} (max abs {:?})",
            time_off.len(),
            time_off.iter().map(|v| v.abs()).max()
        );
        eprintln!(
            "{name}: full {} grown {} differ {} (touching a new stop {}); first {:?}",
            full.len(),
            grown.len(),
            differ.len(),
            differ.iter().filter(|d| touching_new(d)).count(),
            &differ[..differ.len().min(12)]
        );
        failures.push((name, differ.len()));
    }
    assert!(failures.iter().all(|f| f.1 == 0), "{failures:?}");
}

/// A draw at the table's own bound is the table's route, in time and edges.
///
/// Also times a draw at the old limit of one second past the walk, for what the bound costs.
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
        let (mut at_bound, mut at_walk) = (0.0_f64, 0.0_f64);
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
                if i % 9 != 0 {
                    continue;
                }
                let leg = table_search.leg(to).expect("table leg");
                let started = Instant::now();
                assert!(draw.search(&request).expect("draw"));
                at_bound += started.elapsed().as_secs_f64();
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
                let started = Instant::now();
                let shorter = TableRequest {
                    max_seconds: seconds + 1,
                    ..request
                };
                assert!(draw.search(&shorter).expect("short draw"));
                at_walk += started.elapsed().as_secs_f64();
            }
        }
        let per = |total: f64| 1000.0 * total / checked.max(1) as f64;
        eprintln!(
            "{name}: checked {checked}, not drawn {missing}, seconds differ {secs_off}, \
             route differs {edges_off}; per draw {:.2} ms at the bound, {:.2} ms at the walk's \
             own limit",
            per(at_bound),
            per(at_walk)
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
