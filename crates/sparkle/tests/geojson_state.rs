//! GeoJSON sources through tile processing, feature state, global state, queries and
//! promoteId, end to end.

use std::sync::Arc;

use maplibre_expr::Value;
use mlt_core::encoder::{EncoderConfig, Network, Profile};
use mlt_core::geo_types::Coord;
use mlt_core::{GeometryType, LayerWriter, PropKind};
use serde_json::json;
use sparkle::geojson::GeoJsonIndex;
use sparkle::query::PointQuery;
use sparkle::state::{FeatureStates, feature_id};
use sparkle::style::{LayerKind, Source};
use sparkle::{BucketGeometry, ProcessedTile, Style, TileId, TileProcessor, TileResources};

/// Seattle at zoom 12: the tile containing downtown.
const TILE: TileId = TileId {
    z: 12,
    x: 656,
    y: 1430,
};

fn tile_at(lng: f64, lat: f64, z: u8) -> TileId {
    let n = f64::from(z).exp2();
    let s = lat.to_radians().sin();
    let y = 0.5 - 0.25 * ((1.0 + s) / (1.0 - s)).ln() / std::f64::consts::PI;
    TileId::new(z, ((lng + 180.0) / 360.0 * n) as u32, (y * n) as u32)
}

fn data() -> serde_json::Value {
    json!({"type": "FeatureCollection", "features": [
        {"type": "Feature", "id": 1, "properties": {"name": "downtown"},
         "geometry": {"type": "Polygon", "coordinates": [[
            [-122.342, 47.603], [-122.328, 47.603], [-122.328, 47.613], [-122.342, 47.613], [-122.342, 47.603]
         ], [
            [-122.337, 47.606], [-122.333, 47.606], [-122.333, 47.609], [-122.337, 47.609], [-122.337, 47.606]
         ]]}},
        {"type": "Feature", "id": 2, "properties": {"name": "route"},
         "geometry": {"type": "LineString", "coordinates": [[-122.35, 47.60], [-122.32, 47.60]]}},
    ]})
}

fn style() -> Style {
    Style::from_json(&json!({
        "version": 8,
        "state": {"width": {"default": 2}},
        "sources": {
            "g": {"type": "geojson", "data": data(), "lineMetrics": true},
            "pts": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []},
                    "cluster": true}
        },
        "layers": [
            {"id": "fill", "type": "fill", "source": "g",
             "filter": ["==", ["geometry-type"], "Polygon"],
             "paint": {"fill-color": ["case", ["boolean", ["feature-state", "hover"], false],
                 "red", "blue"]}},
            {"id": "line", "type": "line", "source": "g",
             "filter": ["==", ["geometry-type"], "LineString"],
             "paint": {"line-width": ["*", ["global-state", "width"], ["get", "w"]]}},
            {"id": "points", "type": "circle", "source": "pts"},
        ]
    }))
    .unwrap()
}

fn index(style: &Style, source: &str) -> Arc<GeoJsonIndex> {
    let Source::GeoJson(source) = &style.sources[source] else {
        panic!()
    };
    Arc::new(GeoJsonIndex::load(source, None).unwrap())
}

fn process(style: &Style, index: &Arc<GeoJsonIndex>) -> ProcessedTile {
    TileProcessor::new().process_geojson(style, "g", TILE, index, TileResources::default())
}

fn fill_color(tile: &ProcessedTile, style: &Style) -> Vec<f32> {
    let bucket = &tile.buckets[0];
    let BucketGeometry::Fill(fill) = &bucket.geometry else {
        panic!()
    };
    let LayerKind::Fill(layer) = &style.layers[0].kind else {
        panic!()
    };
    fill.paint.value(&layer.paint.attributes(), "color", 0)
}

