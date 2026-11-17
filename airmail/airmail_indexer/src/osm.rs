use std::collections::HashMap;

use airmail::osm::is_name_key;
use airmail::poi::{OsmRef, ToIndexPoi};
use geo::{Geometry, LineString, MultiLineString, MultiPolygon, Point, Polygon, Simplify, coord};

/// `place` values Who's On First covers as administrative areas, which are
/// indexed from there instead.
const ADMIN_PLACES: [&str; 15] = [
    "borough",
    "city",
    "country",
    "county",
    "district",
    "hamlet",
    "municipality",
    "neighbourhood",
    "province",
    "quarter",
    "region",
    "state",
    "suburb",
    "town",
    "village",
];

/// `highway` values of roads and paths that are found by name as streets,
/// biggest first.
const STREET_HIGHWAYS: [&str; 17] = [
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "unclassified",
    "residential",
    "living_street",
    "pedestrian",
    "road",
    "busway",
    "service",
    "track",
    "cycleway",
    "footway",
    "path",
    "bridleway",
];

/// Ways with the same name whose middles are in the same S2 cell of this
/// level (a few kilometres across) are pieces of the same street.
const STREET_CELL_LEVEL: u64 = 12;

/// The most points a street's shape keeps, after simplifying its ways.
const MAX_STREET_POINTS: usize = 1000;

/// How much a street's ways are simplified, in degrees (about 5 m).
const STREET_SIMPLIFY_DEGREES: f64 = 0.00005;

/// One way of a named street.
#[derive(Debug, Clone)]
pub struct StreetWay {
    /// The street's name, lowercased, and the S2 cell its middle is in.
    pub key: (String, u64),
    /// Its `highway` and name tags.
    pub tags: Vec<(String, String)>,
    pub osm: OsmRef,
    /// Simplified, as `(lon, lat)` in single precision (about a metre),
    /// since every named road in the world is held until they're all read.
    line: Vec<(f32, f32)>,
}

impl StreetWay {
    /// A named road or path through `(lat, lon)` points, or `None` if it isn't one.
    pub fn new(tags: &HashMap<&str, &str>, points: &[(f64, f64)], osm: OsmRef) -> Option<Self> {
        let highway = tags.get("highway")?;
        let name = tags.get("name")?.trim();
        if !STREET_HIGHWAYS.contains(highway) || name.is_empty() || points.len() < 2 {
            return None;
        }
        let line: LineString<f64> = points.iter().map(|&(lat, lon)| coord! { x: lon, y: lat }).collect();
        let (lat, lon) = points[points.len() / 2];
        let cell = s2::cellid::CellID::from(s2::latlng::LatLng::from_degrees(lat, lon)).parent(STREET_CELL_LEVEL);
        let tags = tags
            .iter()
            .filter(|(k, _)| **k == "highway" || is_name_key(k))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let line = line.simplify(STREET_SIMPLIFY_DEGREES).0.into_iter().map(|c| (c.x as f32, c.y as f32)).collect();
        Some(Self { key: (name.to_lowercase(), cell.0), tags, osm, line })
    }

    fn highway_rank(&self) -> usize {
        let highway = self.tags.iter().find(|(k, _)| k == "highway").map(|(_, v)| v.as_str());
        STREET_HIGHWAYS.iter().position(|h| Some(*h) == highway).unwrap_or(STREET_HIGHWAYS.len())
    }
}

/// The ways of a street, merged.
#[derive(Debug, Clone)]
pub struct Street {
    /// The tags of its biggest way, lowest ID first.
    tags: Vec<(String, String)>,
    /// Its way with the lowest ID, which identifies it.
    osm: OsmRef,
    rank: usize,
    lines: Vec<Vec<(f32, f32)>>,
    points: usize,
}

impl From<StreetWay> for Street {
    fn from(way: StreetWay) -> Self {
        let rank = way.highway_rank();
        let points = way.line.len();
        Self { tags: way.tags, osm: way.osm, rank, lines: vec![way.line], points }
    }
}

