//! Simplifying places' shapes for the phone.
//!
//! The tolerance is relative: a fraction of the larger side of the shape's
//! bounding box. A fixed tolerance in degrees can't suit both a microhood a
//! few hundred metres across and a country, while a relative one keeps every
//! shape equally recognizable at the zoom where it fills the screen: at the
//! default of 1/1000, a place drawn 1000 pixels wide is off by at most a
//! pixel. For containment the error grows with the place, as it should:
//! about 25 m on Seattle's boundary, a few metres for its neighbourhoods,
//! under a kilometre for Washington state. airmail itself keeps at most 1000
//! vertices per shape, simplifying by area until it gets there, which for
//! most cities and neighbourhoods lands at a similar or coarser tolerance,
//! so the phone gets what its index keeps without paying for more.

use geo::{Area, BoundingRect, Coord, CoordsIter, Geometry, LineString, MultiPolygon, Polygon, Simplify};

/// The default relative tolerance.
pub const DEFAULT_TOLERANCE: f64 = 1e-3;
/// The range a request may ask for: finer than this sends shapes nearly as
/// detailed as Who's On First's, coarser loses their shape.
pub const MIN_TOLERANCE: f64 = 1e-5;
pub const MAX_TOLERANCE: f64 = 0.05;

/// The default most vertices of a shape: airmail's own limit
/// (`airmail::geometry::MAX_VERTICES`), so the phone's index keeps what it
/// gets.
pub const DEFAULT_MAX_VERTICES: usize = 1000;
/// The range a request may ask for.
pub const MIN_MAX_VERTICES: usize = 16;
pub const MAX_MAX_VERTICES: usize = 100_000;

/// Coordinates are rounded to this many degrees, about 10 cm.
const PRECISION: f64 = 1e6;

/// The shortest tolerance in degrees, below the coordinates' precision.
const MIN_EPSILON: f64 = 1e-6;

/// How to simplify shapes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Simplification {
    /// The tolerance, relative to each shape's size.
    pub tolerance: f64,
    /// The most vertices of a shape.
    pub max_vertices: usize,
}

impl Default for Simplification {
    fn default() -> Self {
        Self { tolerance: DEFAULT_TOLERANCE, max_vertices: DEFAULT_MAX_VERTICES }
    }
}

/// A shape simplified with tolerance `relative`, with its coordinates
/// rounded, and with at most `max_vertices` vertices: until it has, the
/// tolerance doubles, and then its smallest parts are dropped. Parts and
/// holes that vanish at a tolerance are dropped; if every part would, the
/// shape is only rounded.
#[must_use]
pub fn simplify(geometry: &Geometry<f64>, simplification: Simplification) -> Geometry<f64> {
    let Simplification { tolerance: relative, max_vertices } = simplification;
    let Some(bounds) = geometry.bounding_rect() else {
        return geometry.clone();
    };
    let mut epsilon = (bounds.width().max(bounds.height()) * relative).max(MIN_EPSILON);
    let mut simplified = once(geometry, epsilon).unwrap_or_else(|| round_geometry(geometry));
    // Each try starts from the last, so the coarser ones are cheap.
    for _ in 0..32 {
        if simplified.coords_count() <= max_vertices {
            return simplified;
        }
        epsilon *= 2.0;
        match once(&simplified, epsilon) {
            Some(coarser) => simplified = coarser,
            None => break,
        }
    }
    match simplified {
        Geometry::MultiPolygon(multi) if multi.coords_count() > max_vertices => largest_parts(multi, max_vertices),
        other => other,
    }
}

/// The largest parts of a shape with at most `max_vertices` vertices in
/// all, and always at least its largest.
fn largest_parts(multi: MultiPolygon<f64>, max_vertices: usize) -> Geometry<f64> {
    let mut parts = multi.0;
    parts.sort_by(|a, b| b.unsigned_area().total_cmp(&a.unsigned_area()));
    let mut kept = Vec::new();
    let mut vertices = 0;
    for part in parts {
        let count = part.coords_count();
        if vertices + count > max_vertices && !kept.is_empty() {
            continue;
        }
        vertices += count;
        kept.push(part);
    }
    Geometry::MultiPolygon(MultiPolygon(kept))
}

/// The shape simplified with tolerance `epsilon` in degrees and rounded, or
/// `None` if nothing is left of it.
fn once(geometry: &Geometry<f64>, epsilon: f64) -> Option<Geometry<f64>> {
    match geometry {
        Geometry::Polygon(p) => polygon(p, epsilon).map(Geometry::Polygon),
        Geometry::MultiPolygon(m) => multi_polygon(m, epsilon),
        Geometry::LineString(l) => line(&l.simplify(epsilon), 2).map(Geometry::LineString),
        Geometry::MultiLineString(m) => {
            let lines: Vec<_> = m.simplify(epsilon).0.iter().filter_map(|l| line(l, 2)).collect();
            (!lines.is_empty()).then_some(Geometry::MultiLineString(geo::MultiLineString(lines)))
        }
        other => Some(round_geometry(other)),
    }
}

