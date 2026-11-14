//! The level 2 tiling and its bins (mainline's `midgard::Tiles<PointLL>` for the local level).
//!
//! Loki searches the edge bins of the local level (2) tiles,
//! which split each 0.25 degree tile into a 5 by 5 grid of 0.05 degree bins.
//! These functions reproduce mainline's arithmetic exactly,
//! including that offsets are computed in single precision
//! (mainline's tile and bin sizes are `float`s, so `index * size` is a float product)
//! before they are added to the double precision bounds.

use alloc::collections::BinaryHeap;
use core::cmp::Ordering;
use std::collections::HashSet;
use valhalla_graphtile::midgard::{Aabb, PointLL};

/// Bins per side of a tile (`kBinsDim`).
pub(crate) const BINS_DIM: i32 = 5;
/// The local level's tile size in degrees.
const TILE_SIZE: f32 = 0.25;
/// The local level's bin size (`SubdivisionSize`), a float division as in mainline.
pub(crate) const SUBDIVISION_SIZE: f32 = TILE_SIZE / 5.0;
/// Tiles across (`ncolumns`).
pub(crate) const COLUMNS: i32 = 1440;
/// Tiles down (`nrows`).
pub(crate) const ROWS: i32 = 720;
/// The bounds of the tiling.
const MIN_X: f64 = -180.0;
const MIN_Y: f64 = -90.0;
const MAX_X: f64 = 180.0;
const MAX_Y: f64 = 90.0;
const WIDTH: f64 = MAX_X - MIN_X;
const HEIGHT: f64 = MAX_Y - MIN_Y;
/// Bins across and down the whole tiling.
const SUBCOLS: i32 = COLUMNS * BINS_DIM;
const SUBROWS: i32 = ROWS * BINS_DIM;

/// `int * float` in single precision, as mainline computes `col * tilesize_`.
#[inline]
#[expect(
    clippy::cast_precision_loss,
    reason = "mainline converts the int to a float; the values here are small enough to be exact"
)]
fn times(i: i32, size: f32) -> f32 {
    i as f32 * size
}

/// The south west corner of a tile (`Tiles::Base`).
#[inline]
pub(crate) fn tile_base(tile_id: i32) -> PointLL {
    let row = tile_id / COLUMNS;
    let col = tile_id - row * COLUMNS;
    PointLL::new(
        MIN_X + f64::from(times(col, TILE_SIZE)),
        MIN_Y + f64::from(times(row, TILE_SIZE)),
    )
}

/// The center of a bin, as Loki's search computes it (`projector_wrapper::next_bin`):
/// `(bin / kBinsDim) * size + size / 2`, all in single precision, added to the tile's base.
#[inline]
pub(crate) fn bin_center(tile_id: i32, bin: u16) -> PointLL {
    let base = tile_base(tile_id);
    let bin = i32::from(bin);
    let lat_offset = times(bin / BINS_DIM, SUBDIVISION_SIZE) + SUBDIVISION_SIZE / 2.0;
    let lng_offset = times(bin % BINS_DIM, SUBDIVISION_SIZE) + SUBDIVISION_SIZE / 2.0;
    PointLL::new(
        base.lng + f64::from(lng_offset),
        base.lat + f64::from(lat_offset),
    )
}

/// The bounding box of a bin (`Tiles::BinBBox`).
pub(crate) fn bin_bbox(tile_id: i32, bin: u16) -> Aabb {
    let bin = i32::from(bin);
    let row = bin / BINS_DIM;
    let col = bin % BINS_DIM;
    let base = tile_base(tile_id);
    // The bin indexes are cast to `double` here, and the size is promoted.
    let west = base.lng + f64::from(col) * f64::from(SUBDIVISION_SIZE);
    let south = base.lat + f64::from(row) * f64::from(SUBDIVISION_SIZE);
    Aabb {
        minx: west,
        miny: south,
        maxx: west + f64::from(SUBDIVISION_SIZE),
        maxy: south + f64::from(SUBDIVISION_SIZE),
    }
}

