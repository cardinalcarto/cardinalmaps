//! The build manifest: what a client needs to pick and use feeds and transfer tiles.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

use crate::compile::{FeedOutcome, FeedRecord};
use crate::error::BuildError;
use crate::write_atomic;

/// Where everything a transfer stage publishes lives, relative to its output directory.
pub const TRANSFERS_DIR: &str = "transfers";

/// The manifest's file name in [`TRANSFERS_DIR`].
pub const MANIFEST_FILE: &str = "manifest.json";

/// How many of the SHA-256's bytes make up the manifest ID.
const MANIFEST_ID_BYTES: usize = 8;

/// The fingerprint a tile carries, to tell whether it matches a manifest's numbering.
///
/// Covers everything that decides what a stop address means: each feed's ID, its archive,
/// and the bytes of its compiled timetable (the stop numbering is the timetable's),
/// and the street network's build (a walk is only meaningful on one graph).
#[must_use]
pub fn manifest_id(feeds: &[FeedRecord], build_id: u16) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(build_id.to_le_bytes());
    for feed in feeds {
        hasher.update(feed.id.as_bytes());
        hasher.update(b"\t");
        hasher.update(feed.sha256.as_bytes());
        hasher.update(b"\t");
        hasher.update(feed.timetable_sha256.as_deref().unwrap_or("-").as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    let mut bytes = [0; MANIFEST_ID_BYTES];
    bytes.copy_from_slice(&digest[..MANIFEST_ID_BYTES]);
    u64::from_le_bytes(bytes)
}

/// A file with counts, for one tile of one mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileEntry {
    /// The tile's column at zoom 12.
    pub x: u32,
    /// The tile's row at zoom 12.
    pub y: u32,
    /// Stops in the tile with at least one walk.
    pub origins: u32,
    /// Walks in the tile.
    pub walks: u64,
    /// Walks in the tile to a stop in another feed.
    pub cross_feed_walks: u64,
    /// Size of the compressed file.
    pub bytes: u64,
    /// Size when decompressed.
    pub raw_bytes: u64,
}

/// Totals for one mode over the whole build.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeTotals {
    /// Stops in every compiled feed, whether snapped or not.
    pub stops: u64,
    /// Stops that were snapped to a street.
    pub snapped: u64,
    /// Stops with no usable street nearby; they have no walks.
    pub unsnapped: u64,
    /// Snapped stops on a disconnected piece of the network.
    pub disconnected: u64,
    /// Stops with at least one walk.
    pub origins: u64,
    /// Walks in the build.
    pub walks: u64,
    /// Walks to a stop in another feed.
    pub cross_feed_walks: u64,
    /// Tile files written.
    pub tiles: u64,
    /// Total compressed bytes.
    pub bytes: u64,
    /// Total decompressed bytes.
    pub raw_bytes: u64,
    /// `transfers.txt` rules between two stops with no walk on the street network between them.
    #[serde(default)]
    pub pair_rules_without_walk: u64,
}

/// The totals of one feed for one mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedModeStats {
    /// Stops snapped.
    pub snapped: u32,
    /// Stops with no usable street nearby.
    pub unsnapped: u32,
    /// Snapped stops on a disconnected piece of the network.
    pub disconnected: u32,
    /// Walks starting at this feed's stops.
    pub walks: u64,
    /// Those walks going to another feed's stops.
    pub cross_feed_walks: u64,
}

/// One feed's saved stop snaps for one mode.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapFileEntry {
    /// The file, relative to the build directory.
    pub file: String,
    /// The SHA-256 of the file.
    pub sha256: String,
}

/// One feed in the manifest. Its index is its position in [`Manifest::feeds`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestFeed {
    /// The feed's ID.
    pub id: String,
    /// The feed's name.
    pub name: String,
    /// The feed's Transitous region.
    pub region: String,
    /// The SHA-256 of the input archive.
    pub sha256: String,
    /// The feed's license, verbatim.
    pub license: Box<RawValue>,
    /// Where the feed came from, verbatim.
    pub source: Box<RawValue>,
    /// The compile outcome.
    pub outcome: FeedOutcome,
    /// The timetable, relative to the compile stage's output, if compiled.
    pub timetable: Option<String>,
    /// The size of the timetable.
    pub timetable_bytes: u64,
    /// The SHA-256 of the timetable.
    pub timetable_sha256: Option<String>,
    /// Stops in the compiled timetable.
    pub stop_count: u32,
    /// Whether the feed runs on or after the date it was fetched.
    pub has_current_service: bool,
    /// The last date the feed's calendars run on.
    pub last_service_date: Option<String>,
    /// How this feed's stops fared, by mode.
    pub stats: BTreeMap<String, FeedModeStats>,
    /// The tiles holding walks from this feed's stops, by mode, as `[x, y]`.
    ///
    /// A client downloads these for each feed it wants.
    pub transfer_tiles: BTreeMap<String, Vec<[u32; 2]>>,
    /// The feed's stops snapped to the street network, by mode.
    ///
    /// A client loads these rather than snapping at open.
    #[serde(default)]
    pub stop_snaps: BTreeMap<String, SnapFileEntry>,
}

/// The build manifest. Everything a client needs is here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    /// The Transitous commit the feeds came from.
    pub transitous_commit: String,
    /// When the feeds were fetched.
    pub fetched_at: String,
    /// The fingerprint tiles carry, in hexadecimal.
    pub manifest_id: String,
    /// The build ID of the street network, as a number.
    pub valhalla_build_id: u32,
    /// The transfer tile format's version.
    pub tile_format_version: u16,
    /// The zoom of the transfer tiles.
    pub zoom: u8,
    /// The walking bound, in seconds.
    pub walking_bound_seconds: u32,
    /// The cycling bound, in seconds.
    pub cycling_bound_seconds: u32,
    /// The wheelchair bound, in seconds, if the build has wheelchair transfers.
    pub wheelchair_bound_seconds: Option<u32>,
    /// The feeds, in index order.
    pub feeds: Vec<ManifestFeed>,
    /// Every tile written, by mode, sorted by `(x, y)`.
    pub tiles: BTreeMap<String, Vec<TileEntry>>,
    /// Totals by mode.
    pub totals: BTreeMap<String, ModeTotals>,
}

/// The manifest's path under a published transfer stage's output directory.
pub fn manifest_path(output: &Path) -> PathBuf {
    output.join(TRANSFERS_DIR).join(MANIFEST_FILE)
}

/// Writes `manifest` into a staging directory, to be published with the tiles.
///
/// # Errors
///
/// If it can't be written.
pub fn write_manifest(staging: &Path, manifest: &Manifest) -> Result<(), BuildError> {
    let path = staging.join(MANIFEST_FILE);
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|source| BuildError::Json {
        path: path.clone(),
        source,
    })?;
    write_atomic(&path, &bytes)
}

/// Reads the published manifest under a transfer stage's output directory.
///
/// # Errors
///
/// If it can't be read or parsed.
pub fn read_manifest(output: &Path) -> Result<Manifest, BuildError> {
    let path = manifest_path(output);
    let bytes = std::fs::read(&path).map_err(|source| BuildError::Read {
        path: path.clone(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| BuildError::Json { path, source })
}
