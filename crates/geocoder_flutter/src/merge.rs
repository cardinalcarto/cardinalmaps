//! Joining the pieces of lines and areas that tiles cut at their edges.
//!
//! Streets come from `transportation_name`, whose pieces have no OSM element of their own
//! (Planetiler merges a street's ways): pieces with the same name, lowercased, whose middles
//! are in the same S2 cell of level 12 (a few kilometres across) are one street, as airmail's
//! indexer joins a street's OSM ways (`airmail_indexer/src/osm.rs`).
//!
//! The other layers' pieces (parks, protected areas, named water and waterways, buildings) are
//! pieces of one OSM element each, which their ID says: the pieces with one ID are that
//! element, one feature, as the server indexes it, however many cells it covers. A park's
//! label point, which has the names its outline lacks, joins its outline too. Pieces without
//! an ID fall back to the name and cell.
//!
//! The result doesn't depend on the order tiles are added in: the joined feature takes the
//! attributes of its best piece (for streets the biggest road class, then the first tile),
//! every name any piece has (the best piece's value of a key first), and the pieces' shapes
//! in that order until the next would take it over [`MAX_POINTS`].

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use geo::{Coord, Geometry, LineString, MultiLineString, MultiPolygon, Polygon, Simplify};

use crate::extract::{ExtractedFeature, SourceLayer, TileKey};

/// The S2 level of the cell a piece's middle is in, as airmail's `STREET_CELL_LEVEL`.
pub const CELL_LEVEL: u64 = 12;

/// The most points a joined feature's shape keeps, as airmail's `MAX_STREET_POINTS`.
pub const MAX_POINTS: usize = 1000;

/// How much pieces are simplified, in degrees (about 5 m), as airmail's streets are.
pub const SIMPLIFY_DEGREES: f64 = 0.00005;

/// How many z14 tile columns (about 25 km in Washington) past the last piece of an element
/// the tiles must be before it's finished. A line's or a polygon's pieces are in a run of
/// columns without gaps (but where a tile drops a sliver), so a few would do; a multipolygon's
/// parts can be farther apart, and parts farther than this become features of their own.
pub const ELEMENT_GAP_COLUMNS: u32 = 16;

/// `transportation_name` classes, biggest first: a street's attributes come from its piece of
/// the biggest class, as airmail takes a street's tags from its biggest way.
const ROAD_CLASSES: [&str; 12] = [
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "minor",
    "busway",
    "service",
    "track",
    "path",
    "raceway",
    "ferry",
];

/// What the merger has joined so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeCounts {
    /// Pieces added.
    pub pieces: u64,
    /// Pieces with no name or address to join (or index) them by, dropped.
    pub unnamed: u64,
    /// Features the pieces make.
    pub features: u64,
}

/// Where a piece sorts among a feature's pieces: lower is better.
type Order = (usize, TileKey, u32);

#[derive(Clone, Debug)]
enum Part {
    Line(Vec<[f32; 2]>),
    Polygon(Vec<Vec<[f32; 2]>>),
}

impl Part {
    fn points(&self) -> usize {
        match self {
            Self::Line(l) => l.len(),
            Self::Polygon(rings) => rings.iter().map(Vec::len).sum(),
        }
    }
}

#[derive(Clone, Debug)]
struct Group {
    /// The best piece so far, without its geometry.
    best: (Order, ExtractedFeature),
    /// Each name key's value from the best piece that has it, by key. (Vectors, not maps:
    /// a region holds hundreds of thousands of these, most with a name or two.)
    names: Vec<(String, Order, String)>,
    /// Its pieces' parts, best first (a piece's own in order), as many as [`Group::trim`]
    /// leaves.
    parts: Vec<((Order, u32), Part)>,
    /// Sorted.
    tiles: Vec<TileKey>,
    id: Option<u64>,
    /// The highest importance a piece has.
    importance: Option<f64>,
    /// A label point's position, the shape if no piece has another.
    point: Option<Coord<f64>>,
    /// For a street, the last tile column (z14 x) a piece of it can come from: the pieces'
    /// middles are in its cell, and a piece's middle is in its own tile. For an element,
    /// the last column a piece came from, plus [`ELEMENT_GAP_COLUMNS`].
    last_column: u32,
}

