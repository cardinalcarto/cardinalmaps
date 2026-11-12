//! What a client fetches to choose and install feeds from a published build.
//!
//! A build's manifest names every feed, and its global tile list alone is most of its size,
//! so a phone can't fetch it to find the feeds near it. This stage writes, from a finished
//! build, the small files that a client fetches instead:
//!
//! - `index/<x>/<y>.json`, one per zoom 6 tile with any stop in it: the feeds with stops
//!   there, each with the zoom 12 tiles of its stops inside that zoom 6 tile.
//!   Cells come from the stops in each compiled timetable, not from the manifest's
//!   transfer tiles, which leave out stops that didn't snap to a street.
//! - `feeds/<id>.json`, a summary of one compiled feed: names, agencies, license, service
//!   dates, and the files a client downloads for it, with their sizes; and `feeds.json`, all of
//!   them in one file, for browsing the whole build. A client choosing feeds in an area fetches
//!   only the summaries it shows: `feeds.json` is megabytes, a summary about one kilobyte.
//! - `manifest/shared.json`, the manifest's scalar fields with `feed_count`, and its feed IDs
//!   in index order as `feed_ids` (transfer tiles address a stop by its feed's index),
//!   and `manifest/feeds/<id>.json`, each feed's manifest entry with its `index`.
//!   A client opens the manifest made of the shared fields (less `feed_ids`) and, as `feeds`,
//!   the entries of its own feeds in index order; it never needs the global tile list
//!   or another feed's entry.
//! - `realtime.json`, the compile stage's GTFS-RT sources, as `{"feeds": [...]}`.
//!
//! Paths in `feeds.json` are where files are published, relative to the release:
//! `timetables/<id>.timetable`, `snaps/<mode>/<index>.bin`, and
//! `transfers/<mode>/<zoom>/<x>/<y>.bin.zst`. A client stores each file at the path the
//! manifest gives, which for snaps is under `transfers/`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use valinor_gtfs::{CompiledTimetable, civil_date_of};
use valinor_transit::{RouteId, StopId};

use crate::compile::{COMPILE_REPORT, CompileReport};
use crate::error::BuildError;
use crate::feeds::hex_digest;
use crate::manifest::{Manifest, ManifestFeed, manifest_path};
use crate::slippy::tile_of;
use crate::write_atomic;

/// The zoom of the spatial index's files.
pub const INDEX_ZOOM: u8 = 6;

/// The zoom of the transfer tiles, which cells in the index are given at.
const CELL_ZOOM: u8 = valinor_walk::transfer_tile::TILE_ZOOM;

/// Where a snap file is in a build, before its mode and file name.
const BUILD_SNAPS_PREFIX: &str = "transfers/snaps/";

/// Where a snap file is published, before its mode and file name.
const PUBLISHED_SNAPS_PREFIX: &str = "snaps/";

/// How to run the stage.
#[derive(Clone, Debug)]
pub struct FeedIndexOptions {
    /// A finished build: `compile.json`, `timetables/`, and `transfers/manifest.json`.
    pub build: PathBuf,
    /// Where to write the files. It must not exist yet, unless `replace` is set.
    pub output: PathBuf,
    /// Remove `output` first if it exists.
    pub replace: bool,
    /// Threads to read timetables on.
    pub threads: NonZeroUsize,
}

/// What the stage wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FeedIndexReport {
    /// The build's manifest ID.
    pub manifest_id: String,
    /// Compiled feeds summarized.
    pub feeds: usize,
    /// Feeds in the manifest with no timetable, left out.
    pub uncompiled: usize,
    /// Zoom 6 index files written.
    pub index_files: usize,
    /// Distinct zoom 12 cells with a stop.
    pub cells: usize,
    /// Stops whose timetable gives no coordinate, so they are in no cell.
    pub stops_without_location: u64,
}

/// A published file: where it is under the release, its size, and its SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileRef {
    /// Relative to the release.
    pub path: String,
    /// Its size in bytes.
    pub bytes: u64,
    /// In hexadecimal.
    pub sha256: String,
}

/// What a client downloads for one feed and mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModeFiles {
    /// The transfer tiles holding walks from the feed's stops.
    pub transfer_tiles: usize,
    /// Their compressed size, together. Tiles are shared between feeds with nearby stops,
    /// so this is what the feed costs alone.
    pub transfer_bytes: u64,
    /// The feed's stops snapped to the streets.
    pub snaps: FileRef,
}

