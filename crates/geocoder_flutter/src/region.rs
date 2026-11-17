//! Reading a whole region: its z14 tiles one after another, then the features joined across
//! them.

use crate::extract::{ExtractedFeature, TileCounts, TileError, TileKey, extract_tile};
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
    /// The tiles' bytes, as given (gzipped).
    pub bytes: u64,
    pub layers: TileCounts,
    pub merge: MergeCounts,
}

/// A region's extraction. Points go to the sink as each tile is added; the joined lines and
/// areas at [`RegionExtractor::finish`].
#[derive(Debug, Default)]
pub struct RegionExtractor {
    merger: Merger,
    counts: RegionCounts,
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
        self.counts.tiles += 1;
        self.counts.bytes += data.len() as u64;
        self.counts.layers.add(&features.counts);
        sink.tile_points(tile, features.points);
        for (index, piece) in features.pieces.into_iter().enumerate() {
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
