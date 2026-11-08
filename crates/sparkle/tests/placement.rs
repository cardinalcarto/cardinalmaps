//! Symbol placement end to end: buckets from MLT tiles, placed for a camera.

use std::f32::consts::PI;
use std::sync::Arc;

use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};
use mlt_core::geo_types::Coord;
use mlt_core::{GeometryType, LayerWriter, PropKind};
use serde_json::{Value, json};
use sparkle::glyphs::{FontMetrics, GlyphKey, GlyphProvider, SdfGlyph, ShapedGlyph};
use sparkle::projection::TileProjection;
use sparkle::style::{LayerKind, SymbolLayer};
use sparkle::symbol::placement::{BucketKey, PlacementBucket, PlacementView, SymbolPlacer};
use sparkle::tile::SymbolBucket;
use sparkle::{BucketGeometry, Style, TileId, TileProcessor, TileResources};

/// Every character is a 10 by 12 pixel box, 10 pixels apart.
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
        Some(Arc::new(SdfGlyph {
            width: 10,
            height: 12,
            left: -3.0,
            top: 15.0,
            pixels: vec![key.glyph as u8; 120],
        }))
    }

    fn metrics(&self, _: &str) -> FontMetrics {
        FontMetrics::default()
    }
}

const EXTENT: f32 = 4096.0;
/// The viewport, in logical pixels: one 512 pixel tile.
const VIEWPORT: f32 = 512.0;

fn encode(layer: LayerWriter<'_>) -> Vec<u8> {
    let cfg = EncoderConfig::from_profile(Profile::Native(Network::Cellular4G))
        .with_tessellation(Tessellation::Off);
    layer.encode(cfg).unwrap()
}

/// A layer of named points: `(x, y, name)`.
fn points(features: &[(i32, i32, &str)]) -> Vec<u8> {
    let mut layer = LayerWriter::new("labels", 4096).unwrap();
    let name = layer.add_property("name", PropKind::Str).unwrap();
    for (i, &(x, y, n)) in features.iter().enumerate() {
        let mut f = layer.feature(GeometryType::Point);
        f.id(Some(i as u64 + 1))
            .points(vec![Coord { x, y }])
            .unwrap()
            .property(name, n)
            .unwrap();
        f.finish().unwrap();
    }
    encode(layer)
}

/// A layer of one named line.
fn line(points: &[(i32, i32)], n: &str) -> Vec<u8> {
    let mut layer = LayerWriter::new("labels", 4096).unwrap();
    let name = layer.add_property("name", PropKind::Str).unwrap();
    let mut f = layer.feature(GeometryType::LineString);
    f.id(Some(1))
        .line(
            points
                .iter()
                .map(|&(x, y)| Coord { x, y })
                .collect::<Vec<_>>(),
        )
        .unwrap()
        .property(name, n)
        .unwrap();
    f.finish().unwrap();
    encode(layer)
}

fn style(layout: Value) -> Style {
    let mut layout = layout;
    layout["text-field"] = json!("{name}");
    Style::from_json(&json!({
        "version": 8,
        "sources": {"s": {"type": "vector"}},
        "layers": [{"id": "labels", "type": "symbol", "source": "s", "source-layer": "labels",
                    "layout": layout}]
    }))
    .unwrap()
}

fn symbol_layer(style: &Style) -> &SymbolLayer {
    let LayerKind::Symbol(layer) = &style.layers[0].kind else {
        unreachable!()
    };
    layer
}

fn try_build(style: &Style, z: u8, tile: &[u8]) -> Option<SymbolBucket> {
    let resources = TileResources {
        images: None,
        glyphs: Some(&Boxes),
    };
    let tile = TileProcessor::new()
        .process_with(style, "s", TileId::new(z, 0, 0), tile, resources)
        .unwrap();
    tile.buckets.into_iter().find_map(|b| match b.geometry {
        BucketGeometry::Symbol(s) => Some(s),
        _ => None,
    })
}

fn build(style: &Style, z: u8, tile: &[u8]) -> SymbolBucket {
    try_build(style, z, tile).expect("a symbol bucket")
}

