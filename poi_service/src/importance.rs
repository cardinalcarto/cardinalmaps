//! How prominent each POI is, for `patch` to tag it with as
//! `cardinal:importance`, computed with airmail's own code
//! (`airmail_importance`), so the tag is what airmail's indexer gives the same
//! element: from its Wikidata entity and QRank, the area of its shape as
//! airmail stores it, and whether it's a transit stop.
//!
//! Shapes need node locations, so this reads a copy of the extract with them
//! on its ways (`osmium add-locations-to-ways`), like airmail's indexer. A
//! way's shape is a polygon if it's closed and its tags say it's an area. A
//! multipolygon relation's is assembled from its member ways, as the indexer
//! does; other relations, like points and lines, have no area. Every element
//! that's a POI by [`is_poi`] gets an importance, whether or not airmail would
//! index it.

use std::collections::HashMap;
use std::path::Path;

use airmail_importance::geo::{Coord, Geometry, LineString, Polygon};
use airmail_importance::multipolygon::{self, Role};
use airmail_importance::qrank::QRank;
use airmail_importance::{importance, osm, shape};
use anyhow::{Context, Result, bail};
use osmpbf::{BlobDecode, BlobReader, Element, PrimitiveBlock, RelMemberType};
use rayon::prelude::*;

use crate::osm::OsmType;
use crate::rules::is_poi;

pub const IMPORTANCE_TAG: &str = "cardinal:importance";

/// What `osmium add-locations-to-ways` marks a file with.
const LOCATIONS_ON_WAYS: &str = "LocationsOnWays";

/// Blobs decoded in parallel at a time.
const BLOBS_PER_CHUNK: usize = 256;

/// A way's node locations in nanodegrees as `(lat, lon)`, divided by 100
/// (OSM's precision) to halve what multipolygons' member ways take.
type Locations = Vec<(i32, i32)>;

/// Every POI's importance, sorted for lookup.
#[derive(Default)]
pub struct Importances {
    /// (osm_id, version, importance) for nodes, ways and relations.
    by_type: [Vec<(i64, i32, f64)>; 3],
}

impl Importances {
    /// Computes the importance of every POI in `path`, which must have node
    /// locations on its ways.
    pub fn compute(path: &Path, qrank: Option<&QRank>) -> Result<Importances> {
        require_locations_on_ways(path)?;
        tracing::info!("finding the ways multipolygon POIs need in {}", path.display());
        let mut needed: Vec<i64> = map_blocks(path, |_, block| Ok(needed_ways(block)))?.into_iter().flatten().collect();
        needed.sort_unstable();
        needed.dedup();
        tracing::info!("multipolygon POIs need {} ways; reading nodes and ways", needed.len());

        let mut ids = Importances::default();
        let mut ways = HashMap::new();
        let mut relation_blobs = Vec::new();
        // Blobs are numbered as `map_blocks` numbers them, header included.
        for (i, (pois, needed_ways, has_relations)) in
            map_blocks(path, |i, block| Ok((i, nodes_and_ways(block, &needed, qrank)?)))?
        {
            ids.extend(pois);
            ways.extend(needed_ways);
            if has_relations {
                relation_blobs.push(i);
            }
        }
        drop(needed);
        tracing::info!("assembling multipolygons from {} ways", ways.len());
        let relations = map_blocks(path, |i, block| {
            if relation_blobs.binary_search(&i).is_err() {
                return Ok(Vec::new());
            }
            relations(block, &ways, qrank)
        })?;
        for pois in relations {
            ids.extend(pois);
        }
        ids.finish();
        Ok(ids)
    }

    #[cfg(test)]
    pub fn from_entries(pois: Vec<(OsmType, i64, i32, f64)>) -> Importances {
        let mut importances = Importances::default();
        importances.extend(pois);
        importances.finish();
        importances
    }