/// A queued bin and its distance, ordered so that a max-heap pops the closest first,
/// and among equal distances the lowest bin (as mainline's priority queue comparator does).
#[derive(Clone, Copy, Debug)]
struct Queued {
    distance: f64,
    subdivision: i32,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Queued {}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed: the smaller distance (then the smaller bin) is the greater element.
        other
            .distance
            .total_cmp(&self.distance)
            .then(other.subdivision.cmp(&self.subdivision))
    }
}

/// Yields the bins of the local level closest first from a point
/// (mainline's `Tiles::ClosestFirst` and its `closest_first_generator_t`).
///
/// Each step gives a tile ID, a bin within it, and the distance in meters from the point
/// to the closest part of the bin.
#[derive(Debug)]
pub(crate) struct ClosestFirst {
    seed: PointLL,
    queued: HashSet<i32>,
    queue: BinaryHeap<Queued>,
}

impl ClosestFirst {
    pub(crate) fn new(seed: PointLL) -> Self {
        let x = (seed.lng - MIN_X) / WIDTH * f64::from(SUBCOLS);
        let y = (seed.lat - MIN_Y) / HEIGHT * f64::from(SUBROWS);
        // `static_cast<int32_t>(y) * subcols + static_cast<int32_t>(x)`, in 32-bit arithmetic.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline casts to int32 (Rust saturates where C++ is undefined)"
        )]
        let subdivision = (y as i32).wrapping_mul(SUBCOLS).wrapping_add(x as i32);
        let mut generator = Self {
            seed,
            queued: HashSet::with_capacity(100),
            queue: BinaryHeap::new(),
        };
        generator.queued.insert(subdivision);
        generator.queue.push(Queued {
            distance: 0.0,
            subdivision,
        });
        generator.neighbors(subdivision);
        generator
    }

    /// The distance to the closest corner or edge midpoint of a bin (`dist`).
    fn dist(&self, sub: i32) -> f64 {
        let x = sub % SUBCOLS;
        let x0 = MIN_X + f64::from(times(x, SUBDIVISION_SIZE));
        let x1 = MIN_X + f64::from(times(x + 1, SUBDIVISION_SIZE));
        let y = sub / SUBCOLS;
        let y0 = MIN_Y + f64::from(times(y, SUBDIVISION_SIZE));
        let y1 = MIN_Y + f64::from(times(y + 1, SUBDIVISION_SIZE));
        let seed = self.seed;
        let mut corners = [PointLL::default(); 8];
        corners[..4].copy_from_slice(&[
            PointLL::new(x0, y0),
            PointLL::new(x1, y0),
            PointLL::new(x0, y1),
            PointLL::new(x1, y1),
        ]);
        let mut n = 4;
        if x0 < seed.lng && x1 > seed.lng {
            corners[n] = PointLL::new(seed.lng, y0);
            corners[n + 1] = PointLL::new(seed.lng, y1);
            n += 2;
        }
        if y0 < seed.lat && y1 > seed.lat {
            corners[n] = PointLL::new(x0, seed.lat);
            corners[n + 1] = PointLL::new(x1, seed.lat);
            n += 2;
        }
        let mut distance = f64::MAX;
        for c in &corners[..n] {
            let d = seed.distance(*c);
            if d < distance {
                distance = d;
            }
        }
        distance
    }

    /// Queues the four neighbors of a bin, wrapping around the antimeridian (`neighbors`).
    fn neighbors(&mut self, s: i32) {
        let x = s % SUBCOLS;
        let y = s / SUBCOLS;
        for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
            let ny = y + dy;
            if ny == -1 || ny == SUBROWS {
                continue;
            }
            let mut nx = x + dx;
            if nx == -1 || nx == SUBCOLS {
                nx = (nx + SUBCOLS) % SUBCOLS;
            }
            let neighbor = ny.wrapping_mul(SUBCOLS).wrapping_add(nx);
            if self.queued.insert(neighbor) {
                let distance = self.dist(neighbor);
                self.queue.push(Queued {
                    distance,
                    subdivision: neighbor,
                });
            }
        }
    }

    /// The next closest bin, or `None` once every bin has been visited
    /// (mainline throws "Subdivisions were exhausted").
    pub(crate) fn next_bin(&mut self) -> Option<(i32, u16, f64)> {
        let best = self.queue.pop()?;
        self.neighbors(best.subdivision);
        let sx = best.subdivision % SUBCOLS;
        let sy = best.subdivision / SUBCOLS;
        let tile_column = sx / BINS_DIM;
        let tile_row = sy / BINS_DIM;
        let tile = tile_row * COLUMNS + tile_column;
        let subdivision = (sy - tile_row * BINS_DIM) * BINS_DIM + (sx - tile_column * BINS_DIM);
        // Mainline stores this in an `unsigned short`.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mainline converts the int to an unsigned short"
        )]
        let subdivision = subdivision as u16;
        Some((tile, subdivision, best.distance))
    }
}

