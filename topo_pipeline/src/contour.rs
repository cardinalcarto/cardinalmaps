//! Marching squares over a [`Grid`], cut to a chunk and simplified.
//!
//! Each line is traced in the whole window but only kept inside the chunk, with a window
//! large enough that decisions about a line (is it a closed loop too small to keep?) come
//! out the same in every chunk it touches. A crossing is computed in a fixed direction
//! from the two lattice values of its edge, so neighboring chunks compute the very same
//! endpoint on their shared boundary.

use crate::dem::Grid;

/// Which contour family a line belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unit {
    Meters = 0,
    Feet = 1,
}

impl Unit {
    pub fn meters(self) -> f64 {
        match self {
            Unit::Meters => 1.0,
            Unit::Feet => 0.3048,
        }
    }
}

/// A contour line in units of 1e-7 degrees (lon, lat). Uphill is to the left, as traced.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub unit: Unit,
    /// The elevation in `unit`.
    pub elevation: i32,
    /// First and last points are the same.
    pub closed: bool,
    pub points: Vec<(i32, i32)>,
}

#[derive(Clone, Debug)]
pub struct Params {
    pub interval_m: u32,
    pub interval_ft: u32,
    pub simplify_m: f64,
    pub min_loop_m: f64,
    /// Rounds simplified corners, one pass of Chaikin's corner cutting.
    pub round_corners: bool,
}

/// Which cells of the window are to be kept, as half-open ranges of window cell indices.
#[derive(Clone, Copy, Debug)]
pub struct Keep {
    pub cols: (usize, usize),
    pub rows: (usize, usize),
}

#[derive(Clone, Copy)]
struct Segment {
    elevation: i32,
    from: u64,
    to: u64,
    cell: u32,
}

const METERS_PER_UNIT: f64 = 111_319.49 / 1e7;

/// Traces every level of `grid` and returns the lines inside `keep`.
pub fn contours(grid: &Grid, keep: Keep, params: &Params) -> Vec<Line> {
    let mut lines = Vec::new();
    for (unit, interval) in [(Unit::Meters, params.interval_m), (Unit::Feet, params.interval_ft)] {
        let mut segments = segments(grid, unit, interval);
        segments.sort_unstable_by_key(|s| (s.elevation, s.from));
        for level in segments.chunk_by(|a, b| a.elevation == b.elevation) {
            trace_level(grid, unit, level, keep, params, &mut lines);
        }
    }
    lines
}

/// Marching squares with the high side on the left of each segment's direction.
fn segments(grid: &Grid, unit: Unit, interval: u32) -> Vec<Segment> {
    let (w, h) = (grid.rect.width(), grid.rect.height());
    let step = f64::from(interval) * unit.meters();
    let mut out = Vec::new();
    if w < 2 || h < 2 {
        return out;
    }
    for row in 0..h - 1 {
        for col in 0..w - 1 {
            let sw = f64::from(grid.at(col, row));
            let se = f64::from(grid.at(col + 1, row));
            let ne = f64::from(grid.at(col + 1, row + 1));
            let nw = f64::from(grid.at(col, row + 1));
            let (lo, hi) = (sw.min(se).min(ne).min(nw), sw.max(se).max(ne).max(nw));
            // NaN makes min and max skip it, so check each corner.
            if sw.is_nan() || se.is_nan() || ne.is_nan() || nw.is_nan() {
                continue;
            }
            let first = (lo / step).floor() as i64 + 1;
            let last = (hi / step).floor() as i64;
            for k in first..=last {
                // Sea level is the coastline's business, not a contour's.
                if k == 0 {
                    continue;
                }
                let level = k as f64 * step;
                let mask = u8::from(sw >= level)
                    | u8::from(se >= level) << 1
                    | u8::from(ne >= level) << 2
                    | u8::from(nw >= level) << 3;
                let south = edge_id(w, col, row, 0);
                let north = edge_id(w, col, row + 1, 0);
                let west = edge_id(w, col, row, 1);
                let east = edge_id(w, col + 1, row, 1);
                let center_high = (sw + se + ne + nw) / 4.0 >= level;
                let (a, b) = match mask {
                    1 => ((south, west), None),
                    2 => ((east, south), None),
                    3 => ((east, west), None),
                    4 => ((north, east), None),
                    6 => ((north, south), None),
                    7 => ((north, west), None),
                    8 => ((west, north), None),
                    9 => ((south, north), None),
                    11 => ((east, north), None),
                    12 => ((west, east), None),
                    13 => ((south, east), None),
                    14 => ((west, south), None),
                    5 if center_high => ((north, west), Some((south, east))),
                    5 => ((south, west), Some((north, east))),
                    10 if center_high => ((west, south), Some((east, north))),
                    10 => ((east, south), Some((west, north))),
                    _ => continue,
                };
                let elevation = k as i32 * interval as i32;
                let cell = (row * w + col) as u32;
                out.push(Segment { elevation, from: a.0, to: a.1, cell });
                if let Some(b) = b {
                    out.push(Segment { elevation, from: b.0, to: b.1, cell });
                }
            }
        }
    }
    out
}

