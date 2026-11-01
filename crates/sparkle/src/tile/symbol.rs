//! `symbol` layers: text and icons at the anchor points of features, as quads that a
//! vertex shader scales to the current size and turns to face the viewer (or the map).
//!
//! Each label is laid out once per tile (see [`crate::symbol`]): its quads are stored
//! relative to its anchor in text units (pixels at `ONE_EM`) or icon units (pixels at
//! `icon-size` 1), and the shader multiplies them by the size it reads from the bucket's
//! paint attributes, so zooming never requires laying out again.
//!
//! Point placement puts labels at point features, the first point of lines, and the pole
//! of inaccessibility of polygons. Line placement spaces them along lines (and polygon
//! outlines); text that follows the line has each glyph's quad relative to the glyph's
//! own center, and placement (see [`crate::symbol::placement`]) moves the glyphs along
//! the line every frame.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use maplibre_expr::EvaluationContext;
use mlt_core::geo_types::{Geometry, LineString, Polygon};

use super::{BucketGeometry, BuildContext, FeatureInput, LayerBuilder, Scratch};
use crate::glyphs::{GlyphAtlas, GlyphAtlasBuilder, GlyphProvider, ONE_EM, SdfGlyph};
use crate::paint::PaintArrays;
use crate::sprite::{AtlasImage, ImageAtlas};
use crate::style::{
    Alignment, Color, Formatted, IconTextFit, SymbolAnchor, SymbolLayer, SymbolPlacement,
    TextJustify, TextRotationAlignment, TextTransform,
};
use crate::symbol::anchors::{pole_of_inaccessibility, radial_offset, variable_offset};
use crate::symbol::lines::{LabelExtent, LineAnchor, center_anchor, clip_line, line_anchors};
use crate::symbol::quads::{
    ShapedIcon, SymbolQuad, fit_icon_to_text, glyph_quad, icon_quads, shape_icon,
};
use crate::symbol::shaping::{ShapeOptions, Shaping, TextSection, anchor_alignment, shape_text};
use crate::symbol::{Labels, LineLabel, SymbolInstance};

/// One corner of a glyph or icon quad.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SymbolVertex {
    /// The label's anchor, in tile coordinates.
    pub anchor: [f32; 2],
    /// From the anchor, `y` down, in text or icon units: multiply by the size (see
    /// [`SymbolQuad::corners`]).
    pub offset: [f32; 2],
    /// Added after scaling `offset`, in pixels.
    pub pixel_offset: [f32; 2],
    /// In texture pixels: of [`SymbolBucket::glyphs`] for text, of the image atlas for icons.
    pub tex: [f32; 2],
    /// A `format` section's `text-color`, premultiplied, when [`symbol_flags::COLOR`] is set.
    pub color: [u8; 4],
    /// [`symbol_flags`].
    pub flags: u32,
}

/// Bits of [`SymbolVertex::flags`].
pub mod symbol_flags {
    /// The texture is a distance field (text, and SDF icons): recolor it with the color
    /// and halo properties.
    pub const SDF: u32 = 1;
    /// Use [`SymbolVertex::color`](super::SymbolVertex::color) instead of `text-color`.
    pub const COLOR: u32 = 2;
    /// An icon fitted to its text: scale it by `text-size / ONE_EM`, not `icon-size`.
    pub const TEXT_SIZED: u32 = 4;
}

/// The quads of one kind (text or icons) of a symbol bucket. Every four vertices are a
/// quad: top left, top right, bottom left, bottom right.
#[derive(Debug, Clone, Default)]
pub struct SymbolQuads {
    pub vertices: Vec<SymbolVertex>,
    /// Triangle list indices into [`Self::vertices`].
    pub triangles: Vec<u32>,
    /// Per-vertex values of [`SymbolLayer::text_attributes`] or
    /// [`SymbolLayer::icon_attributes`].
    pub paint: PaintArrays,
}

