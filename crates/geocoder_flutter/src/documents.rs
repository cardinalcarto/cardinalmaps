//! Tile features as airmail documents, the way the server's indexer makes them from OSM
//! elements (`airmail::osm::document` and `street_document`), with the areas they're in from
//! the Who's On First places of the regions ([`crate::wof::WofPlaces`]).
//!
//! A feature with a stable POI ID (the `poi` and `geocoder` layers' IDs) becomes a pointer
//! document: it keeps its names for ranking, and the rest is read from its tile for display.
//! Every other feature is stored whole, as on the server.

use airmail::importance;
pub use airmail::index::LOCAL_ID_TAG;
use airmail::poi::{Kind, OsmRef, OsmType, Pointer, TileId, ToIndexPoi};
use airmail_importance::osm as importance_osm;
use geo::{CoordsIter, Geometry};

use crate::extract::{ExtractedFeature, SourceLayer, TileKey};
use crate::wof::WofPlaces;

/// What tiles add to a feature that isn't one of its OSM tags (the app's `_notTags`, and
/// the layers' other OpenMapTiles attributes). Not stored or indexed.
const NOT_TAGS: [&str; 11] = [
    "class",
    "subclass",
    "rank",
    "agg_stop",
    "indoor",
    "layer",
    "level",
    "ref_length",
    "intermittent",
    "ele_ft",
    "housenumber",
];

/// The layers whose feature IDs are stable POI IDs.
fn has_poi_ids(layer: SourceLayer) -> bool {
    matches!(layer, SourceLayer::Poi | SourceLayer::Geocoder)
}

/// The OSM element of a feature of the other layers, from its ID: Planetiler's
/// `id * 10 + type` (1 node, 2 way, 3 relation).
#[must_use]
pub fn osm_ref(id: u64) -> Option<OsmRef> {
    let osm_type = match id % 10 {
        1 => OsmType::Node,
        2 => OsmType::Way,
        3 => OsmType::Relation,
        _ => return None,
    };
    Some(OsmRef {
        osm_type,
        id: i64::try_from(id / 10).ok()?,
        version: None,
    })
}

/// The OSM `highway` of a `transportation_name` piece's OpenMapTiles class, or `None` for
/// what isn't a street (ferries, railways, aerialways), as the server's indexer finds only
/// named highways as streets.
fn street_highway(class: Option<&str>, subclass: Option<&str>) -> Option<&'static str> {
    Some(match class? {
        "motorway" => "motorway",
        "trunk" => "trunk",
        "primary" => "primary",
        "secondary" => "secondary",
        "tertiary" => "tertiary",
        "minor" => "residential",
        "service" => "service",
        "track" => "track",
        "raceway" => "raceway",
        "busway" => "busway",
        "path" => match subclass {
            Some("footway") => "footway",
            Some("cycleway") => "cycleway",
            Some("bridleway") => "bridleway",
            Some("steps") => "steps",
            Some("pedestrian") => "pedestrian",
            _ => "path",
        },
        _ => return None,
    })
}

/// The OSM tag that a feature of the label layers stands for, which the tiles give only as
/// its OpenMapTiles class, so that it's found by category ("airport", "peak").
fn label_category(layer: SourceLayer, class: Option<&str>) -> Option<(&'static str, &'static str)> {
    Some(match (layer, class) {
        (SourceLayer::MountainPeak, Some("volcano")) => ("natural", "volcano"),
        (SourceLayer::MountainPeak, Some("saddle")) => ("natural", "saddle"),
        (SourceLayer::MountainPeak, Some("ridge")) => ("natural", "ridge"),
        (SourceLayer::MountainPeak, Some("cliff")) => ("natural", "cliff"),
        (SourceLayer::MountainPeak, _) => ("natural", "peak"),
        (SourceLayer::AerodromeLabel, _) => ("aeroway", "aerodrome"),
        (SourceLayer::WaterName, Some("lake")) => ("natural", "water"),
        (SourceLayer::WaterName, Some("bay")) => ("natural", "bay"),
        (SourceLayer::WaterName, Some("strait")) => ("natural", "strait"),
        (SourceLayer::Park, Some("park")) => ("leisure", "park"),
        (SourceLayer::Park, Some("nature_reserve")) => ("leisure", "nature_reserve"),
        (SourceLayer::Park, Some("national_park")) => ("boundary", "national_park"),
        (SourceLayer::Park, Some("protected_area")) => ("boundary", "protected_area"),
        _ => return None,
    })
}

