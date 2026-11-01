//! Projecting labels with the camera, and laying glyphs out along lines as they appear in
//! the plane labels are drawn in: a port of MapLibre's `symbol_projection`.

use std::f32::consts::PI;

pub use crate::projection::TileProjection;

/// A clip space position in logical pixels from the top left of a viewport of `size`.
pub fn to_screen(clip: [f32; 4], size: [f32; 2]) -> [f32; 2] {
    [
        (clip[0] / clip[3] + 1.0) / 2.0 * size[0],
        (1.0 - clip[1] / clip[3]) / 2.0 * size[1],
    ]
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn len(v: [f32; 2]) -> f32 {
    v[0].hypot(v[1])
}

/// The plane glyphs along a line are laid out in: the map itself (tile units) for text
/// pitched with the map, or the screen (logical pixels) for text facing the viewer.
#[derive(Clone, Copy)]
pub enum LabelPlane<'a> {
    Tile,
    Screen {
        projection: &'a TileProjection,
        size: [f32; 2],
    },
}

impl LabelPlane<'_> {
    /// `p` in the plane, or `None` when it's behind the camera.
    fn project(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        match *self {
            LabelPlane::Tile => Some(p),
            LabelPlane::Screen { projection, size } => {
                let clip = projection.project(p);
                (clip[3] > 0.0).then(|| to_screen(clip, size))
            }
        }
    }

    /// A point `min_length` along the direction from `prev_tile` to `current_tile`, past
    /// `prev` (`prev_tile` in the plane), for when `current_tile` is behind the camera.
    fn truncated(
        &self,
        prev_tile: [f32; 2],
        current_tile: [f32; 2],
        prev: [f32; 2],
        min_length: f32,
    ) -> [f32; 2] {
        let back = sub(prev_tile, current_tile);
        let l = len(back).max(1e-6);
        let behind = [prev_tile[0] + back[0] / l, prev_tile[1] + back[1] / l];
        let projected = self.project(behind).unwrap_or(prev);
        let d = sub(prev, projected);
        let dl = len(d).max(1e-6);
        [
            prev[0] + d[0] * min_length / dl,
            prev[1] + d[1] * min_length / dl,
        ]
    }
}

/// A glyph placed along a line, in the label plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    pub point: [f32; 2],
    /// Of the line where the glyph is, in radians (`y` down), turned by half a turn when
    /// the label is flipped.
    pub angle: f32,
}

/// One label's line in the label plane, with the projected vertices cached.
pub struct LineProjection<'a> {
    pub plane: LabelPlane<'a>,
    /// The whole line, in tile units.
    pub line: &'a [[f32; 2]],
    /// The label's anchor in tile units and in the plane.
    pub anchor_tile: [f32; 2],
    pub anchor: [f32; 2],
    /// The anchor is between `line[segment]` and `line[segment + 1]`.
    pub segment: usize,
    /// Flip labels that would read right to left on screen (`text-keep-upright`).
    pub keep_upright: bool,
    /// Turn glyphs with the line; otherwise their angle is zero.
    pub rotate_to_line: bool,
    cache: Vec<Option<[f32; 2]>>,
}

/// Why a label doesn't fit on its line as it's projected now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineFit {
    /// The line ends before the label does.
    NotEnoughRoom,
    /// It would be upside down; place it again flipped.
    NeedsFlipping,
}

impl<'a> LineProjection<'a> {
    pub fn new(
        plane: LabelPlane<'a>,
        line: &'a [[f32; 2]],
        anchor_tile: [f32; 2],
        anchor: [f32; 2],
        segment: usize,
    ) -> Self {
        Self {
            plane,
            line,
            anchor_tile,
            anchor,
            segment,
            keep_upright: false,
            rotate_to_line: true,
            cache: vec![None; line.len()],
        }
    }

    /// The glyph `offset` along the line from the anchor (plane units, negative is
    /// backwards), moved by `line_offset` (along, across), as MapLibre's
    /// `placeGlyphAlongLine`.
    fn place_glyph(
        &mut self,
        offset: f32,
        line_offset: [f32; 2],
        flip: bool,
    ) -> Option<PlacedGlyph> {
        let combined = if flip {
            offset - line_offset[0]
        } else {
            offset + line_offset[0]
        };
        let mut dir: isize = if combined > 0.0 { 1 } else { -1 };
        let mut angle = 0.0;
        if flip {
            // Walk the line the other way to keep the text upright.
            dir = -dir;
            angle = PI;
        }
        if dir < 0 {
            angle += PI;
        }
        let mut index = self.segment as isize + if dir > 0 { 0 } else { 1 };
        let (mut current, mut prev) = (self.anchor, self.anchor);
        let mut distance_to_prev = 0.0;
        let mut segment_distance = 0.0;
        let target = combined.abs();
        while distance_to_prev + segment_distance <= target {
            index += dir;
            if index < 0 || index as usize >= self.line.len() {
                return None;
            }
            let i = index as usize;
            prev = current;
            current = match self.cache[i] {
                Some(p) => p,
                None => match self.plane.project(self.line[i]) {
                    Some(p) => *self.cache[i].insert(p),
                    None => {
                        // Behind the camera: use a point along the segment far enough out
                        // for this glyph, without caching it.
                        let prev_tile = if distance_to_prev == 0.0 {
                            self.anchor_tile
                        } else {
                            self.line[(index - dir) as usize]
                        };
                        let min_length = target - distance_to_prev + 1.0;
                        self.plane
                            .truncated(prev_tile, self.line[i], prev, min_length)
                    }
                },
            };
            distance_to_prev += segment_distance;
            segment_distance = len(sub(current, prev));
        }
        let t = (target - distance_to_prev) / segment_distance;
        let d = sub(current, prev);
        let mut p = [prev[0] + d[0] * t, prev[1] + d[1] * t];
        let unit = [d[0] / segment_distance, d[1] / segment_distance];
        let across = line_offset[1] * dir as f32;
        p[0] += -unit[1] * across;
        p[1] += unit[0] * across;
        let segment_angle = angle + d[1].atan2(d[0]);
        Some(PlacedGlyph {
            point: p,
            angle: if self.rotate_to_line {
                segment_angle
            } else {
                0.0
            },
        })
    }

