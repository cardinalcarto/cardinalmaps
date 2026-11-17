//! Reading the features the offline index needs out of one z14 MLT tile.
//!
//! Each feature of the layers in [`SourceLayer`] becomes an [`ExtractedFeature`]: its raw OSM
//! name keys, `addr:*`, `cardinal:admin:*`, `cardinal:importance`, the rest of its attributes
//! (the category tags), its ID, and its geometry in longitude and latitude. The names that
//! OpenMapTiles derives (`name_en`, `name_de`, `name_int`, `name:latin`, `name:nonlatin`)
//! are left out: the tiles carry every raw name (see the plan's contract 1).
//!
//! A tile holds the features in a buffer around it too. A point there belongs to (and is
//! extracted from) the neighbouring tile, so it is dropped here. Lines and areas are clipped
//! to the tile's own edges likewise (the buffer's part is the neighbour's piece), so the
//! pieces that go to [`crate::merge::Merger`], which joins the pieces of one street (or area)
//! across tiles, meet at the edges instead of overlapping.

use std::io::Read;

use geo::{Coord, Geometry, MultiPoint, Point};
use mlt_core::geo_types::{self as mlt_geo};
use mlt_core::{Decoder, Layer, LendingIterator, Parser, PropValueRef};

/// The tile layers the offline index reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceLayer {
    Poi,
    Housenumber,
    TransportationName,
    /// Named features no styled layer carries (buildings, landuse, natural, ...): an
    /// unstyled z14 layer made for the geocoder.
    Geocoder,
    MountainPeak,
    WaterName,
    Park,
    AerodromeLabel,
}

impl SourceLayer {
    pub const ALL: [SourceLayer; 8] = [
        Self::Poi,
        Self::Housenumber,
        Self::TransportationName,
        Self::Geocoder,
        Self::MountainPeak,
        Self::WaterName,
        Self::Park,
        Self::AerodromeLabel,
    ];

    /// The layer of the tiles with this name, if the index reads it.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "poi" => Self::Poi,
            "housenumber" => Self::Housenumber,
            "transportation_name" => Self::TransportationName,
            "geocoder" => Self::Geocoder,
            "mountain_peak" => Self::MountainPeak,
            "water_name" => Self::WaterName,
            "park" => Self::Park,
            "aerodrome_label" => Self::AerodromeLabel,
            _ => return None,
        })
    }

    /// Its name in the tiles.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Poi => "poi",
            Self::Housenumber => "housenumber",
            Self::TransportationName => "transportation_name",
            Self::Geocoder => "geocoder",
            Self::MountainPeak => "mountain_peak",
            Self::WaterName => "water_name",
            Self::Park => "park",
            Self::AerodromeLabel => "aerodrome_label",
        }
    }
}

/// A tile, `y` from the top (XYZ).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileKey {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// One feature for the index, as the tiles have it: the intermediate form between the tiles
/// and airmail's document builder.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractedFeature {
    pub layer: SourceLayer,
    /// The feature's ID in the tile: in `poi`, its stable `poi_id` (POIs without one have
    /// none; 0 in the tiles means none). For merged pieces, the lowest of theirs.
    pub id: Option<u64>,
    /// The tile a point is in, or every tile a merged line's or area's pieces came from, in
    /// order. Never empty.
    pub tiles: Vec<TileKey>,
    /// Raw OSM name keys (`name`, `name:<lang>`, and `alt_name`, `official_name`,
    /// `short_name`, `loc_name`, `int_name`, each with an optional `:<lang>`) and their
    /// values, which may be `;`-separated lists. Sorted by key.
    pub names: Vec<(String, String)>,
    /// Every `addr:*` attribute, verbatim. Sorted by key.
    pub address: Vec<(String, String)>,
    /// The Who's On First area of each placetype the feature is in, from
    /// `cardinal:admin:<placetype>`: `(placetype, WOF ID)`. Sorted by placetype.
    pub admins: Vec<(String, i64)>,
    /// `cardinal:importance`, if the tile has it.
    pub importance: Option<f64>,
    /// Every other attribute (`class`, `subclass`, `amenity`, `cuisine`, `housenumber`, ...),
    /// numbers and booleans written out as text. Sorted by key.
    pub tags: Vec<(String, String)>,
    /// In longitude and latitude.
    pub geometry: Geometry<f64>,
}

impl ExtractedFeature {
    /// The value of the name key `key`.
    #[must_use]
    pub fn name(&self, key: &str) -> Option<&str> {
        lookup(&self.names, key)
    }

