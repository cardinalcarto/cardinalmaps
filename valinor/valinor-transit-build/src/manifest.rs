//! The build manifest: what a client needs to pick and use feeds and transfer tiles.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

use crate::compile::FeedOutcome;
use crate::error::BuildError;
use crate::write_atomic;

/// Where everything a transfer stage publishes lives, relative to its output directory.
pub const TRANSFERS_DIR: &str = "transfers";

/// The manifest's file name in [`TRANSFERS_DIR`].
pub const MANIFEST_FILE: &str = "manifest.json";

/// How many of the SHA-256's bytes make up the manifest ID.
const MANIFEST_ID_BYTES: usize = 8;

/// Starts what the manifest ID hashes, so an ID never equals one made by an earlier rule,
/// which covered only the feeds and the street build.
const MANIFEST_ID_DOMAIN: &[u8] = b"valinor-transit-manifest-id-2\n";

/// The ID of a build: the fingerprint every tile carries, and what clients key prepared tables on.
///
/// Covers everything a client reads to plan, so two builds share an ID only when a client
/// would plan the same with either: the street network's build, the tile format, zoom,
/// and bounds; each feed's ID, archive, timetable, and stop count, its stop snaps' files and
/// SHA-256s, and the tiles listed for it; and each mode's tiles with their
/// [content digests](valinor_walk::transfer_tile::TransferTile::content_digest).
/// A field a client starts reading must be added here.
///
/// It leaves out [`Manifest::manifest_id`] itself and each tile's compressed size, which
/// depend on the ID the tiles carry, and what is only descriptive: names, licenses, sources,
/// fetch details, outcomes, service dates, statistics, and totals.
/// That a tile's digest leaves out its manifest ID is what lets the ID cover the tiles.
///
/// # Performance
///
/// One SHA-256 over a few hundred bytes per feed and per tile.
#[must_use]
pub fn manifest_id(manifest: &Manifest) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(MANIFEST_ID_DOMAIN);
    hasher.update(manifest.valhalla_build_id.to_le_bytes());
    hasher.update(manifest.tile_format_version.to_le_bytes());
    hasher.update([manifest.zoom]);
    hasher.update(manifest.walking_bound_seconds.to_le_bytes());
    hasher.update(manifest.cycling_bound_seconds.to_le_bytes());
    hash_option(&mut hasher, manifest.wheelchair_bound_seconds);
    hasher.update((manifest.feeds.len() as u64).to_le_bytes());
    for feed in &manifest.feeds {
        hash_str(&mut hasher, &feed.id);
        hash_str(&mut hasher, &feed.sha256);
        hash_option_str(&mut hasher, feed.timetable.as_deref());
        hash_option_str(&mut hasher, feed.timetable_sha256.as_deref());
        hasher.update(feed.stop_count.to_le_bytes());
        hasher.update((feed.stop_snaps.len() as u64).to_le_bytes());
        for (mode, snaps) in &feed.stop_snaps {
            hash_str(&mut hasher, mode);
            hash_str(&mut hasher, &snaps.file);
            hash_str(&mut hasher, &snaps.sha256);
        }
        hasher.update((feed.transfer_tiles.len() as u64).to_le_bytes());
        for (mode, tiles) in &feed.transfer_tiles {
            hash_str(&mut hasher, mode);
            hasher.update((tiles.len() as u64).to_le_bytes());
            for [x, y] in tiles {
                hasher.update(x.to_le_bytes());
                hasher.update(y.to_le_bytes());
            }
        }
    }
    hasher.update((manifest.tiles.len() as u64).to_le_bytes());
    for (mode, tiles) in &manifest.tiles {
        hash_str(&mut hasher, mode);
        hasher.update((tiles.len() as u64).to_le_bytes());
        for tile in tiles {
            hasher.update(tile.x.to_le_bytes());
            hasher.update(tile.y.to_le_bytes());
            hash_str(&mut hasher, &tile.content_sha256);
        }
    }
    let digest = hasher.finalize();
    let mut bytes = [0; MANIFEST_ID_BYTES];
    bytes.copy_from_slice(&digest[..MANIFEST_ID_BYTES]);
    u64::from_le_bytes(bytes)
}

/// A string, prefixed by its length, so no two lists of strings hash alike.
fn hash_str(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hash_option_str(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hash_str(hasher, value);
        }
        None => hasher.update([0]),
    }
}

fn hash_option(hasher: &mut Sha256, value: Option<u32>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.to_le_bytes());
        }
        None => hasher.update([0]),
    }
}

