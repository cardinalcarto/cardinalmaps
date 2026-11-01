//! Cutting GeoJSON into tiles, as [geojson-vt](https://github.com/mapbox/geojson-vt) does.
//!
//! Features are projected to world coordinates (`0..1` across the Web Mercator square) once,
//! with every vertex of a line or ring given an importance by Douglas-Peucker
//! simplification at the maximum zoom's tolerance. A tile is cut from its nearest cached
//! ancestor by clipping one zoom level at a time (each level's result is cached, so
//! neighbouring tiles share the work), then turned into tile coordinates keeping only the
//! vertices important enough for its zoom level.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mlt_core::geo_types::{
    Coord, Geometry, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon,
};

use super::{LngLatGeometry, TileFeature};
use crate::TileId;

/// Clipped tiles kept for cutting their descendants; the cache starts over past this.
const MAX_CACHED_TILES: usize = 4096;

#[derive(Debug, Clone, Copy)]
pub(crate) struct TilerOptions {
    /// Tile coordinates across a tile.
    pub extent: u32,
    /// Tile coordinates around each tile that are kept, so lines and fills continue under
    /// their neighbours' edges.
    pub buffer: u32,
    /// Douglas-Peucker tolerance in tile coordinates; nothing is simplified at `max_zoom`.
    pub tolerance: f64,
    pub max_zoom: u8,
    /// Keep track of where each piece of a line lies along the whole line.
    pub line_metrics: bool,
}

/// A line or ring in world coordinates.
#[derive(Debug, Clone)]
struct Line {
    /// `[x, y, importance]`: the importance is the squared distance at which simplification
    /// drops the point (`1.0` for points that are always kept, such as the ends).
    points: Vec<[f64; 3]>,
    /// Length of a line or area of a ring, for dropping ones too small to see.
    size: f64,
    /// Distance along the original line where this piece starts and ends.
    start: f64,
    end: f64,
}

#[derive(Debug, Clone)]
enum Shape {
    Points(Vec<[f64; 2]>),
    Lines(Vec<Line>),
    Polygons(Vec<Vec<Line>>),
}

#[derive(Debug, Clone)]
struct ProjectedFeature {
    /// Index of the feature in [`Tiler::new`]'s input.
    source: usize,
    shape: Shape,
    /// `[min x, min y, max x, max y]`
    bbox: [f64; 4],
}

type Features = Arc<Vec<Arc<ProjectedFeature>>>;

pub(crate) struct Tiler {
    options: TilerOptions,
    /// Clipped features by tile, always including the whole world at `0/0/0`.
    cache: Mutex<HashMap<TileId, Features>>,
}

impl Tiler {
    /// Project `features` (`(source index, geometry)`) and prepare them for tiling.
    pub fn new<'a>(
        features: impl IntoIterator<Item = (usize, &'a LngLatGeometry)>,
        options: TilerOptions,
    ) -> Self {
        let tolerance = options.tolerance / (f64::from(options.max_zoom).exp2() * extent(options));
        let sq_tolerance = tolerance * tolerance;
        let mut projected = Vec::new();
        for (source, geometry) in features {
            convert(
                source,
                geometry,
                sq_tolerance,
                options.line_metrics,
                &mut projected,
            );
        }
        let buffer = f64::from(options.buffer) / extent(options);
        let world: Vec<_> = wrap(projected.into_iter().map(Arc::new).collect(), buffer);
        let cache = HashMap::from([(TileId::new(0, 0, 0), Arc::new(world))]);
        Self {
            options,
            cache: Mutex::new(cache),
        }
    }

    /// The features of one tile, in tile coordinates.
    pub fn tile(&self, id: TileId, out: &mut Vec<(usize, TileFeature)>) {
        let features = self.clipped(id);
        let options = self.options;
        let tolerance = if id.z >= options.max_zoom {
            0.0
        } else {
            options.tolerance / (f64::from(id.z).exp2() * extent(options))
        };
        for feature in features.iter() {
            to_tile(feature, id, tolerance, options, out);
        }
    }