    /// The value of the tag `key`.
    #[must_use]
    pub fn tag(&self, key: &str) -> Option<&str> {
        lookup(&self.tags, key)
    }

    /// The value of the address key `key` (with its `addr:`).
    #[must_use]
    pub fn addr(&self, key: &str) -> Option<&str> {
        lookup(&self.address, key)
    }
}

fn lookup<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .binary_search_by(|(k, _)| k.as_str().cmp(key))
        .ok()
        .map(|i| pairs[i].1.as_str())
}

/// The name keys airmail indexes, each also with a `:<lang>` suffix.
const NAME_KEYS: [&str; 6] = [
    "name",
    "alt_name",
    "official_name",
    "short_name",
    "loc_name",
    "int_name",
];

/// Whether `key` is a raw OSM name key the index reads, as airmail's indexer decides it, less
/// the transliterations OpenMapTiles adds under `name:`.
#[must_use]
pub fn is_name_key(key: &str) -> bool {
    let mut parts = key.split(':');
    let base = parts.next().unwrap_or_default();
    NAME_KEYS.contains(&base)
        && key.split(':').count() <= 2
        && !key.contains("etymology")
        && !key.contains("pronunciation")
        && key != "name:latin"
        && key != "name:nonlatin"
}

/// Names OpenMapTiles derives, which the index ignores.
fn is_derived_name(key: &str) -> bool {
    matches!(
        key,
        "name_en" | "name_de" | "name_int" | "name:latin" | "name:nonlatin"
    )
}

const ADDRESS_PREFIX: &str = "addr:";
const ADMIN_PREFIX: &str = "cardinal:admin:";
const IMPORTANCE: &str = "cardinal:importance";

/// Why a tile couldn't be read.
#[derive(Debug, thiserror::Error)]
pub enum TileError {
    #[error("tile {}/{}/{} isn't a whole gzip stream: {source}", .tile.z, .tile.x, .tile.y)]
    Gzip {
        tile: TileKey,
        source: std::io::Error,
    },
    #[error("tile {}/{}/{} isn't MLT: {source}", .tile.z, .tile.x, .tile.y)]
    Mlt {
        tile: TileKey,
        source: mlt_core::MltError,
    },
}

/// What reading a tile found, layer by layer (indexed as [`SourceLayer::ALL`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TileCounts {
    /// Features in the layers read, buffer included.
    pub features: [u64; 8],
    /// Points kept (each point of a multipoint counts).
    pub points: [u64; 8],
    /// Points dropped because they're in the buffer, outside the tile.
    pub buffer_points: [u64; 8],
    /// Pieces of lines and areas, for the merger.
    pub pieces: [u64; 8],
}

impl TileCounts {
    pub fn add(&mut self, other: &TileCounts) {
        for i in 0..8 {
            self.features[i] += other.features[i];
            self.points[i] += other.points[i];
            self.buffer_points[i] += other.buffer_points[i];
            self.pieces[i] += other.pieces[i];
        }
    }
}

/// A tile's features: points, which are whole, and pieces of lines and areas cut at the
/// tile's edges (with the buffer), which [`crate::merge::Merger`] joins.
#[derive(Clone, Debug, Default)]
pub struct TileFeatures {
    pub points: Vec<ExtractedFeature>,
    /// In the order the tile has them.
    pub pieces: Vec<ExtractedFeature>,
    pub counts: TileCounts,
}

/// Decompresses `data` if it's gzipped (as the basemap stores tiles).
///
/// # Errors
///
/// If it starts as gzip but doesn't inflate.
pub fn inflate(tile: TileKey, data: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>, TileError> {
    if !data.starts_with(&[0x1f, 0x8b]) {
        return Ok(std::borrow::Cow::Borrowed(data));
    }
    let mut out = Vec::with_capacity(data.len() * 4);
    flate2::read::GzDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|source| TileError::Gzip { tile, source })?;
    Ok(std::borrow::Cow::Owned(out))
}