/// A feature as a document, without its areas, or `None` if the server's indexer wouldn't
/// index it: an unnamed feature without a street address, a named point of
/// `transportation_name` (a junction), or a line that isn't a street.
#[must_use]
pub fn document(feature: ExtractedFeature) -> Option<ToIndexPoi> {
    let class = feature.tag("class").map(str::to_owned);
    let subclass = feature.tag("subclass").map(str::to_owned);
    let ExtractedFeature {
        layer,
        id,
        tiles,
        names,
        address,
        importance,
        tags,
        geometry,
        ..
    } = feature;
    let mut all: Vec<(String, String)> = names;
    let mut doc = if layer == SourceLayer::TransportationName {
        if matches!(geometry, Geometry::Point(_) | Geometry::MultiPoint(_)) || all.is_empty() {
            return None;
        }
        let highway = street_highway(class.as_deref(), subclass.as_deref())?;
        all.push(("highway".to_owned(), highway.to_owned()));
        let mut doc = airmail::osm::street_document(all, geometry)?;
        doc.importance = importance::street_importance(Some(highway));
        doc
    } else {
        all.extend(address);
        all.extend(
            tags.into_iter()
                .filter(|(k, _)| !NOT_TAGS.contains(&k.as_str())),
        );
        if let Some((key, value)) = label_category(layer, class.as_deref())
            && !all.iter().any(|(k, _)| k == key)
        {
            all.push((key.to_owned(), value.to_owned()));
        }
        let mut doc = airmail::osm::document(all, geometry)?;
        doc.importance = match (doc.kind, importance) {
            (Kind::Address, _) => importance::ADDRESS_IMPORTANCE,
            (_, Some(importance)) => importance,
            _ => {
                let signals = importance_osm::poi_signals(
                    doc.tags.iter().map(|(k, v)| (k.as_str(), v.as_str())),
                    airmail::geometry::area_m2(&doc.geometry),
                    None,
                );
                importance::poi_importance(signals)
            }
        };
        doc
    };
    match id {
        Some(poi_id) if has_poi_ids(layer) => {
            let TileKey { z, x, y } = tiles[0];
            doc.set_pointer(Pointer {
                poi_id,
                tile: TileId { z, x, y },
            });
        }
        Some(id) if !has_poi_ids(layer) => doc.osm = osm_ref(id),
        _ => {}
    }
    Some(doc)
}

/// Gives a document the areas its tile names (`cardinal:admin:*`), expanded through their
/// ancestors.
pub fn place(doc: &mut ToIndexPoi, tile_admins: &[(String, i64)], wof: &mut WofPlaces) {
    if tile_admins.is_empty() {
        return;
    }
    let areas = wof.areas(tile_admins);
    doc.admin_ids.clone_from(&areas.ids);
    doc.admin_names.clone_from(&areas.names);
    doc.country_code.clone_from(&areas.country_code);
    doc.languages.clone_from(&areas.street_languages);
}

/// Whether a document has an ID that finds it again (`poi:`, `wof:`, `osm:`, `postcode:`).
fn has_place_id(doc: &ToIndexPoi) -> bool {
    doc.poi_id().is_some()
        || doc.wof.is_some()
        || doc.osm.is_some()
        || doc.tag("cardinal:postcode_id").is_some()
}

/// The ID within the index of a document with no other ID (a street joined from tile
/// pieces, a POI without a stable or OSM ID): a hash of its kind, center and tags, so a
/// rebuild of the index from the same features gives it the same ID, and a different
/// document almost surely a different one. Never for one that has another ID.
#[must_use]
pub fn local_id(doc: &ToIndexPoi) -> u64 {
    // FNV-1a, which doesn't change between Rust versions as `DefaultHasher` may.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        // A separator, so ("ab", "c") isn't ("a", "bc").
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    };
    eat(doc.kind.as_str().as_bytes());
    // Its center and shape to about a centimetre, as results round them. (Two pieces of a
    // street can share a center.)
    let mut degrees = |c: geo::Coord<f64>| {
        for d in [c.x, c.y] {
            eat(&((d * 1e7).round() as i64).to_le_bytes());
        }
    };
    degrees(doc.center.0);
    for c in doc.geometry.coords_iter() {
        degrees(c);
    }
    let mut tags: Vec<&(String, String)> =
        doc.tags.iter().filter(|(k, _)| k != LOCAL_ID_TAG).collect();
    tags.sort();
    for (k, v) in tags {
        eat(k.as_bytes());
        eat(v.as_bytes());
    }
    // Positive as a signed 64-bit integer too.
    hash >> 1
}