impl SymbolQuads {
    fn push(&mut self, quad: &SymbolQuad, anchor: [f32; 2], color: Option<[u8; 4]>, flags: u32) {
        let first = self.vertices.len() as u32;
        let [x, y, w, h] = quad.tex;
        let tex = [[x, y], [x + w, y], [x, y + h], [x + w, y + h]];
        let px = quad.pixel_offsets;
        let pixel = [px[0], [px[1][0], px[0][1]], [px[0][0], px[1][1]], px[1]];
        let flags = flags
            | if color.is_some() {
                symbol_flags::COLOR
            } else {
                0
            };
        for i in 0..4 {
            self.vertices.push(SymbolVertex {
                anchor,
                offset: quad.corners[i],
                pixel_offset: pixel[i],
                tex: tex[i],
                color: color.unwrap_or_default(),
                flags,
            });
        }
        self.triangles
            .extend([first, first + 1, first + 2, first + 1, first + 3, first + 2]);
    }

    fn len(&self) -> u32 {
        self.vertices.len() as u32
    }
}

/// Text and icons of a symbol layer in one tile.
///
/// Quads overlap freely and are drawn in order, without a stencil or depth test, and
/// outside their tile's clip area too (labels near a tile's edge reach into the next).
/// Which labels show, and where glyphs along lines go, is decided per frame by
/// [`SymbolPlacer`](crate::symbol::placement::SymbolPlacer) from [`Self::labels`].
#[derive(Debug, Clone, Default)]
pub struct SymbolBucket {
    pub text: SymbolQuads,
    pub icons: SymbolQuads,
    /// The distance fields of the glyphs `text` draws.
    pub glyphs: GlyphAtlas,
    /// Shared, so renderers can keep it for placement after uploading the vertices.
    pub labels: Arc<Labels>,
}

/// Tile units per logical pixel at the zoom the tile is laid out at, for 512 pixel tiles:
/// fewer when it's drawn magnified past its own zoom.
fn tile_pixel_ratio(extent: f32, overscale: f32) -> f32 {
    extent / 512.0 / overscale
}

pub(crate) fn builder<'a>(
    layer: &'a SymbolLayer,
    cx: &BuildContext<'a>,
    scratch: &mut Scratch,
) -> Option<Box<dyn LayerBuilder + 'a>> {
    let zoom = f64::from(cx.zoom);
    let l = &layer.layout;
    let placement = l.symbol_placement.evaluate_at_zoom(zoom);
    let along = placement != SymbolPlacement::Point;
    // `auto` rotation alignment is `map` for line placement, and only then do labels
    // follow their lines.
    let text_along_line = along
        && l.text_rotation_alignment.evaluate_at_zoom(zoom) != TextRotationAlignment::Viewport;
    let icons_along_line =
        along && l.icon_rotation_alignment.evaluate_at_zoom(zoom) != Alignment::Viewport;
    let mut atlas: GlyphAtlasBuilder = scratch.take();
    atlas.clear();
    Some(Box::new(SymbolBuilder {
        layer,
        glyphs: cx.resources.glyphs,
        images: cx.resources.images,
        zoom: cx.zoom,
        overscale: (cx.zoom - f32::from(cx.tile.z)).exp2(),
        extent: cx.extent as f32,
        placement,
        bucket: SymbolBucket {
            text: SymbolQuads {
                paint: PaintArrays::new(&layer.text_attributes(), cx.zoom),
                ..Default::default()
            },
            icons: SymbolQuads {
                paint: PaintArrays::new(&layer.icon_attributes(), cx.zoom),
                ..Default::default()
            },
            ..Default::default()
        },
        labels: Labels {
            text_along_line,
            icons_along_line,
            zoom: cx.zoom,
            extent: cx.extent,
            ..Default::default()
        },
        atlas,
        glyph_slots: Vec::new(),
        sorted: false,
        by_rarity: !along && l.sort_by_rarity.evaluate_at_zoom(zoom),
        icon_groups: HashMap::new(),
        icon_counts: Vec::new(),
        label_icons: Vec::new(),
        anchors: Vec::new(),
        lines: Vec::new(),
        repeats: HashMap::new(),
    }))
}

