//! Heatmap layers end to end: MLT points through a style to a heatmap bucket.

use mlt_core::encoder::{EncoderConfig, Network, Profile};
use mlt_core::geo_types::Coord;
use mlt_core::{GeometryType, LayerWriter, PropKind};
use serde_json::json;
use sparkle::paint::Binding;
use sparkle::style::LayerKind;
use sparkle::{BucketGeometry, Style, TileId, TileProcessor};

/// Points with a `rank`: one plain, one multi-point straddling the tile edge, plus a line
/// that heatmaps ignore.
fn tile() -> Vec<u8> {
    let mut poi = LayerWriter::new("poi", 4096).unwrap();
    let rank = poi.add_property("rank", PropKind::U32).unwrap();
    let mut f = poi.feature(GeometryType::Point);
    f.id(Some(1))
        .points(vec![Coord { x: 100, y: 200 }])
        .unwrap()
        .property(rank, 1u32)
        .unwrap();
    f.finish().unwrap();
    let mut f = poi.feature(GeometryType::MultiPoint);
    f.id(Some(2))
        .points(vec![Coord { x: 300, y: 300 }, Coord { x: -40, y: 300 }])
        .unwrap()
        .property(rank, 4u32)
        .unwrap();
    f.finish().unwrap();
    let mut f = poi.feature(GeometryType::LineString);
    f.id(Some(3))
        .line(vec![Coord { x: 0, y: 0 }, Coord { x: 50, y: 50 }])
        .unwrap()
        .property(rank, 2u32)
        .unwrap();
    f.finish().unwrap();
    poi.encode(EncoderConfig::from_profile(Profile::Native(
        Network::Cellular4G,
    )))
    .unwrap()
}

#[test]
fn heatmap_bucket_from_points() {
    let style = Style::from_json(&json!({
        "version": 8,
        "sources": {"s": {"type": "vector"}},
        "layers": [{
            "id": "heat", "type": "heatmap", "source": "s", "source-layer": "poi",
            "paint": {
                "heatmap-weight": ["/", 1, ["get", "rank"]],
                "heatmap-radius": ["interpolate", ["linear"], ["zoom"], 10, 10, 15, ["get", "rank"]],
                "heatmap-intensity": ["interpolate", ["linear"], ["zoom"], 0, 1, 20, 3],
            }
        }]
    }))
    .unwrap();
    assert!(style.warnings.is_empty(), "{:?}", style.warnings);
    let tile = TileProcessor::new()
        .process(&style, "s", TileId::new(12, 1, 2), &tile())
        .unwrap();
    assert_eq!(tile.buckets.len(), 1);
    let BucketGeometry::Heatmap(heat) = &tile.buckets[0].geometry else {
        panic!("{:?}", tile.buckets[0].geometry)
    };
    let LayerKind::Heatmap(layer) = &style.layers[0].kind else {
        panic!()
    };
    let props = layer.paint.attributes();

    // The buffered point and the line are dropped.
    assert_eq!(heat.vertices.len(), 8);
    assert_eq!(heat.indices.len(), 12);
    let ids: Vec<_> = heat.paint.features.iter().map(|f| f.id).collect();
    assert_eq!(ids, [Some(1), Some(2)]);

    let bindings: Vec<_> = heat.paint.attributes.iter().map(|a| a.binding).collect();
    assert_eq!(bindings, [Binding::Source, Binding::Composite]);
    assert_eq!(heat.paint.value(&props, "weight", 0), [1.0]);
    assert_eq!(heat.paint.value(&props, "weight", 4), [0.25]);
    // Radius at zoom 12 (the tile's) is 40% of the way from 10 to the rank.
    assert_eq!(heat.paint.value(&props, "radius", 4), [10.0 - 0.4 * 6.0]);
}
