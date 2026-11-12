"""Publishing one large local file (a PMTiles archive) as one object, resumably."""

import sys
from pathlib import Path

from .common import Upload, sha256_file

PMTILES_CONTENT_TYPE = "application/vnd.pmtiles"


def publish_file(publisher, path: Path, key: str, part_size: int, published=None, content_type=PMTILES_CONTENT_TYPE):
    """Uploads `path` as `key` (relative to the publisher's root) unless the bucket has it.

    With `published`, the `(bytes, sha256)` a complete release recorded for it, nothing is
    uploaded: the file is only compared. A multipart upload a previous run left is resumed.
    Returns `(bytes, sha256, changed)`, `changed` 1 if it was (or would be) uploaded or
    differs from `published`.
    """
    stat = path.stat()
    size = stat.st_size
    identity = f"{stat.st_size}:{stat.st_mtime_ns}"
    path_key = str(path.resolve())
    target = publisher.target.describe()
    state = publisher.state
    sha = state.file_sha(path_key, identity) if state else None
    if published is not None:
        if sha is None:
            print(f"hashing {path} to compare it with the release", file=sys.stderr)
            sha = sha256_file(path)
        return size, sha, int((size, sha) != tuple(published))
    remote = publisher.target.head(publisher.key(key))
    known = state.known(target, key) if state else None
    if sha and remote and remote.size == size and (remote.sha256 == sha or (known and tuple(known) == (size, sha))):
        print(f"{publisher.key(key)} is uploaded already", file=sys.stderr)
        return size, sha, 0
    upload = Upload(key=key, body=None, path=path, size=size, sha256=sha or "", content_type=content_type)
    sha = publisher.target.put_large(path, publisher.key(key), upload, part_size, publisher.jobs, state, publisher.dry_run)
    if state and not publisher.dry_run:
        state.set_file_sha(path_key, identity, sha)
        state.record(target, key, size, sha)
    return size, sha, 1
