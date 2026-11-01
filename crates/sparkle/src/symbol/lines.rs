//! Where labels go along lines, for `symbol-placement: line` and `line-center`: anchors
//! spaced by `symbol-spacing`, kept off corners sharper than `text-max-angle`, as
//! MapLibre's `getAnchors` and `getCenterAnchor` place them.

use std::f32::consts::PI;

/// A point on a line that a label is centered on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineAnchor {
    pub point: [f32; 2],
    /// Index of the segment (from `line[segment]` to `line[segment + 1]`) it's on.
    pub segment: usize,
}

/// What [`line_anchors`] and [`center_anchor`] need to know about a label, in tile units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelExtent {
    /// The label's length along the line.
    pub length: f32,
    /// How far along the line corners add up against `max_angle`; zero for labels
    /// without text, which may sit on any corner.
    pub angle_window: f32,
    /// In radians: the most the line may turn within `angle_window`.
    pub max_angle: f32,
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn line_length(line: &[[f32; 2]]) -> f32 {
    line.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// The angle of the direction from `b` to `a`, as MapLibre's `a.angleTo(b)`.
fn angle_to(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[1] - b[1]).atan2(a[0] - b[0])
}

/// Split `line` into the parts inside `min..max` (both axes), as MapLibre's `clipLine`.
pub fn clip_line(line: &[[f32; 2]], min: f32, max: f32, out: &mut Vec<Vec<[f32; 2]>>) {
    let mut current: Option<usize> = None;
    for w in line.windows(2) {
        let (mut p0, mut p1) = (w[0], w[1]);
        // Clip against each of the four edges in turn: x >= min, y >= min, x <= max,
        // y <= max.
        let mut inside = true;
        for (axis, bound, keep_above) in [
            (0, min, true),
            (1, min, true),
            (0, max, false),
            (1, max, false),
        ] {
            let other = 1 - axis;
            let outside = |p: [f32; 2]| {
                if keep_above {
                    p[axis] < bound
                } else {
                    p[axis] > bound
                }
            };
            match (outside(p0), outside(p1)) {
                (true, true) => {
                    inside = false;
                    break;
                }
                (true, false) | (false, true) => {
                    let t = (bound - p0[axis]) / (p1[axis] - p0[axis]);
                    let mut p = [0.0; 2];
                    p[axis] = bound;
                    p[other] = p0[other] + (p1[other] - p0[other]) * t;
                    if outside(p0) {
                        p0 = p;
                    } else {
                        p1 = p;
                    }
                }
                (false, false) => {}
            }
        }
        if !inside {
            continue;
        }
        let continues = current.is_some_and(|i| out[i].last() == Some(&p0));
        if !continues {
            out.push(vec![p0]);
            current = Some(out.len() - 1);
        }
        out[current.unwrap()].push(p1);
    }
}

/// Whether the line turns too much under a label of `label_length` centered on `anchor`:
/// MapLibre's `checkMaxAngle`.
fn check_max_angle(line: &[[f32; 2]], anchor: &LineAnchor, label: &LabelExtent) -> bool {
    if label.length == 0.0 {
        return true;
    }
    let half = label.length / 2.0;
    // Back along the line to the first segment the label is on.
    let mut index = anchor.segment as isize + 1;
    let mut p = anchor.point;
    let mut anchor_distance = 0.0;
    while anchor_distance > -half {
        index -= 1;
        if index < 0 {
            // Not enough room before the start of the line.
            return false;
        }
        anchor_distance -= dist(line[index as usize], p);
        p = line[index as usize];
    }
    let mut index = index as usize;
    anchor_distance += dist(line[index], line[index + 1]);
    index += 1;

    // Then forward over the label's length, summing the corners within the window.
    let mut recent: std::collections::VecDeque<(f32, f32)> = Default::default();
    let mut recent_delta = 0.0;
    while anchor_distance < half {
        let Some(&next) = line.get(index + 1) else {
            // Not enough room before the end of the line.
            return false;
        };
        let (prev, current) = (line[index - 1], line[index]);
        let delta = angle_to(prev, current) - angle_to(current, next);
        let delta = ((delta + 3.0 * PI).rem_euclid(2.0 * PI) - PI).abs();
        recent.push_back((anchor_distance, delta));
        recent_delta += delta;
        while let Some(&(d, a)) = recent.front()
            && anchor_distance - d > label.angle_window
        {
            recent_delta -= a;
            recent.pop_front();
        }
        if recent_delta > label.max_angle {
            return false;
        }
        index += 1;
        anchor_distance += dist(current, next);
    }
    true
}

/// Anchors every `spacing` along `line` (tile units) where `label` fits, for
/// `symbol-placement: line`. `extent` is the tile's: anchors outside it belong to
/// neighbouring tiles. `overscaling` is how many times the tile is magnified beyond its
/// zoom (1 when not).
pub fn line_anchors(
    line: &[[f32; 2]],
    mut spacing: f32,
    label: &LabelExtent,
    glyph_size: f32,
    overscaling: f32,
    extent: f32,
) -> Vec<LineAnchor> {
    if line.len() < 2 {
        return Vec::new();
    }
    let first = line[0];
    let continued = first[0] == 0.0 || first[0] == extent || first[1] == 0.0 || first[1] == extent;
    // Long labels need room between them: at least a quarter of the spacing.
    if spacing - label.length < spacing / 4.0 {
        spacing = label.length + spacing / 4.0;
    }
    if spacing <= 0.0 {
        return Vec::new();
    }
    // Lines that start in this tile get their first label after half its length plus a
    // little, to keep away from T intersections; lines continuing from a neighbouring
    // tile get theirs half a spacing in, so labels line up across the tile edge.
    let fixed_extra = glyph_size * 2.0;
    let offset = if continued {
        (spacing / 2.0 * overscaling) % spacing
    } else {
        ((label.length / 2.0 + fixed_extra) * overscaling) % spacing
    };
    resample(line, offset, spacing, label, continued, false, extent)
}

