//! Repeating images along lines and over areas: `line-dasharray`, `*-pattern`, and the
//! cross-fade between their values at consecutive zoom levels.
//!
//! These "cross-faded" properties aren't interpolated between zoom levels like colors or
//! widths: a dash pattern or an image can't be halfway between two values. MapLibre draws
//! the value of the integer zoom level below the camera and, for a moment after the camera
//! crosses an integer zoom, fades over from the previous one. [`CrossFade`] fades over a
//! short band of zoom instead of time, so that a frame depends on the camera alone: the
//! band just *before* each integer zoom, so that everywhere else, integer zooms included,
//! it draws what MapLibre draws once its fade is done.
//!
//! Dashes are drawn from a [`DashAtlas`]: one row of signed distances per dash pattern,
//! which a line shader samples by the distance along the line. Rows for round caps hold a
//! 2D distance field across the line too, which gives every dash round ends.

use std::collections::HashMap;

use crate::sprite::AtlasImage;

/// How much of a zoom level a cross-fade takes, before the camera reaches an integer zoom.
pub const FADE_ZOOM: f64 = 0.25;

/// Which values of a cross-faded property to draw at a camera zoom, and how to mix them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrossFade {
    /// The integer zoom at or below the camera, whose pixels dashes and patterns are laid
    /// out in (and whose line width they are measured in).
    pub layout_zoom: f64,
    /// The zoom to evaluate the fading-out value at: `layout_zoom`.
    pub from_zoom: f64,
    /// The zoom to evaluate the fading-in value at: the next integer zoom.
    pub to_zoom: f64,
    /// Sizes in pixels of `layout_zoom`: the `to` value is laid out at the next zoom, where
    /// pixels are half as big.
    pub from_scale: f32,
    pub to_scale: f32,
    /// How far the fade has gone: `0` draws only the `from` value, `1` only `to`.
    pub t: f32,
}

impl CrossFade {
    /// The cross-fade for a camera at `zoom`.
    ///
    /// Over the last [`FADE_ZOOM`] before an integer zoom `z`, the value at `z - 1` fades
    /// into the value at `z`, laid out as it will be once the camera reaches `z`, so that
    /// nothing jumps there.
    pub fn at(zoom: f64) -> Self {
        let layout_zoom = zoom.floor();
        let fraction = zoom - layout_zoom;
        Self {
            layout_zoom,
            from_zoom: layout_zoom,
            to_zoom: layout_zoom + 1.0,
            from_scale: 1.0,
            to_scale: 0.5,
            t: ((fraction - (1.0 - FADE_ZOOM)) / FADE_ZOOM).clamp(0.0, 1.0) as f32,
        }
    }

    /// `2^(zoom - layout_zoom)`: how much the map has grown since [`Self::layout_zoom`],
    /// for things laid out in pixels of that zoom.
    pub fn zoom_scale(&self, zoom: f64) -> f32 {
        (zoom - self.layout_zoom).exp2() as f32
    }
}

/// Where a world-anchored pattern (`fill-pattern`, `background-pattern`) starts in a tile.
///
/// Patterns are laid out in pixels of the integer zoom `reference_zoom`, from the world's
/// origin, so they line up across tiles and stay still while panning. The world origin is
/// far away in pixels at high zoom, so this returns the tile's origin in pattern repeats,
/// reduced to `0..1` in double precision, and a shader adds the position in the tile.
///
/// `tile` is `(z, x, y)` (`x` may be outside `0..2^z` for wrapped copies of the world);
/// `tile_size` is the tile's size in pixels at its own zoom; `pattern_size` the size of
/// one repeat in pixels at `reference_zoom`. Returns the origin in repeats and the repeats
/// per tile unit for a tile of `extent` units.
pub fn pattern_placement(
    tile: (u8, i64, u32),
    extent: u32,
    tile_size: f64,
    reference_zoom: f64,
    pattern_size: [f32; 2],
) -> ([f32; 2], [f32; 2]) {
    let (z, x, y) = tile;
    let tile_pixels = tile_size * (reference_zoom - f64::from(z)).exp2();
    let origin = [x as f64 * tile_pixels, f64::from(y) * tile_pixels];
    let mut out_origin = [0.0; 2];
    let mut per_unit = [0.0; 2];
    for i in 0..2 {
        let size = f64::from(pattern_size[i].max(1e-6));
        out_origin[i] = (origin[i] / size).rem_euclid(1.0) as f32;
        per_unit[i] = (tile_pixels / f64::from(extent) / size) as f32;
    }
    (out_origin, per_unit)
}

