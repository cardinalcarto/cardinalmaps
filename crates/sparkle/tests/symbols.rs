//! Symbol layers end to end: MLT points through a style to text and icon quads.

use std::path::Path;
use std::sync::Arc;

use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};
use mlt_core::geo_types::Coord;
use mlt_core::{GeometryType, LayerWriter, PropKind};
use serde_json::{Value, json};
use sparkle::glyphs::{FontLibrary, FontMetrics, GlyphKey, GlyphProvider, SdfGlyph, ShapedGlyph};
use sparkle::paint::Binding;
use sparkle::sprite::{ImageAtlas, SpriteSheet};
use sparkle::style::LayerKind;
use sparkle::tile::{SymbolBucket, symbol_flags};
use sparkle::{BucketGeometry, Style, TileId, TileProcessor, TileResources};

/// Every character but the space is a 10 by 12 pixel box, 10 pixels apart.
struct Boxes;

impl GlyphProvider for Boxes {
    fn shape(&self, _: &str, text: &str, rtl: bool, out: &mut Vec<ShapedGlyph>) {
        let start = out.len();
        for (i, c) in text.char_indices() {
            out.push(ShapedGlyph {
                key: GlyphKey {
                    font: 0,
                    glyph: c as u16,
                },
                cluster: i as u32,
                advance: 10.0,
                offset: [0.0; 2],
            });
        }
        if rtl {
            out[start..].reverse();
        }
    }

    fn glyph(&self, key: GlyphKey) -> Option<Arc<SdfGlyph>> {
        (key.glyph != u16::from(b' ')).then(|| {
            Arc::new(SdfGlyph {
                width: 10,
                height: 12,
                left: -3.0,
                top: 15.0,
                pixels: vec![key.glyph as u8; 120],
            })
        })
    }

    fn metrics(&self, _: &str) -> FontMetrics {
        FontMetrics::default()
    }
}

/// A layer of named points: `(x, y, name, rank)`.
fn points(features: &[(i32, i32, &str, u32)]) -> Vec<u8> {
    let mut layer = LayerWriter::new("poi", 4096).unwrap();
    let name = layer.add_property("name", PropKind::Str).unwrap();
    let rank = layer.add_property("rank", PropKind::U32).unwrap();
    for (i, &(x, y, n, r)) in features.iter().enumerate() {
        let mut f = layer.feature(GeometryType::Point);
        f.id(Some(i as u64 + 1))
            .points(vec![Coord { x, y }])
            .unwrap()
            .property(name, n)
            .unwrap()
            .property(rank, r)
            .unwrap();
        f.finish().unwrap();
    }
    let cfg = EncoderConfig::from_profile(Profile::Native(Network::Cellular4G))
        .with_tessellation(Tessellation::Off);
    layer.encode(cfg).unwrap()
}

fn style(layout: Value, paint: Value) -> Style {
    Style::from_json(&json!({
        "version": 8,
        "sources": {"s": {"type": "vector"}},
        "layers": [{"id": "labels", "type": "symbol", "source": "s", "source-layer": "poi",
                    "layout": layout, "paint": paint}]
    }))
    .unwrap()
}

fn atlas() -> ImageAtlas {
    let prefix = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sprite/sprite");
    let sheet = SpriteSheet::load(&prefix, 1.0).unwrap();
    ImageAtlas::build([("default", &sheet)])
}

fn build(style: &Style, tile: &[u8], resources: TileResources<'_>) -> Option<SymbolBucket> {
    let tile = TileProcessor::new()
        .process_with(style, "s", TileId::new(14, 0, 0), tile, resources)
        .unwrap();
    tile.buckets.into_iter().find_map(|b| match b.geometry {
        BucketGeometry::Symbol(s) => Some(s),
        _ => None,
    })
}

fn glyphs() -> TileResources<'static> {
    TileResources {
        images: None,
        glyphs: Some(&Boxes),
    }
}