fn multi_polygon(multi: &MultiPolygon<f64>, epsilon: f64) -> Option<Geometry<f64>> {
    let parts: Vec<_> = multi.0.iter().filter_map(|p| polygon(p, epsilon)).collect();
    match parts.len() {
        0 => None,
        _ => Some(Geometry::MultiPolygon(MultiPolygon(parts))),
    }
}

/// A polygon simplified and rounded, without the holes smaller than
/// `epsilon`, or `None` if it's smaller than that itself or collapses.
/// Douglas-Peucker never takes a ring below three distinct vertices, so
/// without dropping small rings, a shape of many islands could only get under
/// a vertex limit by losing the shape of its largest ones.
fn polygon(polygon: &Polygon<f64>, epsilon: f64) -> Option<Polygon<f64>> {
    let ring = |ring: &LineString<f64>| {
        let size = ring.bounding_rect().map_or(0.0, |r| r.width().max(r.height()));
        if size < epsilon { None } else { line(&ring.simplify(epsilon), 4) }
    };
    let exterior = ring(polygon.exterior())?;
    let interiors = polygon.interiors().iter().filter_map(ring).collect();
    Some(Polygon::new(exterior, interiors))
}

/// A line rounded without repeated vertices, or `None` if fewer than `min`
/// are left.
fn line(line: &LineString<f64>, min: usize) -> Option<LineString<f64>> {
    let mut coords: Vec<Coord<f64>> = Vec::with_capacity(line.0.len());
    for c in &line.0 {
        let c = round(*c);
        if coords.last() != Some(&c) {
            coords.push(c);
        }
    }
    (coords.len() >= min).then_some(LineString(coords))
}

fn round(c: Coord<f64>) -> Coord<f64> {
    Coord { x: (c.x * PRECISION).round() / PRECISION, y: (c.y * PRECISION).round() / PRECISION }
}

fn round_geometry(geometry: &Geometry<f64>) -> Geometry<f64> {
    use geo::MapCoords;
    geometry.map_coords(round)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::polygon;

    /// A circle of `n` vertices with radius `r` around the origin.
    fn circle(n: usize, r: f64) -> Polygon<f64> {
        let mut coords: Vec<Coord<f64>> = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                Coord { x: r * a.cos(), y: r * a.sin() }
            })
            .collect();
        coords.push(coords[0]);
        Polygon::new(LineString(coords), vec![])
    }

    #[test]
    fn tolerance_scales_with_size() {
        // The same shape at any scale keeps the same vertices.
        let small = simplify(&Geometry::Polygon(circle(10_000, 0.01)), Simplification::default()).coords_count();
        let large = simplify(&Geometry::Polygon(circle(10_000, 10.0)), Simplification::default()).coords_count();
        assert!(small < 200, "{small}");
        assert!((small as i64 - large as i64).abs() <= 4, "{small} vs {large}");
    }

    #[test]
    fn drops_parts_that_vanish() {
        let big = circle(1000, 1.0);
        let speck = polygon![(x: 5.0, y: 5.0), (x: 5.000_000_1, y: 5.0), (x: 5.0, y: 5.000_000_1), (x: 5.0, y: 5.0)];
        let simplified = simplify(&Geometry::MultiPolygon(MultiPolygon(vec![big, speck])), Simplification::default());
        let Geometry::MultiPolygon(multi) = simplified else { panic!() };
        assert_eq!(multi.0.len(), 1);
    }

    #[test]
    fn keeps_a_shape_that_would_vanish_entirely() {
        let speck = polygon![(x: 5.0, y: 5.0), (x: 5.000_000_1, y: 5.0), (x: 5.0, y: 5.000_000_1), (x: 5.0, y: 5.0)];
        let simplified = simplify(&Geometry::Polygon(speck), Simplification::default());
        assert!(matches!(simplified, Geometry::Polygon(_)));
    }

    #[test]
    fn caps_vertices() {
        let fine = Geometry::Polygon(circle(100_000, 1.0));
        let capped = simplify(&fine, Simplification { tolerance: MIN_TOLERANCE, max_vertices: 100 });
        assert!(capped.coords_count() <= 100, "{}", capped.coords_count());
        assert!(capped.coords_count() > 20, "kept some detail");
        // Many parts: the largest stay.
        let mut parts: Vec<_> = (0..200)
            .map(|i| {
                let mut p = circle(32, 0.01);
                use geo::Translate;
                p.translate_mut(f64::from(i), 0.0);
                p
            })
            .collect();
        parts.push(circle(32, 0.5));
        let capped = simplify(
            &Geometry::MultiPolygon(MultiPolygon(parts)),
            Simplification { tolerance: MIN_TOLERANCE, max_vertices: 100 },
        );
        let Geometry::MultiPolygon(multi) = capped else { panic!() };
        assert!(multi.coords_count() <= 100);
        assert!(multi.0.iter().any(|p| p.unsigned_area() > 0.5), "the largest part is kept");
    }

    #[test]
    fn rounds_coordinates() {
        let p = polygon![(x: 0.123_456_789, y: 0.0), (x: 1.0, y: 0.0), (x: 1.0, y: 1.0), (x: 0.123_456_789, y: 0.0)];
        let Geometry::Polygon(p) = simplify(&Geometry::Polygon(p), Simplification::default()) else { panic!() };
        assert_eq!(p.exterior().0[0].x, 0.123_457);
    }
}