    fn extend(&mut self, pois: Vec<(OsmType, i64, i32, f64)>) {
        for (osm_type, id, version, importance) in pois {
            self.by_type[osm_type as usize].push((id, version, importance));
        }
    }

    fn finish(&mut self) {
        for entries in &mut self.by_type {
            entries.sort_unstable_by_key(|&(id, version, _)| (id, version));
        }
    }

    pub fn len(&self) -> usize {
        self.by_type.iter().map(Vec::len).sum()
    }

    /// The importance of a POI version.
    pub fn get(&self, osm_type: OsmType, osm_id: i64, version: i32) -> Option<f64> {
        let entries = &self.by_type[osm_type as usize];
        let i = entries.binary_search_by_key(&(osm_id, version), |&(id, v, _)| (id, v)).ok()?;
        Some(entries[i].2)
    }
}

/// An importance as a tag value: four decimal places (at least three
/// significant digits, as POIs' importance is at least 0.04), without
/// trailing zeros.
pub fn format(importance: f64) -> String {
    let s = format!("{importance:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Fails unless the file has node locations on its ways.
fn require_locations_on_ways(path: &Path) -> Result<()> {
    let mut reader = BlobReader::from_path(path).with_context(|| format!("opening {}", path.display()))?;
    let header = reader.next().context("empty file")??.to_headerblock()?;
    if !header.optional_features().iter().chain(header.required_features()).any(|f| f == LOCATIONS_ON_WAYS) {
        bail!(
            "{} has no node locations on its ways; add them with `osmium add-locations-to-ways --keep-member-nodes`",
            path.display()
        );
    }
    Ok(())
}

/// Applies `f` to every data block of the file with its blob's index, in
/// parallel, returning the results in file order.
fn map_blocks<T: Send>(path: &Path, f: impl Fn(usize, &PrimitiveBlock) -> Result<T> + Sync) -> Result<Vec<T>> {
    let mut blobs = BlobReader::from_path(path)?.enumerate();
    let mut results = Vec::new();
    loop {
        let chunk = blobs.by_ref().take(BLOBS_PER_CHUNK).map(|(i, b)| Ok((i, b?))).collect::<Result<Vec<_>>>()?;
        if chunk.is_empty() {
            return Ok(results);
        }
        let mapped = chunk
            .par_iter()
            .filter_map(|(i, blob)| match blob.decode() {
                Ok(BlobDecode::OsmData(block)) => Some(f(*i, &block)),
                Ok(_) => None,
                Err(e) => Some(Err(e.into())),
            })
            .collect::<Result<Vec<_>>>()?;
        results.extend(mapped);
    }
}

fn version(osm_type: OsmType, id: i64, version: Option<i32>) -> Result<i32> {
    match version.filter(|&v| v > 0) {
        Some(version) => Ok(version),
        None => bail!("{} {id} has no version; the input must include element metadata", osm_type.as_str()),
    }
}

fn is_multipolygon<'a>(mut tags: impl Iterator<Item = (&'a str, &'a str)>) -> bool {
    tags.any(|tag| tag == ("type", "multipolygon"))
}

/// The member ways of the block's multipolygon POIs.
fn needed_ways(block: &PrimitiveBlock) -> Vec<i64> {
    let mut needed = Vec::new();
    for group in block.groups() {
        for relation in group.relations() {
            if is_multipolygon(relation.tags()) && is_poi(relation.tags()) {
                needed.extend(relation.members().filter(|m| m.member_type == RelMemberType::Way).map(|m| m.member_id));
            }
        }
    }
    needed
}

fn importance<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>, area_m2: f64, qrank: Option<&QRank>) -> f64 {
    importance::poi_importance(osm::poi_signals(tags, area_m2, qrank))
}

/// A `(lon, lat)` point from nanodegrees, as osmpbf (and so airmail) reads
/// it.
fn coord(nano_lat: i64, nano_lon: i64) -> Coord<f64> {
    Coord { x: 1e-9 * nano_lon as f64, y: 1e-9 * nano_lat as f64 }
}