/// What groups pieces into one feature: the layer and an OSM element's ID, or for pieces
/// without one, the label and the S2 cell of the middle.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Key {
    Element(SourceLayer, u64),
    Named(SourceLayer, String, u64),
}

/// Whether a layer's pieces are joined by their ID: every layer's but streets'.
fn joins_by_id(layer: SourceLayer) -> bool {
    layer != SourceLayer::TransportationName
}

/// Joins pieces of lines and areas across tiles.
#[derive(Debug, Default)]
pub struct Merger {
    /// Boxed, so the table's spare capacity costs a pointer a slot, not a group.
    groups: HashMap<Key, Box<Group>>,
    counts: MergeCounts,
}

/// The last z14 tile column that has a part of the S2 cell `cell`, give or take one for the
/// edge between columns; `u32::MAX` for a cell across the antimeridian.
fn last_column(cell: s2::cellid::CellID) -> u32 {
    let bound = s2::cell::Cell::from(cell).rect_bound();
    if bound.lng.is_inverted() || bound.lng.is_full() {
        return u32::MAX;
    }
    let n = f64::from(1u32 << 14);
    let column = ((bound.lng.hi.to_degrees() + 180.0) / 360.0 * n).floor();
    (column.clamp(0.0, n - 1.0) as u32).saturating_add(1)
}

impl Merger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn counts(&self) -> &MergeCounts {
        &self.counts
    }

    /// Adds the piece that's `index`th among its tile's pieces. A point (a park's label)
    /// joins its element's pieces, or is the feature's shape if it has no others.
    pub fn add(&mut self, mut piece: ExtractedFeature, index: u32) {
        self.counts.pieces += 1;
        let Some(label) = label(&piece) else {
            self.counts.unnamed += 1;
            return;
        };
        let parts = parts(&piece.geometry);
        let Some(middle) = middle(&piece.geometry) else {
            self.counts.unnamed += 1;
            return;
        };
        let point = matches!(piece.geometry, Geometry::Point(_)).then_some(middle);
        let tile = piece.tiles[0];
        let (key, last) = match piece.id {
            Some(id) if joins_by_id(piece.layer) => (
                Key::Element(piece.layer, id),
                tile.x.saturating_add(ELEMENT_GAP_COLUMNS),
            ),
            _ => {
                let cell =
                    s2::cellid::CellID::from(s2::latlng::LatLng::from_degrees(middle.y, middle.x))
                        .parent(CELL_LEVEL);
                (Key::Named(piece.layer, label, cell.0), last_column(cell))
            }
        };
        let order = (rank(&piece), tile, index);
        piece.geometry = Geometry::GeometryCollection(geo::GeometryCollection(Vec::new()));
        let names = std::mem::take(&mut piece.names);
        piece.tiles = Vec::new();
        piece.tags.shrink_to_fit();
        piece.address.shrink_to_fit();
        piece.admins.shrink_to_fit();
        let id = piece.id;
        let importance = piece.importance;
        let group = match self.groups.entry(key) {
            Entry::Occupied(entry) => {
                let group = entry.into_mut();
                if order < group.best.0 {
                    group.best = (order, piece);
                }
                group.last_column = group.last_column.max(last);
                group
            }
            Entry::Vacant(entry) => {
                self.counts.features += 1;
                entry.insert(Box::new(Group {
                    best: (order, piece),
                    names: Vec::new(),
                    parts: Vec::new(),
                    tiles: Vec::new(),
                    id,
                    importance: None,
                    point: None,
                    last_column: last,
                }))
            }
        };
        group.importance = match (group.importance, importance) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        if group.point.is_none() {
            group.point = point;
        }
        for (key, value) in names {
            match group.names.binary_search_by(|(k, _, _)| k.cmp(&key)) {
                Ok(i) if group.names[i].1 <= order => {}
                Ok(i) => group.names[i] = (key, order, value),
                Err(i) => group.names.insert(i, (key, order, value)),
            }
        }
        for (i, part) in parts.into_iter().enumerate() {
            let key = (order, u32::try_from(i).unwrap_or(u32::MAX));
            let at = group.parts.partition_point(|(k, _)| *k < key);
            group.parts.insert(at, (key, part));
        }
        group.trim();
        if let Err(i) = group.tiles.binary_search(&tile) {
            group.tiles.insert(i, tile);
        }
        group.id = match (group.id, id) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }

    /// The features no piece can be added to any more once tiles of column `column` (z14
    /// x) and after are all that's left to add, in a stable order (by layer, name, then
    /// cell). For a region read in order of column, they can be given out as it's read
    /// rather than all held to the end.
    pub fn finished(&mut self, column: u32) -> impl Iterator<Item = ExtractedFeature> + use<> {
        let done: Vec<Key> = self
            .groups
            .iter()
            .filter(|(_, g)| g.last_column < column)
            .map(|(k, _)| k.clone())
            .collect();
        let groups: Vec<_> = done
            .into_iter()
            .filter_map(|k| self.groups.remove_entry(&k))
            .collect();
        sorted_features(groups)
    }

    /// The first tile column (z14 x) any piece of a feature not finished yet came from.
    #[must_use]
    pub fn first_open_column(&self) -> Option<u32> {
        // A group's tiles are sorted, by column first.
        self.groups
            .values()
            .filter_map(|g| g.tiles.first())
            .map(|t| t.x)
            .min()
    }

    /// The joined features, in a stable order (by layer, name, then cell), made one by one.
    pub fn finish(self) -> impl Iterator<Item = ExtractedFeature> {
        sorted_features(self.groups.into_iter().collect())
    }
}