    /// Place glyphs at `offsets` (plane units, in reading order) along the line, as
    /// MapLibre's `placeGlyphsAlongLine`. With [`Self::keep_upright`], a label that would
    /// read right to left on screen (`to_screen` maps the plane to the screen) needs
    /// flipping.
    pub fn place_glyphs(
        &mut self,
        offsets: &[f32],
        line_offset: [f32; 2],
        flip: bool,
        to_screen: impl Fn([f32; 2]) -> Option<[f32; 2]>,
        out: &mut Vec<PlacedGlyph>,
    ) -> Result<(), LineFit> {
        out.clear();
        let (Some(&first), Some(&last)) = (offsets.first(), offsets.last()) else {
            return Ok(());
        };
        let check = self.keep_upright && !flip;
        if offsets.len() > 1 {
            let first = self
                .place_glyph(first, line_offset, flip)
                .ok_or(LineFit::NotEnoughRoom)?;
            let last = self
                .place_glyph(last, line_offset, flip)
                .ok_or(LineFit::NotEnoughRoom)?;
            if check {
                let (a, b) = (to_screen(first.point), to_screen(last.point));
                if let (Some(a), Some(b)) = (a, b)
                    && a[0] > b[0]
                {
                    return Err(LineFit::NeedsFlipping);
                }
            }
            out.push(first);
            for &offset in &offsets[1..offsets.len() - 1] {
                // The first and last glyphs fit, so every glyph between them does.
                let glyph = self.place_glyph(offset, line_offset, flip);
                out.push(glyph.ok_or(LineFit::NotEnoughRoom)?);
            }
            out.push(last);
        } else {
            if check {
                // Which way the anchor's segment runs on screen.
                let end = self.line.get(self.segment + 1).copied();
                let a = to_screen(self.anchor);
                let b = end.and_then(|e| self.plane.project(e)).and_then(&to_screen);
                if let (Some(a), Some(b)) = (a, b)
                    && a[0] > b[0]
                {
                    return Err(LineFit::NeedsFlipping);
                }
            }
            let glyph = self.place_glyph(first, line_offset, flip);
            out.push(glyph.ok_or(LineFit::NotEnoughRoom)?);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    fn same_angle(a: f32, b: f32) -> bool {
        let d = (a - b).rem_euclid(2.0 * PI);
        d < 1e-4 || 2.0 * PI - d < 1e-4
    }

    #[test]
    fn glyphs_follow_a_bent_line() {
        // Right 10, then down 10.
        let line = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        let mut proj = LineProjection::new(LabelPlane::Tile, &line, [5.0, 0.0], [5.0, 0.0], 0);
        let mut out = Vec::new();
        proj.place_glyphs(&[-2.0, 0.0, 8.0], [0.0; 2], false, Some, &mut out)
            .unwrap();
        assert!(close(out[0].point, [3.0, 0.0]), "{out:?}");
        assert!(close(out[1].point, [5.0, 0.0]), "{out:?}");
        assert!(close(out[2].point, [10.0, 3.0]), "{out:?}");
        assert!((out[2].angle - PI / 2.0).abs() < 1e-5);
        // Glyphs before the anchor walk backwards, half a turn from the line's direction
        // in the other direction: right way up.
        assert!(same_angle(out[0].angle, 0.0), "{out:?}");

        // Past the end of the line.
        let r = proj.place_glyphs(&[-6.0, 0.0], [0.0; 2], false, Some, &mut out);
        assert_eq!(r, Err(LineFit::NotEnoughRoom));
    }

    #[test]
    fn upside_down_labels_flip() {
        // The line runs right to left.
        let line = [[10.0, 0.0], [0.0, 0.0]];
        let mut proj = LineProjection::new(LabelPlane::Tile, &line, [5.0, 0.0], [5.0, 0.0], 0);
        let mut out = Vec::new();
        let offsets = [-2.0, 2.0];
        proj.keep_upright = true;
        let r = proj.place_glyphs(&offsets, [0.0; 2], false, Some, &mut out);
        assert_eq!(r, Err(LineFit::NeedsFlipping));
        proj.place_glyphs(&offsets, [0.0; 2], true, Some, &mut out)
            .unwrap();
        // Flipped, the first glyph is on the left and the text upright.
        assert!(close(out[0].point, [3.0, 0.0]), "{out:?}");
        assert!(close(out[1].point, [7.0, 0.0]), "{out:?}");
        assert!(same_angle(out[0].angle, 0.0), "{out:?}");
    }

    #[test]
    fn offsets_move_across_the_line() {
        let line = [[0.0, 0.0], [10.0, 0.0]];
        let mut proj = LineProjection::new(LabelPlane::Tile, &line, [5.0, 0.0], [5.0, 0.0], 0);
        let mut out = Vec::new();
        proj.place_glyphs(&[1.0], [1.0, 2.0], false, Some, &mut out)
            .unwrap();
        // Along by 1 + 1; across is to the right of the direction of travel, `y` down.
        assert!(close(out[0].point, [7.0, 2.0]), "{out:?}");
    }
}
