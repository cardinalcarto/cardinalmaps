"""The routing step: Valhalla tiles from a `build_extract` tarball, published loose.

The tarball is read once, in order, straight from disk: `index.bin` (its first member) gives
each tile's ID, offset, and size, and every tile member must agree with it. Each tile is
published at `tiles/<name in the tar>`, and `index/<level 0 tile>.json` lists, for each 4°
level 0 tile, every level 0 to 2 tile inside it with its size and SHA-256, so a client can
fetch an area's tiles and check them. `release.json` is written last, once all of that is up.
"""

import dataclasses
import struct
import sys
import tarfile
from pathlib import Path

from .common import (
    CACHE_RELEASE_MARKER,
    OCTET_STREAM,
    PublishError,
    Upload,
    publisher_commit,
    release_prefix,
)
from .release import finish_release

INDEX_NAME = "index.bin"
INDEX_ENTRY = struct.Struct("<QLL")  # offset, tile ID, size (valhalla_build_extract)

# Valhalla's hierarchy: tile size in degrees by level. Level 3 (transit) isn't published.
LEVEL_SIZES = {0: 4.0, 1: 1.0, 2: 0.25}
LEVEL0_COLUMNS = 90

# GraphTileHeader: `graphid_` is the first 8 bytes; `checksum_` is at byte 88, and its top
# 16 bits are the build ID that valinor checks tiles against (valhalla-graphtile `BuildId`).
HEADER_GRAPH_ID = struct.Struct("<Q")
HEADER_CHECKSUM_OFFSET = 88
HEADER_LEN = HEADER_CHECKSUM_OFFSET + 8
GRAPH_ID_TILE_MASK = (1 << 25) - 1  # level (3 bits) and tile (22 bits); the rest is an index


def level_columns(level):
    return round(360 / LEVEL_SIZES[level])


