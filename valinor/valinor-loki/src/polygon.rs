//! The edges inside a request's `exclude_polygons` (mainline's `edges_in_rings`,
//! `src/loki/polygon_search.cc`).
//!
//! Loki turns exclude polygons into excluded edges, which it adds to the costing options
//! for Thor. It finds the local bins each ring's boundary crosses (rasterizing the ring
//! over the bin grid), flood fills the bins entirely inside it, and then checks the edges of
//! those bins: every edge of a bin inside the ring is excluded, and an edge of a boundary bin
//! is excluded if its shape crosses the ring or lies inside it.
//! With `exclude_levels`, only edges on exactly one of the given levels are excluded.
//!
//! The result is a set (mainline's `std::unordered_set`), so its order is unspecified;
//! [`edges_in_rings`] returns it sorted.
//!
//! # Bounding circles
//!
//! As in the search, an edge whose bounding circle is outside a ring's bounding box is
//! skipped without decoding its shape. The circle is grown by
//! [`CIRCLE_MARGIN_METERS`](crate::search::CIRCLE_MARGIN_METERS) first,
//! so that we never skip an edge that mainline (with another C library's `cosf`) would check.

#![allow(
    clippy::float_cmp,
    clippy::many_single_char_names,
    reason = "a port of mainline's arithmetic, which compares floats exactly and names its terms tersely"
)]

use crate::geo::{Aabb, PointLL, cpp_max, cpp_min, meters_per_lng_degree};
use crate::graph::{Graph, GraphError, edge as get_edge, shape};
use crate::search::{CIRCLE_MARGIN_METERS, point_of};
use crate::tiles::{
    BINS_DIM, COLUMNS, ROWS, SUBDIVISION_SIZE, bin_bbox, bin_center, neighboring_bins,
};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use thiserror::Error;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::GraphTile;
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::Options;
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_response::{ErrorCode, ValhallaError};
use valinor_sif::{Costing, DisallowMask};

/// Meters to radians on mainline's sphere (`RAD_PER_METER`).
const RAD_PER_METER: f64 = 1.0 / 6_378_160.187;
const RAD_PER_DEG_D: f64 = core::f64::consts::PI / 180.0;
const DEG_PER_RAD_D: f64 = 180.0 / core::f64::consts::PI;

