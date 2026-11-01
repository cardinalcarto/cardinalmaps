//! Which labels show, and where, for the camera of one frame: MapLibre's placement.
//!
//! A [`SymbolPlacer`] lives as long as the map. Each frame the renderer hands it every
//! visible symbol bucket with its tile's matrix ([`SymbolPlacer::update`]); it then:
//!
//! - matches labels to the same labels in tiles of other zoom levels ([`CrossTileIndex`]),
//!   so a label keeps its state while a parent tile is replaced by its children, and one
//!   shown by two tiles at once is drawn once;
//! - runs collision detection, top layer first and in each bucket's priority order, in a
//!   [`CollisionGrid`] of screen space. It does so when the buckets change, and otherwise at
//!   most once per fade duration while the camera moves, as MapLibre does, so that labels
//!   don't flicker in and out while panning;
//! - fades labels in and out as their placement changes;
//! - lays out the glyphs of labels along lines for the current camera, every frame.
//!
//! The result per bucket is one [`PlacedBucket`]: a dynamic vertex value per text and icon
//! vertex that the vertex shader reads next to the static quads. See [`PlacedBucket`] for
//! what it holds in each [`DrawMode`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::collision::{CollisionGrid, Shape};
use super::projection::{
    LabelPlane, LineFit, LineProjection, PlacedGlyph, TileProjection, to_screen,
};
use super::shaping::anchor_alignment;
use super::{Labels, SymbolInstance};
use crate::glyphs::ONE_EM;
use crate::paint::{Binding, PaintProperty};
use crate::style::{
    Alignment, Overlap, PropertyValue, SymbolLayer, SymbolPlacement, SymbolZOrder,
    TextRotationAlignment, TranslateAnchor,
};

/// MapLibre's default `fadeDuration`, in seconds.
pub const DEFAULT_FADE_DURATION: f64 = 0.3;

/// Labels along lines whose anchor is more than this many logical pixels outside the
/// viewport aren't laid out, as in MapLibre.
const LINE_CLIP_PADDING: f32 = 256.0;

/// The camera, as placement sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacementView {
    /// In logical pixels.
    pub viewport: [f32; 2],
    pub zoom: f64,
    /// In radians, clockwise.
    pub bearing: f32,
    /// The distance from the camera to the center of the view in logical pixels: the clip
    /// `w` of points at the center, which labels elsewhere are scaled against.
    pub camera_to_center: f32,
}

/// Identifies a symbol bucket across frames: a style layer in a tile at a place on the
/// map. `x` is not wrapped, so copies of the world have their own buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BucketKey {
    pub layer: usize,
    pub z: u8,
    pub x: i64,
    pub y: u32,
}

impl BucketKey {
    /// Whether one tile contains the other.
    fn overlaps(&self, other: &BucketKey) -> bool {
        let z = self.z.min(other.z);
        let up = |k: &BucketKey| {
            let d = k.z - z;
            (k.x.div_euclid(1 << d), k.y >> d)
        };
        up(self) == up(other)
    }

    /// `point` (tile units of a tile of `extent`) in world coordinates, `0..1` across the
    /// world.
    fn world(&self, point: [f32; 2], extent: f32) -> [f64; 2] {
        let scale = f64::from(self.z).exp2();
        [
            (self.x as f64 + f64::from(point[0] / extent)) / scale,
            (f64::from(self.y) + f64::from(point[1] / extent)) / scale,
        ]
    }
}

/// One symbol bucket to place this frame.
pub struct PlacementBucket<'a> {
    pub key: BucketKey,
    pub layer: &'a SymbolLayer,
    pub labels: &'a Arc<Labels>,
    /// Tile units to clip space.
    pub projection: TileProjection,
    /// Tile units per logical pixel at the camera's zoom.
    pub units_per_pixel: f32,
}

/// How a part (text or icons) of a symbol layer is drawn, after resolving `auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartAlignment {
    /// Glyphs are placed one by one along the label's line.
    pub along_line: bool,
    pub rotate_with_map: bool,
    pub pitch_with_map: bool,
    /// Glyphs along a line turn with it (all but `viewport-glyph`).
    pub rotate_to_line: bool,
    pub keep_upright: bool,
}

impl PartAlignment {
    /// For a layer's text, with `along_line` from its [`Labels::text_along_line`].
    pub fn text(layer: &SymbolLayer, zoom: f64, along_line: bool) -> Self {
        let l = &layer.layout;
        let line = l.symbol_placement.evaluate_at_zoom(zoom) != SymbolPlacement::Point;
        let rotation = match l.text_rotation_alignment.evaluate_at_zoom(zoom) {
            TextRotationAlignment::Auto if line => TextRotationAlignment::Map,
            TextRotationAlignment::Auto => TextRotationAlignment::Viewport,
            r => r,
        };
        let pitch_with_map = match l.text_pitch_alignment.evaluate_at_zoom(zoom) {
            Alignment::Auto => rotation == TextRotationAlignment::Map,
            a => a == Alignment::Map,
        };
        Self {
            along_line,
            rotate_with_map: rotation == TextRotationAlignment::Map,
            pitch_with_map,
            rotate_to_line: rotation == TextRotationAlignment::Map,
            keep_upright: l.text_keep_upright.evaluate_at_zoom(zoom),
        }
    }

    /// For a layer's icons, with `along_line` from its [`Labels::icons_along_line`].
    pub fn icon(layer: &SymbolLayer, zoom: f64, along_line: bool) -> Self {
        let l = &layer.layout;
        let line = l.symbol_placement.evaluate_at_zoom(zoom) != SymbolPlacement::Point;
        let rotation = match l.icon_rotation_alignment.evaluate_at_zoom(zoom) {
            Alignment::Auto if line => Alignment::Map,
            Alignment::Auto => Alignment::Viewport,
            r => r,
        };
        let pitch_with_map = match l.icon_pitch_alignment.evaluate_at_zoom(zoom) {
            Alignment::Auto => rotation == Alignment::Map,
            a => a == Alignment::Map,
        };
        Self {
            along_line,
            rotate_with_map: rotation == Alignment::Map,
            pitch_with_map,
            rotate_to_line: rotation == Alignment::Map,
            keep_upright: l.icon_keep_upright.evaluate_at_zoom(zoom),
        }
    }

