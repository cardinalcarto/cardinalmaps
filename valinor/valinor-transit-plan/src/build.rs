//! Loading a transit build directory: its timetables and its transfer tiles.
//!
//! A build (see `valinor-transit-build`) is `timetables/` and `transfers/` with a `manifest.json`
//! listing feeds in index order. A client may have any subset of the feeds, and any subset of the
//! tiles: an offline download covers an area, not every stop of a large agency, so a loaded feed
//! may have stops in tiles it never got. Those stops simply have no walks from them.
//!
//! The manifest is checked three ways before anything is used: tiles carry its ID, timetables
//! must match their SHA-256, and street tiles must match its build ID (checked when opening).

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use valhalla_graphtile::graph_tile::BuildId;
use valinor_gtfs::{LoadError, LoadedTimetable, SetError, TimetableSet};
use valinor_transit::StopId;
use valinor_walk::transfer_tile::{
    DEFAULT_MAX_DECOMPRESSED_BYTES, DecodeLimits, FeedPlacement, LoadRequest, StopPlacement,
    TILE_FORMAT_VERSION, TILE_ZOOM, TileDecodeError, TileLoadError, TransferMode, TransferTile,
    assemble_transfer_table, decode_tile,
};
use valinor_walk::{
    FLAT_TABLE_MAGIC, FlatTableError, StopSnaps, TableKey, TransferTable, decode_snap_file,
};

/// The default bound on one tile's decompressed size.
pub fn default_decode_limits() -> DecodeLimits {
    DecodeLimits {
        max_decompressed_bytes: DEFAULT_MAX_DECOMPRESSED_BYTES,
    }
}

/// The largest compiled timetable that will be read, in bytes.
///
/// Far beyond any real feed (King County's is 14 MB), and a bound on what a manifest can name.
pub(crate) const MAX_TIMETABLE_BYTES: u64 = 4 << 30;

/// The largest manifest that will be read, in bytes.
const MAX_MANIFEST_BYTES: u64 = 256 << 20;

/// The largest compressed transfer tile that will be read, in bytes.
const MAX_TILE_BYTES: u64 = 256 << 20;

