//! The transfer stage: tiled walks between every stop of every feed.
//!
//! Walks are computed by the same search as [`TransferTable::build`](valinor_walk::TransferTable),
//! tile by tile of the origin stop's zoom 12 tile, balanced dynamically across the threads.
//! Everything held for the whole run is one list of stops and one index of them on the street
//! network; a worker's tile memory is given back between tiles.
//!
//! Tiles, reports, and the manifest are written under a staging directory, which is renamed to
//! `transfers/` only once everything has succeeded and been flushed to disk.
//! That one rename is the stage's commit point, so a failed or killed run leaves the previous
//! `transfers/` exactly as it was, and no reader can pair one run's tiles with another's manifest.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use valhalla_graphtile::graph_tile::BuildId;
use valhalla_proto::Costing as CostingProto;
use valinor_router::tiles::TileSets;
use valinor_sif::AnyCosting;
use valinor_sif::costing_options::parse_costing;
use valinor_sif::json::JsonValue;
use valinor_transit::StopId;
use valinor_walk::transfer_tile::{
    TILE_FORMAT_VERSION, TILE_ZOOM, TileWalk, TransferMode, TransferTile, TransferTileRow,
};
use valinor_walk::{
    AccessSide, Hop, SavedSnap, SnapConfig, SnapQuality, SnapReport, StopAttachments, StopSnaps,
    StopTables, TableRequest, TableSeed, encode_snap_file, snap_stops, sorted_walks_from,
};

use crate::compile::{CompileReport, FeedOutcome};
use crate::error::BuildError;
use crate::feeds::hex_digest;
use crate::manifest::{
    FeedModeStats, Manifest, ManifestFeed, ModeTotals, SnapFileEntry, TRANSFERS_DIR, TileEntry,
    manifest_id, write_manifest,
};
use crate::slippy::tile_of;
use crate::stops::StopList;
use crate::{sync_dir, write_synced};
use sha2::{Digest, Sha256};

/// Where a run works until it succeeds, relative to the output directory.
const STAGING_DIR: &str = "transfers.staging";
/// Where the previous published directory goes during the swap.
const RETIRED_DIR: &str = "transfers.replaced";

/// How many stops one snapping batch covers: sized for tile-neighbour batches, not memory.
const SNAP_BATCH: usize = 2048;

/// How to run the transfer stage.
#[derive(Clone, Debug)]
pub struct TransferOptions {
    /// The compile stage's output.
    pub compiled: PathBuf,
    /// The street network: a `build_extract` tarball or a tile directory.
    pub street_tiles: PathBuf,
    /// Where to publish `transfers/`, holding the tiles, reports, and manifest.
    pub output: PathBuf,
    /// How many threads to search and snap on.
    pub threads: NonZeroUsize,
    /// The bound for walking transfers, in seconds.
    pub max_seconds_walking: u32,
    /// The bound for cycling transfers, in seconds.
    pub max_seconds_cycling: u32,
    /// The zstd level of the tiles.
    pub zstd_level: i32,
    /// Which modes to build.
    pub modes: Vec<TransferMode>,
}

/// The default zstd level: tiles are read far more than written.
pub const DEFAULT_ZSTD_LEVEL: i32 = 19;

/// What one tile came to, for the manifest.
#[derive(Debug)]
struct TileOutcome {
    entry: TileEntry,
    /// `(feed, walks starting there, walks starting there to another feed)`.
    feeds: Vec<(u32, u64, u64)>,
}

/// What the stage produced, per mode.
#[derive(Debug, Default)]
struct ModeResult {
    tiles: Vec<TileEntry>,
    /// For each feed index, the tiles with walks from that feed, as `(x, y)`.
    feed_tiles: BTreeMap<u32, BTreeSet<(u32, u32)>>,
    feed_stats: BTreeMap<u32, FeedModeStats>,
    totals: ModeTotals,
    unsnapped: Vec<UnsnappedStop>,
    /// Each feed's saved stop snaps, by feed index.
    feed_snaps: BTreeMap<u32, SnapFileEntry>,
    seconds: f64,
}

/// A stop that has no walks in a mode, for the report.
#[derive(Debug)]
struct UnsnappedStop {
    feed: u32,
    stop: u32,
    coordinate: Option<(f64, f64)>,
}

