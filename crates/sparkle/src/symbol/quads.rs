//! Quads for the glyphs and icons of a label, as MapLibre's `getGlyphQuads` and
//! `getIconQuads` make them, including icons stretched by `icon-text-fit`.

use crate::glyphs::SdfGlyph;
use crate::sprite::{AtlasImage, PADDING};
use crate::style::{IconTextFit, SymbolAnchor};

use super::shaping::{PositionedGlyph, Shaping, anchor_alignment};

/// One textured quad of a label, relative to the label's anchor, `y` down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SymbolQuad {
    /// Top left, top right, bottom left and bottom right corners. Renderers scale these
    /// with the size: by `text-size / ONE_EM` for text and text-fitted icons, by
    /// `icon-size` for other icons.
    pub corners: [[f32; 2]; 4],
    /// Offsets added to the top left and bottom right corners after scaling, in pixels,
    /// so the fixed parts of stretched icons keep their size.
    pub pixel_offsets: [[f32; 2]; 2],
    /// `[x, y, width, height]` in texture pixels: in the glyph's distance field for
    /// glyphs, in the image atlas for icons.
    pub tex: [f32; 4],
}

impl SymbolQuad {
    fn rect(left: f32, top: f32, right: f32, bottom: f32, tex: [f32; 4]) -> Self {
        Self {
            corners: [[left, top], [right, top], [left, bottom], [right, bottom]],
            pixel_offsets: [[0.0; 2]; 2],
            tex,
        }
    }

    /// Turn the corners clockwise by `degrees` around the anchor.
    pub fn rotate(&mut self, degrees: f32) {
        if degrees == 0.0 {
            return;
        }
        let (sin, cos) = degrees.to_radians().sin_cos();
        for c in &mut self.corners {
            *c = [cos * c[0] - sin * c[1], sin * c[0] + cos * c[1]];
        }
    }
}

/// The quad of one positioned glyph, rotated by `rotate` degrees around the anchor.
pub fn glyph_quad(glyph: &PositionedGlyph, sdf: &SdfGlyph, rotate: f32) -> SymbolQuad {
    let s = glyph.scale;
    let left = glyph.x + sdf.left * s;
    let top = glyph.y - sdf.top * s;
    let (w, h) = (sdf.width as f32, sdf.height as f32);
    let mut quad = SymbolQuad::rect(left, top, left + w * s, top + h * s, [0.0, 0.0, w, h]);
    quad.rotate(rotate);
    quad
}

/// Where an icon goes relative to its anchor, in its layout units (see [`SymbolQuad`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedIcon {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    /// Fitted to text by `icon-text-fit`, so in text units.
    pub fitted: bool,
}

/// An icon of `image` placed by `icon-offset` and `icon-anchor`.
pub fn shape_icon(image: &AtlasImage, offset: [f32; 2], anchor: SymbolAnchor) -> ShapedIcon {
    let [h, v] = anchor_alignment(anchor);
    let [w, ht] = image.logical_size();
    let left = offset[0] - w * h;
    let top = offset[1] - ht * v;
    ShapedIcon {
        left,
        top,
        right: left + w,
        bottom: top + ht,
        fitted: false,
    }
}

/// Stretch an icon around `text` for `icon-text-fit`, in text units. `padding`
/// (`[top, right, bottom, left]`) and `offset` are in pixels, at `font_scale` pixels per
/// text unit.
pub fn fit_icon_to_text(
    image: &AtlasImage,
    text: &Shaping,
    fit: IconTextFit,
    padding: [f32; 4],
    offset: [f32; 2],
    font_scale: f32,
) -> ShapedIcon {
    let s = font_scale.max(1e-6);
    let [w, h] = image.logical_size().map(|v| v / s);
    let offset = offset.map(|v| v / s);
    let padding = padding.map(|v| v / s);
    let (left, right) = if matches!(fit, IconTextFit::Width | IconTextFit::Both) {
        (
            offset[0] + text.left - padding[3],
            offset[0] + text.right + padding[1],
        )
    } else {
        let left = offset[0] + (text.left + text.right - w) / 2.0;
        (left, left + w)
    };
    let (top, bottom) = if matches!(fit, IconTextFit::Height | IconTextFit::Both) {
        (
            offset[1] + text.top - padding[0],
            offset[1] + text.bottom + padding[2],
        )
    } else {
        let top = offset[1] + (text.top + text.bottom - h) / 2.0;
        (top, top + h)
    };
    ShapedIcon {
        left,
        top,
        right,
        bottom,
        fitted: true,
    }
}

/// A position in an image along one axis, split into the part in stretchable ranges and
/// the part in fixed ones.
#[derive(Debug, Clone, Copy)]
struct Cut {
    fixed: f32,
    stretch: f32,
}