/// Why a build directory couldn't be loaded.
#[derive(Debug, Error)]
pub enum BuildLoadError {
    /// A file of the build couldn't be read.
    #[error("read {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
    /// A path in the manifest leaves the build directory, or isn't a simple relative path.
    #[error("use the path {0:?} from the manifest, which isn't inside the build")]
    UnsafePath(String),
    /// A file of the build isn't a regular file, or is larger than it can reasonably be.
    #[error("read {path}: it isn't a regular file of at most {limit} bytes")]
    UnsuitableFile {
        /// The file.
        path: PathBuf,
        /// The largest size allowed.
        limit: u64,
    },
    /// The manifest isn't valid.
    #[error("parse the manifest {path}: {source}")]
    Manifest {
        /// The manifest's path.
        path: PathBuf,
        /// What went wrong.
        source: serde_json::Error,
    },
    /// The manifest's ID isn't hexadecimal.
    #[error("read manifest ID {0:?}")]
    ManifestId(String),
    /// The manifest is of a tile format this build can't read.
    #[error("read tile format {found} (this build reads {supported})")]
    TileFormat {
        /// The manifest's.
        found: u16,
        /// This build's.
        supported: u16,
    },
    /// The manifest's tiles are of another zoom.
    #[error("load tiles at zoom {0}")]
    Zoom(u8),
    /// A feed that was asked for isn't in the manifest.
    #[error("load unknown feed {0}")]
    UnknownFeed(String),
    /// A feed that was asked for has no timetable, because it didn't compile.
    #[error("load feed {0}, which has no compiled timetable")]
    NotCompiled(String),
    /// A timetable's bytes don't match the manifest.
    #[error(
        "use the timetable of feed {feed}: its SHA-256 is {found}, the manifest says {expected}"
    )]
    TimetableMismatch {
        /// The feed.
        feed: String,
        /// From the manifest.
        expected: String,
        /// Of the file.
        found: String,
    },
    /// The manifest lists no saved stop snaps for a feed and mode.
    #[error("load saved stop snaps of feed {feed} for {mode}: the manifest has none")]
    MissingSnaps {
        /// The feed.
        feed: String,
        /// The travel mode.
        mode: &'static str,
    },
    /// A snap file isn't what the manifest says, or can't be decoded.
    #[error("use the stop snaps of feed {feed}: {reason}")]
    Snaps {
        /// The feed.
        feed: String,
        /// What's wrong with them.
        reason: String,
    },
    /// A timetable holds something other than its feed.
    #[error("use the timetable of feed {feed}: {reason}")]
    TimetableContents {
        /// The feed.
        feed: String,
        /// What's wrong with it.
        reason: &'static str,
    },
    /// A timetable couldn't be loaded.
    #[error("load the timetable {path}: {source}")]
    Timetable {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: LoadError,
    },
    /// The timetables can't be numbered together.
    #[error(transparent)]
    Set(#[from] SetError),
    /// A prepared table couldn't be written.
    #[error("prepare the transfer table {path}: {source}")]
    Prepare {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
    /// A prepared table just written couldn't be opened.
    #[error("open the prepared transfer table {path}: {source}")]
    Flat {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: FlatTableError,
    },
    /// A tile couldn't be decoded.
    #[error("decode the transfer tile {path}: {source}")]
    Tile {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: TileDecodeError,
    },
    /// The tiles couldn't be assembled into a table.
    #[error("assemble {mode} transfers: {source}")]
    Assemble {
        /// The travel mode.
        mode: &'static str,
        /// What went wrong.
        source: TileLoadError,
    },
    /// The street tiles aren't the ones this build's walks were computed on.
    #[error("use street build {tiles} with transfers built on street build {manifest}")]
    StreetBuildMismatch {
        /// The manifest's build ID.
        manifest: u32,
        /// The opened street tiles' build ID.
        tiles: u32,
    },
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    manifest_id: String,
    valhalla_build_id: u32,
    tile_format_version: u16,
    zoom: u8,
    walking_bound_seconds: u32,
    cycling_bound_seconds: u32,
    feeds: Vec<ManifestFeedFile>,
}

#[derive(Debug, Deserialize)]
struct ManifestFeedFile {
    id: String,
    timetable: Option<String>,
    timetable_sha256: Option<String>,
    stop_count: u32,
    #[serde(default)]
    transfer_tiles: std::collections::BTreeMap<String, Vec<[u32; 2]>>,
    #[serde(default)]
    stop_snaps: std::collections::BTreeMap<String, ManifestSnapFile>,
}

#[derive(Debug, Deserialize)]
struct ManifestSnapFile {
    file: String,
    sha256: String,
}

/// What loading a build found, for logging and checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildLoadReport {
    /// Tiles the prepared table covers, per mode (they are only read when it is rebuilt).
    pub tiles_read: [usize; 2],
    /// Tiles the loaded feeds use but aren't on disk, per mode.
    pub tiles_absent: [usize; 2],
    /// Walks assembled, per mode.
    pub walks: [usize; 2],
}

/// One feed set of a transit build, ready for a planner: timetables and walks.
#[derive(Debug)]
pub struct TransitBuild {
    /// The loaded timetables.
    pub set: TimetableSet,
    /// Walks between stops on foot, for those stops whose tiles are present.
    pub walking: TransferTable,
    /// Walks between stops by bicycle.
    pub cycling: TransferTable,
    /// Every stop's snap on foot, as the build made them, in the set's stop numbering.
    pub walking_snaps: StopSnaps,
    /// Every stop's snap by bicycle.
    pub cycling_snaps: StopSnaps,
    /// The manifest's street build, checked when opening a planner.
    pub valhalla_build_id: u32,
    /// What was found on disk.
    pub report: BuildLoadReport,
}