    /// The world's features clipped to `id` and its buffer.
    fn clipped(&self, id: TileId) -> Features {
        // The nearest ancestor (or the tile itself) that is already cut. The lock isn't held
        // while clipping, so workers cutting different tiles don't wait for each other.
        let (mut z, mut features) = {
            let cache = self.cache.lock().unwrap();
            (0..=id.z)
                .rev()
                .find_map(|z| {
                    let shift = id.z - z;
                    let ancestor = TileId::new(z, id.x >> shift, id.y >> shift);
                    cache.get(&ancestor).map(|f| (z, f.clone()))
                })
                .expect("the root is always cached")
        };
        let buffer = f64::from(self.options.buffer) / extent(self.options);
        let line_metrics = self.options.line_metrics;
        while z < id.z && !features.is_empty() {
            z += 1;
            let shift = id.z - z;
            let tile = TileId::new(z, id.x >> shift, id.y >> shift);
            let scale = f64::from(z).exp2();
            let (x, y) = (f64::from(tile.x), f64::from(tile.y));
            let column = clip(
                &features,
                (x - buffer) / scale,
                (x + 1.0 + buffer) / scale,
                0,
                line_metrics,
            );
            features = Arc::new(clip(
                &column,
                (y - buffer) / scale,
                (y + 1.0 + buffer) / scale,
                1,
                line_metrics,
            ));
            let mut cache = self.cache.lock().unwrap();
            if cache.len() >= MAX_CACHED_TILES {
                cache.retain(|id, _| id.z == 0);
            }
            cache.insert(tile, features.clone());
        }
        features
    }
}

fn extent(options: TilerOptions) -> f64 {
    f64::from(options.extent)
}

fn project([lng, lat]: [f64; 2]) -> [f64; 2] {
    let sin = lat.to_radians().sin();
    let y = 0.5 - 0.25 * ((1.0 + sin) / (1.0 - sin)).ln() / std::f64::consts::PI;
    [lng / 360.0 + 0.5, y.clamp(0.0, 1.0)]
}

fn convert(
    source: usize,
    geometry: &LngLatGeometry,
    sq_tolerance: f64,
    line_metrics: bool,
    out: &mut Vec<ProjectedFeature>,
) {
    let line = |l: &[[f64; 2]]| convert_line(l, sq_tolerance, false);
    let polygon = |rings: &[Vec<[f64; 2]>]| {
        rings
            .iter()
            .map(|r| convert_line(r, sq_tolerance, true))
            .collect::<Vec<_>>()
    };
    let shape = match geometry {
        LngLatGeometry::Point(p) => Shape::Points(vec![project(*p)]),
        LngLatGeometry::MultiPoint(points) => {
            Shape::Points(points.iter().copied().map(project).collect())
        }
        LngLatGeometry::LineString(l) => Shape::Lines(vec![line(l)]),
        // Line metrics are per line, so each line of a multi-line is its own feature.
        LngLatGeometry::MultiLineString(lines) if line_metrics => {
            for l in lines {
                push_feature(out, source, Shape::Lines(vec![line(l)]));
            }
            return;
        }
        LngLatGeometry::MultiLineString(lines) => {
            Shape::Lines(lines.iter().map(|l| line(l)).collect())
        }
        LngLatGeometry::Polygon(rings) => Shape::Polygons(vec![polygon(rings)]),
        LngLatGeometry::MultiPolygon(polygons) => {
            Shape::Polygons(polygons.iter().map(|p| polygon(p)).collect())
        }
    };
    push_feature(out, source, shape);
}

fn push_feature(out: &mut Vec<ProjectedFeature>, source: usize, shape: Shape) {
    let empty = match &shape {
        Shape::Points(p) => p.is_empty(),
        Shape::Lines(l) => l.is_empty(),
        Shape::Polygons(p) => p.is_empty(),
    };
    if !empty {
        out.push(ProjectedFeature {
            source,
            bbox: bbox(&shape),
            shape,
        });
    }
}

