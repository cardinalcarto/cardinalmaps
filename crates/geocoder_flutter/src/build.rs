//! Building the device's index: every z14 tile of the ready regions, once each, with the
//! Who's On First places fetched for them, into a new directory.
//!
//! The app builds a whole index for each change of its regions (see the decisions log's
//! "Device index lifecycle") into a fresh directory, and swaps it in when it's done.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use airmail::index::{AirmailIndex, AirmailIndexWriter, Storage, WriterOptions};
use airmail::poi::{Kind, ToIndexPoi};
use airmail::postcodes::Postcodes;

use crate::documents::{document, drop_duplicate_addresses, give_local_id, place};
use crate::extract::{ExtractedFeature, TileError, TileKey};
use crate::pool::DocumentPool;
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
    /// Builds the documents, on threads of its own.
    pool: DocumentPool,
    wof: WofPlaces,
    postcodes: Postcodes,
    pub counts: BuildCounts,
    /// The first document that failed to build, if one did.
    error: Option<anyhow::Error>,
    /// The areas most of each tile's points are in (an index into `area_sets`), for the
    /// features whose layers don't name theirs: streets, peaks, lakes, parks, airports.
    tile_areas: TileAreas,
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
            if let Some(set) = self.tile_areas.get(tile) {
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
        // Counted as it goes to the pool; if a document fails, the build does.
        self.counts.documents[kind_slot(doc.kind)] += 1;
        self.counts.pointers += u64::from(doc.tile.is_some() && doc.poi_id().is_some());
        self.counts.local_ids += u64::from(local);
        if let Err(e) = self.pool.add(doc, &self.writer) {
            self.error = Some(e);
        }
    }
}

/// Each z14 tile's areas (an index into `IndexSink::area_sets`), by column, then row. Tiles
/// come in order of column, so the rows of each column are pushed in order and the columns
/// behind the reading are forgotten (see `FeatureSink::columns_done`).
#[derive(Debug, Default)]
struct TileAreas(BTreeMap<u32, Vec<(u32, u32)>>);

impl TileAreas {
    fn insert(&mut self, tile: TileKey, set: u32) {
        let rows = self.0.entry(tile.x).or_default();
        match rows.last() {
            Some(&(y, _)) if y < tile.y => rows.push((tile.y, set)),
            None => rows.push((tile.y, set)),
            _ => match rows.binary_search_by_key(&tile.y, |&(y, _)| y) {
                Ok(i) => rows[i].1 = set,
                Err(i) => rows.insert(i, (tile.y, set)),
            },
        }
    }

    fn get(&self, tile: TileKey) -> Option<u32> {
        let rows = self.0.get(&tile.x)?;
        let i = rows.binary_search_by_key(&tile.y, |&(y, _)| y).ok()?;
        Some(rows[i].1)
    }

    fn forget_before(&mut self, column: u32) {
        self.0 = self.0.split_off(&column);
    }
}

/// The tiles read: a sorted list while they come in order, as they do, and a set for any
/// that don't.
#[derive(Debug, Default)]
struct SeenTiles {
    ordered: Vec<u64>,
    other: HashSet<u64>,
}

impl SeenTiles {
    /// Notes a tile; false if it was read already.
    fn insert(&mut self, tile: TileKey) -> bool {
        let key = (u64::from(tile.z) << 56) | (u64::from(tile.x) << 28) | u64::from(tile.y);
        if self.ordered.last().is_none_or(|&last| last < key) {
            self.ordered.push(key);
            return true;
        }
        self.ordered.binary_search(&key).is_err() && self.other.insert(key)
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

    fn columns_done(&mut self, column: u32) {
        self.tile_areas.forget_before(column);
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
    /// Threads building documents, besides the one reading tiles and tantivy's.
    pub builders: usize,
    /// Whether to merge the index into one segment at the end, which makes searches faster.
    pub merge: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            heap_bytes: 64_000_000,
            threads: 2,
            builders: default_builders(2),
            merge: true,
        }
    }
}

/// Threads to build documents on: the cores left beside the one reading tiles and
/// tantivy's `threads`, at least one.
#[must_use]
pub fn default_builders(threads: usize) -> usize {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    cores.saturating_sub(threads + 1).max(1)
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
    seen: SeenTiles,
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
        let pool = DocumentPool::new(&writer.document_builder(), SOURCE, options.builders);
        Ok(Self {
            index,
            extractor: RegionExtractor::new(),
            sink: IndexSink {
                writer,
                pool,
                wof,
                postcodes: Postcodes::default(),
                counts: BuildCounts::default(),
                error: None,
                tile_areas: TileAreas::default(),
                area_sets: Vec::new(),
                area_set_ids: HashMap::new(),
            },
            seen: SeenTiles::default(),
            options,
        })
    }

    /// Reads a tile (gzipped or not). A tile read already is skipped. Tiles in order of column
    /// (x), over all the regions, let the build finish each street as soon as it has passed
    /// it rather than hold them all to the end; any order gives the same features.
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
        // Only the writer is still needed: what was kept for reading goes before the commit
        // and the merge, which take the most memory.
        let Self {
            mut index,
            sink,
            options,
            seen,
            extractor: _,
        } = self;
        drop(seen);
        let IndexSink {
            writer,
            pool,
            counts,
            wof,
            tile_areas,
            area_sets,
            area_set_ids,
            ..
        } = sink;
        drop((wof, tile_areas, area_sets, area_set_ids));
        pool.finish(&writer).map_err(BuildError::Index)?;
        writer.commit().map_err(BuildError::Index)?;
        if options.merge {
            index
                .merge_blocking(WriterOptions {
                    heap_bytes: options.heap_bytes,
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

    fn t(x: u32, y: u32) -> TileKey {
        TileKey { z: 14, x, y }
    }

    #[test]
    fn tiles_are_seen_once_in_any_order() {
        let mut seen = SeenTiles::default();
        assert!(seen.insert(t(1, 5)));
        assert!(seen.insert(t(1, 7)));
        assert!(!seen.insert(t(1, 7)));
        assert!(seen.insert(t(2, 0)));
        // Out of order.
        assert!(!seen.insert(t(1, 5)));
        assert!(seen.insert(t(1, 6)));
        assert!(!seen.insert(t(1, 6)));
        assert!(seen.insert(t(3, 1)));
        assert!(!seen.insert(t(2, 0)));
    }

    #[test]
    fn tile_areas_by_column_and_row() {
        let mut areas = TileAreas::default();
        areas.insert(t(1, 5), 0);
        areas.insert(t(1, 9), 1);
        areas.insert(t(1, 7), 2);
        areas.insert(t(2, 1), 3);
        areas.insert(t(1, 9), 4);
        assert_eq!(
            [t(1, 5), t(1, 7), t(1, 9), t(2, 1), t(2, 2), t(0, 5)].map(|k| areas.get(k)),
            [Some(0), Some(2), Some(4), Some(3), None, None]
        );
        areas.forget_before(2);
        assert_eq!(areas.get(t(1, 5)), None);
        assert_eq!(areas.get(t(2, 1)), Some(3));
    }

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
