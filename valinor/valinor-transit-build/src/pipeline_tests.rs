//! Whole-stage tests over real zip archives and the Andorra street tiles.

#![allow(clippy::all, clippy::pedantic)]

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tempfile::TempDir;
use valinor_router::tiles::TileSets;
use valinor_transit::StopId;
use valinor_walk::transfer_tile::{
    DecodeLimits, FeedPlacement, LoadRequest, StopPlacement, TransferMode, TransferTile,
    assemble_transfer_table, decode_tile,
};
use valinor_walk::{SnapConfig, TransferTable, snap_stops};
use zip::write::SimpleFileOptions;

use crate::compile::{self, COMPILE_REPORT, CompileOptions, FeedOutcome, FeedRecord};
use crate::error::BuildError;
use crate::feeds::hex_digest;
use crate::manifest::{Manifest, read_manifest};
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
        zstd_level: 3,
        modes: TransferMode::ALL.to_vec(),
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
    let on_disk = fs::read_dir(root.join(mode.as_str()).join("12"))
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
    let tiles = TileSets::open(&[street_tiles()], threads(64)).expect("tiles");
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

    let report_lines =
        fs::read_to_string(output.path().join("unsnapped-walking.jsonl")).expect("report");
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
    assert!(!output.path().join("manifest.json").exists());

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
    assert!(matches!(error, BuildError::Timetable { .. }), "{error}");
    assert!(!output.path().join("manifest.json").exists());
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
