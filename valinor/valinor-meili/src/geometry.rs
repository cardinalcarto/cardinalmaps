//! Meili's geometry: projecting onto an edge shape (mainline's `meili::helpers::Project`),
//! the box around a measurement (`midgard::ExpandMeters`),
//! the tiling of the local level's bins (`midgard::Tiles<PointLL>` with the bin size),
//! and turn angles (`midgard::get_turn_degree180`).
//!
//! As in Loki, candidates are decided by exact comparisons of these values
//! (a projection snaps to a node, distances are compared with the search radius),
//! so each function follows mainline's arithmetic, including where it rounds to `float`.

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares floats exactly"
)]

use valhalla_graphtile::midgard::{
    Aabb, METERS_PER_DEGREE_LAT, PointLL, Projector, meters_per_lng_degree, narrow,
};

/// Percentages this close to 0 are 0, and as close to 1 are 1 (in `Project`).
const DOUBLE_PRECISION_AT_ONE: f64 = 1e-15;
const VERY_CLOSE_TO_ONE: f64 = 1.0 - DOUBLE_PRECISION_AT_ONE;

/// A projection onto a shape: the closest point, its approximate squared distance,
/// and how far along the shape it is (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Projection {
    pub(crate) point: PointLL,
    pub(crate) sq_distance: f64,
    pub(crate) percent_along: f64,
}

/// Mainline's `meili::helpers::Project`: the closest point of `shape` to the projector's point,
/// snapped to the first or last shape point when it is within `snap_distance` meters of it.
///
/// Returns `None` for an empty shape (mainline's callers skip those first).
pub(crate) fn project(p: &Projector, shape: &[PointLL], snap_distance: f64) -> Option<Projection> {
    let (&first, rest) = shape.split_first()?;
    let mut closest_point = first;
    let mut closest_segment_point = first;
    let mut closest_distance = f64::MAX;
    let mut closest_partial_length = 0.0;
    let mut total_length = 0.0;
    let mut u = first;
    for &v in rest {
        let projection = p.project(u, v);
        let distance = p.approx.distance_squared(projection);
        if distance < closest_distance {
            closest_point = projection;
            closest_distance = distance;
            closest_partial_length = total_length;
            closest_segment_point = u;
        }
        total_length += u.distance(v);
        u = v;
    }
    closest_partial_length += closest_segment_point.distance(closest_point);
    let mut percent_along = if total_length > 0.0 {
        closest_partial_length / total_length
    } else {
        0.0
    };
    // Mainline snaps values within double precision of 0 and 1.
    if percent_along < DOUBLE_PRECISION_AT_ONE {
        percent_along = 0.0;
    } else if percent_along > VERY_CLOSE_TO_ONE {
        percent_along = 1.0;
    }
    if total_length * percent_along <= snap_distance {
        closest_point = first;
        closest_distance = p.approx.distance_squared(closest_point);
        percent_along = 0.0;
    } else if total_length * (1.0 - percent_along) <= snap_distance {
        // Mainline's `1.f - percent_along` is a double: the float 1 is promoted.
        closest_point = u;
        closest_distance = p.approx.distance_squared(closest_point);
        percent_along = 1.0;
    }
    Some(Projection {
        point: closest_point,
        sq_distance: closest_distance,
        percent_along,
    })
}

/// The box `meters` around a point (`midgard::ExpandMeters(const PointLL&, float)`).
///
/// The deltas are floats: `meters / kMetersPerDegreeLat` and `meters / MetersPerLngDegree(lat)`
/// are double quotients stored in floats.
pub(crate) fn expand_meters(pt: PointLL, meters: f32) -> Aabb {
    let dlat = narrow(f64::from(meters) / METERS_PER_DEGREE_LAT);
    let dlng = narrow(f64::from(meters) / meters_per_lng_degree(pt.lat));
    Aabb {
        minx: pt.lng - f64::from(dlng),
        miny: pt.lat - f64::from(dlat),
        maxx: pt.lng + f64::from(dlng),
        maxy: pt.lat + f64::from(dlat),
    }
}

/// The size of a local level tile in degrees (`TileHierarchy::levels().back().tiles.TileSize()`).
pub(crate) const LOCAL_TILE_SIZE: f32 = 0.25;
/// Local tiles across the world.
const LOCAL_COLUMNS: i32 = 1440;
/// Bins per side of a local tile (`nsubdivisions`).
pub(crate) const BINS_PER_SIDE: i32 = 5;
/// The size of a bin (`SubdivisionSize`, a float quotient).
const BIN_SIZE: f32 = LOCAL_TILE_SIZE / 5.0;
/// Bins across and down the world: mainline's `Tiles(bounds, tilesize)` rounds
/// `360 / 0.05f` and `180 / 0.05f` (as doubles) to the nearest integer.
const BIN_COLUMNS: i32 = 7200;
const BIN_ROWS: i32 = 3600;