/// Per-vertex image data for buckets whose pattern or dash array differs per feature: the
/// renderer reads it instead of the layer-wide value. See [`FeatureImage::pattern`] and
/// [`FeatureImage::dash`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FeatureImage {
    pub a: [f32; 4],
    pub b: [f32; 4],
}

impl FeatureImage {
    /// A sprite image: `a` is its `[x, y, width, height]` in atlas pixels, `b.x` its pixel
    /// ratio. All zeros (no image) when the feature's image isn't in the atlas.
    pub fn pattern(image: Option<&AtlasImage>) -> Self {
        let Some(image) = image else {
            return Self::default();
        };
        Self {
            a: image.rect.map(|v| v as f32),
            b: [image.meta.pixel_ratio, 0.0, 0.0, 0.0],
        }
    }

    /// A dash pattern of a [`DashAtlas`]: `a` is [`DashPattern::uniform`]. All zeros (a
    /// solid line) without one.
    pub fn dash(dash: Option<DashPattern>) -> Self {
        Self {
            a: dash.map_or([0.0; 4], |d| d.uniform()),
            b: [0.0; 4],
        }
    }
}

/// Texels per dash pattern repeat.
pub const DASH_ATLAS_WIDTH: u32 = 512;

/// Rows of a dash pattern for round caps: samples across the line from one edge to the other.
pub const ROUND_DASH_ROWS: u32 = 15;

/// The largest signed distance a [`DashAtlas`] stores, in line widths: texel values map
/// `-DASH_SDF_RANGE..DASH_SDF_RANGE` to `0..1`.
pub const DASH_SDF_RANGE: f32 = 2.0;

/// One dash pattern in a [`DashAtlas`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DashPattern {
    /// The first row, in texels.
    pub row: u32,
    /// [`ROUND_DASH_ROWS`] for round caps, otherwise 1.
    pub rows: u32,
    /// The length of one repeat, in line widths.
    pub length: f32,
}

impl DashPattern {
    /// For shaders: the texel row at the center of the line, the texel rows from there to
    /// either edge, and the length of a repeat in line widths.
    pub fn uniform(&self) -> [f32; 4] {
        let half = (self.rows - 1) as f32 / 2.0;
        [self.row as f32 + 0.5 + half, half, self.length, 1.0]
    }
}

/// Dash patterns as rows of signed distance fields, for an R8 texture that is
/// [`DASH_ATLAS_WIDTH`] wide and repeats horizontally. Each row spans one repeat of its
/// pattern; texel values are the distance to the nearest dash edge in line widths (positive
/// inside dashes), mapped by [`DASH_SDF_RANGE`].
#[derive(Debug, Clone, Default)]
pub struct DashAtlas {
    /// `DASH_ATLAS_WIDTH` bytes per row.
    pub data: Vec<u8>,
    patterns: HashMap<(Vec<u32>, bool), DashPattern>,
}

impl DashAtlas {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn width(&self) -> u32 {
        DASH_ATLAS_WIDTH
    }

    pub fn height(&self) -> u32 {
        self.data.len() as u32 / DASH_ATLAS_WIDTH
    }