struct SymbolBuilder<'a> {
    layer: &'a SymbolLayer,
    glyphs: Option<&'a dyn GlyphProvider>,
    images: Option<&'a ImageAtlas>,
    zoom: f32,
    /// How many times the tile is magnified at `zoom`: 1 at its own zoom.
    overscale: f32,
    extent: f32,
    placement: SymbolPlacement,
    bucket: SymbolBucket,
    labels: Labels,
    atlas: GlyphAtlasBuilder,
    /// The atlas slot of each text quad, to add its position in the atlas at the end.
    glyph_slots: Vec<u32>,
    /// Whether any label has a sort key.
    sorted: bool,
    /// Whether to place labels with rarer icons first (`cardinal-sort-by-rarity`).
    by_rarity: bool,
    /// Each icon's index in `icon_counts`, by name, when sorting by rarity.
    icon_groups: HashMap<String, u32>,
    /// How many features show each icon.
    icon_counts: Vec<u32>,
    /// Each label's icon in `icon_counts`, when sorting by rarity.
    label_icons: Vec<Option<u32>>,
    /// The current feature's anchors, for point placement.
    anchors: Vec<[f32; 2]>,
    /// The current feature's lines, for line placement.
    lines: Vec<Vec<[f32; 2]>>,
    /// Anchors of line labels so far by [`SymbolInstance::key`], to keep the same text
    /// from repeating closer than half the spacing.
    repeats: HashMap<u64, Vec<[f32; 2]>>,
}

/// What a feature's text looks like, laid out.
struct TextLayout {
    shaping: Shaping,
    /// The distance field of each of `shaping.glyphs`, if it has one.
    sdfs: Vec<Option<Arc<SdfGlyph>>>,
    /// Each section's color, premultiplied.
    colors: Vec<Option<[u8; 4]>>,
    variable_anchors: Vec<(SymbolAnchor, [f32; 2])>,
    /// `text-offset` in text units, for text along lines (whose shaping has none).
    line_offset: [f32; 2],
    /// All of the text, for [`SymbolInstance::key`].
    text: String,
}

/// An icon laid out for a label.
struct IconLayout<'a> {
    name: &'a str,
    shaped: ShapedIcon,
    quads: Vec<SymbolQuad>,
    flags: u32,
}

