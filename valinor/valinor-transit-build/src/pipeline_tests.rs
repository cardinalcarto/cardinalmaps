//! Whole-stage tests over real zip archives and the Andorra street tiles.

#![allow(clippy::all, clippy::pedantic)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tempfile::TempDir;
use valinor_router::tiles::TileSets;
use valinor_transit::StopId;
use valinor_transit_plan::{BuildLoadError, TransitBuild};
use valinor_walk::transfer_tile::{
    DecodeLimits, FeedPlacement, LoadRequest, StopPlacement, TransferMode, TransferTile,
    assemble_transfer_table, decode_tile,
};
use valinor_walk::{SnapConfig, TransferTable, snap_stops};
use zip::write::SimpleFileOptions;

use crate::compile::{self, COMPILE_REPORT, CompileOptions, FeedOutcome, FeedRecord};
use crate::error::BuildError;
use crate::feeds::hex_digest;
use crate::manifest::{Manifest, manifest_id, read_manifest};
use crate::stops::StopList;
use crate::transfers::{self, TransferOptions, costing_for, tile_path};

const BOUND: u32 = 900;

fn threads(count: usize) -> NonZeroUsize {
    NonZeroUsize::new(count).expect("nonzero")
}

fn street_tiles() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../valhalla-graphtile/fixtures/andorra-tiles")
}

/// One synthetic feed, an archive of minimal GTFS.
struct Feed<'a> {
    id: &'a str,
    /// `(lat, lon)` of each stop.
    stops: &'a [(f64, f64)],
}

fn gtfs_zip(stops: &[(f64, f64)]) -> Vec<u8> {
    let mut stops_txt = String::from("stop_id,stop_name,stop_lat,stop_lon\n");
    for (i, (lat, lon)) in stops.iter().enumerate() {
        stops_txt.push_str(&format!("s{i},Stop {i},{lat},{lon}\n"));
    }
    let mut stop_times =
        String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence\n");
    for i in 0..stops.len() {
        let t = format!("08:{:02}:{:02}", i / 60, i % 60);
        stop_times.push_str(&format!("t1,{t},{t},s{i},{}\n", i + 1));
    }
    let files = [
        (
            "agency.txt",
            "agency_id,agency_name,agency_url,agency_timezone\n\
             a,Andorra Bus,https://example.com,Europe/Andorra\n"
                .to_owned(),
        ),
        (
            "routes.txt",
            "route_id,agency_id,route_short_name,route_type\nr1,a,1,3\n".to_owned(),
        ),
        (
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             c,1,1,1,1,1,1,1,20260101,20271231\n"
                .to_owned(),
        ),
        (
            "trips.txt",
            "route_id,service_id,trip_id\nr1,c,t1\n".to_owned(),
        ),
        ("stops.txt", stops_txt),
        ("stop_times.txt", stop_times),
    ];
    let mut bytes = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        for (name, content) in files {
            zip.start_file(name, SimpleFileOptions::default())
                .expect("start");
            zip.write_all(content.as_bytes()).expect("write");
        }
        zip.finish().expect("finish");
    }
    bytes
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(&Sha256::digest(bytes))
}

/// Writes an input directory for `feeds` and returns it.
fn input_dir(feeds: &[(&str, Vec<u8>)]) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir(dir.path().join("feeds")).expect("mkdir");
    let mut entries = Vec::new();
    for (id, bytes) in feeds {
        let file = format!("feeds/{id}.gtfs.zip");
        fs::write(dir.path().join(&file), bytes).expect("write feed");
        entries.push(serde_json::json!({
            "id": id,
            "region": "ad",
            "name": id,
            "file": file,
            "sha256": sha256_hex(bytes),
            "bytes": bytes.len(),
            "license": {"spdx": "CC0-1.0"},
            "source": {"name": id, "kind": "test"},
        }));
    }
    let document = serde_json::json!({
        "transitous_commit": "abc123",
        "fetched_at": "2026-11-09T00:00:00Z",
        "feeds": entries,
        "skipped": [{"id": "gone", "reason": "broken"}],
        "realtime": [],
    });
    fs::write(
        dir.path().join("feeds.json"),
        serde_json::to_vec_pretty(&document).expect("json"),
    )
    .expect("write feeds.json");
    dir
}

fn compile_options(input: &Path, output: &Path) -> CompileOptions {
    CompileOptions {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        threads: threads(4),
        retry_failed: false,
        seated_wait_seconds: valinor_gtfs::DEFAULT_SEATED_WAIT_SECONDS,
    }
}

fn transfer_options(compiled: &Path, output: &Path) -> TransferOptions {
    TransferOptions {
        compiled: compiled.to_path_buf(),
        street_tiles: street_tiles(),
        output: output.to_path_buf(),
        threads: threads(4),
        max_seconds_walking: BOUND,
        max_seconds_cycling: BOUND,
        max_seconds_wheelchair: BOUND,
        zstd_level: 3,
        modes: TransferMode::ALL.to_vec(),
        snap: SnapConfig::default(),
        // The tests below compare tiles with the table build walk for walk; pruning has its
        // own test.
        prune_dominated: false,
    }
}

/// A grid of stops over Andorra la Vella's streets, offset so feeds interleave.
fn grid(offset: f64, count: usize) -> Vec<(f64, f64)> {
    (0..count)
        .map(|i| {
            (
                42.5048 + offset + 0.0013 * (i % 8) as f64,
                1.5155 + offset + 0.0017 * (i / 8) as f64,
            )
        })
        .collect()
}

fn compiled_feeds(feeds: &[Feed<'_>]) -> (TempDir, TempDir, compile::CompileReport) {
    let bytes: Vec<(&str, Vec<u8>)> = feeds.iter().map(|f| (f.id, gtfs_zip(f.stops))).collect();
    let input = input_dir(&bytes);
    let output = tempfile::tempdir().expect("output");
    let report = compile::run(&compile_options(input.path(), output.path())).expect("compile");
    (input, output, report)
}

fn record<'a>(report: &'a compile::CompileReport, id: &str) -> &'a FeedRecord {
    report
        .feeds
        .iter()
        .find(|feed| feed.id == id)
        .expect("feed record")
}

