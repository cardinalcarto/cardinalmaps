//! Building the device's index: every z14 tile of the ready regions, once each, with the
//! Who's On First places fetched for them, into a new directory.
//!
//! The app builds a whole index for each change of its regions (see the decisions log's
//! "Device index lifecycle") into a fresh directory, and swaps it in when it's done.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use airmail::index::{AirmailIndex, AirmailIndexWriter, DocumentBuilder, Storage, WriterOptions};
use airmail::poi::{Kind, SchemafiedPoi, ToIndexPoi};
use airmail::postcodes::Postcodes;

use crate::documents::{document, drop_duplicate_addresses, give_local_id, place};
use crate::extract::{ExtractedFeature, TileError, TileKey};
use crate::region::{FeatureSink, RegionExtractor};
use crate::wof::WofPlaces;

/// What documents' `source` field says: everything on the device is one source.
const SOURCE: &str = "device";

/// What a build has made so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildCounts {
    /// Tiles read, and tiles skipped as read already (another region has them too).
    pub tiles: u64,
    pub repeated_tiles: u64,
    pub bytes: u64,
    /// Features read, and those that aren't indexed (unnamed, junctions, ferries...).
    pub features: u64,
    pub unindexed: u64,
    /// Addresses dropped as a POI in their tile has the same one.
    pub duplicate_addresses: u64,
    /// Documents by [`Kind`] (as `Kind::ALL`).
    pub documents: [u64; 5],
    /// Of them, pointers, and those with a local ID (no other ID finds them).
    pub pointers: u64,
    pub local_ids: u64,
}

/// Turns features into documents and adds them to the writer.
pub struct IndexSink {
    writer: AirmailIndexWriter,
    builder: DocumentBuilder,
    wof: WofPlaces,
    postcodes: Postcodes,
    pub counts: BuildCounts,
    /// The first document that failed to build, if one did.
    error: Option<anyhow::Error>,
    /// The areas most of each tile's points are in (an index into `area_sets`), for the
    /// features whose layers don't name theirs: streets, peaks, lakes, parks, airports.
    tile_areas: HashMap<TileKey, u32>,
    area_sets: Vec<Vec<(String, i64)>>,
    area_set_ids: HashMap<Vec<(String, i64)>, u32>,
}

impl IndexSink {
    fn prepare(&mut self, mut feature: ExtractedFeature) -> Option<ToIndexPoi> {
        self.counts.features += 1;
        let admins = std::mem::take(&mut feature.admins);
        let Some(mut doc) = document(feature) else {
            self.counts.unindexed += 1;
            return None;
        };
        if admins.is_empty() {
            let tile = tile_of(doc.center.x(), doc.center.y());
            if let Some(&set) = self.tile_areas.get(&tile) {
                let admins = self.area_sets[set as usize].clone();
                place(&mut doc, &admins, &mut self.wof);
            }
        } else {
            place(&mut doc, &admins, &mut self.wof);
        }
        Some(doc)
    }

    /// Notes the areas most of a tile's points are in.
    fn note_tile_areas(&mut self, tile: TileKey, points: &[ExtractedFeature]) {
        let mut counts: HashMap<&[(String, i64)], usize> = HashMap::new();
        for p in points.iter().filter(|p| !p.admins.is_empty()) {
            *counts.entry(p.admins.as_slice()).or_default() += 1;
        }
        let Some((most, _)) = counts
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))
        else {
            return;
        };
        let set = match self.area_set_ids.get(most) {
            Some(&set) => set,
            None => {
                let set = u32::try_from(self.area_sets.len()).unwrap_or(u32::MAX);
                self.area_sets.push(most.to_vec());
                self.area_set_ids.insert(most.to_vec(), set);
                set
            }
        };
        self.tile_areas.insert(tile, set);
    }

    fn add(&mut self, mut doc: ToIndexPoi) {
        if self.error.is_some() {
            return;
        }
        let local = give_local_id(&mut doc);
        self.postcodes.add(&doc);
        let kind = doc.kind;
        let pointer = doc.tile.is_some() && doc.poi_id().is_some();
        let result = self
            .builder
            .document(SchemafiedPoi::from(doc), SOURCE)
            .and_then(|d| self.writer.add_document(d));
        match result {
            Ok(()) => {
                self.counts.documents[kind_slot(kind)] += 1;
                self.counts.pointers += u64::from(pointer);
                self.counts.local_ids += u64::from(local);
            }
            Err(e) => self.error = Some(e),
        }
    }
}

