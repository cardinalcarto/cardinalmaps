//! Reading an OSM element's tags for its shape and importance.

use crate::importance::{PoiSignals, is_transit_stop_tag};
use crate::qrank::QRank;

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
            "natural" => linear |= matches!(value, "coastline" | "cliff" | "ridge" | "arete" | "tree_row"),
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
    fn reads_wikidata_and_transit_stops() {
        let qrank = QRank::parse("Entity,QRank\nQ60,1000\n".as_bytes()).unwrap();
        let signals = poi_signals([("name", "X"), ("wikidata", "Q60"), ("highway", "bus_stop")], 5.0, Some(&qrank));
        assert!(signals.has_wikidata && signals.transit_stop);
        assert_eq!((signals.qrank, signals.area_m2), (Some(1000), 5.0));
        let signals = poi_signals([("wikidata", "Q60")], 0.0, None);
        assert!(signals.has_wikidata && signals.qrank.is_none() && !signals.transit_stop);
    }
}