#[test]
fn text_labels_at_points() {
    let style = style(
        json!({"text-field": ["get", "name"], "text-anchor": "left",
               "text-size": ["interpolate", ["linear"], ["zoom"], 10, ["get", "rank"], 20, 30]}),
        json!({"text-color": "red", "text-halo-width": 2}),
    );
    let tile = points(&[
        (100, 200, "ab c", 10),
        (5000, 200, "outside", 10),
        (300, 300, "", 10),
        (400, 400, "x", 20),
    ]);
    let bucket = build(&style, &tile, glyphs()).unwrap();
    // The point outside the tile and the one without text get no label.
    assert_eq!(bucket.labels.instances.len(), 2);
    let first = &bucket.labels.instances[0];
    assert_eq!(first.anchor, [100.0, 200.0]);
    assert_eq!(first.feature_id, Some(1));
    // Three glyphs, the space has none.
    assert_eq!(first.text_vertices, 0..12);
    assert_eq!(bucket.text.vertices.len(), 16);
    assert_eq!(bucket.text.triangles.len(), 24);
    let text_box = first.text_box.unwrap();
    assert_eq!(text_box[0], 0.0, "anchored on the left");
    assert_eq!(text_box[2], 40.0);

    // Glyph quads: the "a" starts at the pen, less its left bearing.
    let v = &bucket.text.vertices;
    assert!(v.iter().all(|v| v.flags == symbol_flags::SDF));
    assert_eq!(v[0].offset[0], -3.0);
    assert_eq!(v[1].offset[0] - v[0].offset[0], 10.0);
    assert_eq!(v[8].offset[0], 27.0, "the space advances the pen");
    // Each glyph is in the atlas once, where the quad's texture coordinates say.
    let atlas = &bucket.glyphs;
    for quad in v.chunks(4) {
        let [x, y] = quad[0].tex.map(|t| t as u32);
        assert_eq!(quad[3].tex, [x as f32 + 10.0, y as f32 + 12.0]);
        assert!(x + 10 <= atlas.width && y + 12 <= atlas.height);
        let value = atlas.pixels[(y * atlas.width + x) as usize];
        assert!(b"abcx".contains(&value));
    }

    // The size is per feature and zoom: a composite paint attribute.
    let LayerKind::Symbol(layer) = &style.layers[0].kind else {
        unreachable!()
    };
    let props = layer.text_attributes();
    let size = bucket
        .text
        .paint
        .attributes
        .iter()
        .find(|a| a.name == "size")
        .unwrap();
    assert_eq!(size.binding, Binding::Composite);
    // At zoom 14, rank 10 interpolates to 18 and rank 20 to 24.
    assert_eq!(bucket.text.paint.value(&props, "size", 0), [18.0]);
    assert_eq!(bucket.text.paint.value(&props, "size", 12), [24.0]);
    assert_eq!(bucket.text.paint.value(&props, "halo_width", 0), [2.0]);
    assert_eq!(first.layout_text_size, 20.0, "at zoom 15");
}

#[test]
fn without_glyphs_text_is_skipped() {
    let style = style(json!({"text-field": "{name}"}), json!({}));
    let tile = points(&[(100, 100, "a", 1)]);
    assert!(build(&style, &tile, TileResources::default()).is_none());
}

