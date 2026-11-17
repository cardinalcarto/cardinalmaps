//! Building the device's index: every z14 tile of the ready regions, once each, with the
//! Who's On First places fetched for them, into a new directory.
//!
//! The app builds a whole index for each change of its regions (see the decisions log's
//! "Device index lifecycle") into a fresh directory, and swaps it in when it's done.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

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
    /// Documents placed by their point in the areas' boundaries (their tiles don't name their
    /// areas), and the time that took, in microseconds.
    pub placed_by_point: u64,
    pub placing_micros: u64,
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
            // Streets, peaks, lakes, waterways, parks, airports and POIs without a stable ID:
            // placed by their point, as Planetiler places the others.
            let start = Instant::now();
            let admins = self.wof.admins_at(doc.center);
            place(&mut doc, &admins, &mut self.wof);
            self.counts.placed_by_point += 1;
            self.counts.placing_micros += start.elapsed().as_micros() as u64;
        } else {
            place(&mut doc, &admins, &mut self.wof);
        }
        Some(doc)
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

fn kind_slot(kind: Kind) -> usize {
    Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0)
}

impl FeatureSink for IndexSink {
    fn feature(&mut self, feature: ExtractedFeature) {
        if let Some(doc) = self.prepare(feature) {
            self.add(doc);
        }
    }

    fn tile_points(&mut self, _tile: TileKey, points: Vec<ExtractedFeature>) {
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
            counts,
            wof,
            ..
        } = sink;
        drop(wof);
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

    /// Lacey, in Washington, in the US, as the service sends them.
    const WOF: &str = r#"{"places": [
      {"id": 85633793, "placetype": "country",
       "names": [["und", "default", false, "United States"]], "ancestors": [], "country": "US",
       "lang_x_spoken": ["eng"], "geometry": null, "ancestor_only": true},
      {"id": 85688623, "placetype": "region", "names": [["und", "default", false, "Washington"]],
       "ancestors": [85633793], "country": "US", "geometry": null, "ancestor_only": true},
      {"id": 101730337, "placetype": "locality", "names": [["und", "default", false, "Lacey"]],
       "ancestors": [85688623, 85633793], "country": "US",
       "geometry": {"type": "Polygon", "coordinates": [[[-122.9, 47.0], [-122.7, 47.0], [-122.7, 47.1], [-122.9, 47.1], [-122.9, 47.0]]]}}
    ]}"#;

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn feature(
        layer: crate::extract::SourceLayer,
        id: Option<u64>,
        name: &str,
        geometry: geo::Geometry<f64>,
    ) -> ExtractedFeature {
        ExtractedFeature {
            layer,
            id,
            tiles: vec![t(2601, 5759)],
            names: pairs(&[("name", name)]),
            address: Vec::new(),
            admins: Vec::new(),
            importance: None,
            tags: Vec::new(),
            geometry,
        }
    }

    /// An index of a street (its tile names no areas), a shop whose tile names its areas,
    /// with the only address of its postcode, and a cafe far away.
    fn index() -> (tempfile::TempDir, crate::search::DeviceIndex) {
        use crate::extract::SourceLayer;
        use geo::{Geometry, LineString, MultiLineString, Point};
        let dir = tempfile::tempdir().unwrap();
        let mut wof = WofPlaces::new();
        wof.add_response(WOF.as_bytes()).unwrap();
        let options = BuildOptions {
            heap_bytes: 30_000_000,
            threads: 1,
            merge: false,
        };
        let mut build = IndexBuild::new(&dir.path().join("index"), wof, options).unwrap();
        let mut street = feature(
            SourceLayer::TransportationName,
            None,
            "Sleater Kinney Road Southeast",
            Geometry::MultiLineString(MultiLineString(vec![LineString::from(vec![
                (-122.836, 47.03),
                (-122.835, 47.05),
            ])])),
        );
        street.tags = pairs(&[("class", "secondary")]);
        build.sink.feature(street);
        let mut shop = feature(
            SourceLayer::Housenumber,
            Some(64_441_304_931),
            "Michaels",
            Geometry::Point(Point::new(-122.8355, 47.0412)),
        );
        shop.address = pairs(&[
            ("addr:housenumber", "701"),
            ("addr:postcode", "98503-1133"),
            ("addr:street", "Sleater Kinney Road Southeast"),
        ]);
        shop.admins = vec![("locality".to_owned(), 101_730_337)];
        let cafe = feature(
            SourceLayer::Poi,
            None,
            "Far Cafe",
            Geometry::Point(Point::new(-122.3, 47.6)),
        );
        build.sink.tile_points(t(2601, 5759), vec![shop, cafe]);
        let counts = build.finish().unwrap();
        assert_eq!(counts.placed_by_point, 2, "{counts:?}");
        let index =
            crate::search::DeviceIndex::open(&dir.path().join("index"), None, None).unwrap();
        (dir, index)
    }

    fn search(
        index: &crate::search::DeviceIndex,
        text: &str,
        bbox: Option<[f64; 4]>,
    ) -> Vec<serde_json::Value> {
        index
            .search(&crate::search::Query {
                text: text.into(),
                limit: 10,
                bbox,
                ..crate::search::Query::default()
            })
            .unwrap()
    }

    fn admin_names(result: &serde_json::Value) -> Vec<&str> {
        result["admins"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a["name"].as_str())
            .collect()
    }

    #[test]
    fn features_without_areas_are_placed_by_their_point() {
        let (_dir, index) = index();
        let street = &search(&index, "Sleater Kinney Road Southeast", None)[0];
        assert_eq!(street["kind"], "street");
        // (Results list the areas in the index: Washington and the US are ancestors only.)
        assert_eq!(admin_names(street), vec!["Lacey"]);
        assert_eq!(street["country_code"], "US");
        // Outside every boundary: no areas.
        let cafe = &search(&index, "Far Cafe", None)[0];
        assert!(admin_names(cafe).is_empty(), "{cafe}");
    }

    #[test]
    fn pois_postcodes_are_postcodes() {
        let (_dir, index) = index();
        let results = search(&index, "98503-1133", None);
        assert!(
            results
                .iter()
                .any(|r| r["kind"] == "postcode" && r["id"] == "postcode:US/98503-1133"),
            "{results:?}"
        );
    }

    #[test]
    fn a_bbox_keeps_only_results_in_it() {
        let (_dir, index) = index();
        assert_eq!(search(&index, "Far Cafe", None).len(), 1);
        assert!(search(&index, "Far Cafe", Some([-122.9, 47.0, -122.7, 47.1])).is_empty());
        let near = search(&index, "Michaels", Some([-122.9, 47.0, -122.7, 47.1]));
        assert_eq!(near[0]["name"], "Michaels");
    }
}
