#!/usr/bin/env python3
"""Run Pelias's acceptance tests (https://github.com/pelias/acceptance-tests)
against airmail's /v1/search.

Pelias's API isn't airmail's, so this translates: each test's input becomes
airmail parameters, and each airmail result becomes Pelias-style properties
(layer, name, the admin hierarchy, region_a, country_a, street,
housenumber, postalcode) that the test's expectations are matched against.

Usage:
    git clone --depth 1 https://github.com/pelias/acceptance-tests
    python3 -I eval/pelias_acceptance.py http://127.0.0.1:3000 acceptance-tests \
        [--out results.json] [--file NAME ...] [--failures]

Only search and autocomplete tests with text are run (airmail has no reverse,
structured or place-by-ID endpoint in Pelias's form). Approximations:
- autocomplete sends prefix=true&typos=true, as Cardinal's app does; search
  sends neither.
- lang is en unless a test sets it, as Pelias's API defaults to English.
- boundary.country and layers filter a larger result set afterwards, since
  airmail has no such parameters; boundary.rect maps to bbox, and
  boundary.circle to its bounding box.
- Labels match leniently: the first part must be the name, and every other
  part one of the result's admin names, abbreviations, or its country's
  name or codes, so formatting differences don't count.
- Names match ignoring words that say what kind of area a place is (Maui
  County is Maui, as Who's On First's English names have it), and an
  address's name matches with its words in any order, since its format
  varies by country (Clayallee 170, 170 Clayallee).
- Expected keys that are Pelias internals (gid, source, id, confidence,
  accuracy, match_type, ...) are ignored, and counted.
- Each expected properties object must match one of the first
  priorityThresh results (default 1), as Pelias's fuzzy-tester does.
"""

import argparse
import concurrent.futures
import json
import math
import os
import sys
import unicodedata
import urllib.parse
import urllib.request

ISO = {c["alpha_2"]: c for c in json.load(open("/usr/share/iso-codes/json/iso_3166-1.json"))["3166-1"]}
ALPHA3 = {c["alpha_3"]: c for c in ISO.values()}

IGNORED_KEYS = {
    "gid", "source", "source_id", "id", "confidence", "accuracy", "match_type",
    "postalcode_gid", "country_gid", "region_gid", "locality_gid", "county_gid",
    "distance",
}
ADMIN_LAYERS = {
    "country", "dependency", "macroregion", "region", "macrocounty", "county",
    "localadmin", "locality", "borough", "macrohood", "neighbourhood",
}


def norm(value):
    text = unicodedata.normalize("NFKC", str(value)).casefold().strip()
    return " ".join(text.split())


def properties(result):
    """A result as Pelias-style properties."""
    props = {"name": result.get("name") or ""}
    kind = result["kind"]
    wof = result.get("wof") or {}
    if kind == "poi":
        props["layer"] = "venue"
    elif kind == "address":
        props["layer"] = "address"
    elif kind == "street":
        props["layer"] = "street"
        props["street"] = props["name"]
    elif kind == "postcode":
        props["layer"] = "postalcode"
        props["postalcode"] = props["name"]
    else:
        props["layer"] = wof.get("placetype", "admin")
    admins = list(result.get("admins") or [])
    if kind == "admin":
        admins.insert(0, {"placetype": wof.get("placetype"), "name": result.get("name"), "abbreviation": wof.get("abbreviation")})
    for admin in admins:
        placetype = admin.get("placetype")
        if placetype in ADMIN_LAYERS and placetype not in props:
            props[placetype] = admin.get("name")
            if admin.get("abbreviation"):
                props[placetype + "_a"] = admin["abbreviation"]
    country = ISO.get((result.get("country_code") or "").upper())
    if country:
        props["country_a"] = country["alpha_3"]
        props.setdefault("country", country.get("common_name", country["name"]))
    address = result.get("address") or {}
    if address.get("street"):
        props["street"] = address["street"]
    if address.get("housenumber"):
        props["housenumber"] = address["housenumber"]
    if address.get("postcode") and "postalcode" not in props:
        props["postalcode"] = address["postcode"]
    if kind == "address" and not props["name"]:
        props["name"] = " ".join(p for p in (address.get("housenumber"), address.get("street")) if p)
    # Every name the label's later parts may be.
    known = {norm(a.get("name")) for a in admins if a.get("name")}
    known |= {bare(a.get("name")) for a in admins if a.get("name")}
    known |= {norm(a["abbreviation"]) for a in admins if a.get("abbreviation")}
    if country:
        known |= {norm(country[k]) for k in ("alpha_2", "alpha_3", "name", "common_name", "official_name") if country.get(k)}
        if country["alpha_2"] == "US":
            known |= {"usa", "united states"}
    props["_label_parts"] = known
    props["_center"] = result.get("center")
    return props