#[test]
fn a_feed_that_fails_is_recorded_and_the_others_compile() {
    let good = gtfs_zip(&grid(0.0, 16));
    let mut no_agency = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    no_agency
        .start_file("stops.txt", SimpleFileOptions::default())
        .expect("start");
    no_agency
        .write_all(b"stop_id,stop_lat,stop_lon\n")
        .expect("write");
    let no_agency = no_agency.finish().expect("finish").into_inner();
    let input = input_dir(&[
        ("garbage", b"not a zip".to_vec()),
        ("good", good),
        ("no-agency", no_agency),
        ("wrong-hash", b"also garbage".to_vec()),
    ]);
    // Rewriting the archive after listing it makes its hash wrong.
    fs::write(
        input.path().join("feeds/wrong-hash.gtfs.zip"),
        b"really not a zip",
    )
    .expect("rewrite");
    let output = tempfile::tempdir().expect("output");
    let report = compile::run(&compile_options(input.path(), output.path())).expect("compile");
    let good = record(&report, "good");
    assert_eq!(good.outcome, FeedOutcome::Compiled);
    assert_eq!(good.stops, 16);
    assert!(good.timetable.is_some());
    let FeedOutcome::Failed { error } = &record(&report, "wrong-hash").outcome else {
        panic!("wrong-hash should fail");
    };
    assert!(error.contains("sha256"), "{error}");
    assert!(matches!(
        record(&report, "garbage").outcome,
        FeedOutcome::Failed { .. }
    ));
    assert!(matches!(
        record(&report, "no-agency").outcome,
        FeedOutcome::Failed { .. }
    ));
    assert_eq!(record(&report, "no-agency").timetable, None);
    assert!(report.feeds.iter().map(|f| &f.id).is_sorted());
    assert!(output.path().join(COMPILE_REPORT).exists());
    let license: serde_json::Value = serde_json::from_str(good.license.get()).expect("json");
    assert_eq!(license, serde_json::json!({"spdx": "CC0-1.0"}));
}

#[test]
fn unchanged_input_is_not_recompiled() {
    let (input, output, first) = compiled_feeds(&[Feed {
        id: "good",
        stops: &grid(0.0, 16),
    }]);
    assert!(!record(&first, "good").reused);
    let timetable = output
        .path()
        .join(record(&first, "good").timetable.as_ref().expect("file"));
    let mtime = fs::metadata(&timetable)
        .expect("metadata")
        .modified()
        .expect("mtime");

    let second = compile::run(&compile_options(input.path(), output.path())).expect("resume");
    assert!(record(&second, "good").reused);
    assert_eq!(
        fs::metadata(&timetable)
            .expect("metadata")
            .modified()
            .expect("mtime"),
        mtime,
        "a reused feed isn't rewritten"
    );

    // A timetable that went missing is rebuilt.
    fs::remove_file(&timetable).expect("remove");
    let third = compile::run(&compile_options(input.path(), output.path())).expect("resume");
    assert!(!record(&third, "good").reused);
    assert!(timetable.exists());

    // A changed archive is rebuilt, once its listing names it.
    let bytes = gtfs_zip(&grid(0.0, 24));
    fs::write(input.path().join("feeds/good.gtfs.zip"), &bytes).expect("rewrite");
    let mut listing: serde_json::Value =
        serde_json::from_slice(&fs::read(input.path().join("feeds.json")).expect("read"))
            .expect("parse");
    listing["feeds"][0]["sha256"] = serde_json::Value::String(sha256_hex(&bytes));
    fs::write(
        input.path().join("feeds.json"),
        serde_json::to_vec(&listing).expect("json"),
    )
    .expect("write");
    let fourth = compile::run(&compile_options(input.path(), output.path())).expect("resume");
    assert!(!record(&fourth, "good").reused);
    assert_eq!(record(&fourth, "good").stops, 24);
}

#[test]
fn a_failed_feed_is_kept_until_asked_to_retry() {
    let input = input_dir(&[("bad", b"not a zip".to_vec())]);
    let output = tempfile::tempdir().expect("output");
    let options = compile_options(input.path(), output.path());
    let first = compile::run(&options).expect("compile");
    assert!(!record(&first, "bad").reused);
    let second = compile::run(&options).expect("resume");
    assert!(record(&second, "bad").reused);
    let retried = compile::run(&CompileOptions {
        retry_failed: true,
        ..options
    })
    .expect("retry");
    assert!(!record(&retried, "bad").reused);
    assert!(matches!(
        record(&retried, "bad").outcome,
        FeedOutcome::Failed { .. }
    ));
}

#[test]
fn compiling_the_same_feed_twice_gives_the_same_file() {
    let stops = grid(0.0, 16);
    let (_one_input, one, _) = compiled_feeds(&[Feed {
        id: "good",
        stops: &stops,
    }]);
    let (_two_input, two, _) = compiled_feeds(&[Feed {
        id: "good",
        stops: &stops,
    }]);
    let file = "timetables/good.timetable";
    assert_eq!(
        fs::read(one.path().join(file)).expect("one"),
        fs::read(two.path().join(file)).expect("two"),
        "the compiled numbering must not depend on the run"
    );
}

/// Loads every tile a mode wrote, checking the manifest and disk agree.
fn load_tiles(output: &Path, manifest: &Manifest, mode: TransferMode) -> Vec<TransferTile> {
    let entries = manifest.tiles.get(mode.as_str()).expect("mode's tiles");
    let root = output.join("transfers");
    let on_disk = fs::read_dir(
        root.join(mode.as_str())
            .join(valinor_walk::transfer_tile::TILE_ZOOM.to_string()),
    )
    .into_iter()
    .flatten()
    .flatten()
    .flat_map(|x| fs::read_dir(x.path()).into_iter().flatten().flatten())
    .count();
    assert_eq!(
        on_disk,
        entries.len(),
        "{mode:?}: a tile not in the manifest"
    );
    entries
        .iter()
        .map(|entry| {
            let path = tile_path(&root, mode, entry.x, entry.y);
            let bytes = fs::read(&path).expect("tile file");
            assert_eq!(bytes.len() as u64, entry.bytes, "{path:?}");
            let tile = decode_tile(&bytes, DecodeLimits::default()).expect("decode");
            assert_eq!((tile.x, tile.y), (entry.x, entry.y));
            assert_eq!(tile.mode, mode);
            assert_eq!(tile.max_seconds, BOUND);
            assert_eq!(format!("{:016x}", tile.manifest_id), manifest.manifest_id);
            assert_eq!(tile.walk_count() as u64, entry.walks);
            tile
        })
        .collect()
}

fn placements(stops: &StopList) -> Vec<FeedPlacement> {
    stops
        .feeds
        .iter()
        .enumerate()
        .map(|(index, feed)| FeedPlacement {
            feed_index: index as u32,
            stops: StopPlacement::Contiguous {
                first: feed.first,
                count: feed.count,
            },
        })
        .collect()
}

/// What [`TransferTable::build`] gives for the same stops in one numbering.
fn built_table(mode: TransferMode, stops: &StopList) -> TransferTable {
    let tiles = TileSets::open(&[street_tiles()]).expect("tiles");
    let costing = costing_for(mode).expect("costing");
    let coordinates: Vec<(f64, f64)> = stops
        .coordinates
        .iter()
        .map(|c| c.expect("a coordinate"))
        .collect();
    let snaps = snap_stops(
        &&tiles,
        &costing,
        &SnapConfig::default(),
        tiles.build_id(),
        &coordinates,
    )
    .expect("snap");
    TransferTable::build(
        &&tiles,
        &costing,
        &Arc::new(snaps),
        BOUND,
        threads(4),
        &|| false,
    )
    .expect("build")
    .expect("not interrupted")
}

