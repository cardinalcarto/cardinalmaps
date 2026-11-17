//! Extracting a region's features from a small basemap: tiles encoded here, stored as the app
//! stores them (gzipped, in an MBTiles with `tile_regions`), read back page by page, and two
//! real tiles of the Washington release (`fixtures/`, before the geocoder's tile attributes).

use std::io::Write;
use std::path::PathBuf;

use geo::{Coord, CoordsIter, Geometry};
use geocoder_flutter::extract::{ExtractedFeature, SourceLayer, TileKey, extract_tile};
use geocoder_flutter::merge::CELL_LEVEL;
use geocoder_flutter::region::RegionExtractor;
use mlt_core::encoder::{EncoderConfig, Network, Profile};
use mlt_core::geo_types::Coord as TileCoord;
use mlt_core::{GeometryType, LayerWriter, PropValueRef};
use offline_map::mbtiles::{Basemap, BasemapReader};
use offline_map::plan::ChunkTile;
use offline_map::pmtiles::Compression;

const EXTENT: u32 = 4096;
/// Two tiles side by side over Seattle, and one a few kilometres east.
const WEST: TileKey = TileKey {
    z: 14,
    x: 2629,
    y: 5697,
};
const EAST: TileKey = TileKey {
    z: 14,
    x: 2630,
    y: 5697,
};
const FAR: TileKey = TileKey {
    z: 14,
    x: 2640,
    y: 5697,
};

fn cfg() -> EncoderConfig {
    EncoderConfig::from_profile(Profile::Native(Network::Cellular4G))
}

fn c(x: i32, y: i32) -> TileCoord<i32> {
    TileCoord { x, y }
}

type Props<'a> = Vec<(&'a str, PropValueRef<'a>)>;

enum Shape {
    Point(i32, i32),
    MultiPoint(Vec<(i32, i32)>),
    Line(Vec<(i32, i32)>),
    Ring(Vec<(i32, i32)>),
}

/// A layer of features, each with its own properties (columns are made as needed).
fn layer(name: &str, features: Vec<(Option<u64>, Shape, Props<'_>)>) -> Vec<u8> {
    let mut writer = LayerWriter::new(name, EXTENT).unwrap();
    let mut keys = Vec::new();
    for (_, _, props) in &features {
        for (key, value) in props {
            if !keys.iter().any(|(k, _)| k == key) {
                keys.push((*key, writer.add_property(key, value.kind()).unwrap()));
            }
        }
    }
    for (id, shape, props) in features {
        let kind = match &shape {
            Shape::Point(..) => GeometryType::Point,
            Shape::MultiPoint(_) => GeometryType::MultiPoint,
            Shape::Line(_) => GeometryType::LineString,
            Shape::Ring(_) => GeometryType::Polygon,
        };
        let mut f = writer.feature(kind);
        f.id(id);
        match shape {
            Shape::Point(x, y) => {
                f.points([c(x, y)]).unwrap();
            }
            Shape::MultiPoint(points) => {
                f.points(points.into_iter().map(|(x, y)| c(x, y))).unwrap();
            }
            Shape::Line(points) => {
                f.line(points.into_iter().map(|(x, y)| c(x, y))).unwrap();
            }
            Shape::Ring(points) => {
                f.exterior_ring(points.into_iter().map(|(x, y)| c(x, y)))
                    .unwrap();
            }
        }
        for (key, value) in props {
            let column = keys.iter().find(|(k, _)| *k == key).unwrap().1;
            f.property(column, value).unwrap();
        }
        f.finish().unwrap();
    }
    writer.encode(cfg()).unwrap()
}

fn s(v: &str) -> PropValueRef<'_> {
    PropValueRef::Str(v)
}