    /// What [`PlacedBucket`]'s vertex values mean for this part.
    pub fn draw_mode(&self) -> DrawMode {
        match (self.along_line, self.pitch_with_map) {
            (false, _) => DrawMode::Point,
            (true, true) => DrawMode::LineInTile,
            (true, false) => DrawMode::LineOnScreen,
        }
    }
}

/// How the vertex values of [`PlacedBucket`] position a part's quads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawMode {
    /// `[shift_x, shift_y, 0, opacity]`: quads stay around the anchor, moved by the shift
    /// (in text or icon units, before scaling) of the chosen variable anchor.
    Point,
    /// `[x, y, angle, opacity]`: each quad is centered on `x, y` in tile units (on the
    /// map, `*-translate` included) and turned by `angle` radians (clockwise, `y` down).
    LineInTile,
    /// `[x, y, angle, opacity]` with `x, y` in logical pixels from the viewport's top
    /// left.
    LineOnScreen,
}

/// The dynamic vertex values of one bucket for this frame. A vertex whose opacity is zero
/// is hidden. See [`DrawMode`] for the rest.
#[derive(Debug, Clone, Default)]
pub struct PlacedBucket {
    /// One per vertex of [`SymbolBucket::text`](crate::tile::SymbolBucket::text).
    pub text: Vec<[f32; 4]>,
    /// One per vertex of [`SymbolBucket::icons`](crate::tile::SymbolBucket::icons).
    pub icons: Vec<[f32; 4]>,
    /// Triangle lists in draw order, when labels overlap and are drawn sorted by how far
    /// down the screen they are (`symbol-z-order`); `None` keeps the bucket's order.
    pub text_triangles: Option<Vec<u32>>,
    pub icon_triangles: Option<Vec<u32>>,
    /// Changes whenever the triangle lists do.
    pub triangles_version: u64,
}

/// A size property at the camera's zoom, as the shader computes it (see
/// [`crate::paint`]): per feature values come from [`SymbolInstance::text_sizes`] and
/// [`SymbolInstance::icon_sizes`].
#[derive(Debug, Clone, Copy)]
struct SizeAt {
    binding: Binding,
    value: f32,
    factor: f32,
}

impl SizeAt {
    fn new(property: &PropertyValue<f32>, zoom: f64, tile_zoom: f32) -> Self {
        let binding = PaintProperty::binding(property);
        let lower = f64::from(tile_zoom);
        Self {
            binding,
            value: if binding == Binding::Uniform {
                property.evaluate_at_zoom(zoom)
            } else {
                0.0
            },
            factor: property.zoom_curve().map_or(0.0, |c| {
                c.factor(zoom, lower, lower + 1.0).clamp(0.0, 1.0) as f32
            }),
        }
    }

    fn get(&self, sizes: [f32; 2]) -> f32 {
        match self.binding {
            Binding::Uniform => self.value,
            Binding::Source => sizes[0],
            Binding::Composite => sizes[0] + (sizes[1] - sizes[0]) * self.factor,
        }
    }
}

/// The parts of a layer's layout placement reads, at the camera's zoom.
struct PartParams {
    alignment: PartAlignment,
    overlap: Overlap,
    ignore_placement: bool,
    optional: bool,
    /// `*-translate` in tile units.
    translate: [f32; 2],
}

struct LayerParams {
    text: PartParams,
    icon: PartParams,
    text_size: SizeAt,
    icon_size: SizeAt,
    avoid_edges: bool,
    /// Place labels lowest on screen first (`symbol-z-order: viewport-y`).
    place_by_y: bool,
    /// Draw labels lowest on screen last, when they may overlap.
    draw_by_y: bool,
}

/// `*-translate` in tile units, turned with the map unless anchored to the viewport.
fn translate_units(t: [f32; 2], anchor: TranslateAnchor, bearing: f32, upp: f32) -> [f32; 2] {
    let t = match anchor {
        TranslateAnchor::Map => t,
        TranslateAnchor::Viewport => rotate(t, bearing),
    };
    t.map(|v| v * upp)
}

impl LayerParams {
    fn new(layer: &SymbolLayer, labels: &Labels, view: &PlacementView, upp: f32) -> Self {
        let zoom = view.zoom;
        let (l, p) = (&layer.layout, &layer.paint);
        let overlap = |o: &PropertyValue<Option<Overlap>>, allow: &PropertyValue<bool>| {
            o.evaluate_at_zoom(zoom)
                .unwrap_or(if allow.evaluate_at_zoom(zoom) {
                    Overlap::Always
                } else {
                    Overlap::Never
                })
        };
        let text = PartParams {
            alignment: PartAlignment::text(layer, zoom, labels.text_along_line),
            overlap: overlap(&l.text_overlap, &l.text_allow_overlap),
            ignore_placement: l.text_ignore_placement.evaluate_at_zoom(zoom),
            optional: l.text_optional.evaluate_at_zoom(zoom),
            translate: translate_units(
                p.text_translate.evaluate_at_zoom(zoom),
                p.text_translate_anchor.evaluate_at_zoom(zoom),
                view.bearing,
                upp,
            ),
        };
        let icon = PartParams {
            alignment: PartAlignment::icon(layer, zoom, labels.icons_along_line),
            overlap: overlap(&l.icon_overlap, &l.icon_allow_overlap),
            ignore_placement: l.icon_ignore_placement.evaluate_at_zoom(zoom),
            optional: l.icon_optional.evaluate_at_zoom(zoom),
            translate: translate_units(
                p.icon_translate.evaluate_at_zoom(zoom),
                p.icon_translate_anchor.evaluate_at_zoom(zoom),
                view.bearing,
                upp,
            ),
        };
        let z_order = l.symbol_z_order.evaluate_at_zoom(zoom);
        let sort_by_key = z_order != SymbolZOrder::ViewportY
            && !matches!(l.symbol_sort_key.as_constant(), Some(None));
        let can_overlap = text.overlap != Overlap::Never
            || icon.overlap != Overlap::Never
            || text.ignore_placement
            || icon.ignore_placement;
        let by_y =
            z_order == SymbolZOrder::ViewportY || (z_order == SymbolZOrder::Auto && !sort_by_key);
        Self {
            text_size: SizeAt::new(&l.text_size, zoom, labels.zoom),
            icon_size: SizeAt::new(&l.icon_size, zoom, labels.zoom),
            avoid_edges: l.symbol_avoid_edges.evaluate_at_zoom(zoom),
            place_by_y: z_order == SymbolZOrder::ViewportY,
            draw_by_y: by_y && can_overlap,
            text,
            icon,
        }
    }
}