/// Looking straight down at the top left corner of the world, with tiles of zoom `z`
/// spanning `512 * 2^(z_view - z)` pixels, the view's tile filling the viewport. With
/// `upside_down`, the map is turned by 180 degrees.
fn matrix(z: u8, z_view: u8, upside_down: bool) -> [[f32; 4]; 4] {
    let pixels = VIEWPORT * f32::from(z_view - z).exp2();
    let s = pixels / EXTENT * 2.0 / VIEWPORT;
    let sign = if upside_down { -1.0 } else { 1.0 };
    [
        [s * sign, 0.0, 0.0, 0.0],
        [0.0, -s * sign, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [-sign, sign, 0.0, 1.0],
    ]
}

fn view(bearing: f32) -> PlacementView {
    PlacementView {
        viewport: [VIEWPORT; 2],
        zoom: 14.0,
        bearing,
        // The clip w of every point in these flat views, so labels keep their size.
        camera_to_center: 1.0,
    }
}

fn key(z: u8) -> BucketKey {
    BucketKey {
        layer: 0,
        z,
        x: 0,
        y: 0,
    }
}

fn input<'a>(
    style: &'a Style,
    bucket: &'a SymbolBucket,
    z: u8,
    upside_down: bool,
) -> PlacementBucket<'a> {
    let pixels = VIEWPORT * f32::from(14 - z).exp2();
    PlacementBucket {
        key: key(z),
        layer: symbol_layer(style),
        labels: &bucket.labels,
        projection: TileProjection::flat(matrix(z, 14, upside_down)),
        units_per_pixel: EXTENT / pixels,
    }
}

/// The opacity of each label's text.
fn opacities(placer: &SymbolPlacer, bucket: &SymbolBucket, z: u8) -> Vec<f32> {
    let placed = placer.bucket(&key(z)).unwrap();
    bucket
        .labels
        .instances
        .iter()
        .map(|i| placed.text[i.text_vertices.start as usize][3])
        .collect()
}

/// The labels, by index, in the order `triangles` draws them.
fn drawn(triangles: &[u32], bucket: &SymbolBucket) -> Vec<usize> {
    let mut order = Vec::new();
    for vertex in triangles {
        let label = bucket
            .labels
            .instances
            .iter()
            .position(|i| i.text_vertices.contains(vertex))
            .unwrap();
        if order.last() != Some(&label) {
            order.push(label);
        }
    }
    order
}

/// Which label won placement: the first of the bucket's that's still showing.
fn placed_first_on_top(style: &Style, tile: &[u8]) -> Vec<usize> {
    let bucket = build(style, 14, tile);
    let mut placer = SymbolPlacer::new(0.0);
    placer.update(0.0, &view(0.0), &[input(style, &bucket, 14, false)]);
    drawn(
        placer
            .bucket(&key(14))
            .unwrap()
            .text_triangles
            .as_ref()
            .unwrap(),
        &bucket,
    )
}

#[test]
fn what_wins_placement_draws_on_top() {
    // Whichever is lower on the screen: it's placement that counts.
    let tile = points(&[(2000, 2000, "a"), (2000, 2400, "b"), (2000, 1600, "c")]);
    let style = style(json!({"text-size": 24, "text-allow-overlap": true}));
    assert_eq!(placed_first_on_top(&style, &tile), [2, 1, 0]);
}

#[test]
fn a_sort_key_decides_both_placement_and_what_is_on_top() {
    let tile = points(&[(2000, 2000, "bb"), (2040, 2010, "a"), (2020, 2005, "ccc")]);
    let keyed = style(json!({
        "text-size": 24,
        "text-allow-overlap": true,
        "symbol-sort-key": ["length", ["get", "name"]],
    }));
    let bucket = build(&keyed, 14, &tile);
    // Placed by key: lowest first.
    let names: Vec<_> = bucket.labels.instances.iter().map(|i| i.sort_key).collect();
    assert_eq!(names, [Some(1.0), Some(2.0), Some(3.0)]);
    // So the lowest, placed first, is drawn last.
    assert_eq!(placed_first_on_top(&keyed, &tile), [2, 1, 0]);
}