/// One compiled feed, as `feeds.json` describes it.
#[derive(Clone, Debug, Serialize)]
pub struct FeedSummary {
    /// The feed's ID.
    pub id: String,
    /// Its position in the manifest's feeds, which transfer tiles address it by.
    pub index: u32,
    /// What to call it: its only agency's name, or the feed's name when it has several.
    pub display_name: String,
    /// The feed's name from Transitous.
    pub name: String,
    /// The feed's Transitous region.
    pub region: String,
    /// The names of the agencies that run its trips, sorted and without repeats.
    pub agencies: Vec<String>,
    /// The feed's license, verbatim from the manifest.
    pub license: Box<RawValue>,
    /// The compiled timetable.
    pub timetable: FileRef,
    /// Stops in the timetable.
    pub stops: u32,
    /// The first date any trip runs, as `YYYY-MM-DD`.
    pub first_service_date: Option<String>,
    /// The last date any trip runs, as `YYYY-MM-DD`.
    pub last_service_date: Option<String>,
    /// Zoom 12 cells with a stop of the feed.
    pub cells: usize,
    /// Files by mode.
    pub modes: BTreeMap<String, ModeFiles>,
    /// GTFS-RT URLs for the feed, from the compile stage.
    pub realtime: Vec<String>,
}

#[derive(Serialize)]
struct FeedsDocument<'a> {
    manifest_id: &'a str,
    valhalla_build_id: u32,
    transitous_commit: &'a str,
    fetched_at: &'a str,
    index_zoom: u8,
    cell_zoom: u8,
    feeds: &'a [FeedSummary],
}

#[derive(Serialize)]
struct IndexFeed<'a> {
    id: &'a str,
    cells: Vec<[u32; 2]>,
}

#[derive(Serialize)]
struct IndexDocument<'a> {
    tile: [u32; 2],
    feeds: Vec<IndexFeed<'a>>,
}

/// By zoom 6 tile, each feed with stops there, by manifest position, and its zoom 12 cells.
type IndexTiles = BTreeMap<(u32, u32), Vec<(usize, Vec<[u32; 2]>)>>;

/// What reading one timetable found.
#[derive(Clone, Debug, Default)]
struct FeedContents {
    /// Zoom 12 cells with a stop, sorted.
    cells: BTreeSet<(u32, u32)>,
    agencies: Vec<String>,
    first_service_date: Option<String>,
    last_service_date: Option<String>,
    stops_without_location: u64,
}

