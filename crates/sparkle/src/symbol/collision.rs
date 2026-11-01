//! The screen space labels have claimed so far in a placement: boxes and circles in a
//! uniform grid, as MapLibre's `GridIndex`.

use crate::style::Overlap;

/// Logical pixels per grid cell.
const CELL_SIZE: f32 = 25.0;
/// How far past the viewport's edges labels are still tested, in logical pixels, so labels
/// just outside it are placed (and fade in smoothly when panned into view).
pub const VIEWPORT_PADDING: f32 = 100.0;

/// A shape in screen space, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// `[left, top, right, bottom]`.
    Box([f32; 4]),
    /// `[x, y, radius]`.
    Circle([f32; 3]),
}

impl Shape {
    fn bounds(&self) -> [f32; 4] {
        match *self {
            Shape::Box(b) => b,
            Shape::Circle([x, y, r]) => [x - r, y - r, x + r, y + r],
        }
    }

    fn intersects(&self, other: &Shape) -> bool {
        match (*self, *other) {
            (Shape::Box(a), Shape::Box(b)) => {
                a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
            }
            (Shape::Circle([x, y, r]), Shape::Box(b))
            | (Shape::Box(b), Shape::Circle([x, y, r])) => {
                let dx = x - x.clamp(b[0], b[2]);
                let dy = y - y.clamp(b[1], b[3]);
                dx * dx + dy * dy < r * r
            }
            (Shape::Circle([x1, y1, r1]), Shape::Circle([x2, y2, r2])) => {
                let (dx, dy, r) = (x2 - x1, y2 - y1, r1 + r2);
                dx * dx + dy * dy < r * r
            }
        }
    }
}

struct Entry {
    shape: Shape,
    overlap: Overlap,
}

/// Shapes placed so far, bucketed into cells covering the viewport and
/// [`VIEWPORT_PADDING`] around it.
pub struct CollisionGrid {
    origin: [f32; 2],
    columns: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
    entries: Vec<Entry>,
    /// Per entry, the query that last looked at it, so a query tests each entry once.
    stamps: Vec<u32>,
    stamp: u32,
}

impl CollisionGrid {
    /// A grid for a viewport of `size` logical pixels.
    pub fn new(size: [f32; 2]) -> Self {
        let mut grid = Self {
            origin: [0.0; 2],
            columns: 0,
            rows: 0,
            cells: Vec::new(),
            entries: Vec::new(),
            stamps: Vec::new(),
            stamp: 0,
        };
        grid.reset(size);
        grid
    }

    /// Empty the grid, for a viewport of `size` logical pixels.
    pub fn reset(&mut self, size: [f32; 2]) {
        self.origin = [-VIEWPORT_PADDING; 2];
        let span = size.map(|s| s.max(1.0) + 2.0 * VIEWPORT_PADDING);
        self.columns = (span[0] / CELL_SIZE).ceil() as usize;
        self.rows = (span[1] / CELL_SIZE).ceil() as usize;
        self.cells.iter_mut().for_each(Vec::clear);
        self.cells.resize_with(self.columns * self.rows, Vec::new);
        self.entries.clear();
        self.stamps.clear();
        self.stamp = 0;
    }

    /// Whether `shape` is at least partly inside the grid's area. Shapes outside it are
    /// never placed.
    pub fn is_inside(&self, shape: &Shape) -> bool {
        let [l, t, r, b] = shape.bounds();
        let right = self.origin[0] + self.columns as f32 * CELL_SIZE;
        let bottom = self.origin[1] + self.rows as f32 * CELL_SIZE;
        r >= self.origin[0] && l < right && b >= self.origin[1] && t < bottom
    }