#[test]
fn viewport_y_still_draws_lowest_on_top() {
    let tile = points(&[(2000, 2000, "a"), (2000, 2400, "b"), (2000, 1600, "c")]);
    let style = style(json!({
        "text-size": 24,
        "text-allow-overlap": true,
        "symbol-z-order": "viewport-y",
    }));
    let bucket = build(&style, 14, &tile);
    let mut placer = SymbolPlacer::new(0.0);
    placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);
    let order = drawn(
        placer
            .bucket(&key(14))
            .unwrap()
            .text_triangles
            .as_ref()
            .unwrap(),
        &bucket,
    );
    let ys: Vec<_> = order
        .iter()
        .map(|&i| bucket.labels.instances[i].anchor[1])
        .collect();
    assert!(ys.is_sorted());
}

#[test]
fn labels_that_cannot_overlap_keep_the_buckets_order() {
    let tile = points(&[(2000, 2000, "a"), (3000, 3000, "b")]);
    let style = style(json!({"text-size": 24}));
    let bucket = build(&style, 14, &tile);
    let mut placer = SymbolPlacer::new(0.0);
    placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);
    assert!(placer.bucket(&key(14)).unwrap().text_triangles.is_none());
}

#[test]
fn overlapping_labels_collide() {
    let tile = points(&[(2000, 2000, "abc"), (2040, 2010, "abd"), (3000, 3000, "x")]);
    let style = style(json!({"text-size": 24}));
    let bucket = build(&style, 14, &tile);
    let mut placer = SymbolPlacer::new(0.0);
    let animating = placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);
    assert!(!animating);
    assert_eq!(opacities(&placer, &bucket, 14), [1.0, 0.0, 1.0]);

    let style = self::style(json!({"text-size": 24, "text-allow-overlap": true}));
    let bucket = build(&style, 14, &tile);
    let mut placer = SymbolPlacer::new(0.0);
    placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);
    assert_eq!(opacities(&placer, &bucket, 14), [1.0, 1.0, 1.0]);
}

#[test]
fn labels_are_found_where_they_show() {
    // The second collides with the first, so it isn't shown.
    let tile = points(&[(2000, 2000, "abc"), (2040, 2010, "abd"), (3000, 3000, "x")]);
    let style = style(json!({"text-size": 24}));
    let bucket = build(&style, 14, &tile);
    let mut placer = SymbolPlacer::new(0.0);
    placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);

    // The tile fills the view.
    let shown = [2000.0, 3000.0].map(|c: f32| [c / EXTENT * VIEWPORT; 2]);
    let ids = |p: [f32; 2], tolerance| -> Vec<Option<u64>> {
        placer
            .labels_at(p, tolerance)
            .iter()
            .map(|l| l.feature_id)
            .collect()
    };
    assert_eq!(ids(shown[0], 0.0), [Some(1)]);
    assert_eq!(ids(shown[1], 0.0), [Some(3)]);
    // Only the tolerance reaches past what a label covers, nearest first.
    let beyond = [shown[1][0] + 100.0, shown[1][1]];
    assert!(ids(beyond, 0.0).is_empty());
    assert_eq!(ids(beyond, 200.0), [Some(3)]);
    // Its anchor, as a place in the world.
    let label = placer.labels_at(shown[1], 0.0)[0];
    let scale = 14f64.exp2();
    assert!((label.world[0] * scale - label.key.x as f64 - 3000.0 / 4096.0).abs() < 1e-6);
}

#[test]
fn rarer_icons_win_collisions() {
    let prefix =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sprite/sprite");
    let sheet = sparkle::sprite::SpriteSheet::load(&prefix, 1.0).unwrap();
    let atlas = sparkle::sprite::ImageAtlas::build([("default", &sheet)]);
    // A common dot first, overlapping the only square.
    let tile = points(&[
        (2000, 2000, "dot"),
        (2040, 2010, "square"),
        (3000, 3000, "dot"),
        (1000, 1000, "dot"),
    ]);
    let visible = |layout: Value| {
        let style = style(layout);
        let resources = TileResources {
            images: Some(&atlas),
            glyphs: Some(&Boxes),
        };
        let tile = TileProcessor::new()
            .process_with(&style, "s", TileId::new(14, 0, 0), &tile, resources)
            .unwrap();
        let Some(BucketGeometry::Symbol(bucket)) =
            tile.buckets.into_iter().next().map(|b| b.geometry)
        else {
            unreachable!()
        };
        let mut placer = SymbolPlacer::new(0.0);
        placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);
        let mut shown: Vec<u64> = bucket
            .labels
            .instances
            .iter()
            .zip(opacities(&placer, &bucket, 14))
            .filter(|&(_, o)| o == 1.0)
            .map(|(i, _)| i.feature_id.unwrap())
            .collect();
        shown.sort();
        shown
    };
    let layout = json!({"text-size": 24, "icon-image": "{name}"});
    assert_eq!(visible(layout.clone()), [1, 3, 4], "the first one wins");
    let mut layout = layout;
    layout["cardinal-sort-by-rarity"] = json!(true);
    assert_eq!(visible(layout), [2, 3, 4], "the square wins");
}

