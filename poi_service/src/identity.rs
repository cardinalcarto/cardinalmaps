//! Assigning POI identifiers, deterministically.
//!
//! Each POI version belongs to an identity, numbered sequentially from 1. A
//! POI version keeps its element's previous identity if [`same_place`] says
//! the previous POI version describes the same place, and otherwise starts a
//! new one. Versions that are deleted or aren't POIs don't end an identity:
//! if the element becomes a POI again, it's compared against its last POI
//! version.
//!
//! Every deployment that starts from the same planet and applies the same
//! daily diffs with the same [`RULESET_VERSION`](crate::rules::RULESET_VERSION)
//! assigns the same numbers, because new identities are numbered in
//! (type, id, version) order within each diff. The running [`Checksum`] lets
//! deployments confirm that.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::osm::{ElementKey, ElementVersion};
use crate::rules::{Tags, same_place};

/// What the database knows about an element that has been a POI.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracked {
    /// The element's latest version, which may not be a POI.
    pub version: i32,
    /// The identity of the element's last POI version.
    pub poi_id: i64,
    /// Whether the latest version is a POI.
    pub active: bool,
    /// Tags of the element's last POI version.
    pub tags: Tags,
}

/// One POI version and the identity it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionRow {
    pub key: ElementKey,
    pub version: i32,
    pub poi_id: i64,
}

#[derive(Debug, Default)]
pub struct Applied {
    /// New state of every element that changed, by key.
    pub updates: Vec<(ElementKey, Tracked)>,
    /// Every new POI version, in (type, id, version) order.
    pub versions: Vec<VersionRow>,
    pub new_identities: usize,
}

/// Applies one diff's element versions to the elements in `tracked`, numbering
/// new identities from `next_poi_id`.
///
/// `tracked` must hold every element in `changes` that the database knows.
/// Versions no newer than the tracked one are ignored, so a diff overlapping
/// the planet or a previous diff is harmless.
pub fn apply(tracked: &HashMap<ElementKey, Tracked>, mut changes: Vec<ElementVersion>, next_poi_id: i64) -> Applied {
    changes.sort_by_key(|c| (c.key, c.version));
    let mut applied = Applied::default();
    let mut next_poi_id = next_poi_id;

    for element in changes.chunk_by(|a, b| a.key == b.key) {
        let key = element[0].key;
        let mut state = tracked.get(&key).cloned();
        let mut changed = false;
        for change in element {
            if state.as_ref().is_some_and(|s| change.version <= s.version) {
                continue;
            }
            match (&change.poi_tags, &mut state) {
                (None, None) => continue,
                (None, Some(s)) => {
                    s.version = change.version;
                    s.active = false;
                }
                (Some(tags), _) => {
                    let poi_id = match &state {
                        Some(s) if same_place(&s.tags, tags) => s.poi_id,
                        _ => {
                            // Elements and their versions are visited in
                            // sorted order, so this numbers new identities
                            // in (type, id, version) order.
                            next_poi_id += 1;
                            applied.new_identities += 1;
                            next_poi_id - 1
                        }
                    };
                    state = Some(Tracked { version: change.version, poi_id, active: true, tags: tags.clone() });
                    applied.versions.push(VersionRow { key, version: change.version, poi_id });
                }
            }
            changed = true;
        }
        if changed && let Some(state) = state {
            applied.updates.push((key, state));
        }
    }
    applied
}

/// A running SHA-256 over every POI version row ever assigned:
/// `checksum_n = SHA-256(checksum_{n-1} || rows_n)`, where the bootstrap is
/// step 0 with an all-zero previous checksum and each row is the osm_type
/// (0 node, 1 way, 2 relation), osm_id, version and poi_id as big-endian
/// integers, in (type, id, version) order.
pub struct Checksum(Sha256);

impl Checksum {
    pub fn new(previous: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(previous);
        Checksum(hasher)
    }

    pub fn add(&mut self, row: &VersionRow) {
        self.0.update([row.key.osm_type as u8]);
        self.0.update(row.key.osm_id.to_be_bytes());
        self.0.update(row.version.to_be_bytes());
        self.0.update(row.poi_id.to_be_bytes());
    }

    pub fn finish(self) -> Vec<u8> {
        self.0.finalize().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm::OsmType;
    use crate::rules::tags;

    fn node(osm_id: i64) -> ElementKey {
        ElementKey { osm_type: OsmType::Node, osm_id }
    }

    fn cafe(name: &str) -> Option<Tags> {
        Some(tags([("amenity", "cafe"), ("name", name)]))
    }

    fn change(osm_id: i64, version: i32, poi_tags: Option<Tags>) -> ElementVersion {
        ElementVersion { key: node(osm_id), version, poi_tags }
    }

    #[test]
    fn numbers_new_identities_in_key_order() {
        let tracked = HashMap::from([(
            node(5),
            Tracked { version: 3, poi_id: 1, active: true, tags: cafe("Blue Bottle").unwrap() },
        )]);
        let changes = vec![
            change(9, 1, cafe("Sightglass")),
            change(5, 4, cafe("Blue Bottle Coffee")),
            change(5, 5, cafe("Ritual")),
            change(5, 2, cafe("Ignored, older than tracked")),
            change(7, 1, None),
        ];
        let applied = apply(&tracked, changes, 10);
        let rows: Vec<_> = applied.versions.iter().map(|r| (r.key.osm_id, r.version, r.poi_id)).collect();
        assert_eq!(rows, [(5, 4, 1), (5, 5, 10), (9, 1, 11)]);
        assert_eq!(applied.new_identities, 2);
        assert_eq!(applied.updates.len(), 2);
    }

    #[test]
    fn identity_survives_a_gap_without_poi_tags() {
        let tracked =
            HashMap::from([(node(1), Tracked { version: 1, poi_id: 4, active: true, tags: cafe("Ritual").unwrap() })]);
        let disused = change(1, 2, None);
        let reopened = change(1, 3, cafe("Ritual Coffee"));
        let replaced = change(1, 4, None);

        let applied = apply(&tracked, vec![disused.clone()], 10);
        assert_eq!(applied.updates[0].1, Tracked { version: 2, poi_id: 4, active: false, tags: cafe("Ritual").unwrap() });

        let applied = apply(&tracked, vec![disused, reopened, replaced], 10);
        assert_eq!(applied.versions, [VersionRow { key: node(1), version: 3, poi_id: 4 }]);
        assert!(!applied.updates[0].1.active);
    }

    #[test]
    fn numbering_depends_on_diff_boundaries() {
        let changes = vec![
            change(2, 1, cafe("A")),
            change(1, 1, cafe("B")),
            change(1, 2, cafe("C")),
            change(2, 2, cafe("A Cafe")),
        ];
        let together = apply(&HashMap::new(), changes.clone(), 1);

        // The same changes split into two diffs, applied one after the other.
        let first = apply(&HashMap::new(), changes[..2].to_vec(), 1);
        let tracked: HashMap<_, _> = first.updates.into_iter().collect();
        let second = apply(&tracked, changes[2..].to_vec(), 1 + first.new_identities as i64);
        let mut split = first.versions;
        split.extend(second.versions);
        split.sort_by_key(|r| (r.key, r.version));

        // The same versions share identities either way, but the numbers
        // differ; that's why every deployment must apply the same diffs.
        let ids = |rows: &[VersionRow]| rows.iter().map(|r| r.poi_id).collect::<Vec<_>>();
        assert_eq!(ids(&together.versions), [1, 2, 3, 3]);
        assert_eq!(ids(&split), [1, 3, 2, 2]);
    }
}