    /// The rows for `dasharray` (in line widths, alternating dash and gap), adding them if
    /// needed. `None` for patterns that are a solid line: empty, or of zero total length.
    pub fn add(&mut self, dasharray: &[f32], round: bool) -> Option<DashPattern> {
        let dashes: Vec<f32> = dasharray.iter().map(|&d| d.max(0.0)).collect();
        let length: f32 = dashes.iter().sum();
        if !(length > 0.0 && length.is_finite()) {
            return None;
        }
        let key = (dashes.iter().map(|d| d.to_bits()).collect(), round);
        if let Some(&p) = self.patterns.get(&key) {
            return Some(p);
        }
        let rows = if round { ROUND_DASH_ROWS } else { 1 };
        let pattern = DashPattern {
            row: self.height(),
            rows,
            length,
        };
        let intervals = dash_intervals(&dashes, length);
        let width = DASH_ATLAS_WIDTH as usize;
        for r in 0..rows {
            // Across the line in line widths, from the center: -0.5 to 0.5.
            let across = if round {
                (r as f32 / (rows - 1) as f32 - 0.5).abs()
            } else {
                0.0
            };
            for x in 0..width {
                let u = (x as f32 + 0.5) / width as f32 * length;
                let d = if round {
                    0.5 - distance_outside(&intervals, u).hypot(across)
                } else {
                    signed_distance(&intervals, u)
                };
                let v = (d / DASH_SDF_RANGE * 0.5 + 0.5).clamp(0.0, 1.0);
                self.data.push((v * 255.0).round() as u8);
            }
        }
        self.patterns.insert(key, pattern);
        Some(pattern)
    }

    /// The signed distance (in line widths) that a texel value stands for.
    pub fn decode(value: u8) -> f32 {
        (f32::from(value) / 255.0 - 0.5) * 2.0 * DASH_SDF_RANGE
    }
}

/// The dashes of a pattern of total `length`, over three repeats so that every point of
/// the middle one sees its neighbours, merged where they touch. As in MapLibre, an odd
/// number of entries makes the last dash run into the first.
fn dash_intervals(dashes: &[f32], length: f32) -> Vec<[f32; 2]> {
    let mut one = Vec::new();
    let mut at = 0.0;
    for (i, &d) in dashes.iter().enumerate() {
        if i % 2 == 0 {
            one.push([at, at + d]);
        }
        at += d;
    }
    let mut out: Vec<[f32; 2]> = Vec::new();
    for k in [-1.0, 0.0, 1.0] {
        for &[s, e] in &one {
            let [s, e] = [s + k * length, e + k * length];
            match out.last_mut() {
                Some(last) if s <= last[1] => last[1] = last[1].max(e),
                _ => out.push([s, e]),
            }
        }
    }
    out
}

/// How far `u` is from the nearest dash; zero inside one.
fn distance_outside(intervals: &[[f32; 2]], u: f32) -> f32 {
    intervals
        .iter()
        .map(|&[s, e]| (s - u).max(u - e).max(0.0))
        .fold(f32::INFINITY, f32::min)
}