#[test]
fn geojson_tiles_go_through_the_builders() {
    let style = style();
    let index = index(&style, "g");
    let tile = process(&style, &index);
    assert_eq!(tile.buckets.len(), 2);
    let BucketGeometry::Fill(fill) = &tile.buckets[0].geometry else {
        panic!()
    };
    assert_eq!(fill.paint.features[0].id, Some(1));
    assert_eq!(tile.buckets[0].extent, GeoJsonIndex::EXTENT);
    assert!(matches!(tile.buckets[1].geometry, BucketGeometry::Line(_)));
    assert!(tile.features.is_some());
}

#[test]
fn feature_state_repaints_without_tessellating() {
    let style = style();
    let index = index(&style, "g");
    let mut tile = process(&style, &index);
    let bucket = &mut tile.buckets[0];
    assert!(bucket.retains_features());
    assert_eq!(fill_color(&tile, &style), [0.0, 0.0, 1.0, 1.0]);

    let vertices = match &tile.buckets[0].geometry {
        BucketGeometry::Fill(f) => f.vertices.clone(),
        _ => panic!(),
    };
    let mut states = FeatureStates::new();
    states.set("g", None, 1, [("hover".to_owned(), Value::Bool(true))]);
    let layer = &style.layers[0];
    assert!(tile.buckets[0].update_feature_state(layer, &states, Some(&[1])));
    assert_eq!(fill_color(&tile, &style), [1.0, 0.0, 0.0, 1.0]);
    // Other ids leave it alone.
    assert!(!tile.buckets[0].update_feature_state(layer, &states, Some(&[5])));
    let BucketGeometry::Fill(fill) = &tile.buckets[0].geometry else {
        panic!()
    };
    assert_eq!(fill.vertices, vertices, "geometry is untouched");

    states.remove("g", None, Some(1), Some("hover"));
    assert!(tile.buckets[0].update_feature_state(layer, &states, None));
    assert_eq!(fill_color(&tile, &style), [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn global_state_rebuilds_paint() {
    let mut style = style();
    let index = index(&style, "g");
    let mut tile = process(&style, &index);
    let line = &mut tile.buckets[1];
    assert!(
        line.retains_features(),
        "per-feature paint reads global state"
    );

    let diff = style.set_global_state("width", Some(json!(4)));
    assert_eq!(diff.repaint, [1].into());
    assert!(line.rebuild_paint(&style.layers[1], &FeatureStates::new()));
    let LayerKind::Line(layer) = &style.layers[1].kind else {
        panic!()
    };
    let BucketGeometry::Line(bucket) = &line.geometry else {
        panic!()
    };
    // `w` is unset, so the product falls back to the default width.
    assert_eq!(
        bucket.paint.value(&layer.paint.attributes(), "width", 0),
        [1.0]
    );
}

#[test]
fn point_queries() {
    let style = style();
    let index = index(&style, "g");
    let tile = process(&style, &index);
    let features = tile.features.unwrap();
    // The tile's corner in world coordinates, to aim queries.
    let at = |lng: f64, lat: f64| {
        let x = (lng + 180.0) / 360.0 * 4096.0 - f64::from(TILE.x);
        let s = lat.to_radians().sin();
        let y = (0.5 - 0.25 * ((1.0 + s) / (1.0 - s)).ln() / std::f64::consts::PI) * 4096.0
            - f64::from(TILE.y);
        PointQuery {
            position: [x, y],
            tile_size: 512.0,
            zoom: 12.0,
            tolerance: 0.0,
            layers: Vec::new(),
        }
    };
    let mut states = FeatureStates::new();
    states.set("g", None, 1, [("hover".to_owned(), Value::Bool(true))]);
    let hits = features
        .query_point(&style, "g", &at(-122.340, 47.610), Some(&states))
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, Some(1));
    assert_eq!(hits[0].properties["name"], Value::String("downtown".into()));
    assert_eq!(hits[0].state["hover"], Value::Bool(true));
    // The hole.
    let hits = features
        .query_point(&style, "g", &at(-122.335, 47.6075), None)
        .unwrap();
    assert!(hits.is_empty(), "{hits:?}");
    // On the line, south of the polygon.
    let hits = features
        .query_point(&style, "g", &at(-122.335, 47.60), None)
        .unwrap();
    let layers: Vec<_> = hits.iter().map(|h| h.layer).collect();
    assert_eq!(layers, [1], "{hits:?}");
}

#[test]
fn clusters_are_queryable() {
    let style = style();
    let Source::GeoJson(source) = &style.sources["pts"] else {
        panic!()
    };
    let points: Vec<_> = (0..30)
        .map(|i| {
            json!({"type": "Feature", "properties": {},
                   "geometry": {"type": "Point",
                       "coordinates": [-122.3355 + f64::from(i % 6) * 2e-4, 47.6085 + f64::from(i / 6) * 2e-4]}})
        })
        .collect();
    let index = Arc::new(
        GeoJsonIndex::new(
            source,
            &json!({"type": "FeatureCollection", "features": points}),
        )
        .unwrap(),
    );
    let tile =
        TileProcessor::new().process_geojson(&style, "pts", TILE, &index, TileResources::default());
    let clusters = index.tile(TILE);
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters[0].properties["point_count"], Value::Number(30.0));
    let mlt_core::geo_types::Geometry::Point(p) = clusters[0].geometry else {
        panic!()
    };
    let query = PointQuery {
        position: [f64::from(p.0.x) / 8192.0, f64::from(p.0.y) / 8192.0],
        tile_size: 512.0,
        zoom: 12.0,
        tolerance: 0.0,
        layers: Vec::new(),
    };
    let hits = tile
        .features
        .unwrap()
        .query_point(&style, "pts", &query, None)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].properties["cluster"], Value::Bool(true));
    // Deep enough, the points are apart.
    let deep = index.tile(tile_at(-122.3355, 47.6085, 18));
    assert!(!deep.is_empty());
    assert!(
        deep.iter().all(|f| !f.properties.contains_key("cluster")),
        "{deep:?}"
    );
}

