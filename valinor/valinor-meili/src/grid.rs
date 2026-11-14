//! The grid that candidate search indexes edge segments in
//! (mainline's `GridTraversal` and `GridRangeQuery`).
//!
//! Each bin of a local tile gets a grid over the whole tile, of `grid.size` cells per side,
//! holding the segments of the bin's edges in every cell they cross.
//! A query returns the edges in the cells a box touches.
//! The cells a segment crosses are found by mainline's ray walk, in double precision,
//! so that exactly mainline's edges are found near each measurement.

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares floats exactly"
)]

use std::collections::HashMap;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::midgard::{Aabb, PointLL};

/// `static_cast<int>(floor(x))` of a double, saturating where C++ is undefined.
#[inline]
#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline converts the floored double to int"
)]
fn floor_int(x: f64) -> i32 {
    x.floor() as i32
}

/// Walks the cells of a grid along segments (mainline's `GridTraversal`).
#[derive(Clone, Copy, Debug)]
struct Traversal {
    minx: f64,
    miny: f64,
    maxx: f64,
    maxy: f64,
    square_width: f64,
    square_height: f64,
    ncols: i32,
    nrows: i32,
}

impl Traversal {
    fn new(
        minx: f64,
        miny: f64,
        square_width: f64,
        square_height: f64,
        ncols: i32,
        nrows: i32,
    ) -> Self {
        Self {
            minx,
            miny,
            maxx: minx + square_width * f64::from(ncols),
            maxy: miny + square_height * f64::from(nrows),
            square_width,
            square_height,
            ncols,
            nrows,
        }
    }

    /// `SquareAtPoint`.
    #[inline]
    fn square_at_point(&self, pt: PointLL) -> (i32, i32) {
        let dx = pt.lng - self.minx;
        let dy = pt.lat - self.miny;
        (
            floor_int(dx / self.square_width),
            floor_int(dy / self.square_height),
        )
    }

    /// `IsValidSquare`.
    #[inline]
    const fn is_valid_square(&self, col: i32, row: i32) -> bool {
        0 <= col && col < self.ncols && 0 <= row && row < self.nrows
    }

    /// `IntersectsColumn`: the column where the ray meets the bottom of `row`.
    fn intersects_column(&self, origin: PointLL, cotangent: f64, row: i32) -> i32 {
        if cotangent.is_infinite() {
            return -1;
        }
        let intersect_y = self.miny + f64::from(row) * self.square_height;
        let intersect_x = (intersect_y - origin.lat) * cotangent + origin.lng;
        floor_int((intersect_x - self.minx) / self.square_width)
    }

    /// `IntersectsRow`: the row where the ray meets the left of `col`.
    fn intersects_row(&self, origin: PointLL, tangent: f64, col: i32) -> i32 {
        if tangent.is_infinite() {
            return -1;
        }
        let intersect_x = self.minx + f64::from(col) * self.square_width;
        let intersect_y = (intersect_x - origin.lng) * tangent + origin.lat;
        floor_int((intersect_y - self.miny) / self.square_height)
    }

    /// `StartSquare`: the first square of the segment inside the grid, or (-1, -1).
    fn start_square(
        &self,
        origin: PointLL,
        dest: PointLL,
        tangent: f64,
        cotangent: f64,
    ) -> (i32, i32) {
        let (col, row) = self.square_at_point(origin);
        if self.is_valid_square(col, row) {
            return (col, row);
        }
        if origin.lng < self.minx {
            let left_row = self.intersects_row(origin, tangent, 0);
            if self.minx <= dest.lng && self.is_valid_square(0, left_row) {
                return (0, left_row);
            }
        } else if origin.lng >= self.maxx {
            let right_row = self.intersects_row(origin, tangent, self.ncols - 1);
            if dest.lng < self.maxx && self.is_valid_square(self.ncols - 1, right_row) {
                return (self.ncols - 1, right_row);
            }
        }
        if origin.lat < self.miny {
            let bottom_col = self.intersects_column(origin, cotangent, 0);
            if self.miny <= dest.lat && self.is_valid_square(bottom_col, 0) {
                return (bottom_col, 0);
            }
        } else if origin.lat >= self.maxy {
            let top_col = self.intersects_column(origin, cotangent, self.nrows - 1);
            if dest.lat < self.maxy && self.is_valid_square(top_col, self.nrows - 1) {
                return (top_col, self.nrows - 1);
            }
        }
        (-1, -1)
    }

    /// `Traverse`: the squares the segment from `origin` to `dest` crosses, in order.
    fn traverse(&self, origin: PointLL, dest: PointLL, mut visit: impl FnMut(i32, i32)) {
        let height = dest.lat - origin.lat;
        let width = dest.lng - origin.lng;
        let tangent = if width == 0.0 {
            f64::INFINITY
        } else {
            height / width
        };
        let cotangent = if height == 0.0 {
            f64::INFINITY
        } else {
            width / height
        };
        let (mut col, mut row) = self.start_square(origin, dest, tangent, cotangent);
        let (dest_col, dest_row) = self.square_at_point(dest);
        while !(col == dest_col && row == dest_row) && self.is_valid_square(col, row) {
            visit(col, row);
            if col < dest_col && self.intersects_row(origin, tangent, col + 1) == row {
                col += 1;
                continue;
            }
            if dest_col < col && self.intersects_row(origin, tangent, col) == row {
                col -= 1;
                continue;
            }
            if row < dest_row && self.intersects_column(origin, cotangent, row + 1) == col {
                row += 1;
                continue;
            }
            if dest_row < row && self.intersects_column(origin, cotangent, row) == col {
                row -= 1;
                continue;
            }
            // Diagonally, when none of the four neighbors is crossed.
            if col < dest_col {
                col += 1;
            } else if dest_col < col {
                col -= 1;
            }
            if row < dest_row {
                row += 1;
            } else if dest_row < row {
                row -= 1;
            }
        }
        if self.is_valid_square(col, row) {
            visit(col, row);
        }
    }
}