/// Gives a document with no other ID its [`local_id`], as its [`LOCAL_ID_TAG`], so that a
/// result can be opened again from the index. Returns whether it did.
pub fn give_local_id(doc: &mut ToIndexPoi) -> bool {
    if has_place_id(doc) || doc.tag(LOCAL_ID_TAG).is_some() {
        return false;
    }
    let id = local_id(doc);
    doc.tags.push((LOCAL_ID_TAG.to_owned(), id.to_string()));
    true
}

/// The key of an address for the duplicate rule: its house number and street, folded.
#[must_use]
pub fn address_key(doc: &ToIndexPoi) -> Option<(String, String)> {
    let fold = |s: &str| airmail::index::name_key(s);
    Some((
        fold(doc.house_number.as_deref()?),
        fold(doc.road.as_deref()?),
    ))
}

/// Drops the addresses that duplicate a POI's address among one tile's documents: a POI
/// with its own address is also in the `housenumber` layer as that address.
pub fn drop_duplicate_addresses(docs: &mut Vec<ToIndexPoi>) -> usize {
    let pois: std::collections::HashSet<(String, String)> = docs
        .iter()
        .filter(|d| d.kind == Kind::Poi)
        .filter_map(address_key)
        .collect();
    if pois.is_empty() {
        return 0;
    }
    let before = docs.len();
    docs.retain(|d| d.kind != Kind::Address || address_key(d).is_none_or(|k| !pois.contains(&k)));
    before - docs.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{LineString, MultiLineString, Point};

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn feature(layer: SourceLayer, id: Option<u64>, names: &[(&str, &str)]) -> ExtractedFeature {
        ExtractedFeature {
            layer,
            id,
            tiles: vec![TileKey {
                z: 14,
                x: 2629,
                y: 5697,
            }],
            names: pairs(names),
            address: Vec::new(),
            admins: Vec::new(),
            importance: None,
            tags: Vec::new(),
            geometry: Geometry::Point(Point::new(-122.3, 47.6)),
        }
    }

    #[test]
    fn pois_with_ids_are_pointers() {
        let mut f = feature(SourceLayer::Poi, Some(42), &[("name", "Cafe Uno")]);
        f.tags = pairs(&[("amenity", "cafe"), ("class", "cafe"), ("rank", "3")]);
        f.importance = Some(0.25);
        let doc = document(f).unwrap();
        assert_eq!(doc.kind, Kind::Poi);
        assert_eq!(doc.poi_id(), Some(42));
        assert_eq!(doc.tile.map(|t| (t.x, t.y)), Some((2629, 5697)));
        assert_eq!(doc.tag("amenity"), Some("cafe"));
        assert_eq!(doc.tag("class"), None);
        assert!((doc.importance - 0.25).abs() < 1e-9);
        assert!(doc.osm.is_none());
    }

    #[test]
    fn other_layers_have_osm_elements() {
        let mut f = feature(SourceLayer::Housenumber, Some(2_566_440_962), &[]);
        f.address = pairs(&[
            ("addr:housenumber", "897"),
            ("addr:street", "Hatchery Road"),
        ]);
        f.tags = pairs(&[("housenumber", "897")]);
        let doc = document(f).unwrap();
        assert_eq!(doc.kind, Kind::Address);
        assert!(doc.tile.is_none());
        assert_eq!(
            doc.osm.map(|o| (o.osm_type, o.id)),
            Some((OsmType::Way, 256_644_096))
        );
        assert_eq!(doc.tag("housenumber"), None);
        assert!((doc.importance - importance::ADDRESS_IMPORTANCE).abs() < 1e-9);
        // Unnamed and without an address: nothing.
        assert!(document(feature(SourceLayer::Housenumber, None, &[])).is_none());
    }

    #[test]
    fn streets_but_not_junctions_or_ferries() {
        let line = Geometry::MultiLineString(MultiLineString(vec![LineString::from(vec![
            (-122.3, 47.6),
            (-122.29, 47.6),
        ])]));
        let mut street = feature(
            SourceLayer::TransportationName,
            Some(72),
            &[("name", "Main Street")],
        );
        street.tags = pairs(&[("class", "minor")]);
        street.geometry = line.clone();
        let doc = document(street.clone()).unwrap();
        assert_eq!(doc.kind, Kind::Street);
        assert_eq!(doc.road.as_deref(), Some("Main Street"));
        assert_eq!(doc.tag("highway"), Some("residential"));
        let mut ferry = street.clone();
        ferry.tags = pairs(&[("class", "ferry")]);
        assert!(document(ferry).is_none());
        let mut junction = street;
        junction.geometry = Geometry::Point(Point::new(-122.3, 47.6));
        assert!(document(junction).is_none());
    }

    #[test]
    fn label_layers_get_a_category() {
        let mut f = feature(
            SourceLayer::AerodromeLabel,
            Some(3_686_696_122),
            &[("name", "Sekiu Airport")],
        );
        f.tags = pairs(&[("class", "other"), ("iata", "SEK")]);
        let doc = document(f).unwrap();
        assert_eq!(doc.tag("aeroway"), Some("aerodrome"));
        assert_eq!(doc.tag("iata"), Some("SEK"));
    }

    #[test]
    fn only_documents_without_other_ids_get_local_ids() {
        let mut pointer = document(feature(SourceLayer::Poi, Some(42), &[("name", "A")])).unwrap();
        assert!(!give_local_id(&mut pointer));
        let mut osm = document(feature(SourceLayer::Park, Some(72), &[("name", "P")])).unwrap();
        assert!(!give_local_id(&mut osm));
        assert_eq!(osm.tag(LOCAL_ID_TAG), None);

        let cafe = || document(feature(SourceLayer::Poi, None, &[("name", "Cafe Uno")])).unwrap();
        let mut a = cafe();
        assert!(give_local_id(&mut a));
        assert!(!give_local_id(&mut a), "once");
        let id = a.tag(LOCAL_ID_TAG).unwrap().to_owned();
        assert_eq!(id, local_id(&cafe()).to_string());
        assert!(id.parse::<i64>().is_ok());
        // Built again from the same feature: the same ID. Anything different: another.
        let mut b = cafe();
        give_local_id(&mut b);
        assert_eq!(b.tag(LOCAL_ID_TAG), Some(id.as_str()));
        let mut moved = feature(SourceLayer::Poi, None, &[("name", "Cafe Uno")]);
        moved.geometry = Geometry::Point(Point::new(-122.3, 47.600_001));
        assert_ne!(local_id(&document(moved).unwrap()).to_string(), id);
        let renamed = document(feature(SourceLayer::Poi, None, &[("name", "Cafe Dos")])).unwrap();
        assert_ne!(local_id(&renamed).to_string(), id);
    }

    #[test]
    fn a_pois_own_address_isnt_indexed_again() {
        let mut poi = feature(SourceLayer::Poi, Some(1), &[("name", "Cafe Uno")]);
        poi.address = pairs(&[("addr:housenumber", "12"), ("addr:street", "Main Street")]);
        let address = |number: &str, street: &str| {
            let mut a = feature(SourceLayer::Housenumber, None, &[]);
            a.address = pairs(&[("addr:housenumber", number), ("addr:street", street)]);
            document(a).unwrap()
        };
        let mut docs = vec![
            address("12", "main street"),
            document(poi).unwrap(),
            address("14", "Main Street"),
        ];
        assert_eq!(drop_duplicate_addresses(&mut docs), 1);
        assert_eq!(
            docs.iter()
                .map(|d| (d.kind, d.house_number.clone()))
                .collect::<Vec<_>>(),
            vec![
                (Kind::Poi, Some("12".into())),
                (Kind::Address, Some("14".into()))
            ]
        );
    }
}