#[test]
fn padding_and_ignore_placement() {
    // 8 tile units are a pixel: these labels are 4 pixels apart vertically.
    let tile = points(&[(2000, 2000, "ab"), (2000, 2000 + 8 * 30, "ab")]);
    let place = |layout: Value| {
        let style = style(layout);
        let bucket = build(&style, 14, &tile);
        let mut placer = SymbolPlacer::new(0.0);
        placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);
        opacities(&placer, &bucket, 14)
    };
    assert_eq!(
        place(json!({"text-size": 24, "text-padding": 0})),
        [1.0, 1.0]
    );
    assert_eq!(
        place(json!({"text-size": 24, "text-padding": 4})),
        [1.0, 0.0]
    );
    assert_eq!(
        place(json!({"text-size": 24, "text-padding": 4, "text-ignore-placement": true})),
        [1.0, 1.0]
    );
}

#[test]
fn variable_anchors_find_room() {
    // The second label's point is just below the first.
    let tile = points(&[(2048, 2048, "aaaa"), (2048, 2048 + 8 * 30, "bbbb")]);
    let style = style(json!({"text-size": 24, "text-variable-anchor": ["center", "top"]}));
    let bucket = build(&style, 14, &tile);
    let mut placer = SymbolPlacer::new(0.0);
    placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);
    assert_eq!(opacities(&placer, &bucket, 14), [1.0, 1.0]);
    let placed = placer.bucket(&key(14)).unwrap();
    let shift = |i: usize| {
        let v = bucket.labels.instances[i].text_vertices.start as usize;
        [placed.text[v][0], placed.text[v][1]]
    };
    assert_eq!(shift(0), [0.0, 0.0]);
    // Moved down to sit below its point.
    let [x, y] = shift(1);
    assert_eq!(x, 0.0);
    // Half the line height (28.8), less the baseline offset of 7 that `top` adds.
    assert!((y - 7.4).abs() < 1e-4, "{y}");
}

#[test]
fn labels_are_matched_across_zoom_levels() {
    let style = style(json!({"text-size": 24}));
    // The same label, at the same place, in a tile and its parent.
    let child = build(&style, 14, &points(&[(2000, 2000, "abc")]));
    let parent = build(
        &style,
        13,
        &points(&[(1000, 1000, "abc"), (3000, 3000, "far")]),
    );
    let mut placer = SymbolPlacer::new(0.3);
    placer.update(0.0, &view(0.0), &[input(&style, &parent, 13, false)]);
    // Faded in by now.
    placer.update(1.0, &view(0.0), &[input(&style, &parent, 13, false)]);
    assert_eq!(opacities(&placer, &parent, 13)[0], 1.0);

    // The child arrives: its label is the parent's, so it shows at once, and the
    // parent's copy is hidden.
    let both = [
        input(&style, &parent, 13, false),
        input(&style, &child, 14, false),
    ];
    placer.update(1.01, &view(0.0), &both);
    assert_eq!(opacities(&placer, &child, 14), [1.0]);
    assert_eq!(opacities(&placer, &parent, 13)[0], 0.0);

    // And stays when the parent goes.
    placer.update(1.02, &view(0.0), &[input(&style, &child, 14, false)]);
    assert_eq!(opacities(&placer, &child, 14), [1.0]);
}