/// A failure to find the edges in the polygons.
#[derive(Debug, Error)]
pub enum PolygonError {
    /// The polygons break a limit (167, 176, or 177).
    #[error("{}", .0.message())]
    Limit(ValhallaError),
    /// The graph couldn't be read.
    #[error(transparent)]
    Graph(#[from] GraphError),
}

/// A closed, clockwise ring and its bounding box.
struct Ring {
    points: Vec<PointLL>,
    bbox: Aabb,
}

/// Twice the signed area of a ring, halved (`polygon_area`, the shoelace formula):
/// positive for counter-clockwise rings.
fn polygon_area(ring: &[PointLL]) -> f64 {
    let (Some(&front), Some(&back)) = (ring.first(), ring.last()) else {
        return 0.0;
    };
    let mut area = if back == front {
        0.0
    } else {
        back.lng * front.lat - back.lat * front.lng
    };
    for pair in ring.windows(2) {
        area += pair[0].lng * pair[1].lat - pair[0].lat * pair[1].lng;
    }
    area * 0.5
}

/// Closes an open ring and makes it clockwise (`correct_ring`).
fn correct_ring(ring: &mut Vec<PointLL>) {
    if let (Some(&first), Some(&last)) = (ring.first(), ring.last())
        && (first.lat != last.lat || first.lng != last.lng)
    {
        ring.push(first);
    }
    if polygon_area(ring) > 0.0 {
        ring.reverse();
    }
}

/// The quadrant of `pt1` relative to `pt2` (`quadrant_type`).
fn quadrant(pt1: PointLL, pt2: PointLL) -> i8 {
    if pt1.lng > pt2.lng {
        if pt1.lat > pt2.lat { 0 } else { 3 }
    } else if pt1.lat > pt2.lat {
        1
    } else {
        2
    }
}

/// Whether a point is inside a polygon, by winding (`point_in_poly`).
fn point_in_poly(pt: PointLL, poly: &[PointLL]) -> bool {
    let Some(&front) = poly.first() else {
        return false;
    };
    let mut quad = quadrant(front, pt);
    let mut angle: i8 = 0;
    for i in 0..poly.len() {
        let vertex = poly[i];
        let next_vertex = poly[(i + 1) % poly.len()];
        let next_quad = quadrant(next_vertex, pt);
        let mut delta = next_quad - quad;
        match delta {
            3 => delta = -1,
            -3 => delta = 1,
            2 | -2 => {
                // `x_intercept`: where the edge crosses the point's latitude.
                let x = next_vertex.lng
                    - ((next_vertex.lat - pt.lat)
                        * ((vertex.lng - next_vertex.lng) / (vertex.lat - next_vertex.lat)));
                if x > pt.lng {
                    delta = -delta;
                }
            }
            _ => {}
        }
        angle = angle.wrapping_add(delta);
        quad = next_quad;
    }
    angle == 4 || angle == -4
}

/// The bounding box of a segment (`box_from_shape` over two points).
fn segment_box(a: PointLL, b: PointLL) -> Aabb {
    Aabb {
        minx: if b.lng < a.lng { b.lng } else { a.lng },
        miny: if b.lat < a.lat { b.lat } else { a.lat },
        maxx: if b.lng > a.lng { b.lng } else { a.lng },
        maxy: if b.lat > a.lat { b.lat } else { a.lat },
    }
}

/// Whether two boxes overlap, edges included (as `flatbush` searches).
fn boxes_overlap(a: &Aabb, b: &Aabb) -> bool {
    !(a.maxx < b.minx || a.maxy < b.miny || a.minx > b.maxx || a.miny > b.maxy)
}

/// The indexes of the ring's segments whose boxes overlap `query` (mainline's r-tree search;
/// the order doesn't matter to its callers).
fn segments_near(ring: &[PointLL], query: &Aabb) -> impl Iterator<Item = usize> {
    ring.windows(2)
        .enumerate()
        .filter(move |(_, pair)| boxes_overlap(&segment_box(pair[0], pair[1]), query))
        .map(|(i, _)| i)
}

/// Whether a point is inside a ring, by casting a ray east (`point_in_ring`).
fn point_in_ring(pt: PointLL, ring: &[PointLL]) -> bool {
    let query = Aabb {
        minx: pt.lng,
        miny: pt.lat,
        maxx: f64::MAX,
        maxy: pt.lat,
    };
    let mut crossings = 0;
    for idx in segments_near(ring, &query) {
        let a = ring[idx];
        let b = ring[idx + 1];
        // Half open, so vertices aren't counted twice.
        if !((a.lat <= pt.lat && b.lat > pt.lat) || (b.lat <= pt.lat && a.lat > pt.lat)) {
            continue;
        }
        let x_intersect = a.lng + (pt.lat - a.lat) * (b.lng - a.lng) / (b.lat - a.lat);
        if pt.lng < x_intersect {
            crossings += 1;
        }
    }
    crossings % 2 == 1
}

/// Whether two segments intersect (`LineSegment2::Intersect`).
fn segments_intersect(a1: PointLL, a2: PointLL, b1: PointLL, b2: PointLL) -> bool {
    let (bx, by) = (a2.lng - a1.lng, a2.lat - a1.lat);
    let (dx, dy) = (b2.lng - b1.lng, b2.lat - b1.lat);
    // The perpendicular of d, counter-clockwise.
    let (dpx, dpy) = (-dy, dx);
    let dtb = dpx * bx + dpy * by;
    if dtb == 0.0 {
        return false;
    }
    let (cx, cy) = (b1.lng - a1.lng, b1.lat - a1.lat);
    let t = (dpx * cx + dpy * cy) / dtb;
    if t < 0.0 || t > 1.0 {
        return false;
    }
    let (bpx, bpy) = (-by, bx);
    let u = (bpx * cx + bpy * cy) / dtb;
    !(u < 0.0 || u > 1.0)
}

/// Whether a line crosses a ring or lies inside it (`line_intersects_ring`).
fn line_intersects_ring(line: &[PointLL], ring: &[PointLL]) -> bool {
    for pair in line.windows(2) {
        let query = segment_box(pair[0], pair[1]);
        for idx in segments_near(ring, &query) {
            if segments_intersect(pair[0], pair[1], ring[idx], ring[idx + 1]) {
                return true;
            }
        }
    }
    line.first().is_some_and(|&p| point_in_ring(p, ring))
}

/// Resamples a polyline along great circles at `resolution` meters, keeping its points
/// (`resample_spherical_polyline` with `preserve`).
fn resample_spherical_polyline(polyline: &[PointLL], resolution: f64) -> Vec<PointLL> {
    let Some(&first) = polyline.first() else {
        return Vec::new();
    };
    let mut resampled = vec![first];
    let resolution = resolution * RAD_PER_METER;
    let mut remaining = resolution;
    let mut last = first;
    for &p in &polyline[1..] {
        let lon2 = p.lng * -RAD_PER_DEG_D;
        let lat2 = p.lat * RAD_PER_DEG_D;
        let mut d = if last == p {
            0.0
        } else {
            ((last.lat * RAD_PER_DEG_D).sin() * lat2.sin()
                + (last.lat * RAD_PER_DEG_D).cos()
                    * lat2.cos()
                    * (last.lng * -RAD_PER_DEG_D - lon2).cos())
            .acos()
        };
        if d.is_nan() {
            d = 0.0;
        }
        while d > remaining {
            let lon1 = last.lng * -RAD_PER_DEG_D;
            let lat1 = last.lat * RAD_PER_DEG_D;
            let sd = d.sin();
            let a = (d - remaining).sin() / sd;
            let acs1 = a * lat1.cos();
            let b = remaining.sin() / sd;
            let bcs2 = b * lat2.cos();
            let x = acs1 * lon1.cos() + bcs2 * lon2.cos();
            let y = acs1 * lon1.sin() + bcs2 * lon2.sin();
            let z = a * lat1.sin() + b * lat2.sin();
            last.lng = y.atan2(x) * -DEG_PER_RAD_D;
            last.lat = z.atan2((x * x + y * y).sqrt()) * DEG_PER_RAD_D;
            resampled.push(last);
            d -= remaining;
            remaining = resolution;
        }
        remaining -= d;
        last = p;
        resampled.push(last);
    }
    resampled
}

/// The length of a polyline in meters (`Polyline2::Length`).
fn length(points: &[PointLL]) -> f64 {
    points.windows(2).map(|p| p[0].distance(p[1])).sum()
}

/// Bins by tile, as (tile, bin) pairs.
type Bins = BTreeSet<(i32, u16)>;

/// Marks the bins a pixel is in (`set_pixel`); returns whether it is outside the grid.
fn set_pixel(bins: &mut Bins, x: i32, y: i32) -> bool {
    if x < 0 || y < 0 || x >= BINS_DIM * COLUMNS || y >= BINS_DIM * ROWS {
        return true;
    }
    let tile = (y / BINS_DIM) * COLUMNS + x / BINS_DIM;
    let bin = (y % BINS_DIM) * BINS_DIM + x % BINS_DIM;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the bin is in 0..25"
    )]
    bins.insert((tile, bin as u16));
    false
}

