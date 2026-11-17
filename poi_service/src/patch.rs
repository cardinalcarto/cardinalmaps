//! Adding POI identifiers to an `.osm.pbf` file as `cardinal:poi_id` tags,
//! with `cardinal:version` tags giving the element's OSM version, and given
//! their [`Importances`], every POI's importance as `cardinal:importance`.
//!
//! Every known (type, id, version) is loaded into memory first, from
//! PostgreSQL or from published releases, so neither is involved while the
//! file is rewritten; both give the same identifiers, so the same output. An element gets the tags if its
//! exact version is in `poi_versions`. Blocks without any are copied
//! unchanged; others are decoded, retagged and recompressed. Existing
//! `cardinal:poi_id`, `cardinal:version` and `cardinal:importance` tags are
//! replaced, so a file can be patched again. A POI gets `cardinal:importance`
//! whether or not its version has an identifier.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use futures_util::TryStreamExt;
use prost::Message;
use rayon::prelude::*;
use sqlx::PgPool;

use crate::db::Checkpoint;
use crate::identity::VersionRow;
use crate::importance::{IMPORTANCE_TAG, Importances};
use crate::osm::OsmType;
use crate::pbf::{DenseNodes, PrimitiveBlock, RawBlob};
use crate::rules::{is_poi, is_poi_key, is_poi_value_key};

pub const POI_ID_TAG: &str = "cardinal:poi_id";
/// The element's OSM version. Each poi_id belongs to a single element, so this
/// tells consumers whether a POI's data is fresher than what they cached.
pub const VERSION_TAG: &str = "cardinal:version";

/// Blobs processed in parallel at a time.
const BLOBS_PER_CHUNK: usize = 256;

/// Every known POI version's identifier, sorted for lookup.
#[derive(Default)]
pub struct PoiIds {
    /// (osm_id, version, poi_id) for nodes, ways and relations.
    by_type: [Vec<(i64, i32, i64)>; 3],
}

impl PoiIds {
    pub async fn load(pool: &PgPool) -> Result<PoiIds> {
        let mut by_type: [Vec<(i64, i32, i64)>; 3] = Default::default();
        let mut rows =
            sqlx::query_as::<_, (OsmType, i64, i32, i64)>("SELECT osm_type, osm_id, version, poi_id FROM poi_versions")
                .fetch(pool);
        while let Some((osm_type, osm_id, version, poi_id)) = rows.try_next().await? {
            by_type[osm_type as usize].push((osm_id, version, poi_id));
        }
        let mut ids = PoiIds { by_type };
        ids.finish();
        Ok(ids)
    }

    pub fn push(&mut self, row: &VersionRow) {
        self.by_type[row.key.osm_type as usize].push((row.key.osm_id, row.version, row.poi_id));
    }

    /// Sorts the identifiers pushed, for lookup.
    pub fn finish(&mut self) {
        for entries in &mut self.by_type {
            entries.sort_unstable();
        }
    }

    pub fn len(&self) -> usize {
        self.by_type.iter().map(Vec::len).sum()
    }

    #[cfg(test)]
    pub fn get_for_test(&self, osm_type: OsmType, osm_id: i64, version: i32) -> Option<i64> {
        self.get(osm_type, osm_id, version)
    }

    fn get(&self, osm_type: OsmType, osm_id: i64, version: i32) -> Option<i64> {
        let entries = &self.by_type[osm_type as usize];
        let i = entries.binary_search_by_key(&(osm_id, version), |&(id, v, _)| (id, v)).ok()?;
        Some(entries[i].2)
    }
}

#[derive(Debug, Default)]
struct Stats {
    rewritten_blobs: u64,
    /// Elements tagged, by type.
    tagged: [u64; 3],
    /// POIs whose version has no known identifier, by type.
    unknown: [u64; 3],
    /// Elements tagged with their importance, by type.
    importance: [u64; 3],
}

impl Stats {
    fn add(&mut self, other: &Stats) {
        self.rewritten_blobs += other.rewritten_blobs;
        for i in 0..3 {
            self.tagged[i] += other.tagged[i];
            self.unknown[i] += other.unknown[i];
            self.importance[i] += other.importance[i];
        }
    }
}

