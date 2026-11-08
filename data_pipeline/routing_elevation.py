"""Cuts the elevation tiles that a Valhalla tile build needs from GEDTM.

    routing_elevation.py GEDTM OSM TILE_DIR OUT_DIR [JOBS]

GEDTM is OpenGeoHub's global 1 arcsecond DTM, v1.2, as GDAL reads it (a path, or
/vsicurl/ and a URL). TILE_DIR holds Valhalla graph tiles built from the OSM
extract, up to the elevation stage. OUT_DIR gets one gzipped .hgt for each 1x1
degree cell under them and in the extract's bounding box, named the way
mainline Valhalla's get_hgt_file_name looks for it (N46/N46W122.hgt.gz). It
skips cells where every pixel is no data, like open ocean, and cells outside
GEDTM's 65S to 85N.

Mainline Valhalla reads only 3601x3601 cells, with pixel centers from one whole
degree to the next, sharing their edges with their neighbors, like SRTM. The
source's values are meters, despite its scale tag of 0.1 (Mount Rainier's summit
reads 4413, and its Zenodo record says a scale of 1), so they're kept as they
are, rounded to whole meters, with -32768 for no data. See
topo_pipeline/scripts/tile_gedtm.sh, which cuts the same cells for contours.
"""

import gzip
import math
import multiprocessing
import os
import shutil
import sys
import zlib

import numpy
from osgeo import gdal, osr

gdal.UseExceptions()

DIM = 3601
NO_DATA = -32768
# The pixel at column 4, row 4 of v1.2 is centered on (-180, 85).
OFFSET = 4
SIZE = (1296009, 540009)
ORIGIN = (-180.00125, 85.00125)
PIXEL = 1 / 3600
NORTH, SOUTH = 85, -65

# Valhalla's tile levels: level, tile size in degrees.
LEVELS = {0: 4.0, 1: 1.0, 2: 0.25}