/// Turn `v` clockwise (with `y` down) by `angle` radians.
fn rotate(v: [f32; 2], angle: f32) -> [f32; 2] {
    if angle == 0.0 {
        return v;
    }
    let (s, c) = angle.sin_cos();
    [c * v[0] - s * v[1], s * v[0] + c * v[1]]
}

/// MapLibre's size compensation for labels away from the center of a pitched view: those
/// drawn on the map grow with distance, those facing the viewer shrink, both half as much
/// as the map does. The shader does the same.
fn perspective_ratio(pitch_with_map: bool, w: f32, camera_to_center: f32) -> f32 {
    let ratio = if pitch_with_map {
        w / camera_to_center
    } else {
        camera_to_center / w
    };
    (0.5 + 0.5 * ratio).clamp(0.0, 4.0)
}

/// One bucket's tile as seen by the camera.
struct TileView<'a> {
    projection: &'a TileProjection,
    upp: f32,
    view: &'a PlacementView,
}

impl TileView<'_> {
    fn screen(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        let clip = self.projection.project(p);
        (clip[3] > 0.0).then(|| to_screen(clip, self.view.viewport))
    }

    /// The angle of the map's `x` axis on screen at `p` (clip position `clip`), which
    /// labels rotating with the map but facing the viewer are turned by.
    fn map_angle(&self, p: [f32; 2], clip: [f32; 4]) -> f32 {
        let east = self.projection.project([p[0] + self.upp, p[1]]);
        let [vw, vh] = self.view.viewport;
        let dx = east[0] / east[3] - clip[0] / clip[3];
        let dy = -(east[1] / east[3] - clip[1] / clip[3]);
        (dy * vh).atan2(dx * vw)
    }

    /// The screen box of a point label part: `rect` (in layout units, around the anchor)
    /// turned by `rotate` degrees, moved by `shift` and scaled by `scale`, then placed as
    /// the shader places it. `padding` is `[top, right, bottom, left]` in pixels.
    #[allow(clippy::too_many_arguments)]
    fn point_box(
        &self,
        align: &PartAlignment,
        anchor: [f32; 2],
        rect: [f32; 4],
        rotate_degrees: f32,
        shift: [f32; 2],
        scale: f32,
        padding: [f32; 4],
    ) -> Option<Shape> {
        let clip = self.projection.project(anchor);
        if clip[3] <= 0.0 {
            return None;
        }
        let s =
            scale * perspective_ratio(align.pitch_with_map, clip[3], self.view.camera_to_center);
        let [l, t, r, b] = rect;
        let turn = rotate_degrees.to_radians();
        let corners = [[l, t], [r, t], [l, b], [r, b]].map(|c| {
            let c = rotate(c, turn);
            [(c[0] + shift[0]) * s, (c[1] + shift[1]) * s]
        });
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        let center = to_screen(clip, self.view.viewport);
        let map_angle = if !align.pitch_with_map && align.rotate_with_map {
            self.map_angle(anchor, clip)
        } else {
            0.0
        };
        for c in corners {
            let p = if align.pitch_with_map {
                let c = if align.rotate_with_map {
                    c
                } else {
                    rotate(c, self.view.bearing)
                };
                self.screen([anchor[0] + c[0] * self.upp, anchor[1] + c[1] * self.upp])?
            } else {
                let c = rotate(c, map_angle);
                [center[0] + c[0], center[1] + c[1]]
            };
            for i in 0..2 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        Some(Shape::Box([
            min[0] - padding[3],
            min[1] - padding[0],
            max[0] + padding[1],
            max[1] + padding[2],
        ]))
    }

    /// Whether a box of `rect` scaled by `scale` pixels per unit around `anchor` stays in
    /// the tile, for `symbol-avoid-edges`. Ignores rotation and pitch.
    fn inside_tile(&self, anchor: [f32; 2], rect: [f32; 4], scale: f32, extent: f32) -> bool {
        let s = scale * self.upp;
        let tile = 0.0..=extent;
        tile.contains(&(anchor[0] + rect[0] * s))
            && tile.contains(&(anchor[0] + rect[2] * s))
            && tile.contains(&(anchor[1] + rect[1] * s))
            && tile.contains(&(anchor[1] + rect[3] * s))
    }
}

/// Glyphs of one label laid out along its line for the current camera.
struct LinePlacement {
    /// In the label plane: tile units ([`DrawMode::LineInTile`]) or logical pixels
    /// ([`DrawMode::LineOnScreen`]).
    glyphs: Vec<PlacedGlyph>,
    /// Logical pixels per layout unit on screen at the anchor, roughly.
    screen_scale: f32,
}