impl SymbolBuilder<'_> {
    /// Lay out the feature's text. Text along a line keeps its offset separate.
    fn layout_text(&self, ctx: &EvaluationContext, along_line: bool) -> Option<TextLayout> {
        let glyphs = self.glyphs?;
        let l = &self.layer.layout;
        let field: Formatted = l.text_field.evaluate(ctx);
        if field.is_empty() {
            return None;
        }
        let transform = l.text_transform.evaluate(ctx);
        let fontstack = l.text_font.evaluate(ctx).join(",");
        let sections: Vec<TextSection> = field
            .0
            .iter()
            .filter(|s| s.image.is_none())
            .map(|s| TextSection {
                text: match transform {
                    TextTransform::Uppercase => s.text.to_uppercase(),
                    TextTransform::Lowercase => s.text.to_lowercase(),
                    TextTransform::None => s.text.clone(),
                },
                fontstack: s.font_stack.clone().unwrap_or_else(|| fontstack.clone()),
                scale: s.scale.unwrap_or(1.0),
                color: s.text_color,
            })
            .collect();

        let em = |v: [f32; 2]| v.map(|v| v * ONE_EM);
        let radial = l.text_radial_offset.evaluate(ctx) * ONE_EM;
        let text_offset = em(l.text_offset.evaluate(ctx));
        let point_placement = self.placement == SymbolPlacement::Point;
        // Variable anchors are for point placement only.
        let variable_anchors: Vec<(SymbolAnchor, [f32; 2])> = if !point_placement {
            Vec::new()
        } else {
            match l.text_variable_anchor_offset.evaluate(ctx) {
                Some(offsets) => offsets
                    .0
                    .iter()
                    .map(|&(a, o)| (a, variable_offset(a, em(o))))
                    .collect(),
                None => l
                    .text_variable_anchor
                    .evaluate(ctx)
                    .into_iter()
                    .map(|a| {
                        let offset = if radial != 0.0 {
                            radial_offset(a, radial)
                        } else {
                            variable_offset(a, text_offset)
                        };
                        (a, offset)
                    })
                    .collect(),
            }
        };
        let (anchor, offset) = match variable_anchors.first() {
            Some(&first) => first,
            None if along_line => (l.text_anchor.evaluate(ctx), [0.0; 2]),
            None => {
                let anchor = l.text_anchor.evaluate(ctx);
                let offset = if radial != 0.0 {
                    radial_offset(anchor, radial)
                } else {
                    text_offset
                };
                (anchor, offset)
            }
        };
        let justify = match l.text_justify.evaluate(ctx) {
            TextJustify::Left => 0.0,
            TextJustify::Center => 0.5,
            TextJustify::Right => 1.0,
            TextJustify::Auto => anchor_alignment(anchor)[0],
        };
        let options = ShapeOptions {
            // Labels on lines stay on one line, as in MapLibre.
            max_width: if point_placement {
                l.text_max_width.evaluate(ctx) * ONE_EM
            } else {
                0.0
            },
            line_height: l.text_line_height.evaluate(ctx) * ONE_EM,
            letter_spacing: l.text_letter_spacing.evaluate(ctx) * ONE_EM,
            justify,
            anchor,
            offset,
        };
        let shaping = shape_text(&sections, &options, glyphs)?;
        let sdfs = shaping.glyphs.iter().map(|g| glyphs.glyph(g.key)).collect();
        let colors = sections
            .iter()
            .map(|s| s.color.map(Color::to_rgba8))
            .collect();
        Some(TextLayout {
            shaping,
            sdfs,
            colors,
            variable_anchors,
            line_offset: if along_line { text_offset } else { [0.0; 2] },
            text: sections.iter().map(|s| s.text.as_str()).collect(),
        })
    }

    /// Where the feature's labels go, in tile coordinates, for point placement.
    fn find_anchors(&mut self, geometry: &Geometry<i32>) {
        let anchors = &mut self.anchors;
        anchors.clear();
        let pole = |p: &Polygon<i32>| {
            let rings: Vec<Vec<[f32; 2]>> = std::iter::once(p.exterior())
                .chain(p.interiors())
                .map(line_points)
                .collect();
            pole_of_inaccessibility(&rings, 16.0)
        };
        match geometry {
            Geometry::Point(p) => anchors.push(point(p.0)),
            Geometry::MultiPoint(mp) => anchors.extend(mp.0.iter().map(|p| point(p.0))),
            Geometry::LineString(l) => anchors.extend(l.0.first().map(|&c| point(c))),
            Geometry::MultiLineString(ml) => {
                anchors.extend(ml.0.iter().filter_map(|l| l.0.first().map(|&c| point(c))));
            }
            Geometry::Polygon(p) => anchors.extend(pole(p)),
            Geometry::MultiPolygon(mp) => anchors.extend(mp.0.iter().filter_map(pole)),
            _ => {}
        }
        // Labels belong to the tile their anchor is in, so neighbours don't both draw them.
        let extent = self.extent;
        anchors.retain(|&[x, y]| (0.0..extent).contains(&x) && (0.0..extent).contains(&y));
    }

    /// The lines to place labels along, for line placement: lines and polygon rings,
    /// clipped to the tile for `line` (but not `line-center`) placement.
    fn find_lines(&mut self, geometry: &Geometry<i32>) {
        self.lines.clear();
        let mut raw: Vec<Vec<[f32; 2]>> = Vec::new();
        let mut rings = |p: &Polygon<i32>| {
            raw.extend(
                std::iter::once(p.exterior())
                    .chain(p.interiors())
                    .map(line_points),
            );
        };
        match geometry {
            Geometry::Polygon(p) => rings(p),
            Geometry::MultiPolygon(mp) => mp.0.iter().for_each(rings),
            Geometry::LineString(l) => raw.push(line_points(l)),
            Geometry::MultiLineString(ml) => raw.extend(ml.0.iter().map(line_points)),
            _ => {}
        }
        if self.placement == SymbolPlacement::Line {
            for line in &raw {
                clip_line(line, 0.0, self.extent, &mut self.lines);
            }
        } else {
            self.lines = raw;
        }
        self.lines.retain(|l| l.len() > 1);
    }

    fn layout_icon<'i>(
        &self,
        ctx: &EvaluationContext,
        (image, name): (&'i AtlasImage, &'i str),
        text: Option<&TextLayout>,
        layout_text_size: f32,
    ) -> IconLayout<'i> {
        let l = &self.layer.layout;
        let offset = l.icon_offset.evaluate(ctx);
        let fit = l.icon_text_fit.evaluate(ctx);
        let shaped: ShapedIcon = match text {
            Some(text) if fit != IconTextFit::None => fit_icon_to_text(
                image,
                &text.shaping,
                fit,
                l.icon_text_fit_padding.evaluate(ctx),
                offset,
                layout_text_size / ONE_EM,
            ),
            _ => shape_icon(image, offset, l.icon_anchor.evaluate(ctx)),
        };
        let quads = icon_quads(&shaped, image, l.icon_rotate.evaluate(ctx));
        let mut flags = 0;
        if image.meta.sdf {
            flags |= symbol_flags::SDF;
        }
        if shaped.fitted {
            flags |= symbol_flags::TEXT_SIZED;
        }
        IconLayout {
            name,
            shaped,
            quads,
            flags,
        }
    }

    /// Anchors along the current feature's lines, as `(line index, anchor)`, for a label
    /// of this text and icon.
    fn line_anchors(
        &mut self,
        ctx: &EvaluationContext,
        text: Option<&TextLayout>,
        icon: Option<&IconLayout<'_>>,
        [layout_text_size, layout_icon_size]: [f32; 2],
        key: u64,
    ) -> Vec<LineAt> {
        let l = &self.layer.layout;
        let ratio = tile_pixel_ratio(self.extent, self.overscale);
        let text_scale = layout_text_size / ONE_EM * ratio;
        let icon_scale = match icon {
            Some(i) if i.shaped.fitted => text_scale,
            _ => layout_icon_size * ratio,
        };
        let text_length = text.map_or(0.0, |t| (t.shaping.right - t.shaping.left) * text_scale);
        let icon_length = icon.map_or(0.0, |i| (i.shaped.right - i.shaped.left) * icon_scale);
        let label = LabelExtent {
            length: text_length.max(icon_length),
            angle_window: if text.is_some() {
                3.0 / 5.0 * ONE_EM * text_scale
            } else {
                0.0
            },
            max_angle: l.text_max_angle.evaluate(ctx).to_radians(),
        };
        let spacing = l.symbol_spacing.evaluate(ctx) * ratio;
        let mut out = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            if self.placement == SymbolPlacement::Line {
                let glyph_size = ONE_EM * text_scale;
                let anchors = line_anchors(line, spacing, &label, glyph_size, 1.0, self.extent);
                out.extend(anchors.into_iter().map(|a| (i, a)));
            } else {
                out.extend(center_anchor(line, &label).map(|a| (i, a)));
            }
        }
        let tile = 0.0..self.extent;
        out.retain(|(_, a)| tile.contains(&a.point[0]) && tile.contains(&a.point[1]));
        if text.is_some() && self.placement == SymbolPlacement::Line {
            // The same text no closer than half the spacing, as MapLibre does.
            let repeat = spacing / 2.0;
            let seen = self.repeats.entry(key).or_default();
            out.retain(|(_, a)| {
                let close = seen
                    .iter()
                    .any(|p| (p[0] - a.point[0]).hypot(p[1] - a.point[1]) < repeat);
                if !close {
                    seen.push(a.point);
                }
                !close
            });
        }
        out
    }
}