#[test]
fn promote_id_for_vector_sources() {
    let mut roads = LayerWriter::new("roads", 4096).unwrap();
    let osm = roads.add_property("osm", PropKind::Str).unwrap();
    for (i, key) in ["way/7", "42"].into_iter().enumerate() {
        let y = 100 + i as i32 * 100;
        let mut f = roads.feature(GeometryType::LineString);
        f.id(Some(1000 + i as u64))
            .line(vec![Coord { x: 0, y }, Coord { x: 500, y }])
            .unwrap()
            .property(osm, key)
            .unwrap();
        f.finish().unwrap();
    }
    let data = roads
        .encode(EncoderConfig::from_profile(Profile::Native(
            Network::Cellular4G,
        )))
        .unwrap();
    let style = Style::from_json(&json!({
        "version": 8,
        "sources": {"s": {"type": "vector", "promoteId": {"roads": "osm"}}},
        "layers": [{"id": "roads", "type": "line", "source": "s", "source-layer": "roads",
                    "paint": {"line-color": ["case", ["boolean", ["feature-state", "on"], false], "red", "blue"]}}]
    }))
    .unwrap();
    let tile = TileProcessor::new()
        .process(&style, "s", TileId::new(14, 0, 0), &data)
        .unwrap();
    let BucketGeometry::Line(line) = &tile.buckets[0].geometry else {
        panic!()
    };
    let ids: Vec<_> = line.paint.features.iter().map(|f| f.id).collect();
    assert_eq!(ids, [feature_id(&Value::String("way/7".into())), Some(42)]);
}