fn west_tile() -> Vec<u8> {
    [
        layer(
            "poi",
            vec![
                (
                    Some(42),
                    Shape::Point(100, 100),
                    vec![
                        ("name", s("Café Uno")),
                        ("name:en", s("Cafe One")),
                        ("name_en", s("Cafe One")),
                        ("name:latin", s("Cafe Uno")),
                        ("alt_name:de", s("Kaffee Eins;Café 1")),
                        ("addr:street", s("Main Street")),
                        ("addr:housenumber", s("12")),
                        ("cardinal:admin:locality", PropValueRef::I64(101_730_401)),
                        ("cardinal:importance", PropValueRef::F64(0.25)),
                        ("class", s("cafe")),
                        ("subclass", s("cafe")),
                        ("rank", PropValueRef::U32(3)),
                    ],
                ),
                // In the buffer: the east tile has it.
                (
                    Some(43),
                    Shape::Point(4100, 50),
                    vec![("name", s("Buffer Bakery")), ("class", s("bakery"))],
                ),
            ],
        ),
        layer(
            "housenumber",
            vec![(
                Some(9),
                Shape::MultiPoint(vec![(10, 10), (-20, 10)]),
                vec![("housenumber", s("5"))],
            )],
        ),
        layer(
            "transportation_name",
            vec![(
                Some(7),
                Shape::Line(vec![(4000, 2000), (4060, 2000), (4150, 2000)]),
                vec![
                    ("name", s("Main Street")),
                    ("name:es", s("Calle Mayor")),
                    ("class", s("minor")),
                ],
            )],
        ),
        layer(
            "park",
            vec![(
                Some(11),
                Shape::Ring(vec![(3900, 3900), (4196, 3900), (4196, 4000), (3900, 4000)]),
                vec![("name", s("Green Park")), ("class", s("park"))],
            )],
        ),
        // A layer the index doesn't read.
        layer(
            "water",
            vec![(None, Shape::Point(5, 5), vec![("class", s("lake"))])],
        ),
    ]
    .concat()
}

fn east_tile() -> Vec<u8> {
    [
        layer(
            "poi",
            vec![(
                Some(43),
                Shape::Point(4, 50),
                vec![("name", s("Buffer Bakery")), ("class", s("bakery"))],
            )],
        ),
        layer(
            "transportation_name",
            vec![(
                Some(5),
                Shape::Line(vec![(-100, 2000), (0, 2000), (50, 2000)]),
                vec![
                    ("name", s("Main Street")),
                    ("name:fr", s("Rue Principale")),
                    ("name:es", s("Calle Principal")),
                    ("class", s("primary")),
                ],
            )],
        ),
        layer(
            "park",
            vec![(
                Some(11),
                Shape::Ring(vec![(-100, 3900), (200, 3900), (200, 4000), (-100, 4000)]),
                vec![("name", s("Green Park")), ("class", s("park"))],
            )],
        ),
    ]
    .concat()
}

