//! Transfer tables on real tiles: what they hold, and how a search reads them.

#![allow(clippy::all, clippy::pedantic)]

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

use valinor_router::tiles::TileSets;
use valinor_sif::AnyCosting;
use valinor_transit::{
    Clock, Direction, StopId, TransferArrival, TransferOutcome, TransferRequest, TransferSearch,
    TransferSeed, TransferSink,
};

use super::review_tests::snaps_of;
use super::tile_tests::{costing_named, random_stops};
use crate::snap::StopSnaps;
use crate::table::{AccessSide, StopAttachments, StopTables, TableRequest};
use crate::transfers::{Hop, TableTransfers, TableWalk, TransferTable, sorted_walks_from};

const TILES: [u64; 5] = [762_485, 762_486, 763_925, 763_926, 763_927];
const RADIUS: u32 = 900;

fn andorra() -> TileSets {
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles");
    TileSets::open(&[tiles], NonZeroUsize::new(64).expect("nonzero")).expect("tiles")
}

fn threads(count: usize) -> NonZeroUsize {
    NonZeroUsize::new(count).expect("nonzero")
}

fn built(
    provider: &&TileSets,
    costing: &AnyCosting,
    snaps: &Arc<StopSnaps>,
    count: usize,
) -> TransferTable {
    TransferTable::build(provider, costing, snaps, RADIUS, threads(count), &|| false)
        .expect("build")
        .expect("not interrupted")
}

#[derive(Default)]
struct Sink(Vec<TransferArrival<TableWalk>>);

impl TransferSink<TableWalk> for Sink {
    fn reach(&mut self, arrival: TransferArrival<TableWalk>) {
        self.0.push(arrival);
    }
}

fn search(
    transfers: &mut TableTransfers,
    direction: Direction,
    seeds: &[TransferSeed],
    bound: i32,
) -> HashMap<u32, TransferArrival<TableWalk>> {
    let mut sink = Sink::default();
    transfers.begin_run(direction);
    let outcome = transfers
        .search(
            &TransferRequest {
                direction,
                seeds,
                upper_bound: Clock::new(bound),
                change_margin: TransferRequest::REPORT_ALL,
                interrupt: &|| false,
            },
            &mut sink,
        )
        .expect("search");
    assert_eq!(outcome, TransferOutcome::Completed);
    let mut best: HashMap<u32, TransferArrival<TableWalk>> = HashMap::new();
    for arrival in sink.0 {
        let keep = best
            .get(&arrival.stop.into_inner())
            .is_none_or(|known| arrival.clock < known.clock);
        if keep {
            best.insert(arrival.stop.into_inner(), arrival);
        }
    }
    best
}

fn seed(stop: u32, clock: i32) -> TransferSeed {
    TransferSeed {
        stop: StopId::new(stop),
        clock: Clock::new(clock),
    }
}

fn pairs(table: &TransferTable) -> Vec<(u32, u32, u32)> {
    let mut all = Vec::new();
    for from in 0..table.stop_count() as u32 {
        for (to, seconds) in table.walks_from(StopId::new(from)) {
            all.push((from, to.into_inner(), seconds));
        }
    }
    all
}