fn sorted_features(mut groups: Vec<(Key, Box<Group>)>) -> impl Iterator<Item = ExtractedFeature> {
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    groups.into_iter().map(|(_, g)| g.into_feature())
}

impl Group {
    /// Keeps the best parts up to [`MAX_POINTS`] in all, and always the first, so every
    /// feature has a shape. A part dropped here would be dropped at the end too, whatever is
    /// added later (better parts only take more of the budget), so the pieces of a long street
    /// or a big area aren't all held until the region's tiles are read.
    fn trim(&mut self) {
        let mut kept = 0;
        let mut keep = self.parts.len();
        for (i, (_, part)) in self.parts.iter().enumerate() {
            if i > 0 && kept + part.points() > MAX_POINTS {
                keep = i;
                break;
            }
            kept += part.points();
        }
        self.parts.truncate(keep);
    }

    fn into_feature(self) -> ExtractedFeature {
        let mut feature = self.best.1;
        feature.id = self.id;
        feature.importance = self.importance;
        feature.tiles = self.tiles;
        feature.names = self.names.into_iter().map(|(k, _, v)| (k, v)).collect();
        let (mut lines, mut polygons) = (Vec::new(), Vec::new());
        for (_, part) in self.parts {
            match part {
                Part::Line(l) => lines.push(line(&l)),
                Part::Polygon(rings) => {
                    let mut rings = rings.iter().map(|r| line(r));
                    if let Some(exterior) = rings.next() {
                        polygons.push(Polygon::new(exterior, rings.collect()));
                    }
                }
            }
        }
        feature.geometry = match (lines.is_empty(), polygons.is_empty()) {
            (true, true) if self.point.is_some() => {
                Geometry::Point(geo::Point(self.point.unwrap_or_default()))
            }
            (false, true) => Geometry::MultiLineString(MultiLineString(lines)),
            (true, false) => Geometry::MultiPolygon(MultiPolygon(polygons)),
            _ => Geometry::GeometryCollection(geo::GeometryCollection(
                lines
                    .into_iter()
                    .map(Geometry::LineString)
                    .chain(polygons.into_iter().map(Geometry::Polygon))
                    .collect(),
            )),
        };
        feature
    }
}

