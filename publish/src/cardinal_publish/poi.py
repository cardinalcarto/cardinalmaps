"""The poi steps: uploading `poi_service export`'s staged releases, and pruning old ones.

The staging directory is laid out as the bucket's `poi/` (see README.md): `SEQ/` for each
checkpoint and `snapshots/SEQ/` for each snapshot, each complete once its `release.json`
exists. Everything an export writes is immutable, so a release is uploaded once and a rerun
only checks it. Snapshots go up before the checkpoints that name them, and each checkpoint's
`release.json` after its files, so a client never sees a release it can't restore.

Pruning is a separate step and never runs as part of an upload.
"""

import json
import re
import sys
from datetime import datetime, timedelta
from pathlib import Path

from . import latest
from .archive import publish_file
from .common import CACHE_RELEASE_MARKER, JSON, OCTET_STREAM, PublishError, Upload, publisher_commit
from .release import RELEASE_FILE, comparable, finish_release

POI = "poi/"
SNAPSHOTS = "snapshots/"
FORMAT = 1
# Files at least this big go up as one resumable multipart upload, as the map does.
LARGE_FILE = 256 << 20
SEQUENCE = re.compile(r"^[0-9]+$")


def daily_prefix(sequence: int, base: str = POI) -> str:
    return f"{base}{sequence}/"


def snapshot_prefix(sequence: int, base: str = POI) -> str:
    return f"{base}{SNAPSHOTS}{sequence}/"


