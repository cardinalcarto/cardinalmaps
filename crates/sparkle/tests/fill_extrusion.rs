//! Fill extrusions end to end: buildings encoded as MLT, through a style, to buckets.

use std::path::Path;

use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};
use mlt_core::geo_types::Coord;
use mlt_core::{GeometryType, LayerWriter, PropKind};
use serde_json::json;
use sparkle::paint::Binding;
use sparkle::sprite::{ImageAtlas, SpriteSheet};
use sparkle::style::LayerKind;
use sparkle::{BucketGeometry, Style, TileId, TileProcessor, TileResources};

fn coords(points: &[(i32, i32)]) -> Vec<Coord<i32>> {
    points.iter().map(|&(x, y)| Coord { x, y }).collect()
}

/// Two buildings as OpenMapTiles has them: a tall square tower and a low block with a
/// courtyard, plus a point that extrusions ignore.
fn tile(tessellation: Tessellation) -> Vec<u8> {
    let mut building = LayerWriter::new("building", 4096).unwrap();
    let height = building
        .add_property("render_height", PropKind::U32)
        .unwrap();
    let min_height = building
        .add_property("render_min_height", PropKind::U32)
        .unwrap();
    let mut f = building.feature(GeometryType::Polygon);
    f.id(Some(1))
        .exterior_ring(coords(&[(100, 100), (200, 100), (200, 200), (100, 200)]))
        .unwrap()
        .property(height, 120u32)
        .unwrap()
        .property(min_height, 0u32)
        .unwrap();
    f.finish().unwrap();
    let mut f = building.feature(GeometryType::Polygon);
    f.id(Some(2))
        .exterior_ring(coords(&[(300, 300), (400, 300), (400, 400), (300, 400)]))
        .unwrap()
        .hole(coords(&[(320, 320), (320, 380), (380, 380), (380, 320)]))
        .unwrap()
        .property(height, 15u32)
        .unwrap()
        .property(min_height, 5u32)
        .unwrap();
    f.finish().unwrap();
    let mut f = building.feature(GeometryType::Point);
    f.points(coords(&[(5, 5)])).unwrap();
    f.finish().unwrap();
    let cfg = EncoderConfig::from_profile(Profile::Native(Network::Cellular4G))
        .with_tessellation(tessellation);
    building.encode(cfg).unwrap()
}

fn style(pattern: Option<serde_json::Value>) -> Style {
    let mut paint = json!({
        "fill-extrusion-color": "#aaa",
        "fill-extrusion-height": ["get", "render_height"],
        "fill-extrusion-base": ["interpolate", ["linear"], ["zoom"],
                                14, 0, 15, ["get", "render_min_height"]],
        "fill-extrusion-opacity": 0.8,
    });
    if let Some(pattern) = pattern {
        paint["fill-extrusion-pattern"] = pattern;
    }
    Style::from_json(&json!({
        "version": 8,
        "sources": {"s": {"type": "vector"}},
        "layers": [{"id": "3d", "type": "fill-extrusion", "source": "s",
                    "source-layer": "building", "paint": paint}]
    }))
    .unwrap()
}

#[test]
fn extrudes_buildings() {
    for tessellation in [Tessellation::Off, Tessellation::Always] {
        let style = style(None);
        assert!(style.warnings.is_empty(), "{:?}", style.warnings);
        let tile = TileProcessor::new()
            .process(&style, "s", TileId::new(14, 1, 2), &tile(tessellation))
            .unwrap();
        let [bucket] = tile.buckets.as_slice() else {
            panic!("{tessellation:?}")
        };
        let BucketGeometry::FillExtrusion(b) = &bucket.geometry else {
            panic!()
        };
        let LayerKind::FillExtrusion(layer) = &style.layers[0].kind else {
            panic!()
        };
        let props = layer.paint.attributes();

        // Roof vertices plus four per wall: 4 tower walls, 4 + 4 around the courtyard.
        let roof = b.vertices.iter().filter(|v| v.normal[2] == 1.0).count();
        assert_eq!(roof, 4 + 8, "{tessellation:?}");
        assert_eq!(b.vertices.len(), roof + 4 * 12);
        assert!(b.triangles.iter().all(|&i| (i as usize) < b.vertices.len()));
        // Every wall has as many bottom vertices as top ones.
        let tops = b.vertices.iter().filter(|v| v.top == 1.0).count();
        assert_eq!(tops, roof + 2 * 12);

        let ids: Vec<_> = b.paint.features.iter().map(|f| f.id).collect();
        assert_eq!(ids, [Some(1), Some(2)]);
        assert_eq!(b.paint.vertex_count(), b.vertices.len());
        let [color, height, base] = [0, 1, 2].map(|i| b.paint.attributes[i].binding);
        assert_eq!(color, Binding::Uniform);
        assert_eq!(height, Binding::Source);
        assert_eq!(base, Binding::Composite);
        let value = |name, feature: usize| {
            let at = b.paint.features[feature].vertices.end as usize - 1;
            b.paint.value(&props, name, at)
        };
        assert_eq!(value("height", 0), [120.0]);
        assert_eq!(value("height", 1), [15.0]);
        assert_eq!(value("base", 1), [0.0], "at zoom 14");
        assert!(b.patterns.is_empty());
    }
}

#[test]
fn patterns_come_from_the_sprite() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sprite/sprite");
    let sheet = SpriteSheet::load(&fixture, 1.0).unwrap();
    let atlas = ImageAtlas::build([("default", &sheet)]);
    let resources = TileResources {
        images: Some(&atlas),
        glyphs: None,
    };
    let pattern = json!([
        "case",
        [">", ["get", "render_height"], 100],
        "square",
        "missing"
    ]);
    let style = style(Some(pattern));
    let processed = TileProcessor::new()
        .process_with(
            &style,
            "s",
            TileId::new(14, 1, 2),
            &tile(Tessellation::Off),
            resources,
        )
        .unwrap();
    let BucketGeometry::FillExtrusion(b) = &processed.buckets[0].geometry else {
        panic!()
    };
    assert_eq!(b.patterns.len(), b.vertices.len());
    let tower = b.patterns[b.paint.features[0].vertices.start as usize];
    let square = atlas.get("square").unwrap();
    assert_eq!(tower.rect, square.rect.map(|v| v as f32));
    assert_eq!(tower.size, [4.0, 4.0]);
    // A missing image leaves the feature plain.
    let block = b.patterns[b.paint.features[1].vertices.start as usize];
    assert_eq!(block.rect, [0.0; 4]);

    // Without sprites there is nothing to take patterns from.
    let processed = TileProcessor::new()
        .process(&style, "s", TileId::new(14, 1, 2), &tile(Tessellation::Off))
        .unwrap();
    let BucketGeometry::FillExtrusion(b) = &processed.buckets[0].geometry else {
        panic!()
    };
    assert!(b.patterns.is_empty());
}