fn convert_line(coords: &[[f64; 2]], sq_tolerance: f64, is_ring: bool) -> Line {
    let mut points: Vec<[f64; 3]> = Vec::with_capacity(coords.len());
    let mut size = 0.0;
    for &c in coords {
        let [x, y] = project(c);
        if let Some(&[x0, y0, _]) = points.last() {
            size += if is_ring {
                (x0 * y - x * y0) / 2.0
            } else {
                ((x - x0).powi(2) + (y - y0).powi(2)).sqrt()
            };
        }
        points.push([x, y, 0.0]);
    }
    if let Some(last) = points.len().checked_sub(1) {
        points[0][2] = 1.0;
        simplify(&mut points, 0, last, sq_tolerance);
        points[last][2] = 1.0;
    }
    let size = size.abs();
    Line {
        points,
        size,
        start: 0.0,
        end: size,
    }
}

/// Douglas-Peucker: give each point between `first` and `last` the squared distance at
/// which it stops mattering. Ties go to the point nearest the middle, as in geojson-vt, so
/// results match it.
fn simplify(points: &mut [[f64; 3]], first: usize, last: usize, sq_tolerance: f64) {
    let mut max = sq_tolerance;
    let mid = first + (last - first) / 2;
    let mut min_to_mid = last - first;
    let mut index = None;
    let [ax, ay, _] = points[first];
    let [bx, by, _] = points[last];
    for (i, p) in points.iter().enumerate().take(last).skip(first + 1) {
        let d = sq_segment_distance(p[0], p[1], ax, ay, bx, by);
        if d > max {
            index = Some(i);
            max = d;
        } else if d == max {
            let to_mid = i.abs_diff(mid);
            if to_mid < min_to_mid {
                index = Some(i);
                min_to_mid = to_mid;
            }
        }
    }
    if let Some(index) = index
        && max > sq_tolerance
    {
        if index - first > 1 {
            simplify(points, first, index, sq_tolerance);
        }
        points[index][2] = max;
        if last - index > 1 {
            simplify(points, index, last, sq_tolerance);
        }
    }
}

fn sq_segment_distance(px: f64, py: f64, mut x: f64, mut y: f64, bx: f64, by: f64) -> f64 {
    let (dx, dy) = (bx - x, by - y);
    if dx != 0.0 || dy != 0.0 {
        let t = ((px - x) * dx + (py - y) * dy) / (dx * dx + dy * dy);
        if t > 1.0 {
            (x, y) = (bx, by);
        } else if t > 0.0 {
            x += dx * t;
            y += dy * t;
        }
    }
    (px - x).powi(2) + (py - y).powi(2)
}

fn bbox(shape: &Shape) -> [f64; 4] {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut add = |x: f64, y: f64| {
        b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    };
    match shape {
        Shape::Points(points) => points.iter().for_each(|p| add(p[0], p[1])),
        Shape::Lines(lines) => lines
            .iter()
            .flat_map(|l| &l.points)
            .for_each(|p| add(p[0], p[1])),
        Shape::Polygons(polygons) => polygons
            .iter()
            .flatten()
            .flat_map(|l| &l.points)
            .for_each(|p| add(p[0], p[1])),
    }
    b
}

/// Copy what lies within a buffer of the antimeridian to the other side of the world, so
/// tiles on either edge draw it.
fn wrap(features: Vec<Arc<ProjectedFeature>>, buffer: f64) -> Vec<Arc<ProjectedFeature>> {
    let left = clip(&features, -1.0 - buffer, buffer, 0, false);
    let right = clip(&features, 1.0 - buffer, 2.0 + buffer, 0, false);
    if left.is_empty() && right.is_empty() {
        return features;
    }
    let mut merged = clip(&features, -buffer, 1.0 + buffer, 0, false);
    merged.extend(left.iter().map(|f| Arc::new(shift(f, 1.0))));
    merged.extend(right.iter().map(|f| Arc::new(shift(f, -1.0))));
    merged
}

fn shift(feature: &ProjectedFeature, dx: f64) -> ProjectedFeature {
    let mut f = feature.clone();
    let shift_line = |l: &mut Line| l.points.iter_mut().for_each(|p| p[0] += dx);
    match &mut f.shape {
        Shape::Points(points) => points.iter_mut().for_each(|p| p[0] += dx),
        Shape::Lines(lines) => lines.iter_mut().for_each(shift_line),
        Shape::Polygons(polygons) => polygons.iter_mut().flatten().for_each(shift_line),
    }
    f.bbox[0] += dx;
    f.bbox[2] += dx;
    f
}