/// The tile row of a latitude, or -1 outside the tiling (`Tiles::Row`).
fn row(y: f64) -> i32 {
    if !(MIN_Y..=MAX_Y).contains(&y) {
        return -1;
    }
    if y == MAX_Y {
        ROWS - 1
    } else {
        // The size is promoted to a double.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline casts to int32; y is within the tiling"
        )]
        let row = ((y - MIN_Y) / f64::from(TILE_SIZE)) as i32;
        row
    }
}

/// The tile column of a longitude, or -1 outside the tiling (`Tiles::Col`).
fn col(x: f64) -> i32 {
    if !(MIN_X..=MAX_X).contains(&x) {
        return -1;
    }
    let col = (x - MIN_X) / f64::from(TILE_SIZE);
    if col >= f64::from(COLUMNS) {
        return COLUMNS - 1;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mainline casts to int32; x is within the tiling"
    )]
    let col = if col >= 0.0 {
        col as i32
    } else {
        (col - 1.0) as i32
    };
    col
}

/// The tiles a bounding box touches, row by row (`Tiles::TileList(const AABB2&)`).
pub(crate) fn tile_list(bbox: &Aabb) -> Vec<i32> {
    let bounds = Aabb {
        minx: MIN_X,
        miny: MIN_Y,
        maxx: MAX_X,
        maxy: MAX_Y,
    };
    let mut boxes = Vec::with_capacity(2);
    // The tiling wraps in x.
    if bbox.minx < MIN_X && bbox.maxx > MIN_X {
        boxes.push(Aabb {
            minx: MIN_X,
            miny: bbox.miny,
            maxx: bbox.maxx,
            maxy: bbox.maxy,
        });
        boxes.push(Aabb {
            minx: bbox.minx + WIDTH,
            miny: bbox.miny,
            maxx: MAX_X,
            maxy: bbox.maxy,
        });
    } else if bbox.minx < MAX_X && bbox.maxx > MAX_X {
        boxes.push(Aabb {
            minx: bbox.minx,
            miny: bbox.miny,
            maxx: MAX_X,
            maxy: bbox.maxy,
        });
        boxes.push(Aabb {
            minx: MIN_X,
            miny: bbox.miny,
            maxx: bbox.maxx - WIDTH,
            maxy: bbox.maxy,
        });
    } else {
        boxes.push(bbox.intersection(&bounds));
    }
    let mut tiles = Vec::new();
    for bb in boxes {
        let minrow = row(bb.miny).max(0);
        let maxrow = row(bb.maxy).max(0);
        let mincol = col(bb.minx).max(0);
        let maxcol = col(bb.maxx).max(0);
        for r in minrow..=maxrow {
            tiles.extend((mincol..=maxcol).map(|c| r * COLUMNS + c));
        }
    }
    tiles
}

/// The number of tiles (`Tiles::TileCount`).
const TILE_COUNT: i32 = COLUMNS * ROWS;

/// The tile to the west, wrapping around (`Tiles::LeftNeighbor`).
pub(crate) const fn left_neighbor(tile: i32) -> i32 {
    if tile - (tile / COLUMNS) * COLUMNS > 0 {
        tile - 1
    } else {
        tile + COLUMNS - 1
    }
}

/// The tile to the east, wrapping around (`Tiles::RightNeighbor`).
pub(crate) const fn right_neighbor(tile: i32) -> i32 {
    if tile - (tile / COLUMNS) * COLUMNS < COLUMNS - 1 {
        tile + 1
    } else {
        tile - COLUMNS + 1
    }
}

