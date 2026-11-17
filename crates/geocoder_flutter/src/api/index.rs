//! Building and searching the device's index, from Dart.
//!
//! A build ([`IndexBuilder`]) reads the Who's On First responses stored with the regions,
//! then the regions' z14 tiles page by page (each tile once, however many regions have it),
//! and finishes into its directory. Searching ([`OfflineIndex`]) gives results as the
//! server's JSON; a pointer result's feature is read from its tile with [`poi_feature`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use flutter_rust_bridge::frb;

use crate::api::extract::{ExtractError, ExtractErrorKind, GeocoderTile};
use crate::build::{BuildCounts, BuildError, BuildOptions, IndexBuild};
use crate::extract::{TileKey, poi_properties};
use crate::search::{DeviceIndex, Query};
use crate::wof::WofPlaces;

/// Why an index step failed.
#[derive(Debug)]
pub struct IndexError {
    pub kind: IndexErrorKind,
    /// A description for logs.
    pub message: String,
}

#[derive(Debug)]
pub enum IndexErrorKind {
    /// A WOF response isn't one, or can't be read.
    BadWof,
    /// A tile isn't gzip or MLT.
    BadTile,
    /// The index can't be written, read or searched.
    Storage,
    /// The build was finished already.
    Finished,
}

fn error(kind: IndexErrorKind, message: impl ToString) -> IndexError {
    IndexError {
        kind,
        message: message.to_string(),
    }
}

impl From<BuildError> for IndexError {
    fn from(e: BuildError) -> Self {
        let kind = match e {
            BuildError::Tile(_) => IndexErrorKind::BadTile,
            BuildError::Index(_) => IndexErrorKind::Storage,
        };
        error(kind, e)
    }
}

/// What a build has made so far.
pub struct IndexBuildStats {
    pub tiles: i64,
    /// Tiles skipped because another region had them.
    pub repeated_tiles: i64,
    pub bytes: i64,
    pub features: i64,
    pub unindexed: i64,
    pub duplicate_addresses: i64,
    pub pois: i64,
    pub addresses: i64,
    pub admins: i64,
    pub streets: i64,
    pub postcodes: i64,
    /// Documents that are pointers to their tile features.
    pub pointers: i64,
    /// WOF places known, and those indexed.
    pub wof_places: i64,
    pub wof_indexed: i64,
}

fn signed(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn stats(c: &BuildCounts, wof: (usize, usize)) -> IndexBuildStats {
    IndexBuildStats {
        tiles: signed(c.tiles),
        repeated_tiles: signed(c.repeated_tiles),
        bytes: signed(c.bytes),
        features: signed(c.features),
        unindexed: signed(c.unindexed),
        duplicate_addresses: signed(c.duplicate_addresses),
        pois: signed(c.documents[0]),
        addresses: signed(c.documents[1]),
        admins: signed(c.documents[2]),
        streets: signed(c.documents[3]),
        postcodes: signed(c.documents[4]),
        pointers: signed(c.pointers),
        wof_places: signed(wof.0 as u64),
        wof_indexed: signed(wof.1 as u64),
    }
}

/// Reads stored WOF service responses (JSON, uncompressed) into places.
fn read_wof(paths: &[String]) -> Result<WofPlaces, IndexError> {
    let mut wof = WofPlaces::new();
    for path in paths {
        let bytes = std::fs::read(path)
            .map_err(|e| error(IndexErrorKind::BadWof, format!("{path}: {e}")))?;
        wof.add_response(&bytes)
            .map_err(|e| error(IndexErrorKind::BadWof, format!("{path}: {e}")))?;
    }
    Ok(wof)
}

/// A build of the index. Opaque to Dart.
#[frb(opaque)]
pub struct IndexBuilder {
    inner: Option<IndexBuild>,
    wof: (usize, usize),
}

impl IndexBuilder {
    /// Starts an index in `dir` (empty or not there yet) over the areas of the WOF
    /// responses at `wof_files`, with `heap_mb` MB of indexing memory over `threads`
    /// threads (at least 15 MB each).
    pub fn create(
        dir: String,
        wof_files: Vec<String>,
        heap_mb: u32,
        threads: u32,
    ) -> Result<IndexBuilder, IndexError> {
        let wof = read_wof(&wof_files)?;
        let counts = wof.len();
        let options = BuildOptions {
            heap_bytes: heap_mb as usize * 1_000_000,
            threads: threads.max(1) as usize,
            builders: crate::build::default_builders(threads.max(1) as usize),
            merge: true,
        };
        let inner = IndexBuild::new(Path::new(&dir), wof, options)?;
        Ok(IndexBuilder {
            inner: Some(inner),
            wof: counts,
        })
    }

    /// Reads a page of tiles; returns what's been made so far.
    pub fn add_tiles(&mut self, tiles: Vec<GeocoderTile>) -> Result<IndexBuildStats, IndexError> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| error(IndexErrorKind::Finished, "the build was finished already"))?;
        for t in tiles {
            let key = TileKey {
                z: t.z,
                x: t.x,
                y: t.y,
            };
            inner.add_tile(key, &t.data)?;
        }
        Ok(stats(inner.counts(), self.wof))
    }

    /// Adds what's joined across tiles, the postcodes and the areas, and writes the index.
    /// It can be called once.
    pub fn finish(&mut self) -> Result<IndexBuildStats, IndexError> {
        let inner = self
            .inner
            .take()
            .ok_or_else(|| error(IndexErrorKind::Finished, "the build was finished already"))?;
        let counts = inner.finish()?;
        Ok(stats(&counts, self.wof))
    }
}

