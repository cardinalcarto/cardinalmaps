//! SpatiaLite geometry blobs, read without mod_spatialite.
//!
//! A blob is `0x00`, the byte order (`0x01` little endian, `0x00` big), the
//! SRID (i32), the minimum x and y and maximum x and y (f64), `0x7C`, the
//! class (i32), the geometry, and `0xFE`. Classes are the OGC ones, 1 to 7,
//! plus 1000 for XYZ, 2000 for XYM and 3000 for XYZM, plus 1,000,000 for
//! SpatiaLite's compressed lines and rings, whose vertices between the first
//! and the last are f32 offsets from the one before. Each item of a
//! collection is `0x69`, its class, and its geometry. Only x and y are kept.

use geo::{Coord, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BlobError {
    #[error("not a SpatiaLite geometry blob")]
    NotSpatiaLite,
    #[error("the blob ends before its geometry does")]
    Truncated,
    #[error("unsupported geometry class {0}")]
    UnsupportedClass(i32),
}

const HEADER: usize = 39;
const ENTITY: u8 = 0x69;
const END: u8 = 0xFE;

/// The bounds a blob's header records: `[min x, min y, max x, max y]`.
pub fn bounds(blob: &[u8]) -> Result<[f64; 4], BlobError> {
    let reader = Reader::new(blob)?;
    Ok([reader.f64_at(6)?, reader.f64_at(14)?, reader.f64_at(22)?, reader.f64_at(30)?])
}

/// The geometry in a blob.
pub fn parse(blob: &[u8]) -> Result<Geometry<f64>, BlobError> {
    let mut reader = Reader::new(blob)?;
    reader.offset = HEADER;
    let class = reader.i32()?;
    let geometry = reader.geometry(class)?;
    if reader.offset != blob.len() - 1 {
        return Err(BlobError::NotSpatiaLite);
    }
    Ok(geometry)
}

struct Reader<'a> {
    blob: &'a [u8],
    little: bool,
    offset: usize,
}

/// A class's coordinate count per vertex and whether it's compressed.
#[derive(Clone, Copy)]
struct Layout {
    dims: usize,
    compressed: bool,
}

impl<'a> Reader<'a> {
    fn new(blob: &'a [u8]) -> Result<Self, BlobError> {
        if blob.len() < HEADER + 5 || blob[0] != 0 || blob[HEADER - 1] != 0x7C || blob[blob.len() - 1] != END {
            return Err(BlobError::NotSpatiaLite);
        }
        let little = match blob[1] {
            1 => true,
            0 => false,
            _ => return Err(BlobError::NotSpatiaLite),
        };
        Ok(Self { blob, little, offset: 0 })
    }

    fn bytes<const N: usize>(&self, at: usize) -> Result<[u8; N], BlobError> {
        // The last byte is the end marker, never part of a value.
        let end = at.checked_add(N).filter(|&end| end < self.blob.len()).ok_or(BlobError::Truncated)?;
        Ok(self.blob[at..end].try_into().expect("N bytes"))
    }

    fn f64_at(&self, at: usize) -> Result<f64, BlobError> {
        let bytes = self.bytes::<8>(at)?;
        Ok(if self.little { f64::from_le_bytes(bytes) } else { f64::from_be_bytes(bytes) })
    }

    fn f64(&mut self) -> Result<f64, BlobError> {
        let value = self.f64_at(self.offset)?;
        self.offset += 8;
        Ok(value)
    }

    fn f32(&mut self) -> Result<f32, BlobError> {
        let bytes = self.bytes::<4>(self.offset)?;
        self.offset += 4;
        Ok(if self.little { f32::from_le_bytes(bytes) } else { f32::from_be_bytes(bytes) })
    }

    fn i32(&mut self) -> Result<i32, BlobError> {
        let bytes = self.bytes::<4>(self.offset)?;
        self.offset += 4;
        Ok(if self.little { i32::from_le_bytes(bytes) } else { i32::from_be_bytes(bytes) })
    }

    fn count(&mut self) -> Result<usize, BlobError> {
        let count = usize::try_from(self.i32()?).map_err(|_| BlobError::NotSpatiaLite)?;
        // Each item takes at least 4 bytes, so a larger count is corrupt, and
        // allocating for it could exhaust memory.
        if count > (self.blob.len() - self.offset) / 4 {
            return Err(BlobError::Truncated);
        }
        Ok(count)
    }

    fn layout(class: i32) -> Result<(i32, Layout), BlobError> {
        let compressed = class >= 1_000_000;
        let rest = class % 1_000_000;
        let dims = match rest / 1000 {
            0 => 2,
            1 | 2 => 3,
            3 => 4,
            _ => return Err(BlobError::UnsupportedClass(class)),
        };
        let base = rest % 1000;
        if !(1..=7).contains(&base) || (compressed && base == 1) {
            return Err(BlobError::UnsupportedClass(class));
        }
        Ok((base, Layout { dims, compressed }))
    }