/// Writes the index files of a finished build to `options.output`.
///
/// The files are written to a sibling directory first and renamed into place, so `output`
/// either holds a complete index or doesn't exist.
///
/// # Errors
///
/// If the build can't be read, a timetable isn't the one the manifest names, a snap file
/// isn't under `transfers/snaps/`, or the output exists and `replace` isn't set.
///
/// # Performance
///
/// Reads every compiled timetable once, on `options.threads` threads, holding one per thread.
pub fn run(options: &FeedIndexOptions) -> Result<FeedIndexReport, BuildError> {
    if options.output.exists() && !options.replace {
        return Err(BuildError::OutputExists(options.output.clone()));
    }
    let manifest_file = manifest_path(&options.build);
    let (manifest, raw, raw_feeds) = read_manifest_parts(&manifest_file)?;
    let report = CompileReport::read(&options.build)?;
    let realtime = realtime_by_feed(&report, &options.build)?;

    let contents = read_timetables(&options.build, &manifest.feeds, options.threads)?;

    let staging = staging_path(&options.output);
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|source| BuildError::Write {
            path: staging.clone(),
            source,
        })?;
    }
    let write_error = |path: &Path| {
        let path = path.to_path_buf();
        move |source| BuildError::Write { path, source }
    };
    fs::create_dir_all(staging.join("manifest/feeds")).map_err(write_error(&staging))?;
    fs::create_dir_all(staging.join("feeds")).map_err(write_error(&staging))?;

    let tile_bytes = tile_sizes(&manifest);
    let mut summaries = Vec::new();
    let mut by_index_tile = IndexTiles::new();
    let mut all_cells = BTreeSet::new();
    let mut result = FeedIndexReport {
        manifest_id: manifest.manifest_id.clone(),
        ..FeedIndexReport::default()
    };
    // Both lists come from the same bytes, so they are the same length.
    let entries = manifest.feeds.iter().zip(&raw_feeds).zip(&contents);
    for (index, ((feed, raw_feed), found)) in entries.enumerate() {
        let Some(found) = found else {
            result.uncompiled += 1;
            continue;
        };
        if feed.id.is_empty() || feed.id.contains(['/', '\\']) || feed.id.starts_with('.') {
            return Err(BuildError::UnsafeFeedPath(feed.id.clone()));
        }
        let position = u32::try_from(index).map_err(|_| BuildError::TooManyEntries)?;
        let summary = summarize(
            feed,
            position,
            found,
            &tile_bytes,
            &options.build,
            realtime.get(feed.id.as_str()).cloned().unwrap_or_default(),
        )?;
        write_json(
            &staging.join("feeds").join(format!("{}.json", feed.id)),
            &summary,
        )?;
        write_json(
            &staging
                .join("manifest/feeds")
                .join(format!("{}.json", feed.id)),
            &with_index(raw_feed, position, &manifest_file)?,
        )?;
        summaries.push(summary);
        let shift = CELL_ZOOM - INDEX_ZOOM;
        for &(x, y) in &found.cells {
            all_cells.insert((x, y));
            let entry = by_index_tile.entry((x >> shift, y >> shift)).or_default();
            // Feeds come in index order, so a feed's cells in one index tile are consecutive.
            match entry.last_mut() {
                Some((last, cells)) if *last == index => cells.push([x, y]),
                _ => entry.push((index, vec![[x, y]])),
            }
        }
        result.stops_without_location += found.stops_without_location;
    }
    result.feeds = summaries.len();
    result.cells = all_cells.len();

    write_index_tiles(&staging, &manifest, &by_index_tile)?;
    result.index_files = by_index_tile.len();

    write_json(
        &staging.join("feeds.json"),
        &FeedsDocument {
            manifest_id: &manifest.manifest_id,
            valhalla_build_id: manifest.valhalla_build_id,
            transitous_commit: &manifest.transitous_commit,
            fetched_at: &manifest.fetched_at,
            index_zoom: INDEX_ZOOM,
            cell_zoom: CELL_ZOOM,
            feeds: &summaries,
        },
    )?;
    write_json(
        &staging.join("manifest/shared.json"),
        &shared_manifest(&raw, &manifest, &manifest_file)?,
    )?;
    write_json(
        &staging.join("realtime.json"),
        &serde_json::json!({ "feeds": report.realtime }),
    )?;
    write_json(&staging.join("feed-index.json"), &result)?;

    if options.output.exists() {
        fs::remove_dir_all(&options.output).map_err(write_error(&options.output))?;
    }
    fs::rename(&staging, &options.output).map_err(write_error(&options.output))?;
    Ok(result)
}

/// The manifest, typed, and as JSON: its top-level fields, and its feeds' entries in order.
///
/// Entries are published verbatim, so a field this crate doesn't model still reaches clients.
type ManifestParts = (
    Manifest,
    BTreeMap<String, Box<RawValue>>,
    Vec<Box<RawValue>>,
);

