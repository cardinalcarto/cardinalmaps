"""Scores a running airmail service against search.tsv.

    python3 -I eval/search_eval.py --region wa http://127.0.0.1:3000
    python3 -I eval/search_eval.py --region wa https://host/geocoder --verbose

Works with both the legacy `/search` response and `/v1/search`, so a change can
be compared against the release before it. Prints each region's hits@1,
hits@5 and mean reciprocal rank over the top 10 results.
"""

import argparse
import concurrent.futures
import json
import math
import sys
import urllib.parse
import urllib.request
from pathlib import Path


def load_cases(path, region):
    cases = []
    for line in Path(path).read_text().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        case_region, query, params, expect = line.split("\t")
        if region and case_region != region:
            continue
        params = {} if params == "-" else dict(urllib.parse.parse_qsl(params))
        cases.append((case_region, query, params, parse_expect(expect)))
    return cases


def parse_expect(expect):
    conditions = []
    for condition in expect.split(";"):
        if condition.startswith("name="):
            conditions.append(("name", [n.casefold() for n in condition[5:].split("|")]))
        elif condition.startswith("name~"):
            conditions.append(("name~", [n.casefold() for n in condition[5:].split("|")]))
        elif condition.startswith("near="):
            lat, lon, km = (float(x) for x in condition[5:].split(","))
            conditions.append(("near", (lat, lon, km)))
        elif condition.startswith("tag="):
            key, value = condition[4:].split("=", 1)
            conditions.append(("tag", (key, value)))
        elif condition.startswith("kind="):
            conditions.append(("kind", condition[5:]))
        elif condition.startswith("in="):
            conditions.append(("in", condition[3:].casefold()))
        else:
            raise ValueError(f"unknown condition {condition!r}")
    return conditions


def haversine_km(lat1, lon1, lat2, lon2):
    p1, p2 = math.radians(lat1), math.radians(lat2)
    dp, dl = p2 - p1, math.radians(lon2 - lon1)
    a = math.sin(dp / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dl / 2) ** 2
    return 6371 * 2 * math.asin(math.sqrt(a))


def search(base, query, params):
    """Results as dicts with name, lat, lon, tags and kind (None for legacy)."""
    v1 = fetch(f"{base}/v1/search", {"q": query, **params})
    if v1 is not None:
        return [
            {
                "name": r["name"],
                "lon": r["center"][0],
                "lat": r["center"][1],
                "tags": (r.get("osm") or {}).get("tags", {}),
                "kind": r["kind"],
                "admins": [a["name"].casefold() for a in r["admins"]],
            }
            for r in v1["results"]
        ]
    # The legacy API has one flag for both.
    legacy = dict(params)
    prefix = legacy.pop("prefix", None) == "true"
    typos = legacy.pop("typos", None) == "true"
    if prefix or typos:
        legacy["leniency"] = "true"
    response = fetch(f"{base}/search", {"q": query, **legacy})
    if response is None:
        raise RuntimeError(f"{base} has neither /v1/search nor /search")
    results = []
    for feature in response["features"]:
        tags = dict(feature["tags"])
        results.append(
            {"name": tags.get("name"), "lat": feature["lat"], "lon": feature["lng"], "tags": tags, "kind": None, "admins": None}
        )
    return results


def fetch(url, params):
    request = f"{url}?{urllib.parse.urlencode(params)}"
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise


def matches(result, conditions):
    name = (result["name"] or "").casefold()
    for kind, arg in conditions:
        if kind == "name" and name not in arg:
            return False
        if kind == "name~" and not any(a in name for a in arg):
            return False
        if kind == "near" and haversine_km(arg[0], arg[1], result["lat"], result["lon"]) > arg[2]:
            return False
        if kind == "tag":
            values = [v.strip() for v in result["tags"].get(arg[0], "").split(";")]
            if arg[1] not in values:
                return False
        if kind == "kind" and result["kind"] is not None and result["kind"] != arg:
            return False
        if kind == "in" and result["admins"] is not None and arg not in result["admins"]:
            return False
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("base", help="service URL, without /search")
    parser.add_argument("--region", help="only cases for this region, e.g. wa")
    parser.add_argument("--cases", default=Path(__file__).with_name("search.tsv"))
    parser.add_argument("--verbose", action="store_true", help="print each case's rank and top results")
    parser.add_argument("--jobs", type=int, default=16, help="requests in flight at once")
    args = parser.parse_args()

    cases = list(load_cases(args.cases, args.region))
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        responses = pool.map(lambda case: search(args.base.rstrip("/"), case[1], case[2]), cases)
    totals = {}
    for (region, query, params, conditions), results in zip(cases, responses):
        rank = next((i + 1 for i, r in enumerate(results[:10]) if matches(r, conditions)), None)
        hits1, hits5, rr, n = totals.get(region, (0, 0, 0.0, 0))
        totals[region] = (
            hits1 + (rank == 1),
            hits5 + (rank is not None and rank <= 5),
            rr + (1 / rank if rank else 0),
            n + 1,
        )
        if args.verbose or rank != 1:
            top = " | ".join(str(r["name"]) for r in results[:3])
            flags = "&".join(f"{k}={v}" for k, v in params.items())
            print(f"{rank or '-':>2}  {region}  {query}{'  [' + flags + ']' if flags else ''}  ->  {top}")
    for region, (hits1, hits5, rr, n) in sorted(totals.items()):
        print(f"{region}: hits@1 {hits1}/{n}  hits@5 {hits5}/{n}  MRR {rr / n:.3f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
