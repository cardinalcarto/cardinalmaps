//! Shapes of indexed places: simplifying them for storage, the points that
//! stand for them, the S2 cells that cover them, and bounding box tests.
//!
//! Coordinates are `x` = longitude, `y` = latitude, in WGS 84 degrees.

use anyhow::{Result, bail};
use geo::{
    BoundingRect, Centroid, ChamberlainDuquetteArea, Contains, CoordsIter, Geometry, InteriorPoint,
    Intersects, LineString, MultiPolygon, Point, Polygon, Rect, SimplifyVwPreserve, coord,
};
use geozero::{CoordDimensions, ToGeo, ToWkb, wkb::Wkb};
use s2::{cell::Cell, cellid::CellID, latlng::LatLng, region::RegionCoverer};

/// Shapes are simplified until they have at most this many vertices.
pub const MAX_VERTICES: usize = 1000;

/// The most S2 cells in a shape's covering.
const MAX_COVER_CELLS: usize = 16;

/// The finest S2 level used in coverings, cells about 150 m across. Finer
/// cells would make the spatial terms several times larger without making
/// searches faster; searches check what they find against the box exactly
/// (see `within`), so a coarse covering costs nothing in precision.
pub const MAX_COVER_LEVEL: u64 = 16;

/// The coarsest S2 level used in coverings. Cells at lower levels span
/// continents, and their lat/lng bounds are too loose to refine against.
const MIN_COVER_LEVEL: u64 = 3;

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
    let finite = geometry.coords_iter().all(|c| c.x.is_finite() && c.y.is_finite());
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
        let Geometry::MultiPolygon(multi) = geometry else { unreachable!() };
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
            Geometry::MultiLineString(multi) => Geometry::MultiLineString(multi.simplify_vw_preserve(epsilon)),
            Geometry::Polygon(polygon) => Geometry::Polygon(polygon.simplify_vw_preserve(epsilon)),
            Geometry::MultiPolygon(multi) => Geometry::MultiPolygon(drop_degenerate(multi.simplify_vw_preserve(epsilon))),
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
    MultiPolygon(multi.0.into_iter().filter(|p| p.exterior().0.len() >= 4).collect())
}

fn largest_parts(multi: MultiPolygon<f64>, max_vertices: usize) -> Geometry<f64> {
    let mut parts = multi.0;
    parts.sort_by(|a, b| b.chamberlain_duquette_unsigned_area().total_cmp(&a.chamberlain_duquette_unsigned_area()));
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
    normalize(Geometry::MultiPolygon(MultiPolygon(kept))).unwrap_or(Geometry::MultiPolygon(MultiPolygon(vec![])))
}

/// A point that stands for a geometry: inside it for areas, on it for lines.
/// The centroid if that qualifies, as it looks most natural.
#[must_use]
pub fn representative_point(geometry: &Geometry<f64>) -> Option<Point<f64>> {
    match geometry {
        Geometry::Point(point) => Some(*point),
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => {
            let centroid = geometry.centroid()?;
            if geometry.contains(&centroid) { Some(centroid) } else { geometry.interior_point() }
        }
        _ => geometry.interior_point(),
    }
}

/// The geodesic area of a geometry in square meters, zero for points and
/// lines.
#[must_use]
pub fn area_m2(geometry: &Geometry<f64>) -> f64 {
    match geometry {
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => geometry.chamberlain_duquette_unsigned_area(),
        _ => 0.0,
    }
}