/// The distance from `u` to the nearest dash edge: positive inside a dash. Zero-length
/// dashes count as edges without an inside.
fn signed_distance(intervals: &[[f32; 2]], u: f32) -> f32 {
    let inside = intervals
        .iter()
        .find(|&&[s, e]| s < e && (s..=e).contains(&u));
    match inside {
        Some(&[s, e]) => (u - s).min(e - u),
        None => -distance_outside(intervals, u),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(atlas: &DashAtlas, p: DashPattern, row: u32, u: f32) -> f32 {
        let x = ((u / p.length).rem_euclid(1.0) * DASH_ATLAS_WIDTH as f32) as usize;
        let row = (p.row + row) as usize * DASH_ATLAS_WIDTH as usize;
        DashAtlas::decode(atlas.data[row + x.min(DASH_ATLAS_WIDTH as usize - 1)])
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.03
    }

    #[test]
    fn crossfade_fades_just_before_integer_zooms() {
        let f = CrossFade::at(14.85);
        assert_eq!((f.layout_zoom, f.from_zoom, f.to_zoom), (14.0, 14.0, 15.0));
        assert!((f.t - 0.4).abs() < 1e-5);
        assert_eq!(
            CrossFade::at(14.0).t,
            0.0,
            "integer zooms draw their own value"
        );
        assert_eq!(CrossFade::at(14.7).t, 0.0);
        assert!(CrossFade::at(14.999_999).t > 0.999);
        assert!((CrossFade::at(14.5).zoom_scale(14.5) - 2f32.sqrt()).abs() < 1e-6);
        // Just before 15 the `to` value is laid out at half the size in pixels of 14,
        // which the map has almost doubled: the size it has at 15.
        let f = CrossFade::at(14.999_999);
        assert!((f64::from(f.to_scale * f.zoom_scale(14.999_999)) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn butt_dashes() {
        let mut atlas = DashAtlas::new();
        let p = atlas.add(&[2.0, 1.0], false).unwrap();
        assert_eq!((p.row, p.rows, p.length), (0, 1, 3.0));
        assert_eq!(atlas.height(), 1);
        assert!(close(sample(&atlas, p, 0, 1.0), 1.0), "middle of the dash");
        assert!(close(sample(&atlas, p, 0, 0.25), 0.25));
        assert!(close(sample(&atlas, p, 0, 2.5), -0.5), "middle of the gap");
        // The same pattern again is the same rows.
        assert_eq!(atlas.add(&[2.0, 1.0], false), Some(p));
        assert_eq!(atlas.add(&[0.0, 0.0], false), None);
        assert_eq!(atlas.add(&[], true), None);
    }

    #[test]
    fn round_dashes_have_round_ends() {
        let mut atlas = DashAtlas::new();
        atlas.add(&[1.0], false);
        // Dots: zero-length dashes, drawn as circles one line width across.
        let p = atlas.add(&[0.0, 2.0], true).unwrap();
        assert_eq!((p.row, p.rows), (1, ROUND_DASH_ROWS));
        let center = ROUND_DASH_ROWS / 2;
        assert!(close(sample(&atlas, p, center, 0.0), 0.5), "dot center");
        assert!(
            close(sample(&atlas, p, center, 0.5), 0.0),
            "dot edge along the line"
        );
        assert!(sample(&atlas, p, 0, 0.0).abs() < 0.03, "dot edge across");
        assert!(sample(&atlas, p, center, 1.0) < -0.4, "between dots");
        assert_eq!(p.uniform()[0], 1.0 + 0.5 + 7.0);
    }

    #[test]
    fn odd_dasharrays_join_the_last_dash_to_the_first() {
        let mut atlas = DashAtlas::new();
        let p = atlas.add(&[1.0, 1.0, 1.0], false).unwrap();
        // Dash 0..1, gap 1..2, dash 2..3 continuing into the next 0..1.
        assert!(close(sample(&atlas, p, 0, 2.95), 0.95));
        assert!(close(sample(&atlas, p, 0, 1.5), -0.5));
    }

    #[test]
    fn pattern_placement_is_continuous_across_tiles() {
        // Two neighbouring z14 tiles, laid out at z15, with a 24 pixel pattern.
        let size = [24.0, 24.0];
        let (a, per_unit) = pattern_placement((14, 100, 7), 4096, 512.0, 15.0, size);
        let (b, _) = pattern_placement((14, 101, 7), 4096, 512.0, 15.0, size);
        let end_of_a = a[0] + per_unit[0] * 4096.0;
        assert!(((end_of_a - b[0]).rem_euclid(1.0)).min((b[0] - end_of_a).rem_euclid(1.0)) < 1e-3);
        assert!((per_unit[1] - 1024.0 / 4096.0 / 24.0).abs() < 1e-9);
    }
}
