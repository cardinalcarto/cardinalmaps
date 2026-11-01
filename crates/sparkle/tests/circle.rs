//! Circle layers end to end: MLT points through a style to circle buckets.

use mlt_core::encoder::{EncoderConfig, Network, Profile};
use mlt_core::geo_types::Coord;
use mlt_core::{GeometryType, LayerWriter, PropKind};
use serde_json::json;
use sparkle::paint::{Binding, PaintArrays, PaintProperties};
use sparkle::style::LayerKind;
use sparkle::tile::CircleBucket;
use sparkle::{BucketGeometry, Style, TileId, TileProcessor};

/// Points with a `rank`: three single points, one multipoint of two, and one point in the
/// buffer outside the tile.
fn tile() -> Vec<u8> {
    let mut poi = LayerWriter::new("poi", 4096).unwrap();
    let rank = poi.add_property("rank", PropKind::U32).unwrap();
    for (id, (x, y), r) in [(1, (10, 10), 3u32), (2, (20, 20), 1), (3, (30, 30), 2)] {
        let mut f = poi.feature(GeometryType::Point);
        f.id(Some(id))
            .points([Coord { x, y }])
            .unwrap()
            .property(rank, r)
            .unwrap();
        f.finish().unwrap();
    }
    let mut f = poi.feature(GeometryType::MultiPoint);
    f.id(Some(4))
        .points([Coord { x: 100, y: 100 }, Coord { x: 200, y: 200 }])
        .unwrap()
        .property(rank, 5u32)
        .unwrap();
    f.finish().unwrap();
    let mut f = poi.feature(GeometryType::Point);
    f.id(Some(5))
        .points([Coord { x: -50, y: 10 }])
        .unwrap()
        .property(rank, 0u32)
        .unwrap();
    f.finish().unwrap();
    poi.encode(EncoderConfig::from_profile(Profile::Native(
        Network::Cellular4G,
    )))
    .unwrap()
}

fn style(layout: serde_json::Value, paint: serde_json::Value) -> Style {
    Style::from_json(&json!({
        "version": 8,
        "sources": {"s": {"type": "vector"}},
        "layers": [
            {"id": "poi", "type": "circle", "source": "s", "source-layer": "poi",
             "layout": layout, "paint": paint},
        ]
    }))
    .unwrap()
}

fn process(style: &Style) -> CircleBucket {
    let tile = TileProcessor::new()
        .process(style, "s", TileId::new(14, 0, 0), &tile())
        .unwrap();
    assert_eq!(tile.buckets.len(), 1);
    match &tile.buckets[0].geometry {
        BucketGeometry::Circle(c) => c.clone(),
        other => panic!("expected circles, got {other:?}"),
    }
}

fn circle_paint(style: &Style) -> &sparkle::style::CirclePaint {
    match &style.layers[0].kind {
        LayerKind::Circle(c) => &c.paint,
        _ => unreachable!(),
    }
}

fn feature_value(paint: &PaintArrays, props: &PaintProperties<'_>, name: &str, i: usize) -> f32 {
    paint.value(props, name, paint.features[i].vertices.start as usize)[0]
}

#[test]
fn points_and_multipoints_become_quads() {
    let style = style(json!({}), json!({}));
    let bucket = process(&style);
    // Five points inside the tile; the one in the buffer belongs to the neighbor.
    assert_eq!(bucket.vertices.len(), 5 * 4);
    assert_eq!(bucket.triangles.len(), 5 * 6);
    let ids: Vec<_> = bucket.paint.features.iter().map(|f| f.id).collect();
    assert_eq!(ids, [Some(1), Some(2), Some(3), Some(4)]);
    assert_eq!(bucket.paint.features[3].vertices, 12..20);
    // Every paint property is constant, so nothing is stored per vertex.
    assert_eq!(bucket.paint.stride, 0);
}

#[test]
fn data_driven_and_composite_paint() {
    let style = style(
        json!({}),
        json!({
            "circle-radius": ["get", "rank"],
            "circle-stroke-width": ["interpolate", ["linear"], ["zoom"],
                                    14, ["*", ["get", "rank"], 2], 16, 0],
            "circle-color": ["interpolate", ["linear"], ["zoom"], 0, "red", 20, "blue"],
        }),
    );
    let bucket = process(&style);
    let paint = circle_paint(&style);
    let props = paint.attributes();
    let binding = |name| {
        bucket
            .paint
            .attributes
            .iter()
            .find(|a| a.name == name)
            .unwrap()
            .binding
    };
    assert_eq!(binding("radius"), Binding::Source);
    assert_eq!(binding("stroke_width"), Binding::Composite);
    assert_eq!(binding("color"), Binding::Uniform);

    let radii: Vec<_> = (0..4)
        .map(|i| feature_value(&bucket.paint, &props, "radius", i))
        .collect();
    assert_eq!(radii, [3.0, 1.0, 2.0, 5.0]);
    // Composite: rank * 2 at zoom 14, halfway to 0 at zoom 15.
    let at = |v: usize| {
        let a = bucket
            .paint
            .attributes
            .iter()
            .find(|a| a.name == "stroke_width")
            .unwrap();
        let start = v * bucket.paint.stride as usize + a.offset as usize;
        [bucket.paint.data[start], bucket.paint.data[start + 1]]
    };
    assert_eq!(at(0), [6.0, 3.0]);
    assert_eq!(
        at(19),
        [10.0, 5.0],
        "a multipoint's vertices share its values"
    );
}

#[test]
fn sort_key_orders_triangles() {
    let style = style(json!({"circle-sort-key": ["get", "rank"]}), json!({}));
    let bucket = process(&style);
    // Each quad's first triangle starts at its first vertex; features by rank: 2, 3, 1, 4.
    let quads: Vec<_> = bucket.triangles.chunks(6).map(|t| t[0] / 4).collect();
    assert_eq!(quads, [1, 2, 0, 3, 4]);

    let unsorted = process(&self::style(json!({}), json!({})));
    let quads: Vec<_> = unsorted.triangles.chunks(6).map(|t| t[0] / 4).collect();
    assert_eq!(quads, [0, 1, 2, 3, 4]);
}