/// An anchor on one of a feature's lines, with that line's index.
type LineAt = (usize, LineAnchor);

fn point(c: mlt_core::geo_types::Coord<i32>) -> [f32; 2] {
    [c.x as f32, c.y as f32]
}

fn line_points(l: &LineString<i32>) -> Vec<[f32; 2]> {
    l.0.iter().map(|&c| point(c)).collect()
}

/// Identifies a label by its text and icon, to match it across tiles.
fn label_key(text: Option<&str>, icon: Option<&str>) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    text.hash(&mut h);
    icon.hash(&mut h);
    h.finish()
}

impl LayerBuilder for SymbolBuilder<'_> {
    fn add_feature(&mut self, feature: &mut FeatureInput<'_, '_>) {
        let point_placement = self.placement == SymbolPlacement::Point;
        if point_placement {
            self.find_anchors(feature.geometry);
            if self.anchors.is_empty() {
                return;
            }
        } else {
            self.find_lines(feature.geometry);
            if self.lines.is_empty() {
                return;
            }
        }
        let ctx = &mut *feature.ctx;
        if let Some(images) = self.images
            && ctx.available_images.is_empty()
        {
            // For `["image", name]`, which is null when the image doesn't exist.
            ctx.available_images = images.names().map(str::to_owned).collect();
        }
        let layer = self.layer;
        let l = &layer.layout;
        let text_along_line = self.labels.text_along_line;
        let text = self.layout_text(ctx, text_along_line);
        let resolved = l.icon_image.evaluate(ctx);
        let image = resolved
            .as_ref()
            .and_then(|name| Some((self.images?.resolve(name)?, name.0.as_str())));
        if text.is_none() && image.is_none() {
            return;
        }

        // Sizes for layout are taken one zoom level in, as MapLibre does.
        let zoom = ctx.zoom;
        let (mut text_sizes, mut icon_sizes) = ([0.0; 2], [0.0; 2]);
        for (i, z) in [self.zoom, self.zoom + 1.0].into_iter().enumerate() {
            ctx.zoom = Some(f64::from(z));
            text_sizes[i] = l.text_size.evaluate(ctx);
            icon_sizes[i] = l.icon_size.evaluate(ctx);
        }
        ctx.zoom = zoom;
        let (layout_text_size, layout_icon_size) = (text_sizes[1], icon_sizes[1]);

        let icon = image.map(|image| self.layout_icon(ctx, image, text.as_ref(), layout_text_size));
        let key = label_key(
            text.as_ref().map(|t| t.text.as_str()),
            icon.as_ref().map(|i| i.name),
        );
        // Counted once per feature, however many labels it has.
        let icon_group = icon.as_ref().filter(|_| self.by_rarity).map(|i| {
            let next = self.icon_counts.len() as u32;
            let group = *self.icon_groups.entry(i.name.to_owned()).or_insert(next);
            if group == next {
                self.icon_counts.push(0);
            }
            self.icon_counts[group as usize] += 1;
            group
        });
        let text_rotate = l.text_rotate.evaluate(ctx);
        let icon_rotate = l.icon_rotate.evaluate(ctx);
        let sort_key = l.symbol_sort_key.evaluate(ctx);
        self.sorted |= sort_key.is_some();
        let writing_modes = l.text_writing_mode.evaluate(ctx);
        let text_padding = l.text_padding.evaluate(ctx);
        let icon_padding = l.icon_padding.evaluate(ctx);

        // Each label's anchor, and its line and place on it for line placement.
        let anchors: Vec<([f32; 2], Option<LineAt>)> = if point_placement {
            self.anchors.iter().map(|&a| (a, None)).collect()
        } else {
            let sizes = [layout_text_size, layout_icon_size];
            self.line_anchors(ctx, text.as_ref(), icon.as_ref(), sizes, key)
                .into_iter()
                .map(|(line, a)| (a.point, Some((line, a))))
                .collect()
        };
        // Where each of the feature's lines went in `Labels::lines`, once a label uses it.
        let mut stored_lines: Vec<Option<Range<u32>>> = vec![None; self.lines.len()];

        let bucket = &mut self.bucket;
        let labels = &mut self.labels;
        for (anchor, on_line) in anchors {
            let text_start = bucket.text.len();
            let icon_start = bucket.icons.len();
            if let Some(icon) = &icon {
                for quad in &icon.quads {
                    bucket.icons.push(quad, anchor, None, icon.flags);
                }
            }
            if let Some(text) = &text {
                for (glyph, sdf) in text.shaping.glyphs.iter().zip(&text.sdfs) {
                    let Some(sdf) = sdf else { continue };
                    let quad = if text_along_line {
                        // Relative to the glyph's center, which placement puts on the line.
                        let mut quad = glyph_quad(glyph, sdf, 0.0);
                        let along = (quad.corners[0][0] + quad.corners[1][0]) / 2.0;
                        for c in &mut quad.corners {
                            c[0] -= along;
                        }
                        quad.rotate(text_rotate);
                        labels.glyph_offsets.push(along);
                        quad
                    } else {
                        glyph_quad(glyph, sdf, text_rotate)
                    };
                    let color = text.colors[glyph.section as usize];
                    bucket.text.push(&quad, anchor, color, symbol_flags::SDF);
                    self.glyph_slots.push(self.atlas.slot(glyph.key, sdf));
                }
            }
            let line = on_line.map(|(i, a)| {
                let vertices = stored_lines[i]
                    .get_or_insert_with(|| {
                        let start = labels.lines.len() as u32;
                        labels.lines.extend_from_slice(&self.lines[i]);
                        start..labels.lines.len() as u32
                    })
                    .clone();
                LineLabel {
                    vertices,
                    segment: a.segment as u32,
                    text_offset: text.as_ref().map_or([0.0; 2], |t| t.line_offset),
                }
            });
            labels.instances.push(SymbolInstance {
                anchor,
                feature_id: feature.id,
                key,
                text_vertices: text_start..bucket.text.len(),
                icon_vertices: icon_start..bucket.icons.len(),
                text_box: text.as_ref().map(|t| {
                    [
                        t.shaping.left,
                        t.shaping.top,
                        t.shaping.right,
                        t.shaping.bottom,
                    ]
                }),
                icon_box: icon
                    .as_ref()
                    .map(|i| [i.shaped.left, i.shaped.top, i.shaped.right, i.shaped.bottom]),
                icon_fitted: icon.as_ref().is_some_and(|i| i.shaped.fitted),
                variable_anchors: text
                    .as_ref()
                    .map(|t| t.variable_anchors.clone())
                    .unwrap_or_default(),
                writing_modes: writing_modes.clone(),
                sort_key,
                layout_text_size,
                layout_icon_size,
                text_sizes,
                icon_sizes,
                text_rotate,
                icon_rotate,
                text_padding,
                icon_padding,
                line,
            });
            if self.by_rarity {
                self.label_icons.push(icon_group);
            }
        }
        let text_props = layer.text_attributes();
        bucket
            .text
            .paint
            .populate(&text_props, ctx, feature.id, bucket.text.vertices.len());
        let icon_props = layer.icon_attributes();
        bucket
            .icons
            .paint
            .populate(&icon_props, ctx, feature.id, bucket.icons.vertices.len());
    }

    fn finish(mut self: Box<Self>, scratch: &mut Scratch) -> Option<BucketGeometry> {
        let (atlas, origins) = self.atlas.build();
        for (quad, &slot) in self
            .bucket
            .text
            .vertices
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(&self.glyph_slots)
        {
            let [x, y] = origins[slot as usize].map(|v| v as f32);
            for v in quad {
                v.tex[0] += x;
                v.tex[1] += y;
            }
        }
        self.bucket.glyphs = atlas;
        if self.sorted || self.by_rarity {
            let counts: Vec<u32> = self
                .label_icons
                .iter()
                .map(|g| g.map_or(u32::MAX, |g| self.icon_counts[g as usize]))
                .collect();
            sort_by_key(&mut self.bucket, &mut self.labels, &counts);
        }
        scratch.put(std::mem::take(&mut self.atlas));
        let mut b = self.bucket;
        self.labels.text_vertex_count = b.text.len();
        self.labels.icon_vertex_count = b.icons.len();
        b.labels = Arc::new(self.labels);
        (!b.text.vertices.is_empty() || !b.icons.vertices.is_empty())
            .then_some(BucketGeometry::Symbol(b))
    }
}

