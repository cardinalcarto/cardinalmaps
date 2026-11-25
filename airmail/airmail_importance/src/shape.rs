//! The shapes airmail stores for places, reduced to what its documents can
//! hold, and the measures taken from them.
//!
//! Coordinates are `x` = longitude, `y` = latitude, in WGS 84 degrees.

use geo::{
    BoundingRect, Centroid, ChamberlainDuquetteArea, Contains, CoordsIter, Geometry, InteriorPoint,
    MultiPolygon, Point, SimplifyVwPreserve,
};

/// Shapes are simplified until they have at most this many vertices.
pub const MAX_VERTICES: usize = 1000;
/// Reduces a geometry to the kinds results can have, Point, LineString,
/// MultiLineString, Polygon or MultiPolygon, or `None` if it has no usable
/// shape. Other
/// kinds are replaced by the point that stands for them.
#[must_use]
pub fn normalize(geometry: Geometry<f64>) -> Option<Geometry<f64>> {
    let geometry = match geometry {
        Geometry::Point(_) | Geometry::LineString(_) | Geometry::Polygon(_) => geometry,
        Geometry::MultiLineString(mut multi) => match multi.0.len() {
            0 => return None,
            1 => Geometry::LineString(multi.0.remove(0)),
            _ => Geometry::MultiLineString(multi),
        },
        Geometry::MultiPolygon(mut multi) => match multi.0.len() {
            0 => return None,
            1 => Geometry::Polygon(multi.0.remove(0)),
            _ => Geometry::MultiPolygon(multi),
        },
        Geometry::Rect(rect) => Geometry::Polygon(rect.to_polygon()),
        Geometry::Triangle(triangle) => Geometry::Polygon(triangle.to_polygon()),
        Geometry::Line(line) => Geometry::LineString(line.into()),
        other => Geometry::Point(representative_point(&other)?),
    };
    let finite = geometry
        .coords_iter()
        .all(|c| c.x.is_finite() && c.y.is_finite());
    (finite && geometry.coords_count() > 0).then_some(geometry)
}

/// Simplifies a geometry, keeping its topology, until it has at most
/// `max_vertices` vertices. The tolerance starts small relative to the
/// shape's size and doubles, so small shapes keep their detail.
#[must_use]
pub fn simplify(geometry: Geometry<f64>, max_vertices: usize) -> Geometry<f64> {
    if geometry.coords_count() <= max_vertices {
        return geometry;
    }
    let Some(bounds) = geometry.bounding_rect() else {
        return geometry;
    };
    // A polygon keeps at least four vertices, so a shape with more parts than
    // a quarter of the limit can't get under it by simplifying its rings, and
    // each try on a shape that large is slow.
    if let Geometry::MultiPolygon(multi) = &geometry
        && multi.0.len() * 4 > max_vertices
    {
        let Geometry::MultiPolygon(multi) = geometry else {
            unreachable!()
        };
        return largest_parts(multi, max_vertices);
    }
    let size = bounds.width().max(bounds.height());
    // Visvalingam-Whyatt tolerances are areas.
    let mut epsilon = (size * 1e-4).powi(2);
    // Each try starts from the last, so the later, coarser tries are cheap.
    let mut geometry = geometry;
    let mut fewest = geometry.coords_count();
    for _ in 0..64 {
        let simplified = match &geometry {
            Geometry::LineString(line) => Geometry::LineString(line.simplify_vw_preserve(epsilon)),
            Geometry::MultiLineString(multi) => {
                Geometry::MultiLineString(multi.simplify_vw_preserve(epsilon))
            }
            Geometry::Polygon(polygon) => Geometry::Polygon(polygon.simplify_vw_preserve(epsilon)),
            Geometry::MultiPolygon(multi) => {
                Geometry::MultiPolygon(drop_degenerate(multi.simplify_vw_preserve(epsilon)))
            }
            _ => return geometry,
        };
        let count = simplified.coords_count();
        if count <= max_vertices {
            return simplified;
        }
        // Keeping the topology can stop simplification short; a larger
        // tolerance won't get further.
        if count >= fewest {
            break;
        }
        fewest = count;
        geometry = simplified;
        epsilon *= 4.0;
    }
    // A shape with very many parts can't get under the limit by simplifying
    // its rings; keep its largest parts.
    match geometry {
        Geometry::MultiPolygon(multi) => largest_parts(multi, max_vertices),
        other => other,
    }
}