#[test]
fn tiles_load_to_exactly_what_the_table_build_gives_and_cross_feeds() {
    let one = grid(0.0, 64);
    let two = grid(0.0006, 64);
    let (_input, compiled, report) = compiled_feeds(&[
        Feed {
            id: "feed-b",
            stops: &two,
        },
        Feed {
            id: "feed-a",
            stops: &one,
        },
    ]);
    let output = tempfile::tempdir().expect("output");
    let options = transfer_options(compiled.path(), output.path());
    let manifest = transfers::run(&options).expect("transfers");
    assert_eq!(
        manifest
            .feeds
            .iter()
            .map(|f| f.id.as_str())
            .collect::<Vec<_>>(),
        ["feed-a", "feed-b"],
        "the manifest's order is the feed index"
    );
    let stops = StopList::load(compiled.path(), &report.feeds, threads(2)).expect("stops");
    assert_eq!(stops.len(), 128);

    for mode in TransferMode::ALL {
        let tiles = load_tiles(output.path(), &manifest, mode);
        let built = built_table(mode, &stops);
        assert!(
            built.walk_count() > 500,
            "{mode:?}: only {} walks",
            built.walk_count()
        );
        let places = placements(&stops);
        let (loaded, load_report) = assemble_transfer_table(
            &LoadRequest {
                mode,
                manifest_id: u64::from_str_radix(&manifest.manifest_id, 16).expect("hex"),
                max_seconds: BOUND,
                local_stop_count: stops.len() as u32,
                feeds: &places,
            },
            &tiles,
        )
        .expect("assemble");
        assert_eq!(load_report.walks_to_absent_feeds, 0);
        assert_eq!(load_report.walks_from_absent_feeds, 0);
        assert_eq!(
            loaded, built,
            "{mode:?}: tiles and the table build disagree"
        );

        // Walks between feeds exist, and the totals count them.
        let totals = manifest.totals.get(mode.as_str()).expect("totals");
        let cross = (0..stops.len() as u32)
            .flat_map(|from| {
                built
                    .walks_from(StopId::new(from))
                    .map(move |to| (from, to))
            })
            .filter(|&(from, (to, _))| stops.address(from).0 != stops.address(to.into_inner()).0)
            .count();
        assert!(cross > 100, "{mode:?}: {cross} cross-feed walks");
        assert_eq!(totals.cross_feed_walks, cross as u64);
        assert_eq!(totals.walks as usize, built.walk_count());

        // Each feed's tile list is exactly the tiles holding walks from its stops.
        for (index, feed) in manifest.feeds.iter().enumerate() {
            let listed: BTreeSet<_> = feed.transfer_tiles[mode.as_str()]
                .iter()
                .map(|t| (t[0], t[1]))
                .collect();
            let holding: BTreeSet<_> = tiles
                .iter()
                .filter(|tile| tile.rows().any(|(feed, _, _)| feed == index as u32))
                .map(|tile| (tile.x, tile.y))
                .collect();
            assert_eq!(listed, holding, "{}", feed.id);
        }
    }
}

