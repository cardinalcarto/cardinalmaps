//! End to end: MLT bytes encoded by mlt-core, through a style, to buckets.

use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};
use mlt_core::geo_types::Coord;
use mlt_core::{GeometryType, LayerWriter, PropKind};
use serde_json::json;
use sparkle::paint::{Binding, PaintArrays, PaintProperties};
use sparkle::style::{Color, LayerKind};
use sparkle::{BucketGeometry, Style, TileId, TileProcessor, TileResources};

fn coords(points: &[(i32, i32)]) -> Vec<Coord<i32>> {
    points.iter().map(|&(x, y)| Coord { x, y }).collect()
}

fn encode(layer: &LayerWriter<'_>, tessellation: Tessellation) -> Vec<u8> {
    let cfg = EncoderConfig::from_profile(Profile::Native(Network::Cellular4G))
        .with_tessellation(tessellation);
    layer.encode(cfg).unwrap()
}

/// Water polygons (one with a hole) and roads of two classes, plus a layer no style uses.
fn tile(tessellation: Tessellation) -> Vec<u8> {
    let mut water = LayerWriter::new("water", 4096).unwrap();
    let kind = water.add_property("kind", PropKind::Str).unwrap();
    let mut f = water.feature(GeometryType::Polygon);
    f.id(Some(1))
        .exterior_ring(coords(&[(0, 0), (100, 0), (100, 100), (0, 100)]))
        .unwrap()
        .hole(coords(&[(20, 20), (20, 40), (40, 40), (40, 20)]))
        .unwrap()
        .property(kind, "lake")
        .unwrap();
    f.finish().unwrap();
    let mut f = water.feature(GeometryType::MultiPolygon);
    f.id(Some(2))
        .exterior_ring(coords(&[(200, 200), (300, 200), (300, 300)]))
        .unwrap()
        .exterior_ring(coords(&[(400, 400), (500, 400), (500, 500), (400, 500)]))
        .unwrap()
        .property(kind, "ocean")
        .unwrap();
    f.finish().unwrap();

    let mut roads = LayerWriter::new("roads", 4096).unwrap();
    let class = roads.add_property("class", PropKind::Str).unwrap();
    let width = roads.add_property("width", PropKind::U32).unwrap();
    for (i, (c, w)) in [("primary", 6u32), ("service", 1)].into_iter().enumerate() {
        let y = 1000 + i as i32 * 100;
        let mut f = roads.feature(GeometryType::LineString);
        f.id(Some(10 + i as u64))
            .line(coords(&[(0, y), (500, y), (500, y + 50)]))
            .unwrap()
            .property(class, c)
            .unwrap()
            .property(width, w)
            .unwrap();
        f.finish().unwrap();
    }

    let mut unused = LayerWriter::new("poi", 4096).unwrap();
    let mut f = unused.feature(GeometryType::Point);
    f.points(coords(&[(5, 5)])).unwrap();
    f.finish().unwrap();

    [
        encode(&water, tessellation),
        encode(&roads, tessellation),
        encode(&unused, tessellation),
    ]
    .concat()
}

fn style() -> Style {
    Style::from_json(&json!({
        "version": 8,
        "sources": {"s": {"type": "vector"}},
        "layers": [
            {"id": "background", "type": "background"},
            {"id": "water", "type": "fill", "source": "s", "source-layer": "water",
             "paint": {
                "fill-color": ["match", ["get", "kind"], "lake", "#00f", "#008"],
                "fill-opacity": ["interpolate", ["linear"], ["zoom"], 0, 0.5, 20, 1]
             }},
            {"id": "water-outline", "type": "line", "source": "s", "source-layer": "water",
             "filter": ["==", "kind", "lake"]},
            {"id": "roads", "type": "line", "source": "s", "source-layer": "roads",
             "filter": ["==", ["get", "class"], "primary"],
             "layout": {"line-join": "round", "line-cap": "round"},
             "paint": {"line-width": ["get", "width"], "line-color": "red",
                       "line-opacity": ["interpolate", ["linear"], ["zoom"],
                                        10, ["/", ["get", "width"], 10], 11, 1]}},
            {"id": "other-source", "type": "line", "source": "elsewhere", "source-layer": "roads"},
            {"id": "far-only", "type": "fill", "source": "s", "source-layer": "water", "minzoom": 16},
        ]
    }))
    .unwrap()
}

fn triangle_area(positions: impl Fn(u32) -> [f32; 2], triangles: &[u32]) -> f32 {
    triangles
        .chunks(3)
        .map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(&positions);
            ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
        })
        .sum()
}

fn feature_ids(paint: &PaintArrays) -> Vec<Option<u64>> {
    paint.features.iter().map(|f| f.id).collect()
}

/// The value of attribute `name` for the `feature`th feature of a bucket.
fn feature_value(
    paint: &PaintArrays,
    props: &PaintProperties<'_>,
    name: &str,
    feature: usize,
) -> Vec<f32> {
    paint.value(props, name, paint.features[feature].vertices.start as usize)
}

