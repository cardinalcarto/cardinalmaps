//! Building the device's index from two real tiles of the Washington release (north
//! Seattle, `fixtures/`) and a WOF service response, then searching it as the app does.

use geocoder_flutter::build::{BuildOptions, IndexBuild};
use geocoder_flutter::extract::{SourceLayer, TileKey, poi_properties};
use geocoder_flutter::region::RegionExtractor;
use geocoder_flutter::search::{DeviceIndex, Query};
use geocoder_flutter::wof::WofPlaces;
use serde_json::Value;

const NORTH: TileKey = TileKey {
    z: 14,
    x: 2629,
    y: 5697,
};
const SOUTH: TileKey = TileKey {
    z: 14,
    x: 2629,
    y: 5698,
};

fn fixture(key: TileKey) -> Vec<u8> {
    let path = format!(
        "{}/fixtures/{}-{}-{}.mlt.gz",
        env!("CARGO_MANIFEST_DIR"),
        key.z,
        key.x,
        key.y
    );
    std::fs::read(path).unwrap()
}

/// Seattle, in Washington, in the US, as the service sends them for a box over the tiles.
const WOF: &str = r#"{"places": [
  {"id": 85633793, "placetype": "country",
   "names": [["und", "default", false, "United States"], ["eng", "preferred", true, "US"]],
   "ancestors": [], "country": "US", "lang_x_spoken": ["eng"], "label": [-97.0, 39.0],
   "geometry": null, "ancestor_only": true},
  {"id": 85688623, "placetype": "region",
   "names": [["und", "default", false, "Washington"], ["eng", "preferred", true, "WA"]],
   "ancestors": [85633793], "country": "US", "label": [-120.5, 47.4], "geometry": null,
   "ancestor_only": true},
  {"id": 101730401, "placetype": "locality",
   "names": [["und", "default", false, "Seattle"]],
   "ancestors": [85688623, 85633793], "country": "US", "population": 737015,
   "label": [-122.33, 47.61],
   "geometry": {"type": "Polygon", "coordinates": [[[-122.44, 47.49], [-122.23, 47.49], [-122.23, 47.74], [-122.44, 47.74], [-122.44, 47.49]]]}}
]}"#;

fn build() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut wof = WofPlaces::new();
    wof.add_response(WOF.as_bytes()).unwrap();
    let options = BuildOptions {
        heap_bytes: 30_000_000,
        threads: 1,
        merge: true,
    };
    let mut build = IndexBuild::new(&dir.path().join("index"), wof, options).unwrap();
    // A second region with the same tiles adds nothing.
    for key in [NORTH, SOUTH, NORTH] {
        build.add_tile(key, &fixture(key)).unwrap();
    }
    assert_eq!(build.counts().repeated_tiles, 1);
    let counts = build.finish().unwrap();
    assert_eq!(counts.tiles, 2);
    assert!(counts.pointers > 10, "{counts:?}");
    // Seattle; Washington and the US are only its ancestors.
    assert_eq!(counts.documents[2], 1, "{counts:?}");
    assert!(counts.documents[3] > 10, "{counts:?}");
    dir
}

fn search(index: &DeviceIndex, text: &str) -> Vec<Value> {
    index
        .search(&Query {
            text: text.into(),
            limit: 10,
            focus: Some([-122.33, 47.67]),
            ..Query::default()
        })
        .unwrap()
}

/// A named POI of the north tile with an ID, as extraction makes it.
fn a_named_poi() -> (u64, String) {
    let mut features = Vec::new();
    let mut extractor = RegionExtractor::new();
    extractor
        .add_tile(NORTH, &fixture(NORTH), &mut features)
        .unwrap();
    features
        .iter()
        .filter(|f| f.layer == SourceLayer::Poi && f.id.is_some())
        .filter_map(|f| Some((f.id?, f.name("name")?.to_owned())))
        .filter(|(_, name)| name.split_whitespace().count() >= 2)
        .min()
        .unwrap()
}