/// Reads the features of the layers the index needs from the tile `tile`, whose bytes
/// (gzipped or not) are `data`.
///
/// # Errors
///
/// If the bytes don't inflate or don't decode as MLT. Nothing of the tile is returned then.
pub fn extract_tile(tile: TileKey, data: &[u8]) -> Result<TileFeatures, TileError> {
    let data = inflate(tile, data)?;
    let mlt = |source| TileError::Mlt { tile, source };
    let mut out = TileFeatures::default();
    let mut decoder = Decoder::default();
    for layer in Parser::default().parse_layers(&data).map_err(mlt)? {
        let Layer::Tag01(layer) = layer else { continue };
        let Some(source) = SourceLayer::from_name(layer.name()) else {
            continue;
        };
        let slot = source as usize;
        let extent = layer.extent().get();
        let layer = layer.decode_all(&mut decoder).map_err(mlt)?;
        let project = Projection::new(tile, extent);
        let mut features = layer.iter_features();
        while let Some(feature) = features.next() {
            let feature = feature.map_err(mlt)?;
            out.counts.features[slot] += 1;
            let geometry = feature.geometry();
            let (geometry, is_point) = match geometry {
                mlt_geo::Geometry::Point(p) => {
                    if project.inside(p.0) {
                        (Geometry::Point(project.point(p.0)), true)
                    } else {
                        out.counts.buffer_points[slot] += 1;
                        continue;
                    }
                }
                mlt_geo::Geometry::MultiPoint(mp) => {
                    let total = mp.0.len() as u64;
                    let kept: Vec<Point<f64>> =
                        mp.0.iter()
                            .filter(|p| project.inside(p.0))
                            .map(|p| project.point(p.0))
                            .collect();
                    out.counts.buffer_points[slot] += total - kept.len() as u64;
                    match kept.len() {
                        0 => continue,
                        1 => (Geometry::Point(kept[0]), true),
                        _ => (Geometry::MultiPoint(MultiPoint(kept)), true),
                    }
                }
                other => match project.geometry(other) {
                    Some(g) => (g, false),
                    None => continue,
                },
            };
            let mut feature_out = ExtractedFeature {
                layer: source,
                // Planetiler writes 0 for a feature without an ID (`VectorTile.NO_FEATURE_ID`).
                id: feature.id().filter(|&id| id != 0),
                tiles: vec![tile],
                names: Vec::new(),
                address: Vec::new(),
                admins: Vec::new(),
                importance: None,
                tags: Vec::new(),
                geometry,
            };
            for column in feature.iter_properties() {
                let key = column.name().to_string();
                let value = column.value();
                if is_derived_name(&key) {
                } else if is_name_key(&key) {
                    if let Some(text) = text(value).filter(|t| !t.trim().is_empty()) {
                        feature_out.names.push((key, text));
                    }
                } else if key.starts_with(ADDRESS_PREFIX) {
                    if let Some(text) = text(value) {
                        feature_out.address.push((key, text));
                    }
                } else if let Some(placetype) = key.strip_prefix(ADMIN_PREFIX) {
                    if let Some(id) = integer(value) {
                        feature_out.admins.push((placetype.to_owned(), id));
                    }
                } else if key == IMPORTANCE {
                    feature_out.importance = number(value);
                } else if let Some(text) = text(value) {
                    feature_out.tags.push((key, text));
                }
            }
            feature_out.names.sort();
            feature_out.address.sort();
            feature_out.admins.sort();
            feature_out.tags.sort();
            if is_point {
                out.counts.points[slot] += match &feature_out.geometry {
                    Geometry::MultiPoint(mp) => mp.0.len() as u64,
                    _ => 1,
                };
                out.points.push(feature_out);
            } else {
                out.counts.pieces[slot] += 1;
                out.pieces.push(feature_out);
            }
        }
    }
    Ok(out)
}

/// A POI feature's layer and its properties, as text.
pub type PoiProperties = (SourceLayer, Vec<(String, String)>);

/// Every property, as text, of the feature with the stable POI ID `id` in the tile's `poi`
/// or `geocoder` layer, and the layer; `None` if neither has it. What a search result that's
/// a pointer shows is read from here.
///
/// # Errors
///
/// If the bytes don't inflate or don't decode as MLT.
pub fn poi_properties(
    tile: TileKey,
    data: &[u8],
    id: u64,
) -> Result<Option<PoiProperties>, TileError> {
    let data = inflate(tile, data)?;
    let mlt = |source| TileError::Mlt { tile, source };
    let mut decoder = Decoder::default();
    for layer in Parser::default().parse_layers(&data).map_err(mlt)? {
        let Layer::Tag01(layer) = layer else { continue };
        let source = match SourceLayer::from_name(layer.name()) {
            Some(s @ (SourceLayer::Poi | SourceLayer::Geocoder)) => s,
            _ => continue,
        };
        let layer = layer.decode_all(&mut decoder).map_err(mlt)?;
        let mut features = layer.iter_features();
        while let Some(feature) = features.next() {
            let feature = feature.map_err(mlt)?;
            if feature.id() != Some(id) {
                continue;
            }
            let properties = feature
                .iter_properties()
                .filter_map(|c| Some((c.name().to_string(), text(c.value())?)))
                .collect();
            return Ok(Some((source, properties)));
        }
    }
    Ok(None)
}

