"""Download the daily OSM diffs that follow a planet snapshot, and collect the
latest version of every POI they touch.

    stable-ids-diffs --planet data/planet-261005.osm.pbf --days 21 \
        --diff-dir data/diffs --out data/changes.parquet
"""

import argparse
import datetime as dt
import json
import time
import urllib.request
from pathlib import Path

import osmium
import pyarrow as pa
import pyarrow.parquet as pq
from osmium.replication.server import ReplicationServer

from stable_ids.poi_tags import POI_FILTER_KEYS, is_poi

DAY_REPLICATION_URL = "https://planet.openstreetmap.org/replication/day/"

CHANGES_SCHEMA = pa.schema([
    ("type", pa.string()),
    ("id", pa.int64()),
    ("version", pa.int32()),
    ("timestamp", pa.timestamp("s", tz="UTC")),
    ("tags", pa.large_string()),
])


def planet_timestamp(planet: Path) -> dt.datetime:
    header = osmium.io.Reader(str(planet), osmium.osm.NOTHING).header()
    value = header.get("osmosis_replication_timestamp")
    if not value:
        raise SystemExit(f"{planet} has no replication timestamp; pass --start-date")
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def download(url: str, dest: Path, retries: int = 5) -> None:
    if dest.exists():
        return
    part = dest.with_suffix(dest.suffix + ".part")
    for attempt in range(retries):
        try:
            with urllib.request.urlopen(url, timeout=120) as resp, open(part, "wb") as f:
                while chunk := resp.read(1 << 20):
                    f.write(chunk)
            part.rename(dest)
            return
        except OSError as e:
            if attempt == retries - 1:
                raise
            print(f"  retrying {url}: {e}")
            time.sleep(10 * (attempt + 1))


def download_diffs(start: dt.datetime, days: int, diff_dir: Path, server_url: str) -> list[Path]:
    """Download `days` daily diffs starting with the one that covers `start`."""
    server = ReplicationServer(server_url)
    # The newest state at or before `start`; the diff after it contains the
    # first changes that may be missing from the planet. Overlap with the
    # planet is harmless because pairs only use strictly newer versions.
    seq = server.timestamp_to_sequence(start)
    if seq is None:
        raise SystemExit(f"No replication state found for {start}")
    diff_dir.mkdir(parents=True, exist_ok=True)
    paths = []
    for s in range(seq + 1, seq + 1 + days):
        dest = diff_dir / f"{s:09d}.osc.gz"
        print(f"diff {s} -> {dest}")
        download(server.get_diff_url(s), dest)
        paths.append(dest)
    return paths


def collect_changes(diff_paths: list[Path]) -> dict[tuple[str, int], tuple[int, dt.datetime, dict]]:
    """Map (type, id) to the newest POI version seen in the diffs.

    Versions that are not POIs (including deletions) are ignored, so an element
    that was edited and later deleted still yields its last POI version.
    """
    latest: dict[tuple[str, int], tuple[int, dt.datetime, dict]] = {}
    for path in diff_paths:
        print(f"reading {path}")
        fp = (osmium.FileProcessor(str(path), osmium.osm.NODE | osmium.osm.WAY)
              .with_filter(osmium.filter.KeyFilter(*POI_FILTER_KEYS)))
        for obj in fp:
            tags = dict(obj.tags)
            if not is_poi(tags):
                continue
            key = ("n" if obj.is_node() else "w", obj.id)
            prev = latest.get(key)
            if prev is None or obj.version > prev[0]:
                latest[key] = (obj.version, obj.timestamp, tags)
    return latest


def write_changes(latest: dict, out: Path) -> None:
    rows = {name: [] for name in CHANGES_SCHEMA.names}
    for (typ, osm_id), (version, ts, tags) in latest.items():
        rows["type"].append(typ)
        rows["id"].append(osm_id)
        rows["version"].append(version)
        rows["timestamp"].append(ts)
        rows["tags"].append(json.dumps(tags, ensure_ascii=False))
    out.parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(pa.table(rows, schema=CHANGES_SCHEMA), out)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--planet", type=Path, help="planet file whose replication timestamp starts the window")
    parser.add_argument("--start-date", type=dt.datetime.fromisoformat,
                        help="start of the window (UTC), instead of reading it from --planet")
    parser.add_argument("--days", type=int, default=21)
    parser.add_argument("--diff-dir", type=Path, default=Path("data/diffs"))
    parser.add_argument("--out", type=Path, default=Path("data/changes.parquet"))
    parser.add_argument("--server", default=DAY_REPLICATION_URL)
    args = parser.parse_args()

    if args.start_date:
        start = args.start_date.replace(tzinfo=args.start_date.tzinfo or dt.timezone.utc)
    elif args.planet:
        start = planet_timestamp(args.planet)
    else:
        parser.error("one of --planet or --start-date is required")
    print(f"window starts at {start.isoformat()}")

    paths = download_diffs(start, args.days, args.diff_dir, args.server)
    latest = collect_changes(paths)
    write_changes(latest, args.out)
    print(f"wrote {len(latest)} changed POIs to {args.out}")


if __name__ == "__main__":
    main()
