//! Shapes of indexed places: simplifying them for storage, the points that
//! stand for them, the S2 cells that cover them, and bounding box tests.
//!
//! Coordinates are `x` = longitude, `y` = latitude, in WGS 84 degrees.

use anyhow::{Result, bail};
use geo::{BoundingRect, Geometry, Intersects, LineString, Polygon, Rect, coord};
use geozero::{CoordDimensions, ToGeo, ToWkb, wkb::Wkb};
use s2::{cell::Cell, cellid::CellID, latlng::LatLng, region::RegionCoverer};

// Shared with poi_service, which computes POI importance from the same shapes.
pub use airmail_importance::shape::{MAX_VERTICES, area_m2, normalize, representative_point, simplify};

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
    use geo::{CoordsIter, Point, wkt};

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