    fn geometry(&mut self, class: i32) -> Result<Geometry<f64>, BlobError> {
        let (base, layout) = Self::layout(class)?;
        Ok(match base {
            1 => Geometry::Point(self.point(layout)?),
            2 => Geometry::LineString(self.line(layout)?),
            3 => Geometry::Polygon(self.polygon(layout)?),
            4 => Geometry::MultiPoint(MultiPoint(self.items(class, |r, c| match r.geometry(c)? {
                Geometry::Point(p) => Ok(p),
                _ => Err(BlobError::UnsupportedClass(c)),
            })?)),
            5 => Geometry::MultiLineString(MultiLineString(self.items(class, |r, c| match r.geometry(c)? {
                Geometry::LineString(l) => Ok(l),
                _ => Err(BlobError::UnsupportedClass(c)),
            })?)),
            6 => Geometry::MultiPolygon(MultiPolygon(self.items(class, |r, c| match r.geometry(c)? {
                Geometry::Polygon(p) => Ok(p),
                _ => Err(BlobError::UnsupportedClass(c)),
            })?)),
            _ => Geometry::GeometryCollection(GeometryCollection(self.items(class, Self::geometry)?)),
        })
    }

    /// A collection's items, each with its own class. Items of a typed
    /// collection must have its dimensions; GeometryCollections may mix.
    fn items<T>(
        &mut self,
        class: i32,
        mut item: impl FnMut(&mut Self, i32) -> Result<T, BlobError>,
    ) -> Result<Vec<T>, BlobError> {
        let count = self.count()?;
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            if self.bytes::<1>(self.offset)?[0] != ENTITY {
                return Err(BlobError::NotSpatiaLite);
            }
            self.offset += 1;
            let item_class = self.i32()?;
            if class % 1000 != 7 && (item_class % 1_000_000) / 1000 != (class % 1_000_000) / 1000 {
                return Err(BlobError::UnsupportedClass(item_class));
            }
            items.push(item(self, item_class)?);
        }
        Ok(items)
    }

    fn vertex(&mut self, layout: Layout) -> Result<Coord<f64>, BlobError> {
        let x = self.f64()?;
        let y = self.f64()?;
        self.offset += (layout.dims - 2) * 8;
        Ok(Coord { x, y })
    }

    fn point(&mut self, layout: Layout) -> Result<Point<f64>, BlobError> {
        Ok(Point(self.vertex(layout)?))
    }

    fn line(&mut self, layout: Layout) -> Result<LineString<f64>, BlobError> {
        let count = self.count()?;
        let mut coords = Vec::with_capacity(count);
        for i in 0..count {
            if layout.compressed && i > 0 && i + 1 < count {
                let previous: Coord<f64> = coords[i - 1];
                let dx = f64::from(self.f32()?);
                let dy = f64::from(self.f32()?);
                self.offset += (layout.dims - 2) * 4;
                coords.push(Coord { x: previous.x + dx, y: previous.y + dy });
            } else {
                coords.push(self.vertex(layout)?);
            }
        }
        Ok(LineString(coords))
    }

    fn polygon(&mut self, layout: Layout) -> Result<Polygon<f64>, BlobError> {
        let rings = self.count()?;
        if rings == 0 {
            return Ok(Polygon::new(LineString(Vec::new()), Vec::new()));
        }
        let exterior = self.line(layout)?;
        let mut interiors = Vec::with_capacity(rings - 1);
        for _ in 1..rings {
            interiors.push(self.line(layout)?);
        }
        Ok(Polygon::new(exterior, interiors))
    }
}

/// A geometry as an uncompressed little-endian XY blob with SRID 4326, as
/// the Who's On First databases have them. For building test databases.
#[must_use]
pub fn encode(geometry: &Geometry<f64>) -> Vec<u8> {
    use geo::BoundingRect;
    let rect = geometry.bounding_rect();
    let mut blob = vec![0, 1];
    blob.extend(4326_i32.to_le_bytes());
    let (min, max) = rect.map_or(((0.0, 0.0), (0.0, 0.0)), |r| (r.min().x_y(), r.max().x_y()));
    for v in [min.0, min.1, max.0, max.1] {
        blob.extend(v.to_le_bytes());
    }
    blob.push(0x7C);
    write(&mut blob, geometry);
    blob.push(END);
    blob
}