/// Patches each `(input, output)` file with identifiers known as of
/// `checkpoint` and, if given, importances. The files are copies of the same
/// extract, such as one with node locations on its ways and one without.
pub async fn patch(
    ids: PoiIds,
    checkpoint: Option<Checkpoint>,
    importances: Option<Importances>,
    files: Vec<(PathBuf, PathBuf)>,
) -> Result<()> {
    if let (Some(checkpoint), Some((input, _))) = (checkpoint, files.first()) {
        let input_timestamp = crate::bootstrap::replication_timestamp(input)?;
        tracing::info!("identifiers are at {}; input is at {input_timestamp}", checkpoint.timestamp);
        if input_timestamp > checkpoint.timestamp {
            tracing::warn!("the input is newer than the identifiers, so its newest POI versions won't get them");
        }
    }
    tracing::info!("loaded {} POI versions", ids.len());
    let importances = importances.unwrap_or_default();
    for (input, output) in files {
        let (ids, importances) = (&ids, &importances);
        let stats = tokio::task::block_in_place(|| patch_file(ids, importances, &input, &output))?;
        tracing::info!("patched {}", input.display());
        let types = ["nodes", "ways", "relations"];
        for (i, name) in types.iter().enumerate() {
            tracing::info!(
                "{name}: {} tagged, {} POIs without a known identifier, {} with an importance",
                stats.tagged[i],
                stats.unknown[i],
                stats.importance[i]
            );
        }
        tracing::info!("rewrote {} blobs", stats.rewritten_blobs);
    }
    Ok(())
}

fn patch_file(ids: &PoiIds, importances: &Importances, input: &Path, output: &Path) -> Result<Stats> {
    let mut reader =
        BufReader::with_capacity(1 << 20, File::open(input).with_context(|| format!("opening {}", input.display()))?);
    let partial = output.with_extension("partial");
    let mut writer = BufWriter::with_capacity(1 << 20, File::create(&partial)?);
    let mut stats = Stats::default();
    loop {
        let mut chunk = Vec::with_capacity(BLOBS_PER_CHUNK);
        while chunk.len() < BLOBS_PER_CHUNK
            && let Some(blob) = RawBlob::read(&mut reader)?
        {
            chunk.push(blob);
        }
        if chunk.is_empty() {
            break;
        }
        let patched = chunk.par_iter().map(|blob| patch_blob(blob, ids, importances)).collect::<Result<Vec<_>>>()?;
        for (blob, (patched, blob_stats)) in chunk.iter().zip(patched) {
            stats.add(&blob_stats);
            patched.as_ref().unwrap_or(blob).write(&mut writer)?;
        }
    }
    writer.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    std::fs::rename(&partial, output)?;
    Ok(stats)
}

fn patch_blob(blob: &RawBlob, ids: &PoiIds, importances: &Importances) -> Result<(Option<RawBlob>, Stats)> {
    if blob.header.r#type != "OSMData" {
        return Ok((None, Stats::default()));
    }
    let mut block = PrimitiveBlock::decode(&blob.decompress()?[..]).context("decoding a block")?;
    let mut stats = Stats::default();
    if !patch_block(&mut block, ids, importances, &mut stats)? {
        return Ok((None, stats));
    }
    stats.rewritten_blobs = 1;
    Ok((Some(blob.with_contents(&block.encode_to_vec())?), stats))
}

/// What's known about a POI version.
#[derive(Clone, Copy)]
struct PoiVersion {
    poi_id: Option<i64>,
    version: i32,
    importance: Option<f64>,
}

/// What a string in a block's string table means as a tag key.
#[derive(Clone, Copy, PartialEq)]
enum KeyKind {
    Other,
    /// A key that can make an element a POI, so its tags need checking.
    Candidate,
    /// A tag this module adds, which is removed before retagging.
    Added,
}

/// Retags the elements of one block.
struct Retagger<'a> {
    ids: &'a PoiIds,
    importances: &'a Importances,
    stats: &'a mut Stats,
    /// The block's string table, what each string means as a key, and (once
    /// needed) each string's index.
    strings: &'a [Vec<u8>],
    kinds: Vec<KeyKind>,
    indices: Option<HashMap<&'a [u8], u32>>,
    /// Strings to append to the string table, and their indices.
    new_strings: Vec<Vec<u8>>,
    new_indices: HashMap<Vec<u8>, u32>,
}

