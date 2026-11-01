//! Where labels go: points for polygons, and text offsets for each anchor a label may
//! be placed with.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::style::SymbolAnchor;

/// The baseline of a label sits this far (pixels at `ONE_EM`) below the top of its line
/// box, which `text-radial-offset` and variable anchors compensate for, as MapLibre does.
const BASELINE_OFFSET: f32 = 7.0;

/// The text offset (pixels at `ONE_EM`, `y` down) that keeps a label with this `anchor`
/// `radial` pixels from its point, for `text-radial-offset`.
pub fn radial_offset(anchor: SymbolAnchor, radial: f32) -> [f32; 2] {
    use SymbolAnchor::*;
    let radial = radial.max(0.0);
    let hypotenuse = radial / std::f32::consts::SQRT_2;
    let x = match anchor {
        TopRight | BottomRight => -hypotenuse,
        TopLeft | BottomLeft => hypotenuse,
        Left => radial,
        Right => -radial,
        _ => 0.0,
    };
    let y = match anchor {
        TopRight | TopLeft => hypotenuse - BASELINE_OFFSET,
        BottomRight | BottomLeft => -hypotenuse + BASELINE_OFFSET,
        Bottom => -radial + BASELINE_OFFSET,
        Top => radial - BASELINE_OFFSET,
        _ => 0.0,
    };
    [x, y]
}

/// The text offset (pixels at `ONE_EM`) for a variable `anchor` from a `text-offset` or
/// `text-variable-anchor-offset` entry: its magnitudes, pointed away from the anchor.
pub fn variable_offset(anchor: SymbolAnchor, offset: [f32; 2]) -> [f32; 2] {
    use SymbolAnchor::*;
    let [ax, ay] = offset.map(f32::abs);
    let x = match anchor {
        TopRight | BottomRight | Right => -ax,
        TopLeft | BottomLeft | Left => ax,
        _ => 0.0,
    };
    let y = match anchor {
        TopRight | TopLeft | Top => ay - BASELINE_OFFSET,
        BottomRight | BottomLeft | Bottom => -ay + BASELINE_OFFSET,
        _ => 0.0,
    };
    [x, y]
}

/// The point inside a polygon (exterior ring first, then holes) farthest from its
/// outline, within `precision`: "polylabel", where MapLibre puts polygon labels.
pub fn pole_of_inaccessibility(rings: &[Vec<[f32; 2]>], precision: f32) -> Option<[f32; 2]> {
    let exterior = rings.first().filter(|r| r.len() >= 3)?;
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in exterior {
        for i in 0..2 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    let size = (max[0] - min[0]).min(max[1] - min[1]);
    if size <= 0.0 {
        return Some(min);
    }

    let cell = |x: f32, y: f32, h: f32| {
        let d = signed_distance([x, y], rings);
        Cell {
            center: [x, y],
            h,
            d,
            max: d + h * std::f32::consts::SQRT_2,
        }
    };
    let mut queue = BinaryHeap::new();
    let h = size / 2.0;
    let mut x = min[0];
    while x < max[0] {
        let mut y = min[1];
        while y < max[1] {
            queue.push(cell(x + h, y + h, h));
            y += size;
        }
        x += size;
    }
    let mut best = centroid_cell(exterior)
        .map_or_else(|| cell(min[0], min[1], 0.0), |c| cell(c[0], c[1], 0.0));
    let center = cell((min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0, 0.0);
    if center.d > best.d {
        best = center;
    }
    // Plenty for the tile coordinates labels are placed in.
    let mut budget = 10_000;
    while let Some(c) = queue.pop() {
        if c.d > best.d {
            best = c;
        }
        if c.max - best.d <= precision || budget == 0 {
            continue;
        }
        budget -= 1;
        let h = c.h / 2.0;
        for (dx, dy) in [(-h, -h), (h, -h), (-h, h), (h, h)] {
            queue.push(cell(c.center[0] + dx, c.center[1] + dy, h));
        }
    }
    Some(best.center)
}

#[derive(Clone, Copy)]
struct Cell {
    center: [f32; 2],
    /// Half the cell's size.
    h: f32,
    /// From the center to the polygon's outline, positive inside.
    d: f32,
    /// The farthest any point of the cell can be from the outline.
    max: f32,
}

impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.max == other.max
    }
}

impl Eq for Cell {}

impl PartialOrd for Cell {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cell {
    fn cmp(&self, other: &Self) -> Ordering {
        self.max.total_cmp(&other.max)
    }
}

fn centroid_cell(ring: &[[f32; 2]]) -> Option<[f32; 2]> {
    let mut area = 0.0;
    let (mut x, mut y) = (0.0, 0.0);
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let f = a[0] * b[1] - b[0] * a[1];
        x += (a[0] + b[0]) * f;
        y += (a[1] + b[1]) * f;
        area += f * 3.0;
    }
    (area != 0.0).then(|| [x / area, y / area])
}

fn signed_distance(p: [f32; 2], rings: &[Vec<[f32; 2]>]) -> f32 {
    let mut inside = false;
    let mut best = f32::MAX;
    for ring in rings {
        let n = ring.len();
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + n - 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                inside = !inside;
            }
            best = best.min(segment_distance2(p, a, b));
        }
    }
    let d = best.sqrt();
    if inside { d } else { -d }
}

fn segment_distance2(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let d = [p[0] - a[0] - ab[0] * t, p[1] - a[1] - ab[1] * t];
    d[0] * d[0] + d[1] * d[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pole_avoids_holes_and_concavities() {
        let square = vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]];
        let p = pole_of_inaccessibility(std::slice::from_ref(&square), 1.0).unwrap();
        assert!(
            (p[0] - 50.0).abs() < 2.0 && (p[1] - 50.0).abs() < 2.0,
            "{p:?}"
        );

        // A hole in the middle pushes the label aside.
        let hole = vec![[30.0, 30.0], [70.0, 30.0], [70.0, 70.0], [30.0, 70.0]];
        let p = pole_of_inaccessibility(&[square.clone(), hole], 1.0).unwrap();
        assert!(signed_distance(p, &[square]) > 0.0);
        assert!(
            !(30.0..70.0).contains(&p[0]) || !(30.0..70.0).contains(&p[1]),
            "{p:?}"
        );

        // An L: the centroid is outside the shape, the pole is inside.
        let l = vec![
            [0.0, 0.0],
            [100.0, 0.0],
            [100.0, 20.0],
            [20.0, 20.0],
            [20.0, 100.0],
            [0.0, 100.0],
        ];
        let p = pole_of_inaccessibility(std::slice::from_ref(&l), 0.5).unwrap();
        assert!(signed_distance(p, &[l]) > 9.0, "{p:?}");
    }

    #[test]
    fn offsets_point_away_from_the_anchor() {
        assert_eq!(radial_offset(SymbolAnchor::Left, 24.0), [24.0, 0.0]);
        assert_eq!(radial_offset(SymbolAnchor::Top, 24.0), [0.0, 17.0]);
        assert_eq!(
            variable_offset(SymbolAnchor::BottomRight, [-1.0, 2.0]),
            [-1.0, -2.0 + 7.0]
        );
        assert_eq!(
            variable_offset(SymbolAnchor::Center, [5.0, 5.0]),
            [0.0, 0.0]
        );
    }
}