/// Entries are the seconds of Sif's preferred route, and that route can be rebuilt for a result.
#[test]
fn entries_are_the_sif_route_and_rebuild_to_the_same_leg() {
    let sets = andorra();
    let provider = &sets;
    for name in ["pedestrian", "bicycle"] {
        let costing = costing_named(name);
        let points = random_stops(&provider, &TILES, 120, 11);
        let snaps = snaps_of(&provider, &points);
        let table = built(&provider, &costing, &snaps, 1);
        let walks = pairs(&table);
        assert!(walks.len() > 100, "{name}: only {} walks", walks.len());
        assert_eq!(
            pairs(&built(&provider, &costing, &snaps, 3)),
            walks,
            "{name}"
        );

        let mut tables = StopTables::new(provider, costing.clone());
        tables.bind(Arc::clone(&snaps)).expect("bind");
        for &(from, to, seconds) in walks.iter().step_by(7) {
            assert_ne!(from, to, "{name}: a walk from a stop to itself");
            assert!(seconds <= RADIUS, "{name}: past the bound");
            let seeds = tables.stop_seeds(StopId::new(from)).expect("seeds");
            let found = tables
                .compute(
                    snaps.stop_count(),
                    &TableRequest {
                        side: AccessSide::Access,
                        seeds: &seeds,
                        max_seconds: RADIUS,
                        interrupt: &|| false,
                    },
                )
                .expect("compute")
                .expect("not interrupted");
            assert_eq!(
                found.cost(StopId::new(to)).map(|c| c.into_inner()),
                Some(seconds),
                "{name}: {from} to {to} rebuilt with a different time"
            );
            let leg = tables.leg(StopId::new(to)).expect("leg");
            assert_eq!(leg.secs.round() as u32, seconds, "{name}: leg seconds");
        }
    }
}

/// Forward and backward searches read one number per directed pair, and direction matters.
#[test]
fn forward_and_backward_agree_on_every_directed_pair() {
    let sets = andorra();
    let provider = &sets;
    let costing = costing_named("pedestrian");
    let points = random_stops(&provider, &TILES, 120, 13);
    let snaps = snaps_of(&provider, &points);
    let table = Arc::new(built(&provider, &costing, &snaps, 2));
    let mut transfers = TableTransfers::new(Arc::clone(&table));
    let mut asymmetric = 0;
    for from in 0..table.stop_count() as u32 {
        let forward = search(
            &mut transfers,
            Direction::Forward,
            &[seed(from, 100)],
            1 << 29,
        );
        for (&to, arrival) in &forward {
            let seconds = arrival.clock.seconds() - 100;
            assert_eq!(arrival.parent.from, StopId::new(from));
            assert_eq!(arrival.parent.to, StopId::new(to));
            let backward = search(
                &mut transfers,
                Direction::Backward,
                &[seed(to, -50)],
                1 << 29,
            );
            let mirrored = backward.get(&from).expect("the backward search finds it");
            assert_eq!(mirrored.clock.seconds() + 50, seconds, "{from} to {to}");
            assert_eq!(mirrored.parent.from, StopId::new(from), "real travel order");
            assert_eq!(mirrored.parent.to, StopId::new(to), "real travel order");
            if table.seconds(StopId::new(to), StopId::new(from)) != Some(seconds as u32) {
                asymmetric += 1;
            }
        }
    }
    assert!(asymmetric > 0, "no directed pair differs from its opposite");
}

#[test]
fn a_search_keeps_to_its_bound_and_limit_and_never_walks_to_a_seed_from_itself() {
    let sets = andorra();
    let provider = &sets;
    let costing = costing_named("pedestrian");
    let points = random_stops(&provider, &TILES, 120, 17);
    let snaps = snaps_of(&provider, &points);
    let table = Arc::new(built(&provider, &costing, &snaps, 1));
    let mut transfers = TableTransfers::new(table);

    let all = search(&mut transfers, Direction::Forward, &[seed(5, 0)], 1 << 29);
    assert!(!all.contains_key(&5), "a seed reached from itself");
    let farthest = all
        .values()
        .map(|a| a.clock.seconds())
        .max()
        .expect("some walk");

    let bounded = search(&mut transfers, Direction::Forward, &[seed(5, 0)], farthest);
    assert!(bounded.values().all(|a| a.clock.seconds() < farthest));
    assert!(
        bounded.len() < all.len(),
        "the exclusive bound removed nothing"
    );

    transfers.set_walk_limit(Some(farthest as u32 - 1));
    let limited = search(&mut transfers, Direction::Forward, &[seed(5, 0)], 1 << 29);
    assert_eq!(limited.len(), bounded.len());
    transfers.set_walk_limit(None);

    // Two seeds: each is still reached from the other, even if it arrives later than its clock.
    let (a, b) = all.iter().next().map(|(&b, _)| (5, b)).expect("a neighbor");
    let both = search(
        &mut transfers,
        Direction::Forward,
        &[seed(a, 0), seed(b, 0)],
        1 << 29,
    );
    assert!(both.contains_key(&a) || table_has_no_walk(&transfers, b, a));
    assert!(both.contains_key(&b));
}