impl Street {
    /// Adds another of its ways.
    pub fn add(&mut self, way: StreetWay) {
        let rank = way.highway_rank();
        if (rank, way.osm.id) < (self.rank, self.osm.id) {
            self.rank = rank;
            self.tags = way.tags;
        }
        if way.osm.id < self.osm.id {
            self.osm = way.osm;
        }
        if self.points + way.line.len() <= MAX_STREET_POINTS {
            self.points += way.line.len();
            self.lines.push(way.line);
        }
    }

    /// The street as a document, found by its names, and by them as a road
    /// (`w 26th st` for West 26th Street).
    pub fn index_poi(self) -> Option<ToIndexPoi> {
        let lines = self
            .lines
            .into_iter()
            .map(|line| line.into_iter().map(|(x, y)| coord! { x: f64::from(x), y: f64::from(y) }).collect::<LineString<f64>>())
            .collect();
        let mut poi = airmail::osm::street_document(self.tags, Geometry::MultiLineString(MultiLineString(lines)))?;
        poi.osm = Some(self.osm);
        Some(poi)
    }
}

/// An OSM element worth indexing, with its shape.
#[derive(Debug, Clone)]
pub struct OsmPoi {
    pub tags: HashMap<String, String>,
    pub geometry: Geometry<f64>,
    pub osm: OsmRef,
}

impl OsmPoi {
    /// A node, at `(lat, lon)`.
    pub fn new_from_node(tags: HashMap<&str, &str>, location: (f64, f64), osm: OsmRef) -> Option<Self> {
        let tags = Self::validate_tags(tags)?;
        let geometry = Geometry::Point(Point::new(location.1, location.0));
        Some(Self { tags, geometry, osm })
    }

    /// A way through `(lat, lon)` points: an area if it's closed and its
    /// tags don't say it's a line, else a line.
    pub fn new_from_way(tags: HashMap<&str, &str>, points: &[(f64, f64)], osm: OsmRef) -> Option<Self> {
        let tags = Self::validate_tags(tags)?;
        let line: LineString<f64> = points.iter().map(|&(lat, lon)| coord! { x: lon, y: lat }).collect();
        let geometry = match line.0.len() {
            0 => return None,
            1 => Geometry::Point(Point(line.0[0])),
            _ if line.is_closed() && line.0.len() >= 4 && is_area(&tags) => Geometry::Polygon(Polygon::new(line, vec![])),
            _ => Geometry::LineString(line),
        };
        Some(Self { tags, geometry, osm })
    }

    /// A multipolygon relation, already assembled.
    pub fn new_from_relation(tags: HashMap<&str, &str>, shape: MultiPolygon<f64>, osm: OsmRef) -> Option<Self> {
        let tags = Self::validate_tags(tags)?;
        Some(Self { tags, geometry: Geometry::MultiPolygon(shape), osm })
    }

