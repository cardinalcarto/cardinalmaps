"""The wof step: the Who's On First admin SpatiaLite database `wof_service` serves, for servers.

A wof release is `wof/DATE/`, holding:

- `whosonfirst-data-admin.spatial.db`, the database, uncompressed, as one resumable multipart
  upload. A server downloads it whole onto its volume and opens it read-only, and an
  uncompressed object lets an interrupted download resume with a range request (`curl -C -`),
  as a btrfs image does. A multipart upload carries no `x-amz-meta-sha256`, so `release.json`
  records the database's size and SHA-256 under `database`.

Phones don't download the database: they fetch places from a `wof_service`, so `latest.json`
has no `wof` key. Servers pin a release by hand (`WOF_RELEASE` in `deploy/wof_service/fly.toml`),
as they do a geocoder image.
"""

import sqlite3
import sys
from pathlib import Path

from .archive import publish_file
from .common import CACHE_RELEASE_MARKER, OCTET_STREAM, PublishError, publisher_commit, release_prefix
from .release import finish_release

DATABASE_FILE = "whosonfirst-data-admin.spatial.db"
SQLITE_MAGIC = b"SQLite format 3\x00"
# The tables wof_service reads (wof_service/src/places.rs), R-trees included.
TABLES = ("place", "property", "name", "hierarchy", "geometry", "idx_geometry_geom", "shard", "idx_shard_geom")


def check_database(path: Path):
    """Fails unless `path` is a SQLite database with the tables wof_service reads."""
    if not path.is_file():
        raise PublishError(f"no database at {path}")
    with open(path, "rb") as file:
        if file.read(len(SQLITE_MAGIC)) != SQLITE_MAGIC:
            raise PublishError(f"{path} isn't a SQLite database")
    try:
        # Immutable: only the schema is read, and nothing is written beside the file.
        connection = sqlite3.connect(f"{path.as_uri()}?mode=ro&immutable=1", uri=True)
        try:
            names = {row[0] for row in connection.execute("SELECT name FROM sqlite_master")}
        finally:
            connection.close()
    except sqlite3.Error as error:
        raise PublishError(f"can't read {path}'s schema: {error}") from error
    missing = [table for table in TABLES if table not in names]
    if missing:
        raise PublishError(
            f"{path} isn't a Who's On First SpatiaLite database: it has no {', '.join(missing)}"
        )


def run(publisher, database: Path, date: str, source: str | None, part_size: int):
    prefix = release_prefix("wof", date)
    check_database(database)

    existing = publisher.get_json(prefix + "release.json")
    if existing:
        print(f"wof {date} is published already; checking it matches, uploading nothing", file=sys.stderr)
        publisher = publisher.checking()
    remote = publisher.remote(prefix)

    key = prefix + DATABASE_FILE
    published = existing.get("database") if existing else None
    recorded = (published["bytes"], published["sha256"]) if published else None
    size, sha, changed = publish_file(publisher, database, key, part_size, recorded, OCTET_STREAM)

    release = {
        "kind": "wof",
        "date": date,
        "source": source,
        "publisher_commit": publisher_commit(),
        "database": {"file": DATABASE_FILE, "bytes": size, "sha256": sha},
    }
    finish_release(publisher, prefix, release, existing, {key}, remote, CACHE_RELEASE_MARKER, changed=changed)
    return release