/// `std::floor` to an `int`, as mainline converts it.
#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline converts the floor to int; coordinates are within the grid"
)]
fn floor_i32(v: f64) -> i32 {
    v.floor() as i32
}

/// Rasterizes a segment over the bin grid, marking every bin it touches (`bresenham_line`).
fn bresenham_line(bins: &mut Bins, x0: f64, y0: f64, x1: f64, y1: f64) {
    let mut outside = set_pixel(bins, floor_i32(x0), floor_i32(y0));
    let sx = if x0 < x1 { 1.0 } else { -1.0 };
    let dx = x1 - x0;
    let mut x = x0.floor() + 0.5;
    let sy = if y0 < y1 { 1.0 } else { -1.0 };
    let dy = y1 - y0;
    let mut y = y0.floor() + 0.5;
    // Each step moves one pixel; bound the loop so that NaN input can't spin.
    let bound = (dx.abs() + dy.abs()).min(1e7) + 4.0;
    let mut steps = 0.0;
    while (x.floor() != x1.floor() || y.floor() != y1.floor()) && steps < bound {
        steps += 1.0;
        let tx = (dx * (y - y0) - dy * ((x + sx) - x0)).abs();
        let ty = (dx * ((y + sy) - y0) - dy * (x - x0)).abs();
        if tx < ty || (tx == ty && y0 == y1) {
            x += sx;
        } else {
            y += sy;
        }
        let o = set_pixel(bins, floor_i32(x), floor_i32(y));
        if !outside && o {
            return;
        }
        outside = o;
    }
}