/// Reorder labels by `symbol-sort-key`, lowest (drawn first, so underneath) first, then
/// by `counts` (how many features in the tile share each label's icon, empty when not
/// sorting by rarity), fewest first; ties keep their order.
fn sort_by_key(bucket: &mut SymbolBucket, labels: &mut Labels, counts: &[u32]) {
    let mut order: Vec<usize> = (0..labels.instances.len()).collect();
    order.sort_by(|&a, &b| {
        let key = |i: usize| labels.instances[i].sort_key.unwrap_or(0.0);
        let count = |i: usize| counts.get(i).copied().unwrap_or(0);
        key(a).total_cmp(&key(b)).then(count(a).cmp(&count(b)))
    });
    let text_ranges: Vec<_> = order
        .iter()
        .map(|&i| labels.instances[i].text_vertices.clone())
        .collect();
    let icon_ranges: Vec<_> = order
        .iter()
        .map(|&i| labels.instances[i].icon_vertices.clone())
        .collect();
    if !labels.glyph_offsets.is_empty() {
        // One per text quad, so they move with the quads.
        labels.glyph_offsets = text_ranges
            .iter()
            .flat_map(|r| &labels.glyph_offsets[r.start as usize / 4..r.end as usize / 4])
            .copied()
            .collect();
    }
    let new_text = reorder(&mut bucket.text, &text_ranges);
    let new_icons = reorder(&mut bucket.icons, &icon_ranges);
    let mut instances: Vec<_> = order.iter().map(|&i| labels.instances[i].clone()).collect();
    for (k, instance) in instances.iter_mut().enumerate() {
        instance.text_vertices = new_text[k].clone();
        instance.icon_vertices = new_icons[k].clone();
    }
    labels.instances = instances;
}