fn line(points: &[[f32; 2]]) -> LineString<f64> {
    LineString(
        points
            .iter()
            .map(|&[x, y]| Coord {
                x: f64::from(x),
                y: f64::from(y),
            })
            .collect(),
    )
}

/// What pieces of one feature share: its name, lowercased, or else its address.
fn label(piece: &ExtractedFeature) -> Option<String> {
    let name = piece
        .name("name")
        .or_else(|| piece.names.first().map(|(_, v)| v.as_str()))
        .map(str::trim)
        .filter(|n| !n.is_empty());
    if let Some(name) = name {
        return Some(name.to_lowercase());
    }
    let number = piece.addr("addr:housenumber");
    let street = piece.addr("addr:street").or(piece.addr("addr:place"));
    match (number, street) {
        (Some(n), Some(s)) => Some(format!("\u{1}{} {}", n.trim(), s.trim()).to_lowercase()),
        _ => None,
    }
}

fn rank(piece: &ExtractedFeature) -> usize {
    if piece.layer != SourceLayer::TransportationName {
        return 0;
    }
    let class = piece.tag("class");
    ROAD_CLASSES
        .iter()
        .position(|c| Some(*c) == class)
        .unwrap_or(ROAD_CLASSES.len())
}

/// A piece's middle: its middle point for a line, as airmail takes a way's, and the middle of
/// its bounds for an area.
fn middle(geometry: &Geometry<f64>) -> Option<Coord<f64>> {
    match geometry {
        Geometry::Point(p) => Some(p.0),
        Geometry::LineString(l) => l.0.get(l.0.len() / 2).copied(),
        Geometry::MultiLineString(ml) => {
            let count: usize = ml.0.iter().map(|l| l.0.len()).sum();
            ml.0.iter().flat_map(|l| &l.0).nth(count / 2).copied()
        }
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => {
            use geo::BoundingRect;
            geometry.bounding_rect().map(|r| r.center())
        }
        _ => None,
    }
}

fn compact(line: &LineString<f64>) -> Vec<[f32; 2]> {
    line.simplify(SIMPLIFY_DEGREES)
        .0
        .into_iter()
        .map(|c| [c.x as f32, c.y as f32])
        .collect()
}