/// A search, as the server's `/v1/search` takes it.
pub struct OfflineQuery {
    pub text: String,
    /// A BCP 47 tag, for names in that language.
    pub lang: Option<String>,
    pub prefix: bool,
    pub typos: bool,
    /// `[lon, lat]`.
    pub focus: Option<Vec<f64>>,
    /// `[west, south, east, north]`.
    pub viewport: Option<Vec<f64>>,
    pub limit: u32,
}

/// The device's index, open for searching. Opaque to Dart.
#[frb(opaque)]
pub struct OfflineIndex {
    index: DeviceIndex,
    dir: PathBuf,
}

impl OfflineIndex {
    /// Opens the index in `dir`, with the query tagger at `tagger` and the categorical
    /// model at `categories` if they're given.
    pub fn open(
        dir: String,
        tagger: Option<String>,
        categories: Option<String>,
    ) -> Result<OfflineIndex, IndexError> {
        let dir = PathBuf::from(dir);
        let index = DeviceIndex::open(
            &dir,
            tagger.as_deref().map(Path::new),
            categories.as_deref().map(Path::new),
        )
        .map_err(|e| error(IndexErrorKind::Storage, format!("{e:#}")))?;
        Ok(OfflineIndex { index, dir })
    }

    /// The directory it's in.
    #[frb(sync)]
    pub fn dir(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }

    /// How many documents it has.
    pub fn documents(&self) -> Result<i64, IndexError> {
        self.index
            .documents()
            .map(signed)
            .map_err(|e| error(IndexErrorKind::Storage, format!("{e:#}")))
    }

    /// Results as the server's `{"results": [...]}` JSON, with `pointer` on pointers.
    pub fn search(&self, query: OfflineQuery) -> Result<String, IndexError> {
        let pair = |v: Option<Vec<f64>>| v.and_then(|v| <[f64; 2]>::try_from(v).ok());
        let quad = |v: Option<Vec<f64>>| v.and_then(|v| <[f64; 4]>::try_from(v).ok());
        let query = Query {
            text: query.text,
            lang: query.lang,
            prefix: query.prefix,
            typos: query.typos,
            focus: pair(query.focus),
            viewport: quad(query.viewport),
            limit: query.limit as usize,
        };
        let results = self
            .index
            .search(&query)
            .map_err(|e| error(IndexErrorKind::Storage, format!("{e:#}")))?;
        Ok(serde_json::json!({ "results": results }).to_string())
    }

    /// The place with the result ID `id`, as the server's `result` JSON, or `None`.
    pub fn place(&self, id: String, lang: Option<String>) -> Result<Option<String>, IndexError> {
        self.index
            .place(&id, lang.as_deref())
            .map(|v| v.map(|v| v.to_string()))
            .map_err(|e| error(IndexErrorKind::Storage, format!("{e:#}")))
    }
}

/// The feature of a pointer result: its properties as text, from the `poi` or `geocoder`
/// layer of its tile, or `None` if the tile has no feature with that ID.
pub fn poi_feature(tile: GeocoderTile, poi_id: u64) -> Result<Option<PoiFeature>, ExtractError> {
    let key = TileKey {
        z: tile.z,
        x: tile.x,
        y: tile.y,
    };
    let found = poi_properties(key, &tile.data, poi_id).map_err(|e| ExtractError {
        kind: ExtractErrorKind::BadTile,
        message: e.to_string(),
    })?;
    Ok(found.map(|(layer, properties)| PoiFeature {
        layer: layer.name().to_owned(),
        properties: properties.into_iter().collect(),
    }))
}

/// A box of degrees.
pub struct WofBox {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

/// The boxes to fetch a region's Who's On First places in: its bounding box split into
/// boxes at most `side` degrees each way (see `wof::boxes`).
#[frb(sync)]
pub fn wof_boxes(west: f64, south: f64, east: f64, north: f64, side: f64) -> Vec<WofBox> {
    crate::wof::boxes([west, south, east, north], side)
        .into_iter()
        .map(|[west, south, east, north]| WofBox {
            west,
            south,
            east,
            north,
        })
        .collect()
}

/// A tile feature's layer and properties.
pub struct PoiFeature {
    pub layer: String,
    pub properties: HashMap<String, String>,
}