fn table_has_no_walk(transfers: &TableTransfers, from: u32, to: u32) -> bool {
    transfers
        .table()
        .seconds(StopId::new(from), StopId::new(to))
        .is_none()
}

/// The walks of a full build, with each base stop's walk into a new stop replaced by what
/// that new stop's backward search finds, as `with_new_stops` gives them.
fn expected_with_new_stops(
    provider: &&TileSets,
    costing: &AnyCosting,
    all: &Arc<StopSnaps>,
    full: &TransferTable,
    new_stops: &[StopId],
) -> Vec<(u32, u32, u32)> {
    let is_new = |stop: u32| new_stops.iter().any(|new| new.into_inner() == stop);
    let mut tables = StopTables::new(*provider, costing.clone());
    tables.bind(Arc::clone(all)).expect("bind");
    let mut walks: Vec<(u32, u32, u32)> = pairs(full)
        .into_iter()
        .filter(|&(from, to, _)| is_new(from) || !is_new(to))
        .collect();
    for &stop in new_stops {
        let seeds = tables.stop_seeds(stop).expect("seeds");
        if seeds.is_empty() {
            continue;
        }
        assert!(
            tables
                .search(&TableRequest {
                    side: AccessSide::Egress,
                    seeds: &seeds,
                    max_seconds: RADIUS,
                    interrupt: &|| false,
                })
                .expect("search")
        );
        for hop in sorted_walks_from(&tables, stop) {
            if !is_new(hop.stop) {
                walks.push((hop.stop, stop.into_inner(), hop.seconds));
            }
        }
    }
    walks.sort_unstable();
    walks
}

/// Adding stops to a base table gives the walks of a full build, with walks into a new stop
/// from that new stop's backward search.
#[test]
fn adding_stops_matches_building_with_them() {
    let sets = andorra();
    let provider = &sets;
    let costing = costing_named("pedestrian");
    let points = random_stops(&provider, &TILES, 90, 19);
    let all = snaps_of(&provider, &points);
    let base = snaps_of(&provider, &points[..80]);
    let full = built(&provider, &costing, &all, 1);
    let new_stops: Vec<StopId> = (80..90).map(StopId::new).collect();
    let expected = expected_with_new_stops(&provider, &costing, &all, &full, &new_stops);
    let grown = built(&provider, &costing, &base, 1)
        .with_new_stops(&provider, &costing, &all, &new_stops, &|| false)
        .expect("grow")
        .expect("not interrupted");
    let sorted = |table: &TransferTable| {
        let mut walks = pairs(table);
        walks.sort_unstable();
        walks
    };
    assert_eq!(sorted(&grown), expected);
    assert_eq!(grown.stop_count(), 90);

    // The same, adding to a table that is memory-mapped.
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("table.bin");
    let key = [3; 32];
    let mut bytes = Vec::new();
    built(&provider, &costing, &base, 1)
        .write_flat(&mut bytes, &key)
        .expect("write");
    std::fs::write(&path, &bytes).expect("write");
    // SAFETY: this test owns the file and doesn't change it while mapped.
    let mapped = unsafe { TransferTable::map_flat(&path, &key) }.expect("maps");
    let grown_mapped = mapped
        .with_new_stops(&provider, &costing, &all, &new_stops, &|| false)
        .expect("grow")
        .expect("not interrupted");
    assert_eq!(grown_mapped, grown);
    assert_eq!(sorted(&grown_mapped), expected);
    assert_eq!(
        mapped.bytes(),
        0,
        "the mapped table's walks aren't on the heap"
    );
    assert!(grown_mapped.bytes() < full.bytes());
}