/// Runs the transfer stage.
///
/// # Errors
///
/// Any failure stops the stage with no output change except its staging directory:
/// an unreadable input, street tiles that can't be opened, or a search or disk failure.
pub fn run(options: &TransferOptions) -> Result<Manifest, BuildError> {
    let started = Instant::now();
    let report = CompileReport::read(&options.compiled)?;
    let mut feeds = report.feeds.clone();
    feeds.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    if let Some(pair) = feeds.windows(2).find(|pair| pair[0].id == pair[1].id) {
        return Err(BuildError::DuplicateFeedId(pair[0].id.clone()));
    }
    let tiles = TileSets::open(
        core::slice::from_ref(&options.street_tiles),
        NonZeroUsize::new(64).unwrap_or(NonZeroUsize::MIN),
    )
    .map_err(|source| BuildError::StreetTiles {
        path: options.street_tiles.clone(),
        source: Box::new(source),
    })?;
    let build_id = tiles.build_id();
    let stops = StopList::load(&options.compiled, &feeds, options.threads)?;
    tracing::info!(
        feeds = feeds.len(),
        stops = stops.len(),
        "loaded stop lists"
    );

    let id = manifest_id(&feeds, build_id.value());
    let staging = options.output.join(STAGING_DIR);
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|source| BuildError::Write {
            path: staging.clone(),
            source,
        })?;
    }
    fs::create_dir_all(&staging).map_err(|source| BuildError::Write {
        path: staging.clone(),
        source,
    })?;

    let mut ordered = stops_by_tile(&stops);
    ordered.sort_unstable();
    let order: Vec<u32> = ordered.iter().map(|&(_, stop)| stop).collect();
    let jobs = tile_jobs(&ordered);
    tracing::info!(
        stops_located = order.len(),
        tiles = jobs.len(),
        "assigned stops to tiles"
    );

    let mut by_mode = BTreeMap::new();
    for &mode in &options.modes {
        let result = run_mode(
            options, mode, &tiles, build_id, &stops, &order, &jobs, &staging, id,
        )?;
        let unsnapped_path = staging.join(format!("unsnapped-{}.jsonl", mode.as_str()));
        write_unsnapped(
            &unsnapped_path,
            &result.unsnapped,
            &feeds,
            &stops,
            &options.compiled,
        )?;
        by_mode.insert(mode, result);
    }

    let manifest = assemble_manifest(&report, &feeds, build_id, id, &by_mode, options)?;
    write_manifest(&staging, &manifest)?;
    sync_tree(&staging)?;
    publish_staging(&options.output, &staging)?;
    tracing::info!(
        seconds = started.elapsed().as_secs_f64(),
        "transfer stage done"
    );
    Ok(manifest)
}

/// `(tile key, global stop)` of every stop with a coordinate.
fn stops_by_tile(stops: &StopList) -> Vec<(u32, u32)> {
    let mut ordered = Vec::with_capacity(stops.len());
    for (global, coordinate) in stops.coordinates.iter().enumerate() {
        if let Some((lon, lat)) = *coordinate {
            let (x, y) = tile_of(lat, lon);
            if let Ok(global) = u32::try_from(global) {
                ordered.push((tile_key(x, y), global));
            }
        }
    }
    ordered
}

fn tile_key(x: u32, y: u32) -> u32 {
    (x << 12) | y
}

fn tile_from_key(key: u32) -> (u32, u32) {
    (key >> 12, key & 0xfff)
}

/// One tile's stops, as a range of the ordered stop list.
#[derive(Clone, Copy, Debug)]
struct TileJob {
    x: u32,
    y: u32,
    start: usize,
    end: usize,
}

fn tile_jobs(ordered: &[(u32, u32)]) -> Vec<TileJob> {
    let mut jobs = Vec::new();
    let mut start = 0;
    while let Some(&(key, _)) = ordered.get(start) {
        let end = start + ordered[start..].partition_point(|&(k, _)| k == key);
        let (x, y) = tile_from_key(key);
        jobs.push(TileJob { x, y, start, end });
        start = end;
    }
    jobs
}

