//! Signed distance fields from glyph outlines.
//!
//! MapLibre's text shader expects glyphs as 8-bit distance fields rendered at
//! [`ONE_EM`] pixels per em, with a [`SDF_BUFFER`] pixel border, where a value of 192
//! (`0.75`) is the outline and each step of `255 / SDF_RADIUS` is one pixel further in or
//! out (TinySDF's convention, and that of glyph PBFs). Rather than rasterizing and then
//! estimating distances from coverage as TinySDF does, this measures them exactly from the
//! flattened outline.

use rustybuzz::ttf_parser::{self, GlyphId, OutlineBuilder};

use super::{ONE_EM, SDF_BUFFER, SDF_CUTOFF, SDF_RADIUS, SdfGlyph};

/// Curves are flattened until their chords are this close to them, in pixels.
const FLATNESS: f32 = 0.05;

/// The distance field of `glyph` in `face`, or `None` for glyphs without an outline
/// (spaces) or faces without one (bitmap-only fonts).
pub(crate) fn rasterize(face: &ttf_parser::Face<'_>, glyph: GlyphId) -> Option<SdfGlyph> {
    let scale = ONE_EM / f32::from(face.units_per_em());
    let mut outline = Outline {
        scale,
        segments: Vec::new(),
        start: [0.0; 2],
        last: [0.0; 2],
    };
    face.outline_glyph(glyph, &mut outline)?;
    Some(outline.distance_field())
}

/// An outline as line segments in pixels, `y` up.
struct Outline {
    scale: f32,
    segments: Vec<[[f32; 2]; 2]>,
    start: [f32; 2],
    last: [f32; 2],
}

impl Outline {
    fn point(&self, x: f32, y: f32) -> [f32; 2] {
        [x * self.scale, y * self.scale]
    }

    fn line(&mut self, to: [f32; 2]) {
        if to != self.last {
            self.segments.push([self.last, to]);
        }
        self.last = to;
    }