/// The parts of `features` with `k1 <= coordinate <= k2` on `axis` (0 for x, 1 for y).
fn clip(
    features: &[Arc<ProjectedFeature>],
    k1: f64,
    k2: f64,
    axis: usize,
    line_metrics: bool,
) -> Vec<Arc<ProjectedFeature>> {
    let mut out = Vec::new();
    for feature in features {
        let (min, max) = (feature.bbox[axis], feature.bbox[axis + 2]);
        if min >= k1 && max < k2 {
            out.push(feature.clone());
            continue;
        }
        if max < k1 || min > k2 {
            continue;
        }
        let shape = match &feature.shape {
            Shape::Points(points) => Shape::Points(
                points
                    .iter()
                    .filter(|p| p[axis] >= k1 && p[axis] <= k2)
                    .copied()
                    .collect(),
            ),
            Shape::Lines(lines) => {
                let mut clipped = Vec::new();
                for line in lines {
                    clip_line(line, &mut clipped, k1, k2, axis, false, line_metrics);
                }
                Shape::Lines(clipped)
            }
            Shape::Polygons(polygons) => Shape::Polygons(
                polygons
                    .iter()
                    .filter_map(|rings| {
                        let mut clipped = Vec::new();
                        for ring in rings {
                            clip_line(ring, &mut clipped, k1, k2, axis, true, false);
                        }
                        (!clipped.is_empty()).then_some(clipped)
                    })
                    .collect(),
            ),
        };
        let mut f = Vec::new();
        push_feature(&mut f, feature.source, shape);
        out.extend(f.into_iter().map(Arc::new));
    }
    out
}

/// Clip one line (or ring, closing it again afterwards) into the pieces inside the band.
fn clip_line(
    line: &Line,
    out: &mut Vec<Line>,
    k1: f64,
    k2: f64,
    axis: usize,
    is_ring: bool,
    track_metrics: bool,
) {
    let new_slice = || Line {
        points: Vec::new(),
        size: line.size,
        start: line.start,
        end: line.end,
    };
    let mut slice = new_slice();
    let points = &line.points;
    let Some(last) = points.len().checked_sub(1) else {
        return;
    };
    // Adds the crossing of `a`-`b` with the line at `k` and returns where along `a`-`b` it is.
    let intersect = |slice: &mut Line, a: [f64; 3], b: [f64; 3], k: f64| {
        let t = (k - a[axis]) / (b[axis] - a[axis]);
        let other = 1 - axis;
        let mut p = [0.0, 0.0, 1.0];
        p[axis] = k;
        p[other] = a[other] + (b[other] - a[other]) * t;
        slice.points.push(p);
        t
    };
    let mut length = line.start;
    for i in 0..last {
        let (a, b) = (points[i], points[i + 1]);
        let (av, bv) = (a[axis], b[axis]);
        let segment = if track_metrics {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
        } else {
            0.0
        };
        let mut exit = None;
        if av < k1 {
            if bv > k1 {
                // Entering from below k1.
                let t = intersect(&mut slice, a, b, k1);
                slice.start = length + segment * t;
            }
        } else if av > k2 {
            if bv < k2 {
                // Entering from above k2.
                let t = intersect(&mut slice, a, b, k2);
                slice.start = length + segment * t;
            }
        } else {
            slice.points.push(a);
        }
        if bv < k1 && av >= k1 {
            exit = Some(intersect(&mut slice, a, b, k1));
        }
        if bv > k2 && av <= k2 {
            exit = Some(intersect(&mut slice, a, b, k2));
        }
        if !is_ring && let Some(t) = exit {
            slice.end = length + segment * t;
            out.push(std::mem::replace(&mut slice, new_slice()));
        }
        length += segment;
    }
    let p = points[last];
    if p[axis] >= k1 && p[axis] <= k2 {
        slice.points.push(p);
    }
    if is_ring
        && let (Some(&first), Some(&end)) = (slice.points.first(), slice.points.last())
        && slice.points.len() > 1
        && (first[0] != end[0] || first[1] != end[1])
    {
        slice.points.push(first);
    }
    if !slice.points.is_empty() {
        out.push(slice);
    }
}