/// The S2 cells covering a geometry, at most `MAX_COVER_CELLS` of them, of
/// levels `MIN_COVER_LEVEL` to `MAX_COVER_LEVEL`. A point is covered by its
/// cell at `MAX_COVER_LEVEL`. Other shapes start from a covering of
/// their bounding box, and cells whose bounds don't touch the shape are
/// dropped while the rest are split while there's room, so the covering can
/// be larger than the shape but never misses part of it.
#[must_use]
pub fn covering(geometry: &Geometry<f64>) -> Vec<CellID> {
    if let Geometry::Point(point) = geometry {
        return vec![CellID::from(LatLng::from_degrees(point.y(), point.x())).parent(MAX_COVER_LEVEL)];
    }
    let Some(bounds) = geometry.bounding_rect() else {
        return vec![];
    };
    let region = s2::rect::Rect::from_degrees(bounds.min().y, bounds.min().x, bounds.max().y, bounds.max().x);
    let coverer = RegionCoverer { min_level: MIN_COVER_LEVEL as u8, max_level: MAX_COVER_LEVEL as u8, level_mod: 1, max_cells: 8 };
    let mut cells: Vec<CellID> = coverer
        .covering(&region)
        .0
        .into_iter()
        .filter(|cell| touches(geometry, *cell))
        .collect();
    // Split the largest cells first while the covering stays within budget.
    loop {
        cells.sort_by_key(CellID::level);
        let Some(index) = cells.iter().position(|cell| cell.level() < MAX_COVER_LEVEL) else {
            break;
        };
        let children: Vec<CellID> = cells[index].children().into_iter().filter(|c| touches(geometry, *c)).collect();
        if cells.len() - 1 + children.len() > MAX_COVER_CELLS {
            break;
        }
        cells.swap_remove(index);
        cells.extend(children);
        if cells.len() >= MAX_COVER_CELLS {
            break;
        }
    }
    cells.sort();
    cells
}

/// Whether a geometry might touch a cell, by the cell's lat/lng bounds, which
/// contain the cell.
fn touches(geometry: &Geometry<f64>, cell: CellID) -> bool {
    let bound = Cell::from(cell).rect_bound();
    let rect = Rect::new(
        coord! { x: bound.lo().lng.deg(), y: bound.lo().lat.deg() },
        coord! { x: bound.hi().lng.deg(), y: bound.hi().lat.deg() },
    );
    // Bounds of cells spanning the antimeridian wrap; don't drop those.
    bound.lo().lng.deg() > bound.hi().lng.deg() || geometry.intersects(&rect)
}

/// Every ancestor of `cell`, from level 0 up to its parent.
#[must_use]
pub fn ancestors(cell: CellID) -> Vec<CellID> {
    (0..cell.level()).map(|level| cell.parent(level)).collect()
}

/// A bounding box, `[min_lon, min_lat, max_lon, max_lat]`, as one or two
/// rects: two if it crosses the antimeridian (`min_lon > max_lon`).
#[must_use]
pub fn bbox_rects(bbox: [f64; 4]) -> Vec<Rect<f64>> {
    let [min_lon, min_lat, max_lon, max_lat] = bbox;
    let rect = |a: f64, b: f64| Rect::new(coord! { x: a, y: min_lat }, coord! { x: b, y: max_lat });
    if min_lon > max_lon { vec![rect(min_lon, 180.0), rect(-180.0, max_lon)] } else { vec![rect(min_lon, max_lon)] }
}

/// Whether a geometry intersects any of `rects`.
#[must_use]
pub fn intersects_any(geometry: &Geometry<f64>, rects: &[Rect<f64>]) -> bool {
    rects.iter().any(|rect| geometry.intersects(rect))
}

/// S2 cells covering the rects, for querying documents' coverings. No finer
/// than coverings are, since finer cells would match nothing more.
#[must_use]
pub fn query_cells(rects: &[Rect<f64>]) -> Vec<CellID> {
    let coverer = RegionCoverer { min_level: 0, max_level: MAX_COVER_LEVEL as u8, level_mod: 1, max_cells: 16 };
    let mut cells = Vec::new();
    for rect in rects {
        let region = s2::rect::Rect::from_degrees(rect.min().y, rect.min().x, rect.max().y, rect.max().x);
        let mut union = coverer.covering(&region);
        union.normalize();
        cells.extend(union.0);
    }
    cells
}

/// `[min_lon, min_lat, max_lon, max_lat]` of a geometry.
#[must_use]
pub fn bbox(geometry: &Geometry<f64>) -> Option<[f64; 4]> {
    let rect = geometry.bounding_rect()?;
    Some([rect.min().x, rect.min().y, rect.max().x, rect.max().y])
}

/// Encodes a geometry as WKB for storage.
///
/// # Errors
/// If the geometry can't be encoded.
pub fn to_wkb(geometry: &Geometry<f64>) -> Result<Vec<u8>> {
    Ok(geometry.to_wkb(CoordDimensions::xy())?)
}