/// Rebuild `quads` with the vertices of `ranges` in that order (they cover all of them);
/// returns where each range ended up.
fn reorder(quads: &mut SymbolQuads, ranges: &[Range<u32>]) -> Vec<Range<u32>> {
    let stride = quads.paint.stride as usize;
    let mut vertices = Vec::with_capacity(quads.vertices.len());
    let mut data = Vec::with_capacity(quads.paint.data.len());
    let mut moved = Vec::with_capacity(ranges.len());
    for r in ranges {
        let start = vertices.len() as u32;
        vertices.extend_from_slice(&quads.vertices[r.start as usize..r.end as usize]);
        if stride > 0 {
            data.extend_from_slice(
                &quads.paint.data[r.start as usize * stride..r.end as usize * stride],
            );
        }
        moved.push(start..vertices.len() as u32);
    }
    // Paint values are per feature; with labels moved apart, track them per label.
    let ids = |r: &Range<u32>| {
        quads
            .paint
            .features
            .iter()
            .find(|f| f.vertices.contains(&r.start))
            .and_then(|f| f.id)
    };
    quads.paint.features = ranges
        .iter()
        .zip(&moved)
        .filter(|(_, m)| !m.is_empty())
        .map(|(r, m)| crate::paint::FeatureVertices {
            id: ids(r),
            vertices: m.clone(),
        })
        .collect();
    quads.vertices = vertices;
    if stride > 0 {
        quads.paint.data = data;
    }
    quads.triangles = (0..quads.vertices.len() as u32 / 4)
        .flat_map(|q| {
            let v = q * 4;
            [v, v + 1, v + 2, v + 1, v + 3, v + 2]
        })
        .collect();
    moved
}