#[test]
fn format_sections_and_transform() {
    let style = style(
        json!({"text-field": ["format", ["get", "name"], {},
                              "b", {"font-scale": 2, "text-color": "blue"}],
               "text-transform": "uppercase", "text-anchor": "top-left",
               "text-offset": [1, 2], "text-rotate": 90}),
        json!({}),
    );
    let tile = points(&[(10, 10, "a", 1)]);
    let bucket = build(&style, &tile, glyphs()).unwrap();
    let v = &bucket.text.vertices;
    assert_eq!(v.len(), 8);
    // Upper case, and the second section's color travels with its vertices.
    let atlas = &bucket.glyphs;
    let [x, y] = v[0].tex.map(|t| t as u32);
    assert_eq!(atlas.pixels[(y * atlas.width + x) as usize], b'A');
    assert_eq!(v[0].flags & symbol_flags::COLOR, 0);
    assert_ne!(v[4].flags & symbol_flags::COLOR, 0);
    assert_eq!(v[4].color, [0, 0, 255, 255]);
    // The scaled glyph is twice as large.
    let size = |q: &[sparkle::tile::SymbolVertex]| {
        let d = [
            q[3].offset[0] - q[0].offset[0],
            q[3].offset[1] - q[0].offset[1],
        ];
        (d[0] * d[0] + d[1] * d[1]).sqrt()
    };
    assert!((size(&v[4..8]) - 2.0 * size(&v[0..4])).abs() < 1e-3);
    // Offset by one and two ems, then turned a quarter clockwise: the text runs down.
    let text_box = bucket.labels.instances[0].text_box.unwrap();
    assert_eq!(&text_box[..2], [24.0, 48.0]);
    assert!(v[1].offset[1] > v[0].offset[1]);
    assert!((v[0].offset[0] - v[1].offset[0]).abs() < 1e-3);
}

#[test]
fn variable_anchors_use_the_first() {
    let style = style(
        json!({"text-field": "{name}", "text-variable-anchor": ["right", "left"],
               "text-radial-offset": 1}),
        json!({}),
    );
    let tile = points(&[(10, 10, "ab", 1)]);
    let bucket = build(&style, &tile, glyphs()).unwrap();
    let instance = &bucket.labels.instances[0];
    assert_eq!(instance.variable_anchors.len(), 2);
    assert_eq!(instance.variable_anchors[0].1, [-24.0, 0.0]);
    // Anchored right, one em to the left of the point.
    assert_eq!(instance.text_box.unwrap()[2], -24.0);
}

#[test]
fn icons_from_the_sprite() {
    let atlas = atlas();
    let resources = TileResources {
        images: Some(&atlas),
        glyphs: Some(&Boxes),
    };
    let style = style(
        json!({"icon-image": ["coalesce", ["image", ["get", "name"]], ["image", "square"]],
               "icon-anchor": "bottom", "icon-offset": [0, -1]}),
        json!({"icon-color": "green"}),
    );
    let tile = points(&[(10, 10, "dot", 1), (20, 20, "missing", 1)]);
    let bucket = build(&style, &tile, resources).unwrap();
    assert_eq!(bucket.labels.instances.len(), 2);
    let icons = &bucket.icons.vertices;
    assert_eq!(icons.len(), 8);
    assert_eq!(
        icons[0].flags,
        symbol_flags::SDF,
        "the dot is a distance field"
    );
    assert_eq!(icons[4].flags, 0, "the square is not");
    // The 4 pixel square sits on its anchor, one pixel up, with its border around it.
    let square = &icons[4..8];
    assert_eq!(square[0].offset, [-3.0, -6.0]);
    assert_eq!(square[3].offset, [3.0, 0.0]);
    let image = atlas.get("square").unwrap();
    assert_eq!(
        square[0].tex,
        [image.rect[0] as f32 - 1.0, image.rect[1] as f32 - 1.0]
    );
    assert!(bucket.text.vertices.is_empty());
}

#[test]
fn icons_fitted_to_text() {
    let atlas = atlas();
    let resources = TileResources {
        images: Some(&atlas),
        glyphs: Some(&Boxes),
    };
    let style = style(
        json!({"icon-image": "bar", "icon-text-fit": "both", "text-field": "{name}",
               "icon-text-fit-padding": [2, 4, 2, 4], "text-size": 24}),
        json!({}),
    );
    let tile = points(&[(10, 10, "abcd", 1)]);
    let bucket = build(&style, &tile, resources).unwrap();
    let instance = &bucket.labels.instances[0];
    let text = instance.text_box.unwrap();
    let icon = instance.icon_box.unwrap();
    // Layout size is text-size at zoom 15, 24 pixels, so the padding is in text units.
    assert_eq!(
        icon,
        [text[0] - 4.0, text[1] - 2.0, text[2] + 4.0, text[3] + 2.0]
    );
    let v = &bucket.icons.vertices;
    assert!(v.len() > 4, "stretched in parts");
    assert!(v.iter().all(|v| v.flags & symbol_flags::TEXT_SIZED != 0));
}