/// Lay out a label's `offsets` (layout units along the line, one per quad) along its line,
/// or `None` if it doesn't fit or is too far off screen.
#[allow(clippy::too_many_arguments)]
fn place_on_line(
    tile: &TileView<'_>,
    align: &PartAlignment,
    labels: &Labels,
    instance: &SymbolInstance,
    offsets: &[f32],
    line_offset: [f32; 2],
    size: f32,
    translate: [f32; 2],
) -> Option<LinePlacement> {
    let label = instance.line.as_ref()?;
    let view = tile.view;
    let line = &labels.lines[label.vertices.start as usize..label.vertices.end as usize];
    let anchor = instance.anchor;
    let clip = tile
        .projection
        .project([anchor[0] + translate[0], anchor[1] + translate[1]]);
    let w = clip[3];
    let pad = [0, 1].map(|i| 1.0 + 2.0 * LINE_CLIP_PADDING / view.viewport[i]);
    if w <= 0.0 || clip[0].abs() > w * pad[0] || clip[1].abs() > w * pad[1] {
        return None;
    }
    let persp = perspective_ratio(align.pitch_with_map, w, view.camera_to_center);
    let pitch = align.pitch_with_map;
    let (plane, plane_scale, anchor_plane) = if pitch {
        (LabelPlane::Tile, size * persp * tile.upp, anchor)
    } else {
        let plane = LabelPlane::Screen {
            projection: tile.projection,
            size: view.viewport,
        };
        (plane, size * persp, tile.screen(anchor)?)
    };
    let scaled: Vec<f32> = offsets.iter().map(|o| o * plane_scale).collect();
    let line_offset = line_offset.map(|o| o * plane_scale);
    let mut projection =
        LineProjection::new(plane, line, anchor, anchor_plane, label.segment as usize);
    projection.keep_upright = align.keep_upright;
    projection.rotate_to_line = align.rotate_to_line;
    let to_screen_fn = |p: [f32; 2]| if pitch { tile.screen(p) } else { Some(p) };
    let mut glyphs = Vec::with_capacity(offsets.len());
    let mut place = |flip: bool, glyphs: &mut Vec<PlacedGlyph>| {
        projection.place_glyphs(&scaled, line_offset, flip, to_screen_fn, glyphs)
    };
    match place(false, &mut glyphs) {
        Ok(()) => {}
        Err(LineFit::NeedsFlipping) => place(true, &mut glyphs).ok()?,
        Err(LineFit::NotEnoughRoom) => return None,
    }
    // Moved by `*-translate`: in tile units on the map, or as the anchor moves on screen.
    let shift = if pitch {
        translate
    } else {
        let a = tile.screen(anchor)?;
        let b = tile.screen([anchor[0] + translate[0], anchor[1] + translate[1]])?;
        [b[0] - a[0], b[1] - a[1]]
    };
    for g in &mut glyphs {
        g.point = [g.point[0] + shift[0], g.point[1] + shift[1]];
        if !align.rotate_to_line && pitch {
            // Upright on screen, as the shader keeps point labels on the map.
            g.angle = view.bearing;
        }
    }
    let screen_scale = if pitch {
        size * persp * view.camera_to_center / w
    } else {
        size * persp
    };
    Some(LinePlacement {
        glyphs,
        screen_scale,
    })
}

/// The circles covering a label along a line on screen: along the path through its glyphs,
/// `radius` pixels apart.
fn line_circles(
    tile: &TileView<'_>,
    mode: DrawMode,
    glyphs: &[PlacedGlyph],
    radius: f32,
    out: &mut Vec<Shape>,
) -> Option<()> {
    let mut path = Vec::with_capacity(glyphs.len());
    for g in glyphs {
        path.push(match mode {
            DrawMode::LineInTile => tile.screen(g.point)?,
            _ => g.point,
        });
    }
    let step = radius.max(1.0);
    out.push(Shape::Circle([path[0][0], path[0][1], radius]));
    for w in path.windows(2) {
        let d = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
        let l = d[0].hypot(d[1]);
        let n = (l / step).ceil() as usize;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            out.push(Shape::Circle([
                w[0][0] + d[0] * t,
                w[0][1] + d[1] * t,
                radius,
            ]));
        }
    }
    Some(())
}

/// How one label came out of the last placement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LabelState {
    text: bool,
    icon: bool,
    /// Index into the label's variable anchors.
    anchor: u8,
    /// The same label is placed from another tile.
    duplicate: bool,
}

/// A label's opacity, moving towards whether it's placed.
#[derive(Debug, Clone, Copy, Default)]
struct Fade {
    opacity: f32,
    placed: bool,
}