fn write(blob: &mut Vec<u8>, geometry: &Geometry<f64>) {
    fn count(blob: &mut Vec<u8>, n: usize) {
        blob.extend(i32::try_from(n).expect("small enough").to_le_bytes());
    }
    fn line(blob: &mut Vec<u8>, line: &LineString<f64>) {
        count(blob, line.0.len());
        for c in &line.0 {
            blob.extend(c.x.to_le_bytes());
            blob.extend(c.y.to_le_bytes());
        }
    }
    fn polygon(blob: &mut Vec<u8>, p: &Polygon<f64>) {
        count(blob, 1 + p.interiors().len());
        line(blob, p.exterior());
        for ring in p.interiors() {
            line(blob, ring);
        }
    }
    fn items(blob: &mut Vec<u8>, items: impl ExactSizeIterator<Item = Geometry<f64>>) {
        count(blob, items.len());
        for item in items {
            blob.push(ENTITY);
            write(blob, &item);
        }
    }
    match geometry {
        Geometry::Point(p) => {
            blob.extend(1_i32.to_le_bytes());
            blob.extend(p.x().to_le_bytes());
            blob.extend(p.y().to_le_bytes());
        }
        Geometry::LineString(l) => {
            blob.extend(2_i32.to_le_bytes());
            line(blob, l);
        }
        Geometry::Polygon(p) => {
            blob.extend(3_i32.to_le_bytes());
            polygon(blob, p);
        }
        Geometry::MultiPoint(m) => {
            blob.extend(4_i32.to_le_bytes());
            items(blob, m.0.iter().map(|p| Geometry::Point(*p)));
        }
        Geometry::MultiLineString(m) => {
            blob.extend(5_i32.to_le_bytes());
            items(blob, m.0.iter().cloned().map(Geometry::LineString));
        }
        Geometry::MultiPolygon(m) => {
            blob.extend(6_i32.to_le_bytes());
            items(blob, m.0.iter().cloned().map(Geometry::Polygon));
        }
        Geometry::GeometryCollection(c) => {
            blob.extend(7_i32.to_le_bytes());
            items(blob, c.0.iter().cloned());
        }
        Geometry::Rect(r) => write(blob, &Geometry::Polygon(r.to_polygon())),
        Geometry::Triangle(t) => write(blob, &Geometry::Polygon(t.to_polygon())),
        Geometry::Line(l) => write(blob, &Geometry::LineString((*l).into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, point, polygon};

    #[test]
    fn round_trips_every_class() {
        let square = polygon![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 1.0, y: 1.0), (x: 0.0, y: 0.0)];
        let holed = Polygon::new(
            line_string![(x: 0.0, y: 0.0), (x: 4.0, y: 0.0), (x: 4.0, y: 4.0), (x: 0.0, y: 0.0)],
            vec![line_string![(x: 1.0, y: 1.0), (x: 2.0, y: 1.0), (x: 2.0, y: 2.0), (x: 1.0, y: 1.0)]],
        );
        let geometries = [
            Geometry::Point(point!(x: -122.3, y: 47.6)),
            Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 2.0)]),
            Geometry::Polygon(holed.clone()),
            Geometry::MultiPoint(MultiPoint(vec![point!(x: 1.0, y: 2.0), point!(x: 3.0, y: 4.0)])),
            Geometry::MultiPolygon(MultiPolygon(vec![square.clone(), holed])),
            Geometry::GeometryCollection(GeometryCollection(vec![
                Geometry::Point(point!(x: 1.0, y: 1.0)),
                Geometry::Polygon(square),
            ])),
        ];
        for geometry in geometries {
            assert_eq!(parse(&encode(&geometry)).unwrap(), geometry);
        }
    }

    #[test]
    fn reads_the_header_bounds() {
        let square = Geometry::Polygon(polygon![(x: -2.0, y: 1.0), (x: 3.0, y: 1.0), (x: 3.0, y: 5.0)]);
        assert_eq!(bounds(&encode(&square)).unwrap(), [-2.0, 1.0, 3.0, 5.0]);
    }

    /// A compressed XYZ line, by hand: full first and last vertices, f32
    /// offsets between.
    #[test]
    fn reads_compressed_lines_with_z() {
        let mut blob = vec![0, 1];
        blob.extend(4326_i32.to_le_bytes());
        for v in [0.0_f64, 0.0, 3.0, 3.0] {
            blob.extend(v.to_le_bytes());
        }
        blob.push(0x7C);
        blob.extend(1_001_002_i32.to_le_bytes());
        blob.extend(3_i32.to_le_bytes());
        for v in [1.0_f64, 1.0, 9.0] {
            blob.extend(v.to_le_bytes());
        }
        for v in [0.5_f32, 1.0, 0.0] {
            blob.extend(v.to_le_bytes());
        }
        for v in [3.0_f64, 3.0, 9.0] {
            blob.extend(v.to_le_bytes());
        }
        blob.push(END);
        let expected = Geometry::LineString(line_string![(x: 1.0, y: 1.0), (x: 1.5, y: 2.0), (x: 3.0, y: 3.0)]);
        assert_eq!(parse(&blob).unwrap(), expected);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse(b"hello"), Err(BlobError::NotSpatiaLite));
        let mut blob = encode(&Geometry::Point(point!(x: 1.0, y: 1.0)));
        blob.truncate(blob.len() - 9);
        blob.push(END);
        assert_eq!(parse(&blob), Err(BlobError::Truncated));
        let mut huge = encode(&Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]));
        huge[HEADER + 4..HEADER + 8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert_eq!(parse(&huge), Err(BlobError::Truncated));
    }
}