AREA_TYPE_WORDS = {"county", "parish", "borough", "municipality", "province", "region", "district", "prefecture"}


def bare(value):
    """A name without words saying what kind of area it is."""
    words = [w for w in norm(value).replace(",", " ").split() if w not in AREA_TYPE_WORDS]
    return " ".join(words) or norm(value)


def names_match(key, expected, props):
    actual = props.get(key, "")
    if norm(actual) == norm(expected) or bare(actual) == bare(expected):
        return True
    return key == "name" and props.get("layer") == "address" and sorted(norm(actual).split()) == sorted(norm(expected).split())


def label_matches(expected, props):
    parts = [norm(p) for p in str(expected).split(",")]
    if not parts or not names_match("name", parts[0], props):
        return False
    return all(p in props["_label_parts"] or bare(p) in props["_label_parts"] for p in parts[1:])


def matches(expected, props, ignored):
    for key, value in expected.items():
        if key in IGNORED_KEYS:
            ignored[key] = ignored.get(key, 0) + 1
            continue
        if key == "label":
            if not label_matches(value, props):
                return False
        elif key in ("country_a",) and isinstance(value, str) and len(value) == 2:
            if norm(props.get(key, "")) != norm(ISO.get(value.upper(), {}).get("alpha_3", value)):
                return False
        elif not names_match(key, value, props):
            return False
    return True