#[test]
fn processes_fills_and_lines() {
    for tessellation in [Tessellation::Off, Tessellation::Always] {
        let style = style();
        assert!(style.warnings.is_empty(), "{:?}", style.warnings);
        let tile = TileProcessor::new()
            .process(&style, "s", TileId::new(10, 1, 2), &tile(tessellation))
            .unwrap();

        let ids: Vec<_> = tile
            .buckets
            .iter()
            .map(|b| style.layers[b.style_layer].id.as_str())
            .collect();
        assert_eq!(ids, ["water", "water-outline", "roads"], "{tessellation:?}");
        assert!(tile.buckets.iter().all(|b| b.extent == 4096));

        let BucketGeometry::Fill(water) = &tile.buckets[0].geometry else {
            panic!()
        };
        let LayerKind::Fill(water_style) = &style.layers[1].kind else {
            panic!()
        };
        let props = water_style.paint.attributes();
        assert_eq!(feature_ids(&water.paint), [Some(1), Some(2)]);
        assert_eq!(water.paint.attributes[0].binding, Binding::Source);
        assert_eq!(
            water.paint.attributes[1].binding,
            Binding::Uniform,
            "zoom only"
        );
        assert_eq!(water.paint.vertex_count(), water.vertices.len());
        let lake = feature_value(&water.paint, &props, "color", 0);
        assert_eq!(lake, Color([0.0, 0.0, 1.0, 1.0]).0);
        assert_ne!(feature_value(&water.paint, &props, "color", 1), lake);
        // Lake: 100x100 minus a 20x20 hole. Ocean: a triangle and a square.
        let area = triangle_area(|i| water.vertices[i as usize].position, &water.triangles);
        let expected = (10_000.0 - 400.0) + 5_000.0 + 10_000.0;
        assert_eq!(area, expected, "{tessellation:?}");
        assert_eq!(water.vertices.len(), 4 + 4 + 3 + 4);
        assert_eq!(water.outlines.len(), 2 * water.vertices.len());

        let BucketGeometry::Line(outline) = &tile.buckets[1].geometry else {
            panic!()
        };
        assert_eq!(
            feature_ids(&outline.paint),
            [Some(1)],
            "legacy filter keeps only the lake"
        );
        assert_eq!(outline.paint.stride, 0, "every property is a uniform");

        let BucketGeometry::Line(roads) = &tile.buckets[2].geometry else {
            panic!()
        };
        let LayerKind::Line(roads_style) = &style.layers[3].kind else {
            panic!()
        };
        let props = roads_style.paint.attributes();
        assert_eq!(feature_ids(&roads.paint), [Some(10)]);
        assert_eq!(feature_value(&roads.paint, &props, "width", 0), [6.0]);
        assert_eq!(
            feature_value(&roads.paint, &props, "color", 0),
            [1.0, 0.0, 0.0, 1.0]
        );
        // Composite: the opacity at zoom 10 and 11 for every vertex.
        let opacity = roads.paint.attributes[1];
        assert_eq!(opacity.binding, Binding::Composite);
        let at = opacity.offset as usize;
        for vertex in roads.paint.data.chunks(roads.paint.stride as usize) {
            assert_eq!(vertex[at..at + 2], [0.6, 1.0]);
        }
        assert_eq!(roads.paint.zoom_range, [10.0, 11.0]);
        assert_eq!(
            roads.paint.uniforms(&props, 10.5).factors[0][1],
            0.5,
            "factor for the second attribute"
        );
        let max_distance = roads
            .vertices
            .iter()
            .map(|v| v.distance)
            .fold(0.0, f32::max);
        assert_eq!(max_distance, 550.0);
    }
}

#[test]
fn layers_hidden_at_zoom_are_skipped() {
    let style = style();
    let tile = TileProcessor::new()
        .process(&style, "s", TileId::new(16, 0, 0), &tile(Tessellation::Off))
        .unwrap();
    let ids: Vec<_> = tile
        .buckets
        .iter()
        .map(|b| style.layers[b.style_layer].id.as_str())
        .collect();
    assert_eq!(ids, ["water", "water-outline", "roads", "far-only"]);
}

#[test]
fn magnified_tiles_follow_the_view_zoom() {
    // A tile from a source that stops at 14, drawn at 16: layers and paint follow 16.
    let style = style();
    let processed = TileProcessor::new()
        .process_at(
            &style,
            "s",
            TileId::new(14, 0, 0),
            16,
            &tile(Tessellation::Off),
            TileResources::default(),
        )
        .unwrap();
    assert_eq!(processed.zoom, 16);
    let ids: Vec<_> = processed
        .buckets
        .iter()
        .map(|b| style.layers[b.style_layer].id.as_str())
        .collect();
    assert_eq!(ids, ["water", "water-outline", "roads", "far-only"]);
    let BucketGeometry::Fill(water) = &processed.buckets[0].geometry else {
        panic!()
    };
    assert_eq!(water.paint.zoom_range, [16.0, 17.0]);

    // Never below the tile's own zoom.
    let processed = TileProcessor::new()
        .process_at(
            &style,
            "s",
            TileId::new(14, 0, 0),
            10,
            &tile(Tessellation::Off),
            TileResources::default(),
        )
        .unwrap();
    assert_eq!(processed.zoom, 14);
}

#[test]
fn unknown_source_produces_nothing() {
    let tile = TileProcessor::new()
        .process(
            &style(),
            "nope",
            TileId::new(10, 0, 0),
            &tile(Tessellation::Off),
        )
        .unwrap();
    assert!(tile.buckets.is_empty());
}

#[test]
fn buffers_cast_to_bytes() {
    let tile = TileProcessor::new()
        .process(
            &style(),
            "s",
            TileId::new(10, 0, 0),
            &tile(Tessellation::Off),
        )
        .unwrap();
    let BucketGeometry::Fill(water) = &tile.buckets[0].geometry else {
        panic!()
    };
    let bytes: &[u8] = bytemuck::cast_slice(&water.vertices);
    assert_eq!(bytes.len(), water.vertices.len() * 8);
    assert_eq!(
        water.paint.data.len(),
        water.vertices.len() * water.paint.stride as usize
    );
}
