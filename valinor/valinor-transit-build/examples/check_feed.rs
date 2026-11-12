//! Compares one feed's tiles against `TransferTable::build` on the same stops,
//! and summarizes which feeds' stops walk to which.
//!
//! Run after the transfer stage, with its output directory:
//!
//! ```text
//! check_feed <compiled-dir> <street-tiles> <output-dir> <feed-id>
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use valinor_router::tiles::TileSets;
use valinor_transit::StopId;
use valinor_transit_build::compile::CompileReport;
use valinor_transit_build::manifest::read_manifest;
use valinor_transit_build::stops::StopList;
use valinor_transit_build::transfers::{costing_for, tile_path};
use valinor_walk::transfer_tile::{
    DecodeLimits, FeedPlacement, LoadRequest, StopPlacement, TransferMode, TransferTile,
    assemble_transfer_table, decode_tile,
};
use valinor_walk::{SnapConfig, TransferTable, snap_stops};

fn load_tiles(root: &std::path::Path, mode: TransferMode, list: &[[u32; 2]]) -> Vec<TransferTile> {
    list.iter()
        .map(|[x, y]| {
            let bytes = fs::read(tile_path(root, mode, *x, *y)).expect("tile");
            decode_tile(&bytes, DecodeLimits::default()).expect("decode")
        })
        .collect()
}

#[expect(clippy::too_many_lines, reason = "one linear check of a whole feed")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [compiled, street, output, feed] = args.as_slice() else {
        eprintln!("usage: check_feed <compiled> <street-tiles> <output> <feed-id>");
        std::process::exit(2);
    };
    let (compiled, street, output) = (
        PathBuf::from(compiled),
        PathBuf::from(street),
        PathBuf::from(output),
    );
    let report = CompileReport::read(&compiled).expect("report");
    let mut records = report.feeds;
    records.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    let threads = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    let stops = StopList::load(&compiled, &records, threads).expect("stops");
    let manifest = read_manifest(&output).expect("manifest");
    let index = manifest
        .feeds
        .iter()
        .position(|f| &f.id == feed)
        .expect("feed");
    let feed_stops = stops.feeds[index];
    let coords: Vec<(f64, f64)> = stops.coordinates
        [feed_stops.first as usize..(feed_stops.first + feed_stops.count) as usize]
        .iter()
        .map(|c| c.expect("coordinate"))
        .collect();
    let tiles = TileSets::open(&[street]).expect("tiles");
    let manifest_id = u64::from_str_radix(&manifest.manifest_id, 16).expect("hex");
    let root = output.join("transfers");

    for mode in TransferMode::ALL {
        let name = mode.as_str();
        let feed_tiles = &manifest.feeds[index].transfer_tiles[name];
        let decoded = load_tiles(&root, mode, feed_tiles);
        let placement = [FeedPlacement {
            feed_index: u32::try_from(index).expect("index"),
            stops: StopPlacement::Contiguous {
                first: 0,
                count: feed_stops.count,
            },
        }];
        let started = Instant::now();
        let (loaded, load_report) = assemble_transfer_table(
            &LoadRequest {
                mode,
                manifest_id,
                max_seconds: 900,
                local_stop_count: feed_stops.count,
                feeds: &placement,
            },
            &decoded,
        )
        .expect("assemble");
        let load_seconds = started.elapsed().as_secs_f64();
        let costing = costing_for(mode).expect("costing");
        let started = Instant::now();
        let snaps = snap_stops(
            &&tiles,
            &costing,
            &SnapConfig::default(),
            tiles.build_id(),
            &coords,
        )
        .expect("snap");
        let built =
            TransferTable::build(&&tiles, &costing, &Arc::new(snaps), 900, threads, &|| false)
                .expect("build")
                .expect("not interrupted");
        println!(
            "{name}: loaded {} walks, {:.3}s; build gives {} walks in {:.1}s; identical: {}; \
             walks to other feeds dropped: {}",
            loaded.walk_count(),
            load_seconds,
            built.walk_count(),
            started.elapsed().as_secs_f64(),
            loaded == built,
            load_report.walks_to_absent_feeds,
        );
        if loaded != built {
            let mut differing = 0;
            for from in 0..feed_stops.count {
                let from_id = StopId::new(from);
                let a: Vec<_> = loaded.walks_from(from_id).collect();
                let b: Vec<_> = built.walks_from(from_id).collect();
                if a != b {
                    differing += 1;
                    if differing <= 5 {
                        println!("  stop {from}: loaded {a:?} vs built {b:?}");
                    }
                }
            }
            println!("  {differing} stops differ");
        }

        // How many walks go between which feeds, for the report.
        let all_tiles: Vec<[u32; 2]> = manifest.tiles[name].iter().map(|t| [t.x, t.y]).collect();
        let mut pairs: BTreeMap<(u32, u32), u64> = BTreeMap::new();
        for tile in load_tiles(&root, mode, &all_tiles) {
            for (origin, _, walks) in tile.rows() {
                for walk in walks.iter() {
                    if walk.feed != origin {
                        *pairs.entry((origin, walk.feed)).or_default() += 1;
                    }
                }
            }
        }
        let mut sorted: Vec<_> = pairs.into_iter().collect();
        sorted.sort_unstable_by_key(|&(_, count)| std::cmp::Reverse(count));
        for (&(from, to), count) in sorted.iter().take(8).map(|(p, c)| (p, c)) {
            println!(
                "  {name} cross-feed: {} -> {}: {count}",
                manifest.feeds[from as usize].id, manifest.feeds[to as usize].id
            );
        }
    }
}