fn far_tile() -> Vec<u8> {
    layer(
        "transportation_name",
        vec![
            (
                Some(3),
                Shape::Line(vec![(100, 100), (900, 100)]),
                vec![("name", s("Main Street")), ("class", s("minor"))],
            ),
            // Nothing to index it by.
            (
                Some(4),
                Shape::Line(vec![(100, 300), (900, 300)]),
                vec![("class", s("minor"))],
            ),
        ],
    )
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("geocoder-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A basemap holding `tiles` (gzipped) as region `region`, as a download writes it.
fn basemap(path: &std::path::Path, tiles: &[(TileKey, Vec<u8>)], region: i64) {
    let mut basemap = Basemap::open(path).unwrap();
    let mut chunk = Vec::new();
    let mut list = Vec::new();
    for (key, data) in tiles {
        let data = gzip(data);
        list.push(ChunkTile {
            z: key.z,
            x: key.x,
            y: key.y,
            offset: chunk.len() as u32,
            length: data.len() as u32,
        });
        chunk.extend(data);
    }
    basemap
        .unpack(&chunk, chunk.len() as u64, &list, region, Compression::Gzip)
        .unwrap();
}

/// Every feature of `region`, reading its tiles in pages of `page` (in the given order of
/// pages, reversed if `reverse`).
fn extract(path: &std::path::Path, region: i64, page: u32, reverse: bool) -> Vec<ExtractedFeature> {
    let reader = BasemapReader::open(path).unwrap();
    let mut pages = Vec::new();
    let mut after = None;
    loop {
        let tiles = reader.region_tiles(region, 14, after, page).unwrap();
        let Some(last) = tiles.last() else { break };
        after = Some((last.x, last.y));
        pages.push(tiles);
    }
    if reverse {
        pages.reverse();
    }
    let mut extractor = RegionExtractor::new();
    let mut out = Vec::new();
    for tile in pages.into_iter().flatten() {
        let key = TileKey {
            z: tile.z,
            x: tile.x,
            y: tile.y,
        };
        extractor.add_tile(key, &tile.data, &mut out).unwrap();
    }
    extractor.finish(&mut out);
    out
}

fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn find(features: &[ExtractedFeature], layer: SourceLayer, id: u64) -> &ExtractedFeature {
    let found: Vec<_> = features
        .iter()
        .filter(|f| f.layer == layer && f.id == Some(id))
        .collect();
    assert_eq!(found.len(), 1, "{layer:?} {id}: {found:?}");
    found[0]
}

fn cell(c: Coord<f64>) -> u64 {
    s2::cellid::CellID::from(s2::latlng::LatLng::from_degrees(c.y, c.x))
        .parent(CELL_LEVEL)
        .0
}

#[test]
fn a_regions_features_come_out_once_with_their_attributes() {
    let dir = scratch("region");
    let path = dir.join("basemap.mbtiles");
    basemap(
        &path,
        &[(WEST, west_tile()), (EAST, east_tile()), (FAR, far_tile())],
        3,
    );
    // Another region's tile isn't read.
    basemap(&path, &[(TileKey { z: 14, x: 1, y: 1 }, far_tile())], 4);
    let features = extract(&path, 3, 1, false);

    let cafe = find(&features, SourceLayer::Poi, 42);
    assert_eq!(cafe.tiles, vec![WEST]);
    assert_eq!(
        cafe.names,
        pairs(&[
            ("alt_name:de", "Kaffee Eins;Café 1"),
            ("name", "Café Uno"),
            ("name:en", "Cafe One"),
        ])
    );
    assert_eq!(
        cafe.address,
        pairs(&[("addr:housenumber", "12"), ("addr:street", "Main Street")])
    );
    assert_eq!(cafe.admins, vec![("locality".to_owned(), 101_730_401)]);
    assert_eq!(cafe.importance, Some(0.25));
    assert_eq!(
        cafe.tags,
        pairs(&[("class", "cafe"), ("rank", "3"), ("subclass", "cafe")])
    );
    let Geometry::Point(p) = cafe.geometry else {
        panic!("{:?}", cafe.geometry)
    };
    // 100/4096 of the way into tile 2629 from its west edge.
    let lon = (2629.0 + 100.0 / 4096.0) / 16384.0 * 360.0 - 180.0;
    assert!((p.x() - lon).abs() < 1e-9, "{p:?}");
    assert!((47.9..48.0).contains(&p.y()), "{p:?}");

    // The bakery is in the west tile's buffer: only the east tile gives it.
    assert_eq!(find(&features, SourceLayer::Poi, 43).tiles, vec![EAST]);
    // Of the multipoint, only the point in the tile is kept.
    let house = find(&features, SourceLayer::Housenumber, 9);
    assert!(matches!(house.geometry, Geometry::Point(_)));
    assert_eq!(house.tag("housenumber"), Some("5"));

    // The street's two pieces are one street, with the primary piece's attributes, the lowest
    // ID, every name, and the primary piece's value of a name both have.
    let streets: Vec<_> = features
        .iter()
        .filter(|f| f.layer == SourceLayer::TransportationName)
        .collect();
    assert_eq!(streets.len(), 2, "{streets:?}");
    let main = find(&features, SourceLayer::TransportationName, 5);
    assert_eq!(main.tiles, vec![WEST, EAST]);
    assert_eq!(main.tag("class"), Some("primary"));
    assert_eq!(
        main.names,
        pairs(&[
            ("name", "Main Street"),
            ("name:es", "Calle Principal"),
            ("name:fr", "Rue Principale"),
        ])
    );
    let Geometry::MultiLineString(lines) = &main.geometry else {
        panic!("{:?}", main.geometry)
    };
    assert_eq!(lines.0.len(), 2);
    // Each tile's piece stops at its edge, the edge between them, so they don't overlap.
    let edge = 2630.0 / 16384.0 * 360.0 - 180.0;
    let span = |l: &geo::LineString<f64>| {
        let xs = l.0.iter().map(|c| c.x);
        (
            xs.clone().fold(f64::MAX, f64::min),
            xs.fold(f64::MIN, f64::max),
        )
    };
    let mut spans: Vec<_> = lines.0.iter().map(span).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    // (Joined shapes are in single precision.)
    assert!((spans[0].1 - edge).abs() < 1e-5, "{spans:?}");
    assert!((spans[1].0 - edge).abs() < 1e-5, "{spans:?}");
    // (They're joined because their middles share a cell.)
    assert_eq!(cell(lines.0[0].0[1]), cell(lines.0[1].0[1]));
    // The Main Street kilometres away is another street.
    let far = find(&features, SourceLayer::TransportationName, 3);
    assert_eq!(far.tiles, vec![FAR]);
    assert_ne!(
        cell(far.geometry.coords_iter().next().unwrap()),
        cell(lines.0[0].0[1])
    );

    let park = find(&features, SourceLayer::Park, 11);
    assert_eq!(park.tiles, vec![WEST, EAST]);
    assert!(matches!(&park.geometry, Geometry::MultiPolygon(mp) if mp.0.len() == 2));

    // Nothing else: the unread layer and the unnamed street are left out.
    assert_eq!(features.len(), 6, "{features:#?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Read in order of column, a street or area comes out once the tiles have passed every
/// column it could have pieces in, not only at the end.
#[test]
fn joined_features_come_out_once_the_tiles_have_passed_them() {
    let tiles = [(WEST, west_tile()), (EAST, east_tile()), (FAR, far_tile())];
    let joined = |features: &[ExtractedFeature]| -> Vec<u64> {
        let mut ids: Vec<u64> = features
            .iter()
            .filter(|f| matches!(f.layer, SourceLayer::TransportationName | SourceLayer::Park))
            .filter_map(|f| f.id)
            .collect();
        ids.sort_unstable();
        ids
    };
    let mut extractor = RegionExtractor::new();
    let mut read = Vec::new();
    for (key, tile) in &tiles {
        extractor.add_tile(*key, tile, &mut read).unwrap();
    }
    // Main Street and the park, of the west and east tiles, are finished by the far tile
    // (kilometres east); the far tile's street isn't until the end.
    assert_eq!(joined(&read), vec![5, 11]);
    let mut at_end = Vec::new();
    let counts = extractor.finish(&mut at_end);
    assert_eq!(joined(&at_end), vec![3]);
    assert_eq!(counts.out_of_order, 0);

    // Out of order: the same features, all at the end.
    let mut extractor = RegionExtractor::new();
    let mut read_unordered = Vec::new();
    for i in [2, 0, 1] {
        let (key, tile) = &tiles[i];
        extractor.add_tile(*key, tile, &mut read_unordered).unwrap();
    }
    assert!(joined(&read_unordered).is_empty());
    let counts = extractor.finish(&mut read_unordered);
    assert_eq!(counts.out_of_order, 2);
    assert_eq!(joined(&read_unordered), vec![3, 5, 11]);
    let key = |f: &ExtractedFeature| (f.layer, f.id, f.tiles.clone());
    read.extend(at_end);
    read.sort_by_key(key);
    read_unordered.sort_by_key(key);
    assert_eq!(read, read_unordered);
}

#[test]
fn the_order_tiles_come_in_doesnt_matter() {
    let dir = scratch("order");
    let path = dir.join("basemap.mbtiles");
    basemap(
        &path,
        &[(WEST, west_tile()), (EAST, east_tile()), (FAR, far_tile())],
        1,
    );
    let forward = extract(&path, 1, 1, false);
    let backward = extract(&path, 1, 1, true);
    let key = |f: &ExtractedFeature| (f.layer, f.id, f.tiles.clone());
    let sorted = |mut v: Vec<ExtractedFeature>| {
        v.sort_by_key(key);
        v
    };
    assert_eq!(sorted(forward), sorted(backward));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_bad_tile_says_which() {
    let tile = TileKey { z: 14, x: 1, y: 2 };
    let mut bad = gzip(&west_tile());
    let at = bad.len() / 2;
    bad[at] ^= 0xff;
    let error = extract_tile(tile, &bad).unwrap_err().to_string();
    assert!(error.contains("14/1/2"), "{error}");
    let error = extract_tile(tile, b"not a tile").unwrap_err().to_string();
    assert!(error.contains("MLT"), "{error}");
}

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

/// Two tiles of the Washington release (north Seattle, one above the other).
#[test]
fn real_tiles() {
    let north = TileKey {
        z: 14,
        x: 2629,
        y: 5697,
    };
    let south = TileKey { y: 5698, ..north };
    let mut extractor = RegionExtractor::new();
    let mut features = Vec::new();
    for key in [north, south] {
        extractor
            .add_tile(key, &fixture(key), &mut features)
            .unwrap();
    }
    let counts = extractor.finish(&mut features);
    let poi = SourceLayer::Poi as usize;
    assert!(counts.layers.points[poi] > 10, "{counts:?}");
    assert!(counts.layers.buffer_points[poi] > 0, "{counts:?}");
    // No POI with a poi_id comes out twice (those without have none), and each is in the
    // tile it came from.
    let mut ids: Vec<_> = features
        .iter()
        .filter(|f| f.layer == SourceLayer::Poi)
        .filter_map(|f| f.id)
        .collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert!(count > 10);
    assert_eq!(ids.len(), count);
    for f in features.iter().filter(|f| f.layer == SourceLayer::Poi) {
        let Geometry::Point(p) = f.geometry else {
            panic!()
        };
        let x = ((p.x() + 180.0) / 360.0 * 16384.0).floor() as u32;
        assert_eq!(x, 2629);
        // Derived names never come through.
        assert!(
            f.names
                .iter()
                .all(|(k, _)| k != "name_en" && k != "name:latin")
        );
        assert!(
            f.tags
                .iter()
                .all(|(k, _)| k != "name_en" && k != "name_int")
        );
    }
    // Streets crossing between the two tiles are joined.
    assert!(
        features
            .iter()
            .any(|f| f.layer == SourceLayer::TransportationName && f.tiles == vec![north, south]),
    );
    assert!(features.iter().all(|f| !f.tiles.is_empty()));
    // Each tile gives only its own part of a line, so a joined street has no segment
    // twice (as the tiles' buffers would give it).
    for f in features
        .iter()
        .filter(|f| f.layer == SourceLayer::TransportationName)
    {
        let mut segments: Vec<[i64; 4]> = Vec::new();
        let key = |c: Coord<f64>| [(c.x * 1e6).round() as i64, (c.y * 1e6).round() as i64];
        let lines = match &f.geometry {
            Geometry::LineString(l) => vec![l.clone()],
            Geometry::MultiLineString(ml) => ml.0.clone(),
            other => panic!("{other:?}"),
        };
        for line in lines.iter().flat_map(|l| l.lines()) {
            let (a, b) = (key(line.start), key(line.end));
            if a != b {
                segments.push(if a < b {
                    [a[0], a[1], b[0], b[1]]
                } else {
                    [b[0], b[1], a[0], a[1]]
                });
            }
        }
        let count = segments.len();
        segments.sort_unstable();
        segments.dedup();
        assert_eq!(
            segments.len(),
            count,
            "{:?}: {:?}",
            f.name("name"),
            f.geometry
        );
    }
}
