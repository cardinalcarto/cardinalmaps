//! Geometries as GeoJSON geometry objects, written straight from `geo`
//! types without building a JSON tree.

use geo::{Coord, Geometry, LineString, Polygon};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

/// A geometry that serializes as a GeoJSON geometry object.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoJson(pub Geometry<f64>);

struct Position(Coord<f64>);
struct Positions<'a>(&'a LineString<f64>);
struct Rings<'a>(&'a Polygon<f64>);
struct Seq<'a, T>(&'a [T]);

impl Serialize for Position {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        [self.0.x, self.0.y].serialize(s)
    }
}

impl Serialize for Positions<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.0.len()))?;
        for c in &self.0.0 {
            seq.serialize_element(&Position(*c))?;
        }
        seq.end()
    }
}

impl Serialize for Rings<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(1 + self.0.interiors().len()))?;
        seq.serialize_element(&Positions(self.0.exterior()))?;
        for ring in self.0.interiors() {
            seq.serialize_element(&Positions(ring))?;
        }
        seq.end()
    }
}

impl Serialize for Seq<'_, LineString<f64>> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for line in self.0 {
            seq.serialize_element(&Positions(line))?;
        }
        seq.end()
    }
}

impl Serialize for Seq<'_, Polygon<f64>> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for polygon in self.0 {
            seq.serialize_element(&Rings(polygon))?;
        }
        seq.end()
    }
}

impl Serialize for Seq<'_, Geometry<f64>> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for geometry in self.0 {
            seq.serialize_element(&GeoJson(geometry.clone()))?;
        }
        seq.end()
    }
}

impl Serialize for GeoJson {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Geometry::Rect(r) => return GeoJson(Geometry::Polygon(r.to_polygon())).serialize(s),
            Geometry::Triangle(t) => return GeoJson(Geometry::Polygon(t.to_polygon())).serialize(s),
            Geometry::Line(l) => return GeoJson(Geometry::LineString((*l).into())).serialize(s),
            _ => {}
        }
        let mut map = s.serialize_map(Some(2))?;
        match &self.0 {
            Geometry::Point(p) => {
                map.serialize_entry("type", "Point")?;
                map.serialize_entry("coordinates", &Position(p.0))?;
            }
            Geometry::MultiPoint(m) => {
                map.serialize_entry("type", "MultiPoint")?;
                let points: Vec<[f64; 2]> = m.0.iter().map(|p| [p.x(), p.y()]).collect();
                map.serialize_entry("coordinates", &points)?;
            }
            Geometry::LineString(l) => {
                map.serialize_entry("type", "LineString")?;
                map.serialize_entry("coordinates", &Positions(l))?;
            }
            Geometry::MultiLineString(m) => {
                map.serialize_entry("type", "MultiLineString")?;
                map.serialize_entry("coordinates", &Seq(&m.0))?;
            }
            Geometry::Polygon(p) => {
                map.serialize_entry("type", "Polygon")?;
                map.serialize_entry("coordinates", &Rings(p))?;
            }
            Geometry::MultiPolygon(m) => {
                map.serialize_entry("type", "MultiPolygon")?;
                map.serialize_entry("coordinates", &Seq(&m.0))?;
            }
            Geometry::GeometryCollection(c) => {
                map.serialize_entry("type", "GeometryCollection")?;
                map.serialize_entry("geometries", &Seq(&c.0))?;
            }
            Geometry::Rect(_) | Geometry::Triangle(_) | Geometry::Line(_) => unreachable!("converted above"),
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::polygon;

    #[test]
    fn writes_polygons() {
        let p = GeoJson(Geometry::Polygon(polygon![(x: 0.0, y: 0.0), (x: 1.5, y: 0.0), (x: 1.0, y: 1.0)]));
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"type":"Polygon","coordinates":[[[0.0,0.0],[1.5,0.0],[1.0,1.0],[0.0,0.0]]]}"#
        );
    }
}