impl TransitBuild {
    /// Assembles a build's flat transfer tables on disk, without keeping anything.
    ///
    /// This is the install step: run it after a download (a deployment or the device's installer)
    /// so that opening a planner has nothing to assemble. Opening does it anyway if it finds no
    /// matching prepared table, and it costs only the first open.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn prepare(
        directory: &Path,
        feeds: Option<&[String]>,
    ) -> Result<BuildLoadReport, BuildLoadError> {
        Ok(Self::load(directory, feeds)?.report)
    }

    /// Loads a build directory's timetables and a mapped table of walks per travel mode.
    ///
    /// `feeds` chooses feeds by ID; `None` loads every feed that compiled.
    /// A mode's table is the flat file under `prepared/` for this manifest, set of feeds, bound,
    /// and whichever tiles are present; if there is none it is assembled from those tiles and
    /// published atomically, and tables for other sets are removed.
    /// Files are memory-mapped; to replace one, rename over it; don't modify it in place.
    ///
    /// # Performance
    ///
    /// Hashes each timetable, validates and maps it, and stats each tile.
    /// Only when a table has to be assembled does it decode tiles, once.
    ///
    /// # Errors
    ///
    /// If the manifest can't be read, a named feed is unknown or has no timetable, a timetable
    /// doesn't match the manifest, or a tile present on disk can't be decoded or is from another
    /// build. A tile that isn't on disk is not an error.
    pub fn load(directory: &Path, feeds: Option<&[String]>) -> Result<Self, BuildLoadError> {
        Self::load_with_limits(directory, feeds, default_decode_limits())
    }

    /// As [`Self::load`], with a limit on how large a tile may decompress to.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn load_with_limits(
        directory: &Path,
        feeds: Option<&[String]>,
        limits: DecodeLimits,
    ) -> Result<Self, BuildLoadError> {
        let transfers = directory.join("transfers");
        let manifest_path = transfers.join("manifest.json");
        let bytes = read_bounded(&manifest_path, MAX_MANIFEST_BYTES)?;
        let manifest: ManifestFile =
            serde_json::from_slice(&bytes).map_err(|source| BuildLoadError::Manifest {
                path: manifest_path.clone(),
                source,
            })?;
        if manifest.tile_format_version != TILE_FORMAT_VERSION {
            return Err(BuildLoadError::TileFormat {
                found: manifest.tile_format_version,
                supported: TILE_FORMAT_VERSION,
            });
        }
        if manifest.zoom != TILE_ZOOM {
            return Err(BuildLoadError::Zoom(manifest.zoom));
        }
        let manifest_id = u64::from_str_radix(&manifest.manifest_id, 16)
            .map_err(|_| BuildLoadError::ManifestId(manifest.manifest_id.clone()))?;

        let selected = select_feeds(&manifest, feeds)?;
        let mut files = Vec::with_capacity(selected.len());
        for &index in &selected {
            files.push(load_timetable(directory, &manifest.feeds[index])?);
        }
        let set = TimetableSet::new(files)?;

        let placements: Vec<FeedPlacement> = selected
            .iter()
            .zip(set.parts())
            .map(|(&index, part)| FeedPlacement {
                feed_index: u32::try_from(index).unwrap_or(u32::MAX),
                stops: StopPlacement::Contiguous {
                    first: part.first_stop(),
                    count: u32::try_from(part.compiled().stop_count()).unwrap_or(u32::MAX),
                },
            })
            .collect();
        let local_stop_count = u32::try_from(set.stop_count()).unwrap_or(u32::MAX);

        let mut report = BuildLoadReport::default();
        let (walking, walking_tiles, walking_absent) = load_mode(
            directory,
            TransferMode::Walking,
            &manifest,
            &selected,
            &placements,
            manifest_id,
            manifest.walking_bound_seconds,
            local_stop_count,
            limits,
        )?;
        let (cycling, cycling_tiles, cycling_absent) = load_mode(
            directory,
            TransferMode::Cycling,
            &manifest,
            &selected,
            &placements,
            manifest_id,
            manifest.cycling_bound_seconds,
            local_stop_count,
            limits,
        )?;
        report.tiles_read = [walking_tiles, cycling_tiles];
        report.tiles_absent = [walking_absent, cycling_absent];
        report.walks = [walking.walk_count(), cycling.walk_count()];
        let build_id = BuildId::new(
            u16::try_from(manifest.valhalla_build_id)
                .map_err(|_| BuildLoadError::ManifestId(manifest.manifest_id.clone()))?,
        );
        let walking_snaps = load_snaps(
            directory,
            TransferMode::Walking,
            &manifest,
            &selected,
            &set,
            build_id,
        )?;
        let cycling_snaps = load_snaps(
            directory,
            TransferMode::Cycling,
            &manifest,
            &selected,
            &set,
            build_id,
        )?;
        Ok(Self {
            set,
            walking,
            cycling,
            walking_snaps,
            cycling_snaps,
            valhalla_build_id: manifest.valhalla_build_id,
            report,
        })
    }
}