/// Decodes a stored geometry.
///
/// # Errors
/// If `wkb` isn't a geometry.
pub fn from_wkb(wkb: &[u8]) -> Result<Geometry<f64>> {
    match Wkb(wkb.to_vec()).to_geo()? {
        geometry @ (Geometry::Point(_)
        | Geometry::LineString(_)
        | Geometry::MultiLineString(_)
        | Geometry::Polygon(_)
        | Geometry::MultiPolygon(_)) => {
            Ok(geometry)
        }
        other => bail!("unexpected stored geometry {other:?}"),
    }
}

/// A closed ring from coordinates, or `None` if it has too few.
#[must_use]
pub fn ring(coords: &[(f64, f64)]) -> Option<LineString<f64>> {
    let mut ring: LineString<f64> = coords.iter().map(|&(x, y)| coord! { x: x, y: y }).collect();
    ring.close();
    (ring.0.len() >= 4).then_some(ring)
}

/// A polygon without holes from a closed ring.
#[must_use]
pub fn polygon(exterior: LineString<f64>) -> Polygon<f64> {
    Polygon::new(exterior, vec![])
}

#[cfg(test)]
mod test {
    use super::*;
    use geo::{polygon, wkt};

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

    #[test]
    fn covering_of_a_point_is_its_cell_at_the_finest_level() {
        let point = Point::new(-122.3, 47.6);
        let cells = covering(&Geometry::Point(point));
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].level(), MAX_COVER_LEVEL);
        assert!(cells[0].contains(&CellID::from(LatLng::from_degrees(point.y(), point.x()))));
        assert_eq!(ancestors(cells[0]).len(), MAX_COVER_LEVEL as usize);
    }

    #[test]
    fn coverings_are_no_finer_than_the_cap() {
        // A building-sized square and a short street, both far smaller than a
        // level-16 cell.
        let shapes: [Geometry<f64>; 2] = [
            Geometry::Polygon(circle(-122.3, 47.6, 0.0001, 8)),
            wkt! { LINESTRING(-122.3000 47.6000, -122.3002 47.6001) }.into(),
        ];
        for shape in shapes {
            let cells = covering(&shape);
            assert!(!cells.is_empty());
            assert!(cells.iter().all(|c| (MIN_COVER_LEVEL..=MAX_COVER_LEVEL).contains(&c.level())), "{cells:?}");
        }
        let query = query_cells(&bbox_rects([-122.3001, 47.5999, -122.2999, 47.6001]));
        assert!(query.iter().all(|c| c.level() <= MAX_COVER_LEVEL));
    }

    /// Every point of a shape is in some cell of its covering, and the
    /// covering stays within budget.
    #[test]
    fn covering_contains_the_shape() {
        let shapes = [
            Geometry::Polygon(circle(-122.3, 47.65, 0.02, 64)),
            Geometry::Polygon(circle(8.2, 46.8, 2.0, 256)),
            wkt! { LINESTRING(-122.40 47.50, -122.20 47.70, -121.90 47.60) }.into(),
        ];
        for shape in shapes {
            let cells = covering(&shape);
            assert!(!cells.is_empty() && cells.len() <= MAX_COVER_CELLS, "{} cells", cells.len());
            for c in shape.coords_iter() {
                let leaf = CellID::from(LatLng::from_degrees(c.y, c.x));
                assert!(cells.iter().any(|cell| cell.contains(&leaf)), "{c:?} isn't covered");
            }
        }
    }

    #[test]
    fn bbox_across_the_antimeridian_is_two_rects() {
        let rects = bbox_rects([179.0, -1.0, -179.0, 1.0]);
        assert_eq!(rects.len(), 2);
        assert!(intersects_any(&Geometry::Point(Point::new(179.5, 0.0)), &rects));
        assert!(intersects_any(&Geometry::Point(Point::new(-179.5, 0.0)), &rects));
        assert!(!intersects_any(&Geometry::Point(Point::new(0.0, 0.0)), &rects));
    }

    #[test]
    fn wkb_round_trips() {
        let shape = Geometry::Polygon(circle(8.5, 47.4, 0.1, 32));
        assert_eq!(from_wkb(&to_wkb(&shape).unwrap()).unwrap(), shape);
    }
}