def distance_m(a, b):
    lon1, lat1, lon2, lat2 = map(math.radians, (a[0], a[1], b[0], b[1]))
    h = math.sin((lat2 - lat1) / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin((lon2 - lon1) / 2) ** 2
    return 6371008.8 * 2 * math.asin(math.sqrt(h))


def request(base, endpoint, given, limit):
    params = {"q": given.get("text", ""), "limit": str(limit)}
    params["lang"] = given.get("lang") or "en"
    if endpoint == "autocomplete":
        params["prefix"] = "true"
        params["typos"] = "true"
    if "focus.point.lat" in given:
        params["focus"] = f"{given['focus.point.lon']},{given['focus.point.lat']}"
    if "boundary.rect.min_lon" in given:
        params["bbox"] = ",".join(str(given[f"boundary.rect.{k}"]) for k in ("min_lon", "min_lat", "max_lon", "max_lat"))
    elif "boundary.circle.lat" in given:
        lat, lon = float(given["boundary.circle.lat"]), float(given["boundary.circle.lon"])
        km = float(given.get("boundary.circle.radius", 50))
        dlat = km / 111.0
        dlon = km / (111.0 * max(math.cos(math.radians(lat)), 0.01))
        params["bbox"] = f"{lon - dlon},{lat - dlat},{lon + dlon},{lat + dlat}"
    url = f"{base}/v1/search?{urllib.parse.urlencode(params)}"
    try:
        with urllib.request.urlopen(url, timeout=30) as response:
            return json.load(response)["results"]
    except Exception as error:  # noqa: BLE001 - reported per test
        return error


def wanted(props, given):
    layers = given.get("layers")
    if layers:
        allowed = {l.strip() for l in str(layers).split(",")}
        if "coarse" in allowed:
            allowed |= ADMIN_LAYERS
        if props["layer"] not in allowed:
            return False
    country = given.get("boundary.country")
    if country:
        codes = {c.strip().upper() for c in str(country).split(",")}
        codes |= {ALPHA3[c]["alpha_2"] for c in list(codes) if c in ALPHA3}
        if props.get("country_a") not in {ISO[c]["alpha_3"] for c in codes if c in ISO}:
            return False
    return True


def run_test(base, endpoint, test, file_thresh, ignored):
    given = test.get("in", {})
    expected = test.get("expected") or {}
    thresh = int(expected.get("priorityThresh", test.get("priorityThresh", file_thresh)))
    filtered = "layers" in given or "boundary.country" in given
    limit = 50 if filtered else max(thresh, 10)
    results = request(base, endpoint, given, limit)
    if isinstance(results, Exception):
        return False, f"error: {results}", []
    props = [p for p in map(properties, results) if wanted(p, given)][:max(thresh, 10)]
    top = props[:thresh]
    for item in expected.get("properties") or []:
        if not isinstance(item, dict):
            continue
        if not any(matches(item, p, ignored) for p in top):
            return False, "no match for " + json.dumps(item, ensure_ascii=False), props
    if "coordinates" in expected:
        targets = expected["coordinates"]
        if targets and not isinstance(targets[0], list):
            targets = [targets]
        thresh_m = float(expected.get("distanceThresh", test.get("distanceThresh", 500)))
        for target in targets:
            if not any(p["_center"] and distance_m(p["_center"], target) <= thresh_m for p in top):
                return False, f"nothing within {thresh_m} m of {target}", props
    return True, "", props


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("base")
    parser.add_argument("tests", help="a checkout of pelias/acceptance-tests")
    parser.add_argument("--out", help="write per-test results here as JSON")
    parser.add_argument("--file", action="append", help="only these test files (names without .json)")
    parser.add_argument("--failures", action="store_true", help="print each failure")
    parser.add_argument("--jobs", type=int, default=16, help="tests run at once")
    args = parser.parse_args()
    directory = os.path.join(args.tests, "test_cases")
    ignored, records, totals = {}, [], {}
    tests = []
    for name in sorted(os.listdir(directory)):
        stem = name[:-5]
        if args.file and stem not in args.file:
            continue
        suite = json.load(open(os.path.join(directory, name)))
        endpoint = suite.get("endpoint", "search")
        if endpoint not in ("search", "autocomplete"):
            continue
        for test in suite["tests"]:
            if not str((test.get("in") or {}).get("text", "")).strip():
                continue  # Reverse geocoding by point, which airmail doesn't do.
            tests.append((stem, endpoint, test, suite.get("priorityThresh", 1)))

    def run(item):
        stem, endpoint, test, thresh = item
        own_ignored = {}
        return run_test(args.base, endpoint, test, thresh, own_ignored), own_ignored

    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        outcomes = list(pool.map(run, tests))
    for (stem, endpoint, test, _), ((ok, why, props), own_ignored) in zip(tests, outcomes):
        for key, count in own_ignored.items():
            ignored[key] = ignored.get(key, 0) + count
        upstream = test.get("status") or "?"
        records.append({"file": stem, "id": test.get("id"), "upstream": upstream, "ok": ok, "why": why,
                        "in": test.get("in"), "top": [{k: v for k, v in p.items() if not k.startswith("_")} for p in props[:3]]})
        t = totals.setdefault(stem, [0, 0])
        t[0] += ok
        t[1] += 1
        if args.failures and not ok and upstream == "pass":
            top = " | ".join(f"{p['name']} ({p['layer']})" for p in props[:3])
            print(f"FAIL {stem}#{test.get('id')} {json.dumps(test.get('in'), ensure_ascii=False)}: {why} -> {top}")
    for stem, (ok, n) in totals.items():
        print(f"{ok:4d}/{n:<4d} {stem}")
    up = [r for r in records if r["upstream"] == "pass"]
    print(f"all: {sum(r['ok'] for r in records)}/{len(records)}; "
          f"of those Pelias passes: {sum(r['ok'] for r in up)}/{len(up)}")
    if ignored:
        print("ignored expected keys:", ", ".join(f"{k}×{v}" for k, v in sorted(ignored.items())))
    if args.out:
        json.dump(records, open(args.out, "w"), ensure_ascii=False, indent=1)


if __name__ == "__main__":
    sys.exit(main())
