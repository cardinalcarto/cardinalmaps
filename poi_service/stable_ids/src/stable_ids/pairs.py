"""Build labelled POI pairs for measuring the same/different rules.

Every pair is one POI's tags in the planet against its tags after the
following diffs, for each POI whose `tags_to_text` changed: an OSM element that
changed between two extracts, which is the comparison the POI service makes.

Pairs are labelled by `same_place_label`:

- "same" (label 1): the name is unchanged, respelled, or switched to one of
  the place's other names (e.g. a translation), and the brand didn't change.
- "different" (label 0): the name changed entirely, which is how mappers
  record a new business reusing the old element, or the name changed and the
  brand switched to another one (e.g. "OMV Šmarje-Sap" to "MOL Šmarje-Sap").

    stable-ids-pairs --pois data/pois-261005.parquet \
        --changes data/changes.parquet --out data/pairs.parquet
"""

import argparse
import json
from pathlib import Path

import numpy as np
import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.parquet as pq

from stable_ids.poi_tags import changed_top_level_keys, same_place_label, tags_to_text

PAIRS_SCHEMA = pa.schema([
    ("text_a", pa.string()),
    ("text_b", pa.string()),
    ("label", pa.int8()),
    ("class_changed", pa.bool_()),
    ("changed_top_level_keys", pa.list_(pa.string())),
    ("osm_type", pa.string()),
    ("osm_id", pa.int64()),
    ("old_version", pa.int32()),
    ("new_version", pa.int32()),
])


def element_keys(table: pa.Table) -> np.ndarray:
    is_way = pc.equal(table["type"], "w").to_numpy(zero_copy_only=False)
    return table["id"].to_numpy() * 2 + is_way


class Pois:
    def __init__(self, path: Path):
        table = pq.read_table(path)
        self.keys = element_keys(table)
        self.versions = table["version"].to_numpy()
        self._tags = table["tags"].combine_chunks()

    def tags(self, i: int) -> dict[str, str]:
        return json.loads(self._tags[i].as_py())

    def lookup(self, keys: np.ndarray) -> np.ndarray:
        """Indices of `keys` in this table, or -1 where absent."""
        order = np.argsort(self.keys)
        sorted_keys = self.keys[order]
        pos = np.searchsorted(sorted_keys, keys).clip(max=len(order) - 1)
        return np.where(sorted_keys[pos] == keys, order[pos], -1)


def build_pairs(pois: Pois, changes: pa.Table) -> dict[str, list]:
    rows = {name: [] for name in PAIRS_SCHEMA.names}
    matches = pois.lookup(element_keys(changes))
    change_versions = changes["version"].to_numpy()
    change_types = changes["type"].to_pylist()
    change_ids = changes["id"].to_numpy()
    change_tags = changes["tags"].combine_chunks()
    present = matches >= 0
    newer = present & (change_versions > pois.versions[np.where(present, matches, 0)])
    candidates = np.flatnonzero(newer)
    print(f"{len(changes)} changed POIs, {len(candidates)} present and older in the planet")

    for c in candidates:
        i = int(matches[c])
        old, new = pois.tags(i), json.loads(change_tags[c].as_py())
        a, b = tags_to_text(old), tags_to_text(new)
        if a == b:
            continue
        rows["text_a"].append(a)
        rows["text_b"].append(b)
        rows["label"].append(int(same_place_label(old, new)))
        changed_keys = changed_top_level_keys(old, new)
        rows["class_changed"].append(bool(changed_keys))
        rows["changed_top_level_keys"].append(changed_keys)
        rows["osm_type"].append(change_types[c])
        rows["osm_id"].append(int(change_ids[c]))
        rows["old_version"].append(int(pois.versions[i]))
        rows["new_version"].append(int(change_versions[c]))
    labels = np.array(rows["label"])
    print(f"{len(labels)} pairs: {labels.sum()} same, {len(labels) - labels.sum()} different")
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--pois", type=Path, required=True)
    parser.add_argument("--changes", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    print(f"loading {args.pois}")
    pois = Pois(args.pois)
    changes = pq.read_table(args.changes)
    rows = build_pairs(pois, changes)
    table = pa.table(rows, schema=PAIRS_SCHEMA)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(table, args.out)
    print(f"wrote {len(table)} pairs to {args.out}")


if __name__ == "__main__":
    main()