/// Turn a clipped feature into tile coordinates for `tile`, dropping vertices and whole
/// lines or rings below `tolerance` (in world units).
fn to_tile(
    feature: &ProjectedFeature,
    tile: TileId,
    tolerance: f64,
    options: TilerOptions,
    out: &mut Vec<(usize, TileFeature)>,
) {
    let scale = f64::from(tile.z).exp2();
    let extent = extent(options);
    let (tx, ty) = (f64::from(tile.x), f64::from(tile.y));
    let to_coord = |p: &[f64]| Coord {
        x: (extent * (p[0] * scale - tx)).round() as i32,
        y: (extent * (p[1] * scale - ty)).round() as i32,
    };
    let sq_tolerance = tolerance * tolerance;
    let simplified = |line: &Line| -> Vec<Coord<i32>> {
        let mut coords: Vec<Coord<i32>> = Vec::new();
        for p in &line.points {
            if tolerance == 0.0 || p[2] > sq_tolerance {
                let c = to_coord(p);
                if coords.last() != Some(&c) {
                    coords.push(c);
                }
            }
        }
        coords
    };
    let mut push = |geometry: Geometry<i32>, line_clip: Option<[f64; 2]>| {
        out.push((
            feature.source,
            TileFeature {
                id: None,
                geometry,
                properties: Default::default(),
                line_clip,
            },
        ));
    };
    match &feature.shape {
        Shape::Points(points) => {
            let mut points: Vec<Point<i32>> = points.iter().map(|p| Point(to_coord(p))).collect();
            match points.len() {
                0 => {}
                1 => push(Geometry::Point(points.pop().unwrap()), None),
                _ => push(Geometry::MultiPoint(MultiPoint(points)), None),
            }
        }
        Shape::Lines(lines) => {
            let mut kept = Vec::new();
            for line in lines {
                if tolerance > 0.0 && line.size < tolerance {
                    continue;
                }
                let coords = simplified(line);
                if coords.len() < 2 {
                    continue;
                }
                if options.line_metrics {
                    let clip = if line.size > 0.0 {
                        [line.start / line.size, line.end / line.size]
                    } else {
                        [0.0, 1.0]
                    };
                    push(Geometry::LineString(LineString(coords)), Some(clip));
                } else {
                    kept.push(LineString(coords));
                }
            }
            match kept.len() {
                0 => {}
                1 => push(Geometry::LineString(kept.pop().unwrap()), None),
                _ => push(Geometry::MultiLineString(MultiLineString(kept)), None),
            }
        }
        Shape::Polygons(polygons) => {
            let mut kept = Vec::new();
            for rings in polygons {
                let mut rings = rings.iter().map(|ring| {
                    if tolerance > 0.0 && ring.size < sq_tolerance {
                        return None;
                    }
                    let mut coords = simplified(ring);
                    if coords.first() != coords.last() {
                        coords.push(coords[0]);
                    }
                    // A ring needs three corners besides the closing one.
                    (coords.len() >= 4).then_some(LineString(coords))
                });
                let Some(Some(exterior)) = rings.next() else {
                    continue;
                };
                kept.push(Polygon::new(exterior, rings.flatten().collect()));
            }
            match kept.len() {
                0 => {}
                1 => push(Geometry::Polygon(kept.pop().unwrap()), None),
                _ => push(Geometry::MultiPolygon(MultiPolygon(kept)), None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> TilerOptions {
        TilerOptions {
            extent: 4096,
            buffer: 64,
            tolerance: 3.0,
            max_zoom: 14,
            line_metrics: false,
        }
    }

    fn tile(tiler: &Tiler, z: u8, x: u32, y: u32) -> Vec<TileFeature> {
        let mut out = Vec::new();
        tiler.tile(TileId::new(z, x, y), &mut out);
        out.into_iter().map(|(_, f)| f).collect()
    }

    #[test]
    fn projects_corners() {
        assert_eq!(project([0.0, 0.0]), [0.5, 0.5]);
        let [x, y] = project([-180.0, 85.051_128_78]);
        assert!(x.abs() < 1e-12 && y.abs() < 1e-8);
    }

    #[test]
    fn simplification_keeps_corners() {
        // A nearly straight line with one real corner.
        let coords = [[0.0, 0.0], [1.0, 0.000_001], [2.0, 0.0], [2.0, 2.0]];
        let line = convert_line(&coords, 1e-12, false);
        let importance: Vec<f64> = line.points.iter().map(|p| p[2]).collect();
        assert_eq!(importance[0], 1.0);
        assert_eq!(importance[3], 1.0);
        assert!(importance[2] > 1e-12, "the corner matters");
        assert!(importance[1] <= 1e-12, "the wiggle doesn't: {importance:?}");
    }

    #[test]
    fn polygon_is_clipped_to_tile_and_buffer() {
        // A square around lng/lat 0,0 spanning all four z1 tiles.
        let square = LngLatGeometry::Polygon(vec![vec![
            [-10.0, -10.0],
            [10.0, -10.0],
            [10.0, 10.0],
            [-10.0, 10.0],
            [-10.0, -10.0],
        ]]);
        let tiler = Tiler::new([(0, &square)], options());
        let features = tile(&tiler, 1, 1, 1);
        assert_eq!(features.len(), 1);
        let Geometry::Polygon(p) = &features[0].geometry else {
            panic!("{:?}", features[0].geometry)
        };
        let xs: Vec<i32> = p.exterior().0.iter().map(|c| c.x).collect();
        let ys: Vec<i32> = p.exterior().0.iter().map(|c| c.y).collect();
        // Clipped to the buffer on the left and top, at 64 units outside the tile.
        assert_eq!(*xs.iter().min().unwrap(), -64);
        assert_eq!(*ys.iter().min().unwrap(), -64);
        // 10 degrees of a 180 degree tile.
        let right = (4096.0 * 10.0 / 180.0f64).round() as i32;
        assert_eq!(*xs.iter().max().unwrap(), right);
        assert!(p.exterior().is_closed());
        // Nothing reaches a tile far away.
        assert!(tile(&tiler, 4, 0, 0).is_empty());
    }

    #[test]
    fn lines_split_into_pieces_with_metrics() {
        // From lng -100 to 100 along the equator: z1 tile 1/0/1 holds the part east of 0.
        let line = LngLatGeometry::LineString(vec![[-100.0, 0.0], [100.0, 0.0]]);
        let tiler = Tiler::new(
            [(0, &line)],
            TilerOptions {
                line_metrics: true,
                buffer: 0,
                ..options()
            },
        );
        let features = tile(&tiler, 1, 1, 1);
        assert_eq!(features.len(), 1);
        let [start, end] = features[0].line_clip.unwrap();
        assert!((start - 0.5).abs() < 1e-9, "{start}");
        assert!((end - 1.0).abs() < 1e-9, "{end}");
        let Geometry::LineString(l) = &features[0].geometry else {
            panic!()
        };
        assert_eq!(l.0[0].x, 0);
    }

    #[test]
    fn small_rings_are_dropped_at_low_zoom() {
        let tiny = LngLatGeometry::Polygon(vec![vec![
            [0.0, 0.0],
            [0.0001, 0.0],
            [0.0001, 0.0001],
            [0.0, 0.0],
        ]]);
        let tiler = Tiler::new([(0, &tiny)], options());
        assert!(tile(&tiler, 2, 2, 1).is_empty());
        // At the max zoom nothing is simplified away.
        let (x, y) = (1 << 13, 1 << 13);
        assert_eq!(tile(&tiler, 14, x, y - 1).len(), 1);
    }

    #[test]
    fn antimeridian_is_wrapped() {
        let point = LngLatGeometry::Point([179.9, 0.0]);
        let tiler = Tiler::new([(0, &point)], options());
        // Within the buffer of the western edge's tile at z2.
        let west = tile(&tiler, 2, 0, 1);
        assert_eq!(west.len(), 1, "{west:?}");
        let Geometry::Point(p) = west[0].geometry else {
            panic!()
        };
        assert!(p.0.x < 0);
    }
}
