//! Which map tiles a region's polygon touches.
//!
//! A polygon is a ring of `(longitude, latitude)` points, in degrees, that doesn't cross the
//! antimeridian. A tile is in the region if its square intersects the polygon: some of it is
//! inside, or the polygon is inside it. Edges are straight in longitude and latitude, which
//! at a region's scale is within metres of a straight line on the map.

/// A ring of `(longitude, latitude)` points. The closing point may repeat the first or not.
#[derive(Clone, Debug)]
pub struct Polygon {
    points: Vec<(f64, f64)>,
    bounds: [f64; 4],
}

impl Polygon {
    /// The polygon through `points`, or `None` if it has fewer than three distinct points
    /// or one isn't a finite longitude and latitude.
    pub fn new(mut points: Vec<(f64, f64)>) -> Option<Self> {
        if points.len() > 1 && points.first() == points.last() {
            points.pop();
        }
        let valid = |&(lng, lat): &(f64, f64)| {
            lng.is_finite() && lat.is_finite() && lng.abs() <= 180.0 && lat.abs() <= 90.0
        };
        if points.len() < 3 || !points.iter().all(valid) {
            return None;
        }
        let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for &(lng, lat) in &points {
            bounds = [
                bounds[0].min(lng),
                bounds[1].min(lat),
                bounds[2].max(lng),
                bounds[3].max(lat),
            ];
        }
        Some(Self { points, bounds })
    }

    /// `[west, south, east, north]`.
    pub fn bounds(&self) -> [f64; 4] {
        self.bounds
    }

    /// Whether the point is inside (even-odd rule).
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        let mut inside = false;
        let n = self.points.len();
        for i in 0..n {
            let (xi, yi) = self.points[i];
            let (xj, yj) = self.points[(i + n - 1) % n];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
        }
        inside
    }

    /// Whether the rectangle `[west, south, east, north]` intersects the polygon.
    pub fn intersects(&self, rect: [f64; 4]) -> bool {
        let [w, s, e, n] = rect;
        let b = self.bounds;
        if b[0] > e || b[2] < w || b[1] > n || b[3] < s {
            return false;
        }
        // A vertex inside the rectangle, or the rectangle inside the polygon.
        if self
            .points
            .iter()
            .any(|&(x, y)| x >= w && x <= e && y >= s && y <= n)
            || self.contains(((w + e) / 2.0, (s + n) / 2.0))
        {
            return true;
        }
        // Otherwise they intersect only if an edge crosses the rectangle's boundary.
        let corners = [(w, s), (e, s), (e, n), (w, n)];
        let count = self.points.len();
        (0..count).any(|i| {
            let a = self.points[i];
            let b = self.points[(i + 1) % count];
            (0..4).any(|j| segments_cross(a, b, corners[j], corners[(j + 1) % 4]))
        })
    }
}

fn orientation(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

fn on_segment(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> bool {
    p.0 >= a.0.min(b.0) && p.0 <= a.0.max(b.0) && p.1 >= a.1.min(b.1) && p.1 <= a.1.max(b.1)
}

fn segments_cross(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    let (o1, o2) = (orientation(a, b, c), orientation(a, b, d));
    let (o3, o4) = (orientation(c, d, a), orientation(c, d, b));
    if ((o1 > 0.0 && o2 < 0.0) || (o1 < 0.0 && o2 > 0.0))
        && ((o3 > 0.0 && o4 < 0.0) || (o3 < 0.0 && o4 > 0.0))
    {
        return true;
    }
    (o1 == 0.0 && on_segment(a, b, c))
        || (o2 == 0.0 && on_segment(a, b, d))
        || (o3 == 0.0 && on_segment(c, d, a))
        || (o4 == 0.0 && on_segment(c, d, b))
}

/// A web mercator tile's `[west, south, east, north]`, in degrees.
pub fn tile_bounds(z: u8, x: u32, y: u32) -> [f64; 4] {
    let n = f64::from(1u32 << z);
    let lng = |x: f64| x / n * 360.0 - 180.0;
    let lat = |y: f64| {
        (std::f64::consts::PI * (1.0 - 2.0 * y / n))
            .sinh()
            .atan()
            .to_degrees()
    };
    let (x, y) = (f64::from(x), f64::from(y));
    [lng(x), lat(y + 1.0), lng(x + 1.0), lat(y)]
}

/// Every tile from `min_zoom` to `max_zoom` that intersects the polygon, as `(z, x, y)`,
/// by zoom and then in the order they are found.
///
/// A tile's children are only looked at if it intersects, so this is quick for large regions.
pub fn cover(polygon: &Polygon, min_zoom: u8, max_zoom: u8) -> Vec<(u8, u32, u32)> {
    let mut found = Vec::new();
    let mut level = vec![(0u32, 0u32)];
    for z in 0..=max_zoom {
        let mut next = Vec::with_capacity(level.len() * 4);
        for &(x, y) in &level {
            if !polygon.intersects(tile_bounds(z, x, y)) {
                continue;
            }
            if z >= min_zoom {
                found.push((z, x, y));
            }
            if z < max_zoom {
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    next.push((x * 2 + dx, y * 2 + dy));
                }
            }
        }
        level = next;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Polygon {
        Polygon::new(vec![
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
            (0.0, 0.0),
        ])
        .unwrap()
    }

    #[test]
    fn rectangles_intersect_a_polygon() {
        let p = square();
        assert!(p.intersects([0.2, 0.2, 0.3, 0.3])); // inside
        assert!(p.intersects([-1.0, -1.0, 2.0, 2.0])); // around
        assert!(p.intersects([0.5, -1.0, 0.6, 2.0])); // crossing, no vertex inside either
        assert!(!p.intersects([1.1, 0.0, 2.0, 1.0]));
        // A triangle's empty corner of its bounding box.
        let triangle = Polygon::new(vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]).unwrap();
        assert!(!triangle.intersects([0.8, 0.8, 0.9, 0.9]));
        assert!(triangle.intersects([0.4, 0.4, 0.6, 0.6]));
    }

    #[test]
    fn rejects_degenerate_polygons() {
        assert!(Polygon::new(vec![(0.0, 0.0), (1.0, 1.0)]).is_none());
        assert!(Polygon::new(vec![(0.0, 0.0), (1.0, 1.0), (f64::NAN, 0.0)]).is_none());
    }

    #[test]
    fn covers_every_zoom() {
        // Around Westlake, Seattle: one tile per zoom up to where it spans a tile boundary.
        let p = Polygon::new(vec![
            (-122.34, 47.61),
            (-122.335, 47.61),
            (-122.335, 47.613),
            (-122.34, 47.613),
        ])
        .unwrap();
        let tiles = cover(&p, 0, 14);
        assert_eq!(tiles[0], (0, 0, 0));
        assert!(tiles.contains(&(14, 2624, 5721)));
        for z in 0..=10 {
            assert_eq!(tiles.iter().filter(|t| t.0 == z).count(), 1, "zoom {z}");
        }
        let at = tile_bounds(14, 2624, 5721);
        assert!(at[0] < -122.337 && at[2] > -122.337 && at[1] < 47.612 && at[3] > 47.612);
    }
}
