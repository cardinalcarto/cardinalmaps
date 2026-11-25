use std::{
    collections::{HashMap, HashSet},
    hash::{BuildHasher, RandomState},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::cache::{IndexerCache, WofCacheItem};
use crate::multipolygon::{self, Role};
use airmail::poi::{OsmRef, OsmType, ToIndexPoi};
use anyhow::{Result, bail};
use clap::ValueEnum;
use crossbeam::channel::Sender;
use log::{info, warn};
use osmpbf::{BlobDecode, BlobReader, Element, ElementReader, RelMemberType};

use crate::osm::{OsmPoi, Street, StreetWay};

/// Streets' ways as they're read, merged by name and place.
struct Streets {
    shards: Vec<Mutex<HashMap<(String, u64), Street>>>,
    hasher: RandomState,
}

impl Streets {
    const SHARDS: usize = 64;

    fn new() -> Self {
        Self {
            shards: (0..Self::SHARDS).map(|_| Mutex::default()).collect(),
            hasher: RandomState::new(),
        }
    }

    fn add(&self, way: StreetWay) {
        let shard = self.hasher.hash_one(&way.key) as usize % Self::SHARDS;
        let mut streets = self.shards[shard].lock().expect("streets");
        match streets.get_mut(&way.key) {
            Some(street) => street.add(way),
            None => {
                streets.insert(way.key.clone(), Street::from(way));
            }
        }
    }

    fn into_streets(self) -> impl Iterator<Item = Street> {
        self.shards
            .into_iter()
            .flat_map(|shard| shard.into_inner().expect("streets").into_values())
    }
}

/// What the first pass finds: the ways multipolygons need, and the street
/// names of houses in associated street relations.
#[derive(Default)]
struct Scan {
    needed_ways: HashSet<i64>,
    /// By member type (0 for nodes, 1 for ways) and ID.
    house_streets: HashMap<(u8, i64), Arc<str>>,
}

impl Scan {
    fn merge(mut self, other: Scan) -> Scan {
        self.needed_ways.extend(other.needed_ways);
        self.house_streets.extend(other.house_streets);
        self
    }

    /// Tags with the street from an associated street relation, if the
    /// element is a house in one and has no `addr:street`.
    fn with_street<'a>(
        &'a self,
        mut tags: HashMap<&'a str, &'a str>,
        member: (u8, i64),
    ) -> HashMap<&'a str, &'a str> {
        if tags.contains_key("addr:housenumber")
            && !tags.contains_key("addr:street")
            && let Some(street) = self.house_streets.get(&member)
        {
            tags.insert("addr:street", street);
        }
        tags
    }
}

/// Whether a relation groups a street's houses, as in France: its `house`
/// (or `address`) members have house numbers but not `addr:street`.
fn is_associated_street(tags: &HashMap<&str, &str>) -> bool {
    matches!(tags.get("type"), Some(&("associatedStreet" | "street"))) && tags.contains_key("name")
}

/// What `osmium add-locations-to-ways` marks a file with: each way carries
/// its nodes' locations.
const LOCATIONS_ON_WAYS: &str = "LocationsOnWays";

/// Fails unless the file has node locations on its ways, which the loader
/// needs, saying how to add them.
pub fn require_locations_on_ways(path: &Path) -> Result<()> {
    let has = match BlobReader::from_path(path)?.next().transpose()? {
        Some(blob) => match blob.decode()? {
            BlobDecode::OsmHeader(header) => header
                .optional_features()
                .iter()
                .chain(header.required_features())
                .any(|f| f == LOCATIONS_ON_WAYS),
            _ => false,
        },
        None => false,
    };
    if !has {
        bail!(
            "{} has no node locations on its ways; add them with `osmium add-locations-to-ways --keep-member-nodes`",
            path.display()
        );
    }
    Ok(())
}

/// An OpenStreetMap PBF file loader, for a file with node locations on its
/// ways (see `require_locations_on_ways`).
///
/// Extracts points of interest from nodes, ways and multipolygon relations.
/// The first pass notes the ways that interesting relations need, whose
/// coordinates the second pass caches, so relations can be assembled without
/// holding every way in memory.
pub struct OsmPbf {
    pbf_path: PathBuf,
    ignore: Vec<ParseOsmTypes>,
    sender: Sender<ToIndexPoi>,
    indexer_cache: Arc<IndexerCache>,
}