#[test]
fn sort_key_orders_labels() {
    let style = style(
        json!({"text-field": "{name}", "symbol-sort-key": ["-", ["get", "rank"]]}),
        json!({"text-color": ["match", ["get", "name"], "low", "red", "blue"]}),
    );
    let tile = points(&[(10, 10, "low", 1), (20, 20, "high", 5)]);
    let bucket = build(&style, &tile, glyphs()).unwrap();
    let names: Vec<_> = bucket
        .labels
        .instances
        .iter()
        .map(|i| i.feature_id)
        .collect();
    assert_eq!(names, [Some(2), Some(1)]);
    assert_eq!(bucket.labels.instances[0].text_vertices, 0..16);
    assert_eq!(bucket.labels.instances[1].text_vertices, 16..28);
    let LayerKind::Symbol(layer) = &style.layers[0].kind else {
        unreachable!()
    };
    let props = layer.text_attributes();
    // The colors moved with their labels.
    assert_eq!(
        bucket.text.paint.value(&props, "color", 0),
        [0.0, 0.0, 1.0, 1.0]
    );
    assert_eq!(
        bucket.text.paint.value(&props, "color", 20),
        [1.0, 0.0, 0.0, 1.0]
    );
    assert_eq!(bucket.text.paint.features[0].id, Some(2));
    assert_eq!(bucket.text.triangles[24..30], [16, 17, 18, 17, 19, 18]);
}

#[test]
fn rarer_icons_place_first() {
    let atlas = atlas();
    let resources = TileResources {
        images: Some(&atlas),
        glyphs: Some(&Boxes),
    };
    let tile = points(&[
        (10, 10, "dot", 2),
        (20, 20, "dot", 1),
        (30, 30, "square", 1),
        (40, 40, "dot", 1),
    ]);
    let ids = |layout: Value| {
        let bucket = build(&style(layout, json!({})), &tile, resources).unwrap();
        bucket
            .labels
            .instances
            .iter()
            .map(|i| i.feature_id.unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(json!({"icon-image": "{name}"})),
        [1, 2, 3, 4],
        "off by default"
    );
    assert_eq!(
        ids(json!({"icon-image": "{name}", "cardinal-sort-by-rarity": true})),
        [3, 1, 2, 4]
    );
    // The sort key comes first.
    assert_eq!(
        ids(
            json!({"icon-image": "{name}", "cardinal-sort-by-rarity": true,
                   "symbol-sort-key": ["get", "rank"]})
        ),
        [3, 2, 4, 1]
    );
}

#[test]
fn local_fonts_end_to_end() {
    let dir = Path::new("/usr/share/fonts/google-noto-vf");
    if !dir.is_dir() {
        eprintln!("skipped: no Noto fonts");
        return;
    }
    let fonts = FontLibrary::only([dir.to_owned()]);
    let resources = TileResources {
        images: None,
        glyphs: Some(&fonts),
    };
    let style = style(
        json!({"text-field": "{name}\n{rank}", "text-font": ["Noto Sans Bold"],
               "text-max-width": 3}),
        json!({}),
    );
    let tile = points(&[(10, 10, "Lake Union Park", 7)]);
    let bucket = build(&style, &tile, resources).unwrap();
    let text_box = bucket.labels.instances[0].text_box.unwrap();
    // Three words and the rank on four lines, each 1.2 ems.
    let height = text_box[3] - text_box[1];
    assert!((height - 4.0 * 1.2 * 24.0).abs() < 1e-3, "{height}");
    // "LakeUnionPark7" has 14 glyphs.
    assert_eq!(bucket.text.vertices.len(), 14 * 4);
    assert!(bucket.glyphs.pixels.iter().any(|&v| v > 200));
}