/// Edges by grid cell (mainline's `GridRangeQuery<GraphId, PointLL>`, with sparse cells).
#[derive(Clone, Debug)]
pub(crate) struct Grid {
    traversal: Traversal,
    cells: HashMap<u32, Vec<GraphId>>,
}

impl Grid {
    /// An empty grid over `bbox` with cells of the given size (in degrees).
    pub(crate) fn new(bbox: &Aabb, square_width: f32, square_height: f32) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline converts the ceiled double to int"
        )]
        let ncols = ((bbox.maxx - bbox.minx) / f64::from(square_width)).ceil() as i32;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline converts the ceiled double to int"
        )]
        let nrows = ((bbox.maxy - bbox.miny) / f64::from(square_height)).ceil() as i32;
        Self {
            traversal: Traversal::new(
                bbox.minx,
                bbox.miny,
                f64::from(square_width),
                f64::from(square_height),
                ncols,
                nrows,
            ),
            cells: HashMap::new(),
        }
    }

    #[inline]
    fn key(&self, col: i32, row: i32) -> u32 {
        // Only valid squares are visited, and the config limits the grid's size,
        // so this is in range.
        u32::try_from(i64::from(col) + i64::from(row) * i64::from(self.traversal.ncols))
            .unwrap_or(u32::MAX)
    }

    /// Adds an edge's segment to every cell it crosses (`AddLineSegment`).
    pub(crate) fn add_segment(&mut self, item: GraphId, u: PointLL, v: PointLL) {
        let traversal = self.traversal;
        traversal.traverse(u, v, |col, row| {
            let key = self.key(col, row);
            self.cells.entry(key).or_default().push(item);
        });
    }

    /// Appends the edges in the cells `range` touches (`Query`; the caller removes duplicates).
    pub(crate) fn query(&self, range: &Aabb, out: &mut Vec<GraphId>) {
        let t = &self.traversal;
        let (mincol, minrow) = t.square_at_point(PointLL::new(range.minx, range.miny));
        let (maxcol, maxrow) = t.square_at_point(PointLL::new(range.maxx, range.maxy));
        // `std::max(0, std::min(x, n - 1))`.
        let clamp = |x: i32, n: i32| 0.max(x.min(n - 1));
        let (mincol, maxcol) = (clamp(mincol, t.ncols), clamp(maxcol, t.ncols));
        let (minrow, maxrow) = (clamp(minrow, t.nrows), clamp(maxrow, t.nrows));
        for row in minrow..=maxrow {
            for col in mincol..=maxcol {
                if let Some(items) = self.cells.get(&self.key(col, row)) {
                    out.extend_from_slice(items);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn squares(t: &Traversal, a: (f64, f64), b: (f64, f64)) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        t.traverse(PointLL::new(a.0, a.1), PointLL::new(b.0, b.1), |c, r| {
            out.push((c, r));
        });
        out
    }

    #[test]
    fn traverses_like_mainline() {
        let t = Traversal::new(0.0, 0.0, 1.0, 1.0, 10, 10);
        // Horizontal and vertical.
        assert_eq!(
            squares(&t, (0.5, 0.5), (3.5, 0.5)),
            [(0, 0), (1, 0), (2, 0), (3, 0)]
        );
        assert_eq!(
            squares(&t, (0.5, 2.5), (0.5, 0.5)),
            [(0, 2), (0, 1), (0, 0)]
        );
        // A diagonal through corners moves diagonally.
        assert_eq!(
            squares(&t, (0.5, 0.5), (2.5, 2.5)),
            [(0, 0), (1, 1), (2, 2)]
        );
        // From outside the grid, starting at its edge.
        assert_eq!(squares(&t, (-1.5, 0.5), (1.5, 0.5)), [(0, 0), (1, 0)]);
        // Entirely outside.
        assert!(squares(&t, (-5.0, -5.0), (-4.0, -4.0)).is_empty());
        // A point segment.
        assert_eq!(squares(&t, (3.2, 4.7), (3.2, 4.7)), [(3, 4)]);
    }

    #[test]
    fn queries_clamp_to_the_grid() {
        let bbox = Aabb {
            minx: 0.0,
            miny: 0.0,
            maxx: 1.0,
            maxy: 1.0,
        };
        let mut grid = Grid::new(&bbox, 0.1, 0.1);
        let a = GraphId::try_from_components(2, 1, 0).unwrap();
        let b = GraphId::try_from_components(2, 1, 1).unwrap();
        grid.add_segment(a, PointLL::new(0.05, 0.05), PointLL::new(0.95, 0.05));
        grid.add_segment(b, PointLL::new(0.55, 0.55), PointLL::new(0.56, 0.56));
        let mut out = Vec::new();
        grid.query(
            &Aabb {
                minx: -1.0,
                miny: -1.0,
                maxx: 0.01,
                maxy: 0.01,
            },
            &mut out,
        );
        assert_eq!(out, [a]);
        out.clear();
        grid.query(
            &Aabb {
                minx: 0.5,
                miny: 0.0,
                maxx: 2.0,
                maxy: 2.0,
            },
            &mut out,
        );
        out.sort_by_key(GraphId::value);
        out.dedup();
        assert_eq!(out, [a, b]);
    }
}