/// The importance of the block's POI nodes and ways, the locations of the
/// ways in `needed`, and whether the block has relations.
#[allow(clippy::type_complexity)]
fn nodes_and_ways(
    block: &PrimitiveBlock,
    needed: &[i64],
    qrank: Option<&QRank>,
) -> Result<(Vec<(OsmType, i64, i32, f64)>, Vec<(i64, Locations)>, bool)> {
    let (mut pois, mut ways, mut has_relations) = (Vec::new(), Vec::new(), false);
    for element in block.elements() {
        match element {
            Element::DenseNode(node) if is_poi(node.tags()) => {
                let version = version(OsmType::Node, node.id(), node.info().map(|i| i.version()))?;
                pois.push((OsmType::Node, node.id(), version, importance(node.tags(), 0.0, qrank)));
            }
            Element::Node(node) if is_poi(node.tags()) => {
                let version = version(OsmType::Node, node.id(), node.info().version())?;
                pois.push((OsmType::Node, node.id(), version, importance(node.tags(), 0.0, qrank)));
            }
            Element::Way(way) => {
                if needed.binary_search(&way.id()).is_ok() {
                    let locations = way
                        .node_locations()
                        .map(|l| Ok((decimicro(l.nano_lat())?, decimicro(l.nano_lon())?)))
                        .collect::<Result<_>>()?;
                    ways.push((way.id(), locations));
                }
                if !is_poi(way.tags()) {
                    continue;
                }
                let version = version(OsmType::Way, way.id(), way.info().version())?;
                // As airmail's indexer reads a way: an area if it's closed
                // and its tags don't say it's a line, else a line or point.
                let line: LineString<f64> = way.node_locations().map(|l| coord(l.nano_lat(), l.nano_lon())).collect();
                let area = if line.0.len() >= 4 && line.is_closed() && osm::is_area(way.tags()) {
                    shape::stored_area_m2(Geometry::Polygon(Polygon::new(line, vec![])))
                } else {
                    0.0
                };
                pois.push((OsmType::Way, way.id(), version, importance(way.tags(), area, qrank)));
            }
            Element::Relation(_) => has_relations = true,
            _ => {}
        }
    }
    Ok((pois, ways, has_relations))
}

fn decimicro(nano: i64) -> Result<i32> {
    if nano % 100 != 0 {
        bail!("a node location ({nano} nanodegrees) is finer than OSM's precision");
    }
    Ok(i32::try_from(nano / 100)?)
}