/// The positions of the chosen feeds in the manifest, in manifest order.
fn select_feeds(
    manifest: &ManifestFile,
    feeds: Option<&[String]>,
) -> Result<Vec<usize>, BuildLoadError> {
    let Some(wanted) = feeds else {
        return Ok(manifest
            .feeds
            .iter()
            .enumerate()
            .filter(|(_, feed)| feed.timetable.is_some())
            .map(|(index, _)| index)
            .collect());
    };
    let mut selected = BTreeSet::new();
    for id in wanted {
        let index = manifest
            .feeds
            .iter()
            .position(|feed| &feed.id == id)
            .ok_or_else(|| BuildLoadError::UnknownFeed(id.clone()))?;
        if manifest.feeds[index].timetable.is_none() {
            return Err(BuildLoadError::NotCompiled(id.clone()));
        }
        selected.insert(index);
    }
    Ok(selected.into_iter().collect())
}

fn load_timetable(
    directory: &Path,
    feed: &ManifestFeedFile,
) -> Result<LoadedTimetable, BuildLoadError> {
    let relative = feed
        .timetable
        .as_deref()
        .ok_or_else(|| BuildLoadError::NotCompiled(feed.id.clone()))?;
    let path = inside_build(directory, relative)?;
    let found = hex(&sha256_of_file(&path, MAX_TIMETABLE_BYTES)?);
    let expected = feed.timetable_sha256.as_deref().unwrap_or_default();
    if found != expected {
        return Err(BuildLoadError::TimetableMismatch {
            feed: feed.id.clone(),
            expected: expected.to_owned(),
            found,
        });
    }
    // SAFETY: a timetable under a build directory is published by renaming over it, and
    // `TransitBuild::load`'s documentation forbids modifying it in place while it is open.
    let loaded =
        unsafe { LoadedTimetable::map(&path) }.map_err(|source| BuildLoadError::Timetable {
            path: path.clone(),
            source,
        })?;
    let compiled = loaded.timetable();
    if !compiled.feeds().any(|info| info.id == feed.id) {
        return Err(BuildLoadError::TimetableContents {
            feed: feed.id.clone(),
            reason: "it doesn't hold this feed",
        });
    }
    if u32::try_from(compiled.stop_count()) != Ok(feed.stop_count) {
        return Err(BuildLoadError::TimetableContents {
            feed: feed.id.clone(),
            reason: "its stop count differs from the manifest",
        });
    }
    Ok(loaded)
}

/// A path from the manifest, resolved inside the build directory.
///
/// Only plain relative paths are accepted: no root, `..`, or `.` components.
fn inside_build(directory: &Path, relative: &str) -> Result<PathBuf, BuildLoadError> {
    let path = Path::new(relative);
    let plain = !relative.is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !plain {
        return Err(BuildLoadError::UnsafePath(relative.to_owned()));
    }
    Ok(directory.join(path))
}