    /// Flatten a cubic Bézier (quadratics are raised to cubics) from the current point.
    fn cubic(&mut self, c1: [f32; 2], c2: [f32; 2], to: [f32; 2]) {
        let p0 = self.last;
        // The control polygon's deviation from the chord bounds the curve's; each halving
        // of the step divides it by four.
        let dev = |c: [f32; 2]| {
            let a = [c[0] - p0[0], c[1] - p0[1]];
            let b = [to[0] - p0[0], to[1] - p0[1]];
            let len = (b[0] * b[0] + b[1] * b[1]).sqrt().max(1e-6);
            (a[0] * b[1] - a[1] * b[0]).abs() / len
        };
        let deviation = dev(c1).max(dev(c2));
        let steps = ((deviation / FLATNESS).sqrt().ceil() as usize).clamp(1, 64);
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.line([
                a * p0[0] + b * c1[0] + c * c2[0] + d * to[0],
                a * p0[1] + b * c1[1] + c * c2[1] + d * to[1],
            ]);
        }
    }

    fn distance_field(&self) -> SdfGlyph {
        let buffer = SDF_BUFFER as i32;
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in self.segments.iter().flatten() {
            for i in 0..2 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        if self.segments.is_empty() {
            (min, max) = ([0.0; 2], [0.0; 2]);
        }
        let (x0, y1) = (min[0].floor() as i32, max[1].ceil() as i32);
        let width = (max[0].ceil() as i32 - x0).max(0) + 2 * buffer;
        let height = (y1 - min[1].floor() as i32).max(0) + 2 * buffer;
        let left = (x0 - buffer) as f32;
        let top = (y1 + buffer) as f32;

        let mut pixels = Vec::with_capacity((width * height) as usize);
        for row in 0..height {
            let y = top - row as f32 - 0.5;
            for col in 0..width {
                let p = [left + col as f32 + 0.5, y];
                let d = self.signed_distance(p);
                let v = 255.0 * (1.0 - SDF_CUTOFF + d / SDF_RADIUS);
                pixels.push(v.round().clamp(0.0, 255.0) as u8);
            }
        }
        SdfGlyph {
            width: width as u32,
            height: height as u32,
            left,
            top,
            pixels,
        }
    }

    /// Distance to the outline in pixels, positive inside (non-zero winding).
    fn signed_distance(&self, p: [f32; 2]) -> f32 {
        let mut best = f32::MAX;
        let mut winding = 0;
        for &[a, b] in &self.segments {
            let ab = [b[0] - a[0], b[1] - a[1]];
            let ap = [p[0] - a[0], p[1] - a[1]];
            let len2 = ab[0] * ab[0] + ab[1] * ab[1];
            let t = if len2 > 0.0 {
                ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let d = [ap[0] - ab[0] * t, ap[1] - ab[1] * t];
            best = best.min(d[0] * d[0] + d[1] * d[1]);

            let cross = ab[0] * ap[1] - ab[1] * ap[0];
            if a[1] <= p[1] && b[1] > p[1] && cross > 0.0 {
                winding += 1;
            } else if b[1] <= p[1] && a[1] > p[1] && cross < 0.0 {
                winding -= 1;
            }
        }
        let d = best.sqrt();
        if winding != 0 { d } else { -d }
    }
}

impl OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = self.point(x, y);
        self.last = self.start;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.point(x, y);
        self.line(p);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, to, from) = (self.point(x1, y1), self.point(x, y), self.last);
        let lerp = |a: [f32; 2], b: [f32; 2]| {
            [
                a[0] + (b[0] - a[0]) * 2.0 / 3.0,
                a[1] + (b[1] - a[1]) * 2.0 / 3.0,
            ]
        };
        self.cubic(lerp(from, c), lerp(to, c), to);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1, c2, to) = (self.point(x1, y1), self.point(x2, y2), self.point(x, y));
        self.cubic(c1, c2, to);
    }

    fn close(&mut self) {
        let start = self.start;
        self.line(start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10 by 10 pixel square with its lower left corner on the origin.
    fn square() -> Outline {
        let mut o = Outline {
            scale: 1.0,
            segments: Vec::new(),
            start: [0.0; 2],
            last: [0.0; 2],
        };
        o.move_to(0.0, 0.0);
        o.line_to(10.0, 0.0);
        o.line_to(10.0, 10.0);
        o.line_to(0.0, 10.0);
        o.close();
        o
    }

    #[test]
    fn distances_are_signed() {
        let o = square();
        assert_eq!(o.signed_distance([5.0, 5.0]), 5.0);
        assert_eq!(o.signed_distance([5.0, 12.0]), -2.0);
        assert_eq!(o.signed_distance([1.0, 5.0]), 1.0);
    }

    #[test]
    fn field_has_buffer_and_edge_value() {
        let g = square().distance_field();
        let b = SDF_BUFFER;
        assert_eq!((g.width, g.height), (10 + 2 * b, 10 + 2 * b));
        assert_eq!((g.left, g.top), (-(b as f32), 10.0 + b as f32));
        // Half a pixel inside the left edge, mid height.
        let row = (g.height / 2) as usize;
        let at = |col: u32| g.pixels[row * g.width as usize + col as usize];
        let inside = at(b);
        let outside = at(b - 1);
        let edge = 255.0 * (1.0 - SDF_CUTOFF);
        assert!(f32::from(inside) > edge && f32::from(outside) < edge);
        assert_eq!(
            f32::from(inside) - f32::from(outside),
            (255.0 / SDF_RADIUS).round()
        );
    }

    #[test]
    fn curves_are_flattened_closely() {
        let mut o = Outline {
            scale: 1.0,
            segments: Vec::new(),
            start: [0.0; 2],
            last: [0.0; 2],
        };
        o.move_to(0.0, 0.0);
        o.quad_to(10.0, 20.0, 20.0, 0.0);
        o.close();
        // The quadratic's apex is at (10, 10).
        let top = o
            .segments
            .iter()
            .flatten()
            .map(|p| p[1])
            .fold(f32::MIN, f32::max);
        assert!((top - 10.0).abs() < FLATNESS * 2.0, "{top}");
        assert!(o.segments.len() > 4);
    }
}