/// The bins a ring's boundary crosses (`Tiles::Intersect` of a linestring).
fn intersect(ring: &[PointLL]) -> Bins {
    let mut bins = Bins::new();
    let Some(front) = ring.first() else {
        return bins;
    };
    // Long segments are resampled so that the great circle between their points is followed.
    let max_meters = cpp_max(
        1.0,
        f64::from(SUBDIVISION_SIZE) * 0.25 * meters_per_lng_degree(front.lat),
    );
    let resampled;
    let line = if length(ring) > max_meters {
        resampled = resample_spherical_polyline(ring, max_meters);
        &resampled
    } else {
        ring
    };
    // `(x - minx) / width * ncolumns * nsubdivisions`, multiplied in that order.
    let to_pixel = |p: PointLL| {
        (
            (p.lng + 180.0) / 360.0 * f64::from(COLUMNS) * f64::from(BINS_DIM),
            (p.lat + 90.0) / 180.0 * f64::from(ROWS) * f64::from(BINS_DIM),
        )
    };
    if line.len() == 1 {
        let (x, y) = to_pixel(line[0]);
        set_pixel(&mut bins, floor_i32(x), floor_i32(y));
        return bins;
    }
    for pair in line.windows(2) {
        let (x0, y0) = to_pixel(pair[0]);
        let (x1, y1) = to_pixel(pair[1]);
        let (ix0, ix1, iy0, iy1) = (floor_i32(x0), floor_i32(x1), floor_i32(y0), floor_i32(y1));
        let ddx = i64::from(ix0) - i64::from(ix1);
        let ddy = i64::from(iy0) - i64::from(iy1);
        let ds = ddx * ddx + ddy * ddy;
        if ds == 0 {
            set_pixel(&mut bins, ix0, iy0);
        } else if ds == 1 {
            set_pixel(&mut bins, ix0, iy0);
            set_pixel(&mut bins, ix1, iy1);
        } else {
            bresenham_line(&mut bins, x0, y0, x1, y1);
        }
    }
    bins
}

/// Whether a circle is entirely outside a box (`circle_outside_bounds`).
fn circle_outside_bounds(center: PointLL, radius_sq: f32, bbox: &Aabb) -> bool {
    let closest = PointLL::new(
        cpp_min(cpp_max(center.lng, bbox.minx), bbox.maxx),
        cpp_min(cpp_max(center.lat, bbox.miny), bbox.maxy),
    );
    center.distance_squared(closest) > f64::from(radius_sq)
}