/// Checks that `path` is a regular file of at most `limit` bytes, before anything reads it.
///
/// Reading a device or a pipe can never end. Symbolic links are followed, since a build may be
/// assembled from links to a shared directory; what they lead to is what is checked.
fn check_regular(path: &Path, limit: u64) -> Result<(), BuildLoadError> {
    let metadata = std::fs::metadata(path).map_err(|source| BuildLoadError::Read {
        path: path.to_owned(),
        source,
    })?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(BuildLoadError::UnsuitableFile {
            path: path.to_owned(),
            limit,
        });
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, BuildLoadError> {
    check_regular(path, limit)?;
    std::fs::read(path).map_err(|source| BuildLoadError::Read {
        path: path.to_owned(),
        source,
    })
}

/// The SHA-256 of a file, streamed, so it costs no memory proportional to the file.
fn sha256_of_file(path: &Path, limit: u64) -> Result<Vec<u8>, BuildLoadError> {
    check_regular(path, limit)?;
    let read_error = |source| BuildLoadError::Read {
        path: path.to_owned(),
        source,
    };
    let mut file = std::fs::File::open(path).map_err(read_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    loop {
        let count = file.read(&mut buffer).map_err(read_error)?;
        if count == 0 {
            return Ok(hasher.finalize().to_vec());
        }
        hasher.update(&buffer[..count]);
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0xf), 16).unwrap_or('0'));
    }
    out
}

/// The largest saved stop snap file that will be read, in bytes.
const MAX_SNAP_FILE_BYTES: u64 = 1 << 30;

/// Reads each loaded feed's saved snaps for a mode into one table in the set's numbering.
fn load_snaps(
    directory: &Path,
    mode: TransferMode,
    manifest: &ManifestFile,
    selected: &[usize],
    set: &TimetableSet,
    build_id: BuildId,
) -> Result<StopSnaps, BuildLoadError> {
    let mut snaps = StopSnaps::new(build_id, set.stop_count());
    for (&index, part) in selected.iter().zip(set.parts()) {
        let feed = &manifest.feeds[index];
        let entry =
            feed.stop_snaps
                .get(mode.as_str())
                .ok_or_else(|| BuildLoadError::MissingSnaps {
                    feed: feed.id.clone(),
                    mode: mode.as_str(),
                })?;
        let path = inside_build(directory, &entry.file)?;
        let bytes = read_bounded(&path, MAX_SNAP_FILE_BYTES)?;
        let found = hex(&Sha256::digest(&bytes));
        if found != entry.sha256 {
            return Err(BuildLoadError::Snaps {
                feed: feed.id.clone(),
                reason: format!("its SHA-256 is {found}, the manifest says {}", entry.sha256),
            });
        }
        let count = u32::try_from(part.compiled().stop_count()).unwrap_or(u32::MAX);
        let decoded =
            decode_snap_file(&bytes, build_id, count).map_err(|error| BuildLoadError::Snaps {
                feed: feed.id.clone(),
                reason: error.to_string(),
            })?;
        for (local, snapped) in decoded.into_iter().enumerate() {
            let local = StopId::new(u32::try_from(local).unwrap_or(u32::MAX));
            snaps
                .insert(part.global_stop(local), snapped)
                .map_err(|error| BuildLoadError::Snaps {
                    feed: feed.id.clone(),
                    reason: error.to_string(),
                })?;
        }
    }
    Ok(snaps)
}

/// Where prepared flat tables live, under a build directory.
const PREPARED_DIR: &str = "prepared";

/// How a mode's prepared table is named and keyed; see [`prepared_table`].
struct PreparedName {
    /// Depends on the manifest, feeds, bound, and numbering, not on which tiles are present.
    set_prefix: String,
    /// The whole key: `set_prefix`, and the tiles present.
    key: TableKey,
    /// `<mode>-<set>-<tiles>.bin`.
    file_name: String,
}

