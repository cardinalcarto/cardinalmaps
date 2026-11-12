"""The map steps: converting the MBTiles tileset to PMTiles, and publishing it.

`map-convert` runs go-pmtiles' `convert`, pinned below and downloaded with its checksum checked
unless `--pmtiles-bin` names one. go-pmtiles (as of 1.31.2) only recognizes MVT and raster
formats in MBTiles metadata, so it writes an MLT tileset with an unknown tile type and no
tile compression, although the tiles are gzipped MLT. `pmtiles serve` then serves them with
no extension in its TileJSON, which it answers 404 to, and without `Content-Encoding`.
`--mlt-header` sets those two header bytes after converting; without it, an MLT input is
refused before converting.

`map` publishes `planet.pmtiles` as one multipart upload, which a rerun resumes.
"""

import os
import platform
import shutil
import sqlite3
import struct
import subprocess
import sys
import tarfile
import urllib.request
import zipfile
from dataclasses import dataclass
from pathlib import Path

from .common import CACHE_RELEASE_MARKER, PublishError, Upload, publisher_commit, release_prefix, sha256_file
from .release import finish_release

PMTILES_VERSION = "1.31.2"
# SHA-256 of each release archive, from https://github.com/protomaps/go-pmtiles/releases/tag/v1.31.2.
PMTILES_ARCHIVES = {
    ("Linux", "x86_64"): ("go-pmtiles_1.31.2_Linux_x86_64.tar.gz", "3ed7dbf4ec2e6dfe5e25b6f70d1ffc932729f93c86db353bf514dd71010a312f"),
    ("Linux", "aarch64"): ("go-pmtiles_1.31.2_Linux_arm64.tar.gz", "f8bd47e7ea866863489cad588fbaf2f31f42e5821f7a03f009b3769f05801cb1"),
    ("Darwin", "arm64"): ("go-pmtiles-1.31.2_Darwin_arm64.zip", "40528f7f616fcbf91207cd48c8fc023d213f6d86c0cbf1f748732803d1880f3d"),
    ("Darwin", "x86_64"): ("go-pmtiles-1.31.2_Darwin_x86_64.zip", "1f0dc02eee6c58312dd6c509faee1b5c32f0596568af1bf51f1b034e7a88a65b"),
}
PMTILES_URL = "https://github.com/protomaps/go-pmtiles/releases/download/v{version}/{archive}"

PMTILES_CONTENT_TYPE = "application/vnd.pmtiles"
MAP_FILE = "planet.pmtiles"

# PMTiles v3 header (https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md).
HEADER_LEN = 127
TILE_COMPRESSION_OFFSET = 98
TILE_TYPE_OFFSET = 99
COMPRESSION = {0: "unknown", 1: "none", 2: "gzip", 3: "brotli", 4: "zstd"}
TILE_TYPES = {0: "unknown", 1: "mvt", 2: "png", 3: "jpeg", 4: "webp", 5: "avif", 6: "mlt"}
MLT, GZIP, NONE, UNKNOWN = 6, 2, 1, 0

# How MBTiles metadata names MLT: planetiler writes the media type.
MLT_FORMATS = ("application/vnd.maplibre-vector-tile", "mlt")


@dataclass(frozen=True)
class Header:
    tile_compression: int
    tile_type: int
    min_zoom: int
    max_zoom: int
    bounds: tuple
    center: tuple
    addressed_tiles: int

    def describe(self):
        return {
            "tile_type": TILE_TYPES.get(self.tile_type, str(self.tile_type)),
            "tile_compression": COMPRESSION.get(self.tile_compression, str(self.tile_compression)),
            "min_zoom": self.min_zoom,
            "max_zoom": self.max_zoom,
            "bounds": list(self.bounds),
            "center": list(self.center),
            "addressed_tiles": self.addressed_tiles,
        }


