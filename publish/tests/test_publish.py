"""End-to-end tests of each step against a local directory and an in-process S3 (moto)."""

import gzip
import io
import json
import struct
import tarfile
from pathlib import Path

import pytest

from cardinal_publish import latest, mapfile, routing, transit
from cardinal_publish.common import PublishError
from cardinal_publish.routing import level0_cell, tile_name
from cardinal_publish.target import DirTarget, Publisher, S3Target, State


def graph_id(level, tile):
    return (tile << 3) | level


def tile_bytes(gid, build, size=200, fill=b"x"):
    data = bytearray(fill * size)
    struct.pack_into("<Q", data, 0, gid)
    struct.pack_into("<Q", data, routing.HEADER_CHECKSUM_OFFSET, (build << 48) | 0x1234)
    return bytes(data)


# Seattle's level 0 tile, and level 1 and 2 tiles inside it, plus one tile elsewhere.
SEATTLE = [graph_id(0, 3074), graph_id(1, 49377), graph_id(2, 792229), graph_id(2, 793670)]
ELSEWHERE = [graph_id(0, 529)]


def write_tar(path: Path, gids, build=4060, fill=b"x"):
    """A tarball laid out as valhalla_build_extract writes one."""
    tiles = [(gid, tile_bytes(gid, build, 200 + 10 * i, fill)) for i, gid in enumerate(gids)]
    index_size = routing.INDEX_ENTRY.size * len(tiles)
    # Offsets: the index member is first (header 512 + data padded to 512), then each tile.
    offset = 512 + ((index_size + 511) // 512) * 512
    entries = []
    for gid, data in tiles:
        entries.append(routing.INDEX_ENTRY.pack(offset + 512, gid, len(data)))
        offset += 512 + ((len(data) + 511) // 512) * 512
    with tarfile.open(path, "w", format=tarfile.USTAR_FORMAT) as tar:
        index = b"".join(entries)
        info = tarfile.TarInfo(routing.INDEX_NAME)
        info.size = len(index)
        tar.addfile(info, io.BytesIO(index))
        for gid, data in tiles:
            info = tarfile.TarInfo(tile_name(gid))
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return dict(tiles)


def test_tile_names_and_cells_follow_valhalla():
    assert tile_name(graph_id(0, 529)) == "0/000/529.gph"
    assert tile_name(graph_id(1, 49377)) == "1/049/377.gph"
    assert tile_name(graph_id(2, 792229)) == "2/000/792/229.gph"
    assert {level0_cell(gid) for gid in SEATTLE} == {3074}
    assert level0_cell(graph_id(2, 1036799)) == 4049  # the last tile is in the last cell
    with pytest.raises(PublishError):
        tile_name(graph_id(3, 1))


STATES = {}


def state_at(path):
    """One State per file, as one run has: a second connection would wait on its transaction."""
    if path not in STATES:
        STATES[path] = State(path)
    return STATES[path]


def dir_publisher(tmp_path, dry_run=False):
    return Publisher(DirTarget(tmp_path / "bucket"), "releases", state_at(tmp_path / "state.sqlite"), 4, dry_run)


def test_routing_publishes_tiles_loose_with_an_index_and_reruns_cleanly(tmp_path):
    tar = tmp_path / "tiles.tar"
    tiles = write_tar(tar, SEATTLE + ELSEWHERE)
    release = routing.run(dir_publisher(tmp_path), tar, "2026-11-11", "test", "abc")
    root = tmp_path / "bucket/releases/routing/2026-11-11"
    for gid, data in tiles.items():
        assert (root / "tiles" / tile_name(gid)).read_bytes() == data
    assert release["valhalla_build_id"] == 4060
    assert release["index_cells"] == [529, 3074]
    assert release["levels"] == {"0": 2, "1": 1, "2": 2}
    shard = json.loads(gzip.decompress((root / "index/3074.json").read_bytes()))
    assert [entry["path"] for entry in shard] == sorted("tiles/" + tile_name(gid) for gid in SEATTLE)
    meta = json.loads((tmp_path / "bucket/.meta/releases/routing/2026-11-11/index/3074.json.json").read_text())
    assert (meta["content_type"], meta["content_encoding"]) == ("application/json", "gzip")
    on_disk = json.loads((root / "release.json").read_text())
    assert on_disk["tiles"] == 5

    # A rerun uploads nothing and accepts the release it finds.
    before = {p: p.stat().st_mtime_ns for p in root.rglob("*")}
    routing.run(dir_publisher(tmp_path), tar, "2026-11-11", "test", "abc")
    assert {p: p.stat().st_mtime_ns for p in root.rglob("*")} == before

    # Other tiles under a published date are refused, and nothing is overwritten.
    other = tmp_path / "other.tar"
    write_tar(other, SEATTLE + ELSEWHERE, fill=b"y")
    with pytest.raises(PublishError, match="next release of the day"):
        routing.run(dir_publisher(tmp_path), other, "2026-11-11", "test", "abc")
    assert {p: p.stat().st_mtime_ns for p in root.rglob("*")} == before
    routing.run(dir_publisher(tmp_path), other, "2026-11-11.1", "test", "abc")


def test_routing_resumes_and_refuses_leftovers(tmp_path):
    tar = tmp_path / "tiles.tar"
    write_tar(tar, SEATTLE)
    # An interrupted run left some tiles, and one with other bytes.
    root = tmp_path / "bucket/releases/routing/2026-11-11/tiles"
    first = root / tile_name(SEATTLE[0])
    first.parent.mkdir(parents=True)
    first.write_bytes(b"truncated")
    publisher = dir_publisher(tmp_path)
    routing.run(publisher, tar, "2026-11-11", None, None)
    assert first.read_bytes() == tile_bytes(SEATTLE[0], 4060, 200)

    # A tile left by a run of other inputs blocks release.json.
    stray = tmp_path / "bucket/releases/routing/2026-11-12/tiles/0/000/001.gph"
    stray.parent.mkdir(parents=True)
    stray.write_bytes(b"old")
    with pytest.raises(PublishError, match="aren't part of this release"):
        routing.run(dir_publisher(tmp_path), tar, "2026-11-12", None, None)
    assert not (tmp_path / "bucket/releases/routing/2026-11-12/release.json").exists()


def test_routing_checks_the_tarball(tmp_path):
    tar = tmp_path / "mixed.tar"
    write_tar(tar, SEATTLE)
    # Restamp one tile with another build ID.
    raw = bytearray(tar.read_bytes())
    at = raw.index(tile_bytes(SEATTLE[1], 4060, 210))
    struct.pack_into("<Q", raw, at + routing.HEADER_CHECKSUM_OFFSET, 77 << 48)
    tar.write_bytes(bytes(raw))
    with pytest.raises(PublishError, match="build IDs"):
        routing.run(dir_publisher(tmp_path), tar, "2026-11-11", None, None)
    assert not (tmp_path / "bucket/releases/routing/2026-11-11/release.json").exists()


def test_dry_run_writes_nothing(tmp_path):
    tar = tmp_path / "tiles.tar"
    write_tar(tar, SEATTLE)
    routing.run(dir_publisher(tmp_path, dry_run=True), tar, "2026-11-11", None, None)
    assert not (tmp_path / "bucket").exists()


def fake_release(publisher, kind, date, **fields):
    from cardinal_publish.common import Upload

    publisher.put(Upload.of_plain_json(f"{kind}/{date}/release.json", {"kind": kind, "date": date, **fields}, "x"))


def test_latest_refuses_transit_on_other_routing(tmp_path):
    publisher = dir_publisher(tmp_path)
    fake_release(publisher, "routing", "2026-11-11", valhalla_build_id=1)
    fake_release(publisher, "routing", "2026-11-12", valhalla_build_id=2)
    fake_release(publisher, "transit", "2026-11-11", valhalla_build_id=1, routing_release="2026-11-11")
    fake_release(publisher, "map", "2026-11-11")
    assert latest.run(publisher, {"map": "2026-11-11", "routing": "2026-11-11", "transit": "2026-11-11"}) == {
        "map": "2026-11-11", "routing": "2026-11-11", "transit": "2026-11-11"
    }
    with pytest.raises(PublishError, match="Valhalla build 1"):
        latest.run(publisher, {"map": None, "routing": "2026-11-12", "transit": None})
    with pytest.raises(PublishError, match="isn't published"):
        latest.run(publisher, {"map": "2026-11-13", "routing": None, "transit": None})
    document = json.loads((tmp_path / "bucket/releases/latest.json").read_text())
    assert document["routing"] == "2026-11-11"


def write_pmtiles(path: Path, size, tile_type=6, compression=2):
    header = bytearray(127)
    header[:8] = b"PMTiles\x03"
    header[98], header[99], header[100], header[101] = compression, tile_type, 0, 14
    body = bytes(range(256)) * (size // 256 + 1)
    path.write_bytes(bytes(header) + body[: size - 127])


def test_map_refuses_an_unknown_tile_type(tmp_path):
    pmtiles = tmp_path / "planet.pmtiles"
    write_pmtiles(pmtiles, 1000, tile_type=0, compression=1)
    with pytest.raises(PublishError, match="unknown tile type"):
        mapfile.publish(dir_publisher(tmp_path), pmtiles, "2026-11-11", None, None, 1 << 20)


@pytest.fixture
def s3(tmp_path):
    moto_server = pytest.importorskip("moto.server")
    server = moto_server.ThreadedMotoServer(port=0, verbose=False)
    server.start()
    host, port = server.get_host_and_port()
    endpoint = f"http://{host}:{port}"
    target = S3Target("bucket", endpoint, "us-east-1", "test", "test", 4)
    # moto can't resolve virtual-host addressing on 127.0.0.1.
    import boto3
    from botocore.config import Config

    target.client = boto3.client(
        "s3", endpoint_url=endpoint, region_name="us-east-1", aws_access_key_id="test",
        aws_secret_access_key="test", config=Config(s3={"addressing_style": "path"}),
    )
    target.client.create_bucket(Bucket="bucket")
    yield target
    server.stop()


def test_routing_and_map_on_s3(tmp_path, s3):
    tar = tmp_path / "tiles.tar"
    tiles = write_tar(tar, SEATTLE + ELSEWHERE)
    publisher = Publisher(s3, "releases", state_at(tmp_path / "state.sqlite"), 4, False)
    routing.run(publisher, tar, "2026-11-11", None, None)
    head = s3.client.head_object(Bucket="bucket", Key="releases/routing/2026-11-11/index/3074.json")
    assert head["ContentEncoding"] == "gzip" and head["ContentType"] == "application/json"
    gid = SEATTLE[2]
    obj = s3.client.get_object(Bucket="bucket", Key=f"releases/routing/2026-11-11/tiles/{tile_name(gid)}")
    assert obj["Body"].read() == tiles[gid]
    assert obj["Metadata"]["sha256"] == __import__("hashlib").sha256(tiles[gid]).hexdigest()

    # Without its local state, a rerun still skips by the objects' own SHA-256 headers.
    fresh = Publisher(s3, "releases", state_at(tmp_path / "other.sqlite"), 4, False)
    routing.run(fresh, tar, "2026-11-11", None, None)

    # The map's multipart upload resumes one a previous run left.
    pmtiles = tmp_path / "planet.pmtiles"
    write_pmtiles(pmtiles, 11 << 20)
    part = 5 << 20
    state = state_at(tmp_path / "map.sqlite")
    key = "releases/map/2026-11-11/planet.pmtiles"
    upload_id = s3.client.create_multipart_upload(Bucket="bucket", Key=key)["UploadId"]
    with open(pmtiles, "rb") as file:
        s3.client.upload_part(Bucket="bucket", Key=key, UploadId=upload_id, PartNumber=1, Body=file.read(part))
    stat = pmtiles.stat()
    state.set_multipart(s3.describe(), key, f"{stat.st_size}:{int(stat.st_mtime)}:{part}", upload_id)
    sent = []
    original = s3.client.upload_part
    s3.client.upload_part = lambda **kwargs: (sent.append(kwargs["PartNumber"]), original(**kwargs))[1]
    release = mapfile.publish(Publisher(s3, "releases", state, 2, False), pmtiles, "2026-11-11", None, None, part)
    assert sorted(sent) == [2, 3]
    body = s3.client.get_object(Bucket="bucket", Key=key)["Body"].read()
    assert body == pmtiles.read_bytes()
    assert release["sha256"] == __import__("hashlib").sha256(body).hexdigest()
    assert release["tile_type"] == "mlt"
    # Publishing again sends nothing.
    sent.clear()
    mapfile.publish(Publisher(s3, "releases", state, 2, False), pmtiles, "2026-11-11", None, None, part)
    assert sent == []


def write_transit_build(root: Path, build_id=4060):
    """A two-feed transit build and its feed index, as valinor-transit-build writes them."""
    import hashlib

    def put(relative, data):
        path = root / "build" / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return hashlib.sha256(data).hexdigest()

    feeds = []
    for index, feed_id in enumerate(["a", "b"]):
        timetable = f"timetables/{feed_id}.timetable"
        sha = put(timetable, feed_id.encode() * 50)
        snaps = {mode: {"file": f"transfers/snaps/{mode}/{index}.bin",
                        "sha256": put(f"transfers/snaps/{mode}/{index}.bin", f"{mode}{index}".encode())}
                 for mode in ("walking", "cycling")}
        feeds.append({"id": feed_id, "timetable": timetable, "timetable_bytes": 50, "timetable_sha256": sha,
                      "stop_count": 3, "stop_snaps": snaps, "transfer_tiles": {"walking": [[2624, 5721]]}})
    tiles = {}
    for mode in ("walking", "cycling"):
        data = mode.encode() * 10
        put(f"transfers/{mode}/14/2624/5721.bin.zst", data)
        tiles[mode] = [{"x": 2624, "y": 5721, "bytes": len(data)}]
    manifest = {"manifest_id": "00ff", "valhalla_build_id": build_id, "tile_format_version": 1, "zoom": 14,
                "feeds": feeds, "tiles": tiles}
    put("transfers/manifest.json", json.dumps(manifest).encode())
    staging = root / "staging"
    (staging / "index/10").mkdir(parents=True)
    (staging / "feed-index.json").write_text(json.dumps(
        {"manifest_id": "00ff", "feeds": 2, "uncompiled": 0, "index_files": 1}))
    (staging / "feeds.json").write_text("{}")
    (staging / "realtime.json").write_text('{"feeds":[]}')
    (staging / "index/10/22.json").write_text('{"tile":[10,22],"feeds":[]}')
    return root / "build", staging


@pytest.fixture(scope="session")
def pack_bin():
    """cardinal-pack, built from this checkout."""
    import subprocess

    crates = Path(__file__).resolve().parents[2] / "crates"
    subprocess.run(["cargo", "build", "--release", "-q", "-p", "offline_map", "--bin", "cardinal-pack"],
                   cwd=crates, check=True)
    return transit.default_pack_bin()


def read_archive_tile(archive: bytes, z, x, y):
    """A tile's bytes from an archive cardinal-pack wrote (no leaf directories)."""
    import gzip as gz

    def varints(data):
        value = shift = 0
        for byte in data:
            value |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80:
                yield value
                value = shift = 0

    root_offset, root_length = struct.unpack_from("<QQ", archive, 8)
    data_offset = struct.unpack_from("<Q", archive, 56)[0]
    numbers = list(varints(gz.decompress(archive[root_offset:root_offset + root_length])))
    n = numbers[0]
    ids, runs, lengths, offsets = (numbers[1 + i * n:1 + (i + 1) * n] for i in range(4))
    tile_ids = [sum(ids[:i + 1]) for i in range(n)]

    def hilbert(z, x, y):
        acc = ((1 << (2 * z)) - 1) // 3
        d, s = 0, 1 << (z - 1)
        while s > 0:
            rx, ry = int(x & s > 0), int(y & s > 0)
            d += s * s * ((3 * rx) ^ ry)
            if ry == 0:
                if rx == 1:
                    x, y = s - 1 - (x & (s - 1)), s - 1 - (y & (s - 1))
                x, y = y, x
            s >>= 1
        return acc + d

    at, position = 0, {}
    for i in range(n):
        at = at if offsets[i] == 0 and i > 0 else offsets[i] - 1
        position[tile_ids[i]] = (at, lengths[i])
        at += lengths[i]
    start, length = position[hilbert(z, x, y)]
    return archive[data_offset + start:data_offset + start + length]


def test_transit_publishes_against_its_routing_release(tmp_path, pack_bin):
    build, staging = write_transit_build(tmp_path)
    publisher = dir_publisher(tmp_path)
    work = tmp_path / "work"

    def publish(date, routing_date, commit, publisher=publisher):
        return transit.run(publisher, build, staging, date, routing_date, commit, work, pack_bin)

    with pytest.raises(PublishError, match="isn't published"):
        publish("2026-11-11", "2026-11-11", None)
    fake_release(publisher, "routing", "2026-11-10", valhalla_build_id=99)
    with pytest.raises(PublishError, match="Valhalla build 4060"):
        publish("2026-11-11", "2026-11-10", None)
    fake_release(publisher, "routing", "2026-11-11", valhalla_build_id=4060)
    release = publish("2026-11-11", "2026-11-11", "abc")
    assert release["routing_release"] == "2026-11-11" and release["valhalla_build_id"] == 4060
    root = tmp_path / "bucket/releases/transit/2026-11-11"
    assert (root / "timetables/a.timetable").read_bytes() == b"a" * 50
    assert (root / "snaps/cycling/1.bin").read_bytes() == b"cycling1"
    # Transfer tiles are in one archive per mode, not loose.
    assert not (root / "transfers/walking/14").exists()
    archive = (root / "transfers/walking.pmtiles").read_bytes()
    assert archive[:8] == b"PMTiles\x03" and archive[98:100] == bytes([1, 0])
    assert read_archive_tile(archive, 14, 2624, 5721) == b"walking" * 10
    assert release["transfer_archives"]["walking"] == {
        "path": "transfers/walking.pmtiles", "bytes": len(archive),
        "sha256": __import__("hashlib").sha256(archive).hexdigest(), "tiles": 1,
    }
    meta = json.loads((tmp_path / "bucket/.meta/releases/transit/2026-11-11/transfers/walking.pmtiles.json").read_text())
    assert meta["content_type"] == "application/vnd.pmtiles"
    assert json.loads(gzip.decompress((root / "index/10/22.json").read_bytes()))["tile"] == [10, 22]
    assert json.loads(gzip.decompress((root / "manifest/manifest.json").read_bytes()))["manifest_id"] == "00ff"
    # A rerun reuses the packed archives and uploads nothing.
    before = {p: p.stat().st_mtime_ns for p in root.rglob("*")}
    packed = {p: p.stat().st_mtime_ns for p in work.rglob("*")}
    publish("2026-11-11", "2026-11-11", "abc", dir_publisher(tmp_path))
    assert {p: p.stat().st_mtime_ns for p in root.rglob("*")} == before
    assert {p: p.stat().st_mtime_ns for p in work.rglob("*")} == packed

    # A dry run of another date packs (reusing the work) but uploads nothing.
    publish("2026-11-13", "2026-11-11", None, dir_publisher(tmp_path, dry_run=True))
    assert not (tmp_path / "bucket/releases/transit/2026-11-13").exists()

    # A transfer tile that isn't the size the manifest gives is refused when packing.
    tile = build / "transfers/cycling/14/2624/5721.bin.zst"
    tile.write_bytes(b"short")
    for path in work.rglob("cycling.*"):
        path.unlink()
    with pytest.raises(PublishError, match="manifest says"):
        publish("2026-11-12", "2026-11-11", None, dir_publisher(tmp_path))
    tile.write_bytes(b"cycling" * 10)

    # A timetable that isn't the one the manifest names is refused.
    (build / "timetables/b.timetable").write_bytes(b"c" * 50)
    with pytest.raises(PublishError, match="SHA-256"):
        publish("2026-11-12", "2026-11-11", None, dir_publisher(tmp_path))


# The poi steps.

from datetime import datetime, timedelta, timezone

from cardinal_publish import poi

T0 = datetime(2026, 10, 5, tzinfo=timezone.utc)


def poi_release(kind, sequence, snapshot, files, previous=True):
    checksum = f"{sequence:064x}"
    return {
        "format": 1, "kind": kind, "sequence": sequence,
        "timestamp": (T0 + timedelta(days=sequence - 100)).isoformat().replace("+00:00", "Z"),
        "next_poi_id": 1000 + sequence, "checksum": checksum,
        "previous": ({"sequence": sequence - 1, "next_poi_id": 999 + sequence, "checksum": f"{sequence - 1:064x}"}
                     if previous else None),
        "snapshot": snapshot, "ruleset": 3, "planet_timestamp": "2026-10-05T00:00:03Z",
        "replication_url": "https://planet.openstreetmap.org/replication/day/", "files": files, "counts": {},
    }


def stage_dir(directory: Path, kind, sequence, snapshot, contents, previous=True):
    import hashlib

    directory.mkdir(parents=True)
    files = {}
    for name, data in contents.items():
        (directory / name).write_bytes(data)
        files[name] = {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    release = poi_release(kind, sequence, snapshot, files, previous)
    (directory / "release.json").write_text(json.dumps(release, indent=2))
    return release


def stage_poi(staging: Path, sequences, snapshot_every=7, first=100):
    """A staging directory as `poi_service export` leaves it: the planet's checkpoint, a delta per
    later one, and a snapshot every `snapshot_every` checkpoints."""
    for sequence in sequences:
        snapshot = first + (sequence - first) // snapshot_every * snapshot_every
        if snapshot == sequence:
            stage_dir(staging / "snapshots" / str(sequence), "snapshot", sequence, sequence, {
                "versions.bin.zst": b"v" * sequence, "pois.bin.zst": b"p" * sequence,
                "status.bin.zst": b"s" * sequence, "checkpoints.json": b"[]",
            }, previous=False)
        if sequence == first:
            stage_dir(staging / str(sequence), "planet", sequence, snapshot, {}, previous=False)
        else:
            stage_dir(staging / str(sequence), "delta", sequence, snapshot, {"delta.bin.zst": b"d" * sequence})


def test_poi_publishes_staged_releases_and_reruns_cleanly(tmp_path):
    staging = tmp_path / "staging"
    stage_poi(staging, range(100, 103))
    # An export in progress is ignored.
    (staging / ".103.partial").mkdir()
    publisher = dir_publisher(tmp_path)
    assert poi.run(publisher, staging, True, 1 << 20) == 102
    bucket = tmp_path / "bucket/releases"
    assert json.loads((bucket / "latest.json").read_text()) == {"poi": 102}
    assert (bucket / "poi/snapshots/100/pois.bin.zst").read_bytes() == b"p" * 100
    assert (bucket / "poi/102/delta.bin.zst").read_bytes() == b"d" * 102
    assert json.loads((bucket / "poi/100/release.json").read_text())["kind"] == "planet"
    meta = json.loads((tmp_path / "bucket/.meta/releases/poi/101/delta.bin.zst.json").read_text())
    assert (meta["content_type"], meta["content_encoding"]) == ("application/octet-stream", None)
    meta = json.loads((tmp_path / "bucket/.meta/releases/poi/101/release.json.json").read_text())
    assert meta["cache_control"] == "public, max-age=300"

    # A rerun uploads nothing; more checkpoints go up on their own.
    before = {p: p.stat().st_mtime_ns for p in bucket.rglob("*") if p.is_file() and "latest" not in p.name}
    stage_poi(staging, range(103, 105), first=100)
    assert poi.run(publisher, staging, False, 1 << 20) == 104
    assert all(p.stat().st_mtime_ns == t for p, t in before.items())
    assert json.loads((bucket / "latest.json").read_text()) == {"poi": 102}

    # Staging that disagrees with what's published is refused.
    release = json.loads((staging / "101/release.json").read_text())
    release["checksum"] = "0" * 64
    (staging / "101/release.json").write_text(json.dumps(release))
    with pytest.raises(PublishError, match="other contents"):
        poi.run(publisher, staging, False, 1 << 20)


def test_poi_needs_the_snapshot_and_previous_checkpoint(tmp_path):
    staging = tmp_path / "staging"
    stage_poi(staging, range(100, 104))
    import shutil

    shutil.rmtree(staging / "snapshots/100")
    with pytest.raises(PublishError, match="snapshot 100"):
        poi.run(dir_publisher(tmp_path), staging, False, 1 << 20)
    staging = tmp_path / "staging2"
    stage_poi(staging, [100, 101, 103])
    with pytest.raises(PublishError, match="follows 102"):
        poi.run(dir_publisher(tmp_path / "other"), staging, False, 1 << 20)


def test_poi_dry_run_writes_nothing(tmp_path):
    staging = tmp_path / "staging"
    stage_poi(staging, range(100, 103))
    poi.run(dir_publisher(tmp_path, dry_run=True), staging, True, 1 << 20)
    assert not (tmp_path / "bucket").exists()


def published_poi(bucket: Path):
    root = bucket / "releases/poi"
    dailies = sorted(int(p.name) for p in root.iterdir() if p.name.isdigit())
    snapshots = sorted(int(p.name) for p in (root / "snapshots").iterdir())
    return dailies, snapshots


def test_poi_prune_keeps_four_weeks_and_their_snapshots(tmp_path):
    staging = tmp_path / "staging"
    stage_poi(staging, range(100, 145))  # 45 days, snapshots at 100, 107, ..., 142
    publisher = dir_publisher(tmp_path)
    poi.run(publisher, staging, False, 1 << 20)
    latest.run(publisher, {"poi": 110})

    # A dry run deletes nothing.
    doomed = poi.prune(dir_publisher(tmp_path, dry_run=True), 28)
    assert published_poi(tmp_path / "bucket") == (list(range(100, 145)), list(range(100, 143, 7)))
    assert "poi/100/" in doomed and "poi/110/" not in doomed

    poi.prune(publisher, 28)
    dailies, snapshots = published_poi(tmp_path / "bucket")
    # 144 is the newest; 116 is 28 days older, and restores from snapshot 114
    # with delta 115. 110 is what latest.json names, restored from 107 with
    # 108 and 109.
    assert dailies == [108, 109, 110] + list(range(115, 145))
    assert snapshots == [107, 114, 121, 128, 135, 142]
    # Every checkpoint left can be restored: its snapshot and every delta after it are there.
    for sequence in dailies:
        release = json.loads((tmp_path / f"bucket/releases/poi/{sequence}/release.json").read_text())
        assert release["snapshot"] in snapshots
        assert set(range(release["snapshot"] + 1, sequence + 1)) <= set(dailies), sequence
    assert not (tmp_path / "bucket/releases/poi/100").exists()
    assert not (tmp_path / "bucket/.meta/releases/poi/100").exists()
    # Pruning again changes nothing.
    assert poi.prune(publisher, 28) == []


def test_poi_prune_works_on_a_staging_directory(tmp_path):
    staging = tmp_path / "staging"
    stage_poi(staging, range(100, 140))
    publisher = Publisher(DirTarget(staging), "", None, 4, False)
    poi.prune(publisher, 28, base="")
    dailies = sorted(int(p.name) for p in staging.iterdir() if p.name.isdigit())
    # 111 is 28 days before 139, and restores from 107.
    assert dailies == list(range(108, 140))
    assert sorted(int(p.name) for p in (staging / "snapshots").iterdir()) == [107, 114, 121, 128, 135]


def test_latest_names_a_poi_checkpoint_without_pairing(tmp_path):
    publisher = dir_publisher(tmp_path)
    fake_release(publisher, "map", "2026-11-11")
    stage_poi(tmp_path / "staging", range(100, 102))
    poi.run(publisher, tmp_path / "staging", False, 1 << 20)
    assert latest.run(publisher, {"map": "2026-11-11", "poi": "101"}) == {"map": "2026-11-11", "poi": 101}
    with pytest.raises(PublishError, match="isn't published"):
        latest.run(publisher, {"poi": "102"})
    with pytest.raises(PublishError, match="replication sequence"):
        latest.run(publisher, {"poi": "2026-11-11"})


def test_poi_on_s3(tmp_path, s3):
    staging = tmp_path / "staging"
    stage_poi(staging, range(100, 140))
    publisher = Publisher(s3, "releases", state_at(tmp_path / "state.sqlite"), 4, False)
    # Snapshot files this big go up as a multipart upload.
    big = staging / "snapshots/135/pois.bin.zst"
    big.write_bytes(b"P" * (6 << 20))
    release = json.loads((staging / "snapshots/135/release.json").read_text())
    import hashlib

    release["files"]["pois.bin.zst"] = {"size": 6 << 20, "sha256": hashlib.sha256(big.read_bytes()).hexdigest()}
    (staging / "snapshots/135/release.json").write_text(json.dumps(release))
    old_large = poi.LARGE_FILE
    poi.LARGE_FILE = 1 << 20
    try:
        # S3's smallest part is 5 MiB.
        assert poi.run(publisher, staging, True, 5 << 20) == 139
    finally:
        poi.LARGE_FILE = old_large
    body = s3.client.get_object(Bucket="bucket", Key="releases/poi/snapshots/135/pois.bin.zst")["Body"].read()
    assert body == b"P" * (6 << 20)
    poi.prune(publisher, 28)
    keys = s3.list("releases/poi/")
    assert "releases/poi/107/release.json" not in keys and "releases/poi/108/delta.bin.zst" in keys
    assert not any(k.startswith("releases/poi/snapshots/100/") for k in keys)
    assert "releases/poi/snapshots/107/versions.bin.zst" in keys


from cardinal_publish import geocoder
from cardinal_publish.image import SUPERBLOCK_MAGIC, SUPERBLOCK_MAGIC_OFFSET


def write_btrfs(path: Path, size=200_000, fill=b"z"):
    """Enough of a btrfs image for the steps' check: the superblock's magic at 64 KiB."""
    data = bytearray(fill * size)
    data[SUPERBLOCK_MAGIC_OFFSET:SUPERBLOCK_MAGIC_OFFSET + len(SUPERBLOCK_MAGIC)] = SUPERBLOCK_MAGIC
    path.write_bytes(bytes(data))
    return bytes(data)


def test_routing_publishes_its_image_beside_the_loose_tiles(tmp_path):
    import hashlib

    tar = tmp_path / "tiles.tar"
    write_tar(tar, SEATTLE)
    image = tmp_path / "routing.btrfs"
    data = write_btrfs(image)
    release = routing.run(dir_publisher(tmp_path), tar, "2026-11-11", None, None, image, 1 << 16)
    root = tmp_path / "bucket/releases/routing/2026-11-11"
    assert (root / "routing.btrfs").read_bytes() == data
    assert release["image"] == {"file": "routing.btrfs", "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    assert (root / "tiles" / tile_name(SEATTLE[0])).exists()

    # A rerun checks the image too, and needs it to.
    routing.run(dir_publisher(tmp_path), tar, "2026-11-11", None, None, image, 1 << 16)
    with pytest.raises(PublishError, match="pass --image"):
        routing.run(dir_publisher(tmp_path), tar, "2026-11-11", None, None)
    other = tmp_path / "other.btrfs"
    write_btrfs(other, fill=b"y")
    with pytest.raises(PublishError, match="next release of the day"):
        routing.run(dir_publisher(tmp_path), tar, "2026-11-11", None, None, other, 1 << 16)
    assert (root / "routing.btrfs").read_bytes() == data


def test_geocoder_publishes_the_image_with_its_schema_version(tmp_path):
    image = tmp_path / "geocoder.btrfs"
    data = write_btrfs(image)
    metadata = tmp_path / "geocoder.json"
    metadata.write_text(json.dumps({"schema_version": 7}))
    release = geocoder.run(dir_publisher(tmp_path), image, metadata, "2026-11-14", "planet-261005", "abc", 1 << 16)
    root = tmp_path / "bucket/releases/geocoder/2026-11-14"
    assert (root / "geocoder.btrfs").read_bytes() == data
    assert release["schema_version"] == 7
    on_disk = json.loads((root / "release.json").read_text())
    assert on_disk["image"]["bytes"] == len(data)

    before = {p: p.stat().st_mtime_ns for p in root.rglob("*")}
    geocoder.run(dir_publisher(tmp_path), image, metadata, "2026-11-14", "planet-261005", "abc", 1 << 16)
    assert {p: p.stat().st_mtime_ns for p in root.rglob("*")} == before

    other = tmp_path / "other.btrfs"
    write_btrfs(other, fill=b"y")
    with pytest.raises(PublishError, match="next release of the day"):
        geocoder.run(dir_publisher(tmp_path), other, metadata, "2026-11-14", "planet-261005", "abc", 1 << 16)


def test_geocoder_refuses_what_isnt_a_btrfs_image(tmp_path):
    image = tmp_path / "geocoder.btrfs"
    image.write_bytes(b"x" * 200_000)
    metadata = tmp_path / "geocoder.json"
    metadata.write_text(json.dumps({"schema_version": 7}))
    with pytest.raises(PublishError, match="isn't a btrfs image"):
        geocoder.run(dir_publisher(tmp_path), image, metadata, "2026-11-14", None, None, 1 << 16)
    metadata.write_text("{}")
    with pytest.raises(PublishError, match="schema_version"):
        geocoder.run(dir_publisher(tmp_path), image, metadata, "2026-11-14", None, None, 1 << 16)
    assert not (tmp_path / "bucket/releases/geocoder").exists()


def test_images_on_s3(tmp_path, s3):
    tar = tmp_path / "tiles.tar"
    write_tar(tar, SEATTLE)
    image = tmp_path / "routing.btrfs"
    write_btrfs(image, size=12 << 20)
    publisher = Publisher(s3, "releases", state_at(tmp_path / "state.sqlite"), 4, False)
    # moto's buckets outlive each test's server, so this uses a date no other test does.
    routing.run(publisher, tar, "2026-11-20", None, None, image, 5 << 20)
    body = s3.client.get_object(Bucket="bucket", Key="releases/routing/2026-11-20/routing.btrfs")["Body"].read()
    assert body == image.read_bytes()