/// A file with counts, for one tile of one mode.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The tile's [content digest](valinor_walk::transfer_tile::TransferTile::content_digest),
    /// in hexadecimal: what the manifest ID covers of it.
    ///
    /// Empty in manifests written before the manifest ID covered tiles.
    #[serde(default)]
    pub content_sha256: String,
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
    /// The build's ID, which tiles carry, in hexadecimal; see [`manifest_id`].
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

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(json: &str) -> Box<RawValue> {
        RawValue::from_string(json.to_owned()).expect("valid JSON")
    }

    fn feed(id: &str) -> ManifestFeed {
        let snaps = |mode: &str| SnapFileEntry {
            file: format!("transfers/snaps/{mode}/{id}.bin"),
            sha256: format!("{id}-{mode}-snaps"),
        };
        ManifestFeed {
            id: id.to_owned(),
            name: format!("Feed {id}"),
            region: "us-wa".to_owned(),
            sha256: format!("{id}-archive"),
            license: raw(r#"{"spdx":"CC0-1.0"}"#),
            source: raw(r#"{"kind":"test"}"#),
            outcome: FeedOutcome::Compiled,
            timetable: Some(format!("timetables/{id}.timetable")),
            timetable_bytes: 100,
            timetable_sha256: Some(format!("{id}-timetable")),
            stop_count: 10,
            has_current_service: true,
            last_service_date: Some("2026-12-31".to_owned()),
            stats: BTreeMap::new(),
            transfer_tiles: [("walking", vec![[1, 2]]), ("cycling", vec![[1, 2], [1, 3]])]
                .into_iter()
                .map(|(mode, tiles)| (mode.to_owned(), tiles))
                .collect(),
            stop_snaps: ["walking", "cycling"]
                .into_iter()
                .map(|mode| (mode.to_owned(), snaps(mode)))
                .collect(),
        }
    }

    fn tile(x: u32, y: u32, content: &str) -> TileEntry {
        TileEntry {
            x,
            y,
            origins: 3,
            walks: 9,
            cross_feed_walks: 1,
            content_sha256: content.to_owned(),
            bytes: 50,
            raw_bytes: 90,
        }
    }

    fn sample() -> Manifest {
        Manifest {
            transitous_commit: "abc".to_owned(),
            fetched_at: "2026-11-09T00:00:00Z".to_owned(),
            manifest_id: "0123456789abcdef".to_owned(),
            valhalla_build_id: 7,
            tile_format_version: 1,
            zoom: 12,
            walking_bound_seconds: 900,
            cycling_bound_seconds: 900,
            wheelchair_bound_seconds: None,
            feeds: vec![feed("a"), feed("b")],
            tiles: [
                ("walking", vec![tile(1, 2, "w12")]),
                ("cycling", vec![tile(1, 2, "c12"), tile(1, 3, "c13")]),
            ]
            .into_iter()
            .map(|(mode, tiles)| (mode.to_owned(), tiles))
            .collect(),
            totals: BTreeMap::new(),
        }
    }

    /// Whether `edit` changes the ID of [`sample`].
    fn changes_id(edit: impl FnOnce(&mut Manifest)) -> bool {
        let mut manifest = sample();
        edit(&mut manifest);
        manifest_id(&manifest) != manifest_id(&sample())
    }

    fn tiles<'a>(manifest: &'a mut Manifest, mode: &str) -> &'a mut Vec<TileEntry> {
        manifest.tiles.get_mut(mode).expect("mode")
    }

    #[test]
    fn what_a_client_reads_changes_the_id() {
        assert!(changes_id(
            |m| tiles(m, "cycling")[1].content_sha256 = "other".to_owned()
        ));
        assert!(changes_id(|m| {
            tiles(m, "cycling").pop();
        }));
        assert!(changes_id(|m| tiles(m, "walking")[0].y = 3));
        assert!(changes_id(|m| {
            m.feeds[1]
                .stop_snaps
                .get_mut("walking")
                .expect("snaps")
                .sha256 = "other".to_owned();
        }));
        assert!(changes_id(|m| {
            m.feeds[0].stop_snaps.remove("cycling");
        }));
        assert!(changes_id(|m| {
            m.feeds[0]
                .transfer_tiles
                .get_mut("cycling")
                .expect("tiles")
                .pop();
        }));
        assert!(changes_id(|m| m.feeds[0].timetable_sha256 = None));
        assert!(changes_id(|m| m.feeds[0].stop_count += 1));
        assert!(changes_id(|m| m.feeds.swap(0, 1)));
        assert!(changes_id(|m| m.valhalla_build_id += 1));
        assert!(changes_id(|m| m.tile_format_version += 1));
        assert!(changes_id(|m| m.walking_bound_seconds += 1));
        assert!(changes_id(|m| m.cycling_bound_seconds += 1));
        assert!(changes_id(|m| m.wheelchair_bound_seconds = Some(900)));
    }

    #[test]
    fn what_only_describes_a_build_keeps_the_id() {
        assert!(!changes_id(
            |m| m.manifest_id = "fedcba9876543210".to_owned()
        ));
        assert!(!changes_id(|m| m.fetched_at = "later".to_owned()));
        assert!(!changes_id(|m| m.transitous_commit = "def".to_owned()));
        assert!(!changes_id(|m| m.feeds[0].name = "Renamed".to_owned()));
        assert!(!changes_id(|m| m.feeds[0].license = raw("null")));
        assert!(!changes_id(|m| m.feeds[0].has_current_service = false));
        assert!(!changes_id(|m| {
            m.feeds[0]
                .stats
                .insert("walking".to_owned(), FeedModeStats::default());
        }));
        // A tile's compressed size depends on the ID it carries, so it can't be covered.
        assert!(!changes_id(|m| tiles(m, "walking")[0].bytes += 1));
        assert!(!changes_id(|m| {
            m.totals.insert("walking".to_owned(), ModeTotals::default());
        }));
    }

    #[test]
    fn strings_are_delimited() {
        // The same bytes, split differently between two fields.
        assert_eq!(sample().feeds[0].sha256, "a-archive");
        assert!(changes_id(|m| {
            m.feeds[0].id = "aa-".to_owned();
            m.feeds[0].sha256 = "archive".to_owned();
        }));
    }
}