def read_header(path: Path) -> Header:
    with open(path, "rb") as file:
        raw = file.read(HEADER_LEN)
    if len(raw) < HEADER_LEN or raw[:7] != b"PMTiles" or raw[7] != 3:
        raise PublishError(f"{path} isn't a PMTiles v3 archive")
    (addressed,) = struct.unpack_from("<Q", raw, 72)
    min_lon, min_lat, max_lon, max_lat = struct.unpack_from("<4i", raw, 102)
    center_zoom = raw[118]
    center_lon, center_lat = struct.unpack_from("<2i", raw, 119)
    return Header(
        tile_compression=raw[TILE_COMPRESSION_OFFSET],
        tile_type=raw[TILE_TYPE_OFFSET],
        min_zoom=raw[100],
        max_zoom=raw[101],
        bounds=(min_lon / 1e7, min_lat / 1e7, max_lon / 1e7, max_lat / 1e7),
        center=(center_lon / 1e7, center_lat / 1e7, center_zoom),
        addressed_tiles=addressed,
    )


def mbtiles_metadata(path: Path):
    if not path.is_file():
        raise PublishError(f"no MBTiles at {path}")
    connection = sqlite3.connect(f"file:{path}?mode=ro&immutable=1", uri=True)
    try:
        return dict(connection.execute("SELECT name, value FROM metadata"))
    finally:
        connection.close()


def pmtiles_binary(cache: Path) -> Path:
    """The pinned go-pmtiles, downloaded into `cache` once and checked by SHA-256."""
    key = (platform.system(), platform.machine())
    if key not in PMTILES_ARCHIVES:
        raise PublishError(f"no pinned go-pmtiles for {key}; pass --pmtiles-bin")
    archive, expected = PMTILES_ARCHIVES[key]
    directory = cache / f"go-pmtiles-{PMTILES_VERSION}"
    binary = directory / "pmtiles"
    if binary.is_file():
        return binary
    directory.mkdir(parents=True, exist_ok=True)
    download = directory / archive
    url = PMTILES_URL.format(version=PMTILES_VERSION, archive=archive)
    print(f"downloading {url}", file=sys.stderr)
    with urllib.request.urlopen(url, timeout=120) as response, open(download, "wb") as out:
        shutil.copyfileobj(response, out)
    found = sha256_file(download)
    if found != expected:
        download.unlink()
        raise PublishError(f"{archive} has SHA-256 {found}, not the pinned {expected}")
    partial = directory / "pmtiles.part"
    if archive.endswith(".zip"):
        with zipfile.ZipFile(download) as bundle, bundle.open("pmtiles") as source, open(partial, "wb") as out:
            shutil.copyfileobj(source, out)
    else:
        with tarfile.open(download) as bundle:
            source = bundle.extractfile("pmtiles")
            if source is None:
                raise PublishError(f"{archive} has no pmtiles binary")
            with open(partial, "wb") as out:
                shutil.copyfileobj(source, out)
    partial.chmod(0o755)
    os.replace(partial, binary)
    download.unlink()
    return binary


def check_space(mbtiles: Path, out: Path, tmpdir: Path):
    """Fails before converting if the output and temporary files can't fit.

    Each takes about the input's size; on one disk, that disk needs room for both.
    """
    need = mbtiles.stat().st_size
    same_disk = os.stat(out.parent).st_dev == os.stat(tmpdir).st_dev
    for directory in {out.parent, tmpdir} if not same_disk else {out.parent}:
        wanted = need * (2 if same_disk else 1)
        free = shutil.disk_usage(directory).free
        if free < wanted:
            raise PublishError(
                f"{directory} has {free / 1e9:.0f} GB free; converting {mbtiles} needs about "
                f"{wanted / 1e9:.0f} GB there (output and temporary files each about the input's size). "
                "Put --tmpdir on another disk"
            )