impl Fade {
    fn step(&mut self, delta: f32) -> bool {
        let target = if self.placed { 1.0 } else { 0.0 };
        self.opacity = if delta.is_infinite() {
            target
        } else if self.placed {
            (self.opacity + delta).min(1.0)
        } else {
            (self.opacity - delta).max(0.0)
        };
        self.opacity != target
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct LabelOpacity {
    text: Fade,
    icon: Fade,
}

/// Labels of one layer in one tile, as the [`CrossTileIndex`] knows them.
struct IndexEntry {
    key: u64,
    world: [f64; 2],
    id: u32,
}

/// Gives each label an id that is the same for the same label in overlapping tiles of
/// different zoom levels: MapLibre's `CrossTileSymbolIndex`.
#[derive(Default)]
struct CrossTileIndex {
    tiles: HashMap<BucketKey, Vec<IndexEntry>>,
    next_id: u32,
}

/// How close (in logical pixels at the lower zoom) two labels must be to be the same.
const MATCH_TOLERANCE: f64 = 3.0;

impl CrossTileIndex {
    /// Ids for the labels of a bucket that just appeared, reusing those of matching labels
    /// in tiles above or below it, or in the bucket it replaces.
    fn add(&mut self, bucket: BucketKey, labels: &Labels) -> Vec<u32> {
        let mut candidates: HashMap<u64, Vec<([f64; 2], u32, u8)>> = HashMap::new();
        for (k, entries) in &self.tiles {
            if k.layer == bucket.layer && bucket.overlaps(k) {
                for e in entries {
                    candidates
                        .entry(e.key)
                        .or_default()
                        .push((e.world, e.id, k.z));
                }
            }
        }
        let extent = labels.extent as f32;
        let mut used = HashSet::new();
        let mut entries = Vec::with_capacity(labels.instances.len());
        for instance in &labels.instances {
            let world = bucket.world(instance.anchor, extent);
            let found = candidates.get(&instance.key).and_then(|c| {
                c.iter()
                    .filter(|(_, id, _)| !used.contains(id))
                    .filter(|(p, _, z)| {
                        let pixel = 1.0 / (512.0 * f64::from(bucket.z.min(*z)).exp2());
                        let tolerance = MATCH_TOLERANCE * pixel;
                        (p[0] - world[0]).abs() <= tolerance && (p[1] - world[1]).abs() <= tolerance
                    })
                    .map(|&(_, id, _)| id)
                    .next()
            });
            let id = found.unwrap_or_else(|| {
                self.next_id = self.next_id.wrapping_add(1);
                self.next_id
            });
            used.insert(id);
            entries.push(IndexEntry {
                key: instance.key,
                world,
                id,
            });
        }
        let ids = entries.iter().map(|e| e.id).collect();
        self.tiles.insert(bucket, entries);
        ids
    }
}

struct BucketState {
    labels: Arc<Labels>,
    /// Cross-tile id per label.
    ids: Vec<u32>,
    placed: Vec<LabelState>,
    out: PlacedBucket,
    /// The bearing labels were last sorted for, when drawn sorted by viewport `y`.
    sorted_for: Option<f32>,
}

/// Placement state kept from frame to frame. See the [module docs](self).
pub struct SymbolPlacer {
    /// In seconds; zero shows placement changes at once.
    fade_duration: f64,
    index: CrossTileIndex,
    buckets: HashMap<BucketKey, BucketState>,
    opacities: HashMap<u32, LabelOpacity>,
    grid: CollisionGrid,
    /// When placement last ran, and what it ran with.
    placed_at: Option<f64>,
    placed_for: Vec<(BucketKey, TileProjection)>,
    placed_view: Option<PlacementView>,
    last_frame: Option<f64>,
    /// Scratch.
    shapes: Vec<Shape>,
}

impl Default for SymbolPlacer {
    fn default() -> Self {
        Self::new(DEFAULT_FADE_DURATION)
    }
}

impl SymbolPlacer {
    /// A placer whose labels fade in and out over `fade_duration` seconds, which is also
    /// how often placement may run while the camera moves.
    pub fn new(fade_duration: f64) -> Self {
        Self {
            fade_duration: fade_duration.max(0.0),
            index: CrossTileIndex::default(),
            buckets: HashMap::new(),
            opacities: HashMap::new(),
            grid: CollisionGrid::new([1.0; 2]),
            placed_at: None,
            placed_for: Vec::new(),
            placed_view: None,
            last_frame: None,
            shapes: Vec::new(),
        }
    }

    /// The vertex values of a bucket from the last [`Self::update`].
    pub fn bucket(&self, key: &BucketKey) -> Option<&PlacedBucket> {
        self.buckets.get(key).map(|s| &s.out)
    }

    /// Place labels for a frame at `now` (seconds, from any fixed point) with every visible
    /// symbol bucket. Buckets listed twice are placed once. Returns whether another frame
    /// is needed soon: while labels fade, or while the placement is out of date with a
    /// camera that moved since.
    pub fn update(
        &mut self,
        now: f64,
        view: &PlacementView,
        buckets: &[PlacementBucket<'_>],
    ) -> bool {
        let mut seen = HashSet::new();
        let inputs: Vec<&PlacementBucket<'_>> =
            buckets.iter().filter(|b| seen.insert(b.key)).collect();

        let mut changed = false;
        for b in &inputs {
            let current = self.buckets.get(&b.key);
            if current.is_some_and(|s| Arc::ptr_eq(&s.labels, b.labels)) {
                continue;
            }
            // A rebuilt tile's labels keep their ids, and with them their opacity and
            // anchor, rather than fading in again.
            let previous: HashMap<u32, LabelState> = current
                .map(|s| s.ids.iter().copied().zip(s.placed.iter().copied()).collect())
                .unwrap_or_default();
            let ids = self.index.add(b.key, b.labels);
            let placed = ids
                .iter()
                .map(|id| previous.get(id).copied().unwrap_or_default())
                .collect();
            self.buckets.insert(
                b.key,
                BucketState {
                    labels: b.labels.clone(),
                    ids,
                    placed,
                    out: PlacedBucket::default(),
                    sorted_for: None,
                },
            );
            changed = true;
        }
        let before = self.buckets.len();
        self.buckets.retain(|k, _| seen.contains(k));
        if self.buckets.len() != before {
            changed = true;
            let index = &mut self.index;
            let buckets = &self.buckets;
            index.tiles.retain(|k, _| buckets.contains_key(k));
        }

        let mut signature: Vec<(BucketKey, TileProjection)> = inputs
            .iter()
            .map(|b| (b.key, b.projection.clone()))
            .collect();
        signature.sort_by_key(|(k, _)| *k);
        let stale = signature != self.placed_for || self.placed_view.as_ref() != Some(view);
        let due = self
            .placed_at
            .is_none_or(|t| now - t >= self.fade_duration || now < t);
        let mut pending = false;
        if changed || (stale && due) {
            self.place(view, &inputs);
            self.placed_at = Some(now);
            self.placed_for = signature;
            self.placed_view = Some(*view);
        } else {
            pending = stale;
        }

        let dt = self.last_frame.map_or(0.0, |t| (now - t).max(0.0));
        self.last_frame = Some(now);
        let delta = if self.fade_duration == 0.0 {
            f32::INFINITY
        } else {
            (dt / self.fade_duration) as f32
        };
        let mut fading = false;
        for o in self.opacities.values_mut() {
            fading |= o.text.step(delta);
            fading |= o.icon.step(delta);
        }

        for b in &inputs {
            self.write(view, b);
        }
        // Forget labels that are gone and fully faded out.
        let live: HashSet<u32> = self
            .buckets
            .values()
            .flat_map(|s| s.ids.iter().copied())
            .collect();
        self.opacities
            .retain(|id, o| live.contains(id) || o.text.opacity > 0.0 || o.icon.opacity > 0.0);
        pending || fading
    }

    /// Collision detection over every bucket, top layer first.
    fn place(&mut self, view: &PlacementView, inputs: &[&PlacementBucket<'_>]) {
        self.grid.reset(view.viewport);
        let mut order: Vec<&PlacementBucket<'_>> = inputs.to_vec();
        order.sort_by(|a, b| {
            (b.key.layer, b.key.z)
                .cmp(&(a.key.layer, a.key.z))
                .then((a.key.x, a.key.y).cmp(&(b.key.x, b.key.y)))
        });
        let mut seen: HashSet<u32> = HashSet::new();
        let mut targets: HashMap<u32, (bool, bool)> = HashMap::new();
        for b in order {
            let Some(state) = self.buckets.get_mut(&b.key) else {
                continue;
            };
            let labels = state.labels.clone();
            let tile = TileView {
                projection: &b.projection,
                upp: b.units_per_pixel,
                view,
            };
            let params = LayerParams::new(b.layer, &labels, view, b.units_per_pixel);
            let mut indices: Vec<usize> = (0..labels.instances.len()).collect();
            if params.place_by_y {
                // Lowest on screen first.
                let ys = viewport_ys(&labels, view.bearing);
                indices.sort_by(|&a, &b| ys[b].total_cmp(&ys[a]));
            }
            for i in indices {
                let id = state.ids[i];
                if !seen.insert(id) {
                    state.placed[i] = LabelState {
                        duplicate: true,
                        ..Default::default()
                    };
                    continue;
                }
                let prev = state.placed[i];
                let placed = place_label(
                    &mut self.grid,
                    &mut self.shapes,
                    &tile,
                    &params,
                    &labels,
                    &labels.instances[i],
                    prev,
                );
                state.placed[i] = placed;
                targets.insert(id, (placed.text, placed.icon));
            }
        }
        for (id, o) in &mut self.opacities {
            let (text, icon) = targets.remove(id).unwrap_or_default();
            o.text.placed = text;
            o.icon.placed = icon;
        }
        for (id, (text, icon)) in targets {
            let mut o = LabelOpacity::default();
            o.text.placed = text;
            o.icon.placed = icon;
            self.opacities.insert(id, o);
        }
    }

    /// Fill a bucket's vertex values for this frame.
    fn write(&mut self, view: &PlacementView, b: &PlacementBucket<'_>) {
        let Some(state) = self.buckets.get_mut(&b.key) else {
            return;
        };
        let labels = state.labels.clone();
        let tile = TileView {
            projection: &b.projection,
            upp: b.units_per_pixel,
            view,
        };
        let params = LayerParams::new(b.layer, &labels, view, b.units_per_pixel);
        let out = &mut state.out;
        out.text.clear();
        out.text.resize(labels.text_vertex_count as usize, [0.0; 4]);
        out.icons.clear();
        out.icons
            .resize(labels.icon_vertex_count as usize, [0.0; 4]);
        let text_mode = params.text.alignment.draw_mode();
        let icon_mode = params.icon.alignment.draw_mode();
        for (i, instance) in labels.instances.iter().enumerate() {
            let placed = state.placed[i];
            let opacity = if placed.duplicate {
                LabelOpacity::default()
            } else {
                self.opacities
                    .get(&state.ids[i])
                    .copied()
                    .unwrap_or_default()
            };
            let shift = anchor_shift(instance, placed.anchor as usize);
            let text_size = params.text_size.get(instance.text_sizes);

            let range = |r: &std::ops::Range<u32>| r.start as usize..r.end as usize;
            let text = &mut out.text[range(&instance.text_vertices)];
            if opacity.text.opacity > 0.0 {
                match text_mode {
                    DrawMode::Point => text.fill([shift[0], shift[1], 0.0, opacity.text.opacity]),
                    _ => {
                        let quads = range(&instance.text_vertices);
                        let offsets = &labels.glyph_offsets[quads.start / 4..quads.end / 4];
                        let line_offset =
                            instance.line.as_ref().map_or([0.0; 2], |l| l.text_offset);
                        let placement = place_on_line(
                            &tile,
                            &params.text.alignment,
                            &labels,
                            instance,
                            offsets,
                            line_offset,
                            text_size / ONE_EM,
                            params.text.translate,
                        );
                        if let Some(p) = placement {
                            for (quad, g) in text.chunks_mut(4).zip(&p.glyphs) {
                                quad.fill([g.point[0], g.point[1], g.angle, opacity.text.opacity]);
                            }
                        }
                    }
                }
            }

            let icons = &mut out.icons[range(&instance.icon_vertices)];
            if opacity.icon.opacity > 0.0 {
                let icon_shift = if instance.icon_fitted {
                    shift
                } else {
                    [0.0; 2]
                };
                match icon_mode {
                    DrawMode::Point => {
                        icons.fill([icon_shift[0], icon_shift[1], 0.0, opacity.icon.opacity]);
                    }
                    _ => {
                        let size = icon_scale(&params, instance, text_size);
                        let placement = place_on_line(
                            &tile,
                            &params.icon.alignment,
                            &labels,
                            instance,
                            &[0.0],
                            [0.0; 2],
                            size,
                            params.icon.translate,
                        );
                        if let Some(g) = placement.and_then(|p| p.glyphs.first().copied()) {
                            icons.fill([g.point[0], g.point[1], g.angle, opacity.icon.opacity]);
                        }
                    }
                }
            }
        }

        if params.draw_by_y && state.sorted_for != Some(view.bearing) {
            state.sorted_for = Some(view.bearing);
            let ys = viewport_ys(&labels, view.bearing);
            let mut order: Vec<usize> = (0..labels.instances.len()).collect();
            order.sort_by(|&a, &b| ys[a].total_cmp(&ys[b]));
            let triangles = |ranges: &mut dyn Iterator<Item = std::ops::Range<u32>>| {
                ranges
                    .flat_map(|r| (r.start / 4..r.end / 4).map(|q| q * 4))
                    .flat_map(|v| [v, v + 1, v + 2, v + 1, v + 3, v + 2])
                    .collect::<Vec<u32>>()
            };
            let instances = &labels.instances;
            out.text_triangles = Some(triangles(
                &mut order.iter().map(|&i| instances[i].text_vertices.clone()),
            ));
            out.icon_triangles = Some(triangles(
                &mut order.iter().map(|&i| instances[i].icon_vertices.clone()),
            ));
            out.triangles_version += 1;
        } else if !params.draw_by_y && state.sorted_for.take().is_some() {
            out.text_triangles = None;
            out.icon_triangles = None;
            out.triangles_version += 1;
        }
    }
}

/// How far down the screen each label's anchor is with the map turned by `bearing`, as
/// MapLibre sorts by.
fn viewport_ys(labels: &Labels, bearing: f32) -> Vec<f32> {
    let (s, c) = bearing.sin_cos();
    labels
        .instances
        .iter()
        .map(|i| -s * i.anchor[0] + c * i.anchor[1])
        .collect()
}

/// Pixels per icon unit for a label's icon.
fn icon_scale(params: &LayerParams, instance: &SymbolInstance, text_size: f32) -> f32 {
    if instance.icon_fitted {
        text_size / ONE_EM
    } else {
        params.icon_size.get(instance.icon_sizes)
    }
}

/// How far a label's text moves (in text units) from where it was laid out, to be placed
/// with its variable anchor `index` instead of the first.
fn anchor_shift(instance: &SymbolInstance, index: usize) -> [f32; 2] {
    let anchors = &instance.variable_anchors;
    let (Some(&(first, first_offset)), Some(&(anchor, offset)), Some([l, t, r, b])) =
        (anchors.first(), anchors.get(index), instance.text_box)
    else {
        return [0.0; 2];
    };
    if index == 0 {
        return [0.0; 2];
    }
    let [h0, v0] = anchor_alignment(first);
    let [h, v] = anchor_alignment(anchor);
    [
        (h0 - h) * (r - l) + offset[0] - first_offset[0],
        (v0 - v) * (b - t) + offset[1] - first_offset[1],
    ]
}

/// Collision detection for one label: whether its text and icon fit, with which variable
/// anchor, claiming their space in `grid` if they do.
fn place_label(
    grid: &mut CollisionGrid,
    shapes: &mut Vec<Shape>,
    tile: &TileView<'_>,
    params: &LayerParams,
    labels: &Labels,
    instance: &SymbolInstance,
    prev: LabelState,
) -> LabelState {
    let extent = labels.extent as f32;
    let has_text = instance.text_box.is_some() && !instance.text_vertices.is_empty();
    let has_icon = instance.icon_box.is_some() && !instance.icon_vertices.is_empty();
    let text_size = params.text_size.get(instance.text_sizes);
    let text_scale = text_size / ONE_EM;
    let icon_scale = icon_scale(params, instance, text_size);
    let text_padding = [instance.text_padding; 4];
    let (tp, ip) = (&params.text, &params.icon);
    let translated = |t: [f32; 2]| [instance.anchor[0] + t[0], instance.anchor[1] + t[1]];

    let mut result = LabelState::default();
    shapes.clear();
    let mut text_shapes = 0;
    if has_text {
        if tp.alignment.along_line {
            let quads =
                instance.text_vertices.start as usize / 4..instance.text_vertices.end as usize / 4;
            let line_offset = instance.line.as_ref().map_or([0.0; 2], |l| l.text_offset);
            let placement = place_on_line(
                tile,
                &tp.alignment,
                labels,
                instance,
                &labels.glyph_offsets[quads],
                line_offset,
                text_scale,
                tp.translate,
            );
            if let Some(p) = placement {
                let [_, top, _, bottom] = instance.text_box.unwrap_or_default();
                let radius = (bottom - top) / 2.0 * p.screen_scale + instance.text_padding;
                let mode = tp.alignment.draw_mode();
                let inside = !params.avoid_edges
                    || mode != DrawMode::LineInTile
                    || p.glyphs.iter().all(|g| {
                        (0.0..=extent).contains(&g.point[0]) && (0.0..=extent).contains(&g.point[1])
                    });
                if inside && line_circles(tile, mode, &p.glyphs, radius, shapes).is_some() {
                    result.text = shapes.iter().all(|s| grid.fits(s, tp.overlap));
                }
            }
        } else {
            let rect = instance.text_box.unwrap_or_default();
            let count = instance.variable_anchors.len().max(1);
            // The previous anchor first, so labels don't jump between anchors.
            let mut candidates: Vec<usize> = (0..count).collect();
            if (prev.anchor as usize) < count && prev.anchor > 0 {
                candidates.retain(|&k| k != prev.anchor as usize);
                candidates.insert(0, prev.anchor as usize);
            }
            // With overlap allowed, still prefer an anchor where the text doesn't overlap.
            let passes: &[Overlap] = if tp.overlap == Overlap::Never || count == 1 {
                std::slice::from_ref(&tp.overlap)
            } else {
                &[Overlap::Never, tp.overlap]
            };
            'search: for &overlap in passes {
                for &k in &candidates {
                    let shift = anchor_shift(instance, k);
                    let anchor = translated(tp.translate);
                    let Some(shape) = tile.point_box(
                        &tp.alignment,
                        anchor,
                        rect,
                        instance.text_rotate,
                        shift,
                        text_scale,
                        text_padding,
                    ) else {
                        continue;
                    };
                    let shifted = [
                        rect[0] + shift[0],
                        rect[1] + shift[1],
                        rect[2] + shift[0],
                        rect[3] + shift[1],
                    ];
                    if params.avoid_edges && !tile.inside_tile(anchor, shifted, text_scale, extent)
                    {
                        continue;
                    }
                    if grid.fits(&shape, overlap) {
                        result.text = true;
                        result.anchor = k as u8;
                        shapes.push(shape);
                        break 'search;
                    }
                }
            }
        }
        text_shapes = shapes.len();
    }

    if has_icon {
        let rect = instance.icon_box.unwrap_or_default();
        let shift = if instance.icon_fitted {
            anchor_shift(instance, result.anchor as usize)
        } else {
            [0.0; 2]
        };
        let anchor = translated(ip.translate);
        let shape = if ip.alignment.along_line {
            place_on_line(
                tile,
                &ip.alignment,
                labels,
                instance,
                &[0.0],
                [0.0; 2],
                icon_scale,
                ip.translate,
            )
            .and_then(|p| {
                let g = p.glyphs.first()?;
                let point = match ip.alignment.draw_mode() {
                    DrawMode::LineInTile => tile.screen(g.point)?,
                    _ => g.point,
                };
                let half = (rect[2] - rect[0]).max(rect[3] - rect[1]) / 2.0;
                let padding = instance.icon_padding.0.into_iter().fold(0.0, f32::max);
                let radius = half * p.screen_scale + padding;
                Some(Shape::Circle([point[0], point[1], radius]))
            })
        } else {
            let edges_ok =
                !params.avoid_edges || tile.inside_tile(anchor, rect, icon_scale, extent);
            edges_ok
                .then(|| {
                    tile.point_box(
                        &ip.alignment,
                        anchor,
                        rect,
                        instance.icon_rotate,
                        shift,
                        icon_scale,
                        instance.icon_padding.0,
                    )
                })
                .flatten()
        };
        if let Some(shape) = shape {
            result.icon = grid.fits(&shape, ip.overlap);
            shapes.push(shape);
        }
    }

    // A label shows its text and icon together unless one of them is optional.
    if has_text && has_icon {
        let (text_ok, icon_ok) = (result.text, result.icon);
        if !tp.optional && !ip.optional {
            result.text = text_ok && icon_ok;
            result.icon = result.text;
        } else if !ip.optional {
            result.text = text_ok && icon_ok;
        } else if !tp.optional {
            result.icon = text_ok && icon_ok;
        }
    }
    if result.text && !tp.ignore_placement {
        for &s in &shapes[..text_shapes] {
            grid.insert(s, tp.overlap);
        }
    }
    if result.icon && !ip.ignore_placement {
        for &s in &shapes[text_shapes..] {
            grid.insert(s, ip.overlap);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::SymbolAnchor;

    #[test]
    fn tiles_overlap_their_ancestors_only() {
        let key = |z, x, y| BucketKey { layer: 0, z, x, y };
        assert!(key(3, 5, 2).overlaps(&key(1, 1, 0)));
        assert!(key(1, 1, 0).overlaps(&key(3, 5, 2)));
        assert!(!key(3, 5, 2).overlaps(&key(1, 0, 0)));
        assert!(!key(3, 5, 2).overlaps(&key(3, 4, 2)));
        // Copies of the world don't overlap.
        assert!(!key(1, -1, 0).overlaps(&key(1, 1, 0)));
    }

    #[test]
    fn variable_anchor_shifts_move_the_text_box() {
        let instance = SymbolInstance {
            anchor: [0.0; 2],
            feature_id: None,
            key: 0,
            text_vertices: 0..4,
            icon_vertices: 0..0,
            text_box: Some([-20.0, -10.0, 20.0, 10.0]),
            icon_box: None,
            icon_fitted: false,
            variable_anchors: vec![
                (SymbolAnchor::Center, [0.0; 2]),
                (SymbolAnchor::Left, [5.0, 0.0]),
                (SymbolAnchor::Top, [0.0, 3.0]),
            ],
            writing_modes: Vec::new(),
            sort_key: None,
            layout_text_size: 16.0,
            layout_icon_size: 1.0,
            text_sizes: [16.0; 2],
            icon_sizes: [1.0; 2],
            text_rotate: 0.0,
            icon_rotate: 0.0,
            text_padding: 0.0,
            icon_padding: Default::default(),
            line: None,
        };
        assert_eq!(anchor_shift(&instance, 0), [0.0; 2]);
        // Left of the text at the anchor: half its width right, plus the offset.
        assert_eq!(anchor_shift(&instance, 1), [25.0, 0.0]);
        assert_eq!(anchor_shift(&instance, 2), [0.0, 13.0]);
    }

    #[test]
    fn fades_move_towards_placement() {
        let mut f = Fade {
            opacity: 0.0,
            placed: true,
        };
        assert!(f.step(0.4));
        assert!(f.step(0.4));
        assert!(!f.step(0.4));
        assert_eq!(f.opacity, 1.0);
        f.placed = false;
        assert!(!f.step(f32::INFINITY));
        assert_eq!(f.opacity, 0.0);
    }
}