fn text(value: PropValueRef<'_>) -> Option<String> {
    Some(match value {
        PropValueRef::Bool(v) => v.to_string(),
        PropValueRef::I8(v) => v.to_string(),
        PropValueRef::U8(v) => v.to_string(),
        PropValueRef::I32(v) => v.to_string(),
        PropValueRef::U32(v) => v.to_string(),
        PropValueRef::I64(v) => v.to_string(),
        PropValueRef::U64(v) => v.to_string(),
        PropValueRef::F32(v) => v.to_string(),
        PropValueRef::F64(v) => v.to_string(),
        PropValueRef::Str(v) => v.to_owned(),
    })
}

fn number(value: PropValueRef<'_>) -> Option<f64> {
    match value {
        PropValueRef::Bool(_) => None,
        PropValueRef::I8(v) => Some(v.into()),
        PropValueRef::U8(v) => Some(v.into()),
        PropValueRef::I32(v) => Some(v.into()),
        PropValueRef::U32(v) => Some(v.into()),
        PropValueRef::I64(v) => Some(v as f64),
        PropValueRef::U64(v) => Some(v as f64),
        PropValueRef::F32(v) => Some(v.into()),
        PropValueRef::F64(v) => Some(v),
        PropValueRef::Str(v) => v.trim().parse().ok(),
    }
    .filter(|v: &f64| v.is_finite())
}

fn integer(value: PropValueRef<'_>) -> Option<i64> {
    match value {
        PropValueRef::I8(v) => Some(v.into()),
        PropValueRef::U8(v) => Some(v.into()),
        PropValueRef::I32(v) => Some(v.into()),
        PropValueRef::U32(v) => Some(v.into()),
        PropValueRef::I64(v) => Some(v),
        PropValueRef::U64(v) => i64::try_from(v).ok(),
        PropValueRef::F32(v) => whole(f64::from(v)),
        PropValueRef::F64(v) => whole(v),
        PropValueRef::Str(v) => v.trim().parse().ok(),
        PropValueRef::Bool(_) => None,
    }
}

fn whole(v: f64) -> Option<i64> {
    (v.fract() == 0.0 && v.abs() < 9.0e15).then_some(v as i64)
}

/// Tile coordinates to longitude and latitude.
struct Projection {
    tile: TileKey,
    extent: f64,
    extent_i: i64,
}

impl Projection {
    fn new(tile: TileKey, extent: u32) -> Self {
        Self {
            tile,
            extent: f64::from(extent),
            extent_i: i64::from(extent),
        }
    }

    /// Whether a point is in the tile itself, not its buffer: `[0, extent)` both ways, so a
    /// point on an edge between two tiles is in exactly one.
    fn inside(&self, c: mlt_geo::Coord<i32>) -> bool {
        let (x, y) = (i64::from(c.x), i64::from(c.y));
        (0..self.extent_i).contains(&x) && (0..self.extent_i).contains(&y)
    }

    fn coord(&self, c: mlt_geo::Coord<i32>) -> Coord<f64> {
        self.coord_f([f64::from(c.x), f64::from(c.y)])
    }

