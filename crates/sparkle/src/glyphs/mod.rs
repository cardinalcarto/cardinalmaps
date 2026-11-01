//! Glyphs for text: shaping and signed distance fields from local fonts.
//!
//! sparkle draws text from glyphs rasterized as signed distance fields from local TTF/OTF
//! files, never from glyph PBFs. Symbol layout asks a [`GlyphProvider`] (passed in
//! [`TileResources`](crate::tile::TileResources)) to shape runs of text in a style's
//! fontstack and for the distance fields of the glyphs that shaping picked;
//! [`FontLibrary`] is the implementation that finds and reads font files.
//!
//! Every length here is in pixels at a font size of [`ONE_EM`], MapLibre's glyph size,
//! which renderers scale by `text-size / ONE_EM`.

mod fonts;
mod sdf;

use std::collections::HashMap;
use std::sync::Arc;

pub use fonts::FontLibrary;

/// The font size glyphs are shaped and rasterized at, in pixels.
pub const ONE_EM: f32 = 24.0;
/// Pixels of distance field around each glyph's outline bounds.
pub const SDF_BUFFER: u32 = 3;
/// Pixels from the outline at which the distance field saturates.
pub const SDF_RADIUS: f32 = 8.0;
/// Where the outline is in the distance field: at `1 - SDF_CUTOFF` (`0.75`, byte 192).
pub const SDF_CUTOFF: f32 = 0.25;

/// A glyph of one of a provider's fonts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphKey {
    /// The provider's identifier of the font (a file at some variation coordinates).
    pub font: u32,
    pub glyph: u16,
}

/// One glyph of a shaped run of text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub key: GlyphKey,
    /// The byte offset in the shaped text of the first character this glyph draws.
    pub cluster: u32,
    /// How far the pen moves after this glyph.
    pub advance: f32,
    /// Where to draw the glyph relative to the pen, `y` down.
    pub offset: [f32; 2],
}

/// A glyph's signed distance field, one byte per pixel, rows top down. See
/// [`SDF_CUTOFF`] and [`SDF_RADIUS`] for what the values mean.
#[derive(Debug, Clone, PartialEq)]
pub struct SdfGlyph {
    /// Including [`SDF_BUFFER`] on each side.
    pub width: u32,
    pub height: u32,
    /// From the pen to the bitmap's left edge (buffer included).
    pub left: f32,
    /// From the baseline up to the bitmap's top edge (buffer included).
    pub top: f32,
    pub pixels: Vec<u8>,
}

/// Vertical metrics of a font, positive upwards from the baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    pub ascender: f32,
    pub descender: f32,
    /// The height of capital letters, which labels are centered on.
    pub cap_height: f32,
}

impl Default for FontMetrics {
    /// Typical values, for when no font is found.
    fn default() -> Self {
        Self {
            ascender: 0.9 * ONE_EM,
            descender: -0.25 * ONE_EM,
            cap_height: 0.7 * ONE_EM,
        }
    }
}

/// What symbol layout needs from fonts. Implementations are shared by every tile worker.
pub trait GlyphProvider: Send + Sync {
    /// Shape `text`, a run of one direction (`rtl` for right-to-left), with the fonts that
    /// `fontstack` names (comma separated, as in `text-font` and `format` sections),
    /// appending its glyphs to `out` in visual order, left to right. Characters no font of
    /// the stack has come from any other font that has them; characters no font has are
    /// left out.
    fn shape(&self, fontstack: &str, text: &str, rtl: bool, out: &mut Vec<ShapedGlyph>);

    /// The distance field of a glyph that [`Self::shape`] returned, or `None` for glyphs
    /// with nothing to draw, such as spaces.
    fn glyph(&self, key: GlyphKey) -> Option<Arc<SdfGlyph>>;

    /// The metrics of the first font of `fontstack` that can be found.
    fn metrics(&self, fontstack: &str) -> FontMetrics;
}

/// The glyphs one bucket draws, packed into a single-channel texture.
#[derive(Debug, Clone, Default)]
pub struct GlyphAtlas {
    pub width: u32,
    pub height: u32,
    /// One byte per pixel, rows top down.
    pub pixels: Vec<u8>,
}

/// Collects the glyphs a bucket uses, each once, to pack them at the end.
#[derive(Default)]
pub(crate) struct GlyphAtlasBuilder {
    slots: HashMap<GlyphKey, u32>,
    glyphs: Vec<Arc<SdfGlyph>>,
}

impl GlyphAtlasBuilder {
    /// The slot of `key`, whose distance field is `glyph`.
    pub fn slot(&mut self, key: GlyphKey, glyph: &Arc<SdfGlyph>) -> u32 {
        *self.slots.entry(key).or_insert_with(|| {
            self.glyphs.push(glyph.clone());
            self.glyphs.len() as u32 - 1
        })
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.glyphs.clear();
    }

    /// Pack the glyphs: the atlas and the top left corner of each slot in it.
    pub fn build(&self) -> (GlyphAtlas, Vec<[u32; 2]>) {
        // A pixel of padding keeps linear filtering from reading neighbours.
        const PADDING: u32 = 1;
        let mut order: Vec<usize> = (0..self.glyphs.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(self.glyphs[i].height));
        let sizes: Vec<_> = order
            .iter()
            .map(|&i| {
                let g = &self.glyphs[i];
                [g.width + 2 * PADDING, g.height + 2 * PADDING]
            })
            .collect();
        let ([width, height], placed) = crate::sprite::pack(&sizes);
        let mut pixels = vec![0; (width * height) as usize];
        let mut origins = vec![[0; 2]; self.glyphs.len()];
        for (&i, [x, y]) in order.iter().zip(placed) {
            let g = &self.glyphs[i];
            let (x, y) = (x + PADDING, y + PADDING);
            for row in 0..g.height {
                let src = (row * g.width) as usize;
                let dst = ((y + row) * width + x) as usize;
                pixels[dst..dst + g.width as usize]
                    .copy_from_slice(&g.pixels[src..src + g.width as usize]);
            }
            origins[i] = [x, y];
        }
        (
            GlyphAtlas {
                width,
                height,
                pixels,
            },
            origins,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(width: u32, height: u32, value: u8) -> Arc<SdfGlyph> {
        Arc::new(SdfGlyph {
            width,
            height,
            left: 0.0,
            top: 0.0,
            pixels: vec![value; (width * height) as usize],
        })
    }

    #[test]
    fn atlas_packs_each_glyph_once() {
        let mut b = GlyphAtlasBuilder::default();
        let a = GlyphKey { font: 0, glyph: 1 };
        let c = GlyphKey { font: 1, glyph: 1 };
        assert_eq!(b.slot(a, &glyph(4, 6, 10)), 0);
        assert_eq!(b.slot(c, &glyph(5, 3, 20)), 1);
        assert_eq!(b.slot(a, &glyph(4, 6, 10)), 0);
        let (atlas, origins) = b.build();
        for (slot, (w, h, v)) in [(4, 6, 10), (5, 3, 20)].into_iter().enumerate() {
            let [x, y] = origins[slot];
            assert!(x + w <= atlas.width && y + h <= atlas.height);
            for row in y..y + h {
                for col in x..x + w {
                    assert_eq!(atlas.pixels[(row * atlas.width + col) as usize], v);
                }
            }
        }
    }
}
