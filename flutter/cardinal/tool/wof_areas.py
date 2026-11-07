#!/usr/bin/env python3
"""Writes assets/map/wof_areas.jsongz, gzipped: the names of Who's On First's large areas.

    tool/wof_areas.py WOF.spatial.db assets/map/wof_areas.jsongz

Tiles name the small areas their POIs are in, but not countries, dependencies, macroregions
and regions, which hold most of the bytes and would repeat in every tile. The app has those
instead. They rarely change, so this only needs running when that matters. It needs no
SpatiaLite.

The file maps each area's ID to its names as in tiles (see `WofAdmins` in
planetiler-openmaptiles: `name`, `abbr` and ISO 639-3 codes), and `country` to its
ISO 3166-1 alpha-2 code for countries and dependencies.
"""
import gzip
import json
import re
import sqlite3
import sys

PLACETYPES = ("country", "dependency", "macroregion", "region")
CODE = re.compile(r"^[A-Z]{2}$")

source, target = sys.argv[1:]
db = sqlite3.connect(f"file:{source}?mode=ro&immutable=1", uri=True)
marks = ",".join("?" * len(PLACETYPES))
areas = db.execute(
    f"""SELECT DISTINCT p.id, p.type FROM place p JOIN shard s ON s.source = p.source AND s.id = p.id
        WHERE p.source = 'wof' AND p.type IN ({marks})""",
    PLACETYPES,
).fetchall()


def names(id):
    rows = db.execute(
        "SELECT lang, tag, abbr, name FROM name WHERE source = 'wof' AND id = ?", (id,)
    ).fetchall()
    rows = [r for r in rows if r[3]]
    full = [r for r in rows if not r[2]]
    if not full:
        return None
    pick = lambda lang, tag: next((r[3] for r in full if r[0] == lang and r[1] == tag), None)
    default = pick("und", "default") or pick("eng", "preferred") or full[0][3]
    out = {"name": default}
    for lang, tag, _, name in full:
        if tag == "preferred" and lang != "und" and name != default:
            out.setdefault(lang, name)
    rank = {"eng": 0, "und": 1}
    abbreviations = sorted((r for r in rows if r[2]), key=lambda r: rank.get(r[0], 2))
    if abbreviations:
        out["abbr"] = abbreviations[0][3]
    return out


def country(id):
    row = db.execute(
        """SELECT value FROM property WHERE source = 'wof' AND id = ? AND key IN ('alpha2', 'wof:country')
           ORDER BY key = 'alpha2' DESC LIMIT 1""",
        (id,),
    ).fetchone()
    code = row[0].upper() if row and row[0] else None
    return code if code and CODE.match(code) and code != "XX" else None


table = {}
for id, placetype in sorted(areas, key=lambda a: int(a[0])):
    area = names(id)
    if area is None:
        continue
    if placetype in ("country", "dependency") and (code := country(id)):
        area["country"] = code
    table[id] = area

data = json.dumps(table, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode()
# Without a timestamp, so that the same data gives the same file.
with open(target, "wb") as out:
    out.write(gzip.compress(data, 9, mtime=0))
print(f"{len(table)} areas, {len(data) / 1e6:.1f} MB, {len(gzip.compress(data, 9)) / 1e6:.1f} MB compressed")