#[test]
fn pois_are_pointers_to_their_tiles() {
    let dir = build();
    let index = DeviceIndex::open(&dir.path().join("index"), None, None).unwrap();
    let (poi_id, name) = a_named_poi();
    let results = search(&index, &name);
    let hit = results
        .iter()
        .find(|r| r["id"] == format!("poi:{poi_id}"))
        .unwrap_or_else(|| panic!("{name}: {results:#?}"));
    assert_eq!(hit["kind"], "poi");
    assert_eq!(hit["name"], name.as_str());
    assert_eq!(hit["pointer"]["poi_id"], poi_id);
    assert_eq!(hit["pointer"]["tile"], serde_json::json!([14, 2629, 5697]));
    // Only its names are stored; the rest is in its tile.
    let tags = hit["osm"]["tags"].as_object().unwrap();
    assert!(tags.keys().all(|k| k.contains("name")), "{tags:?}");
    let (layer, properties) = poi_properties(NORTH, &fixture(NORTH), poi_id)
        .unwrap()
        .unwrap();
    assert_eq!(layer, SourceLayer::Poi);
    assert!(properties.contains(&("name".to_owned(), name.clone())));
    assert!(properties.iter().any(|(k, _)| k == "class"));
    // And found again by its ID.
    let place = index
        .place(&format!("poi:{poi_id}"), None)
        .unwrap()
        .unwrap();
    assert_eq!(place["name"], name.as_str());
}

#[test]
fn streets_and_areas_are_stored_whole() {
    let dir = build();
    let index = DeviceIndex::open(&dir.path().join("index"), None, None).unwrap();
    let seattle = &search(&index, "seattle")[0];
    assert_eq!(seattle["id"], "wof:101730401");
    assert_eq!(seattle["kind"], "admin");
    assert_eq!(seattle["feature"]["geometry"]["type"], "Polygon");
    assert_eq!(seattle["country_code"], "US");

    // A street of the tiles, found by its name, with its line.
    let mut features = Vec::new();
    let mut extractor = RegionExtractor::new();
    for key in [NORTH, SOUTH] {
        extractor
            .add_tile(key, &fixture(key), &mut Vec::new())
            .unwrap();
    }
    extractor.finish(&mut features);
    let name = features
        .iter()
        .filter(|f| f.layer == SourceLayer::TransportationName && f.tag("class") == Some("minor"))
        .filter_map(|f| f.name("name"))
        .min()
        .unwrap()
        .to_owned();
    let found = search(&index, &name);
    let street = found
        .iter()
        .find(|r| r["kind"] == "street" && r["name"] == name.as_str())
        .unwrap_or_else(|| panic!("{name}: {found:#?}"));
    assert!(street["id"].as_str().unwrap().starts_with("osm:way/"));
    assert!(
        street["feature"]["geometry"]["type"]
            .as_str()
            .unwrap()
            .contains("LineString")
    );
    assert!(street.get("pointer").is_none_or(Value::is_null));
}

/// Every result opens again by its ID, the way the app's place card fetches a place: a
/// pointer by `poi:`, a document with no other ID (a POI with neither a stable nor an OSM
/// ID, a joined street) by its `local:` ID.
#[test]
fn every_result_opens_by_its_id() {
    let dir = build();
    let index = DeviceIndex::open(&dir.path().join("index"), None, None).unwrap();
    let mut features = Vec::new();
    let mut extractor = RegionExtractor::new();
    for key in [NORTH, SOUTH] {
        extractor
            .add_tile(key, &fixture(key), &mut features)
            .unwrap();
    }
    extractor.finish(&mut features);
    let mut names: Vec<String> = features
        .iter()
        .filter_map(|f| f.name("name").map(str::to_owned))
        .collect();
    names.sort();
    names.dedup();
    let mut prefixes = std::collections::BTreeMap::<String, usize>::new();
    for name in &names {
        for result in search(&index, name) {
            let id = result["id"].as_str().unwrap();
            let prefix = id.split(':').next().unwrap();
            assert_ne!(prefix, "point", "{result:#}");
            *prefixes.entry(prefix.to_owned()).or_default() += 1;
            let place = index
                .place(id, None)
                .unwrap()
                .unwrap_or_else(|| panic!("{id} doesn't open: {result:#}"));
            for key in ["id", "kind", "name", "center"] {
                assert_eq!(place[key], result[key], "{key}: {place:#} {result:#}");
            }
            // Search joins a street's nearby pieces into its best one's result, as the
            // server does; the ID opens that piece.
            if result["kind"] != "street" {
                assert_eq!(place["feature"], result["feature"]);
            }
            let tags = place["osm"]["tags"].as_object().unwrap();
            assert!(!tags.contains_key("cardinal:local_id"), "{place:#}");
        }
    }
    assert!(
        prefixes.get("local").is_some_and(|n| *n > 0),
        "{prefixes:?}"
    );
    assert!(prefixes.get("poi").is_some_and(|n| *n > 0), "{prefixes:?}");
    // Not IDs of this index.
    for id in [
        "local:1",
        "local:01",
        "local:+5",
        "local:",
        "point:-122.3,47.6",
    ] {
        assert!(index.place(id, None).unwrap().is_none(), "{id}");
    }
}