/// Total length of `ranges` within `min..max`.
fn sum_within(ranges: &[[f32; 2]], min: f32, max: f32) -> f32 {
    ranges
        .iter()
        .map(|r| r[1].clamp(min, max) - r[0].clamp(min, max))
        .sum()
}

/// The cuts between the fixed and stretchable parts of an image, padding included.
fn stretch_cuts(zones: &[[f32; 2]], fixed_size: f32, stretch_size: f32) -> Vec<Cut> {
    let border = PADDING as f32;
    let mut cuts = vec![Cut {
        fixed: -border,
        stretch: 0.0,
    }];
    for &[c1, c2] in zones {
        let last = *cuts.last().unwrap();
        cuts.push(Cut {
            fixed: c1 - last.stretch,
            stretch: last.stretch,
        });
        cuts.push(Cut {
            fixed: c1 - last.stretch,
            stretch: last.stretch + (c2 - c1),
        });
    }
    cuts.push(Cut {
        fixed: fixed_size + border,
        stretch: stretch_size,
    });
    cuts
}

/// The quads of an icon: one, or a grid of them when a text-fitted icon has stretchable
/// ranges, so that only those ranges stretch. `rotate` is in degrees, clockwise.
pub fn icon_quads(icon: &ShapedIcon, image: &AtlasImage, rotate: f32) -> Vec<SymbolQuad> {
    let [ax, ay, iw, ih] = image.rect;
    let (image_width, image_height) = (iw as f32, ih as f32);
    let ratio = image.meta.pixel_ratio.max(1e-6);
    let full_x = [[0.0, image_width]];
    let full_y = [[0.0, image_height]];
    let stretch_x: &[[f32; 2]] = if image.meta.stretch_x.is_empty() {
        &full_x
    } else {
        &image.meta.stretch_x
    };
    let stretch_y: &[[f32; 2]] = if image.meta.stretch_y.is_empty() {
        &full_y
    } else {
        &image.meta.stretch_y
    };
    let stretch_width: f32 = stretch_x.iter().map(|r| r[1] - r[0]).sum();
    let stretch_height: f32 = stretch_y.iter().map(|r| r[1] - r[0]).sum();
    let fixed_width = image_width - stretch_width;
    let fixed_height = image_height - stretch_height;

    let (mut stretch_offset_x, mut stretch_content_width) = (0.0, stretch_width);
    let (mut stretch_offset_y, mut stretch_content_height) = (0.0, stretch_height);
    let (mut fixed_offset_x, mut fixed_content_width) = (0.0, fixed_width);
    let (mut fixed_offset_y, mut fixed_content_height) = (0.0, fixed_height);
    if let (Some(c), true) = (image.meta.content, icon.fitted) {
        stretch_offset_x = sum_within(stretch_x, 0.0, c[0]);
        stretch_offset_y = sum_within(stretch_y, 0.0, c[1]);
        stretch_content_width = sum_within(stretch_x, c[0], c[2]);
        stretch_content_height = sum_within(stretch_y, c[1], c[3]);
        fixed_offset_x = c[0] - stretch_offset_x;
        fixed_offset_y = c[1] - stretch_offset_y;
        fixed_content_width = (c[2] - c[0]) - stretch_content_width;
        fixed_content_height = (c[3] - c[1]) - stretch_content_height;
    }

    let icon_width = icon.right - icon.left;
    let icon_height = icon.bottom - icon.top;
    let em = |stretch: f32, size: f32, icon_size: f32, origin: f32| {
        if size == 0.0 {
            origin
        } else {
            stretch / size * icon_size + origin
        }
    };
    let px = |fixed: f32, fixed_size: f32, stretch: f32, stretch_size: f32| {
        if stretch_size == 0.0 {
            fixed
        } else {
            fixed - fixed_size * stretch / stretch_size
        }
    };
    let make = |left: Cut, top: Cut, right: Cut, bottom: Cut| {
        let left_em = em(
            left.stretch - stretch_offset_x,
            stretch_content_width,
            icon_width,
            icon.left,
        );
        let left_px = px(
            left.fixed - fixed_offset_x,
            fixed_content_width,
            left.stretch,
            stretch_width,
        );
        let top_em = em(
            top.stretch - stretch_offset_y,
            stretch_content_height,
            icon_height,
            icon.top,
        );
        let top_px = px(
            top.fixed - fixed_offset_y,
            fixed_content_height,
            top.stretch,
            stretch_height,
        );
        let right_em = em(
            right.stretch - stretch_offset_x,
            stretch_content_width,
            icon_width,
            icon.left,
        );
        let right_px = px(
            right.fixed - fixed_offset_x,
            fixed_content_width,
            right.stretch,
            stretch_width,
        );
        let bottom_em = em(
            bottom.stretch - stretch_offset_y,
            stretch_content_height,
            icon_height,
            icon.top,
        );
        let bottom_px = px(
            bottom.fixed - fixed_offset_y,
            fixed_content_height,
            bottom.stretch,
            stretch_height,
        );
        let x1 = left.stretch + left.fixed;
        let x2 = right.stretch + right.fixed;
        let y1 = top.stretch + top.fixed;
        let y2 = bottom.stretch + bottom.fixed;
        let mut quad = SymbolQuad::rect(
            left_em,
            top_em,
            right_em,
            bottom_em,
            [ax as f32 + x1, ay as f32 + y1, x2 - x1, y2 - y1],
        );
        quad.pixel_offsets = [
            [left_px / ratio, top_px / ratio],
            [right_px / ratio, bottom_px / ratio],
        ];
        quad.rotate(rotate);
        quad
    };

    let stretches = !image.meta.stretch_x.is_empty() || !image.meta.stretch_y.is_empty();
    if !icon.fitted || !stretches {
        // The whole image, with its transparent border so edges filter smoothly.
        let border = PADDING as f32;
        return vec![make(
            Cut {
                fixed: 0.0,
                stretch: -border,
            },
            Cut {
                fixed: 0.0,
                stretch: -border,
            },
            Cut {
                fixed: 0.0,
                stretch: image_width + border,
            },
            Cut {
                fixed: 0.0,
                stretch: image_height + border,
            },
        )];
    }
    let x_cuts = stretch_cuts(stretch_x, fixed_width, stretch_width);
    let y_cuts = stretch_cuts(stretch_y, fixed_height, stretch_height);
    let mut quads = Vec::new();
    for xs in x_cuts.windows(2) {
        for ys in y_cuts.windows(2) {
            quads.push(make(xs[0], ys[0], xs[1], ys[1]));
        }
    }
    quads
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sprite::ImageMeta;

    fn image(width: u32, height: u32, meta: ImageMeta) -> AtlasImage {
        AtlasImage {
            rect: [10, 20, width, height],
            meta: ImageMeta {
                pixel_ratio: 1.0,
                ..meta
            },
        }
    }

    #[test]
    fn plain_icon_covers_its_image_and_border() {
        let img = image(8, 4, ImageMeta::default());
        let icon = shape_icon(&img, [1.0, 0.0], SymbolAnchor::Bottom);
        assert_eq!(
            (icon.left, icon.top, icon.right, icon.bottom),
            (-3.0, -4.0, 5.0, 0.0)
        );
        let quads = icon_quads(&icon, &img, 0.0);
        assert_eq!(quads.len(), 1);
        let q = quads[0];
        assert_eq!(q.tex, [9.0, 19.0, 10.0, 6.0]);
        assert_eq!(q.corners[0], [-4.0, -5.0]);
        assert_eq!(q.corners[3], [6.0, 1.0]);
        assert_eq!(q.pixel_offsets, [[0.0; 2]; 2]);
    }

    #[test]
    fn rotation_is_clockwise() {
        let mut q = SymbolQuad::rect(1.0, 0.0, 1.0, 0.0, [0.0; 4]);
        q.rotate(90.0);
        let [x, y] = q.corners[0];
        assert!(x.abs() < 1e-6 && (y - 1.0).abs() < 1e-6, "{x} {y}");
    }

    #[test]
    fn fitted_icon_stretches_only_its_stretch_ranges() {
        // 6 pixels wide: 1 fixed, 4 stretchable, 1 fixed.
        let img = image(
            6,
            2,
            ImageMeta {
                stretch_x: vec![[1.0, 5.0]],
                ..Default::default()
            },
        );
        let text = Shaping {
            left: -20.0,
            right: 20.0,
            top: -10.0,
            bottom: 10.0,
            ..Default::default()
        };
        let icon = fit_icon_to_text(&img, &text, IconTextFit::Both, [0.0; 4], [0.0; 2], 1.0);
        assert_eq!((icon.left, icon.right), (-20.0, 20.0));
        let quads = icon_quads(&icon, &img, 0.0);
        // Three columns (fixed, stretched, fixed) by three rows (the border above, the
        // image, the border below), column by column.
        assert_eq!(quads.len(), 9);
        let left = quads[1];
        // The fixed column is one pixel wide plus the border, whatever the stretch.
        let width = (left.corners[1][0] + left.pixel_offsets[1][0])
            - (left.corners[0][0] + left.pixel_offsets[0][0]);
        assert!((width - 2.0).abs() < 1e-4, "{width}");
        let middle = quads[4];
        assert_eq!(middle.tex[2], 4.0);
        let right_edge = quads[7].corners[1][0] + quads[7].pixel_offsets[1][0];
        assert!((right_edge - 21.0).abs() < 1e-4, "{right_edge}");
    }
}
