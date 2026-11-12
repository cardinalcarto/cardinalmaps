"""The transit step: a transit build and its feed index, published for clients to pick from.

The build directory is the transfer stage's output (`compile.json`, `timetables/`,
`transfers/`); the staging directory is `valinor-transit-build feed-index`'s output for it.
Every file is checked against the build's manifest, and the release records the routing
release its walks were computed on, which must have the same Valhalla build ID.
"""

import json
import sys
from pathlib import Path

from .common import (
    CACHE_RELEASE_MARKER,
    JSON,
    OCTET_STREAM,
    PublishError,
    Upload,
    check_date,
    gzip_bytes,
    publisher_commit,
    release_prefix,
)
from .release import finish_release

ZSTD = "application/zstd"
BUILD_SNAPS = "transfers/snaps/"
STAGED_JSON = ("feeds.json", "realtime.json", "feeds", "index", "manifest")


def file_upload(key, path: Path, content_type, size=None, sha256=None):
    actual = path.stat().st_size
    if size is not None and actual != size:
        raise PublishError(f"{path} is {actual} bytes; the manifest says {size}")
    return Upload(key=key, body=None, path=path, size=actual, sha256=sha256, content_type=content_type)


def build_uploads(build: Path, manifest, prefix):
    """Every file of the build that clients download, in the release's layout."""
    for feed in manifest["feeds"]:
        timetable = feed.get("timetable")
        if timetable:
            if not timetable.startswith("timetables/") or ".." in timetable.split("/"):
                raise PublishError(f"feed {feed['id']}'s timetable path {timetable!r} isn't under timetables/")
            yield file_upload(prefix + timetable, build / timetable, OCTET_STREAM,
                              feed["timetable_bytes"], feed["timetable_sha256"])
        for snaps in feed.get("stop_snaps", {}).values():
            path = snaps["file"]
            if not path.startswith(BUILD_SNAPS) or ".." in path.split("/"):
                raise PublishError(f"feed {feed['id']}'s snap file {path!r} isn't under {BUILD_SNAPS}")
            yield file_upload(prefix + "snaps/" + path[len(BUILD_SNAPS):], build / path, OCTET_STREAM,
                              sha256=snaps["sha256"])
    zoom = manifest["zoom"]
    for mode, tiles in manifest["tiles"].items():
        for tile in tiles:
            relative = f"transfers/{mode}/{zoom}/{tile['x']}/{tile['y']}.bin.zst"
            yield file_upload(prefix + relative, build / relative, ZSTD, tile["bytes"])


def staged_uploads(staging: Path, prefix):
    for name in STAGED_JSON:
        root = staging / name
        paths = [root] if root.is_file() else sorted(root.rglob("*.json"))
        for path in paths:
            relative = path.relative_to(staging).as_posix()
            yield Upload.of_bytes(prefix + relative, gzip_bytes(path.read_bytes()), JSON, "gzip")


def run(publisher, build: Path, staging: Path, date: str, routing_date: str, build_commit: str | None):
    prefix = release_prefix("transit", date)
    check_date(routing_date)
    manifest_path = build / "transfers" / "manifest.json"
    if not manifest_path.is_file():
        raise PublishError(f"no transit build at {build} (no {manifest_path})")
    manifest_bytes = manifest_path.read_bytes()
    manifest = json.loads(manifest_bytes)
    index_report_path = staging / "feed-index.json"
    if not index_report_path.is_file():
        raise PublishError(f"no feed index at {staging}; run `valinor-transit-build feed-index` for the build")
    index_report = json.loads(index_report_path.read_text())
    if index_report["manifest_id"] != manifest["manifest_id"]:
        raise PublishError(
            f"the feed index at {staging} is of build {index_report['manifest_id']}, "
            f"not {manifest['manifest_id']}; run feed-index again"
        )

    routing = publisher.get_json(f"routing/{routing_date}/release.json")
    if routing is None:
        raise PublishError(f"routing release {routing_date} isn't published (no routing/{routing_date}/release.json)")
    if routing["valhalla_build_id"] != manifest["valhalla_build_id"]:
        raise PublishError(
            f"this transit build's walks are on Valhalla build {manifest['valhalla_build_id']}, but routing "
            f"release {routing_date} is build {routing['valhalla_build_id']}"
        )

    existing = publisher.get_json(prefix + "release.json")
    if existing:
        print(f"transit {date} is published already; checking it matches, uploading nothing", file=sys.stderr)
        publisher = publisher.checking()
    remote = publisher.remote(prefix)
    print(f"transit {date}: {len(remote)} objects already under {publisher.key(prefix)}", file=sys.stderr)

    keys = set()

    def counted(uploads):
        for upload in uploads:
            keys.add(upload.key)
            yield upload

    files = publisher.sync(counted(build_uploads(build, manifest, prefix)), remote, f"transit {date} files")
    staged = list(staged_uploads(staging, prefix))
    staged.append(Upload.of_bytes(prefix + "manifest/manifest.json", gzip_bytes(manifest_bytes), JSON, "gzip"))
    keys.update(upload.key for upload in staged)
    index = publisher.sync(iter(staged), remote, f"transit {date} index")

    release = {
        "kind": "transit",
        "date": date,
        "manifest_id": manifest["manifest_id"],
        "valhalla_build_id": manifest["valhalla_build_id"],
        "routing_release": routing_date,
        "transitous_commit": manifest.get("transitous_commit"),
        "fetched_at": manifest.get("fetched_at"),
        "build_commit": build_commit,
        "publisher_commit": publisher_commit(),
        "feeds": index_report["feeds"],
        "uncompiled_feeds": index_report["uncompiled"],
        "index_files": index_report["index_files"],
        "zoom": manifest["zoom"],
        "tiles": {mode: len(tiles) for mode, tiles in sorted(manifest["tiles"].items())},
        "files": files.items,
        "file_bytes": files.bytes,
    }
    finish_release(
        publisher, prefix, release, existing, keys, remote, CACHE_RELEASE_MARKER,
        changed=files.uploaded + index.uploaded,
    )
    return release
