//! A phone's index stores documents with a stable POI ID as pointers to the
//! tile features they were read from, and everything else as the server does.

use airmail::index::{AirmailIndex, Hydrate, SearchRequest, Storage, WriterOptions};
use airmail::poi::{AdminBand, Kind, OsmRef, OsmType, PlaceId, Pointer, SchemafiedPoi, TileId, ToIndexPoi};
use geo::{Geometry, LineString, Point, Polygon};

const SEATTLE: u64 = 101_730_401;
const TILE: TileId = TileId { z: 14, x: 2_624, y: 5_722 };

fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn in_seattle(mut doc: ToIndexPoi) -> ToIndexPoi {
    doc.admin_ids = vec![SEATTLE];
    doc.admin_names = vec![(AdminBand::Locality, "Seattle".into())];
    doc.country_code = Some("US".into());
    doc.importance = 0.1;
    doc
}

/// A named feature from a tile, with its stable ID.
fn poi(name: &str, shape: Geometry<f64>, poi_id: Option<u64>) -> ToIndexPoi {
    let mut doc = airmail::osm::document(tags(&[("amenity", "cafe"), ("name", name), ("cuisine", "coffee_shop")]), shape).unwrap();
    doc.osm = Some(OsmRef { osm_type: OsmType::Node, id: 42, version: Some(3) });
    if let Some(poi_id) = poi_id {
        doc.set_pointer(Pointer { poi_id, tile: TILE });
    }
    in_seattle(doc)
}

fn address() -> ToIndexPoi {
    let at = Geometry::Point(Point::new(-122.3405, 47.6097));
    in_seattle(airmail::osm::document(tags(&[("addr:housenumber", "1530"), ("addr:street", "Post Alley")]), at).unwrap())
}

fn square(x: f64, y: f64, half: f64) -> Geometry<f64> {
    let ring = LineString::from(vec![(x - half, y - half), (x + half, y - half), (x + half, y + half), (x - half, y + half), (x - half, y - half)]);
    Geometry::Polygon(Polygon::new(ring, vec![]))
}

fn build(docs: Vec<ToIndexPoi>, storage: Storage) -> (tempfile::TempDir, AirmailIndex) {
    let dir = tempfile::tempdir().unwrap();
    let mut index = AirmailIndex::create(dir.path()).unwrap();
    let mut writer = index.writer_with(WriterOptions { heap_bytes: 50_000_000, threads: Some(1), storage }).unwrap();
    for doc in docs {
        writer.add_poi(SchemafiedPoi::from(doc), "region-1").unwrap();
    }
    writer.commit().unwrap();
    (dir, index)
}

fn search(text: &str) -> SearchRequest<'_> {
    SearchRequest { text, limit: 10, ..SearchRequest::default() }
}

#[test]
fn pointers_store_only_where_the_feature_is() {
    let docs = vec![poi("Corner Cafe", square(-122.34, 47.61, 0.0002), Some(7)), address()];
    let (_dir, index) = build(docs, Storage::Pointers);

    let hits = index.search(&search("corner cafe")).unwrap().hits;
    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert_eq!(hit.kind, Kind::Poi);
    assert_eq!(hit.pointer, Some(Pointer { poi_id: 7, tile: TILE }));
    // Its names, for ranking, and nothing else of its tags.
    assert_eq!(hit.tags, tags(&[("name", "Corner Cafe")]));
    assert!(hit.osm.is_none() && hit.wof.is_none() && hit.poi.is_none());
    assert_eq!(hit.geometry, Geometry::Point(hit.center));
    assert!((hit.center.x() + 122.34).abs() < 1e-6 && (hit.center.y() - 47.61).abs() < 1e-6);
    assert_eq!(hit.admin_ids, vec![SEATTLE]);
    assert!((hit.importance - 0.1).abs() < 1e-9);

    // Indexed as before: by category, by its areas' names, by ID.
    let by_tag = SearchRequest { tags: vec!["amenity=cafe".into()], ..search("cafe seattle") };
    assert_eq!(index.search(&by_tag).unwrap().hits.len(), 1);
    let found = index.lookup_blocking(&PlaceId::Poi(7)).unwrap().hits;
    assert_eq!(found[0].pointer.map(|p| p.poi_id), Some(7));
    let found = index.lookup_blocking(&PlaceId::Osm(OsmType::Node, 42)).unwrap().hits;
    assert_eq!(found.len(), 1);

    // A document without a stable ID is stored whole.
    let hits = index.search(&search("1530 post alley")).unwrap().hits;
    assert_eq!(hits.len(), 1);
    assert!(hits[0].pointer.is_none());
    assert_eq!(hits[0].tag("addr:street"), Some("Post Alley"));
}