/// A walk into a new stop records that it came from a backward search,
/// and drawing it that way gives the walk's seconds; every other walk is forward.
#[test]
fn a_walk_into_a_new_stop_is_found_and_drawn_backward() {
    let sets = andorra();
    let provider = &sets;
    for name in ["pedestrian", "bicycle"] {
        let costing = costing_named(name);
        let points = random_stops(&provider, &TILES, 90, 41);
        let all = snaps_of(&provider, &points);
        let base = snaps_of(&provider, &points[..80]);
        let new_stops: Vec<StopId> = (80..90).map(StopId::new).collect();
        let grown = Arc::new(
            built(&provider, &costing, &base, 1)
                .with_new_stops(&provider, &costing, &all, &new_stops, &|| false)
                .expect("grow")
                .expect("not interrupted"),
        );
        let mut draw = StopTables::new(provider, costing.clone());
        draw.bind(Arc::clone(&all)).expect("bind");
        let mut drawn_backward = 0;
        let mut transfers = TableTransfers::new(Arc::clone(&grown));
        for from in 0..80_u32 {
            let reached = search(
                &mut transfers,
                Direction::Forward,
                &[seed(from, 0)],
                i32::MAX,
            );
            for (to, arrival) in reached {
                let walk = arrival.parent;
                assert_eq!(walk.from.into_inner(), from);
                if to < 80 {
                    assert_eq!(walk.search, AccessSide::Access, "{name}: {from}->{to}");
                    continue;
                }
                assert_eq!(walk.search, AccessSide::Egress, "{name}: {from}->{to}");
                let seeds = draw.stop_seeds(walk.to).expect("seeds");
                assert!(
                    draw.search_to_stop(
                        &TableRequest {
                            side: AccessSide::Egress,
                            seeds: &seeds,
                            max_seconds: RADIUS,
                            interrupt: &|| false,
                        },
                        walk.from,
                    )
                    .expect("draw")
                );
                let leg = draw.leg(walk.from).expect("drawn");
                assert_eq!(
                    leg.secs.round() as u32,
                    walk.seconds,
                    "{name}: {from}->{to}"
                );
                drawn_backward += 1;
            }
        }
        assert!(drawn_backward > 0, "{name}: no walk came into a new stop");
    }
}