fn read_manifest_parts(path: &Path) -> Result<ManifestParts, BuildError> {
    let json_error = |source| BuildError::Json {
        path: path.to_path_buf(),
        source,
    };
    let bytes = fs::read(path).map_err(|source| BuildError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(json_error)?;
    let raw: BTreeMap<String, Box<RawValue>> =
        serde_json::from_slice(&bytes).map_err(json_error)?;
    let feeds = match raw.get("feeds") {
        Some(feeds) => serde_json::from_str(feeds.get()).map_err(json_error)?,
        None => Vec::new(),
    };
    Ok((manifest, raw, feeds))
}

/// Writes `index/<x>/<y>.json` for each zoom 6 tile.
fn write_index_tiles(
    staging: &Path,
    manifest: &Manifest,
    by_index_tile: &IndexTiles,
) -> Result<(), BuildError> {
    for (&(x, y), feeds) in by_index_tile {
        let document = IndexDocument {
            tile: [x, y],
            feeds: feeds
                .iter()
                .filter_map(|(index, cells)| {
                    Some(IndexFeed {
                        id: &manifest.feeds.get(*index)?.id,
                        cells: cells.clone(),
                    })
                })
                .collect(),
        };
        let directory = staging.join("index").join(x.to_string());
        fs::create_dir_all(&directory).map_err(|source| BuildError::Write {
            path: directory.clone(),
            source,
        })?;
        write_json(&directory.join(format!("{y}.json")), &document)?;
    }
    Ok(())
}

/// `<output>.part`, beside the output.
fn staging_path(output: &Path) -> PathBuf {
    let mut name = output
        .file_name()
        .map_or_else(Default::default, std::ffi::OsStr::to_os_string);
    name.push(".part");
    output.with_file_name(name)
}

fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), BuildError> {
    let bytes = serde_json::to_vec(value).map_err(|source| BuildError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    write_atomic(path, &bytes)
}

/// The manifest without its feeds and tiles, with the feeds' IDs in index order and their count.
fn shared_manifest(
    raw: &BTreeMap<String, Box<RawValue>>,
    manifest: &Manifest,
    path: &Path,
) -> Result<BTreeMap<String, Box<RawValue>>, BuildError> {
    let mut shared: BTreeMap<String, Box<RawValue>> = raw
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "feeds" | "tiles"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let ids: Vec<&str> = manifest.feeds.iter().map(|feed| feed.id.as_str()).collect();
    let ids = serde_json::value::to_raw_value(&ids).map_err(|source| BuildError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    shared.insert("feed_ids".to_owned(), ids);
    let count = u32::try_from(manifest.feeds.len()).map_err(|_| BuildError::TooManyEntries)?;
    let count = serde_json::value::to_raw_value(&count).map_err(|source| BuildError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    shared.insert("feed_count".to_owned(), count);
    Ok(shared)
}

/// A manifest entry with `"index"` added.
fn with_index(
    entry: &RawValue,
    index: u32,
    path: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, BuildError> {
    let json_error = |source| BuildError::Json {
        path: path.to_path_buf(),
        source,
    };
    let mut object: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(entry.get()).map_err(json_error)?;
    object.insert("index".to_owned(), index.into());
    Ok(object)
}

/// Each compressed tile's size, by mode and `(x, y)`.
fn tile_sizes(manifest: &Manifest) -> BTreeMap<(&str, u32, u32), u64> {
    manifest
        .tiles
        .iter()
        .flat_map(|(mode, tiles)| {
            tiles
                .iter()
                .map(move |tile| ((mode.as_str(), tile.x, tile.y), tile.bytes))
        })
        .collect()
}

/// The compile stage's GTFS-RT URLs, by feed ID.
fn realtime_by_feed(
    report: &CompileReport,
    build: &Path,
) -> Result<BTreeMap<String, Vec<String>>, BuildError> {
    #[derive(serde::Deserialize)]
    struct Source {
        id: String,
        url: String,
    }
    let mut by_feed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in &report.realtime {
        let source: Source =
            serde_json::from_str(entry.get()).map_err(|source| BuildError::Json {
                path: build.join(COMPILE_REPORT),
                source,
            })?;
        by_feed.entry(source.id).or_default().push(source.url);
    }
    Ok(by_feed)
}

fn summarize(
    feed: &ManifestFeed,
    index: u32,
    found: &FeedContents,
    tile_bytes: &BTreeMap<(&str, u32, u32), u64>,
    build: &Path,
    realtime: Vec<String>,
) -> Result<FeedSummary, BuildError> {
    let mut modes = BTreeMap::new();
    for (mode, snaps) in &feed.stop_snaps {
        let published = snaps
            .file
            .strip_prefix(BUILD_SNAPS_PREFIX)
            .filter(|rest| !rest.is_empty() && !rest.split('/').any(|part| part == ".."))
            .ok_or_else(|| BuildError::UnsafeFeedPath(snaps.file.clone()))?;
        let path = build.join(&snaps.file);
        let bytes = fs::metadata(&path)
            .map_err(|source| BuildError::Read { path, source })?
            .len();
        let tiles = feed.transfer_tiles.get(mode).map_or(&[][..], Vec::as_slice);
        let mut transfer_bytes = 0;
        for &[x, y] in tiles {
            transfer_bytes += tile_bytes
                .get(&(mode.as_str(), x, y))
                .copied()
                .ok_or_else(|| BuildError::MissingTile {
                    mode: mode.clone(),
                    x,
                    y,
                })?;
        }
        modes.insert(
            mode.clone(),
            ModeFiles {
                transfer_tiles: tiles.len(),
                transfer_bytes,
                snaps: FileRef {
                    path: format!("{PUBLISHED_SNAPS_PREFIX}{published}"),
                    bytes,
                    sha256: snaps.sha256.clone(),
                },
            },
        );
    }
    let display_name = match found.agencies.as_slice() {
        [only] => only.clone(),
        _ => feed.name.clone(),
    };
    Ok(FeedSummary {
        id: feed.id.clone(),
        index,
        display_name,
        name: feed.name.clone(),
        region: feed.region.clone(),
        agencies: found.agencies.clone(),
        license: feed.license.clone(),
        timetable: FileRef {
            path: feed.timetable.clone().unwrap_or_default(),
            bytes: feed.timetable_bytes,
            sha256: feed.timetable_sha256.clone().unwrap_or_default(),
        },
        stops: feed.stop_count,
        first_service_date: found.first_service_date.clone(),
        last_service_date: found.last_service_date.clone(),
        cells: found.cells.len(),
        modes,
        realtime,
    })
}

/// Reads each compiled feed's timetable, in manifest order; `None` for a feed with none.
fn read_timetables(
    build: &Path,
    feeds: &[ManifestFeed],
    threads: NonZeroUsize,
) -> Result<Vec<Option<FeedContents>>, BuildError> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<FeedContents>>> = Mutex::new(vec![None; feeds.len()]);
    let first_error: Mutex<Option<BuildError>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..threads.get().min(feeds.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(feed) = feeds.get(index) else {
                        break;
                    };
                    match read_feed(build, feed) {
                        Ok(Some(found)) => {
                            if let Ok(mut results) = results.lock() {
                                results[index] = Some(found);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            if let Ok(mut first) = first_error.lock() {
                                first.get_or_insert(error);
                            }
                            // Stop handing out work; the stage fails anyway.
                            next.store(feeds.len(), Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    });
    if let Some(error) = first_error.into_inner().ok().flatten() {
        return Err(error);
    }
    results.into_inner().map_err(|_| BuildError::WorkerPanicked)
}

fn read_feed(build: &Path, feed: &ManifestFeed) -> Result<Option<FeedContents>, BuildError> {
    let Some(relative) = &feed.timetable else {
        return Ok(None);
    };
    if relative
        .split('/')
        .any(|part| part == ".." || part.is_empty())
    {
        return Err(BuildError::UnsafeFeedPath(relative.clone()));
    }
    let path = build.join(relative);
    let bytes = fs::read(&path).map_err(|source| BuildError::Read { path, source })?;
    if feed.timetable_sha256.as_deref() != Some(hex_digest(&Sha256::digest(&bytes)).as_str()) {
        return Err(BuildError::TimetableChanged {
            feed: feed.id.clone(),
        });
    }
    let timetable = CompiledTimetable::parse(&bytes).map_err(|source| BuildError::Timetable {
        feed: feed.id.clone(),
        source: source.into(),
    })?;
    if u32::try_from(timetable.stop_count()) != Ok(feed.stop_count) {
        return Err(BuildError::TimetableChanged {
            feed: feed.id.clone(),
        });
    }
    let mut found = FeedContents::default();
    for stop in 0..feed.stop_count {
        match timetable
            .stop(StopId::new(stop))
            .and_then(|info| info.coordinate)
        {
            Some(coordinate) => {
                found.cells.insert(tile_of(coordinate.lat, coordinate.lon));
            }
            None => found.stops_without_location += 1,
        }
    }
    found.agencies = agencies(&timetable);
    if let Some(info) = timetable.feeds().find(|info| info.id == feed.id) {
        found.first_service_date = civil_date_of(info.first_service_date)
            .ok()
            .map(|date| date.to_string());
        found.last_service_date = civil_date_of(info.last_service_date)
            .ok()
            .map(|date| date.to_string());
    }
    Ok(Some(found))
}

/// The names of the agencies whose routes have trips, sorted and without repeats.
///
/// The format reaches agencies only through routes, so an agency with no trips in the
/// timetable isn't listed, which is also what a rider would expect.
fn agencies(timetable: &CompiledTimetable<'_>) -> Vec<String> {
    let mut names = BTreeSet::new();
    for pattern in 0..timetable.pattern_count() {
        let Ok(pattern) = u32::try_from(pattern) else {
            break;
        };
        let name = timetable
            .pattern(RouteId::new(pattern))
            .and_then(|pattern| timetable.route(pattern.route))
            .and_then(|route| route.agency)
            .and_then(|agency| timetable.agency(agency))
            .map(|agency| agency.name);
        if let Some(name) = name {
            names.insert(name.to_owned());
        }
    }
    names.into_iter().collect()
}