/// A piece's lines or polygons, simplified and in single precision (about a metre), since
/// every piece is held until the region's tiles are all read.
fn parts(geometry: &Geometry<f64>) -> Vec<Part> {
    let polygon = |p: &Polygon<f64>| -> Option<Part> {
        let rings: Vec<_> = std::iter::once(p.exterior())
            .chain(p.interiors())
            .map(compact)
            .filter(|r| r.len() >= 4)
            .collect();
        (!rings.is_empty()).then_some(Part::Polygon(rings))
    };
    match geometry {
        Geometry::LineString(l) => vec![Part::Line(compact(l))],
        Geometry::MultiLineString(ml) => ml.0.iter().map(|l| Part::Line(compact(l))).collect(),
        Geometry::Polygon(p) => polygon(p).into_iter().collect(),
        Geometry::MultiPolygon(mp) => mp.0.iter().filter_map(polygon).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{LineString, Point, Polygon};

    fn piece(
        layer: SourceLayer,
        id: Option<u64>,
        name: &str,
        x: u32,
        geometry: Geometry<f64>,
    ) -> ExtractedFeature {
        ExtractedFeature {
            layer,
            id,
            tiles: vec![TileKey { z: 14, x, y: 5700 }],
            names: vec![("name".to_owned(), name.to_owned())],
            address: Vec::new(),
            admins: Vec::new(),
            importance: None,
            tags: Vec::new(),
            geometry,
        }
    }

    fn square(lon: f64) -> Geometry<f64> {
        Geometry::Polygon(Polygon::new(
            LineString::from(vec![
                (lon, 46.8),
                (lon + 0.01, 46.8),
                (lon + 0.01, 46.81),
                (lon, 46.81),
                (lon, 46.8),
            ]),
            vec![],
        ))
    }

    fn column(lon: f64) -> u32 {
        ((lon + 180.0) / 360.0 * 16384.0) as u32
    }

    #[test]
    fn an_elements_pieces_are_one_feature_across_cells() {
        // A wilderness's pieces, tens of kilometres apart (many level-12 cells), with its ID.
        let mut merger = Merger::new();
        for (i, lon) in [-121.9, -121.8, -121.7, -121.6].into_iter().enumerate() {
            let p = piece(
                SourceLayer::Park,
                Some(72),
                "Mount Rainier Wilderness",
                column(lon),
                square(lon),
            );
            merger.add(p, i as u32);
        }
        // Without an ID, by name and cell: two cells, two features.
        for (i, lon) in [-121.9, -121.6].into_iter().enumerate() {
            merger.add(
                piece(
                    SourceLayer::Park,
                    None,
                    "Nameless Woods",
                    column(lon),
                    square(lon),
                ),
                i as u32,
            );
        }
        // Streets join by name and cell even with IDs.
        for (i, lon) in [-121.9, -121.6].into_iter().enumerate() {
            let line =
                Geometry::LineString(LineString::from(vec![(lon, 46.8), (lon + 0.001, 46.8)]));
            merger.add(
                piece(
                    SourceLayer::TransportationName,
                    Some(9),
                    "Main Street",
                    column(lon),
                    line,
                ),
                i as u32,
            );
        }
        let features: Vec<_> = merger.finish().collect();
        let count = |layer, name: &str| {
            features
                .iter()
                .filter(|f| f.layer == layer && f.name("name") == Some(name))
                .count()
        };
        assert_eq!(count(SourceLayer::Park, "Mount Rainier Wilderness"), 1);
        assert_eq!(count(SourceLayer::Park, "Nameless Woods"), 2);
        assert_eq!(count(SourceLayer::TransportationName, "Main Street"), 2);
        let wilderness = features.iter().find(|f| f.id == Some(72)).unwrap();
        assert!(matches!(&wilderness.geometry, Geometry::MultiPolygon(mp) if mp.0.len() == 4));
        assert_eq!(wilderness.tiles.len(), 4);
    }

    #[test]
    fn an_element_is_finished_once_the_tiles_are_well_past_it() {
        let mut merger = Merger::new();
        let x = column(-121.9);
        merger.add(
            piece(
                SourceLayer::Waterway,
                Some(5),
                "Salzer Creek",
                x,
                square(-121.9),
            ),
            0,
        );
        assert_eq!(merger.finished(x + ELEMENT_GAP_COLUMNS).count(), 0);
        merger.add(
            piece(
                SourceLayer::Waterway,
                Some(5),
                "Salzer Creek",
                x + 3,
                square(-121.85),
            ),
            0,
        );
        assert_eq!(merger.finished(x + ELEMENT_GAP_COLUMNS + 1).count(), 0);
        assert_eq!(merger.finished(x + 3 + ELEMENT_GAP_COLUMNS + 1).count(), 1);
    }

    #[test]
    fn a_label_point_alone_is_its_features_shape() {
        let mut merger = Merger::new();
        let mut label = piece(
            SourceLayer::Park,
            Some(8),
            "Tiny Park",
            100,
            Geometry::Point(Point::new(-122.3, 47.6)),
        );
        label.importance = Some(0.2);
        merger.add(label, 0);
        let features: Vec<_> = merger.finish().collect();
        assert_eq!(features.len(), 1);
        assert_eq!(
            features[0].geometry,
            Geometry::Point(Point::new(-122.3, 47.6))
        );
        assert_eq!(features[0].importance, Some(0.2));
    }
}