fn version(version: Option<i32>) -> Option<u32> {
    version
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0)
}

/// Whether a relation might be indexed: a named multipolygon or one with an
/// address. Its tags are checked again once it's assembled.
fn wants_relation(tags: &HashMap<&str, &str>) -> bool {
    tags.get("type") == Some(&"multipolygon")
        && (tags.contains_key("name")
            || (tags.contains_key("addr:housenumber") && tags.contains_key("addr:street")))
}

impl OsmPbf {
    pub fn new(
        osm_pbf_path: &Path,
        ignore: Vec<ParseOsmTypes>,
        sender: Sender<ToIndexPoi>,
        indexer_cache: Arc<IndexerCache>,
    ) -> Self {
        Self {
            pbf_path: osm_pbf_path.to_path_buf(),
            ignore,
            sender,
            indexer_cache,
        }
    }

    fn send(&self, poi: Option<OsmPoi>, counter: &AtomicUsize) -> u64 {
        match poi.and_then(OsmPoi::index_poi) {
            Some(poi) => {
                counter.fetch_add(1, Ordering::Relaxed);
                self.sender.send(poi).expect("sender failed");
                1
            }
            None => 0,
        }
    }

    pub fn parse_osm(self) -> Result<()> {
        require_locations_on_ways(&self.pbf_path)?;
        let relations = !self.ignore.contains(&ParseOsmTypes::Relations);
        info!(
            "Finding the ways multipolygons need in {}",
            self.pbf_path.display()
        );
        let scan = self.scan(relations)?;
        let needed_ways = &scan.needed_ways;
        info!(
            "Multipolygon relations need {} ways; associated streets have {} houses",
            needed_ways.len(),
            scan.house_streets.len()
        );
        let streets = Streets::new();

        let count_ways = AtomicUsize::new(0);
        let count_nodes = AtomicUsize::new(0);
        let count_relations = AtomicUsize::new(0);

        info!("Parsing POIs");
        let pois = ElementReader::from_path(&self.pbf_path)?.par_map_reduce(
            |element| match element {
                Element::DenseNode(dn) => {
                    if self.ignore.contains(&ParseOsmTypes::DenseNodes) {
                        return 0;
                    }
                    let osm = OsmRef {
                        osm_type: OsmType::Node,
                        id: dn.id(),
                        version: version(dn.info().map(|i| i.version())),
                    };
                    let tags = scan.with_street(dn.tags().collect::<HashMap<_, _>>(), (0, dn.id()));
                    self.send(
                        OsmPoi::new_from_node(tags, (dn.lat(), dn.lon()), osm),
                        &count_nodes,
                    )
                }
                Element::Node(node) => {
                    if self.ignore.contains(&ParseOsmTypes::Nodes) {
                        return 0;
                    }
                    let osm = OsmRef {
                        osm_type: OsmType::Node,
                        id: node.id(),
                        version: version(node.info().version()),
                    };
                    let tags =
                        scan.with_street(node.tags().collect::<HashMap<_, _>>(), (0, node.id()));
                    self.send(
                        OsmPoi::new_from_node(tags, (node.lat(), node.lon()), osm),
                        &count_nodes,
                    )
                }
                Element::Way(way) => {
                    let needed = needed_ways.contains(&way.id());
                    if self.ignore.contains(&ParseOsmTypes::Ways) && !needed {
                        return 0;
                    }
                    let points: Vec<(f64, f64)> =
                        way.node_locations().map(|n| (n.lat(), n.lon())).collect();
                    if points.is_empty() {
                        return 0;
                    }
                    if needed {
                        let coords = points.iter().map(|&(lat, lon)| (lon, lat)).collect();
                        if let Err(e) = self
                            .indexer_cache
                            .buffered_write_item(WofCacheItem::WayCoords(way.id(), coords))
                        {
                            warn!("Error caching way {}: {e}", way.id());
                        }
                    }
                    if self.ignore.contains(&ParseOsmTypes::Ways) {
                        return 0;
                    }
                    let osm = OsmRef {
                        osm_type: OsmType::Way,
                        id: way.id(),
                        version: version(way.info().version()),
                    };
                    let tags =
                        scan.with_street(way.tags().collect::<HashMap<_, _>>(), (1, way.id()));
                    if let Some(street) = StreetWay::new(&tags, &points, osm) {
                        streets.add(street);
                        return 0;
                    }
                    self.send(OsmPoi::new_from_way(tags, &points, osm), &count_ways)
                }
                Element::Relation(_) => 0,
            },
            || 0_u64,
            |a, b| a + b,
        )?;
        self.indexer_cache.flush()?;

        let count_streets = AtomicUsize::new(0);
        for street in streets.into_streets() {
            if let Some(poi) = street.index_poi() {
                count_streets.fetch_add(1, Ordering::Relaxed);
                self.sender.send(poi).expect("sender failed");
            }
        }
        info!(
            "Merged named roads into {} streets",
            count_streets.load(Ordering::Relaxed)
        );

        let pois = if relations {
            info!("Assembling multipolygons");
            let relation_pois = ElementReader::from_path(&self.pbf_path)?.par_map_reduce(
                |element| {
                    let Element::Relation(relation) = element else {
                        return 0;
                    };
                    let tags = relation.tags().collect::<HashMap<_, _>>();
                    if !wants_relation(&tags) {
                        return 0;
                    }
                    let mut members = Vec::new();
                    for member in relation.members() {
                        if member.member_type != RelMemberType::Way {
                            continue;
                        }
                        let Some(role) = member.role().ok().and_then(Role::parse) else {
                            continue;
                        };
                        match self.indexer_cache.query_way_coords(member.member_id) {
                            Ok(Some(coords)) => members.push((role, coords)),
                            // Members outside an extract are missing.
                            Ok(None) => {}
                            Err(e) => warn!("Error reading way {}: {e}", member.member_id),
                        }
                    }
                    let Some(shape) = multipolygon::assemble(members) else {
                        return 0;
                    };
                    let osm = OsmRef {
                        osm_type: OsmType::Relation,
                        id: relation.id(),
                        version: version(relation.info().version()),
                    };
                    self.send(
                        OsmPoi::new_from_relation(tags, shape, osm),
                        &count_relations,
                    )
                },
                || 0_u64,
                |a, b| a + b,
            )?;
            pois + relation_pois
        } else {
            pois
        };

        let count_ways = count_ways.load(Ordering::Relaxed);
        let count_nodes = count_nodes.load(Ordering::Relaxed);
        let count_relations = count_relations.load(Ordering::Relaxed);
        info!(
            "Loaded {pois} interesting pois: {count_nodes} nodes, {count_ways} ways and {count_relations} relations"
        );
        if count_ways == 0 {
            warn!(
                "No ways found in OSM PBF file. Ensure your pbf file has locations present, see Google: LocationsOnWays"
            );
        }
        Ok(())
    }