def tile_name(graph_id):
    """The path Valhalla gives a tile: `2/000/818/660.gph`, digits padded to groups of 3."""
    level = graph_id & 0x7
    tile = (graph_id >> 3) & ((1 << 22) - 1)
    if level not in LEVEL_SIZES:
        raise PublishError(f"tile {graph_id} is on level {level}, which isn't published")
    columns = level_columns(level)
    max_id = columns * (columns // 2) - 1
    digits = len(str(max_id))
    digits += (3 - digits % 3) % 3
    text = str(tile).zfill(digits)
    groups = [text[i:i + 3] for i in range(0, digits, 3)]
    return f"{level}/{'/'.join(groups)}.gph"


def level0_cell(graph_id):
    """The level 0 tile that holds a tile of any level (each level's tiles nest exactly)."""
    level = graph_id & 0x7
    tile = (graph_id >> 3) & ((1 << 22) - 1)
    columns = level_columns(level)
    row, column = divmod(tile, columns)
    per_level0 = round(LEVEL_SIZES[0] / LEVEL_SIZES[level])
    return (row // per_level0) * LEVEL0_COLUMNS + column // per_level0


def build_id(header: bytes):
    (checksum,) = struct.unpack_from("<Q", header, HEADER_CHECKSUM_OFFSET)
    return checksum >> 48


def read_index(raw):
    if len(raw) % INDEX_ENTRY.size:
        raise PublishError(f"{INDEX_NAME} is {len(raw)} bytes, not a whole number of entries")
    by_offset = {}
    for offset, graph_id, size in INDEX_ENTRY.iter_unpack(raw):
        by_offset[offset] = (graph_id, size)
    return by_offset


def tiles(tar_path: Path, found):
    """Yields an Upload for each tile, checking each against index.bin and its own header.

    Fills `found` with the index entries, levels, and build IDs seen.
    """
    with open(tar_path, "rb") as raw_file, tarfile.open(fileobj=raw_file, mode="r:") as tar:
        first = tar.next()
        if first is None or first.name != INDEX_NAME:
            raise PublishError(f"{tar_path}'s first member isn't {INDEX_NAME}, so it isn't a build_extract tarball")
        index = read_index(tar.extractfile(first).read())
        found["index_entries"] = len(index)
        seen = 0
        while (member := tar.next()) is not None:
            # TarFile keeps every member it has read; a planet has 200,000 of them.
            tar.members.clear()
            if not member.isfile():
                raise PublishError(f"{member.name} in {tar_path} isn't a regular file")
            entry = index.get(member.offset_data)
            if entry is None:
                raise PublishError(f"{member.name} at offset {member.offset_data} isn't in {INDEX_NAME}")
            graph_id, size = entry
            name = tile_name(graph_id)
            if member.name != name or member.size != size:
                raise PublishError(
                    f"{member.name} ({member.size} bytes) doesn't match {INDEX_NAME}'s "
                    f"{name} ({size} bytes)"
                )
            data = tar.extractfile(member).read()
            if len(data) < HEADER_LEN:
                raise PublishError(f"{name} is too short for a tile header")
            (header_id,) = HEADER_GRAPH_ID.unpack_from(data, 0)
            if header_id & GRAPH_ID_TILE_MASK != graph_id:
                raise PublishError(f"{name}'s header says it is tile {header_id & GRAPH_ID_TILE_MASK}")
            found["build_ids"].add(build_id(data))
            level = graph_id & 0x7
            found["levels"][level] = found["levels"].get(level, 0) + 1
            seen += 1
            upload = Upload.of_bytes(f"tiles/{name}", data, OCTET_STREAM)
            found["cells"].setdefault(level0_cell(graph_id), []).append(
                {"path": upload.key, "bytes": upload.size, "sha256": upload.sha256}
            )
            yield upload
        if seen != len(index):
            raise PublishError(f"{tar_path} has {seen} tiles but {INDEX_NAME} lists {len(index)}")


def run(publisher, tar_path: Path, date: str, source: str | None, build_commit: str | None):
    prefix = release_prefix("routing", date)
    if not tar_path.is_file():
        raise PublishError(f"no tarball at {tar_path}")
    existing = publisher.get_json(prefix + "release.json")
    if existing:
        print(f"routing {date} is published already; checking it matches, uploading nothing", file=sys.stderr)
        publisher = publisher.checking()
    remote = publisher.remote(prefix)
    print(f"routing {date}: {len(remote)} objects already under {publisher.key(prefix)}", file=sys.stderr)

    found = {"build_ids": set(), "levels": {}, "cells": {}}
    uploads = (dataclasses.replace(upload, key=prefix + upload.key) for upload in tiles(tar_path, found))
    size = tar_path.stat().st_size
    progress = publisher.sync(uploads, remote, f"routing {date}", total_bytes=size)

    if len(found["build_ids"]) != 1:
        raise PublishError(f"tiles carry build IDs {sorted(found['build_ids'])}; a release needs exactly one")
    (valhalla_build_id,) = found["build_ids"]

    shards = []
    for cell, entries in sorted(found["cells"].items()):
        entries.sort(key=lambda entry: entry["path"])
        shards.append(Upload.of_json(f"{prefix}index/{cell}.json", entries))
    shard_progress = publisher.sync(iter(shards), remote, f"routing {date} index")

    release = {
        "kind": "routing",
        "date": date,
        "valhalla_build_id": valhalla_build_id,
        "source": source,
        "build_commit": build_commit,
        "publisher_commit": publisher_commit(),
        "tiles": progress.items,
        "tile_bytes": progress.bytes,
        "levels": {str(level): count for level, count in sorted(found["levels"].items())},
        "index_cells": sorted(found["cells"]),
        "tarball_bytes": size,
    }
    expected = {prefix + entry["path"] for entries in found["cells"].values() for entry in entries}
    expected |= {upload.key for upload in shards}
    finish_release(
        publisher, prefix, release, existing, expected, remote, CACHE_RELEASE_MARKER,
        changed=progress.uploaded + shard_progress.uploaded,
    )
    return release
