"""Sorts bench_quality.py's diagnosis (cases only the server passes) into root-cause
categories by rules read off the evidence, with counts and examples.

    python3 -I -B bench_categorize.py <dir>/diagnosis.jsonl > <dir>/categories.txt

The rules encode what a manual read of the 2026-11-17 Washington run found; a case no
rule matches is listed as "unclassified" for a person to read.
"""

import json
import sys
from pathlib import Path
from collections import defaultdict

WA = (-124.85, 45.54, -116.91, 49.01)

LABEL_LAYER_TAGS = {("natural", "water"), ("natural", "peak"), ("natural", "volcano"), ("boundary", "national_park"),
                    ("boundary", "protected_area"), ("leisure", "park"), ("aeroway", "aerodrome"), ("leisure", "nature_reserve")}


def category(r):
    a = r.get("server_answer") or {}
    tags = a.get("tags") or {}
    o = r.get("offline_doc") or {}
    q = r["q"]
    center = a.get("center") or [0, 0]
    if r["set"] == "airmail-eval" and r["id"] in BBOX_CASES:
        return "API: the device has no bbox parameter"
    if a.get("kind") == "admin" and not (WA[0] <= center[0] <= WA[2] and WA[1] <= center[1] <= WA[3]):
        return "WOF scope: only the region's areas are on the device"
    if r.get("found") == "missing":
        if "waterway" in tags:
            return "missing: waterway layer not indexed by the device"
        if any(k == "power" for k in tags):
            return "missing: power=* excluded from tiles by contract"
        if a.get("kind") == "postcode":
            return "missing: postcodes only from addresses, not from POIs' addr:postcode"
        if any((k, v) in LABEL_LAYER_TAGS for k, v in tags.items()):
            return "ranking: label-layer feature without cardinal:importance (and split duplicates)"
        if tags.get("building") or any(k.startswith("addr:") for k in tags) or a.get("address"):
            return "missing: named building/address-bearing element without its name in tiles"
        return "missing: other"
    imp_s, imp_o = (r.get("importance") or [None, None])
    if imp_s is not None and imp_o is not None and imp_s - imp_o > 0.02:
        return "ranking: lower importance offline (no cardinal:importance on that layer)"
    if a.get("kind") == "street" and not o.get("admins"):
        return "different admins: street has no areas offline (tile-majority rule)"
    if a.get("kind") == "postcode":
        top = r.get("offline_top") or []
        if top and top[0]["kind"] == "postcode" and (top[0]["name"] or "").startswith(a.get("name") or "?"):
            return "ranking: ZIP+4 postcodes tie with the 5-digit code"
    if a.get("kind") == "street":
        names = [t["name"] for t in (r.get("offline_top") or [])[: (r.get("offline_rank") or 1)]]
        if all(n == a.get("name") for n in names):
            return "ranking: equal-score same-name streets in another order"
    if a.get("kind") == "address":
        return "ranking: the street outranks its own address"
    return "unclassified"


# search.tsv cases with a bbox (the device ignores it); filled from the cases file.
BBOX_CASES = set()


def main():
    rows = [json.loads(line) for line in open(sys.argv[1])]
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import bench_quality

    BBOX_CASES.update(c["id"] for c in bench_quality.airmail_eval_cases() if "bbox" in c["params"])
    groups = defaultdict(list)
    for r in rows:
        groups[category(r)].append(r)
    print(f"{len(rows)} cases only the server passes\n")
    for name, rs in sorted(groups.items(), key=lambda kv: -len(kv[1])):
        sets = defaultdict(int)
        for r in rs:
            sets[r["set"]] += 1
        print(f"{len(rs):3d}  {name}  ({', '.join(f'{k} {v}' for k, v in sorted(sets.items()))})")
        for r in rs[:4]:
            a = r.get("server_answer") or {}
            o = r.get("offline_doc") or {}
            imp = r.get("importance")
            imp = f" importance server {imp[0]:.3f} offline {imp[1]:.3f}" if imp else ""
            top = ", ".join(f"{t['name']} ({t['kind']})" for t in (r.get("offline_top") or [])[:2])
            print(f"       {r['q']!r}: server -> {a.get('name')} ({a.get('kind')} {a.get('id')}){imp}; offline top: {top or 'nothing'}"
                  + (f"; offline admins {[x[1] for x in o.get('admins') or []][:2]}" if o else ""))
    print()


if __name__ == "__main__":
    main()