#[expect(clippy::too_many_arguments, reason = "one mode of one build's inputs")]
fn run_mode(
    options: &TransferOptions,
    mode: TransferMode,
    tiles: &TileSets,
    build_id: BuildId,
    stops: &StopList,
    order: &[u32],
    jobs: &[TileJob],
    staging: &Path,
    id: u64,
) -> Result<ModeResult, BuildError> {
    let started = Instant::now();
    let costing = costing_for(mode)?;
    let snaps = snap_all(options.threads, tiles, &costing, build_id, stops, order)?;
    let snap_count = snaps.reports().len();
    let (mut unsnapped_list, disconnected_stops) = split_reports(&snaps, stops);
    tracing::info!(
        mode = mode.as_str(),
        unsnapped = unsnapped_list.len(),
        disconnected = disconnected_stops.len(),
        reports = snap_count,
        "snapped stops"
    );
    let snaps = Arc::new(snaps);
    let feed_snaps = write_feed_snaps(mode, build_id, stops, &snaps, staging)?;
    let attachments = Arc::new(StopAttachments::new(
        tiles,
        costing.clone(),
        Arc::clone(&snaps),
    )?);
    let max_seconds = match mode {
        TransferMode::Walking => options.max_seconds_walking,
        TransferMode::Cycling => options.max_seconds_cycling,
    };

    let context = Context {
        options,
        mode,
        tiles,
        costing: &costing,
        max_seconds,
        attachments: &attachments,
        stops,
        order,
        staging,
        manifest_id: id,
    };
    let outcomes = run_jobs(&context, jobs)?;
    let mut result = summarize(outcomes, stops, &unsnapped_list, &disconnected_stops);
    result.seconds = started.elapsed().as_secs_f64();
    result.unsnapped = core::mem::take(&mut unsnapped_list);
    result.feed_snaps = feed_snaps;
    tracing::info!(
        mode = mode.as_str(),
        tiles = result.tiles.len(),
        walks = result.totals.walks,
        bytes = result.totals.bytes,
        seconds = result.seconds,
        "built transfers"
    );
    Ok(result)
}

/// Where a mode's stop snaps go, relative to [`TRANSFERS_DIR`].
const SNAPS_DIR: &str = "snaps";

/// Saves each feed's snaps for this mode as one file, in the feed's own stop numbering.
fn write_feed_snaps(
    mode: TransferMode,
    build_id: BuildId,
    stops: &StopList,
    snaps: &StopSnaps,
    staging: &Path,
) -> Result<BTreeMap<u32, SnapFileEntry>, BuildError> {
    let directory = staging.join(SNAPS_DIR).join(mode.as_str());
    fs::create_dir_all(&directory).map_err(|source| BuildError::Write {
        path: directory.clone(),
        source,
    })?;
    let disconnected: std::collections::HashSet<u32> = snaps
        .reports()
        .iter()
        .filter_map(|report| match report {
            SnapReport::Disconnected(stop, _) => Some(stop.into_inner()),
            SnapReport::Unsnapped(_) => None,
        })
        .collect();
    let mut files = BTreeMap::new();
    for (index, feed) in stops.feeds.iter().enumerate() {
        if feed.count == 0 {
            continue;
        }
        let saved: Vec<SavedSnap> = (feed.first..feed.first + feed.count)
            .map(|stop| {
                snaps.point(StopId::new(stop)).map(|point| {
                    let quality = if disconnected.contains(&stop) {
                        SnapQuality::Disconnected
                    } else {
                        SnapQuality::Connected
                    };
                    (point, quality)
                })
            })
            .collect();
        let bytes = encode_snap_file(build_id, &saved);
        let path = directory.join(format!("{index}.bin"));
        write_synced(&path, &bytes).map_err(|source| BuildError::Write {
            path: path.clone(),
            source,
        })?;
        let feed_index = u32::try_from(index).map_err(|_| BuildError::WorkerPanicked)?;
        files.insert(
            feed_index,
            SnapFileEntry {
                file: format!("{TRANSFERS_DIR}/{SNAPS_DIR}/{}/{index}.bin", mode.as_str()),
                sha256: hex_digest(&Sha256::digest(&bytes)),
            },
        );
    }
    Ok(files)
}