/// The importance of the block's POI relations, assembling multipolygons
/// from their member ways' locations.
fn relations(
    block: &PrimitiveBlock,
    ways: &HashMap<i64, Locations>,
    qrank: Option<&QRank>,
) -> Result<Vec<(OsmType, i64, i32, f64)>> {
    let mut pois = Vec::new();
    for group in block.groups() {
        for relation in group.relations() {
            if !is_poi(relation.tags()) {
                continue;
            }
            let version = version(OsmType::Relation, relation.id(), relation.info().version())?;
            let area = if is_multipolygon(relation.tags()) {
                let members = relation
                    .members()
                    .filter(|m| m.member_type == RelMemberType::Way)
                    .filter_map(|m| {
                        let role = m.role().ok().and_then(Role::parse)?;
                        // Members outside an extract are missing.
                        let locations = ways.get(&m.member_id)?;
                        let points = locations
                            .iter()
                            .map(|&(lat, lon)| coord(i64::from(lat) * 100, i64::from(lon) * 100))
                            .map(|c| (c.x, c.y))
                            .collect();
                        Some((role, points))
                    })
                    .collect();
                multipolygon::assemble(members)
                    .map_or(0.0, |shape| shape::stored_area_m2(Geometry::MultiPolygon(shape)))
            } else {
                0.0
            };
            pois.push((OsmType::Relation, relation.id(), version, importance(relation.tags(), area, qrank)));
        }
    }
    Ok(pois)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbf::{
        BlobHeader, DenseInfo, DenseNodes, Info, PrimitiveBlock as RawBlock, PrimitiveGroup, RawBlob, Relation,
        StringTable, Way,
    };
    use airmail_importance::geo::MultiPolygon;
    use airmail_importance::importance::PoiSignals;
    use prost::Message;

    #[derive(Clone, PartialEq, Message)]
    struct HeaderBlock {
        #[prost(string, repeated, tag = "4")]
        required_features: Vec<String>,
        #[prost(string, repeated, tag = "5")]
        optional_features: Vec<String>,
    }

    fn blob(kind: &str, contents: &[u8]) -> RawBlob {
        let header = BlobHeader { r#type: kind.to_string(), indexdata: None, datasize: 0 };
        RawBlob { header, header_bytes: Vec::new(), data: Vec::new() }.with_contents(contents).unwrap()
    }

    /// Writes a file with one header and one data block.
    fn write_file(path: &Path, features: &[&str], block: &RawBlock) {
        let header = HeaderBlock {
            required_features: vec!["OsmSchema-V0.6".into(), "DenseNodes".into()],
            optional_features: features.iter().map(|f| f.to_string()).collect(),
        };
        let mut file = std::fs::File::create(path).unwrap();
        blob("OSMHeader", &header.encode_to_vec()).write(&mut file).unwrap();
        blob("OSMData", &block.encode_to_vec()).write(&mut file).unwrap();
    }

    fn info(version: i32) -> Option<Info> {
        Some(Info { version: Some(version), ..Default::default() })
    }

    /// A location in units of the default granularity, 100 nanodegrees.
    fn units(lat: f64, lon: f64) -> (i64, i64) {
        ((lat * 1e7).round() as i64, (lon * 1e7).round() as i64)
    }

    /// The polygon through `(lat, lon)` points as read from a file.
    fn polygon(points: &[(f64, f64)]) -> Polygon<f64> {
        let ring = points.iter().map(|&(lat, lon)| {
            let (lat, lon) = units(lat, lon);
            coord(lat * 100, lon * 100)
        });
        Polygon::new(ring.collect(), vec![])
    }

    /// A way through `(lat, lon)` points in degrees, with locations on it.
    fn way(id: i64, keys: Vec<u32>, vals: Vec<u32>, points: &[(f64, f64)]) -> Way {
        let units: Vec<(i64, i64)> = points.iter().map(|&(lat, lon)| units(lat, lon)).collect();
        let deltas = |f: fn(&(i64, i64)) -> i64| {
            let mut last = 0;
            let deltas = units.iter().map(|p| {
                let delta = f(p) - last;
                last = f(p);
                delta
            });
            deltas.collect::<Vec<_>>()
        };
        Way { id, keys, vals, info: info(1), refs: vec![1; points.len()], lat: deltas(|p| p.0), lon: deltas(|p| p.1) }
    }

    #[test]
    fn computes_importance_from_shapes() {
        let strings = [
            "",
            "name",
            "Cafe",
            "amenity",
            "cafe",
            "wikidata",
            "Q60",
            "leisure",
            "park",
            "Park",
            "type",
            "multipolygon",
            "outer",
            "barrier",
            "fence",
            "highway",
            "bus_stop",
        ];
        let square = |lat: f64, lon: f64, size: f64| {
            vec![(lat, lon), (lat, lon + size), (lat + size, lon + size), (lat + size, lon), (lat, lon)]
        };
        let block = RawBlock {
            stringtable: StringTable { s: strings.iter().map(|s| s.as_bytes().to_vec()).collect() },
            primitivegroup: vec![
                PrimitiveGroup {
                    dense: Some(DenseNodes {
                        // A cafe with a Wikidata entity and a bus stop.
                        id: vec![1, 1],
                        denseinfo: Some(DenseInfo {
                            version: vec![1, 4],
                            timestamp: vec![0, 0],
                            changeset: vec![0, 0],
                            uid: vec![0, 0],
                            user_sid: vec![0, 0],
                            visible: vec![],
                        }),
                        lat: vec![476_000_000, 0],
                        lon: vec![-1_223_000_000, 0],
                        keys_vals: vec![1, 2, 3, 4, 5, 6, 0, 1, 2, 15, 16, 0],
                    }),
                    ..Default::default()
                },
                PrimitiveGroup {
                    ways: vec![
                        way(10, vec![1, 7], vec![9, 8], &square(47.6, -122.3, 0.001)),
                        // A fence around something isn't an area.
                        way(11, vec![1, 13], vec![9, 14], &square(47.6, -122.3, 0.001)),
                        // A multipolygon's two halves.
                        way(12, vec![], vec![], &[(47.0, -122.0), (47.0, -121.99), (47.01, -121.99)]),
                        way(13, vec![], vec![], &[(47.01, -121.99), (47.01, -122.0), (47.0, -122.0)]),
                    ],
                    ..Default::default()
                },
                PrimitiveGroup {
                    relations: vec![Relation {
                        id: 20,
                        keys: vec![1, 7, 10],
                        vals: vec![9, 8, 11],
                        info: info(3),
                        roles_sid: vec![12, 12],
                        memids: vec![12, 1],
                        types: vec![1, 1],
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extract.osm.pbf");
        write_file(&path, &[], &block);
        assert!(Importances::compute(&path, None).is_err(), "needs locations on ways");
        write_file(&path, &["LocationsOnWays"], &block);
        let qrank = QRank::parse("Entity,QRank\nQ60,1000000\n".as_bytes()).unwrap();
        let importances = Importances::compute(&path, Some(&qrank)).unwrap();
        assert_eq!(importances.len(), 5);

        let expected = |signals: PoiSignals| Some(importance::poi_importance(signals));
        let cafe = PoiSignals { has_wikidata: true, qrank: Some(1_000_000), ..Default::default() };
        assert_eq!(importances.get(OsmType::Node, 1, 1), expected(cafe));
        assert_eq!(importances.get(OsmType::Node, 1, 2), None, "other versions have none");
        let stop = PoiSignals { transit_stop: true, ..Default::default() };
        assert_eq!(importances.get(OsmType::Node, 2, 4), expected(stop));
        let park = shape::stored_area_m2(Geometry::Polygon(polygon(&square(47.6, -122.3, 0.001))));
        assert!((park - 8_300.0).abs() < 200.0, "{park}");
        assert_eq!(importances.get(OsmType::Way, 10, 1), expected(PoiSignals { area_m2: park, ..Default::default() }));
        assert_eq!(importances.get(OsmType::Way, 11, 1), expected(PoiSignals::default()));
        // The ring starts where the first member does.
        let ring = [(47.0, -122.0), (47.0, -121.99), (47.01, -121.99), (47.01, -122.0), (47.0, -122.0)];
        let big_park = shape::stored_area_m2(Geometry::MultiPolygon(MultiPolygon(vec![polygon(&ring)])));
        assert!(big_park > 100.0 * park);
        let big_park = expected(PoiSignals { area_m2: big_park, ..Default::default() });
        assert_eq!(importances.get(OsmType::Relation, 20, 3), big_park);
    }

    #[test]
    fn formats_a_few_digits() {
        assert_eq!(format(0.08), "0.08");
        assert_eq!(format(0.123456), "0.1235");
        assert_eq!(format(0.04), "0.04");
        assert_eq!(format(1.0), "1");
    }

    #[test]
    fn locations_round_trip_exactly() {
        let nano = -122_345_678_900_i64;
        assert_eq!(coord(i64::from(decimicro(nano).unwrap()) * 100, 0).y, coord(nano, 0).y);
        assert!(decimicro(1).is_err());
    }
}