#[test]
fn labels_survive_their_tile_being_rebuilt() {
    let style = style(json!({"text-size": 24}));
    let tile = points(&[(2000, 2000, "abc"), (3000, 3000, "far")]);
    let before = build(&style, 14, &tile);
    let mut placer = SymbolPlacer::new(0.3);
    placer.update(0.0, &view(0.0), &[input(&style, &before, 14, false)]);
    placer.update(1.0, &view(0.0), &[input(&style, &before, 14, false)]);
    assert_eq!(opacities(&placer, &before, 14), [1.0, 1.0]);

    // The same tile built again (as when the view crosses a zoom past the source's
    // maxzoom), with a label gone: the others stay shown.
    let after = build(&style, 14, &points(&[(2000, 2000, "abc")]));
    placer.update(1.01, &view(0.0), &[input(&style, &after, 14, false)]);
    assert_eq!(opacities(&placer, &after, 14), [1.0]);
}

#[test]
fn labels_fade_and_placement_is_throttled() {
    let style = style(json!({"text-size": 24}));
    let bucket = build(&style, 14, &points(&[(2000, 2000, "abc")]));
    let mut placer = SymbolPlacer::new(0.3);
    let here = input(&style, &bucket, 14, false);
    assert!(placer.update(0.0, &view(0.0), std::slice::from_ref(&here)));
    assert_eq!(opacities(&placer, &bucket, 14), [0.0]);
    assert!(placer.update(0.15, &view(0.0), std::slice::from_ref(&here)));
    assert!((opacities(&placer, &bucket, 14)[0] - 0.5).abs() < 1e-5);

    // Moved so the label is far off screen; placement doesn't run again right away.
    let mut away = input(&style, &bucket, 14, false);
    away.projection.matrix[3][0] += 10.0;
    assert!(placer.update(0.2, &view(0.0), std::slice::from_ref(&away)));
    let o = opacities(&placer, &bucket, 14)[0];
    assert!(o > 0.6, "still fading in: {o}");
    // Once it does, the label fades out.
    placer.update(0.35, &view(0.0), std::slice::from_ref(&away));
    let p = opacities(&placer, &bucket, 14)[0];
    assert!(p < o, "{p} {o}");
    placer.update(1.0, &view(0.0), std::slice::from_ref(&away));
    assert!(!placer.update(1.1, &view(0.0), std::slice::from_ref(&away)));
    assert_eq!(opacities(&placer, &bucket, 14), [0.0]);
}

/// The glyph positions (tile units) and angles of a line label.
fn glyphs(placer: &SymbolPlacer, bucket: &SymbolBucket, label: usize) -> Vec<[f32; 3]> {
    let placed = placer.bucket(&key(14)).unwrap();
    let r = &bucket.labels.instances[label].text_vertices;
    placed.text[r.start as usize..r.end as usize]
        .chunks(4)
        .map(|q| {
            assert!(
                q.iter().all(|v| v == &q[0]),
                "a quad's corners share a position"
            );
            assert!(q[0][3] > 0.0, "visible");
            [q[0][0], q[0][1], q[0][2]]
        })
        .collect()
}

#[test]
fn text_follows_lines() {
    let style = style(json!({"symbol-placement": "line", "text-size": 24}));
    let bucket = build(&style, 14, &line(&[(0, 2048), (4096, 2048)], "abc"));
    let labels = &bucket.labels;
    assert!(labels.text_along_line);
    // Every 250 pixels, starting half that in as the line comes from the tile's edge.
    let xs: Vec<f32> = labels.instances.iter().map(|i| i.anchor[0]).collect();
    assert_eq!(xs, [1000.0, 3000.0]);
    assert_eq!(labels.glyph_offsets.len(), 6);
    assert_eq!(labels.glyph_offsets[..3], [-13.0, -3.0, 7.0]);

    let mut placer = SymbolPlacer::new(0.0);
    placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);
    let g = glyphs(&placer, &bucket, 0);
    // In tile units: 8 per pixel, and pixels are text units at size 24.
    let x: Vec<f32> = g.iter().map(|g| g[0]).collect();
    assert_eq!(
        x,
        [1000.0 - 13.0 * 8.0, 1000.0 - 3.0 * 8.0, 1000.0 + 7.0 * 8.0]
    );
    assert!(
        g.iter()
            .all(|g| g[1] == 2048.0 && g[2].rem_euclid(2.0 * PI) < 1e-4)
    );

    // Upside down, the text is flipped to read left to right on screen: right to left on
    // the map, turned by half a turn.
    placer.update(1.0, &view(PI), &[input(&style, &bucket, 14, true)]);
    let g = glyphs(&placer, &bucket, 0);
    assert!(g[0][0] > g[1][0] && g[1][0] > g[2][0], "{g:?}");
    assert!(
        g.iter()
            .all(|g| (g[2].rem_euclid(2.0 * PI) - PI).abs() < 1e-4),
        "{g:?}"
    );

    // Without keep-upright it reads upside down.
    let style = self::style(json!({"symbol-placement": "line", "text-keep-upright": false}));
    let bucket = build(&style, 14, &line(&[(0, 2048), (4096, 2048)], "abc"));
    let mut placer = SymbolPlacer::new(0.0);
    placer.update(0.0, &view(PI), &[input(&style, &bucket, 14, true)]);
    let g = glyphs(&placer, &bucket, 0);
    assert!(g[0][0] < g[2][0], "{g:?}");
}