impl Retagger<'_> {
    fn kind(&self, sid: u32) -> KeyKind {
        self.kinds.get(sid as usize).copied().unwrap_or(KeyKind::Other)
    }

    /// True if the tags given by key and value string indices are a POI's.
    /// Only elements with a candidate key need checking.
    fn is_poi(&self, tags: impl Iterator<Item = (u32, u32)> + Clone) -> bool {
        if !tags.clone().any(|(k, _)| self.kind(k) == KeyKind::Candidate) {
            return false;
        }
        let string = |i: u32| self.strings.get(i as usize).and_then(|s| std::str::from_utf8(s).ok()).unwrap_or("");
        is_poi(tags.map(|(k, v)| (string(k), string(v))))
    }

    /// The index of `s` in the string table, adding it if it's not there.
    fn string_index(&mut self, s: &[u8]) -> u32 {
        let strings = self.strings;
        let indices = self.indices.get_or_insert_with(|| {
            // Index 0 is reserved as a delimiter.
            strings.iter().enumerate().skip(1).map(|(i, s)| (&s[..], i as u32)).rev().collect()
        });
        if let Some(&i) = indices.get(s).or_else(|| self.new_indices.get(s)) {
            return i;
        }
        let i = (self.kinds.len() + self.new_strings.len()) as u32;
        self.new_strings.push(s.to_vec());
        self.new_indices.insert(s.to_vec(), i);
        i
    }

    /// The (key, value) string indices of the tags for a POI version.
    fn tags(&mut self, poi: PoiVersion) -> Vec<(u32, u32)> {
        let mut tags = Vec::with_capacity(3);
        if let Some(poi_id) = poi.poi_id {
            tags.push((self.string_index(POI_ID_TAG.as_bytes()), self.string_index(poi_id.to_string().as_bytes())));
            tags.push((
                self.string_index(VERSION_TAG.as_bytes()),
                self.string_index(poi.version.to_string().as_bytes()),
            ));
        }
        if let Some(importance) = poi.importance {
            let value = crate::importance::format(importance);
            tags.push((self.string_index(IMPORTANCE_TAG.as_bytes()), self.string_index(value.as_bytes())));
        }
        tags
    }

    /// What's known about an element, if it's a POI version with an
    /// identifier or importance.
    fn poi(&mut self, osm_type: OsmType, id: i64, version: Option<i32>, is_poi: bool) -> Result<Option<PoiVersion>> {
        if !is_poi {
            return Ok(None);
        }
        let Some(version) = version.filter(|&v| v > 0) else {
            bail!("{} {id} has no version; the input must include element metadata", osm_type.as_str());
        };
        let poi_id = self.ids.get(osm_type, id, version);
        match poi_id {
            Some(_) => self.stats.tagged[osm_type as usize] += 1,
            None => self.stats.unknown[osm_type as usize] += 1,
        }
        let importance = self.importances.get(osm_type, id, version);
        if importance.is_some() {
            self.stats.importance[osm_type as usize] += 1;
        }
        Ok((poi_id.is_some() || importance.is_some()).then_some(PoiVersion { poi_id, version, importance }))
    }

    /// Retags an element with parallel key and value lists, returning whether
    /// they changed.
    fn retag(
        &mut self,
        osm_type: OsmType,
        id: i64,
        version: Option<i32>,
        keys: &mut Vec<u32>,
        vals: &mut Vec<u32>,
    ) -> Result<bool> {
        let had_tag = keys.iter().any(|&k| self.kind(k) == KeyKind::Added);
        if had_tag {
            (*keys, *vals) = keys.iter().zip(vals.iter()).filter(|&(&k, _)| self.kind(k) != KeyKind::Added).unzip();
        }
        let is_poi = self.is_poi(keys.iter().copied().zip(vals.iter().copied()));
        let poi = self.poi(osm_type, id, version, is_poi)?;
        if let Some(poi) = poi {
            for (k, v) in self.tags(poi) {
                keys.push(k);
                vals.push(v);
            }
        }
        Ok(had_tag || poi.is_some())
    }

    /// Retags dense nodes, whose tags are `((key value)* 0)*` over all nodes.
    fn retag_dense(&mut self, dense: &mut DenseNodes) -> Result<bool> {
        if dense.keys_vals.is_empty() {
            return Ok(false);
        }
        let mut keys_vals = Vec::with_capacity(dense.keys_vals.len() + 16);
        let (mut pos, mut id, mut changed) = (0, 0i64, false);
        for (i, delta) in dense.id.iter().enumerate() {
            id += delta;
            let start = keys_vals.len();
            while let Some(&k) = dense.keys_vals.get(pos)
                && k != 0
            {
                let v = *dense.keys_vals.get(pos + 1).context("odd dense keys_vals")?;
                pos += 2;
                if self.kind(k as u32) == KeyKind::Added {
                    changed = true;
                    continue;
                }
                keys_vals.extend([k, v]);
            }
            pos += 1;
            let version = dense.denseinfo.as_ref().and_then(|info| info.version.get(i).copied());
            let tags = keys_vals[start..].as_chunks::<2>().0.iter().map(|&[k, v]| (k as u32, v as u32));
            let is_poi = self.is_poi(tags);
            if let Some(poi) = self.poi(OsmType::Node, id, version, is_poi)? {
                for (k, v) in self.tags(poi) {
                    keys_vals.extend([k as i32, v as i32]);
                }
                changed = true;
            }
            keys_vals.push(0);
        }
        if changed {
            dense.keys_vals = keys_vals;
        }
        Ok(changed)
    }
}