    fn coord_f(&self, [tx, ty]: [f64; 2]) -> Coord<f64> {
        let size = f64::from(self.tile.z).exp2();
        let x = (f64::from(self.tile.x) + tx / self.extent) / size;
        let y = (f64::from(self.tile.y) + ty / self.extent) / size;
        let lng = x * 360.0 - 180.0;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * y))
            .sinh()
            .atan()
            .to_degrees();
        Coord { x: lng, y: lat }
    }

    fn point(&self, c: mlt_geo::Coord<i32>) -> Point<f64> {
        Point(self.coord(c))
    }

    fn line(&self, points: &[[f64; 2]]) -> geo::LineString<f64> {
        geo::LineString(points.iter().map(|&c| self.coord_f(c)).collect())
    }

    /// The parts of a line inside the tile itself.
    fn lines(&self, l: &mlt_geo::LineString<i32>) -> Vec<geo::LineString<f64>> {
        clip_line(&tile_points(l), self.extent)
            .iter()
            .map(|part| self.line(part))
            .collect()
    }

    /// The part of an area inside the tile itself, if any.
    fn polygon(&self, p: &mlt_geo::Polygon<i32>) -> Option<geo::Polygon<f64>> {
        let ring = |r: &mlt_geo::LineString<i32>| {
            let clipped = clip_ring(&tile_points(r), self.extent);
            (clipped.len() >= 3).then(|| self.line(&clipped))
        };
        Some(geo::Polygon::new(
            ring(p.exterior())?,
            p.interiors().iter().filter_map(ring).collect(),
        ))
    }

    /// A line's or an area's geometry, clipped to the tile itself (its buffer is the
    /// neighbouring tiles', which have the same pieces), or `None` for an empty or
    /// unexpected one, or one only in the buffer.
    fn geometry(&self, g: &mlt_geo::Geometry<i32>) -> Option<Geometry<f64>> {
        let lines: Vec<_> = match g {
            mlt_geo::Geometry::LineString(l) => self.lines(l),
            mlt_geo::Geometry::MultiLineString(ml) => {
                ml.0.iter().flat_map(|l| self.lines(l)).collect()
            }
            mlt_geo::Geometry::Polygon(p) => {
                return self.polygon(p).map(Geometry::Polygon);
            }
            mlt_geo::Geometry::MultiPolygon(mp) => {
                let polygons: Vec<_> = mp.0.iter().filter_map(|p| self.polygon(p)).collect();
                if polygons.is_empty() {
                    return None;
                }
                return Some(Geometry::MultiPolygon(geo::MultiPolygon(polygons)));
            }
            _ => return None,
        };
        let mut lines = lines.into_iter();
        match (lines.next(), lines.len()) {
            (None, _) => None,
            (Some(only), 0) => Some(Geometry::LineString(only)),
            (Some(first), _) => Some(Geometry::MultiLineString(geo::MultiLineString(
                std::iter::once(first).chain(lines).collect(),
            ))),
        }
    }
}

fn tile_points(l: &mlt_geo::LineString<i32>) -> Vec<[f64; 2]> {
    l.0.iter()
        .map(|c| [f64::from(c.x), f64::from(c.y)])
        .collect()
}

/// The part of the segment from `a` to `b` inside `[0, extent]` both ways, if any
/// (Liang–Barsky). An end inside comes back exactly as given.
fn clip_segment(a: [f64; 2], b: [f64; 2], extent: f64) -> Option<([f64; 2], [f64; 2])> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for axis in 0..2 {
        for (p, q) in [(-d[axis], a[axis]), (d[axis], extent - a[axis])] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
    }
    if t0 > t1 {
        return None;
    }
    let at = |t: f64| {
        if t <= 0.0 {
            a
        } else if t >= 1.0 {
            b
        } else {
            [a[0] + t * d[0], a[1] + t * d[1]]
        }
    };
    Some((at(t0), at(t1)))
}

/// The parts of a line, in tile coordinates, inside `[0, extent]` both ways: each a line
/// of at least two distinct points.
fn clip_line(points: &[[f64; 2]], extent: f64) -> Vec<Vec<[f64; 2]>> {
    let mut parts = Vec::new();
    let mut part: Vec<[f64; 2]> = Vec::new();
    for pair in points.windows(2) {
        match clip_segment(pair[0], pair[1], extent) {
            Some((from, to)) if from != to => {
                if part.last() != Some(&from) {
                    if part.len() >= 2 {
                        parts.push(std::mem::take(&mut part));
                    }
                    part.clear();
                    part.push(from);
                }
                part.push(to);
            }
            Some(_) => {}
            None => {
                if part.len() >= 2 {
                    parts.push(std::mem::take(&mut part));
                }
                part.clear();
            }
        }
    }
    if part.len() >= 2 {
        parts.push(part);
    }
    parts
}

