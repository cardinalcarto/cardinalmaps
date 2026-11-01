"""Extract every POI node and way from a planet file into a Parquet table with
one location per POI (way centroids are the mean of their node locations).

This runs in two steps so the planet only needs a node-location index for the
few nodes that POI ways reference:

1. Write POIs plus the nodes their ways reference to a small .osm.pbf.
2. Read that file with locations and write the table.

    stable-ids-extract --planet data/planet-261005.osm.pbf \
        --pbf-out data/pois-261005.osm.pbf --out data/pois-261005.parquet
"""

import argparse
import json
from pathlib import Path

import osmium
import pyarrow as pa
import pyarrow.parquet as pq

from stable_ids.poi_tags import POI_FILTER_KEYS, is_poi

POIS_SCHEMA = pa.schema([
    ("type", pa.string()),
    ("id", pa.int64()),
    ("version", pa.int32()),
    ("lat", pa.float64()),
    ("lon", pa.float64()),
    ("tags", pa.large_string()),
])

BATCH_ROWS = 200_000


def poi_processor(path: Path) -> osmium.FileProcessor:
    return (osmium.FileProcessor(str(path), osmium.osm.NODE | osmium.osm.WAY)
            .with_filter(osmium.filter.KeyFilter(*POI_FILTER_KEYS)))


def filter_planet(planet: Path, pbf_out: Path) -> None:
    count = 0
    partial = pbf_out.with_name(pbf_out.name.removesuffix(".osm.pbf") + ".partial.osm.pbf")
    with osmium.BackReferenceWriter(str(partial), ref_src=str(planet), overwrite=True) as writer:
        for obj in poi_processor(planet):
            if is_poi(obj.tags):
                writer.add(obj)
                count += 1
                if count % 1_000_000 == 0:
                    print(f"  {count} POIs")
        print(f"{count} POIs; adding referenced nodes")
    partial.rename(pbf_out)


def write_table(pbf: Path, out: Path) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    rows = {name: [] for name in POIS_SCHEMA.names}
    count = 0
    with pq.ParquetWriter(out, POIS_SCHEMA) as writer:
        def flush():
            writer.write_table(pa.table(rows, schema=POIS_SCHEMA))
            for column in rows.values():
                column.clear()

        for obj in poi_processor(pbf).with_locations():
            tags = dict(obj.tags)
            if not is_poi(tags):
                continue
            if obj.is_node():
                if not obj.location.valid():
                    continue
                lat, lon = obj.location.lat, obj.location.lon
            else:
                locs = [n.location for n in obj.nodes if n.location.valid()]
                if len(locs) > 1 and obj.is_closed():
                    locs = locs[:-1]
                if not locs:
                    continue
                lat = sum(l.lat for l in locs) / len(locs)
                lon = sum(l.lon for l in locs) / len(locs)
            rows["type"].append("n" if obj.is_node() else "w")
            rows["id"].append(obj.id)
            rows["version"].append(obj.version)
            rows["lat"].append(lat)
            rows["lon"].append(lon)
            rows["tags"].append(json.dumps(tags, ensure_ascii=False))
            count += 1
            if len(rows["id"]) >= BATCH_ROWS:
                flush()
        flush()
    print(f"wrote {count} POIs to {out}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--planet", type=Path, required=True)
    parser.add_argument("--pbf-out", type=Path, required=True,
                        help="intermediate POI-only .osm.pbf (reused if it exists)")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    if not args.pbf_out.exists():
        filter_planet(args.planet, args.pbf_out)
    write_table(args.pbf_out, args.out)


if __name__ == "__main__":
    main()