    fn cell_range(&self, shape: &Shape) -> ([usize; 2], [usize; 2]) {
        let [l, t, r, b] = shape.bounds();
        let cell = |v: f32, origin: f32, n: usize| {
            (((v - origin) / CELL_SIZE).floor().max(0.0) as usize).min(n - 1)
        };
        (
            [
                cell(l, self.origin[0], self.columns),
                cell(r, self.origin[0], self.columns),
            ],
            [
                cell(t, self.origin[1], self.rows),
                cell(b, self.origin[1], self.rows),
            ],
        )
    }

    /// Whether `shape` may be placed with `overlap`: always for [`Overlap::Always`];
    /// otherwise not where it would overlap a shape placed before, except that
    /// [`Overlap::Cooperative`] shapes may overlap each other.
    pub fn fits(&mut self, shape: &Shape, overlap: Overlap) -> bool {
        if overlap == Overlap::Always {
            return true;
        }
        if !self.is_inside(shape) {
            return false;
        }
        self.stamp = self.stamp.wrapping_add(1);
        let ([x0, x1], [y0, y1]) = self.cell_range(shape);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for &e in &self.cells[y * self.columns + x] {
                    let e = e as usize;
                    if self.stamps[e] == self.stamp {
                        continue;
                    }
                    self.stamps[e] = self.stamp;
                    let entry = &self.entries[e];
                    let blocks = overlap == Overlap::Never || entry.overlap == Overlap::Never;
                    if blocks && entry.shape.intersects(shape) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Claim the space of `shape`, placed with `overlap`.
    pub fn insert(&mut self, shape: Shape, overlap: Overlap) {
        if !self.is_inside(&shape) {
            return;
        }
        let index = self.entries.len() as u32;
        let ([x0, x1], [y0, y1]) = self.cell_range(&shape);
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.cells[y * self.columns + x].push(index);
            }
        }
        self.entries.push(Entry { shape, overlap });
        self.stamps.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_and_circles_collide() {
        let mut grid = CollisionGrid::new([400.0, 300.0]);
        let a = Shape::Box([10.0, 10.0, 60.0, 30.0]);
        assert!(grid.fits(&a, Overlap::Never));
        grid.insert(a, Overlap::Never);
        assert!(!grid.fits(&Shape::Box([50.0, 25.0, 90.0, 40.0]), Overlap::Never));
        assert!(grid.fits(&Shape::Box([61.0, 10.0, 90.0, 40.0]), Overlap::Never));
        assert!(!grid.fits(&Shape::Circle([70.0, 20.0, 11.0]), Overlap::Never));
        assert!(grid.fits(&Shape::Circle([70.0, 40.0, 11.0]), Overlap::Never));
        assert!(grid.fits(&Shape::Box([50.0, 25.0, 90.0, 40.0]), Overlap::Always));
        // Never placed shapes block cooperative ones too.
        assert!(!grid.fits(&Shape::Box([50.0, 25.0, 90.0, 40.0]), Overlap::Cooperative));
    }

    #[test]
    fn cooperative_shapes_overlap_each_other() {
        let mut grid = CollisionGrid::new([400.0, 300.0]);
        grid.insert(Shape::Box([10.0, 10.0, 60.0, 30.0]), Overlap::Cooperative);
        assert!(grid.fits(&Shape::Box([20.0, 20.0, 70.0, 40.0]), Overlap::Cooperative));
        assert!(!grid.fits(&Shape::Box([20.0, 20.0, 70.0, 40.0]), Overlap::Never));
    }

    #[test]
    fn far_outside_the_viewport_never_fits() {
        let mut grid = CollisionGrid::new([400.0, 300.0]);
        assert!(!grid.fits(&Shape::Box([600.0, 0.0, 650.0, 20.0]), Overlap::Never));
        assert!(grid.fits(&Shape::Box([450.0, 0.0, 480.0, 20.0]), Overlap::Never));
        // Large shapes reaching into the grid from outside are tested against it.
        grid.insert(Shape::Box([-500.0, -500.0, 20.0, 20.0]), Overlap::Never);
        assert!(!grid.fits(&Shape::Circle([10.0, 10.0, 2.0]), Overlap::Never));
    }
}