/// The z14 tile a point is in.
fn tile_of(lng: f64, lat: f64) -> TileKey {
    let n = f64::from(1u32 << 14);
    let x = ((lng + 180.0) / 360.0 * n).floor().clamp(0.0, n - 1.0);
    let lat = lat.to_radians();
    let y = ((1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
        .floor()
        .clamp(0.0, n - 1.0);
    TileKey {
        z: 14,
        x: x as u32,
        y: y as u32,
    }
}

fn kind_slot(kind: Kind) -> usize {
    Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0)
}

impl FeatureSink for IndexSink {
    fn feature(&mut self, feature: ExtractedFeature) {
        if let Some(doc) = self.prepare(feature) {
            self.add(doc);
        }
    }

    fn tile_points(&mut self, tile: TileKey, points: Vec<ExtractedFeature>) {
        self.note_tile_areas(tile, &points);
        let mut docs: Vec<ToIndexPoi> =
            points.into_iter().filter_map(|f| self.prepare(f)).collect();
        self.counts.duplicate_addresses += drop_duplicate_addresses(&mut docs) as u64;
        for doc in docs {
            self.add(doc);
        }
    }
}

/// How much a build may use.
#[derive(Clone, Copy, Debug)]
pub struct BuildOptions {
    /// Tantivy's indexing memory, shared by its threads (at least 15 MB each).
    pub heap_bytes: usize,
    pub threads: usize,
    /// Whether to merge the index into one segment at the end, which makes searches faster.
    pub merge: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            heap_bytes: 64_000_000,
            threads: 2,
            merge: true,
        }
    }
}

/// Why a build failed.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    Tile(#[from] TileError),
    #[error("the index: {0:#}")]
    Index(anyhow::Error),
}

/// A build of the index, tile page by tile page.
pub struct IndexBuild {
    index: AirmailIndex,
    extractor: RegionExtractor,
    sink: IndexSink,
    seen: HashSet<TileKey>,
    options: BuildOptions,
}

impl IndexBuild {
    /// Starts an index in `dir`, which must be empty or not exist, of the areas in `wof`.
    ///
    /// # Errors
    ///
    /// If the index can't be created there.
    pub fn new(dir: &Path, wof: WofPlaces, options: BuildOptions) -> Result<Self, BuildError> {
        let mut index = AirmailIndex::create(dir).map_err(BuildError::Index)?;
        let writer = index
            .writer_with(WriterOptions {
                heap_bytes: options.heap_bytes,
                threads: Some(options.threads),
                storage: Storage::Pointers,
            })
            .map_err(BuildError::Index)?;
        let builder = writer.document_builder();
        Ok(Self {
            index,
            extractor: RegionExtractor::new(),
            sink: IndexSink {
                writer,
                builder,
                wof,
                postcodes: Postcodes::default(),
                counts: BuildCounts::default(),
                error: None,
                tile_areas: HashMap::new(),
                area_sets: Vec::new(),
                area_set_ids: HashMap::new(),
            },
            seen: HashSet::new(),
            options,
        })
    }

    /// Reads a tile (gzipped or not). A tile read already is skipped.
    ///
    /// # Errors
    ///
    /// If the tile doesn't decode (nothing of it is added), or a document couldn't be added.
    pub fn add_tile(&mut self, tile: TileKey, data: &[u8]) -> Result<(), BuildError> {
        if !self.seen.insert(tile) {
            self.sink.counts.repeated_tiles += 1;
            return Ok(());
        }
        self.extractor.add_tile(tile, data, &mut self.sink)?;
        self.sink.counts.tiles += 1;
        self.sink.counts.bytes += data.len() as u64;
        self.check()
    }

    fn check(&mut self) -> Result<(), BuildError> {
        match self.sink.error.take() {
            Some(e) => Err(BuildError::Index(e)),
            None => Ok(()),
        }
    }

    #[must_use]
    pub fn counts(&self) -> &BuildCounts {
        &self.sink.counts
    }

    /// Joins streets and areas across tiles, adds them, the postcodes and the areas
    /// themselves, and commits (and merges) the index.
    ///
    /// # Errors
    ///
    /// If a document couldn't be added or the index can't be written.
    pub fn finish(mut self) -> Result<BuildCounts, BuildError> {
        let extractor = std::mem::take(&mut self.extractor);
        extractor.finish(&mut self.sink);
        self.check()?;
        let admins: Vec<ToIndexPoi> = self.sink.wof.documents().collect();
        for doc in admins {
            self.sink.add(doc);
        }
        for doc in std::mem::take(&mut self.sink.postcodes).into_pois() {
            self.sink.add(doc);
        }
        self.check()?;
        let IndexSink { writer, counts, .. } = self.sink;
        writer.commit().map_err(BuildError::Index)?;
        if self.options.merge {
            self.index
                .merge_blocking(WriterOptions {
                    heap_bytes: self.options.heap_bytes,
                    threads: Some(1),
                    storage: Storage::Pointers,
                })
                .map_err(BuildError::Index)?;
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_are_in_their_tiles() {
        // The fixtures' north tile, by Everett.
        assert_eq!(
            tile_of(-122.225, 47.97),
            TileKey {
                z: 14,
                x: 2629,
                y: 5697
            }
        );
        assert_eq!(tile_of(180.0, -89.0).x, 16383);
    }
}
