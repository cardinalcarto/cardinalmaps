use std::collections::HashMap;

use airmail::poi::{Kind, OsmRef, ToIndexPoi};
use geo::{Geometry, LineString, MultiPolygon, Point, Polygon, coord};

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

/// Name keys a place is searched by, each also with a `:<language>` suffix.
const NAME_KEYS: [&str; 6] = ["name", "alt_name", "official_name", "short_name", "loc_name", "int_name"];

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

/// Whether a tag key is a name the element is searched by.
fn is_name_key(key: &str) -> bool {
    let base = key.split(':').next().unwrap_or_default();
    NAME_KEYS.contains(&base)
        && key.split(':').count() <= 2
        && !key.contains("etymology")
        && !key.contains("pronunciation")
}

impl From<OsmPoi> for Option<ToIndexPoi> {
    fn from(poi: OsmPoi) -> Option<ToIndexPoi> {
        let tag = |key: &str| poi.tags.get(key).map(ToString::to_string);
        let house_number = tag("addr:housenumber");
        let road = tag("addr:street");

        let mut names = Vec::new();
        for (key, value) in &poi.tags {
            if !is_name_key(key) {
                continue;
            }
            for name in value.split(';').map(str::trim).filter(|n| !n.is_empty()) {
                names.push(name.to_string());
                // TODO: Remove once we get stemmers again.
                if name.contains("'s") {
                    names.push(name.replace("'s", ""));
                    names.push(name.replace("'s", "s"));
                }
            }
        }
        names.sort();
        names.dedup();

        let kind = if !names.is_empty() {
            Kind::Poi
        } else if house_number.is_some() && road.is_some() {
            Kind::Address
        } else {
            return None;
        };

        let mut indexed = ToIndexPoi::new(kind, names, poi.geometry, poi.tags.clone().into_iter().collect())?;
        indexed.house_number = house_number;
        indexed.road = road;
        indexed.unit = tag("addr:unit");
        indexed.postcode = tag("addr:postcode");
        indexed.osm = Some(poi.osm);
        Some(indexed)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use airmail::poi::OsmType;

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
    fn unnamed_addresses_are_addresses() {
        let poi = OsmPoi::new_from_node([("addr:housenumber", "1530"), ("addr:street", "Post Alley")].into(), (47.6, -122.3), WAY)
            .unwrap()
            .index_poi()
            .unwrap();
        assert_eq!(poi.kind, Kind::Address);
    }
}