/// The world's bounds.
const MIN_X: f64 = -180.0;
const MIN_Y: f64 = -90.0;
const MAX_X: f64 = 180.0;
const MAX_Y: f64 = 90.0;

/// The bin row of a latitude, or -1 outside the world (`Tiles::Row`).
fn bin_row(y: f64) -> i32 {
    if !(MIN_Y..=MAX_Y).contains(&y) {
        return -1;
    }
    if y == MAX_Y {
        BIN_ROWS - 1
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline casts to int32; y is within the world"
        )]
        let row = ((y - MIN_Y) / f64::from(BIN_SIZE)) as i32;
        row
    }
}

/// The bin column of a longitude, or -1 outside the world (`Tiles::Col`).
fn bin_col(x: f64) -> i32 {
    if !(MIN_X..=MAX_X).contains(&x) {
        return -1;
    }
    let col = (x - MIN_X) / f64::from(BIN_SIZE);
    if col >= f64::from(BIN_COLUMNS) {
        return BIN_COLUMNS - 1;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mainline casts to int32; x is within the world"
    )]
    let col = if col >= 0.0 {
        col as i32
    } else {
        (col - 1.0) as i32
    };
    col
}

/// The bins a box touches, row by row (`Tiles::TileList(const AABB2&)` on the bin tiling,
/// which wraps in longitude).
pub(crate) fn bin_list(bbox: &Aabb, out: &mut Vec<i32>) {
    out.clear();
    let width = MAX_X - MIN_X;
    let world = Aabb {
        minx: MIN_X,
        miny: MIN_Y,
        maxx: MAX_X,
        maxy: MAX_Y,
    };
    let mut boxes = [world; 2];
    let count = if bbox.minx < MIN_X && bbox.maxx > MIN_X {
        boxes[0] = Aabb {
            minx: MIN_X,
            ..*bbox
        };
        boxes[1] = Aabb {
            minx: bbox.minx + width,
            maxx: MAX_X,
            ..*bbox
        };
        2
    } else if bbox.minx < MAX_X && bbox.maxx > MAX_X {
        boxes[0] = Aabb {
            maxx: MAX_X,
            ..*bbox
        };
        boxes[1] = Aabb {
            minx: MIN_X,
            maxx: bbox.maxx - width,
            ..*bbox
        };
        2
    } else {
        boxes[0] = bbox.intersection(&world);
        1
    };
    for bb in &boxes[..count] {
        let minrow = bin_row(bb.miny).max(0);
        let maxrow = bin_row(bb.maxy).max(0);
        let mincol = bin_col(bb.minx).max(0);
        let maxcol = bin_col(bb.maxx).max(0);
        for row in minrow..=maxrow {
            out.extend((mincol..=maxcol).map(|col| row * BIN_COLUMNS + col));
        }
    }
}

/// The local tile of a bin and the bin's index within it (mainline's `CandidateGridQuery::GetGrid`).
pub(crate) const fn bin_tile(bin_id: i32) -> (i32, usize) {
    let row = bin_id / BIN_COLUMNS;
    let col = bin_id % BIN_COLUMNS;
    let tile_id = (row / BINS_PER_SIDE) * LOCAL_COLUMNS + col / BINS_PER_SIDE;
    #[expect(
        clippy::cast_sign_loss,
        reason = "bin IDs from bin_list are non-negative"
    )]
    let index = ((row % BINS_PER_SIDE) * BINS_PER_SIDE + col % BINS_PER_SIDE) as usize;
    (tile_id, index)
}

/// The bounds of a local tile (`GraphTile::BoundingBox`: `Tiles::TileBounds(tileid)`,
/// whose base adds the float products `col * tilesize` to the double bounds).
pub(crate) fn local_tile_bounds(tile_id: i32) -> Aabb {
    let row = tile_id / LOCAL_COLUMNS;
    let col = tile_id - row * LOCAL_COLUMNS;
    #[expect(
        clippy::cast_precision_loss,
        reason = "mainline converts the int to a float; tile rows and columns are exact"
    )]
    let (x, y) = (
        MIN_X + f64::from(col as f32 * LOCAL_TILE_SIZE),
        MIN_Y + f64::from(row as f32 * LOCAL_TILE_SIZE),
    );
    Aabb {
        minx: x,
        miny: y,
        maxx: x + f64::from(LOCAL_TILE_SIZE),
        maxy: y + f64::from(LOCAL_TILE_SIZE),
    }
}

