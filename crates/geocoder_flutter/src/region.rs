//! Reading a whole region: its z14 tiles one after another, then the features joined across
//! them.

use crate::extract::{ExtractedFeature, SourceLayer, TileCounts, TileError, TileKey, extract_tile};
use crate::merge::{MergeCounts, Merger};

/// Where extracted features go: the index's documents ([`crate::build::IndexSink`]), or a
/// count of them.
pub trait FeatureSink {
    fn feature(&mut self, feature: ExtractedFeature);

    /// A tile's points, all at once (each tile's in turn), for rules between the features
    /// of one tile. Gives each to [`FeatureSink::feature`] unless overridden.
    fn tile_points(&mut self, _tile: TileKey, points: Vec<ExtractedFeature>) {
        for point in points {
            self.feature(point);
        }
    }

    /// No feature still to come is in a tile of a column before `column` (z14 x), or has
    /// its center there: what the sink keeps by tile for those columns can go.
    fn columns_done(&mut self, _column: u32) {}
}

impl FeatureSink for Vec<ExtractedFeature> {
    fn feature(&mut self, feature: ExtractedFeature) {
        self.push(feature);
    }
}

/// Counts the features that would be indexed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CountingSink {
    pub features: u64,
    pub with_id: u64,
    pub with_names: u64,
    pub with_address: u64,
    pub with_admins: u64,
    pub with_importance: u64,
}

impl FeatureSink for CountingSink {
    fn feature(&mut self, f: ExtractedFeature) {
        self.features += 1;
        self.with_id += u64::from(f.id.is_some());
        self.with_names += u64::from(!f.names.is_empty());
        self.with_address += u64::from(!f.address.is_empty());
        self.with_admins += u64::from(!f.admins.is_empty());
        self.with_importance += u64::from(f.importance.is_some());
    }
}

/// What a region's extraction read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionCounts {
    pub tiles: u64,
    /// Tiles that came in a column before one read already. From the first, joined
    /// features are only given out at the end (see [`RegionExtractor`]).
    pub out_of_order: u64,
    /// The tiles' bytes, as given (gzipped).
    pub bytes: u64,
    pub layers: TileCounts,
    pub merge: MergeCounts,
}

/// A region's extraction. Points go to the sink as each tile is added. The joined lines and
/// areas go to it once no tile still to come can add to them, if tiles come in order of
/// column (z14 x, as the basemap gives a region's, and the app several regions'), so a big
/// region's streets aren't all held to the end; the rest at [`RegionExtractor::finish`].
/// Tiles in another order give the same features, only all at the end.
#[derive(Debug)]
pub struct RegionExtractor {
    merger: Merger,
    counts: RegionCounts,
    /// The last column read, while tiles come in order.
    column: Option<u32>,
    ordered: bool,
}

impl Default for RegionExtractor {
    fn default() -> Self {
        Self {
            merger: Merger::default(),
            counts: RegionCounts::default(),
            column: None,
            ordered: true,
        }
    }
}

impl RegionExtractor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads a tile (gzipped or not).
    ///
    /// # Errors
    ///
    /// If the tile doesn't decode; nothing of it is added then.
    pub fn add_tile(
        &mut self,
        tile: TileKey,
        data: &[u8],
        sink: &mut impl FeatureSink,
    ) -> Result<(), TileError> {
        let features = extract_tile(tile, data)?;
        match self.column {
            Some(column) if tile.x < column => {
                self.ordered = false;
                self.counts.out_of_order += 1;
            }
            Some(column) if tile.x > column && self.ordered => {
                for feature in self.merger.finished(tile.x) {
                    sink.feature(feature);
                }
                // A joined feature's center is among its pieces' tiles, give or take the
                // column of an edge it's on.
                let first = self
                    .merger
                    .first_open_column()
                    .map_or(tile.x, |c| c.min(tile.x));
                sink.columns_done(first.saturating_sub(1));
            }
            _ => {}
        }
        self.column = Some(self.column.map_or(tile.x, |c| c.max(tile.x)));
        self.counts.tiles += 1;
        self.counts.bytes += data.len() as u64;
        self.counts.layers.add(&features.counts);
        // A park's label point (with the names its outline lacks) joins its outline.
        let (labels, points): (Vec<_>, Vec<_>) = features
            .points
            .into_iter()
            .partition(|p| p.layer == SourceLayer::Park && p.id.is_some());
        sink.tile_points(tile, points);
        for (index, piece) in features.pieces.into_iter().chain(labels).enumerate() {
            self.merger
                .add(piece, u32::try_from(index).unwrap_or(u32::MAX));
        }
        Ok(())
    }

    /// What's been read so far.
    #[must_use]
    pub fn counts(&self) -> RegionCounts {
        RegionCounts {
            merge: self.merger.counts().clone(),
            ..self.counts.clone()
        }
    }

    /// Joins the lines and areas across tiles, gives them to the sink, and says what was read.
    pub fn finish(self, sink: &mut impl FeatureSink) -> RegionCounts {
        let counts = self.counts();
        for feature in self.merger.finish() {
            sink.feature(feature);
        }
        counts
    }
}
