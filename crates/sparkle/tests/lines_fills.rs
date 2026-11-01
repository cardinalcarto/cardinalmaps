//! Line and fill buckets for the properties tile processing handles per feature: sort keys,
//! data-driven dash arrays and patterns, and line progress for `line-gradient`.

use std::path::Path;

use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};
use mlt_core::geo_types::Coord;
use mlt_core::{GeometryType, LayerWriter, PropKind};
use serde_json::{Value, json};
use sparkle::sprite::{ImageAtlas, SpriteSheet};
use sparkle::tile::{FillBucket, LineBucket};
use sparkle::{BucketGeometry, Style, TileId, TileProcessor, TileResources};

fn coords(points: &[(i32, i32)]) -> Vec<Coord<i32>> {
    points.iter().map(|&(x, y)| Coord { x, y }).collect()
}

/// Two roads (a primary over a path) and two squares (a tall one, then a low one).
fn tile() -> Vec<u8> {
    let cfg = EncoderConfig::from_profile(Profile::Native(Network::Cellular4G))
        .with_tessellation(Tessellation::Off);

    let mut roads = LayerWriter::new("roads", 4096).unwrap();
    let class = roads.add_property("class", PropKind::Str).unwrap();
    for (i, c) in ["primary", "path"].into_iter().enumerate() {
        let y = 1000 + i as i32 * 100;
        let mut f = roads.feature(GeometryType::LineString);
        f.id(Some(10 + i as u64))
            .line(coords(&[(0, y), (300, y), (300, y + 100)]))
            .unwrap()
            .property(class, c)
            .unwrap();
        f.finish().unwrap();
    }

    let mut buildings = LayerWriter::new("buildings", 4096).unwrap();
    let height = buildings.add_property("height", PropKind::U32).unwrap();
    for (i, h) in [50u32, 5].into_iter().enumerate() {
        let x = i as i32 * 200;
        let mut f = buildings.feature(GeometryType::Polygon);
        f.id(Some(20 + i as u64))
            .exterior_ring(coords(&[(x, 0), (x + 100, 0), (x + 100, 100), (x, 100)]))
            .unwrap()
            .property(height, h)
            .unwrap();
        f.finish().unwrap();
    }
    [roads.encode(cfg).unwrap(), buildings.encode(cfg).unwrap()].concat()
}

fn style(source: Value, layer: Value) -> Style {
    let style = Style::from_json(&json!({
        "version": 8,
        "sources": {"s": source},
        "layers": [layer],
    }))
    .unwrap();
    assert!(style.warnings.is_empty(), "{:?}", style.warnings);
    style
}

fn atlas() -> ImageAtlas {
    let prefix = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sprite/sprite");
    let sheet = SpriteSheet::load(&prefix, 1.0).unwrap();
    ImageAtlas::build([("default", &sheet)])
}

fn process(style: &Style, atlas: Option<&ImageAtlas>) -> BucketGeometry {
    let resources = TileResources {
        images: atlas,
        glyphs: None,
    };
    let mut tile = TileProcessor::new()
        .process_with(style, "s", TileId::new(14, 1, 2), &tile(), resources)
        .unwrap();
    assert_eq!(tile.buckets.len(), 1);
    tile.buckets.remove(0).geometry
}

fn line(style: &Style) -> LineBucket {
    match process(style, None) {
        BucketGeometry::Line(line) => line,
        other => panic!("{other:?}"),
    }
}

fn fill(style: &Style, atlas: Option<&ImageAtlas>) -> FillBucket {
    match process(style, atlas) {
        BucketGeometry::Fill(fill) => fill,
        other => panic!("{other:?}"),
    }
}

/// The feature (index into `paint.features`) each index belongs to.
fn features_of(paint: &sparkle::paint::PaintArrays, indices: &[u32]) -> Vec<usize> {
    let mut out: Vec<usize> = indices
        .iter()
        .map(|&i| {
            paint
                .features
                .iter()
                .position(|f| f.vertices.contains(&i))
                .unwrap()
        })
        .collect();
    out.dedup();
    out
}

fn vector() -> Value {
    json!({"type": "vector"})
}

#[test]
fn line_sort_key_orders_triangles() {
    let roads = |key: Value| {
        let style = style(
            vector(),
            json!({"id": "roads", "type": "line", "source": "s", "source-layer": "roads",
                   "layout": {"line-sort-key": key}}),
        );
        let bucket = line(&style);
        features_of(&bucket.paint, &bucket.triangles)
    };
    assert_eq!(
        roads(json!(["match", ["get", "class"], "path", 0, 1])),
        [1, 0]
    );
    assert_eq!(
        roads(json!(["match", ["get", "class"], "path", 2, 1])),
        [0, 1]
    );
}

