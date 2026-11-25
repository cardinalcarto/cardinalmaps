//! OSM features as documents, from their tags and shapes: the indexer's
//! elements, or a phone's tile features, which carry the same tags.

use geo::Geometry;

use crate::poi::{Kind, ToIndexPoi};

pub use airmail_importance::osm::{NAME_KEYS, is_name_key};

/// The values of name tags, split on `;`.
fn name_values(tags: &[(String, String)]) -> impl Iterator<Item = &str> {
    tags.iter()
        .filter(|(k, _)| is_name_key(k))
        .flat_map(|(_, v)| v.split(';').map(str::trim).filter(|n| !n.is_empty()))
}

/// A named feature (a POI) or an unnamed address, or `None` if it's neither
/// or has no usable shape. Its house number, road, unit and postcode come
/// from its `addr:*` tags. Areas, admins, importance and the rest are left
/// for the caller.
#[must_use]
pub fn document(tags: Vec<(String, String)>, geometry: Geometry<f64>) -> Option<ToIndexPoi> {
    let tag = |key: &str| tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let house_number = tag("addr:housenumber");
    let road = tag("addr:street");

    let mut names = Vec::new();
    for name in name_values(&tags) {
        names.push(name.to_string());
        // TODO: Remove once we get stemmers again.
        if name.contains("'s") {
            names.push(name.replace("'s", ""));
            names.push(name.replace("'s", "s"));
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
    let unit = tag("addr:unit");
    let postcode = tag("addr:postcode");
    let mut indexed = ToIndexPoi::new(kind, names, geometry, tags)?;
    indexed.house_number = house_number;
    indexed.road = road;
    indexed.unit = unit;
    indexed.postcode = postcode;
    Some(indexed)
}

/// A named street from the tags of its biggest way (`highway` and names) and
/// its ways' lines, found by its names, and by them as a road (`w 26th st`
/// for West 26th Street), or `None` if it has no usable shape.
#[must_use]
pub fn street_document(tags: Vec<(String, String)>, geometry: Geometry<f64>) -> Option<ToIndexPoi> {
    let mut names: Vec<String> = name_values(&tags).map(str::to_string).collect();
    names.sort();
    names.dedup();
    let road = tags
        .iter()
        .find(|(k, _)| k == "name")
        .map(|(_, v)| v.clone());
    let mut poi = ToIndexPoi::new(Kind::Street, names, geometry, tags)?;
    poi.road = road;
    Some(poi)
}

#[cfg(test)]
mod test {
    use super::*;
    use geo::Point;

    fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn names_but_not_name_metadata() {
        let poi = document(
            tags(&[
                ("amenity", "university"),
                ("name", "University of Washington"),
                ("short_name", "UW"),
                ("name:etymology:wikidata", "Q1223"),
            ]),
            Geometry::Point(Point::new(-122.3, 47.65)),
        )
        .unwrap();
        assert_eq!(poi.names, vec!["UW", "University of Washington"]);
        assert_eq!(poi.kind, Kind::Poi);
    }

    #[test]
    fn unnamed_addresses_are_addresses() {
        let at = Geometry::Point(Point::new(-122.34, 47.61));
        let poi = document(
            tags(&[
                ("addr:housenumber", "1530"),
                ("addr:street", "Post Alley"),
                ("addr:postcode", "98101"),
            ]),
            at.clone(),
        )
        .unwrap();
        assert_eq!(poi.kind, Kind::Address);
        assert_eq!(
            (
                poi.house_number.as_deref(),
                poi.road.as_deref(),
                poi.postcode.as_deref()
            ),
            (Some("1530"), Some("Post Alley"), Some("98101"))
        );
        assert!(document(tags(&[("addr:housenumber", "1530")]), at).is_none());
    }
}