/// The tile to the north, or the tile itself on the top row (`Tiles::TopNeighbor`).
pub(crate) const fn top_neighbor(tile: i32) -> i32 {
    if tile < TILE_COUNT - COLUMNS {
        tile + COLUMNS
    } else {
        tile
    }
}

/// The tile to the south, or the tile itself on the bottom row (`Tiles::BottomNeighbor`).
pub(crate) const fn bottom_neighbor(tile: i32) -> i32 {
    if tile < COLUMNS { tile } else { tile - COLUMNS }
}

/// A bin's neighbors as (tile, bin), clockwise from the lower left
/// (`Tiles::GetNeighboringBins`); with `four_way`, only the sides.
pub(crate) fn neighboring_bins(tile: i32, bin: u16, four_way: bool) -> Vec<(i32, u16)> {
    // Lower left, then clockwise.
    const DX: [i32; 8] = [-1, -1, -1, 0, 1, 1, 1, 0];
    const DY: [i32; 8] = [-1, 0, 1, 1, 1, 0, -1, -1];
    let tx = tile % COLUMNS;
    let ty = tile / COLUMNS;
    let bin = i32::from(bin);
    let global_x = tx * BINS_DIM + bin % BINS_DIM;
    let global_y = ty * BINS_DIM + bin / BINS_DIM;
    let (start, step) = if four_way { (1, 2) } else { (0, 1) };
    (start..8)
        .step_by(step)
        .map(|i| {
            // Mainline only moves in x away from the edges of the tiling (it doesn't wrap).
            let nx = if global_x == COLUMNS * BINS_DIM || global_x == 0 {
                global_x
            } else {
                global_x + DX[i]
            };
            let ny = global_y + DY[i];
            let new_tile = nx / BINS_DIM + (ny / BINS_DIM) * COLUMNS;
            let new_bin = (nx % BINS_DIM) + (ny % BINS_DIM) * BINS_DIM;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "mainline converts to uint32 and unsigned short"
            )]
            let new_bin = new_bin as u16;
            (new_tile, new_bin)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp, reason = "the values are exact")]
    use super::*;

    #[test]
    fn closest_first_starts_in_the_seed_bin_and_grows() {
        let seed = PointLL::new(1.5211, 42.5078);
        let mut bins = ClosestFirst::new(seed);
        let (tile, bin, distance) = bins.next_bin().unwrap();
        // Andorra la Vella is in tile 2/763926.
        assert_eq!(tile, 763_926);
        assert_eq!(bin, 0);
        assert_eq!(distance, 0.0);
        let mut last = 0.0;
        for _ in 0..50 {
            let (_, _, d) = bins.next_bin().unwrap();
            assert!(d >= last);
            last = d;
        }
    }

    #[test]
    fn bin_centers_are_inside_their_tiles() {
        let center = bin_center(763_926, 0);
        assert!((center.lng - 1.525).abs() < 1e-6);
        assert!((center.lat - 42.525).abs() < 1e-6);
        let bbox = bin_bbox(763_926, 24);
        assert!((bbox.maxx - 1.75).abs() < 1e-6 && (bbox.maxy - 42.75).abs() < 1e-6);
    }

    #[test]
    fn tile_lists_and_neighbors() {
        let p = PointLL::new(1.5211, 42.5078);
        let bbox = Aabb {
            minx: p.lng,
            miny: p.lat,
            maxx: p.lng,
            maxy: p.lat,
        };
        assert_eq!(tile_list(&bbox), vec![763_926]);
        assert_eq!(left_neighbor(0), COLUMNS - 1);
        assert_eq!(right_neighbor(COLUMNS - 1), 0);
        assert_eq!(bottom_neighbor(5), 5);
        assert_eq!(top_neighbor(TILE_COUNT - 1), TILE_COUNT - 1);
        assert_eq!(neighboring_bins(763_926, 12, true).len(), 4);
        assert_eq!(neighboring_bins(763_926, 12, false).len(), 8);
    }
}