#[test]
fn magnified_tiles_space_line_labels_in_view_pixels() {
    let style = style(json!({"symbol-placement": "line", "text-size": 24}));
    let resources = TileResources {
        images: None,
        glyphs: Some(&Boxes),
    };
    let tile = TileProcessor::new()
        .process_at(
            &style,
            "s",
            TileId::new(14, 0, 0),
            16,
            &line(&[(0, 2048), (4096, 2048)], "abc"),
            resources,
        )
        .unwrap();
    let Some(BucketGeometry::Symbol(bucket)) = tile.buckets.into_iter().next().map(|b| b.geometry)
    else {
        unreachable!()
    };
    assert_eq!(bucket.labels.zoom, 16.0);
    // Drawn 4 times as big as at 14, 250 pixels are 500 tile units rather than 2000.
    let xs: Vec<f32> = bucket
        .labels
        .instances
        .iter()
        .map(|i| i.anchor[0])
        .collect();
    assert_eq!(
        xs,
        [250.0, 750.0, 1250.0, 1750.0, 2250.0, 2750.0, 3250.0, 3750.0]
    );
}

#[test]
fn line_labels_avoid_sharp_corners_and_collide() {
    // A zigzag with right angle corners every 56 pixels: too sharp for the default
    // text-max-angle of 45 degrees under an 80 pixel label.
    let zigzag: Vec<(i32, i32)> = (0..12)
        .map(|i| (300 + i * 320, 2048 + (i % 2) * 320))
        .collect();
    let tile = line(&zigzag, "abcdefgh");
    let style = self::style(json!({"symbol-placement": "line", "text-size": 24}));
    assert!(try_build(&style, 14, &tile).is_none());
    let style = self::style(json!({"symbol-placement": "line", "text-size": 24,
                                   "text-max-angle": 100}));
    assert!(!build(&style, 14, &tile).labels.instances.is_empty());
    let style = self::style(json!({"symbol-placement": "line", "text-size": 24}));

    // Two parallel lines 5 pixels apart: their labels collide.
    let mut layer = LayerWriter::new("labels", 4096).unwrap();
    let name = layer.add_property("name", PropKind::Str).unwrap();
    for (i, y) in [2048, 2088].into_iter().enumerate() {
        let mut f = layer.feature(GeometryType::LineString);
        f.id(Some(i as u64 + 1))
            .line(vec![Coord { x: 0, y }, Coord { x: 4096, y }])
            .unwrap()
            .property(name, ["one", "two"][i])
            .unwrap();
        f.finish().unwrap();
    }
    let bucket = build(&style, 14, &encode(layer));
    assert_eq!(bucket.labels.instances.len(), 4);
    let mut placer = SymbolPlacer::new(0.0);
    placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);
    assert_eq!(opacities(&placer, &bucket, 14), [1.0, 1.0, 0.0, 0.0]);
}

#[test]
fn symbols_avoid_edges() {
    let tile = points(&[(10, 2000, "abc"), (2000, 2000, "abc")]);
    let style = style(json!({"text-size": 24, "symbol-avoid-edges": true}));
    let bucket = build(&style, 14, &tile);
    let mut placer = SymbolPlacer::new(0.0);
    placer.update(0.0, &view(0.0), &[input(&style, &bucket, 14, false)]);
    assert_eq!(opacities(&placer, &bucket, 14), [0.0, 1.0]);
}
