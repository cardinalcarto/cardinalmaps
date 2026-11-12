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
        let (props, roof_props) = (layer.paint.attributes(), layer.paint.roof_attributes());

        // Roof vertices, and four per wall: 4 tower walls, 4 + 4 around the courtyard.
        assert_eq!(b.roofs.vertices.len(), 4 + 8, "{tessellation:?}");
        assert!(
            b.roofs
                .triangles
                .iter()
                .all(|&i| (i as usize) < b.roofs.vertices.len())
        );
        assert_eq!(b.walls.vertices.len(), 4 * 12);
        assert_eq!(b.walls.triangles.len(), 6 * 12);

        // Roofs are at the height whatever the base, so they have no base values.
        for (paint, props, count) in [
            (&b.roofs.paint, &roof_props[..], b.roofs.vertices.len()),
            (&b.walls.paint, &props[..], b.walls.vertices.len()),
        ] {
            let ids: Vec<_> = paint.features.iter().map(|f| f.id).collect();
            assert_eq!(ids, [Some(1), Some(2)]);
            assert_eq!(paint.vertex_count(), count);
            let bindings: Vec<_> = paint.attributes.iter().map(|a| a.binding).collect();
            let expected = [Binding::Uniform, Binding::Source, Binding::Composite];
            assert_eq!(bindings, expected[..props.len()]);
            let value = |name, feature: usize| {
                let at = paint.features[feature].vertices.end as usize - 1;
                paint.value(props, name, at)
            };
            assert_eq!(value("height", 0), [120.0]);
            assert_eq!(value("height", 1), [15.0]);
        }
        let at = b.walls.paint.features[1].vertices.end as usize - 1;
        assert_eq!(b.walls.paint.value(&props, "base", at), [0.0], "at zoom 14");
        assert!(b.roofs.patterns.is_empty() && b.walls.patterns.is_empty());
        assert!(b.walls.edge_distances.is_empty());
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
    for (patterns, paint, count) in [
        (&b.roofs.patterns, &b.roofs.paint, b.roofs.vertices.len()),
        (&b.walls.patterns, &b.walls.paint, b.walls.vertices.len()),
    ] {
        assert_eq!(patterns.len(), count);
        let tower = patterns[paint.features[0].vertices.start as usize];
        let square = atlas.get("square").unwrap();
        assert_eq!(tower.rect, square.rect.map(|v| v as f32));
        assert_eq!(tower.size, [4.0, 4.0]);
        // A missing image leaves the feature plain.
        let block = patterns[paint.features[1].vertices.start as usize];
        assert_eq!(block.rect, [0.0; 4]);
    }

    // Walls with patterns know how far along their rings they are.
    assert_eq!(b.walls.edge_distances.len(), b.walls.vertices.len());

    // Without sprites there is nothing to take patterns from.
    let processed = TileProcessor::new()
        .process(&style, "s", TileId::new(14, 1, 2), &tile(Tessellation::Off))
        .unwrap();
    let BucketGeometry::FillExtrusion(b) = &processed.buckets[0].geometry else {
        panic!()
    };
    assert!(b.roofs.patterns.is_empty() && b.walls.patterns.is_empty());
    assert!(b.walls.edge_distances.is_empty());
}