/// The costing a mode's transfers use, the same defaults the planner uses.
///
/// # Errors
///
/// Only if the costing can't be built, which would be a bug.
pub fn costing_for(mode: TransferMode) -> Result<AnyCosting, BuildError> {
    let mode_name = mode.as_str();
    let error = |reason: String| BuildError::Costing {
        mode: mode_name,
        reason,
    };
    let costing_type = match mode {
        TransferMode::Walking => valhalla_proto::costing::Type::Pedestrian,
        TransferMode::Cycling => valhalla_proto::costing::Type::Bicycle,
    };
    let doc = JsonValue::parse(b"{}").map_err(|e| error(format!("{e:?}")))?;
    let mut proto = CostingProto::default();
    parse_costing(
        &doc,
        "/none",
        &mut proto,
        &mut Vec::new(),
        Some(costing_type),
    )
    .map_err(|e| error(format!("{e:?}")))?;
    AnyCosting::new(&proto).map_err(|e| error(e.to_string()))
}

fn snap_all(
    threads: NonZeroUsize,
    tiles: &TileSets,
    costing: &AnyCosting,
    build_id: BuildId,
    stops: &StopList,
    order: &[u32],
) -> Result<StopSnaps, BuildError> {
    let snaps = Mutex::new(StopSnaps::new(build_id, stops.len()));
    let batches: Vec<&[u32]> = order.chunks(SNAP_BATCH).collect();
    let next = AtomicUsize::new(0);
    let first_error: Mutex<Option<BuildError>> = Mutex::new(None);
    let failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..threads.get() {
            scope.spawn(|| {
                let config = SnapConfig::default();
                while !failed.load(Ordering::Relaxed) {
                    let Some(batch) = batches.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let coordinates: Vec<(f64, f64)> = batch
                        .iter()
                        .filter_map(|&stop| stops.coordinates.get(stop as usize).copied().flatten())
                        .collect();
                    let outcome = snap_stops(&tiles, costing, &config, build_id, &coordinates)
                        .map_err(BuildError::from)
                        .and_then(|part| {
                            let mut snaps = snaps.lock().map_err(|_| BuildError::WorkerPanicked)?;
                            snaps.absorb(&part, batch).map_err(BuildError::from)
                        });
                    if let Err(error) = outcome {
                        failed.store(true, Ordering::Relaxed);
                        if let Ok(mut first) = first_error.lock() {
                            first.get_or_insert(error);
                        }
                    }
                }
            });
        }
    });
    if let Some(error) = first_error.into_inner().ok().flatten() {
        return Err(error);
    }
    snaps.into_inner().map_err(|_| BuildError::WorkerPanicked)
}

/// Unsnapped stops, by global stop, and the disconnected ones.
fn split_reports(snaps: &StopSnaps, stops: &StopList) -> (Vec<UnsnappedStop>, Vec<u32>) {
    let mut unsnapped = Vec::new();
    let mut disconnected = Vec::new();
    for report in snaps.reports() {
        match *report {
            SnapReport::Unsnapped(stop) => {
                let global = stop.into_inner();
                let (feed, number) = stops.address(global);
                unsnapped.push(UnsnappedStop {
                    feed,
                    stop: number,
                    coordinate: stops.coordinates.get(global as usize).copied().flatten(),
                });
            }
            SnapReport::Disconnected(stop, _) => disconnected.push(stop.into_inner()),
        }
    }
    // Stops with no coordinate never reached Loki, so report them too.
    for (global, coordinate) in stops.coordinates.iter().enumerate() {
        if coordinate.is_none()
            && let Ok(global) = u32::try_from(global)
        {
            let (feed, number) = stops.address(global);
            unsnapped.push(UnsnappedStop {
                feed,
                stop: number,
                coordinate: None,
            });
        }
    }
    unsnapped.sort_unstable_by_key(|s| (s.feed, s.stop));
    (unsnapped, disconnected)
}

struct Context<'a> {
    options: &'a TransferOptions,
    mode: TransferMode,
    tiles: &'a TileSets,
    costing: &'a AnyCosting,
    max_seconds: u32,
    attachments: &'a Arc<StopAttachments>,
    stops: &'a StopList,
    order: &'a [u32],
    staging: &'a Path,
    manifest_id: u64,
}

