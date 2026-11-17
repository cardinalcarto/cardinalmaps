//! Reading a region's tiles for the offline index, from Dart.
//!
//! The app reads the region's z14 tiles from the basemap with sparkle's
//! `basemapReadRegionTiles` (the basemap is read through the SQLite that writes it, which
//! isn't in this library), page by page, and hands each page to
//! [`RegionExtraction::add_tiles`]; then it calls [`RegionExtraction::finish`].
//!
//! For now the features are only counted: the index's documents are built from them once
//! airmail's document builder is plugged in (see [`crate::region::FeatureSink`]).

use flutter_rust_bridge::frb;

use crate::extract::{SourceLayer, TileKey};
use crate::region::{CountingSink, RegionCounts, RegionExtractor};

/// One of a region's tiles, as the basemap stores it (gzipped), `y` from the top.
pub struct GeocoderTile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
    pub data: Vec<u8>,
}

/// What one layer of the tiles held.
pub struct LayerStats {
    /// The layer's name in the tiles.
    pub layer: String,
    /// Features, the buffer around each tile included.
    pub features: i64,
    /// Points kept (a multipoint's each).
    pub points: i64,
    /// Points dropped as they're in a tile's buffer (another tile has them).
    pub buffer_points: i64,
    /// Pieces of lines and areas, joined across tiles.
    pub pieces: i64,
}

/// What an extraction has read and made so far.
pub struct ExtractionStats {
    pub tiles: i64,
    /// The tiles' bytes as given (gzipped).
    pub bytes: i64,
    pub layers: Vec<LayerStats>,
    /// Pieces with no name or address, dropped.
    pub unnamed_pieces: i64,
    /// Features joined from pieces (at the end).
    pub joined: i64,
    /// Features made, points and joined ones.
    pub features: i64,
    /// Of them, those with an ID (in `poi`, a stable `poi_id`).
    pub with_id: i64,
    pub with_names: i64,
    pub with_address: i64,
    pub with_admins: i64,
    pub with_importance: i64,
}

/// What kind of problem it is.
pub enum ExtractErrorKind {
    /// A tile isn't gzip or MLT. Nothing of the page from that tile on was added.
    BadTile,
    /// The extraction was finished already.
    Finished,
}

/// Why an extraction step failed.
pub struct ExtractError {
    pub kind: ExtractErrorKind,
    /// A description for logs.
    pub message: String,
}

/// A region's extraction, tile page by tile page. Opaque to Dart.
#[frb(opaque)]
pub struct RegionExtraction {
    inner: Option<RegionExtractor>,
    counter: CountingSink,
}

fn signed(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

impl RegionExtraction {
    #[frb(sync)]
    pub fn new() -> RegionExtraction {
        RegionExtraction {
            inner: Some(RegionExtractor::new()),
            counter: CountingSink::default(),
        }
    }

    /// Reads a page of the region's tiles; returns what's been read so far.
    pub fn add_tiles(&mut self, tiles: Vec<GeocoderTile>) -> Result<ExtractionStats, ExtractError> {
        let Some(inner) = self.inner.as_mut() else {
            return Err(finished());
        };
        for t in tiles {
            let key = TileKey {
                z: t.z,
                x: t.x,
                y: t.y,
            };
            inner
                .add_tile(key, &t.data, &mut self.counter)
                .map_err(|e| ExtractError {
                    kind: ExtractErrorKind::BadTile,
                    message: e.to_string(),
                })?;
        }
        let counts = inner.counts();
        Ok(self.stats(&counts))
    }

    /// Joins lines and areas across the tiles read; returns what the extraction made. It can
    /// be called once.
    pub fn finish(&mut self) -> Result<ExtractionStats, ExtractError> {
        let inner = self.inner.take().ok_or_else(finished)?;
        let counts = inner.finish(&mut self.counter);
        Ok(self.stats(&counts))
    }

    fn stats(&self, counts: &RegionCounts) -> ExtractionStats {
        let c = &self.counter;
        let l = &counts.layers;
        ExtractionStats {
            tiles: signed(counts.tiles),
            bytes: signed(counts.bytes),
            layers: SourceLayer::ALL
                .iter()
                .enumerate()
                .map(|(i, layer)| LayerStats {
                    layer: layer.name().to_owned(),
                    features: signed(l.features[i]),
                    points: signed(l.points[i]),
                    buffer_points: signed(l.buffer_points[i]),
                    pieces: signed(l.pieces[i]),
                })
                .collect(),
            unnamed_pieces: signed(counts.merge.unnamed),
            joined: signed(if self.inner.is_none() {
                counts.merge.features
            } else {
                0
            }),
            features: signed(c.features),
            with_id: signed(c.with_id),
            with_names: signed(c.with_names),
            with_address: signed(c.with_address),
            with_admins: signed(c.with_admins),
            with_importance: signed(c.with_importance),
        }
    }
}

fn finished() -> ExtractError {
    ExtractError {
        kind: ExtractErrorKind::Finished,
        message: "the extraction was finished already".to_owned(),
    }
}