fn resample(
    line: &[[f32; 2]],
    offset: f32,
    spacing: f32,
    label: &LabelExtent,
    continued: bool,
    middle: bool,
    extent: f32,
) -> Vec<LineAnchor> {
    let half = label.length / 2.0;
    let total = line_length(line);
    let mut distance = 0.0;
    let mut marked = offset - spacing;
    let mut anchors = Vec::new();
    for (i, w) in line.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let segment = dist(a, b);
        while marked + spacing < distance + segment {
            marked += spacing;
            let t = (marked - distance) / segment;
            let point = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            let in_tile = (0.0..extent).contains(&point[0]) && (0.0..extent).contains(&point[1]);
            if in_tile && marked - half >= 0.0 && marked + half <= total {
                let anchor = LineAnchor {
                    point: point.map(f32::round),
                    segment: i,
                };
                if label.angle_window == 0.0 || check_max_angle(line, &anchor, label) {
                    anchors.push(anchor);
                }
            }
        }
        distance += segment;
    }
    if !middle && anchors.is_empty() && !continued {
        // Nothing fits at the regular spacing; try one label in the middle, which helps
        // short lines most.
        anchors = resample(
            line,
            distance / 2.0,
            spacing,
            label,
            continued,
            true,
            extent,
        );
    }
    anchors
}

/// The anchor in the middle of `line`, if the label fits there, for
/// `symbol-placement: line-center`.
pub fn center_anchor(line: &[[f32; 2]], label: &LabelExtent) -> Option<LineAnchor> {
    let center = line_length(line) / 2.0;
    let mut prev = 0.0;
    for (i, w) in line.windows(2).enumerate() {
        let segment = dist(w[0], w[1]);
        if prev + segment > center {
            let t = (center - prev) / segment;
            let point = [
                w[0][0] + (w[1][0] - w[0][0]) * t,
                w[0][1] + (w[1][1] - w[0][1]) * t,
            ];
            let anchor = LineAnchor {
                point: point.map(f32::round),
                segment: i,
            };
            return (label.angle_window == 0.0 || check_max_angle(line, &anchor, label))
                .then_some(anchor);
        }
        prev += segment;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(length: f32) -> LabelExtent {
        LabelExtent {
            length,
            angle_window: 0.0,
            max_angle: PI,
        }
    }

    #[test]
    fn clipping_splits_lines_at_the_box() {
        let mut out = Vec::new();
        clip_line(
            &[
                [-10.0, 5.0],
                [5.0, 5.0],
                [5.0, 20.0],
                [5.0, 30.0],
                [8.0, 8.0],
            ],
            0.0,
            10.0,
            &mut out,
        );
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0], [[0.0, 5.0], [5.0, 5.0], [5.0, 10.0]]);
        assert_eq!(out[1].last(), Some(&[8.0, 8.0]));
        assert_eq!(out[1][0][1], 10.0);
    }

    #[test]
    fn anchors_are_spaced_and_fit_the_label() {
        let line = [[0.0, 100.0], [1000.0, 100.0]];
        // Starts at the tile's edge, so the first anchor is half a spacing in.
        let anchors = line_anchors(&line, 200.0, &label(50.0), 24.0, 1.0, 4096.0);
        let xs: Vec<f32> = anchors.iter().map(|a| a.point[0]).collect();
        assert_eq!(xs, [100.0, 300.0, 500.0, 700.0, 900.0]);

        // A label longer than the line fits nowhere.
        assert!(line_anchors(&line, 200.0, &label(1200.0), 24.0, 1.0, 4096.0).is_empty());

        // A short line that starts inside the tile gets one label in its middle.
        let short = [[100.0, 100.0], [160.0, 100.0]];
        let anchors = line_anchors(&short, 200.0, &label(40.0), 24.0, 1.0, 4096.0);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].point, [130.0, 100.0]);
    }

    #[test]
    fn sharp_corners_reject_anchors() {
        // A hairpin at x = 100.
        let line = [[0.0, 0.0], [100.0, 0.0], [0.0, 10.0]];
        let mut extent = label(60.0);
        extent.angle_window = 30.0;
        extent.max_angle = 45f32.to_radians();
        let near_corner = LineAnchor {
            point: [90.0, 0.0],
            segment: 0,
        };
        assert!(!check_max_angle(&line, &near_corner, &extent));
        let far = LineAnchor {
            point: [40.0, 0.0],
            segment: 0,
        };
        assert!(check_max_angle(&line, &far, &extent));
        assert!(center_anchor(&line, &extent).is_none());
        extent.max_angle = PI;
        assert_eq!(center_anchor(&line, &extent).unwrap().segment, 1);
    }
}