fn prepared_name(
    mode: TransferMode,
    manifest_id: u64,
    max_seconds: u32,
    placements: &[FeedPlacement],
    local_stop_count: u32,
    present: &[[u32; 2]],
) -> PreparedName {
    let mut set = Sha256::new();
    set.update(b"transfer-table-set-1\n");
    set.update(mode.as_str().as_bytes());
    set.update(manifest_id.to_le_bytes());
    set.update(max_seconds.to_le_bytes());
    set.update(local_stop_count.to_le_bytes());
    for placement in placements {
        set.update(placement.feed_index.to_le_bytes());
        if let StopPlacement::Contiguous { first, count } = placement.stops {
            set.update(first.to_le_bytes());
            set.update(count.to_le_bytes());
        }
    }
    let set: [u8; 32] = set.finalize().into();
    let mut tiles = Sha256::new();
    for [x, y] in present {
        tiles.update(x.to_le_bytes());
        tiles.update(y.to_le_bytes());
    }
    let tiles: [u8; 32] = tiles.finalize().into();
    let mut key = Sha256::new();
    key.update(set);
    key.update(tiles);
    let key: TableKey = key.finalize().into();
    let set_prefix = format!("{}-{}", mode.as_str(), hex(&set[..8]));
    let file_name = format!("{set_prefix}-{}.bin", hex(&tiles[..8]));
    PreparedName {
        set_prefix,
        key,
        file_name,
    }
}

/// Which of the tiles this mode needs are on disk, as a regular file.
fn present_tiles(
    transfers: &Path,
    mode: TransferMode,
    manifest: &ManifestFile,
    selected: &[usize],
) -> Result<(Vec<[u32; 2]>, usize), BuildLoadError> {
    let needed: BTreeSet<[u32; 2]> = selected
        .iter()
        .filter_map(|&index| manifest.feeds[index].transfer_tiles.get(mode.as_str()))
        .flatten()
        .copied()
        .collect();
    let mut present = Vec::with_capacity(needed.len());
    let mut absent = 0;
    for [x, y] in needed {
        let path = tile_path(transfers, mode, x, y);
        match check_regular(&path, MAX_TILE_BYTES) {
            Ok(()) => present.push([x, y]),
            // An area download holds only some of a feed's tiles; the rest simply have no walks.
            Err(BuildLoadError::Read { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                absent += 1;
            }
            Err(error) => return Err(error),
        }
    }
    Ok((present, absent))
}

fn tile_path(transfers: &Path, mode: TransferMode, x: u32, y: u32) -> PathBuf {
    transfers
        .join(mode.as_str())
        .join(TILE_ZOOM.to_string())
        .join(x.to_string())
        .join(format!("{y}.bin.zst"))
}

/// An existing prepared table for this exact key, or `None` if it has to be built.
///
/// When no tile of the mode is on disk, a client has discarded its tiles after installing them,
/// so a table prepared for this feed set is used whatever tiles it was made from.
fn prepared_table(
    prepared: &Path,
    name: &PreparedName,
    any_tile_present: bool,
) -> Option<TransferTable> {
    if any_tile_present {
        // SAFETY: a prepared table is only ever published by renaming over it, and this module's
        // documentation forbids modifying a build in place while it is open.
        return unsafe { TransferTable::map_flat(&prepared.join(&name.file_name), &name.key) }.ok();
    }
    let entries = std::fs::read_dir(prepared).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let matches = entry.file_name().to_str().is_some_and(|file| {
            file.starts_with(&format!("{}-", name.set_prefix)) && is_table_file(file)
        });
        if !matches {
            continue;
        }
        let key = read_key(&path)?;
        // SAFETY: as above.
        if let Ok(table) = unsafe { TransferTable::map_flat(&path, &key) } {
            return Some(table);
        }
    }
    None
}