fn run_jobs(context: &Context<'_>, jobs: &[TileJob]) -> Result<Vec<TileOutcome>, BuildError> {
    let next = AtomicUsize::new(0);
    let outcomes: Mutex<Vec<TileOutcome>> = Mutex::new(Vec::new());
    let first_error: Mutex<Option<BuildError>> = Mutex::new(None);
    let failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..context.options.threads.get() {
            scope.spawn(|| {
                let mut tables = StopTables::new(context.tiles, context.costing.clone());
                tables.bind_shared(Arc::clone(context.attachments));
                while !failed.load(Ordering::Relaxed) {
                    let Some(&job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    match build_tile(context, &mut tables, job) {
                        Ok(Some(outcome)) => {
                            if let Ok(mut outcomes) = outcomes.lock() {
                                outcomes.push(outcome);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            failed.store(true, Ordering::Relaxed);
                            if let Ok(mut first) = first_error.lock() {
                                first.get_or_insert(error);
                            }
                        }
                    }
                    let done = next.load(Ordering::Relaxed);
                    if done.is_multiple_of(1024) {
                        tracing::info!(
                            mode = context.mode.as_str(),
                            done,
                            total = jobs.len(),
                            "tiles done"
                        );
                    }
                }
            });
        }
    });
    if let Some(error) = first_error.into_inner().ok().flatten() {
        return Err(error);
    }
    outcomes
        .into_inner()
        .map_err(|_| BuildError::WorkerPanicked)
}

/// Computes one tile's walks and writes its file, if any stop in it has a walk.
fn build_tile(
    context: &Context<'_>,
    tables: &mut StopTables<&TileSets, AnyCosting>,
    job: TileJob,
) -> Result<Option<TileOutcome>, BuildError> {
    tables.forget_tiles();
    let origins = context.order.get(job.start..job.end).unwrap_or(&[]);
    let mut rows: Vec<(u32, u32, Vec<TileWalk>)> = Vec::new();
    let mut per_feed: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
    let mut walk_total = 0_u64;
    let mut cross_total = 0_u64;
    for &origin in origins {
        let origin_id = StopId::new(origin);
        let seeds: Vec<TableSeed> = tables.stop_seeds(origin_id)?;
        if seeds.is_empty() {
            continue;
        }
        tables.search(&TableRequest {
            side: AccessSide::Access,
            seeds: &seeds,
            max_seconds: context.max_seconds,
            interrupt: &|| false,
        })?;
        let hops: Vec<Hop> = sorted_walks_from(tables, origin_id);
        if hops.is_empty() {
            continue;
        }
        let (origin_feed, origin_stop) = context.stops.address(origin);
        let mut walks = Vec::with_capacity(hops.len());
        let mut cross = 0_u64;
        for hop in hops {
            let (feed, stop) = context.stops.address(hop.stop);
            if feed != origin_feed {
                cross += 1;
            }
            walks.push(TileWalk {
                feed,
                stop,
                seconds: hop.seconds,
            });
        }
        let entry = per_feed.entry(origin_feed).or_default();
        entry.0 += walks.len() as u64;
        entry.1 += cross;
        walk_total += walks.len() as u64;
        cross_total += cross;
        rows.push((origin_feed, origin_stop, walks));
    }
    if rows.is_empty() {
        return Ok(None);
    }
    // `origins` is sorted by global stop, and so by `(feed, stop)`, so the rows already are.
    let rows_view: Vec<TransferTileRow<'_>> = rows
        .iter()
        .map(|(feed, stop, walks)| TransferTileRow {
            feed: *feed,
            stop: *stop,
            walks,
        })
        .collect();
    let tile = TransferTile::from_rows(
        context.mode,
        (job.x, job.y),
        context.manifest_id,
        context.max_seconds,
        &rows_view,
    )?;
    let raw = tile.as_bytes();
    let compressed =
        zstd::bulk::compress(raw, context.options.zstd_level).map_err(BuildError::Compress)?;
    let path = tile_path(context.staging, context.mode, job.x, job.y);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| BuildError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    write_synced(&path, &compressed).map_err(|source| BuildError::Write {
        path: path.clone(),
        source,
    })?;
    Ok(Some(TileOutcome {
        entry: TileEntry {
            x: job.x,
            y: job.y,
            origins: u32::try_from(rows.len()).map_err(|_| BuildError::TooManyEntries)?,
            walks: walk_total,
            cross_feed_walks: cross_total,
            bytes: compressed.len() as u64,
            raw_bytes: raw.len() as u64,
        },
        feeds: per_feed
            .into_iter()
            .map(|(feed, (walks, cross))| (feed, walks, cross))
            .collect(),
    }))
}

/// Where a tile goes under `root`.
#[must_use]
pub fn tile_path(root: &Path, mode: TransferMode, x: u32, y: u32) -> PathBuf {
    root.join(mode.as_str())
        .join(TILE_ZOOM.to_string())
        .join(x.to_string())
        .join(format!("{y}.bin.zst"))
}