/// Pruning only leaves walks out, and counts each one it leaves out.
#[test]
fn pruning_leaves_out_walks_and_counts_them() {
    let one = grid(0.0, 64);
    let two = grid(0.0006, 64);
    let (_input, compiled, _report) = compiled_feeds(&[
        Feed {
            id: "feed-a",
            stops: &one,
        },
        Feed {
            id: "feed-b",
            stops: &two,
        },
    ]);
    let full_out = tempfile::tempdir().expect("output");
    let full = transfers::run(&transfer_options(compiled.path(), full_out.path())).expect("full");
    let pruned_out = tempfile::tempdir().expect("output");
    let mut options = transfer_options(compiled.path(), pruned_out.path());
    options.prune_dominated = true;
    let pruned = transfers::run(&options).expect("pruned");
    let walks = |output: &Path, manifest: &Manifest, mode| -> BTreeSet<(u32, u32, u32, u32)> {
        load_tiles(output, manifest, mode)
            .iter()
            .flat_map(|tile| {
                tile.rows()
                    .flat_map(|(feed, stop, walks)| {
                        walks
                            .iter()
                            .map(move |walk| (feed, stop, walk.feed, walk.stop))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let mut left_out = 0;
    for mode in TransferMode::ALL {
        let all = walks(full_out.path(), &full, mode);
        let kept = walks(pruned_out.path(), &pruned, mode);
        assert!(kept.is_subset(&all), "{mode:?}: pruning added a walk");
        let totals = &pruned.totals[mode.as_str()];
        assert_eq!(totals.pruned as usize, all.len() - kept.len(), "{mode:?}");
        assert_eq!(full.totals[mode.as_str()].pruned, 0);
        let per_tile: u64 = pruned.tiles[mode.as_str()].iter().map(|t| t.pruned).sum();
        // Tiles whose every walk went aren't written, so they aren't counted per tile.
        assert!(per_tile <= totals.pruned);
        left_out += totals.pruned;
    }
    assert!(left_out > 0, "nothing on the fixture's lines was dominated");
}

#[test]
fn a_client_dropping_a_feed_gets_the_same_walks_without_it() {
    let one = grid(0.0, 40);
    let two = grid(0.0006, 40);
    let (_input, compiled, report) = compiled_feeds(&[
        Feed {
            id: "feed-a",
            stops: &one,
        },
        Feed {
            id: "feed-b",
            stops: &two,
        },
    ]);
    let output = tempfile::tempdir().expect("output");
    let manifest =
        transfers::run(&transfer_options(compiled.path(), output.path())).expect("transfers");
    let stops = StopList::load(compiled.path(), &report.feeds, threads(2)).expect("stops");
    let mode = TransferMode::Walking;
    let tiles = load_tiles(output.path(), &manifest, mode);
    let full = built_table(mode, &stops);
    let keep = stops.feeds[1];
    // A client with only feed B numbers its stops from zero.
    let places = [FeedPlacement {
        feed_index: 1,
        stops: StopPlacement::Contiguous {
            first: 0,
            count: keep.count,
        },
    }];
    let (loaded, load_report) = assemble_transfer_table(
        &LoadRequest {
            mode,
            manifest_id: u64::from_str_radix(&manifest.manifest_id, 16).expect("hex"),
            max_seconds: BOUND,
            local_stop_count: keep.count,
            feeds: &places,
        },
        tiles.iter().filter(|tile| {
            manifest.feeds[1].transfer_tiles[mode.as_str()].contains(&[tile.x, tile.y])
        }),
    )
    .expect("assemble");
    assert!(
        load_report.walks_from_absent_feeds > 0,
        "feed A's rows were skipped"
    );
    assert!(
        load_report.walks_to_absent_feeds > 0,
        "walks to feed A were skipped"
    );
    let offset = keep.first;
    let mut expected = Vec::new();
    for from in offset..offset + keep.count {
        for (to, seconds) in full.walks_from(StopId::new(from)) {
            let to = to.into_inner();
            if to >= offset && to < offset + keep.count {
                expected.push((from - offset, to - offset, seconds));
            }
        }
    }
    let actual: Vec<_> = (0..keep.count)
        .flat_map(|from| {
            loaded
                .walks_from(StopId::new(from))
                .map(move |(to, seconds)| (from, to.into_inner(), seconds))
        })
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), loaded.walk_count());
    assert!(expected.len() > 20);
}

#[test]
fn stops_with_no_street_are_reported_and_a_failed_feed_keeps_its_index() {
    // Far outside the Andorra fixtures, so nothing can be snapped.
    let lost = [(10.0, 10.0), (10.0002, 10.0)];
    let input = input_dir(&[
        ("a-lost", gtfs_zip(&lost)),
        ("b-broken", b"not a zip".to_vec()),
        ("c-good", gtfs_zip(&grid(0.0, 16))),
    ]);
    let compiled = tempfile::tempdir().expect("compiled");
    let report = compile::run(&compile_options(input.path(), compiled.path())).expect("compile");
    assert!(matches!(
        record(&report, "b-broken").outcome,
        FeedOutcome::Failed { .. }
    ));
    let output = tempfile::tempdir().expect("output");
    let manifest =
        transfers::run(&transfer_options(compiled.path(), output.path())).expect("transfers");
    assert_eq!(manifest.feeds[1].id, "b-broken");
    assert_eq!(manifest.feeds[1].stop_count, 0);
    assert!(matches!(
        manifest.feeds[1].outcome,
        FeedOutcome::Failed { .. }
    ));
    assert_eq!(manifest.feeds[2].id, "c-good");
    assert!(!manifest.feeds[2].transfer_tiles["walking"].is_empty());
    let walking = &manifest.totals["walking"];
    assert_eq!(walking.stops, 18);
    assert_eq!(manifest.feeds[0].stats["walking"].unsnapped, 2);
    assert!(manifest.feeds[0].transfer_tiles["walking"].is_empty());
    assert_eq!(
        walking.unsnapped,
        2 + manifest.feeds[2].stats["walking"].unsnapped as u64
    );

    let report_lines = fs::read_to_string(output.path().join("transfers/unsnapped-walking.jsonl"))
        .expect("report");
    assert!(report_lines.lines().any(|line| {
        let value: serde_json::Value = serde_json::from_str(line).expect("json");
        value["feed"] == "a-lost" && value["gtfs_stop_id"].as_str() != Some("")
    }));

    let reread = read_manifest(output.path()).expect("read");
    assert_eq!(reread.manifest_id, manifest.manifest_id);
    assert_eq!(reread.feeds.len(), 3);
}

#[test]
fn the_transfer_stage_stops_cleanly_on_bad_input() {
    let output = tempfile::tempdir().expect("output");
    // No compile report.
    let empty = tempfile::tempdir().expect("compiled");
    assert!(matches!(
        transfers::run(&transfer_options(empty.path(), output.path())),
        Err(BuildError::Read { .. })
    ));
    assert!(!output.path().join("transfers/manifest.json").exists());

    // Unreadable street tiles.
    let (_input, compiled, _report) = compiled_feeds(&[Feed {
        id: "good",
        stops: &grid(0.0, 16),
    }]);
    let mut options = transfer_options(compiled.path(), output.path());
    options.street_tiles = output.path().join("no-such-tiles.tar");
    assert!(matches!(
        transfers::run(&options),
        Err(BuildError::StreetTiles { .. })
    ));

    // A compiled feed whose timetable has since been corrupted.
    fs::write(
        compiled.path().join("timetables/good.timetable"),
        b"garbage",
    )
    .expect("corrupt");
    let error = transfers::run(&transfer_options(compiled.path(), output.path()))
        .expect_err("corrupted timetable");
    assert!(
        matches!(error, BuildError::TimetableChanged { .. }),
        "{error}"
    );
    assert!(!output.path().join("transfers").exists());
}

#[test]
fn a_rerun_replaces_what_an_earlier_run_published() {
    let (_input, compiled, _report) = compiled_feeds(&[Feed {
        id: "good",
        stops: &grid(0.0, 24),
    }]);
    let output = tempfile::tempdir().expect("output");
    let mut options = transfer_options(compiled.path(), output.path());
    options.modes = vec![TransferMode::Walking];
    let first = transfers::run(&options).expect("first");
    // A stale tile from before must not survive, and staging must be gone.
    let stale = tile_path(
        &output.path().join("transfers"),
        TransferMode::Walking,
        1,
        1,
    );
    fs::create_dir_all(stale.parent().expect("parent")).expect("mkdir");
    fs::write(&stale, b"stale").expect("write");
    let second = transfers::run(&options).expect("second");
    assert_eq!(first.manifest_id, second.manifest_id);
    assert!(!stale.exists());
    assert!(!output.path().join("transfers.staging").exists());
    assert_eq!(
        load_tiles(output.path(), &second, TransferMode::Walking).len(),
        second.tiles["walking"].len()
    );
}

fn rewrite_listing(input: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut listing: serde_json::Value =
        serde_json::from_slice(&fs::read(input.join("feeds.json")).expect("read")).expect("parse");
    edit(&mut listing);
    fs::write(
        input.join("feeds.json"),
        serde_json::to_vec(&listing).expect("json"),
    )
    .expect("write");
}

/// `compile.rs` says a previous failure is kept "on unchanged input". This input did change:
/// the bytes on disk now are the archive that `feeds.json` names.
#[test]
fn a_repaired_archive_is_compiled() {
    let good = gtfs_zip(&grid(0.0, 16));
    let input = input_dir(&[("good", good.clone())]);
    let archive = input.path().join("feeds/good.gtfs.zip");
    fs::write(&archive, b"half a download").expect("corrupt");
    let output = tempfile::tempdir().expect("output");
    let options = compile_options(input.path(), output.path());
    let first = compile::run(&options).expect("compile");
    assert!(matches!(
        record(&first, "good").outcome,
        FeedOutcome::Failed { .. }
    ));
    fs::write(&archive, &good).expect("repair");
    let second = compile::run(&options).expect("resume");
    assert_eq!(
        record(&second, "good").outcome,
        FeedOutcome::Compiled,
        "reused = {}",
        record(&second, "good").reused
    );
}

/// A read error of the archive is recorded as a failure of the listed archive, and sticks.
#[test]
fn an_unreadable_archive_is_not_a_permanent_failure() {
    let good = gtfs_zip(&grid(0.0, 16));
    let input = input_dir(&[("good", good.clone())]);
    let archive = input.path().join("feeds/good.gtfs.zip");
    fs::remove_file(&archive).expect("remove");
    let output = tempfile::tempdir().expect("output");
    let options = compile_options(input.path(), output.path());
    compile::run(&options).expect("compile");
    fs::write(&archive, &good).expect("restore");
    let second = compile::run(&options).expect("resume");
    assert_eq!(record(&second, "good").outcome, FeedOutcome::Compiled);
}

/// A license correction upstream with the same archive must reach the manifest.
#[test]
fn reuse_keeps_current_metadata() {
    let (input, output, first) = compiled_feeds(&[Feed {
        id: "good",
        stops: &grid(0.0, 16),
    }]);
    assert!(!record(&first, "good").reused);
    rewrite_listing(input.path(), |listing| {
        listing["feeds"][0]["license"] = serde_json::json!({"spdx": "CC-BY-4.0"});
        listing["feeds"][0]["name"] = serde_json::json!("renamed");
        listing["feeds"][0]["source"] = serde_json::json!({"name": "moved", "kind": "test"});
    });
    let second = compile::run(&compile_options(input.path(), output.path())).expect("resume");
    let feed = record(&second, "good");
    assert!(feed.reused);
    assert_eq!(
        (
            feed.license.get().contains("CC-BY-4.0"),
            feed.name.as_str(),
            feed.source.get().contains("moved")
        ),
        (true, "renamed", true),
        "license {} name {} source {}",
        feed.license.get(),
        feed.name,
        feed.source.get()
    );
}

/// `manifest_id` claims to cover "everything that decides what a stop address means".
/// Stops renumbered with the same archive, as after a killed compile or a compiler change,
/// must not produce tiles that a client can load against the old numbering.
#[test]
fn a_renumbered_timetable_is_refused() {
    let stops = grid(0.0, 24);
    let (_input, compiled, report) = compiled_feeds(&[Feed {
        id: "good",
        stops: &stops,
    }]);
    let output = tempfile::tempdir().expect("output");
    let mut options = transfer_options(compiled.path(), output.path());
    options.modes = vec![TransferMode::Walking];
    transfers::run(&options).expect("first");

    let mut reversed = stops.clone();
    reversed.reverse();
    let (_other_input, other, _) = compiled_feeds(&[Feed {
        id: "good",
        stops: &reversed,
    }]);
    let path = compiled.path().join("timetables/good.timetable");
    fs::copy(other.path().join("timetables/good.timetable"), &path).expect("swap");

    let second_out = tempfile::tempdir().expect("output");
    let mut options = transfer_options(compiled.path(), second_out.path());
    options.modes = vec![TransferMode::Walking];
    let outcome = transfers::run(&options);
    let on_disk = sha256_hex(&fs::read(&path).expect("read"));
    let recorded = record(&report, "good")
        .timetable_sha256
        .clone()
        .expect("sha");
    assert_ne!(on_disk, recorded, "the test did swap the timetable");
    assert!(matches!(outcome, Err(BuildError::TimetableChanged { .. })));
}

/// A failing stage leaves the published tiles, manifest, and reports as they were.
#[test]
fn a_failing_stage_changes_nothing_but_staging() {
    let (_input, compiled, _) = compiled_feeds(&[Feed {
        id: "good",
        stops: &grid(0.0, 24),
    }]);
    let output = tempfile::tempdir().expect("output");
    let mut options = transfer_options(compiled.path(), output.path());
    options.modes = vec![TransferMode::Walking];
    let first = transfers::run(&options).expect("first");
    let report = output.path().join("transfers/unsnapped-walking.jsonl");
    let earlier_report = fs::read(&report).expect("report");

    // Blocks the retired directory's place, so the stage fails at publishing, its last step.
    fs::write(output.path().join("transfers.replaced"), b"in the way").expect("write");
    options.modes = TransferMode::ALL.to_vec();
    assert!(transfers::run(&options).is_err());
    assert_eq!(fs::read(&report).expect("report"), earlier_report);
    let after = read_manifest(output.path()).expect("the manifest still reads");
    assert_eq!(after.manifest_id, first.manifest_id);
    assert!(!after.tiles.contains_key("cycling"));
    assert!(!output.path().join("transfers/cycling").exists());
}

/// A later run that builds fewer modes leaves nothing of the mode it dropped.
#[test]
fn a_rerun_with_fewer_modes_leaves_no_stale_report() {
    let (_input, compiled, _) = compiled_feeds(&[Feed {
        id: "good",
        stops: &grid(0.0, 24),
    }]);
    let output = tempfile::tempdir().expect("output");
    let mut options = transfer_options(compiled.path(), output.path());
    transfers::run(&options).expect("first");
    assert!(
        output
            .path()
            .join("transfers/unsnapped-cycling.jsonl")
            .exists()
    );
    options.modes = vec![TransferMode::Walking];
    let second = transfers::run(&options).expect("second");
    assert!(!second.tiles.contains_key("cycling"));
    assert!(
        !output
            .path()
            .join("transfers/unsnapped-cycling.jsonl")
            .exists(),
        "a report for a mode this manifest doesn't have"
    );
    assert!(!output.path().join("transfers/cycling").exists());
}

/// One feed entirely outside the tile set, on its own batch: must be unsnapped, not fatal.
#[test]
fn a_batch_with_nothing_to_snap_is_not_fatal() {
    let (_input, compiled, _) = compiled_feeds(&[Feed {
        id: "lost",
        stops: &[(10.0, 10.0), (10.0002, 10.0), (-45.0, 170.0)],
    }]);
    let output = tempfile::tempdir().expect("output");
    let manifest = transfers::run(&transfer_options(compiled.path(), output.path()))
        .expect("transfers failed on a feed outside the tile set");
    assert_eq!(manifest.totals["walking"].unsnapped, 3);
}

/// A coordinate GTFS shouldn't have, if the compiler lets it through.
#[test]
fn a_bad_coordinate_does_not_stop_the_stage() {
    let input = input_dir(&[(
        "wild",
        gtfs_zip(&[(95.0, 200.0), (42.5048, 1.5155), (42.5049, 1.5156)]),
    )]);
    let compiled = tempfile::tempdir().expect("compiled");
    let report = compile::run(&compile_options(input.path(), compiled.path())).expect("compile");
    eprintln!(
        "wild: outcome {:?} stops {}",
        record(&report, "wild").outcome,
        record(&report, "wild").stops
    );
    if !matches!(record(&report, "wild").outcome, FeedOutcome::Compiled) {
        return;
    }
    let output = tempfile::tempdir().expect("output");
    let result = transfers::run(&transfer_options(compiled.path(), output.path()));
    assert!(result.is_ok(), "{:?}", result.err());
}

/// The real Washington build: what a client assembles from tiles equals a fresh build over the
/// same numbering, for every walk.
#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn real_tiles_equal_a_fresh_table() {
    let root = Path::new("/mnt/maps/cardinal/transit/global/us-wa-2026-11-09");
    let report = compile::CompileReport::read(root).expect("report");
    let mut feeds = report.feeds.clone();
    feeds.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    let stops = StopList::load(root, &feeds, threads(8)).expect("stops");
    let manifest = read_manifest(root).expect("manifest");
    let tile_set = TileSets::open(&[PathBuf::from(
        "/mnt/maps/cardinal/routing-elevation-i2b/washington/tiles.tar",
    )])
    .expect("tiles");
    for mode in TransferMode::ALL {
        let entries = manifest.tiles.get(mode.as_str()).expect("tiles");
        let dir = root.join("transfers");
        let tiles: Vec<TransferTile> = entries
            .iter()
            .map(|e| {
                decode_tile(
                    &fs::read(tile_path(&dir, mode, e.x, e.y)).expect("read"),
                    DecodeLimits::default(),
                )
                .expect("decode")
            })
            .collect();
        let places = placements(&stops);
        let (loaded, report) = assemble_transfer_table(
            &LoadRequest {
                mode,
                manifest_id: u64::from_str_radix(&manifest.manifest_id, 16).expect("hex"),
                max_seconds: BOUND,
                local_stop_count: stops.len() as u32,
                feeds: &places,
            },
            &tiles,
        )
        .expect("assemble");
        assert_eq!(
            report.walks_to_absent_feeds + report.walks_from_absent_feeds,
            0
        );
        let coordinates: Vec<(f64, f64)> = stops
            .coordinates
            .iter()
            .map(|c| c.unwrap_or((0.0, 0.0)))
            .collect();
        let costing = costing_for(mode).expect("costing");
        let snaps = snap_stops(
            &&tile_set,
            &costing,
            &SnapConfig::default(),
            tile_set.build_id(),
            &coordinates,
        )
        .expect("snap");
        let built = TransferTable::build(
            &&tile_set,
            &costing,
            &Arc::new(snaps),
            BOUND,
            threads(24),
            &|| false,
        )
        .expect("build")
        .expect("done");
        eprintln!(
            "{mode:?}: walks loaded {} built {}",
            loaded.walk_count(),
            built.walk_count()
        );
        assert_eq!(loaded, built, "{mode:?}");
    }
}

#[test]
fn changing_the_seated_wait_recompiles_an_unchanged_archive() {
    let (input, output, first) = compiled_feeds(&[Feed {
        id: "good",
        stops: &grid(0.0, 16),
    }]);
    assert!(!record(&first, "good").reused);
    let options = CompileOptions {
        seated_wait_seconds: 1,
        ..compile_options(input.path(), output.path())
    };
    let second = compile::run(&options).expect("second run");
    assert!(
        !record(&second, "good").reused,
        "--seated-wait-seconds changed but the old timetable was reused"
    );
}

/// Every file under `root`, by its path relative to `root`.
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                directories.push(path);
            } else {
                let relative = path.strip_prefix(root).expect("under root").to_path_buf();
                files.insert(relative, fs::read(&path).expect("read"));
            }
        }
    }
    files
}