    /// Finds the IDs of ways that interesting multipolygon relations have as
    /// members, and the houses of associated street relations.
    fn scan(&self, relations: bool) -> Result<Scan> {
        let needed = ElementReader::from_path(&self.pbf_path)?.par_map_reduce(
            |element| {
                let mut scan = Scan::default();
                match element {
                    Element::Relation(relation) => {
                        let tags = relation.tags().collect::<HashMap<_, _>>();
                        if relations && wants_relation(&tags) {
                            scan.needed_ways = relation
                                .members()
                                .filter(|m| m.member_type == RelMemberType::Way)
                                .map(|m| m.member_id)
                                .collect();
                        } else if is_associated_street(&tags) {
                            let street: Arc<str> = Arc::from(tags["name"]);
                            for member in relation.members() {
                                let house = matches!(member.role(), Ok("house" | "address"));
                                let member_type = match member.member_type {
                                    RelMemberType::Node => 0,
                                    RelMemberType::Way => 1,
                                    RelMemberType::Relation => continue,
                                };
                                if house {
                                    scan.house_streets
                                        .insert((member_type, member.member_id), street.clone());
                                }
                            }
                        }
                    }
                    _ => {}
                }
                scan
            },
            Scan::default,
            Scan::merge,
        )?;
        Ok(needed)
    }
}

#[derive(Debug, Eq, PartialEq, Clone, ValueEnum)]
pub enum ParseOsmTypes {
    Ways,
    Nodes,
    DenseNodes,
    Relations,
}