    /// The tags of an element worth indexing, or `None`.
    fn validate_tags(tags: HashMap<&str, &str>) -> Option<HashMap<String, String>> {
        if tags.is_empty() {
            return None;
        }
        // Roads and administrative areas are found through addresses and Who's
        // On First instead. Bus stops are POIs though their key is `highway`.
        let bus_stop = tags.get("highway") == Some(&"bus_stop");
        if (tags.contains_key("highway") && !bus_stop)
            || tags.contains_key("boundary")
            || tags.contains_key("admin_level")
            || tags.get("place").is_some_and(|place| ADMIN_PLACES.contains(place))
        {
            return None;
        }
        Some(tags.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    pub fn index_poi(self) -> Option<ToIndexPoi> {
        self.into()
    }
}

/// Whether a closed way with these tags is an area rather than a ring-shaped
/// line, following the usual OSM conventions.
fn is_area(tags: &HashMap<String, String>) -> bool {
    match tags.get("area").map(String::as_str) {
        Some("yes") => return true,
        Some("no") => return false,
        _ => {}
    }
    let linear = |key: &str, values: &[&str]| tags.get(key).is_some_and(|v| values.contains(&v.as_str()));
    !(tags.contains_key("barrier")
        || tags.contains_key("route")
        || linear("natural", &["coastline", "cliff", "ridge", "arete", "tree_row"])
        || (tags.contains_key("waterway") && !linear("waterway", &["riverbank", "dock", "boatyard"])))
}

impl From<OsmPoi> for Option<ToIndexPoi> {
    fn from(poi: OsmPoi) -> Option<ToIndexPoi> {
        let mut indexed = airmail::osm::document(poi.tags.into_iter().collect(), poi.geometry)?;
        indexed.osm = Some(poi.osm);
        Some(indexed)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use airmail::poi::{Kind, OsmType};

    const WAY: OsmRef = OsmRef { osm_type: OsmType::Way, id: 1, version: Some(1) };

    fn square() -> Vec<(f64, f64)> {
        vec![(47.0, -122.0), (47.0, -121.9), (47.1, -121.9), (47.1, -122.0), (47.0, -122.0)]
    }

    #[test]
    fn closed_ways_are_areas_unless_tagged_as_lines() {
        let park = OsmPoi::new_from_way([("leisure", "park"), ("name", "Park")].into(), &square(), WAY).unwrap();
        assert!(matches!(park.geometry, Geometry::Polygon(_)));
        let fence = OsmPoi::new_from_way([("barrier", "fence"), ("name", "Fence")].into(), &square(), WAY).unwrap();
        assert!(matches!(fence.geometry, Geometry::LineString(_)));
    }

    #[test]
    fn admin_places_come_from_whos_on_first() {
        let city = OsmPoi::new_from_node([("place", "city"), ("name", "Seattle")].into(), (47.6, -122.3), WAY);
        assert!(city.is_none());
        let square = OsmPoi::new_from_node([("place", "square"), ("name", "Pioneer Square")].into(), (47.6, -122.3), WAY);
        assert!(square.is_some());
    }

    #[test]
    fn indexes_alternative_names_but_not_name_metadata() {
        let poi = OsmPoi::new_from_way(
            [
                ("amenity", "university"),
                ("name", "University of Washington"),
                ("short_name", "UW"),
                ("name:etymology:wikidata", "Q1223"),
            ]
            .into(),
            &square(),
            WAY,
        )
        .unwrap()
        .index_poi()
        .unwrap();
        assert_eq!(poi.names, vec!["UW", "University of Washington"]);
        assert_eq!(poi.kind, Kind::Poi);
    }

    #[test]
    fn named_roads_are_streets() {
        let way = |id: i64, highway: &str, points: &[(f64, f64)]| {
            let osm = OsmRef { osm_type: OsmType::Way, id, version: Some(1) };
            StreetWay::new(&[("highway", highway), ("name", "Broadway"), ("surface", "asphalt")].into(), points, osm)
        };
        let a = way(5, "residential", &[(47.60, -122.32), (47.61, -122.32)]).unwrap();
        let b = way(3, "secondary", &[(47.6101, -122.3201), (47.6102, -122.3202)]).unwrap();
        assert_eq!(a.key, b.key);
        assert!(way(6, "residential", &[(40.7, -74.0), (40.71, -74.0)]).unwrap().key != a.key);
        assert!(way(7, "proposed", &[(47.60, -122.32), (47.61, -122.32)]).is_none());

        let mut street = Street::from(a);
        street.add(b);
        let poi = street.index_poi().unwrap();
        assert_eq!(poi.kind, Kind::Street);
        assert_eq!(poi.names, vec!["Broadway"]);
        assert_eq!(poi.road.as_deref(), Some("Broadway"));
        assert_eq!(poi.osm.unwrap().id, 3);
        assert_eq!(poi.tag("highway"), Some("secondary"));
        assert_eq!(poi.tag("surface"), None);
        assert!(matches!(poi.geometry, Geometry::MultiLineString(ref m) if m.0.len() == 2));
    }

    #[test]
    fn unnamed_addresses_are_addresses() {
        let poi = OsmPoi::new_from_node([("addr:housenumber", "1530"), ("addr:street", "Post Alley")].into(), (47.6, -122.3), WAY)
            .unwrap()
            .index_poi()
            .unwrap();
        assert_eq!(poi.kind, Kind::Address);
    }
}
