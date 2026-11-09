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
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use valinor_gtfs::{LoadError, LoadedTimetable, SetError, TimetableSet};
use valinor_walk::TransferTable;
use valinor_walk::transfer_tile::{
    DEFAULT_MAX_DECOMPRESSED_BYTES, DecodeLimits, FeedPlacement, LoadReport, LoadRequest,
    StopPlacement, TILE_FORMAT_VERSION, TILE_ZOOM, TileDecodeError, TileLoadError, TransferMode,
    TransferTile, assemble_transfer_table, decode_tile,
};

/// The default bound on one tile's decompressed size.
pub fn default_decode_limits() -> DecodeLimits {
    DecodeLimits {
        max_decompressed_bytes: DEFAULT_MAX_DECOMPRESSED_BYTES,
    }
}

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
}

/// What loading a build found, for logging and checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildLoadReport {
    /// Tiles decoded, per mode.
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
    /// The manifest's street build, checked when opening a planner.
    pub valhalla_build_id: u32,
    /// What was found on disk.
    pub report: BuildLoadReport,
}

impl TransitBuild {
    /// Loads a build directory's timetables and whichever of the needed tiles are on disk.
    ///
    /// `feeds` chooses feeds by ID; `None` loads every feed that compiled.
    /// Files are memory-mapped; to replace one, rename over it; don't modify it in place.
    ///
    /// # Performance
    ///
    /// Hashes each timetable, validates and maps it, and decodes the tiles found.
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
        let bytes = std::fs::read(&manifest_path).map_err(|source| BuildLoadError::Read {
            path: manifest_path.clone(),
            source,
        })?;
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
        let (walking, walking_report, walking_absent) = load_mode(
            &transfers,
            TransferMode::Walking,
            &manifest,
            &selected,
            &placements,
            manifest_id,
            manifest.walking_bound_seconds,
            local_stop_count,
            limits,
        )?;
        let (cycling, cycling_report, cycling_absent) = load_mode(
            &transfers,
            TransferMode::Cycling,
            &manifest,
            &selected,
            &placements,
            manifest_id,
            manifest.cycling_bound_seconds,
            local_stop_count,
            limits,
        )?;
        report.tiles_read = [walking_report.tiles, cycling_report.tiles];
        report.tiles_absent = [walking_absent, cycling_absent];
        report.walks = [walking_report.walks, cycling_report.walks];
        Ok(Self {
            set,
            walking,
            cycling,
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
    let path = directory.join(relative);
    let bytes = std::fs::read(&path).map_err(|source| BuildLoadError::Read {
        path: path.clone(),
        source,
    })?;
    let found = hex(&Sha256::digest(&bytes));
    let expected = feed.timetable_sha256.as_deref().unwrap_or_default();
    if found != expected {
        return Err(BuildLoadError::TimetableMismatch {
            feed: feed.id.clone(),
            expected: expected.to_owned(),
            found,
        });
    }
    drop(bytes);
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

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0xf), 16).unwrap_or('0'));
    }
    out
}

/// Reads whichever of one mode's needed tiles are on disk, and assembles them.
#[allow(clippy::too_many_arguments, reason = "one internal call site per mode")]
fn load_mode(
    transfers: &Path,
    mode: TransferMode,
    manifest: &ManifestFile,
    selected: &[usize],
    placements: &[FeedPlacement],
    manifest_id: u64,
    max_seconds: u32,
    local_stop_count: u32,
    limits: DecodeLimits,
) -> Result<(TransferTable, LoadReport, usize), BuildLoadError> {
    let needed: BTreeSet<[u32; 2]> = selected
        .iter()
        .filter_map(|&index| manifest.feeds[index].transfer_tiles.get(mode.as_str()))
        .flatten()
        .copied()
        .collect();
    let mut tiles: Vec<TransferTile> = Vec::with_capacity(needed.len());
    let mut absent = 0;
    for [x, y] in needed {
        let path = transfers
            .join(mode.as_str())
            .join(TILE_ZOOM.to_string())
            .join(x.to_string())
            .join(format!("{y}.bin.zst"));
        let compressed = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            // An area download holds only some of a feed's tiles; the rest simply have no walks.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                absent += 1;
                continue;
            }
            Err(source) => return Err(BuildLoadError::Read { path, source }),
        };
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
    let (table, report) =
        assemble_transfer_table(&request, &tiles).map_err(|source| BuildLoadError::Assemble {
            mode: mode.as_str(),
            source,
        })?;
    Ok((table, report, absent))
}