/// Feature state reaches every layer type with per-feature paint, not just fills and
/// lines: circles, heatmaps, extrusions and symbol icons.
#[test]
fn feature_state_reaches_every_paint_kind() {
    let prefix =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sprite/sprite");
    let sheet = sparkle::sprite::SpriteSheet::load(&prefix, 1.0).unwrap();
    let atlas = sparkle::sprite::ImageAtlas::build([("default", &sheet)]);
    let hover = |on: f64, off: f64| {
        json!([
            "case",
            ["boolean", ["feature-state", "hover"], false],
            on,
            off
        ])
    };
    let style = Style::from_json(&json!({
        "version": 8,
        "sources": {"g": {"type": "geojson", "data": {"type": "FeatureCollection", "features": [
            {"type": "Feature", "id": 1, "properties": {},
             "geometry": {"type": "Point", "coordinates": [-122.335, 47.608]}},
            {"type": "Feature", "id": 3, "properties": {},
             "geometry": {"type": "Polygon", "coordinates": [[
                [-122.34, 47.60], [-122.33, 47.60], [-122.33, 47.61], [-122.34, 47.60]]]}},
        ]}}},
        "layers": [
            {"id": "circle", "type": "circle", "source": "g",
             "paint": {"circle-radius": hover(9.0, 3.0)}},
            {"id": "heat", "type": "heatmap", "source": "g",
             "paint": {"heatmap-weight": hover(9.0, 3.0)}},
            {"id": "extrusion", "type": "fill-extrusion", "source": "g",
             "paint": {"fill-extrusion-height": hover(9.0, 3.0)}},
            {"id": "icons", "type": "symbol", "source": "g",
             "filter": ["==", ["geometry-type"], "Point"],
             "layout": {"icon-image": "square"},
             "paint": {"icon-opacity": hover(0.9, 0.3)}},
        ]
    }))
    .unwrap();
    let index = index(&style, "g");
    let resources = TileResources {
        images: Some(&atlas),
        glyphs: None,
    };
    let mut tile = TileProcessor::new().process_geojson(&style, "g", TILE, &index, resources);
    assert_eq!(tile.buckets.len(), 4);
    let value = |tile: &ProcessedTile, i: usize| -> f32 {
        let layer = &style.layers[i];
        let geometry = &tile.buckets[i].geometry;
        match (geometry, &layer.kind) {
            (BucketGeometry::Circle(b), LayerKind::Circle(l)) => {
                b.paint.value(&l.paint.attributes(), "radius", 0)[0]
            }
            (BucketGeometry::Heatmap(b), LayerKind::Heatmap(l)) => {
                b.paint.value(&l.paint.attributes(), "weight", 0)[0]
            }
            (BucketGeometry::FillExtrusion(b), LayerKind::FillExtrusion(l)) => {
                let roofs = b.roofs.paint.value(&l.paint.roof_attributes(), "height", 0)[0];
                let walls = b.walls.paint.value(&l.paint.attributes(), "height", 0)[0];
                assert_eq!(roofs, walls);
                roofs
            }
            (BucketGeometry::Symbol(b), LayerKind::Symbol(l)) => {
                b.icons.paint.value(&l.icon_attributes(), "opacity", 0)[0]
            }
            _ => panic!("bucket {i} doesn't match its layer"),
        }
    };
    let before: Vec<f32> = (0..4).map(|i| value(&tile, i)).collect();
    assert_eq!(before, [3.0, 3.0, 3.0, 0.3]);

    let mut states = FeatureStates::new();
    states.set("g", None, 1, [("hover".to_owned(), Value::Bool(true))]);
    states.set("g", None, 3, [("hover".to_owned(), Value::Bool(true))]);
    for (i, bucket) in tile.buckets.iter_mut().enumerate() {
        assert!(bucket.retains_features(), "bucket {i}");
        assert!(
            bucket.update_feature_state(&style.layers[i], &states, None),
            "bucket {i}"
        );
    }
    let after: Vec<f32> = (0..4).map(|i| value(&tile, i)).collect();
    assert_eq!(after, [9.0, 9.0, 9.0, 0.9]);
}