/// The edges (and their opposing edges) inside the request's exclude polygons
/// (mainline's `edges_in_rings`), sorted by ID.
///
/// # Errors
///
/// Fails with 177 if a ring crosses the antimeridian, 167 if the rings are longer than
/// `max_length` meters in all, 176 if they have more than `max_vertices` vertices,
/// and if the graph can't be read.
#[expect(
    clippy::too_many_lines,
    reason = "a step by step port of mainline's function"
)]
pub(crate) fn edges_in_rings<P: GraphTileProvider, C: Costing>(
    options: &Options,
    graph: Graph<'_, P>,
    costing: &C,
    max_length: f32,
    max_vertices: usize,
) -> Result<Vec<u64>, PolygonError> {
    // Bogus input means no exclusions.
    let Some(first) = options.exclude_polygons.first() else {
        return Ok(Vec::new());
    };
    match first.coords.first() {
        Some(c)
            if matches!(c.has_lat, Some(HasLat::Lat(_)))
                && matches!(c.has_lng, Some(HasLng::Lng(_))) => {}
        _ => return Ok(Vec::new()),
    }

    // Mainline keeps these in an `unordered_set<float>`, where 0 and -0 are the same level
    // and NaN is never found; comparing with `==` does the same.
    let exclude_levels: Vec<Vec<f32>> = (0..options.exclude_polygons.len())
        .map(|i| {
            options
                .exclude_levels
                .get(i)
                .map(|l| l.levels.clone())
                .unwrap_or_default()
        })
        .collect();

    let mut rings = Vec::with_capacity(options.exclude_polygons.len());
    let mut rings_length = 0.0;
    let mut vertex_count = 0;
    for ring in &options.exclude_polygons {
        let mut points: Vec<PointLL> = ring.coords.iter().map(|c| point_of(Some(c))).collect();
        if points.is_empty() {
            // Mainline reads the first point of an empty ring; such a ring excludes nothing.
            continue;
        }
        correct_ring(&mut points);
        for pair in points.windows(2) {
            rings_length += pair[0].distance(pair[1]);
            if (pair[1].lng - pair[0].lng).abs() > 180.0 {
                return Err(PolygonError::Limit(
                    ErrorCode::PolygonCrossesAntimeridian.into(),
                ));
            }
        }
        vertex_count += points.len();
        let Some(bbox) = Aabb::of_points(&points) else {
            continue;
        };
        rings.push(Ring { points, bbox });
    }
    if rings_length > f64::from(max_length) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mainline prints the limit as a size_t"
        )]
        let limit = max_length as usize;
        return Err(PolygonError::Limit(ValhallaError::with_detail(
            ErrorCode::ExcludePolygonsPerimeterExceeded,
            format!("{limit} meters"),
        )));
    }
    if vertex_count > max_vertices {
        return Err(PolygonError::Limit(ValhallaError::with_detail(
            ErrorCode::ExcludePolygonsVerticesExceeded,
            max_vertices.to_string(),
        )));
    }

    // The bins each ring's boundary crosses, and the bins entirely inside it.
    let mut intersected: BTreeMap<(i32, u16), Vec<usize>> = BTreeMap::new();
    let mut contained: BTreeMap<(i32, u16), Vec<usize>> = BTreeMap::new();
    for (ring_index, ring) in rings.iter().enumerate() {
        let boundary = intersect(&ring.points);
        for &bin in &boundary {
            intersected.entry(bin).or_default().push(ring_index);
        }
        // Flood fill from the first neighbor of a boundary bin that is inside the ring
        // (only the first area found is filled, as in mainline).
        let start = boundary.iter().find_map(|&(tile, bin)| {
            neighboring_bins(tile, bin, false)
                .into_iter()
                .find(|neighbor| {
                    if boundary.contains(neighbor) {
                        return false;
                    }
                    let bbox = bin_bbox(neighbor.0, neighbor.1);
                    let center = PointLL::new(
                        f64::midpoint(bbox.minx, bbox.maxx),
                        f64::midpoint(bbox.miny, bbox.maxy),
                    );
                    point_in_poly(center, &ring.points)
                })
        });
        if let Some(start) = start {
            let mut processed = HashSet::from([start]);
            let mut queue = VecDeque::from([start]);
            let mut n = 0;
            while n < 1_000_000 {
                n += 1;
                let Some(bin) = queue.pop_front() else {
                    break;
                };
                contained.entry(bin).or_default().push(ring_index);
                for neighbor in neighboring_bins(bin.0, bin.1, true) {
                    if !processed.insert(neighbor) || boundary.contains(&neighbor) {
                        continue;
                    }
                    queue.push_back(neighbor);
                }
            }
        }
    }

    let mut avoid: BTreeSet<u64> = BTreeSet::new();
    let check = |avoid: &mut BTreeSet<u64>,
                 (tile_index, bin): (i32, u16),
                 ring_indexes: &[usize],
                 intersect: bool|
     -> Result<(), PolygonError> {
        let Some(bin_tile_id) = u64::try_from(tile_index)
            .ok()
            .and_then(|t| GraphId::try_from_components(2, t, 0).ok())
        else {
            return Ok(());
        };
        let Some(bin_tile) = graph.tile(bin_tile_id)? else {
            return Ok(());
        };
        let has_bounding_circles = bin_tile.header().has_bounding_circles();
        let circles = bin_tile.bounding_circles_in_bin(usize::from(bin));
        let center = bin_center(tile_index, bin);
        let mut tile = bin_tile.clone();
        for (edge_index, &edge_id) in bin_tile.edges_in_bin(usize::from(bin)).iter().enumerate() {
            if avoid.contains(&edge_id.value()) {
                continue;
            }
            let mut radius = 0.0;
            let mut circle_center = PointLL::default();
            if intersect
                && has_bounding_circles
                && let Some(circle) = circles
                    .get(edge_index)
                    .and_then(|c| c.decode(geo::coord! {x: center.lng, y: center.lat}))
            {
                circle_center = PointLL::from(circle.center);
                radius = circle.radius_meters;
            }
            // Grown by the margin, as in the search.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "mainline passes the squared radius as a float"
            )]
            let radius_sq =
                ((radius + CIRCLE_MARGIN_METERS) * (radius + CIRCLE_MARGIN_METERS)) as f32;

            let Some(edge_tile) = graph.tile_from(edge_id, &tile)? else {
                continue;
            };
            tile = edge_tile;
            let edge = get_edge(&tile, edge_id)?;
            let mut opp = None;
            if !costing.edge_filter(edge, &tile, DisallowMask::NONE) {
                match graph.opposing(edge_id, &tile)? {
                    Some((opp_id, opp_tile))
                        if costing.edge_filter(
                            get_edge(&opp_tile, opp_id)?,
                            &opp_tile,
                            DisallowMask::NONE,
                        ) =>
                    {
                        opp = Some(opp_id);
                    }
                    _ => continue,
                }
            }

            let mut exclude = false;
            let mut edge_shape: Option<Vec<PointLL>> = None;
            let mut levels: Option<Vec<(f32, f32)>> = None;
            for &ring_index in ring_indexes {
                let Some(ring) = rings.get(ring_index) else {
                    continue;
                };
                // A bin inside the ring means the edge is in it.
                let mut intersects = !intersect;
                if intersect
                    && (radius == 0.0
                        || !circle_outside_bounds(circle_center, radius_sq, &ring.bbox))
                {
                    if edge_shape.is_none() {
                        edge_shape =
                            Some(shape(&tile.get_edge_info(edge).map_err(GraphError::from)?)?);
                    }
                    intersects = edge_shape
                        .as_deref()
                        .is_some_and(|s| line_intersects_ring(s, &ring.points));
                }
                if !intersects {
                    continue;
                }
                let wanted = &exclude_levels[ring_index];
                if wanted.is_empty() {
                    exclude = true;
                    break;
                }
                // With levels, only edges on (not across) one of them are excluded.
                if levels.is_none() {
                    let info = tile.get_edge_info(edge).map_err(GraphError::from)?;
                    levels = Some(match info.levels().map_err(GraphError::from)? {
                        Some(l) => l
                            .ranges()
                            .collect::<Result<_, _>>()
                            .map_err(GraphError::from)?,
                        None => Vec::new(),
                    });
                }
                if let Some([(lo, hi)]) = levels.as_deref()
                    && lo == hi
                    && wanted.iter().any(|level| level == lo)
                {
                    exclude = true;
                    break;
                }
            }
            if exclude {
                avoid.insert(edge_id.value());
                let opp_id = match opp {
                    Some(id) => Some(id),
                    None => graph.opposing(edge_id, &tile)?.map(|(id, _)| id),
                };
                // Mainline inserts an invalid ID when the opposing edge's tile is missing.
                avoid.insert(opp_id.map_or(INVALID_GRAPH_ID, |id| id.value()));
            }
        }
        Ok(())
    };

    for (bin, ring_indexes) in &contained {
        check(&mut avoid, *bin, ring_indexes, false)?;
    }
    for (bin, ring_indexes) in &intersected {
        check(&mut avoid, *bin, ring_indexes, true)?;
    }
    Ok(avoid.into_iter().collect())
}