fn drop_degenerate(multi: MultiPolygon<f64>) -> MultiPolygon<f64> {
    MultiPolygon(
        multi
            .0
            .into_iter()
            .filter(|p| p.exterior().0.len() >= 4)
            .collect(),
    )
}

fn largest_parts(multi: MultiPolygon<f64>, max_vertices: usize) -> Geometry<f64> {
    let mut parts = multi.0;
    parts.sort_by(|a, b| {
        b.chamberlain_duquette_unsigned_area()
            .total_cmp(&a.chamberlain_duquette_unsigned_area())
    });
    let mut kept = Vec::new();
    let mut vertices = 0;
    for part in parts {
        let simplified = simplify(Geometry::Polygon(part), max_vertices);
        let count = simplified.coords_count();
        if vertices + count > max_vertices && !kept.is_empty() {
            break;
        }
        if let Geometry::Polygon(polygon) = simplified {
            vertices += count;
            kept.push(polygon);
        }
    }
    normalize(Geometry::MultiPolygon(MultiPolygon(kept)))
        .unwrap_or(Geometry::MultiPolygon(MultiPolygon(vec![])))
}

/// A point that stands for a geometry: inside it for areas, on it for lines.
/// The centroid if that qualifies, as it looks most natural.
#[must_use]
pub fn representative_point(geometry: &Geometry<f64>) -> Option<Point<f64>> {
    match geometry {
        Geometry::Point(point) => Some(*point),
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => {
            let centroid = geometry.centroid()?;
            if geometry.contains(&centroid) {
                Some(centroid)
            } else {
                geometry.interior_point()
            }
        }
        _ => geometry.interior_point(),
    }
}

/// The geodesic area of a geometry in square meters, zero for points and
/// lines.
#[must_use]
pub fn area_m2(geometry: &Geometry<f64>) -> f64 {
    match geometry {
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => {
            geometry.chamberlain_duquette_unsigned_area()
        }
        _ => 0.0,
    }
}

/// The area of a place's shape as airmail stores it (normalized, then
/// simplified to [`MAX_VERTICES`]), in square meters.
#[must_use]
pub fn stored_area_m2(geometry: Geometry<f64>) -> f64 {
    normalize(geometry).map_or(0.0, |geometry| area_m2(&simplify(geometry, MAX_VERTICES)))
}

#[cfg(test)]
mod test {
    use super::*;
    use geo::{Intersects, LineString, Polygon, coord, polygon, wkt};

    fn circle(cx: f64, cy: f64, r: f64, n: usize) -> Polygon<f64> {
        let ring: LineString<f64> = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                coord! { x: cx + r * a.cos(), y: cy + r * a.sin() }
            })
            .collect();
        Polygon::new(ring, vec![])
    }

    #[test]
    fn simplify_respects_the_vertex_limit() {
        let detailed = Geometry::Polygon(circle(8.5, 47.4, 0.5, 20_000));
        let simplified = simplify(detailed, MAX_VERTICES);
        assert!(simplified.coords_count() <= MAX_VERTICES);
        assert!(simplified.coords_count() > 50, "kept some detail");
    }

    #[test]
    fn simplify_keeps_small_shapes() {
        let small = Geometry::Polygon(circle(8.5, 47.4, 0.01, 100));
        assert_eq!(simplify(small.clone(), MAX_VERTICES), small);
    }

    #[test]
    fn representative_point_of_a_ring_shaped_area_is_inside_it() {
        // A "C" shape: its centroid is in the gap.
        let shape: Geometry<f64> = Geometry::Polygon(polygon![
            (x: 0., y: 0.), (x: 10., y: 0.), (x: 10., y: 2.), (x: 2., y: 2.),
            (x: 2., y: 8.), (x: 10., y: 8.), (x: 10., y: 10.), (x: 0., y: 10.),
        ]);
        let point = representative_point(&shape).unwrap();
        assert!(shape.contains(&point));
    }

    #[test]
    fn representative_point_of_a_line_is_on_it() {
        let line: Geometry<f64> = wkt! { LINESTRING(0. 0., 1. 1., 2. 0.) }.into();
        let point = representative_point(&line).unwrap();
        assert!(line.intersects(&point));
    }
}