/// Tags the block's POIs with their identifiers, returning whether anything
/// changed.
fn patch_block(block: &mut PrimitiveBlock, ids: &PoiIds, importances: &Importances, stats: &mut Stats) -> Result<bool> {
    let PrimitiveBlock { stringtable, primitivegroup, .. } = block;
    let kinds = stringtable
        .s
        .iter()
        .map(|s| match std::str::from_utf8(s) {
            Ok(POI_ID_TAG | VERSION_TAG | IMPORTANCE_TAG) => KeyKind::Added,
            Ok(key) if is_poi_key(key) || is_poi_value_key(key) => KeyKind::Candidate,
            _ => KeyKind::Other,
        })
        .collect();
    let mut retagger = Retagger {
        ids,
        importances,
        stats,
        strings: &stringtable.s,
        kinds,
        indices: None,
        new_strings: Vec::new(),
        new_indices: HashMap::new(),
    };
    let mut changed = false;
    for group in primitivegroup {
        for n in &mut group.nodes {
            let version = n.info.as_ref().and_then(|i| i.version);
            changed |= retagger.retag(OsmType::Node, n.id, version, &mut n.keys, &mut n.vals)?;
        }
        for w in &mut group.ways {
            let version = w.info.as_ref().and_then(|i| i.version);
            changed |= retagger.retag(OsmType::Way, w.id, version, &mut w.keys, &mut w.vals)?;
        }
        for r in &mut group.relations {
            let version = r.info.as_ref().and_then(|i| i.version);
            changed |= retagger.retag(OsmType::Relation, r.id, version, &mut r.keys, &mut r.vals)?;
        }
        if let Some(dense) = &mut group.dense {
            changed |= retagger.retag_dense(dense)?;
        }
    }
    let new_strings = retagger.new_strings;
    stringtable.s.extend(new_strings);
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbf::{DenseInfo, Info, PrimitiveGroup, StringTable, Way};

    fn strings(table: &[&str]) -> StringTable {
        StringTable { s: table.iter().map(|s| s.as_bytes().to_vec()).collect() }
    }

    #[test]
    fn tags_known_pois_and_replaces_old_tags() {
        let mut block = PrimitiveBlock {
            stringtable: strings(&["", "name", "X", "shop", "bakery", POI_ID_TAG, "7", VERSION_TAG, "9"]),
            primitivegroup: vec![PrimitiveGroup {
                dense: Some(DenseNodes {
                    id: vec![1, 1, 1], // Nodes 1, 2 and 3.
                    denseinfo: Some(DenseInfo { version: vec![3, 1, 1], ..Default::default() }),
                    lat: vec![0, 0, 0],
                    lon: vec![0, 0, 0],
                    // Node 1 is a POI, node 2 has stale tags and no longer
                    // is, and node 3 is a POI of an unknown version.
                    keys_vals: vec![1, 2, 3, 4, 0, 1, 2, 5, 6, 7, 8, 0, 1, 2, 3, 4, 0],
                }),
                ways: vec![Way {
                    id: 10,
                    keys: vec![1, 3],
                    vals: vec![2, 4],
                    info: Some(Info { version: Some(2), ..Default::default() }),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let ids = PoiIds { by_type: [vec![(1, 3, 100)], vec![(10, 2, 200)], vec![]] };
        let mut stats = Stats::default();
        assert!(patch_block(&mut block, &ids, &Importances::default(), &mut stats).unwrap());

        let table: Vec<_> = block.stringtable.s.iter().map(|s| String::from_utf8_lossy(s).into_owned()).collect();
        let expected = ["", "name", "X", "shop", "bakery", POI_ID_TAG, "7", VERSION_TAG, "9", "200", "2", "100", "3"];
        assert_eq!(table, expected);
        let group = &block.primitivegroup[0];
        assert_eq!(group.dense.as_ref().unwrap().keys_vals, [1, 2, 3, 4, 5, 11, 7, 12, 0, 1, 2, 0, 1, 2, 3, 4, 0]);
        assert_eq!((&group.ways[0].keys[..], &group.ways[0].vals[..]), (&[1, 3, 5, 7][..], &[2, 4, 9, 10][..]));
        assert_eq!((stats.tagged, stats.unknown), ([1, 1, 0], [1, 0, 0]));

        // Patching again changes nothing but reports the same tags.
        let before = block.clone();
        patch_block(&mut block, &ids, &Importances::default(), &mut Stats::default()).unwrap();
        assert_eq!(block, before);
    }

    #[test]
    fn tags_importance_with_or_without_an_identifier() {
        let mut block = PrimitiveBlock {
            stringtable: strings(&["", "name", "X", "shop", "bakery", IMPORTANCE_TAG, "0.5"]),
            primitivegroup: vec![PrimitiveGroup {
                ways: (1..=3)
                    .map(|id| Way {
                        id,
                        keys: if id == 3 { vec![1, 3, 5] } else { vec![1, 3] },
                        vals: if id == 3 { vec![2, 4, 6] } else { vec![2, 4] },
                        info: Some(Info { version: Some(1), ..Default::default() }),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        };
        // Way 1 has an identifier and an importance, way 2 only an importance,
        // and way 3 a stale importance tag and none now.
        let ids = PoiIds { by_type: [vec![], vec![(1, 1, 100)], vec![]] };
        let importances = Importances::from_entries(vec![(OsmType::Way, 1, 1, 0.123456), (OsmType::Way, 2, 1, 0.08)]);
        let mut stats = Stats::default();
        assert!(patch_block(&mut block, &ids, &importances, &mut stats).unwrap());
        let tags = |way: &Way| -> Vec<(String, String)> {
            let s = |i: u32| String::from_utf8(block.stringtable.s[i as usize].clone()).unwrap();
            way.keys.iter().zip(&way.vals).map(|(&k, &v)| (s(k), s(v))).collect()
        };
        let ways = &block.primitivegroup[0].ways;
        let tag = |k: &str, v: &str| (k.to_string(), v.to_string());
        let base = vec![tag("name", "X"), tag("shop", "bakery")];
        let with = |extra: &[(String, String)]| [base.clone(), extra.to_vec()].concat();
        assert_eq!(
            tags(&ways[0]),
            with(&[tag(POI_ID_TAG, "100"), tag(VERSION_TAG, "1"), tag(IMPORTANCE_TAG, "0.1235")])
        );
        assert_eq!(tags(&ways[1]), with(&[tag(IMPORTANCE_TAG, "0.08")]));
        assert_eq!(tags(&ways[2]), base);
        assert_eq!((stats.tagged, stats.unknown, stats.importance), ([0, 1, 0], [0, 2, 0], [0, 2, 0]));

        let before = block.clone();
        patch_block(&mut block, &ids, &importances, &mut Stats::default()).unwrap();
        assert_eq!(block, before);
    }

    #[test]
    fn tags_pois_identified_by_key_and_value() {
        let mut block = PrimitiveBlock {
            stringtable: strings(&["", "name", "X", "highway", "bus_stop", "residential"]),
            primitivegroup: vec![PrimitiveGroup {
                ways: [(1, 4), (2, 5)]
                    .map(|(id, value)| Way {
                        id,
                        keys: vec![1, 3],
                        vals: vec![2, value],
                        info: Some(Info { version: Some(1), ..Default::default() }),
                        ..Default::default()
                    })
                    .to_vec(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let ids = PoiIds { by_type: [vec![], vec![(1, 1, 100), (2, 1, 200)], vec![]] };
        let mut stats = Stats::default();
        patch_block(&mut block, &ids, &Importances::default(), &mut stats).unwrap();
        // Only the bus stop is a POI, so the residential road isn't tagged
        // even though (contrived here) its version has an identifier.
        let ways = &block.primitivegroup[0].ways;
        assert_eq!((ways[0].keys.len(), ways[1].keys.len()), (4, 2));
        assert_eq!(stats.tagged, [0, 1, 0]);
    }
}