/// The snapping change that moves exactly one of [`one_feed_build`]'s stops: at 390 m rather than
/// 400 m, one stop no longer reaches a street, as a stop-snapping fix might move one stop.
fn one_snap_moved(options: &mut TransferOptions) {
    options.snap.max_distance_m = 390;
}

/// One feed compiled into a directory a client can load: its compile output is the build's.
fn one_feed_build() -> (TempDir, TempDir) {
    let (input, compiled, _report) = compiled_feeds(&[Feed {
        id: "good",
        stops: &grid(0.0, 24),
    }]);
    (input, compiled)
}

#[test]
fn the_manifest_id_changes_exactly_when_what_a_client_reads_does() {
    let (_input, build) = one_feed_build();
    let options = transfer_options(build.path(), build.path());
    let first = transfers::run(&options).expect("first");
    let first_files = tree(&build.path().join("transfers"));
    assert_eq!(
        format!(
            "{:016x}",
            manifest_id(&read_manifest(build.path()).expect("read"))
        ),
        first.manifest_id,
        "the published manifest's ID is the one its contents give"
    );
    assert!(
        first.tiles["walking"]
            .iter()
            .all(|tile| tile.content_sha256.len() == 64)
    );

    // An unchanged rebuild is the same build, byte for byte.
    let again = transfers::run(&options).expect("again");
    assert_eq!(again.manifest_id, first.manifest_id);
    assert!(tree(&build.path().join("transfers")) == first_files);

    // One stop's snap moves, and with it its walks.
    let mut moved_options = options.clone();
    one_snap_moved(&mut moved_options);
    let moved = transfers::run(&moved_options).expect("moved");
    assert_eq!(
        moved.totals["walking"].unsnapped,
        first.totals["walking"].unsnapped + 1,
        "the fixture moved: this should unsnap exactly one stop"
    );
    assert_ne!(moved.manifest_id, first.manifest_id);
    for tile in load_tiles(build.path(), &moved, TransferMode::Walking) {
        assert_eq!(format!("{:016x}", tile.manifest_id), moved.manifest_id);
    }
    // Exactly the snap file of the moved stop's feed changed, in each mode it moved in.
    let snap_sha = |manifest: &Manifest| manifest.feeds[0].stop_snaps["walking"].sha256.clone();
    assert_ne!(snap_sha(&moved), snap_sha(&first));
}