/// Gathers one mode's tile outcomes into its totals and per-feed lists.
fn summarize(
    outcomes: Vec<TileOutcome>,
    stops: &StopList,
    unsnapped: &[UnsnappedStop],
    disconnected: &[u32],
) -> ModeResult {
    let mut result = ModeResult::default();
    let mut totals = ModeTotals::default();
    for outcome in outcomes {
        totals.tiles += 1;
        totals.origins += u64::from(outcome.entry.origins);
        totals.walks += outcome.entry.walks;
        totals.cross_feed_walks += outcome.entry.cross_feed_walks;
        totals.bytes += outcome.entry.bytes;
        totals.raw_bytes += outcome.entry.raw_bytes;
        let tile = (outcome.entry.x, outcome.entry.y);
        for (feed, walks, cross) in outcome.feeds {
            result.feed_tiles.entry(feed).or_default().insert(tile);
            let stats = result.feed_stats.entry(feed).or_default();
            stats.walks += walks;
            stats.cross_feed_walks += cross;
        }
        result.tiles.push(outcome.entry);
    }
    result.tiles.sort_unstable_by_key(|tile| (tile.x, tile.y));
    totals.stops = stops.len() as u64;
    totals.unsnapped = unsnapped.len() as u64;
    totals.disconnected = disconnected.len() as u64;
    totals.snapped = totals.stops - totals.unsnapped;
    for stop in unsnapped {
        result.feed_stats.entry(stop.feed).or_default().unsnapped += 1;
    }
    for &global in disconnected {
        result
            .feed_stats
            .entry(stops.address(global).0)
            .or_default()
            .disconnected += 1;
    }
    // Snapped per feed is what's left of the feed's stops after those with no snap.
    for (index, feed) in stops.feeds.iter().enumerate() {
        let Ok(index) = u32::try_from(index) else {
            continue;
        };
        let stats = result.feed_stats.entry(index).or_default();
        stats.snapped = feed.count - stats.unsnapped;
    }
    result.totals = totals;
    result
}

fn assemble_manifest(
    report: &CompileReport,
    feeds: &[crate::compile::FeedRecord],
    build_id: BuildId,
    id: u64,
    by_mode: &BTreeMap<TransferMode, ModeResult>,
    options: &TransferOptions,
) -> Result<Manifest, BuildError> {
    let mut manifest_feeds = Vec::with_capacity(feeds.len());
    for (index, feed) in feeds.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| BuildError::TooManyEntries)?;
        let mut stats = BTreeMap::new();
        let mut transfer_tiles = BTreeMap::new();
        let mut stop_snaps = BTreeMap::new();
        for (mode, result) in by_mode {
            if let Some(entry) = result.feed_snaps.get(&index) {
                stop_snaps.insert(mode.as_str().to_owned(), entry.clone());
            }
            stats.insert(
                mode.as_str().to_owned(),
                result.feed_stats.get(&index).copied().unwrap_or_default(),
            );
            transfer_tiles.insert(
                mode.as_str().to_owned(),
                result
                    .feed_tiles
                    .get(&index)
                    .map(|set| set.iter().map(|&(x, y)| [x, y]).collect())
                    .unwrap_or_default(),
            );
        }
        manifest_feeds.push(ManifestFeed {
            id: feed.id.clone(),
            name: feed.name.clone(),
            region: feed.region.clone(),
            sha256: feed.sha256.clone(),
            license: feed.license.clone(),
            source: feed.source.clone(),
            outcome: feed.outcome.clone(),
            timetable: feed.timetable.clone(),
            timetable_bytes: feed.timetable_bytes,
            timetable_sha256: feed.timetable_sha256.clone(),
            stop_count: feed.stops,
            has_current_service: feed.has_current_service,
            last_service_date: feed.last_service_date.clone(),
            stats,
            transfer_tiles,
            stop_snaps,
        });
    }
    let mut tiles = BTreeMap::new();
    let mut totals = BTreeMap::new();
    for (mode, result) in by_mode {
        tiles.insert(mode.as_str().to_owned(), result.tiles.clone());
        totals.insert(mode.as_str().to_owned(), result.totals);
    }
    Ok(Manifest {
        transitous_commit: report.transitous_commit.clone(),
        fetched_at: report.fetched_at.clone(),
        manifest_id: format!("{id:016x}"),
        valhalla_build_id: u32::from(build_id.value()),
        tile_format_version: TILE_FORMAT_VERSION,
        zoom: TILE_ZOOM,
        walking_bound_seconds: options.max_seconds_walking,
        cycling_bound_seconds: options.max_seconds_cycling,
        feeds: manifest_feeds,
        tiles,
        totals,
    })
}