#[test]
fn fill_sort_key_orders_triangles_and_outlines() {
    let style = style(
        vector(),
        json!({"id": "b", "type": "fill", "source": "s", "source-layer": "buildings",
               "layout": {"fill-sort-key": ["get", "height"]}}),
    );
    let bucket = fill(&style, None);
    // The low building is drawn first, under the tall one.
    assert_eq!(features_of(&bucket.paint, &bucket.triangles), [1, 0]);
    assert_eq!(features_of(&bucket.paint, &bucket.outlines), [1, 0]);
}

#[test]
fn data_driven_dasharray_is_resolved_per_feature() {
    let style = style(
        vector(),
        json!({"id": "roads", "type": "line", "source": "s", "source-layer": "roads",
               "layout": {"line-cap": "round"},
               "paint": {"line-dasharray": ["match", ["get", "class"], "path",
                                            ["literal", [1, 1]], ["literal", [3, 1]]]}}),
    );
    let bucket = line(&style);
    let dashes = bucket.dashes.as_ref().expect("a dash atlas");
    assert_eq!(bucket.images.len(), bucket.vertices.len());
    let first = bucket.images[0].a;
    let last = bucket.images.last().unwrap().a;
    // Round caps: a band of rows per pattern, lengths in line widths.
    assert_eq!(first[2], 4.0);
    assert_eq!(last[2], 2.0);
    assert_ne!(first[0], last[0]);
    assert_eq!(dashes.height(), 2 * sparkle::pattern::ROUND_DASH_ROWS);
}

#[test]
fn layer_wide_dasharray_is_left_to_the_renderer() {
    let style = style(
        vector(),
        json!({"id": "roads", "type": "line", "source": "s", "source-layer": "roads",
               "paint": {"line-dasharray": [2, 1]}}),
    );
    let bucket = line(&style);
    assert!(bucket.images.is_empty());
    assert!(bucket.dashes.is_none());
}

#[test]
fn line_progress_needs_geojson_line_metrics() {
    let layer = json!({"id": "roads", "type": "line", "source": "s", "source-layer": "roads",
                       "paint": {"line-gradient": ["interpolate", ["linear"], ["line-progress"],
                                                   0, "blue", 1, "red"]}});
    let progress = |source| {
        let bucket = line(&style(source, layer.clone()));
        bucket
            .vertices
            .iter()
            .map(|v| v.progress)
            .fold(0.0, f32::max)
    };
    assert_eq!(progress(vector()), 0.0);
    assert_eq!(
        progress(json!({"type": "geojson", "data": {}, "lineMetrics": false})),
        0.0
    );
    assert_eq!(
        progress(json!({"type": "geojson", "data": {}, "lineMetrics": true})),
        1.0
    );
}

#[test]
fn data_driven_patterns_are_resolved_per_feature() {
    let atlas = atlas();
    let pattern = json!(["case", [">", ["get", "height"], 10], "square", "missing"]);
    let fill_style = style(
        vector(),
        json!({"id": "b", "type": "fill", "source": "s", "source-layer": "buildings",
               "paint": {"fill-pattern": pattern}}),
    );
    let bucket = fill(&fill_style, Some(&atlas));
    assert_eq!(bucket.images.len(), bucket.vertices.len());
    let square = atlas.get("square").unwrap();
    assert_eq!(bucket.images[0].a, square.rect.map(|v| v as f32));
    assert_eq!(bucket.images[0].b[0], 1.0, "pixel ratio");
    // An image that isn't in the atlas is all zeros, which the renderer leaves out.
    assert_eq!(bucket.images.last().unwrap().a, [0.0; 4]);

    let line_style = style(
        vector(),
        json!({"id": "roads", "type": "line", "source": "s", "source-layer": "roads",
               "paint": {"line-pattern": ["match", ["get", "class"], "path", "dot", "bar"]}}),
    );
    let BucketGeometry::Line(roads) = process(&line_style, Some(&atlas)) else {
        panic!()
    };
    assert!(roads.dashes.is_none());
    let bar = atlas.get("bar").unwrap().rect.map(|v| v as f32);
    let dot = atlas.get("dot").unwrap().rect.map(|v| v as f32);
    assert_eq!(roads.images[0].a, bar);
    assert_eq!(roads.images.last().unwrap().a, dot);
}

/// `["image", name]` is null for images the sprite doesn't have, so `coalesce` falls
/// through to one it does. That is only known where the sprite is, so such a pattern is
/// resolved per feature even though it reads nothing of the feature.
#[test]
fn image_expressions_fall_back_to_available_images() {
    let atlas = atlas();
    let fill_style = style(
        vector(),
        json!({"id": "b", "type": "fill", "source": "s", "source-layer": "buildings",
               "paint": {"fill-pattern": ["coalesce", ["image", "missing"], ["image", "square"]]}}),
    );
    let bucket = fill(&fill_style, Some(&atlas));
    let square = atlas.get("square").unwrap().rect.map(|v| v as f32);
    assert_eq!(bucket.images.len(), bucket.vertices.len());
    assert!(bucket.images.iter().all(|i| i.a == square));
}