#[inline]
fn edge_id(w: usize, col: usize, row: usize, dir: u64) -> u64 {
    (row * w + col) as u64 * 2 + dir
}

/// Where an edge meets a level, in rounded 1e-7 degrees. Always computed from the edge's
/// west or south end, whichever chunk or direction asks.
fn crossing(grid: &Grid, edge: u64, level: f64) -> (i32, i32) {
    let w = grid.rect.width();
    let (index, dir) = ((edge / 2) as usize, edge & 1);
    let (col, row) = (index % w, index / w);
    let a = f64::from(grid.at(col, row));
    let b = f64::from(if dir == 0 { grid.at(col + 1, row) } else { grid.at(col, row + 1) });
    let t = (level - a) / (b - a);
    let (mut x, mut y) = ((grid.rect.x0 + col as i64) as f64, (grid.rect.y0 + row as i64) as f64);
    if dir == 0 {
        x += t;
    } else {
        y += t;
    }
    ((Grid::lon(x, grid.refine) * 1e7).round() as i32, (Grid::lat(y, grid.refine) * 1e7).round() as i32)
}

fn trace_level(
    grid: &Grid,
    unit: Unit,
    segments: &[Segment],
    keep: Keep,
    params: &Params,
    out: &mut Vec<Line>,
) {
    let elevation = segments[0].elevation;
    let level = f64::from(elevation) * unit.meters();
    let mut incoming: Vec<u64> = segments.iter().map(|s| s.to).collect();
    incoming.sort_unstable();
    let mut visited = vec![false; segments.len()];
    let next = |edge: u64| segments.binary_search_by_key(&edge, |s| s.from).ok();

    let walk = |start: usize, visited: &mut [bool]| {
        let mut edges = vec![segments[start].from];
        let mut cells = Vec::new();
        let mut at = start;
        loop {
            visited[at] = true;
            edges.push(segments[at].to);
            cells.push(segments[at].cell);
            match next(segments[at].to) {
                Some(following) if !visited[following] => at = following,
                _ => break,
            }
        }
        (edges, cells)
    };

    // Lines that start somewhere, at the window's edge or at missing data.
    for start in 0..segments.len() {
        if incoming.binary_search(&segments[start].from).is_err() {
            let (edges, cells) = walk(start, &mut visited);
            let points = edges.iter().map(|&e| crossing(grid, e, level)).collect();
            emit(grid, unit, elevation, points, cells, false, keep, params, out);
        }
    }
    // What remains are loops.
    for start in 0..segments.len() {
        if !visited[start] {
            let (edges, cells) = walk(start, &mut visited);
            let points: Vec<_> = edges.iter().map(|&e| crossing(grid, e, level)).collect();
            if length_m(&points) >= params.min_loop_m {
                emit(grid, unit, elevation, points, cells, true, keep, params, out);
            }
        }
    }
}