/// A search at a limit reports exactly what a wider search reports with cost and seconds within
/// that limit's bound: a bound drops only routes too costly, and never changes the cheapest.
#[test]
fn a_tables_entries_are_a_wider_searchs_within_its_bound() {
    let sets = andorra();
    let provider = &sets;
    let mut limited_out = 0;
    for (name, mode) in [
        ("pedestrian", valinor_sif::TravelMode::Pedestrian),
        ("bicycle", valinor_sif::TravelMode::Bicycle),
    ] {
        let costing = costing_named(name);
        let points = random_stops(&provider, &TILES, 90, 43);
        let snaps = snaps_of(&provider, &points);
        let mut wide = StopTables::new(provider, costing.clone());
        wide.bind(Arc::clone(&snaps)).expect("bind");
        let mut narrow = StopTables::new(provider, costing.clone());
        narrow.bind(Arc::clone(&snaps)).expect("bind");
        let cost_of = |leg: &crate::table::AccessLeg| -> f32 {
            leg.edges
                .iter()
                .map(|e| e.transition.cost + e.traversal.cost)
                .sum()
        };
        for side in [AccessSide::Access, AccessSide::Egress] {
            for origin in (0..90_u32).step_by(7) {
                let stop = StopId::new(origin);
                let seeds = wide.stop_seeds(stop).expect("seeds");
                if seeds.is_empty() {
                    continue;
                }
                let run = |tables: &mut StopTables<&TileSets, AnyCosting>, limit: u32| {
                    assert!(
                        tables
                            .search(&TableRequest {
                                side,
                                seeds: &seeds,
                                max_seconds: limit,
                                interrupt: &|| false,
                            })
                            .expect("search")
                    );
                };
                run(&mut wide, 7200);
                let full: HashMap<StopId, (f32, f32)> = wide
                    .reached_stops()
                    .map(|(to, _)| {
                        let leg = wide.leg(to).expect("leg");
                        (to, (leg.secs, cost_of(&leg)))
                    })
                    .collect();
                for limit in [30_u32, 60, 120, 240, 480, 900, 1800, 3600] {
                    run(&mut narrow, limit);
                    let bound = limit as f32 * crate::table::cost_factor(mode);
                    let got: HashMap<StopId, (f32, f32)> = narrow
                        .reached_stops()
                        .map(|(to, _)| {
                            let leg = narrow.leg(to).expect("leg");
                            (to, (leg.secs, cost_of(&leg)))
                        })
                        .collect();
                    for (to, &(secs, cost)) in &full {
                        // Float noise of the sum at the edge of the bound or limit can go either way.
                        if (cost - bound).abs() < 0.05 || (secs - limit as f32).abs() < 0.05 {
                            continue;
                        }
                        let should = cost <= bound && secs <= limit as f32;
                        assert_eq!(
                            got.contains_key(to),
                            should,
                            "{name} {side:?} from {origin} to {to:?} at {limit}: {secs}s, cost {cost}"
                        );
                        if should {
                            let (s, c) = got[to];
                            assert!(
                                (c - cost).abs() < 0.05 && (s - secs).abs() < 2.0,
                                "{name} {side:?} from {origin} to {to:?} at {limit}: \
                                 {s}s/{c} against {secs}s/{cost}"
                            );
                        } else {
                            limited_out += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(
        limited_out > 0,
        "no limit changed anything, so this tested nothing"
    );
}

/// A search stopped at its target gives the target what the full search would.
#[test]
fn a_search_to_a_stop_finds_the_full_searchs_route() {
    let sets = andorra();
    let provider = &sets;
    for name in ["pedestrian", "bicycle"] {
        let costing = costing_named(name);
        let points = random_stops(&provider, &TILES, 90, 47);
        let snaps = snaps_of(&provider, &points);
        let mut full = StopTables::new(provider, costing.clone());
        full.bind(Arc::clone(&snaps)).expect("bind");
        let mut draw = StopTables::new(provider, costing.clone());
        draw.bind(Arc::clone(&snaps)).expect("bind");
        let mut checked = 0;
        for side in [AccessSide::Access, AccessSide::Egress] {
            for origin in (0..90_u32).step_by(3) {
                let seeds = full.stop_seeds(StopId::new(origin)).expect("seeds");
                if seeds.is_empty() {
                    continue;
                }
                let request = TableRequest {
                    side,
                    seeds: &seeds,
                    max_seconds: RADIUS,
                    interrupt: &|| false,
                };
                assert!(full.search(&request).expect("search"));
                let reached: Vec<StopId> = full.reached_stops().map(|(to, _)| to).collect();
                for to in reached {
                    assert!(draw.search_to_stop(&request, to).expect("draw"));
                    let drawn = draw.leg(to).expect("drawn");
                    let tabled = full.leg(to).expect("tabled");
                    assert_eq!(
                        drawn.edges, tabled.edges,
                        "{name} {side:?} {origin}->{to:?}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 50, "{name}: only {checked} draws");
    }
}

#[test]
fn an_interrupted_build_stops() {
    let sets = andorra();
    let provider = &sets;
    let costing = costing_named("pedestrian");
    let points = random_stops(&provider, &TILES, 20, 23);
    let snaps = snaps_of(&provider, &points);
    let outcome = TransferTable::build(&provider, &costing, &snaps, RADIUS, threads(1), &|| true)
        .expect("build");
    assert!(outcome.is_none());
}

/// The allocation-free read finds what the table does, stop by stop, in either direction.
#[test]
fn reading_what_a_search_reached_matches_the_full_table() {
    let sets = andorra();
    let provider = &sets;
    let costing = costing_named("pedestrian");
    let points = random_stops(&provider, &TILES, 120, 29);
    let snaps = snaps_of(&provider, &points);
    let mut tables = StopTables::new(provider, costing.clone());
    tables.bind(Arc::clone(&snaps)).expect("bind");
    for from in 0..snaps.stop_count() as u32 {
        let seeds = tables.stop_seeds(StopId::new(from)).expect("seeds");
        if seeds.is_empty() {
            continue;
        }
        for side in [AccessSide::Access, AccessSide::Egress] {
            let request = TableRequest {
                side,
                seeds: &seeds,
                max_seconds: RADIUS,
                interrupt: &|| false,
            };
            let table = tables
                .compute(snaps.stop_count(), &request)
                .expect("compute")
                .expect("not interrupted");
            let expected: Vec<(u32, u32)> = (0..snaps.stop_count() as u32)
                .filter_map(|to| {
                    table
                        .cost(StopId::new(to))
                        .map(|cost| (to, cost.into_inner()))
                })
                .collect();
            assert!(tables.search(&request).expect("search"));
            let mut reached: Vec<(u32, u32)> = tables
                .reached_stops()
                .map(|(stop, cost)| (stop.into_inner(), cost.into_inner()))
                .collect();
            reached.sort_unstable();
            assert_eq!(reached, expected, "stop {from}, {side:?}");
        }
    }
}

/// Shared attachments and forgetting tiles between searches change no walk.
#[test]
fn shared_attachments_and_forgotten_tiles_change_no_walk() {
    let sets = andorra();
    let provider = &sets;
    for name in ["pedestrian", "bicycle"] {
        let costing = costing_named(name);
        let points = random_stops(&provider, &TILES, 120, 31);
        let snaps = snaps_of(&provider, &points);
        let expected = pairs(&built(&provider, &costing, &snaps, 2));
        let attachments = Arc::new(
            StopAttachments::new(provider, costing.clone(), Arc::clone(&snaps)).expect("attach"),
        );
        let mut tables = StopTables::new(provider, costing.clone());
        tables.bind_shared(Arc::clone(&attachments));
        let mut found = Vec::new();
        for from in 0..snaps.stop_count() as u32 {
            // A build forgets tiles between batches, so check every search survives it.
            tables.forget_tiles();
            let from_id = StopId::new(from);
            let seeds = tables.stop_seeds(from_id).expect("seeds");
            if seeds.is_empty() {
                continue;
            }
            let ran = tables
                .search(&TableRequest {
                    side: AccessSide::Access,
                    seeds: &seeds,
                    max_seconds: RADIUS,
                    interrupt: &|| false,
                })
                .expect("search");
            assert!(ran);
            for Hop { stop, seconds } in sorted_walks_from(&tables, from_id) {
                found.push((from, stop, seconds));
            }
        }
        assert_eq!(found, expected, "{name}");
    }
}

/// A table loaded from sorted walks is the built table, in every field.
#[test]
fn a_table_from_sorted_walks_equals_the_built_table() {
    let sets = andorra();
    let provider = &sets;
    let costing = costing_named("pedestrian");
    let points = random_stops(&provider, &TILES, 90, 37);
    let snaps = snaps_of(&provider, &points);
    let built = built(&provider, &costing, &snaps, 2);
    let walks: Vec<(u32, Hop)> = pairs(&built)
        .into_iter()
        .map(|(from, stop, seconds)| (from, Hop { stop, seconds }))
        .collect();
    let loaded =
        TransferTable::from_sorted_walks(built.stop_count(), RADIUS, &walks).expect("load");
    assert_eq!(loaded, built);
}

#[test]
fn sorted_walks_refuse_bad_input() {
    let hop = |stop, seconds| Hop { stop, seconds };
    for walks in [
        vec![(0, hop(1, 10)), (0, hop(1, 12))],
        vec![(1, hop(1, 10))],
        vec![(1, hop(0, 10)), (0, hop(1, 10))],
        vec![(0, hop(9, 10))],
        vec![(0, hop(1, 901))],
    ] {
        assert!(TransferTable::from_sorted_walks(3, 900, &walks).is_err());
    }
}

/// A flat file gives the same table back, through a mapping, and refuses to be misused.
#[test]
fn a_table_round_trips_through_a_flat_file() {
    use crate::flat_table::{FLAT_TABLE_MAGIC, FlatTableError};
    use crate::transfers::Hop as TransferHop;

    let walks: Vec<(u32, TransferHop)> = vec![
        (
            0,
            TransferHop {
                stop: 1,
                seconds: 10,
            },
        ),
        (
            0,
            TransferHop {
                stop: 2,
                seconds: 20,
            },
        ),
        (
            2,
            TransferHop {
                stop: 0,
                seconds: 30,
            },
        ),
    ];
    let table = TransferTable::from_sorted_walks(4, 900, &walks).expect("valid");
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("table.bin");
    let key = [7; 32];
    let mut bytes = Vec::new();
    table.write_flat(&mut bytes, &key).expect("write");
    std::fs::write(&path, &bytes).expect("write");

    // SAFETY: this test owns the file and doesn't change it while mapped.
    let mapped = unsafe { TransferTable::map_flat(&path, &key) }.expect("maps");
    assert_eq!(mapped, table);
    assert_eq!(mapped.max_seconds(), 900);
    assert_eq!(mapped.walk_count(), 3);
    assert_eq!(mapped.bytes(), 0, "a mapped table holds no heap");
    assert_eq!(mapped.seconds(StopId::new(2), StopId::new(0)), Some(30));

    // SAFETY: as above.
    let error = unsafe { TransferTable::map_flat(&path, &[9; 32]) }.expect_err("wrong key");
    assert!(matches!(error, FlatTableError::KeyMismatch));

    // A file of the wrong length is refused, at every length.
    for length in [0, 7, 40, bytes.len() - 1, bytes.len() + 1] {
        let mut shortened = bytes.clone();
        shortened.resize(length, 0);
        std::fs::write(&path, &shortened).expect("write");
        // SAFETY: as above.
        assert!(unsafe { TransferTable::map_flat(&path, &key) }.is_err());
    }

    // Offsets that leave the file or go backward are refused.
    let mut broken = bytes.clone();
    let offset_at = 8 + 32 + 16;
    broken[offset_at..offset_at + 4].copy_from_slice(&1_000_000_u32.to_le_bytes());
    std::fs::write(&path, &broken).expect("write");
    // SAFETY: as above.
    assert!(matches!(
        unsafe { TransferTable::map_flat(&path, &key) },
        Err(FlatTableError::Invalid(_))
    ));

    // Walks past the table are unused by the search's bounds checks.
    let mut broken = bytes.clone();
    let stops_at = 8 + 32 + 16 + 5 * 4;
    broken[stops_at..stops_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(broken[..8], FLAT_TABLE_MAGIC);
    std::fs::write(&path, &broken).expect("write");
    // SAFETY: as above.
    let mapped = unsafe { TransferTable::map_flat(&path, &key) }.expect("maps");
    let mut transfers = TableTransfers::new(Arc::new(mapped));
    let mut sink = CollectingSink::default();
    transfers
        .search(
            &TransferRequest {
                direction: Direction::Forward,
                seeds: &[TransferSeed {
                    stop: StopId::new(0),
                    clock: Clock::new(0),
                }],
                upper_bound: Clock::new(10_000),
                change_margin: TransferRequest::REPORT_ALL,
                interrupt: &|| false,
            },
            &mut sink,
        )
        .expect("searches");
}

#[derive(Default)]
struct CollectingSink(Vec<StopId>);

impl TransferSink<TableWalk> for CollectingSink {
    fn reach(&mut self, arrival: TransferArrival<TableWalk>) {
        self.0.push(arrival.stop);
    }
}