#[test]
fn a_build_is_byte_identical_for_any_thread_count() {
    let (_input, compiled) = one_feed_build();
    let mut published = Vec::new();
    for count in [1, 3, 8] {
        let output = tempfile::tempdir().expect("output");
        let mut options = transfer_options(compiled.path(), output.path());
        options.threads = threads(count);
        transfers::run(&options).expect("transfers");
        published.push(tree(&output.path().join("transfers")));
    }
    assert!(!published[0].is_empty());
    assert!(published.iter().all(|files| *files == published[0]));
}

/// The names of a client's prepared tables.
fn prepared_tables(build: &Path) -> BTreeSet<String> {
    fs::read_dir(build.join("prepared"))
        .expect("prepared")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn a_rebuild_with_one_snap_moved_replaces_a_clients_prepared_tables() {
    let (_input, build) = one_feed_build();
    let options = transfer_options(build.path(), build.path());
    let first = transfers::run(&options).expect("first");
    let loaded = TransitBuild::load(build.path(), None).expect("loads");
    assert_eq!(
        loaded.walking.walk_count() as u64,
        first.totals["walking"].walks
    );
    drop(loaded);
    let first_tables = prepared_tables(build.path());
    let first_transfers = tempfile::tempdir().expect("copy");
    for (relative, bytes) in tree(&build.path().join("transfers")) {
        let path = first_transfers.path().join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, bytes).expect("write");
    }

    // The rebuild lands in place, under a client that has prepared the first.
    let mut moved_options = options.clone();
    one_snap_moved(&mut moved_options);
    let moved = transfers::run(&moved_options).expect("moved");
    assert_ne!(moved.totals["walking"].walks, first.totals["walking"].walks);
    let loaded = TransitBuild::load(build.path(), None).expect("loads");
    assert_eq!(
        loaded.walking.walk_count() as u64,
        moved.totals["walking"].walks
    );
    assert_eq!(
        loaded.cycling.walk_count() as u64,
        moved.totals["cycling"].walks
    );
    drop(loaded);
    let moved_tables = prepared_tables(build.path());
    assert_eq!(moved_tables.len(), first_tables.len());
    assert!(
        moved_tables.is_disjoint(&first_tables),
        "every mode's table was prepared again: {first_tables:?} then {moved_tables:?}"
    );

    // A tile of the first build among the rebuild's is refused, not mixed in.
    fs::remove_dir_all(build.path().join("prepared")).expect("remove");
    let entry = &moved.tiles["walking"][0];
    let tile = |root: &Path| tile_path(root, TransferMode::Walking, entry.x, entry.y);
    fs::copy(
        tile(first_transfers.path()),
        tile(&build.path().join("transfers")),
    )
    .expect("copy");
    let error = TransitBuild::load(build.path(), None).expect_err("a tile of another build");
    assert!(matches!(error, BuildLoadError::Assemble { .. }), "{error}");
}