fn length_m(points: &[(i32, i32)]) -> f64 {
    points
        .windows(2)
        .map(|p| {
            let lat = f64::from(p[0].1) * 1e-7;
            let dx = f64::from(p[1].0 - p[0].0) * lat.to_radians().cos();
            let dy = f64::from(p[1].1 - p[0].1);
            dx.hypot(dy) * METERS_PER_UNIT
        })
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn emit(
    grid: &Grid,
    unit: Unit,
    elevation: i32,
    points: Vec<(i32, i32)>,
    cells: Vec<u32>,
    closed: bool,
    keep: Keep,
    params: &Params,
    out: &mut Vec<Line>,
) {
    let w = grid.rect.width() as u32;
    let kept = |cell: u32| {
        let (col, row) = ((cell % w) as usize, (cell / w) as usize);
        (keep.cols.0..keep.cols.1).contains(&col) && (keep.rows.0..keep.rows.1).contains(&row)
    };
    let flags: Vec<bool> = cells.iter().map(|&c| kept(c)).collect();
    if closed && flags.iter().all(|&k| k) {
        if let Some(points) = finish(points, true, params) {
            out.push(Line { unit, elevation, closed: true, points });
        }
        return;
    }
    let (points, flags) = if closed {
        // Start the loop where it enters the chunk, so its first run isn't split in two.
        let n = flags.len();
        let Some(enter) = (0..n).find(|&i| flags[i] && !flags[(i + n - 1) % n]) else {
            return;
        };
        let mut rotated = Vec::with_capacity(n + 1);
        rotated.extend_from_slice(&points[enter..n]);
        rotated.extend_from_slice(&points[..=enter]);
        let mut rotated_flags = flags[enter..].to_vec();
        rotated_flags.extend_from_slice(&flags[..enter]);
        (rotated, rotated_flags)
    } else {
        (points, flags)
    };
    let mut i = 0;
    while i < flags.len() {
        if !flags[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < flags.len() && flags[i] {
            i += 1;
        }
        if let Some(points) = finish(points[start..=i].to_vec(), false, params) {
            out.push(Line { unit, elevation, closed: false, points });
        }
    }
}

/// Removes repeated points and simplifies. The ends stay put, which is what keeps the
/// shared endpoints on chunk boundaries identical.
fn finish(mut points: Vec<(i32, i32)>, closed: bool, params: &Params) -> Option<Vec<(i32, i32)>> {
    points.dedup();
    let points = simplify(&points, closed, params.simplify_m);
    let enough = if closed { 4 } else { 2 };
    if points.len() < enough {
        return None;
    }
    Some(if params.round_corners { round_corners(&points, closed) } else { points })
}

/// One pass of Chaikin's corner cutting. An open line keeps its ends, so chunks still meet.
pub fn round_corners(points: &[(i32, i32)], closed: bool) -> Vec<(i32, i32)> {
    let cut = |a: (i32, i32), b: (i32, i32), t: f64| {
        let along = |a: i32, b: i32| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as i32;
        (along(a.0, b.0), along(a.1, b.1))
    };
    let mut out = Vec::with_capacity(points.len() * 2);
    let last = points.len() - 1;
    if !closed {
        out.push(points[0]);
    }
    for (i, pair) in points.windows(2).enumerate() {
        if closed || i > 0 {
            out.push(cut(pair[0], pair[1], 0.25));
        }
        if closed || i + 1 < last {
            out.push(cut(pair[0], pair[1], 0.75));
        }
    }
    if closed {
        out.push(out[0]);
    } else {
        out.push(points[last]);
    }
    out.dedup();
    out
}

/// Douglas-Peucker, in meters on a plane fit to the line's latitude.
pub fn simplify(points: &[(i32, i32)], closed: bool, tolerance_m: f64) -> Vec<(i32, i32)> {
    if points.len() < 3 || tolerance_m <= 0.0 {
        return points.to_vec();
    }
    let lat = f64::from(points[0].1) * 1e-7;
    let scale_x = lat.to_radians().cos() * METERS_PER_UNIT;
    let xy = |p: (i32, i32)| (f64::from(p.0) * scale_x, f64::from(p.1) * METERS_PER_UNIT);
    let mut keep = vec![false; points.len()];
    let last = points.len() - 1;
    keep[0] = true;
    keep[last] = true;
    let mut stack = Vec::new();
    if closed {
        // A loop's ends coincide, so anchor it at the point farthest from its start too.
        let start = xy(points[0]);
        let far = (1..last)
            .max_by(|&a, &b| {
                let distance = |i: usize| {
                    let p = xy(points[i]);
                    (p.0 - start.0).hypot(p.1 - start.1)
                };
                distance(a).total_cmp(&distance(b))
            })
            .unwrap_or(0);
        keep[far] = true;
        stack.push((0, far));
        stack.push((far, last));
    } else {
        stack.push((0, last));
    }
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (xy(points[a]), xy(points[b]));
        let (dx, dy) = (pb.0 - pa.0, pb.1 - pa.1);
        let len2 = dx * dx + dy * dy;
        let (mut worst, mut worst_d) = (a, -1.0);
        for i in a + 1..b {
            let p = xy(points[i]);
            let t = if len2 > 0.0 {
                (((p.0 - pa.0) * dx + (p.1 - pa.1) * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let d = (p.0 - pa.0 - t * dx).hypot(p.1 - pa.1 - t * dy);
            if d > worst_d {
                (worst, worst_d) = (i, d);
            }
        }
        if worst_d > tolerance_m {
            keep[worst] = true;
            stack.push((a, worst));
            stack.push((worst, b));
        }
    }
    points.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}
