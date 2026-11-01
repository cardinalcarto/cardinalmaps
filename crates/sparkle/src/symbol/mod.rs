//! Symbol layout and placement: turning a feature's `text-field` and `icon-image` into
//! positioned glyph and icon quads on the CPU, and deciding every frame which labels show.
//!
//! Layout, once per tile (driven by the bucket builder in `tile::symbol`):
//!
//! - [`shaping`] lays out text: `format` sections, line breaking, bidirectional text and
//!   complex scripts (through a [`GlyphProvider`](crate::glyphs::GlyphProvider)),
//!   justification and anchoring.
//! - [`quads`] makes the textured quads of glyphs and icons, including icons stretched to
//!   fit their text.
//! - [`anchors`] finds label points in polygons and computes the text offsets of
//!   `text-variable-anchor` and `text-radial-offset`; [`lines`] spaces labels along lines.
//!
//! Placement, per frame (see [`placement`]): projecting labels with the camera, laying
//! glyphs out along their lines on screen, collision detection in a [`collision`] grid,
//! matching labels across tiles of different zoom levels, and fading.

pub mod anchors;
pub mod collision;
pub mod lines;
pub mod placement;
mod projection;
pub mod quads;
pub mod shaping;

use std::ops::Range;

use crate::style::{Padding, SymbolAnchor, WritingMode};

/// One label (text and/or icon at one anchor) of a symbol bucket, with what placement
/// needs to know about it. Boxes are `[left, top, right, bottom]` relative to the anchor,
/// in the units of [`quads::SymbolQuad`] corners.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolInstance {
    /// In tile units.
    pub anchor: [f32; 2],
    /// The feature's id, if it has one.
    pub feature_id: Option<u64>,
    /// Identifies what the label shows (its text and icon), to find the same label in
    /// tiles of other zoom levels.
    pub key: u64,
    /// The label's vertices in [`SymbolBucket::text`](crate::tile::SymbolBucket::text).
    pub text_vertices: Range<u32>,
    /// The label's vertices in [`SymbolBucket::icons`](crate::tile::SymbolBucket::icons).
    pub icon_vertices: Range<u32>,
    /// In text units (pixels at `ONE_EM`), without rotation.
    pub text_box: Option<[f32; 4]>,
    /// In icon units (pixels at `icon-size` 1), or text units for text-fitted icons.
    pub icon_box: Option<[f32; 4]>,
    /// Whether the icon is fitted to the text, so it's in text units and moves with it.
    pub icon_fitted: bool,
    /// The anchors the text may be placed with, first choice first, with the offset (in
    /// text units) each needs; the quads were laid out with the first. Empty unless
    /// `text-variable-anchor` or `text-variable-anchor-offset` is set.
    pub variable_anchors: Vec<(SymbolAnchor, [f32; 2])>,
    pub writing_modes: Vec<WritingMode>,
    pub sort_key: Option<f32>,
    /// `text-size` and `icon-size` at the tile's zoom + 1, as MapLibre lays out with.
    pub layout_text_size: f32,
    pub layout_icon_size: f32,
    /// `text-size` and `icon-size` at the tile's zoom and the next, which placement
    /// interpolates between as shaders do when the sizes differ per feature.
    pub text_sizes: [f32; 2],
    pub icon_sizes: [f32; 2],
    /// `text-rotate` and `icon-rotate`, in degrees clockwise: the quads are turned by it
    /// already, the boxes not.
    pub text_rotate: f32,
    pub icon_rotate: f32,
    /// In pixels.
    pub text_padding: f32,
    pub icon_padding: Padding,
    /// Set for labels laid out along a line.
    pub line: Option<LineLabel>,
}

/// Where a label laid out along a line is on it.
#[derive(Debug, Clone, PartialEq)]
pub struct LineLabel {
    /// The line, in [`Labels::lines`].
    pub vertices: Range<u32>,
    /// The anchor lies on the segment from `vertices.start + segment` to the next vertex.
    pub segment: u32,
    /// `text-offset` in text units: `x` along the line, `y` across it.
    pub text_offset: [f32; 2],
}

/// Everything about a symbol bucket's labels that placement needs, without its vertices.
#[derive(Debug, Clone, Default)]
pub struct Labels {
    /// In draw order.
    pub instances: Vec<SymbolInstance>,
    /// The lines labels are laid out along, in tile units.
    pub lines: Vec<[f32; 2]>,
    /// For each text quad of labels laid out along lines, how far the glyph's center is
    /// along the line from the anchor, in text units. Empty when no label is.
    pub glyph_offsets: Vec<f32>,
    /// Whether text and icons follow their lines (line placement with rotation alignment
    /// other than `viewport`); their quads are relative to each glyph's center then.
    pub text_along_line: bool,
    pub icons_along_line: bool,
    /// The zoom the tile was laid out at.
    pub zoom: f32,
    pub extent: u32,
    pub text_vertex_count: u32,
    pub icon_vertex_count: u32,
}