/// A copy of a build directory, without anything a client has prepared in it.
fn copy_build(root: &Path) -> TempDir {
    let copy = tempfile::tempdir().expect("copy");
    for (relative, bytes) in tree(root) {
        if relative.starts_with("prepared") {
            continue;
        }
        let path = copy.path().join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, bytes).expect("write");
    }
    copy
}

/// Each answer as JSON, from a fresh planner over a load of `build` with `feeds`.
fn plans_of(
    build: &Path,
    feeds: Option<&[String]>,
    requests: &[valinor_transit_plan::TransitRequest],
) -> Result<Vec<serde_json::Value>, String> {
    use valinor_transit_plan::{StreetConfig, StreetPlanner};
    let loaded = TransitBuild::load(build, feeds).map_err(|error| format!("load: {error}"))?;
    let planner = StreetPlanner::from_build(loaded, &[street_tiles()], StreetConfig::default())
        .map_err(|error| format!("open: {error}"))?;
    requests
        .iter()
        .map(|request| {
            let response = planner
                .plan(request, "en-US", &|| false)
                .map_err(|error| format!("plan: {error}"))?;
            Ok(serde_json::to_value(&response).expect("json"))
        })
        .collect()
}

/// Many threads opening one build at once, with the same feeds and with others, plan exactly
/// as a single-threaded open of a fresh copy does: while every table is still being prepared,
/// and once they are.
#[test]
fn concurrent_opens_plan_like_a_single_threaded_open() {
    use valinor_gtfs::LatLon;
    use valinor_transit_plan::TransitRequest;

    let (_input, build, _report) = compiled_feeds(&[
        Feed {
            id: "feed-a",
            stops: &grid(0.0, 40),
        },
        Feed {
            id: "feed-b",
            stops: &grid(0.0006, 40),
        },
    ]);
    transfers::run(&transfer_options(build.path(), build.path())).expect("transfers");
    // 2026-06-15 07:55 in Andorra; each feed's one trip leaves its first stop at 08:00.
    let departure = 1_781_502_900;
    let near_first = LatLon {
        lat: 42.5046,
        lon: 1.5153,
    };
    let near_last = LatLon {
        lat: 42.5141,
        lon: 1.5225,
    };
    let requests = [
        TransitRequest::new(near_first, near_last, departure, departure - 60),
        TransitRequest::new(near_last, near_first, departure, departure - 60),
    ];
    let sets: [Option<Vec<String>>; 3] = [
        None,
        Some(vec!["feed-a".to_owned()]),
        Some(vec!["feed-b".to_owned()]),
    ];
    let reference: Vec<Vec<serde_json::Value>> = sets
        .iter()
        .map(|feeds| {
            let fresh = copy_build(build.path());
            plans_of(fresh.path(), feeds.as_deref(), &requests).expect("plans alone")
        })
        .collect();
    // Not vacuous: every feed set rides its trip on the way along it.
    for answers in &reference {
        let rides = answers[0]["journeys"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|journey| journey["legs"].as_array().into_iter().flatten())
            .any(|leg| leg["type"] == "transit");
        assert!(rides, "{}", answers[0]);
    }

    let shared = copy_build(build.path());
    let opens_per_set = 4;
    for label in ["preparing", "prepared"] {
        let barrier = std::sync::Barrier::new(sets.len() * opens_per_set);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..opens_per_set)
                .flat_map(|_| sets.iter().enumerate())
                .map(|(index, feeds)| {
                    let (barrier, requests, path) = (&barrier, &requests, shared.path());
                    scope.spawn(move || {
                        barrier.wait();
                        (index, plans_of(path, feeds.as_deref(), requests))
                    })
                })
                .collect();
            for handle in handles {
                let (index, answers) = handle.join().expect("no panic");
                let answers = answers.unwrap_or_else(|error| panic!("{label}: {error}"));
                assert!(
                    answers == reference[index],
                    "{label}: feed set {:?} planned differently",
                    sets[index]
                );
            }
        });
    }
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).expect("read")).expect("json")
}

/// Copies a build-relative file from one build directory into another.
fn copy_into(from: &Path, to: &Path, relative: &str) {
    let target = to.join(relative);
    fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
    fs::copy(from.join(relative), target).expect("copy");
}