/// Flushes every directory under `root`, so the files written in it survive a power loss
/// (the files themselves are synced as they are written).
fn sync_tree(root: &Path) -> Result<(), BuildError> {
    let entries = fs::read_dir(root).map_err(|source| BuildError::Read {
        path: root.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| BuildError::Read {
            path: root.to_path_buf(),
            source,
        })?;
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            sync_tree(&entry.path())?;
        }
    }
    sync_dir(root)
}

/// Publishes the finished staging directory as `transfers/`, by a single rename.
///
/// The old directory is moved aside first, so `transfers/` is briefly absent, never mixed.
/// A failure here, or a kill, leaves staging and any retired copy for the next run to clear.
fn publish_staging(output: &Path, staging: &Path) -> Result<(), BuildError> {
    let target = output.join(TRANSFERS_DIR);
    let retired = output.join(RETIRED_DIR);
    let write_error = |path: PathBuf| move |source| BuildError::Write { path, source };
    if retired.exists() {
        fs::remove_dir_all(&retired).map_err(write_error(retired.clone()))?;
    }
    if target.exists() {
        fs::rename(&target, &retired).map_err(write_error(retired.clone()))?;
    }
    fs::rename(staging, &target).map_err(write_error(target.clone()))?;
    sync_dir(output)?;
    if retired.exists() {
        fs::remove_dir_all(&retired).map_err(write_error(retired.clone()))?;
    }
    sync_dir(output)
}

fn write_unsnapped(
    path: &Path,
    unsnapped: &[UnsnappedStop],
    feeds: &[crate::compile::FeedRecord],
    stops: &StopList,
    compiled: &Path,
) -> Result<(), BuildError> {
    let write_error = |source| BuildError::Write {
        path: path.to_path_buf(),
        source,
    };
    let mut out = String::new();
    let mut names: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for stop in unsnapped {
        let ids = match names.entry(stop.feed) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(gtfs_ids(compiled, feeds, stops, stop.feed)?)
            }
        };
        let feed_id = feeds
            .get(stop.feed as usize)
            .map_or("", |feed| feed.id.as_str());
        let gtfs_id = ids.get(stop.stop as usize).map_or("", String::as_str);
        let (lon, lat) = stop.coordinate.unwrap_or((f64::NAN, f64::NAN));
        let line = serde_json::json!({
            "feed": feed_id,
            "stop": stop.stop,
            "gtfs_stop_id": gtfs_id,
            "lat": if lat.is_nan() { serde_json::Value::Null } else { lat.into() },
            "lon": if lon.is_nan() { serde_json::Value::Null } else { lon.into() },
        });
        out.push_str(&line.to_string());
        out.push('\n');
    }
    write_synced(path, out.as_bytes()).map_err(write_error)
}

/// The GTFS stop IDs of one feed, only when something needs reporting.
fn gtfs_ids(
    compiled: &Path,
    feeds: &[crate::compile::FeedRecord],
    stops: &StopList,
    feed: u32,
) -> Result<Vec<String>, BuildError> {
    let Some(record) = feeds.get(feed as usize) else {
        return Ok(Vec::new());
    };
    if record.outcome != FeedOutcome::Compiled {
        return Ok(Vec::new());
    }
    let Some(relative) = &record.timetable else {
        return Ok(Vec::new());
    };
    let path = compiled.join(relative);
    let bytes = fs::read(&path).map_err(|source| BuildError::Read { path, source })?;
    let timetable =
        valinor_gtfs::CompiledTimetable::parse(&bytes).map_err(|source| BuildError::Timetable {
            feed: record.id.clone(),
            source: source.into(),
        })?;
    let count = stops.feeds.get(feed as usize).map_or(0, |f| f.count);
    Ok((0..count)
        .map(|stop| {
            timetable
                .stop(StopId::new(stop))
                .map_or_else(String::new, |info| info.gtfs_id.to_owned())
        })
        .collect())
}