fn is_table_file(file: &str) -> bool {
    Path::new(file).extension().is_some_and(|ext| ext == "bin")
}

/// The key stored in a prepared table's header.
fn read_key(path: &Path) -> Option<TableKey> {
    let bytes = read_bounded(path, MAX_TIMETABLE_BYTES).ok()?;
    let start = FLAT_TABLE_MAGIC.len();
    bytes
        .get(start..start + size_of::<TableKey>())?
        .try_into()
        .ok()
}

/// Reads the mode's table from the prepared file, assembling it from tiles first if needed.
#[allow(clippy::too_many_arguments, reason = "one internal call site per mode")]
fn load_mode(
    directory: &Path,
    mode: TransferMode,
    manifest: &ManifestFile,
    selected: &[usize],
    placements: &[FeedPlacement],
    manifest_id: u64,
    max_seconds: u32,
    local_stop_count: u32,
    limits: DecodeLimits,
) -> Result<(TransferTable, usize, usize), BuildLoadError> {
    let transfers = directory.join("transfers");
    let (present, absent) = present_tiles(&transfers, mode, manifest, selected)?;
    let name = prepared_name(
        mode,
        manifest_id,
        max_seconds,
        placements,
        local_stop_count,
        &present,
    );
    let prepared = directory.join(PREPARED_DIR);
    if let Some(table) = prepared_table(&prepared, &name, !present.is_empty()) {
        return Ok((table, present.len(), absent));
    }

    let mut tiles: Vec<TransferTile> = Vec::with_capacity(present.len());
    for &[x, y] in &present {
        let path = tile_path(&transfers, mode, x, y);
        let compressed = read_bounded(&path, MAX_TILE_BYTES)?;
        let tile = decode_tile(&compressed, limits)
            .map_err(|source| BuildLoadError::Tile { path, source })?;
        tiles.push(tile);
    }
    let request = LoadRequest {
        mode,
        manifest_id,
        max_seconds,
        local_stop_count,
        feeds: placements,
    };
    let (table, _) =
        assemble_transfer_table(&request, &tiles).map_err(|source| BuildLoadError::Assemble {
            mode: mode.as_str(),
            source,
        })?;
    drop(tiles);
    let path = prepared.join(&name.file_name);
    publish_flat(&table, &name, &prepared, &path).map_err(|source| BuildLoadError::Prepare {
        path: path.clone(),
        source,
    })?;
    drop(table);
    // SAFETY: just published by renaming, which is the only way it ever changes.
    let table = unsafe { TransferTable::map_flat(&path, &name.key) }
        .map_err(|source| BuildLoadError::Flat { path, source })?;
    Ok((table, present.len(), absent))
}

/// Writes a table to `path` so that readers see the whole file or none of it, and drops others.
///
/// Other prepared tables of this mode are for feed or tile sets no longer in use.
fn publish_flat(
    table: &TransferTable,
    name: &PreparedName,
    prepared: &Path,
    path: &Path,
) -> std::io::Result<()> {
    std::fs::create_dir_all(prepared)?;
    let temp = prepared.join(format!(".{}.{}.tmp", name.file_name, std::process::id()));
    let result = (|| {
        let mut file = std::io::BufWriter::new(std::fs::File::create(&temp)?);
        table.write_flat(&mut file, &name.key)?;
        file.into_inner()
            .map_err(std::io::IntoInnerError::into_error)?
            .sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result?;
    std::fs::File::open(prepared)?.sync_all()?;
    let mode_prefix = name
        .set_prefix
        .split('-')
        .next()
        .map(|mode| format!("{mode}-"))
        .unwrap_or_default();
    for entry in std::fs::read_dir(prepared)?.filter_map(Result::ok) {
        let stale = entry.file_name().to_str().is_some_and(|file| {
            file.starts_with(&mode_prefix) && is_table_file(file) && file != name.file_name
        });
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
}