def parse_time(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def read_staged(staging: Path):
    """Complete staged releases: ({sequence: (dir, release)} for snapshots, then for checkpoints)."""
    def scan(directory: Path, snapshot: bool):
        found = {}
        if not directory.is_dir():
            return found
        for entry in directory.iterdir():
            if not entry.is_dir() or not SEQUENCE.match(entry.name):
                continue
            path = entry / RELEASE_FILE
            if not path.is_file():
                continue
            release = json.loads(path.read_text())
            sequence = int(entry.name)
            kind = release.get("kind")
            if release.get("format") != FORMAT or release.get("sequence") != sequence:
                raise PublishError(f"{path} isn't format {FORMAT} release {sequence}")
            if (kind == "snapshot") != snapshot or kind not in ("planet", "delta", "snapshot"):
                raise PublishError(f"{path} is a {kind} release")
            for name in release["files"]:
                if "/" in name or name.startswith(".") or not (entry / name).is_file():
                    raise PublishError(f"{path} lists {name!r}, which isn't in {entry}")
            found[sequence] = (entry, release)
        return found

    if not staging.is_dir():
        raise PublishError(f"no staging directory at {staging}")
    return scan(staging / SNAPSHOTS.rstrip("/"), True), scan(staging, False)


def publish_release(publisher, prefix: str, directory: Path, release: dict, part_size: int):
    """Uploads one staged release's files, then its `release.json`; returns whether anything was uploaded."""
    existing = publisher.get_json(prefix + RELEASE_FILE)
    document = dict(release, publisher_commit=publisher_commit())
    if existing is not None:
        if comparable(existing) != comparable(document):
            raise PublishError(
                f"{publisher.key(prefix)} is published with other contents than {directory}; "
                "the staging directory isn't this bucket's, or an export changed history"
            )
        return False
    remote = publisher.remote(prefix)
    small = []
    changed = 0
    for name, info in sorted(release["files"].items()):
        path = directory / name
        if path.stat().st_size != info["size"]:
            raise PublishError(f"{path} is {path.stat().st_size} bytes, but its release says {info['size']}")
        content_type = JSON if name.endswith(".json") else OCTET_STREAM
        if info["size"] >= LARGE_FILE:
            size, sha, uploaded = publish_file(publisher, path, prefix + name, part_size, content_type=content_type)
            if (size, sha) != (info["size"], info["sha256"]):
                raise PublishError(f"{path} has SHA-256 {sha}, but its release says {info['sha256']}")
            changed += uploaded
        else:
            small.append(Upload(key=prefix + name, body=None, path=path, size=info["size"],
                                sha256=info["sha256"], content_type=content_type))
    progress = publisher.sync(iter(small), remote, f"poi {prefix.rstrip('/')}")
    expected = {prefix + name for name in release["files"]}
    finish_release(publisher, prefix, document, None, expected, remote, CACHE_RELEASE_MARKER,
                   changed=changed + progress.uploaded)
    return True


def run(publisher, staging: Path, set_latest: bool, part_size: int):
    snapshots, dailies = read_staged(staging)
    if not dailies:
        raise PublishError(f"{staging} has no complete releases; run `poi_service export` first")
    published_snapshots = set()
    for sequence, (directory, release) in sorted(snapshots.items()):
        if publish_release(publisher, snapshot_prefix(sequence), directory, release, part_size):
            print(f"{'would publish' if publisher.dry_run else 'published'} snapshot {sequence}", file=sys.stderr)
        published_snapshots.add(sequence)

    newest = None
    uploaded = 0
    for sequence, (directory, release) in sorted(dailies.items()):
        snapshot = release["snapshot"]
        if snapshot not in published_snapshots and publisher.get_json(snapshot_prefix(snapshot) + RELEASE_FILE) is None:
            raise PublishError(f"release {sequence} restores from snapshot {snapshot}, which is neither staged nor published")
        previous = release.get("previous")
        if previous is not None and previous["sequence"] not in dailies:
            published = publisher.get_json(daily_prefix(previous["sequence"]) + RELEASE_FILE)
            if published is None or published["checksum"] != previous["checksum"]:
                raise PublishError(f"release {sequence} follows {previous['sequence']}, which isn't staged or published")
        elif previous is not None and dailies[previous["sequence"]][1]["checksum"] != previous["checksum"]:
            raise PublishError(f"staged release {sequence} doesn't follow staged {previous['sequence']}")
        if publish_release(publisher, daily_prefix(sequence), directory, release, part_size):
            uploaded += 1
        newest = sequence
    print(f"poi: {'would publish' if publisher.dry_run else 'published'} {uploaded} new checkpoint releases, "
          f"{len(dailies) - uploaded} published already; newest {newest}", file=sys.stderr)
    if set_latest and publisher.dry_run:
        print(f"would point {publisher.key(latest.LATEST)}'s poi at {newest}", file=sys.stderr)
    elif set_latest:
        latest.run(publisher, {"poi": newest})
    return newest


def plan_prune(publisher, keep_days: int, base: str = POI):
    """What pruning would delete: ({prefix: [keys]}, kept daily sequences, kept snapshot sequences)."""
    remote = publisher.remote(base)
    groups = {}
    for key in remote:
        rest = key[len(base):]
        parts = rest.split("/")
        if parts[0] + "/" == SNAPSHOTS and len(parts) > 2 and SEQUENCE.match(parts[1]):
            groups.setdefault(("snapshot", int(parts[1])), []).append(key)
        elif len(parts) > 1 and SEQUENCE.match(parts[0]):
            groups.setdefault(("daily", int(parts[0])), []).append(key)
    dailies = {}
    for (kind, sequence) in groups:
        if kind == "daily":
            release = publisher.get_json(daily_prefix(sequence, base) + RELEASE_FILE)
            if release is not None:
                dailies[sequence] = release
    if not dailies:
        return {}, set(), set()
    newest = max(dailies)
    cutoff = parse_time(dailies[newest]["timestamp"]) - timedelta(days=keep_days)
    keep = {s for s, r in dailies.items() if parse_time(r["timestamp"]) >= cutoff}
    keep.add(newest)
    current = (publisher.get_json(latest.LATEST) or {}).get("poi") if base == POI else None
    if current is not None:
        keep.add(current)
    keep_snapshots = {dailies[s]["snapshot"] for s in keep if s in dailies}
    doomed = {}
    for (kind, sequence), keys in groups.items():
        if kind == "daily":
            # A prefix without release.json may be an upload in progress; it's left alone.
            if sequence in dailies and sequence not in keep:
                doomed[daily_prefix(sequence, base)] = keys
        elif sequence not in keep_snapshots and sequence < newest:
            # A snapshot newer than every checkpoint release is one whose release is still to come.
            if any(key.endswith("/" + RELEASE_FILE) for key in keys):
                doomed[snapshot_prefix(sequence, base)] = keys
    return doomed, keep, keep_snapshots


def prune(publisher, keep_days: int, base: str = POI):
    """Deletes checkpoint releases more than `keep_days` older than the newest, and snapshots none of
    the rest restores from. Never the newest release, nor the one `latest.json` names."""
    if keep_days < 1:
        raise PublishError("--keep-days must be at least 1")
    doomed, keep, keep_snapshots = plan_prune(publisher, keep_days, base)
    verb = "would delete" if publisher.dry_run else "deleting"
    print(f"poi prune: keeping {len(keep)} checkpoint releases ({min(keep, default=None)} to "
          f"{max(keep, default=None)}) and snapshots {sorted(keep_snapshots)}", file=sys.stderr)
    for prefix, keys in sorted(doomed.items()):
        print(f"{verb} {publisher.key(prefix)} ({len(keys)} objects)", file=sys.stderr)
        if publisher.dry_run:
            continue
        # release.json first: a half-deleted release is incomplete rather than broken.
        marker = prefix + RELEASE_FILE
        publisher.delete([key for key in keys if key == marker])
        publisher.delete([key for key in keys if key != marker])
    return sorted(doomed)