def cells_under_tiles(tile_dir):
    """The 1x1 degree cells, as (lon, lat) of their southwest corners, that
    Valhalla's graph tiles in tile_dir cover."""
    cells = set()
    for level, size in LEVELS.items():
        columns = round(360 / size)
        for root, _, files in os.walk(os.path.join(tile_dir, str(level))):
            for name in files:
                if not name.endswith(".gph"):
                    continue
                path = os.path.relpath(os.path.join(root, name[: -len(".gph")]), tile_dir)
                tile = int("".join(path.split(os.sep)[1:]))
                west = -180 + (tile % columns) * size
                south = -90 + (tile // columns) * size
                for lat in range(int(south // 1), int(-(-(south + size) // 1))):
                    for lon in range(int(west // 1), int(-(-(west + size) // 1))):
                        cells.add((lon, lat))
    return sorted(cells)


def _varint(data, i):
    value, shift = 0, 0
    while True:
        byte = data[i]
        i += 1
        value |= (byte & 0x7F) << shift
        shift += 7
        if byte < 0x80:
            return value, i


def _fields(data):
    """A protobuf message's fields, as (number, value), for varint and
    length-delimited fields."""
    i = 0
    while i < len(data):
        key, i = _varint(data, i)
        number, kind = key >> 3, key & 7
        if kind == 0:
            value, i = _varint(data, i)
        elif kind == 2:
            length, i = _varint(data, i)
            value, i = data[i : i + length], i + length
        elif kind == 1:
            value, i = data[i : i + 8], i + 8
        elif kind == 5:
            value, i = data[i : i + 4], i + 4
        else:
            raise ValueError(f"unexpected protobuf wire type {kind}")
        yield number, value


def pbf_bbox(path):
    """The bounding box in an OSM PBF's header, as (west, south, east, north),
    or None if it has none."""
    with open(path, "rb") as f:
        size = int.from_bytes(f.read(4), "big")
        header = dict(_fields(f.read(size)))
        if header.get(1) != b"OSMHeader":
            raise ValueError(f"{path} doesn't start with an OSMHeader")
        blob = dict(_fields(f.read(header[3])))
    if 1 in blob:
        block = blob[1]
    elif 3 in blob:
        block = zlib.decompress(blob[3])
    else:
        raise ValueError(f"{path}'s header is compressed in a way this can't read")
    box = dict(_fields(block)).get(1)
    if box is None:
        return None
    # left, right, top, bottom, as zigzag-encoded nanodegrees.
    sides = {n: ((v >> 1) ^ -(v & 1)) / 1e9 for n, v in _fields(box)}
    return sides[1], sides[4], sides[2], sides[3]


def hgt_name(lon, lat):
    ns = "S" if lat < 0 else "N"
    ew = "W" if lon < 0 else "E"
    return f"{ns}{abs(lat):02d}/{ns}{abs(lat):02d}{ew}{abs(lon):03d}.hgt"


def check_layout(source):
    """Refuses any layout but v1.2's, which the offsets above would get wrong."""
    ds = gdal.Open(source)
    gt = ds.GetGeoTransform()
    if (
        (ds.RasterXSize, ds.RasterYSize) != SIZE
        or abs(gt[0] - ORIGIN[0]) > 1e-9
        or abs(gt[3] - ORIGIN[1]) > 1e-9
        or abs(gt[1] - PIXEL) > 1e-12
        or abs(gt[5] + PIXEL) > 1e-12
        or gt[2] != 0
        or gt[4] != 0
    ):
        sys.exit(f"unexpected layout, which the offsets would get wrong: size {ds.RasterXSize}x{ds.RasterYSize}, geotransform {gt}")


_source = None


def _open(source):
    global _source
    _source = gdal.Open(source)


def cut(job):
    """Writes one cell's .hgt.gz, returning its size, or 0 if it had no data."""
    lon, lat, out = job
    path = os.path.join(out, hgt_name(lon, lat) + ".gz")
    if os.path.exists(path):
        return os.path.getsize(path)
    band = _source.GetRasterBand(1)
    column = (lon + 180) * 3600 + OFFSET
    row = (NORTH - lat - 1) * 3600 + OFFSET
    # Raw values, without the scale tag.
    values = band.ReadAsArray(column, row, DIM, DIM)
    missing = ~numpy.isfinite(values)
    source_no_data = band.GetNoDataValue()
    if source_no_data is not None:
        missing |= values == numpy.float32(source_no_data)
    if missing.all():
        return 0
    heights = numpy.clip(numpy.rint(numpy.where(missing, 0, values)), -32767, 32767).astype(numpy.int16)
    heights[missing] = NO_DATA

    mem = gdal.GetDriverByName("MEM").Create("", DIM, DIM, 1, gdal.GDT_Int16)
    mem.SetGeoTransform((lon - PIXEL / 2, PIXEL, 0, lat + 1 + PIXEL / 2, 0, -PIXEL))
    srs = osr.SpatialReference()
    srs.ImportFromEPSG(4326)
    mem.SetProjection(srs.ExportToWkt())
    mem.GetRasterBand(1).SetNoDataValue(NO_DATA)
    mem.GetRasterBand(1).WriteArray(heights)

    os.makedirs(os.path.dirname(path), exist_ok=True)
    raw = path[: -len(".gz")]
    gdal.GetDriverByName("SRTMHGT").CreateCopy(raw, mem)
    mem = None
    if os.path.getsize(raw) != DIM * DIM * 2:
        sys.exit(f"{raw} isn't a {DIM}x{DIM} .hgt")
    partial = path + ".partial"
    with open(raw, "rb") as f, gzip.open(partial, "wb", compresslevel=6) as g:
        shutil.copyfileobj(f, g, 1 << 20)
    os.remove(raw)
    for aux in (raw + ".aux.xml",):
        if os.path.exists(aux):
            os.remove(aux)
    os.rename(partial, path)
    return os.path.getsize(path)


def main():
    if len(sys.argv) not in (5, 6):
        sys.exit(__doc__)
    source, osm, tile_dir, out = sys.argv[1:5]
    jobs = int(sys.argv[5]) if len(sys.argv) == 6 else 8
    # Each job reads only the blocks it needs, and doesn't need them again.
    os.environ.setdefault("GDAL_CACHEMAX", "512")
    # A remote source is read with range requests, which can fail now and then.
    os.environ.setdefault("GDAL_HTTP_MAX_RETRY", "10")
    os.environ.setdefault("GDAL_HTTP_RETRY_DELAY", "2")
    os.environ.setdefault("GDAL_DISABLE_READDIR_ON_OPEN", "EMPTY_DIR")
    check_layout(source)

    cells = cells_under_tiles(tile_dir)
    print(f"{len(cells)} cells under the graph tiles", flush=True)
    # Level 0's tiles are 4 degrees on a side, so for an extract, many of
    # their cells are outside it.
    bbox = pbf_bbox(osm)
    if bbox:
        west, south, east, north = bbox
        cells = [
            (lon, lat)
            for lon, lat in cells
            if math.floor(west) <= lon < math.ceil(east) and math.floor(south) <= lat < math.ceil(north)
        ]
        print(f"{len(cells)} of them in the extract's bounding box {bbox}", flush=True)
    covered = [(lon, lat) for lon, lat in cells if SOUTH <= lat < NORTH]
    if len(covered) < len(cells):
        print(f"{len(cells) - len(covered)} of them outside GEDTM", flush=True)
    os.makedirs(out, exist_ok=True)
    written, total = 0, 0
    with multiprocessing.Pool(jobs, initializer=_open, initargs=(source,)) as pool:
        for size in pool.imap_unordered(cut, [(lon, lat, out) for lon, lat in covered]):
            if size:
                written += 1
                total += size
    print(f"{written} elevation tiles, {len(covered) - written} with no data, {total / 1e9:.2f} GB", flush=True)


if __name__ == "__main__":
    main()