def convert(mbtiles: Path, out: Path, tmpdir: Path, binary, mlt_header: bool):
    """Converts, unless `out` is converted already; `binary()` gives go-pmtiles, once checks pass."""
    metadata = mbtiles_metadata(mbtiles)
    tile_format = metadata.get("format", "")
    is_mlt = tile_format in MLT_FORMATS
    if is_mlt and not mlt_header:
        raise PublishError(
            f"{mbtiles} holds MLT tiles ({tile_format}), which go-pmtiles {PMTILES_VERSION} writes with an "
            "unknown tile type and no compression, so `pmtiles serve` can't serve them. "
            "Pass --mlt-header to set the type and compression in the header after converting"
        )
    if out.exists():
        header = read_header(out)
        if header.tile_type == UNKNOWN:
            raise PublishError(f"{out} exists with an unknown tile type; remove it and convert again")
        print(f"{out} is converted already ({header.describe()})", file=sys.stderr)
        return header
    if not tmpdir.is_dir():
        raise PublishError(f"--tmpdir {tmpdir} isn't a directory")
    out.parent.mkdir(parents=True, exist_ok=True)
    check_space(mbtiles, out, tmpdir)
    partial = out.with_name(out.stem + ".partial.pmtiles")
    if partial.exists():
        partial.unlink()
    subprocess.run(
        [str(binary()), "convert", f"--tmpdir={tmpdir}", str(mbtiles), str(partial)],
        check=True,
    )
    header = read_header(partial)
    if is_mlt:
        compression = {"gzip": GZIP, "": NONE, "none": NONE}.get(metadata.get("compression", ""))
        if compression is None:
            raise PublishError(f"{mbtiles} names tile compression {metadata['compression']!r}, which isn't handled")
        if (header.tile_type, header.tile_compression) == (UNKNOWN, NONE):
            with open(partial, "r+b") as file:
                file.seek(TILE_COMPRESSION_OFFSET)
                file.write(bytes([compression, MLT]))
                file.flush()
                os.fsync(file.fileno())
            header = read_header(partial)
        if (header.tile_type, header.tile_compression) != (MLT, compression):
            raise PublishError(f"{partial} has tile type {header.tile_type} and compression {header.tile_compression}")
    os.replace(partial, out)
    print(f"wrote {out} ({header.describe()})", file=sys.stderr)
    return header


def publish(publisher, pmtiles: Path, date: str, source: str | None, build_commit: str | None, part_size: int):
    prefix = release_prefix("map", date)
    if not pmtiles.is_file():
        raise PublishError(f"no PMTiles at {pmtiles}")
    header = read_header(pmtiles)
    if header.tile_type == UNKNOWN:
        raise PublishError(f"{pmtiles} has an unknown tile type; convert it with map-convert --mlt-header")
    existing = publisher.get_json(prefix + "release.json")
    if existing:
        print(f"map {date} is published already; checking it matches, uploading nothing", file=sys.stderr)
        publisher = publisher.checking()
    key = prefix + MAP_FILE
    stat = pmtiles.stat()
    size = stat.st_size
    identity = f"{stat.st_size}:{stat.st_mtime_ns}"
    path_key = str(pmtiles.resolve())
    target = publisher.target.describe()
    state = publisher.state
    sha = state.file_sha(path_key, identity) if state else None
    changed = 0
    if existing:
        if sha is None:
            print(f"hashing {pmtiles} to compare it with the release", file=sys.stderr)
            sha = sha256_file(pmtiles)
        changed = int((size, sha) != (existing.get("bytes"), existing.get("sha256")))
    else:
        remote = publisher.target.head(publisher.key(key))
        known = state.known(target, key) if state else None
        if sha and remote and remote.size == size and (remote.sha256 == sha or (known and tuple(known) == (size, sha))):
            print(f"{publisher.key(key)} is uploaded already", file=sys.stderr)
        else:
            upload = Upload(key=key, body=None, path=pmtiles, size=size, sha256=sha or "",
                            content_type=PMTILES_CONTENT_TYPE)
            sha = publisher.target.put_large(
                pmtiles, publisher.key(key), upload, part_size, publisher.jobs, state, publisher.dry_run
            )
            if state and not publisher.dry_run:
                state.set_file_sha(path_key, identity, sha)
                state.record(target, key, size, sha)
    release = {
        "kind": "map",
        "date": date,
        "file": MAP_FILE,
        "bytes": size,
        "sha256": sha,
        "source": source,
        "build_commit": build_commit,
        "publisher_commit": publisher_commit(),
        **header.describe(),
    }
    remote_keys = publisher.remote(prefix)
    finish_release(
        publisher, prefix, release, existing, {key}, remote_keys, CACHE_RELEASE_MARKER,
        changed=changed,
    )
    return release