/// A ring, in tile coordinates, clipped to `[0, extent]` both ways (Sutherland–Hodgman),
/// closed; fewer than three distinct points if nothing of it is inside.
fn clip_ring(ring: &[[f64; 2]], extent: f64) -> Vec<[f64; 2]> {
    let mut points: Vec<[f64; 2]> = ring.to_vec();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    // Each edge of the tile: the axis, whether inside is below the value, and the value.
    for (axis, below, value) in [
        (0, false, 0.0),
        (0, true, extent),
        (1, false, 0.0),
        (1, true, extent),
    ] {
        if points.is_empty() {
            break;
        }
        let inside = |p: &[f64; 2]| {
            if below {
                p[axis] <= value
            } else {
                p[axis] >= value
            }
        };
        let cross = |a: [f64; 2], b: [f64; 2]| {
            let t = (value - a[axis]) / (b[axis] - a[axis]);
            let mut p = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
            p[axis] = value;
            p
        };
        let input = std::mem::take(&mut points);
        for (i, &current) in input.iter().enumerate() {
            let previous = input[(i + input.len() - 1) % input.len()];
            match (inside(&previous), inside(&current)) {
                (true, true) => points.push(current),
                (true, false) => points.push(cross(previous, current)),
                (false, true) => {
                    points.push(cross(previous, current));
                    points.push(current);
                }
                (false, false) => {}
            }
        }
    }
    points.dedup();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    if points.len() < 3 {
        return Vec::new();
    }
    let first = points[0];
    points.push(first);
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_keys() {
        for key in [
            "name",
            "name:en",
            "name:sr-Latn",
            "alt_name",
            "alt_name:de",
            "official_name",
            "short_name:fr",
            "loc_name",
            "int_name",
        ] {
            assert!(is_name_key(key), "{key}");
        }
        for key in [
            "name_en",
            "name_int",
            "name:latin",
            "name:nonlatin",
            "name:etymology",
            "name:en:pronunciation",
            "old_name",
            "brand",
            "addr:street",
        ] {
            assert!(!is_name_key(key), "{key}");
        }
    }

    #[test]
    fn lines_are_clipped_to_the_tile() {
        let e = 100.0;
        // Out of the buffer, along the tile, out again: the part inside, ends on the edges.
        assert_eq!(
            clip_line(
                &[[-20.0, 50.0], [10.0, 50.0], [90.0, 50.0], [120.0, 50.0]],
                e
            ),
            vec![vec![[0.0, 50.0], [10.0, 50.0], [90.0, 50.0], [100.0, 50.0]]]
        );
        // Leaving and coming back: two parts.
        assert_eq!(
            clip_line(
                &[[50.0, 50.0], [50.0, 120.0], [60.0, 120.0], [60.0, 50.0]],
                e
            ),
            vec![
                vec![[50.0, 50.0], [50.0, 100.0]],
                vec![[60.0, 100.0], [60.0, 50.0]]
            ]
        );
        // Only in the buffer, or only touching a corner: nothing.
        assert!(clip_line(&[[-20.0, 50.0], [-5.0, 60.0]], e).is_empty());
        assert!(clip_line(&[[-10.0, 10.0], [10.0, -10.0]], e).is_empty());
        // Inside: as it was.
        let inside = vec![[1.0, 2.0], [3.0, 4.0], [5.0, 4.0]];
        assert_eq!(clip_line(&inside, e), vec![inside]);
    }

    #[test]
    fn rings_are_clipped_to_the_tile() {
        let e = 100.0;
        let square = [
            [80.0, 10.0],
            [130.0, 10.0],
            [130.0, 40.0],
            [80.0, 40.0],
            [80.0, 10.0],
        ];
        let clipped = clip_ring(&square, e);
        assert_eq!(clipped.first(), clipped.last());
        let mut corners = clipped[..clipped.len() - 1].to_vec();
        corners.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(
            corners,
            vec![[80.0, 10.0], [80.0, 40.0], [100.0, 10.0], [100.0, 40.0]]
        );
        // All in the buffer: nothing.
        assert!(clip_ring(&[[110.0, 0.0], [120.0, 0.0], [120.0, 10.0]], e).is_empty());
        // Inside: as it was.
        let inside = [[1.0, 1.0], [5.0, 1.0], [5.0, 5.0], [1.0, 1.0]];
        assert_eq!(clip_ring(&inside, e), inside.to_vec());
    }

    #[test]
    fn edges_belong_to_one_tile() {
        let p = Projection::new(TileKey { z: 14, x: 0, y: 0 }, 4096);
        assert!(p.inside(mlt_geo::Coord { x: 0, y: 0 }));
        assert!(p.inside(mlt_geo::Coord { x: 4095, y: 4095 }));
        assert!(!p.inside(mlt_geo::Coord { x: 4096, y: 10 }));
        assert!(!p.inside(mlt_geo::Coord { x: -1, y: 10 }));
        let c = Projection::new(TileKey { z: 1, x: 1, y: 0 }, 4096)
            .coord(mlt_geo::Coord { x: 4096, y: 0 });
        assert!((c.x - 180.0).abs() < 1e-9);
        assert!((c.y - 85.051_128_78).abs() < 1e-6);
    }
}