#[test]
fn the_feed_index_places_each_stop_and_its_manifest_parts_open_one_feed() {
    // Two feeds in Andorra, and one far away in another zoom 6 tile.
    let one = grid(0.0, 40);
    let two = grid(0.0006, 40);
    let far = [(42.0, 10.0), (42.0005, 10.0005)];
    let (_input, build, _report) = compiled_feeds(&[
        Feed {
            id: "feed-a",
            stops: &one,
        },
        Feed {
            id: "feed-b",
            stops: &two,
        },
        Feed {
            id: "feed-far",
            stops: &far,
        },
    ]);
    let manifest =
        transfers::run(&transfer_options(build.path(), build.path())).expect("transfers");
    let full = TransitBuild::load(build.path(), Some(&["feed-b".to_owned()])).expect("full");

    let out = tempfile::tempdir().expect("out");
    let index_dir = out.path().join("index");
    let options = crate::feed_index::FeedIndexOptions {
        build: build.path().to_path_buf(),
        output: index_dir.clone(),
        replace: false,
        threads: threads(2),
    };
    let report = crate::feed_index::run(&options).expect("feed index");
    assert_eq!(report.feeds, 3);
    assert_eq!(report.uncompiled, 0);
    assert_eq!(report.stops_without_location, 0);
    assert!(!out.path().join("index.part").exists());

    // Every stop's zoom 12 cell is listed for its feed, in the zoom 6 tile holding it.
    let mut expected: BTreeMap<(u32, u32), BTreeMap<String, BTreeSet<[u32; 2]>>> = BTreeMap::new();
    for (id, stops) in [
        ("feed-a", &one[..]),
        ("feed-b", &two[..]),
        ("feed-far", &far[..]),
    ] {
        for &(lat, lon) in stops {
            let (x, y) = crate::slippy::tile_of(lat, lon, crate::feed_index::CELL_ZOOM);
            expected
                .entry((x >> 6, y >> 6))
                .or_default()
                .entry(id.to_owned())
                .or_default()
                .insert([x, y]);
        }
    }
    assert!(expected.len() >= 2, "the far feed is in another index tile");
    let mut found: BTreeMap<(u32, u32), BTreeMap<String, BTreeSet<[u32; 2]>>> = BTreeMap::new();
    for (relative, bytes) in tree(&index_dir.join("index")) {
        let parts: Vec<String> = relative
            .iter()
            .map(|part| part.to_string_lossy().into_owned())
            .collect();
        let x: u32 = parts[0].parse().expect("x");
        let y: u32 = parts[1].trim_end_matches(".json").parse().expect("y");
        let document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(document["tile"], serde_json::json!([x, y]));
        for feed in document["feeds"].as_array().expect("feeds") {
            let cells: Vec<[u32; 2]> =
                serde_json::from_value(feed["cells"].clone()).expect("cells");
            let mut sorted = cells.clone();
            sorted.sort_unstable();
            assert_eq!(cells, sorted, "cells are sorted");
            let set: BTreeSet<[u32; 2]> = cells.into_iter().collect();
            let previous = found
                .entry((x, y))
                .or_default()
                .insert(feed["id"].as_str().expect("id").to_owned(), set);
            assert!(previous.is_none(), "a feed is listed once per index tile");
        }
    }
    assert_eq!(found, expected);
    assert_eq!(
        report.cells,
        expected
            .values()
            .flat_map(|feeds| feeds.values().flatten())
            .collect::<BTreeSet<_>>()
            .len()
    );

    // A feed's summary names its agency and the files to download, with their sizes.
    let summary = read_json(&index_dir.join("feeds/feed-b.json"));
    assert_eq!(summary["index"], 1);
    assert_eq!(summary["agencies"], serde_json::json!(["Andorra Bus"]));
    assert_eq!(summary["display_name"], "Andorra Bus");
    assert_eq!(summary["first_service_date"], "2026-01-01");
    assert_eq!(summary["last_service_date"], "2027-12-31");
    let timetable = &manifest.feeds[1];
    assert_eq!(
        summary["timetable"]["sha256"],
        timetable.timetable_sha256.clone().expect("sha")
    );
    assert_eq!(
        summary["timetable"]["bytes"],
        fs::metadata(
            build
                .path()
                .join(timetable.timetable.as_ref().expect("path"))
        )
        .expect("stat")
        .len()
    );
    for mode in TransferMode::ALL {
        let files = &summary["modes"][mode.as_str()];
        let snaps = &timetable.stop_snaps[mode.as_str()];
        assert_eq!(
            files["snaps"]["path"],
            snaps
                .file
                .strip_prefix("transfers/")
                .expect("under transfers")
        );
        assert_eq!(files["snaps"]["sha256"], snaps.sha256.as_str());
        let tiles = &timetable.transfer_tiles[mode.as_str()];
        assert_eq!(files["transfer_tiles"], tiles.len());
        let bytes: u64 = manifest.tiles[mode.as_str()]
            .iter()
            .filter(|tile| tiles.contains(&[tile.x, tile.y]))
            .map(|tile| tile.bytes)
            .sum();
        assert_eq!(files["transfer_bytes"], bytes);
    }
    let all = read_json(&index_dir.join("feeds.json"));
    assert_eq!(all["feeds"][1], summary);
    assert_eq!(all["manifest_id"], manifest.manifest_id.as_str());

    // A client with only feed B opens a manifest of the shared part and B's entry as published,
    // index included, and gets the same walks as from the whole build.
    let shared = read_json(&index_dir.join("manifest/shared.json"));
    assert!(shared.get("tiles").is_none() && shared.get("feeds").is_none());
    assert_eq!(shared["feed_count"], 3);
    let ids = shared["feed_ids"].as_array().expect("ids").clone();
    assert_eq!(
        ids,
        serde_json::json!(["feed-a", "feed-b", "feed-far"])
            .as_array()
            .expect("a")
            .clone()
    );
    let entry = read_json(&index_dir.join("manifest/feeds/feed-b.json"));
    assert_eq!(entry["index"], 1);
    let mut client_manifest = shared.clone();
    let object = client_manifest.as_object_mut().expect("object");
    object.remove("feed_ids");
    object.insert("feeds".to_owned(), serde_json::json!([entry]));
    let client = tempfile::tempdir().expect("client");
    fs::create_dir_all(client.path().join("transfers")).expect("mkdir");
    fs::write(
        client.path().join("transfers/manifest.json"),
        serde_json::to_vec(&client_manifest).expect("json"),
    )
    .expect("write manifest");
    copy_into(
        build.path(),
        client.path(),
        timetable.timetable.as_ref().expect("path"),
    );
    for mode in TransferMode::ALL {
        copy_into(
            build.path(),
            client.path(),
            &timetable.stop_snaps[mode.as_str()].file,
        );
        for &[x, y] in &timetable.transfer_tiles[mode.as_str()] {
            let relative = tile_path(Path::new(""), mode, x, y);
            copy_into(
                &build.path().join("transfers"),
                &client.path().join("transfers"),
                relative.to_str().expect("utf-8"),
            );
        }
    }
    let opened = TransitBuild::load(client.path(), Some(&["feed-b".to_owned()])).expect("opens");
    assert_eq!(opened.report.tiles_absent, [0, 0, 0]);
    assert_eq!(opened.report.walks, full.report.walks);
    assert!(opened.report.walks[0] > 0);
    assert_eq!(opened.set.stop_count(), full.set.stop_count());

    // The realtime sources are passed through as the app reads them.
    assert_eq!(
        read_json(&index_dir.join("realtime.json")),
        serde_json::json!({ "feeds": [] })
    );

    // An existing output is kept unless replacing is asked for.
    assert!(matches!(
        crate::feed_index::run(&options),
        Err(BuildError::OutputExists(_))
    ));
    let replaced = crate::feed_index::run(&crate::feed_index::FeedIndexOptions {
        replace: true,
        ..options
    })
    .expect("replace");
    assert_eq!(replaced, report);
}