/// Mainline's invalid graph ID (`kInvalidGraphId`).
const INVALID_GRAPH_ID: u64 = 0x3fff_ffff_ffff;

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<PointLL> {
        vec![
            PointLL::new(0.0, 0.0),
            PointLL::new(1.0, 0.0),
            PointLL::new(1.0, 1.0),
            PointLL::new(0.0, 1.0),
        ]
    }

    #[test]
    fn rings_are_closed_and_clockwise() {
        let mut ring = square();
        assert!(polygon_area(&ring) > 0.0);
        correct_ring(&mut ring);
        assert_eq!(ring.len(), 5);
        assert_eq!(ring.first(), ring.last());
        assert!(polygon_area(&ring) < 0.0);
    }

    #[test]
    fn points_in_rings() {
        let mut ring = square();
        correct_ring(&mut ring);
        assert!(point_in_poly(PointLL::new(0.5, 0.5), &ring));
        assert!(!point_in_poly(PointLL::new(1.5, 0.5), &ring));
        assert!(point_in_ring(PointLL::new(0.5, 0.5), &ring));
        assert!(!point_in_ring(PointLL::new(-0.5, 0.5), &ring));
        assert!(line_intersects_ring(
            &[PointLL::new(-1.0, 0.5), PointLL::new(2.0, 0.5)],
            &ring
        ));
        assert!(line_intersects_ring(
            &[PointLL::new(0.2, 0.5), PointLL::new(0.3, 0.5)],
            &ring
        ));
        assert!(!line_intersects_ring(
            &[PointLL::new(2.0, 0.5), PointLL::new(3.0, 0.5)],
            &ring
        ));
    }

    #[test]
    fn boundaries_cover_their_bins() {
        let ring = vec![
            PointLL::new(1.51, 42.50),
            PointLL::new(1.53, 42.50),
            PointLL::new(1.53, 42.51),
            PointLL::new(1.51, 42.50),
        ];
        let bins = intersect(&ring);
        assert!(!bins.is_empty());
        // Resampling keeps the ring's own points.
        let resampled = resample_spherical_polyline(&ring, 100.0);
        assert!(resampled.len() > ring.len());
        for p in &ring {
            assert!(resampled.contains(p));
        }
    }
}
