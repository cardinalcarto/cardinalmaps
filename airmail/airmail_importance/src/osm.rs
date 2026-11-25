//! Reading an OSM element's tags for its shape and importance, and which
//! elements airmail's indexer makes a POI of (so that poi_service gives every
//! one of them an importance).

use crate::importance::{PoiSignals, is_transit_stop_tag};
use crate::qrank::QRank;

/// Name keys a place is searched by, each also with a `:<language>` suffix.
pub const NAME_KEYS: [&str; 6] = [
    "name",
    "alt_name",
    "official_name",
    "short_name",
    "loc_name",
    "int_name",
];

/// `place` values Who's On First covers as administrative areas, which are
/// indexed from there instead of from OSM.
pub const ADMIN_PLACES: [&str; 15] = [
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

/// Whether a tag key is a name the element is searched by.
#[must_use]
pub fn is_name_key(key: &str) -> bool {
    let base = key.split(':').next().unwrap_or_default();
    NAME_KEYS.contains(&base)
        && key.split(':').count() <= 2
        && !key.contains("etymology")
        && !key.contains("pronunciation")
}

/// Whether airmail's indexer reads an element with these tags at all: it has
/// tags, and it isn't a road (bus stops aside), a boundary or an
/// administrative place, which are found through addresses and Who's On First
/// instead.
#[must_use]
pub fn is_indexed<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> bool {
    let mut any = false;
    for (key, value) in tags {
        any = true;
        let excluded = match key {
            "highway" => value != "bus_stop",
            "boundary" | "admin_level" => true,
            "place" => ADMIN_PLACES.contains(&value),
            _ => false,
        };
        if excluded {
            return false;
        }
    }
    any
}

/// Whether airmail's indexer makes a POI document of an element with these
/// tags, if its shape is usable: one it reads ([`is_indexed`]) with a
/// non-empty name. (A relation must also be a multipolygon with a `name`.)
#[must_use]
pub fn is_indexed_poi<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)> + Clone) -> bool {
    is_indexed(tags.clone())
        && tags
            .into_iter()
            .any(|(k, v)| is_name_key(k) && v.split(';').any(|n| !n.trim().is_empty()))
}

/// Whether a closed way with these tags is an area rather than a ring-shaped
/// line, following the usual OSM conventions.
#[must_use]
pub fn is_area<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> bool {
    let (mut area, mut linear, mut waterway_area) = (None, false, None);
    for (key, value) in tags {
        match key {
            "area" => match value {
                "yes" => area = Some(true),
                "no" => area = Some(false),
                _ => {}
            },
            "barrier" | "route" => linear = true,
            "natural" => {
                linear |= matches!(
                    value,
                    "coastline" | "cliff" | "ridge" | "arete" | "tree_row"
                )
            }
            "waterway" => waterway_area = Some(matches!(value, "riverbank" | "dock" | "boatyard")),
            _ => {}
        }
    }
    area.unwrap_or(!(linear || waterway_area == Some(false)))
}

/// What a POI's tags, the area of its shape as stored (see
/// [`crate::shape::stored_area_m2`]) and QRank say about its prominence.
#[must_use]
pub fn poi_signals<'a>(
    tags: impl IntoIterator<Item = (&'a str, &'a str)>,
    area_m2: f64,
    qrank: Option<&QRank>,
) -> PoiSignals {
    let (mut wikidata, mut transit_stop) = (None, false);
    for (key, value) in tags {
        if key == "wikidata" && wikidata.is_none() {
            wikidata = Some(value);
        }
        transit_stop |= is_transit_stop_tag(key, value);
    }
    PoiSignals {
        has_wikidata: wikidata.is_some(),
        qrank: wikidata.zip(qrank).and_then(|(id, qrank)| qrank.get(id)),
        area_m2,
        transit_stop,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn closed_ways_are_areas_unless_tagged_as_lines() {
        assert!(is_area([("leisure", "park")]));
        assert!(!is_area([("barrier", "fence")]));
        assert!(!is_area([("natural", "coastline")]));
        assert!(is_area([("natural", "water")]));
        assert!(!is_area([("waterway", "river")]));
        assert!(is_area([("waterway", "riverbank")]));
        assert!(is_area([("barrier", "fence"), ("area", "yes")]));
        assert!(!is_area([("leisure", "park"), ("area", "no")]));
    }

    #[test]
    fn indexed_pois_are_named_and_not_roads_or_admin_areas() {
        assert!(is_indexed_poi([
            ("building", "yes"),
            ("name", "Building 43")
        ]));
        assert!(is_indexed_poi([
            ("waterway", "stream"),
            ("name:en", "Salzer Creek")
        ]));
        assert!(is_indexed_poi([
            ("highway", "bus_stop"),
            ("name", "3rd & Pike")
        ]));
        assert!(is_indexed_poi([
            ("place", "square"),
            ("name", "Occidental Square")
        ]));
        assert!(!is_indexed_poi([
            ("highway", "residential"),
            ("name", "Pike Street")
        ]));
        assert!(!is_indexed_poi([
            ("boundary", "protected_area"),
            ("name", "Wilderness")
        ]));
        assert!(!is_indexed_poi([("place", "town"), ("name", "Forks")]));
        assert!(!is_indexed_poi([("building", "yes"), ("name", " ; ")]));
        assert!(!is_indexed_poi([
            ("building", "yes"),
            ("name:etymology", "Q1")
        ]));
        assert!(!is_indexed_poi([
            ("addr:housenumber", "1"),
            ("addr:street", "Main Street")
        ]));
        assert!(is_indexed([
            ("addr:housenumber", "1"),
            ("addr:street", "Main Street")
        ]));
        assert!(!is_indexed([]));
    }

    #[test]
    fn reads_wikidata_and_transit_stops() {
        let qrank = QRank::parse("Entity,QRank\nQ60,1000\n".as_bytes()).unwrap();
        let signals = poi_signals(
            [("name", "X"), ("wikidata", "Q60"), ("highway", "bus_stop")],
            5.0,
            Some(&qrank),
        );
        assert!(signals.has_wikidata && signals.transit_stop);
        assert_eq!((signals.qrank, signals.area_m2), (Some(1000), 5.0));
        let signals = poi_signals([("wikidata", "Q60")], 0.0, None);
        assert!(signals.has_wikidata && signals.qrank.is_none() && !signals.transit_stop);
    }
}