#[test]
fn the_server_stores_everything() {
    let (_dir, index) = build(vec![poi("Corner Cafe", Geometry::Point(Point::new(-122.34, 47.61)), Some(7))], Storage::Full);
    let hit = &index.search(&search("corner cafe")).unwrap().hits[0];
    assert!(hit.pointer.is_none());
    assert_eq!(hit.tag("name"), Some("Corner Cafe"));
    assert!(hit.osm.is_some());
}

#[test]
fn a_pointer_needs_its_tile() {
    let dir = tempfile::tempdir().unwrap();
    let mut index = AirmailIndex::create(dir.path()).unwrap();
    let writer = index.writer_with(WriterOptions { heap_bytes: 50_000_000, threads: Some(1), storage: Storage::Pointers }).unwrap();
    let mut doc = poi("Corner Cafe", Geometry::Point(Point::new(-122.34, 47.61)), Some(7));
    doc.tile = None;
    assert!(writer.document_builder().document(SchemafiedPoi::from(doc), "region-1").is_err());
}

#[test]
fn pointers_in_a_box_are_where_their_centers_are() {
    let docs = vec![poi("Corner Cafe", square(-122.34, 47.61, 0.0002), Some(7)), poi("Corner Cafe", Geometry::Point(Point::new(-122.30, 47.61)), Some(8))];
    let (_dir, index) = build(docs, Storage::Pointers);
    let request = SearchRequest { bbox: Some([-122.3401, 47.6099, -122.3399, 47.6101]), ..search("corner cafe") };
    let hits = index.search(&request).unwrap().hits;
    assert_eq!(hits.iter().map(|h| h.pointer.unwrap().poi_id).collect::<Vec<_>>(), vec![7]);
    // Its center is outside this box, though its square isn't.
    let request = SearchRequest { bbox: Some([-122.33985, 47.6099, -122.3398, 47.6101]), ..search("corner cafe") };
    assert!(index.search(&request).unwrap().hits.is_empty());
}

/// Reads tags as a phone would from its tiles.
struct Tiles(Vec<(u64, Vec<(String, String)>)>);

impl Hydrate for Tiles {
    fn tags(&self, pointer: &Pointer) -> Option<Vec<(String, String)>> {
        assert_eq!(pointer.tile, TILE);
        self.0.iter().find(|(id, _)| *id == pointer.poi_id).map(|(_, tags)| tags.clone())
    }
}

/// A pointer stores its names, so it ranks by how much of them a query is:
/// its exact name outranks a stored document's longer one, hydrated or not.
#[test]
fn pointers_rank_by_their_names() {
    let at = Geometry::Point(Point::new(-122.34, 47.61));
    let docs = vec![poi("Pike Market", at.clone(), Some(7)), poi("Pike Market Garage", at, None)];
    let (_dir, index) = build(docs, Storage::Pointers);
    let first = |hydrate: Option<&dyn Hydrate>| {
        let hits = index.search(&SearchRequest { hydrate, ..search("pike market") }).unwrap().hits;
        assert_eq!(hits.len(), 2);
        hits.into_iter().next().unwrap()
    };
    assert_eq!(first(None).pointer.map(|p| p.poi_id), Some(7));
    let tiles = Tiles(vec![(7, tags(&[("amenity", "cafe"), ("name", "Pike Market"), ("cardinal:poi_id", "7")]))]);
    let hit = first(Some(&tiles));
    assert_eq!(hit.pointer.map(|p| p.poi_id), Some(7));
    assert_eq!(hit.tag("name"), Some("Pike Market"));
    assert_eq!(hit.tag("cardinal:poi_id"), None);
}