/// The turn between two headings, in degrees from 0 (straight) to 180 (a U-turn)
/// (`midgard::get_turn_degree180`), or `None` for a heading outside `[0, 360)`
/// (mainline throws).
pub(crate) fn turn_degree180(inbound: u16, outbound: u16) -> Option<usize> {
    if !(inbound < 360 && outbound < 360) {
        return None;
    }
    let turn = inbound.abs_diff(outbound);
    Some(usize::from(if 180 < turn { 360 - turn } else { turn }))
}

/// A heading in degrees clamped to `[0, 359]` and truncated, as mainline's routing does with
/// shape headings: `static_cast<uint16_t>(std::max(0.f, std::min(359.f, heading)))`.
pub(crate) fn clamp_heading(heading: f64) -> u16 {
    let heading = narrow(heading);
    // `std::min(359.f, h)` is `h < 359 ? h : 359`, and `std::max(0.f, m)` is `0 < m ? m : 0`.
    let min = if heading < 359.0 { heading } else { 359.0 };
    let clamped = if 0.0 < min { min } else { 0.0 };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is within [0, 359]"
    )]
    let degrees = clamped as u16;
    degrees
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_and_tiles_agree() {
        // Andorra la Vella.
        let p = PointLL::new(1.5211, 42.5078);
        let mut bins = Vec::new();
        bin_list(&expand_meters(p, 1.0), &mut bins);
        assert_eq!(bins.len(), 1);
        let (tile, index) = bin_tile(bins[0]);
        let bounds = local_tile_bounds(tile);
        assert!(bounds.minx <= p.lng && p.lng < bounds.maxx);
        assert!(bounds.miny <= p.lat && p.lat < bounds.maxy);
        let row = i32::try_from(index).unwrap() / BINS_PER_SIDE;
        let col = i32::try_from(index).unwrap() % BINS_PER_SIDE;
        let south = bounds.miny + f64::from(row) * 0.05;
        let west = bounds.minx + f64::from(col) * 0.05;
        assert!(south <= p.lat && p.lat < south + 0.05);
        assert!(west <= p.lng && p.lng < west + 0.05);
    }

    #[test]
    fn bin_list_wraps_at_the_antimeridian() {
        let mut bins = Vec::new();
        bin_list(
            &expand_meters(PointLL::new(179.9999, 0.0), 100.0),
            &mut bins,
        );
        assert_eq!(bins.len(), 4);
        assert!(bins.iter().any(|b| b % BIN_COLUMNS == 0));
        assert!(bins.iter().any(|b| b % BIN_COLUMNS == BIN_COLUMNS - 1));
    }

    #[test]
    fn projects_and_snaps_to_ends() {
        let shape = [PointLL::new(1.5, 42.5), PointLL::new(1.501, 42.5)];
        let p = Projector::new(PointLL::new(1.5005, 42.5001));
        let mid = project(&p, &shape, 0.0).unwrap();
        assert!((mid.percent_along - 0.5).abs() < 1e-9);
        // Within a meter of the start: the first point.
        let p = Projector::new(PointLL::new(1.500_005, 42.5001));
        let snapped = project(&p, &shape, 1.0).unwrap();
        assert_eq!(snapped.point, shape[0]);
        assert_eq!(snapped.percent_along, 0.0);
        let p = Projector::new(PointLL::new(1.500_995, 42.4999));
        let snapped = project(&p, &shape, 1.0).unwrap();
        assert_eq!(snapped.point, shape[1]);
        assert_eq!(snapped.percent_along, 1.0);
        // One point: it, at 0.
        let single = project(&p, &shape[..1], 1.0).unwrap();
        assert_eq!(single.point, shape[0]);
        assert_eq!(single.percent_along, 0.0);
        assert_eq!(project(&p, &[], 1.0), None);
    }

    #[test]
    fn turn_degrees() {
        assert_eq!(turn_degree180(10, 350), Some(20));
        assert_eq!(turn_degree180(0, 180), Some(180));
        assert_eq!(turn_degree180(90, 90), Some(0));
        assert_eq!(turn_degree180(360, 0), None);
        assert_eq!(clamp_heading(359.7), 359);
        assert_eq!(clamp_heading(12.9), 12);
        assert_eq!(clamp_heading(-0.0), 0);
    }
}
