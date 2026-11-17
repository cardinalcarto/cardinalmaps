"""Compares the offline (tile-built, on-device) geocoder with the server geocoder on the
same data, through the same `/v1/search` API: the server's airmail_service, and the
device's index behind `offline_serve` (this directory).

    # Each system, one at a time (requests are sequential, so latencies compare):
    python3 -I -B bench_quality.py run --base http://127.0.0.1:3930 --out <dir>/server \
        --pelias <acceptance-tests checkout> --sample <sample.jsonl>
    python3 -I -B bench_quality.py run --base http://127.0.0.1:3920 --out <dir>/offline ...
    # Pass rates, recall per kind, latencies, and the cases only one side passes:
    python3 -I -B bench_quality.py compare <dir>/server <dir>/offline
    # For each case only the server passes: is the server's right answer in the offline
    # index (by ID, else by name near it), and how does it differ there?
    python3 -I -B bench_quality.py diagnose <dir>/server <dir>/offline --server-base ... \
        --offline-base ... [--offline-notagger-base ...] --out <dir>/diagnosis.jsonl

Sets:
- airmail-eval: airmail/eval/search.tsv's `wa` cases (search_eval's matching).
- flagged: queries an earlier run flagged.
- pelias-wa: the Pelias acceptance tests whose input or expected answer is in Washington
  (pelias_acceptance's matching).
- categorical-eng: airmail/categorical/eval/queries.eng.tsv, focused on Seattle; a result
  is relevant if it has the top-level tag, a listed cuisine, or a listed diet.
- sample-bare / sample-area: documents sampled from the server index (sample_docs),
  searched by name or address with the focus on them (bare), or with their locality
  appended and no focus (area). Found = the same document in the top k.

Every query is sent with lang=en unless it sets one: the server uses a categorical model
only when given a language, the device always uses its one model.
"""

import argparse
import json
import math
import os
import statistics
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO / "airmail" / "eval"))
import pelias_acceptance  # noqa: E402
import search_eval  # noqa: E402

SEATTLE = (-122.3321, 47.6062)
# Washington's bounding box, as the offline region's WOF fetch used.
WA = (-124.85, 45.54, -116.91, 49.01)

_local = threading.local()


def timed_fetch(base_url, params):
    """GET a /v1 endpoint; the parsed JSON (None on 404), and the time it took."""
    url = f"{base_url}?{urllib.parse.urlencode(params)}"
    start = time.perf_counter()
    try:
        with urllib.request.urlopen(url, timeout=60) as response:
            body = json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            body = None
        else:
            raise
    elapsed = (time.perf_counter() - start) * 1000
    if HYDRATE_BASE and body:
        for result in body.get("results") or []:
            hydrate(result)
    _local.last = {"ms": elapsed, "body": body, "params": params}
    return body


# Pointer results have only their names; the app reads the rest of their tags from the
# tile. The bench tiles and the server index come from the same extract with the same
# (synthetic) poi_ids, so the server's tags for that ID stand in for the tile's.
HYDRATE_BASE = None
_hydrated = {}


def hydrate(result):
    if result.get("pointer") is None:
        return
    pid = result["id"]
    if pid not in _hydrated:
        url = f"{HYDRATE_BASE}/v1/place?{urllib.parse.urlencode({'id': pid})}"
        try:
            with urllib.request.urlopen(url, timeout=60) as response:
                found = json.load(response).get("result")
        except urllib.error.HTTPError:
            found = None
        _hydrated[pid] = found
    found = _hydrated[pid]
    if found is not None:
        osm = result.setdefault("osm", {})
        osm["tags"] = {**((found.get("osm") or {}).get("tags") or {}), **(osm.get("tags") or {})}
        result["hydrated"] = True
        if not result.get("address"):
            result["address"] = found.get("address")


def trim(result):
    """What's kept of a result for comparing and diagnosing."""
    tags = (result.get("osm") or {}).get("tags") or {}
    return {
        "id": result.get("id"),
        "kind": result.get("kind"),
        "name": result.get("name"),
        "center": result.get("center"),
        "address": result.get("address"),
        "admins": [(a.get("placetype"), a.get("name")) for a in result.get("admins") or []],
        "importance": result.get("importance"),
        "score": result.get("score"),
        "pointer": result.get("pointer") is not None,
        "tags": tags,
        "placetype": (result.get("wof") or {}).get("placetype"),
    }


def last_call():
    call = getattr(_local, "last", None) or {}
    body = call.get("body") or {}
    return {
        "ms": call.get("ms"),
        "search_ms": (body.get("query") or {}).get("search_ms"),
        "category_language": (body.get("query") or {}).get("category_language"),
        "top": [trim(r) for r in (body.get("results") or [])[:10]],
    }


# search_eval and pelias_acceptance make their own requests; time and keep them.
search_eval.fetch = timed_fetch


def _pelias_request(base, endpoint, given, limit):
    given = dict(given)
    params = {"q": given.get("text", ""), "limit": str(limit), "lang": given.get("lang") or "en"}
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
    try:
        body = timed_fetch(f"{base}/v1/search", params)
        return body["results"]
    except Exception as error:  # noqa: BLE001 - reported per test
        return error


# Same as pelias_acceptance.request, through timed_fetch (kept in step by hand: it only
# builds parameters).
pelias_acceptance.request = _pelias_request


def in_wa(lon, lat):
    return WA[0] <= lon <= WA[2] and WA[1] <= lat <= WA[3]


def km(a, b):
    return search_eval.haversine_km(a[1], a[0], b[1], b[0])


# ---- cases ---------------------------------------------------------------------------

FLAGGED = [
    # Missed offline in the desktop e2e: the mountain, or its national park.
    ("mt rainier", {}, "name~rainier;near=46.8529,-121.7604,20", None),
    # A postcode outranked by POIs in it with the tagger.
    ("98101", {}, "kind=postcode;name=98101", None),
    # The address itself (or a POI with that address), at Pike Place.
    ("1530 post alley", {}, "near=47.6090,-122.3415,0.3", "1530"),
]


def airmail_eval_cases():
    path = REPO / "airmail" / "eval" / "search.tsv"
    cases = []
    for i, (_, query, params, conditions) in enumerate(search_eval.load_cases(path, "wa")):
        cases.append({"set": "airmail-eval", "id": f"airmail-eval#{i}", "q": query, "params": params, "conditions": conditions})
    for i, (query, params, expect, housenumber) in enumerate(FLAGGED):
        cases.append({"set": "flagged", "id": f"flagged#{i}", "q": query, "params": params,
                      "conditions": search_eval.parse_expect(expect), "housenumber": housenumber})
    return cases


def pelias_wa(test):
    """Whether a Pelias test is about Washington: its focus or expected point is in the
    state, its expected properties name the state, or its text does."""
    given = test.get("in") or {}
    expected = test.get("expected") or {}
    if "focus.point.lat" in given and in_wa(float(given["focus.point.lon"]), float(given["focus.point.lat"])):
        return True
    coords = expected.get("coordinates")
    if coords:
        targets = coords if isinstance(coords[0], list) else [coords]
        if any(in_wa(float(t[0]), float(t[1])) for t in targets):
            return True
    for props in expected.get("properties") or []:
        if not isinstance(props, dict):
            continue
        region = str(props.get("region", "")).casefold()
        region_a = str(props.get("region_a", "")).casefold()
        label = str(props.get("label", "")).casefold()
        dc = ", dc" in label or "district of columbia" in label
        if region == "washington" or region_a == "wa" or (not dc and (", wa," in label or ", washington," in label)):
            return True
        if str(props.get("locality", "")).casefold() in WA_PLACES:
            return True
    # "Washington" alone is mostly DC in these tests; Washington's places by name.
    text = " " + str(given.get("text", "")).casefold().replace(",", " ") + " "
    return any(f" {place} " in text for place in WA_PLACES)


# Washington's biggest places, for picking Pelias tests about the state by their text.
WA_PLACES = {"seattle", "spokane", "tacoma", "vancouver wa", "bellevue wa", "kent wa", "everett", "renton",
             "yakima", "spokane valley", "kirkland", "bellingham", "auburn wa", "kennewick", "pasco", "redmond",
             "olympia", "bremerton", "walla walla", "wenatchee", "puyallup", "lynnwood", "issaquah"}


def pelias_cases(checkout):
    directory = os.path.join(checkout, "test_cases")
    cases = []
    for name in sorted(os.listdir(directory)):
        suite = json.load(open(os.path.join(directory, name)))
        endpoint = suite.get("endpoint", "search")
        if endpoint not in ("search", "autocomplete"):
            continue
        for test in suite["tests"]:
            if not str((test.get("in") or {}).get("text", "")).strip() or not pelias_wa(test):
                continue
            cases.append({"set": "pelias-wa", "id": f"pelias-wa#{name[:-5]}#{test.get('id')}", "endpoint": endpoint,
                          "test": test, "thresh": suite.get("priorityThresh", 1), "upstream": test.get("status") or "?",
                          "q": test["in"]["text"]})
    return cases


def categorical_cases():
    path = REPO / "airmail" / "categorical" / "eval" / "queries.eng.tsv"
    cases = []
    for i, line in enumerate(path.read_text().splitlines()):
        if not line.strip() or line.startswith("#"):
            continue
        parts = line.split("\t") + ["", "", ""]
        cases.append({"set": "categorical-eng", "id": f"categorical-eng#{parts[0]}", "q": parts[0], "top": parts[1],
                      "cuisine": [c for c in parts[2].split(";") if c], "diet": [d for d in parts[3].split(";") if d]})
    return cases


def sample_query(doc, with_area):
    kind = doc["kind"]
    if kind == "address":
        text = " ".join(p for p in (doc.get("housenumber"), doc.get("street")) if p)
    else:
        text = doc.get("name") or ""
    if with_area and kind in ("poi", "street", "address"):
        area = next((a["name"] for a in doc["admins"] if a["placetype"] in ("locality", "localadmin")), None) or next(
            (a["name"] for a in doc["admins"] if a["placetype"] == "county"), None)
        if area:
            text = f"{text}, {area}"
    if with_area and kind == "admin":
        parent = next((a["name"] for a in doc["admins"] if a["placetype"] in ("locality", "county", "region") and a["name"] != text), None)
        if parent:
            text = f"{text}, {parent}"
    return text


def sample_cases(path):
    cases = []
    for line in Path(path).read_text().splitlines():
        doc = json.loads(line)
        for variant in ("bare", "area"):
            text = sample_query(doc, variant == "area")
            if not text.strip():
                continue
            params = {}
            if variant == "bare":
                params["focus"] = f"{doc['center'][0]},{doc['center'][1]}"
            cases.append({"set": f"sample-{variant}", "id": f"sample-{variant}#{doc['id']}#{doc['kind']}", "q": text,
                          "params": params, "doc": doc})
    return cases


def all_cases(args):
    cases = airmail_eval_cases()
    if args.pelias:
        cases += pelias_cases(args.pelias)
    cases += categorical_cases()
    if args.sample:
        cases += sample_cases(args.sample)
    return cases


# ---- evaluation ----------------------------------------------------------------------

# How far a result may be from a sampled document and still be it, by kind (km): streets
# are joined from pieces differently, areas have different label points.
SAME_KM = {"poi": 0.15, "address": 0.05, "street": 2.0, "admin": 3.0, "postcode": 5.0}


def same_doc(doc, result):
    if result["id"] == doc["id"]:
        return True
    if result["kind"] != doc["kind"] or not result.get("center"):
        return False
    if km(result["center"], doc["center"]) > SAME_KM[doc["kind"]]:
        return False
    if doc["kind"] == "address":
        address = result.get("address") or {}
        return (address.get("housenumber") or "").casefold() == (doc.get("housenumber") or "").casefold() and (
            address.get("street") or "").casefold() == (doc.get("street") or "").casefold()
    return (result.get("name") or "").casefold() == (doc.get("name") or "").casefold()


def relevant(case, result):
    tags = result.get("tags") or {}
    key, _, value = case["top"].partition("=")
    if value in [v.strip() for v in tags.get(key, "").split(";")]:
        return True
    cuisines = [c.strip() for c in tags.get("cuisine", "").split(";")]
    if any(c in cuisines for c in case["cuisine"]):
        return True
    return any(tags.get(f"diet:{d}") in ("yes", "only") for d in case["diet"])


def evaluate(base, case):
    """One case against one system: its rank (1-based, None if not in the top 10), whether
    it passes, and what came back."""
    record = {"id": case["id"], "set": case["set"], "q": case["q"]}
    kind = case["set"]
    if kind in ("airmail-eval", "flagged"):
        params = {"lang": "en", **case["params"]}
        results = search_eval.search(base, case["q"], params)
        call = last_call()
        rank = None
        for i, r in enumerate(results[:10]):
            if not search_eval.matches(r, case["conditions"]):
                continue
            hn = case.get("housenumber")
            if hn and ((call["top"][i].get("address") or {}).get("housenumber") or "") != hn:
                continue
            rank = i + 1
            break
        record.update(rank=rank, ok=rank == 1)
    elif kind == "pelias-wa":
        ok, why, props = pelias_acceptance.run_test(base, case["endpoint"], case["test"], case["thresh"], {})
        call = last_call()
        record.update(ok=ok, why=why, upstream=case["upstream"], rank=None)
    elif kind == "categorical-eng":
        timed_fetch(f"{base}/v1/search", {"q": case["q"], "lang": "en", "focus": f"{SEATTLE[0]},{SEATTLE[1]}", "limit": "10"})
        call = last_call()
        flags = [relevant(case, r) for r in call["top"]]
        rank = next((i + 1 for i, f in enumerate(flags) if f), None)
        record.update(rank=rank, ok=rank == 1, precision5=sum(flags[:5]) / 5)
    else:
        params = {"q": case["q"], "lang": "en", "limit": "10", **case["params"]}
        timed_fetch(f"{base}/v1/search", params)
        call = last_call()
        rank = next((i + 1 for i, r in enumerate(call["top"]) if same_doc(case["doc"], r)), None)
        record.update(rank=rank, ok=rank == 1, kind=case["doc"]["kind"])
    record.update(call)
    return record


def run(args):
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    cases = all_cases(args)
    # Warm up: the first searches open segments and page in the index.
    for q in ("seattle", "coffee", "1600 pennsylvania"):
        timed_fetch(f"{args.base}/v1/search", {"q": q, "lang": "en"})
    with open(out / "cases.jsonl", "w") as f:
        for i, case in enumerate(cases):
            record = evaluate(args.base, case)
            f.write(json.dumps(record, ensure_ascii=False) + "\n")
            if (i + 1) % 100 == 0:
                print(f"{i + 1}/{len(cases)}", file=sys.stderr)
    print(f"{len(cases)} cases -> {out / 'cases.jsonl'}")


# ---- comparison ----------------------------------------------------------------------


def load(path):
    return {r["id"]: r for r in map(json.loads, open(Path(path) / "cases.jsonl"))}


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]


def compare(args):
    server, offline = load(args.server), load(args.offline)
    sets = {}
    for r in server.values():
        sets.setdefault(r["set"], []).append(r["id"])
    print("== pass rates (hits@1; Pelias: its own pass rule)\nset\tn\tserver\toffline\tboth\tserver-only\toffline-only")
    for name, ids in sets.items():
        s = sum(server[i]["ok"] for i in ids)
        o = sum(offline[i]["ok"] for i in ids)
        both = sum(server[i]["ok"] and offline[i]["ok"] for i in ids)
        print(f"{name}\t{len(ids)}\t{s}\t{o}\t{both}\t{s - both}\t{o - both}")
        if name == "pelias-wa":
            up = [i for i in ids if server[i].get("upstream") == "pass"]
            print(f"  (of those Pelias passes upstream: n={len(up)} server {sum(server[i]['ok'] for i in up)} "
                  f"offline {sum(offline[i]['ok'] for i in up)})")
        if name in ("airmail-eval", "categorical-eng", "flagged"):
            for label, data in (("server", server), ("offline", offline)):
                h5 = sum(1 for i in ids if data[i]["rank"] and data[i]["rank"] <= 5)
                mrr = sum(1 / data[i]["rank"] for i in ids if data[i]["rank"]) / len(ids)
                extra = ""
                if name == "categorical-eng":
                    extra = f" precision@5 {statistics.mean(data[i]['precision5'] for i in ids):.2f}"
                print(f"  {label}: hits@5 {h5}/{len(ids)} MRR {mrr:.3f}{extra}")
    print("\n== recall per kind (sampled server documents)\nset\tkind\tn\tserver@1\tserver@5\toffline@1\toffline@5")
    for name in ("sample-bare", "sample-area"):
        kinds = {}
        for i in sets.get(name, []):
            kinds.setdefault(server[i]["kind"], []).append(i)
        for kind, ids in sorted(kinds.items()):
            row = [name, kind, str(len(ids))]
            for data in (server, offline):
                for k in (1, 5):
                    row.append(f"{sum(1 for i in ids if data[i]['rank'] and data[i]['rank'] <= k) / len(ids):.2f}")
            print("\t".join(row))
    print("\n== latency over every query (ms; client-side over HTTP, sequential)")
    for label, data in (("server", server), ("offline", offline)):
        ms = [r["ms"] for r in data.values() if r.get("ms") is not None]
        line = f"{label}: n={len(ms)} p50 {pct(ms, 50):.1f} p95 {pct(ms, 95):.1f} max {max(ms):.1f}"
        inproc = [r["search_ms"] for r in data.values() if r.get("search_ms") is not None]
        if inproc:
            line += f"; in-process search p50 {pct(inproc, 50):.1f} p95 {pct(inproc, 95):.1f}"
        print(line)
    print("\n== queries whose top 10 repeats a result ID (or, without IDs, a kind+name within 1 km)")
    for label, data in (("server", server), ("offline", offline)):
        repeated = 0
        for r in data.values():
            seen, dup = [], False
            for t in r["top"]:
                key = t["id"] if not str(t["id"]).startswith("point:") else None
                for other in seen:
                    if key and key == other["key"]:
                        dup = True
                    elif not key and t["kind"] == other["t"]["kind"] and t["name"] == other["t"]["name"] and t["center"] and other["t"]["center"] and km(t["center"], other["t"]["center"]) < 1:
                        dup = True
                seen.append({"key": key, "t": t})
            repeated += dup
        print(f"{label}: {repeated}/{len(data)}")
    print("\n== cases only the server passes")
    for name, ids in sets.items():
        for i in ids:
            if server[i]["ok"] and not offline[i]["ok"]:
                top = " | ".join(f"{t['name']} ({t['kind']})" for t in offline[i]["top"][:3])
                print(f"{i}\t{server[i]['q']!r}\toffline rank {offline[i]['rank']}\t-> {top}")
    print("\n== cases only offline passes")
    for name, ids in sets.items():
        for i in ids:
            if offline[i]["ok"] and not server[i]["ok"]:
                top = " | ".join(f"{t['name']} ({t['kind']})" for t in server[i]["top"][:3])
                print(f"{i}\t{server[i]['q']!r}\tserver rank {server[i]['rank']}\t-> {top}")


# ---- diagnosis -----------------------------------------------------------------------

NAME_PREFIXES = ("name", "alt_name", "official_name", "short_name", "loc_name", "int_name", "old_name")


def name_tags(tags):
    return {k: v for k, v in (tags or {}).items() if k.split(":")[0] in NAME_PREFIXES}


def server_answer(case, server_record):
    """The result in the server's top that made it pass (or its top result)."""
    top = server_record["top"]
    if server_record.get("rank"):
        return top[server_record["rank"] - 1]
    return top[0] if top else None


def find_offline(base, answer):
    """The server's answer in the offline index (as stored, not hydrated): by ID, else by
    its name near it."""
    global HYDRATE_BASE
    saved, HYDRATE_BASE = HYDRATE_BASE, None
    try:
        return _find_offline(base, answer)
    finally:
        HYDRATE_BASE = saved


def _find_offline(base, answer):
    if answer["id"] and not answer["id"].startswith("point:"):
        body = timed_fetch(f"{base}/v1/place", {"id": answer["id"], "lang": "en"})
        if body and body.get("result"):
            return trim(body["result"]), "id"
    text = answer.get("name")
    if not text and answer.get("address"):
        text = " ".join(p for p in (answer["address"].get("housenumber"), answer["address"].get("street")) if p)
    if not text:
        return None, "unnamed"
    body = timed_fetch(f"{base}/v1/search", {"q": text, "lang": "en", "limit": "20",
                                             "focus": f"{answer['center'][0]},{answer['center'][1]}"})
    doc = {"id": answer["id"], "kind": answer["kind"], "center": answer["center"], "name": answer.get("name"),
           "housenumber": (answer.get("address") or {}).get("housenumber"),
           "street": (answer.get("address") or {}).get("street")}
    for r in (body or {}).get("results") or []:
        t = trim(r)
        if same_doc(doc, t):
            return t, "name"
    # Same name anywhere nearby, of another kind.
    for r in (body or {}).get("results") or []:
        t = trim(r)
        if (t.get("name") or "").casefold() == (answer.get("name") or "").casefold() and t.get("center") and km(t["center"], answer["center"]) < 1:
            return t, "name-other-kind"
    return None, "missing"


def diagnose(args):
    server, offline = load(args.server), load(args.offline)
    cases = {c["id"]: c for c in all_cases(args)}
    out = open(args.out, "w")
    for i, s in server.items():
        o = offline[i]
        if not s["ok"] or o["ok"]:
            continue
        case = cases[i]
        answer = server_answer(case, s)
        row = {"id": i, "set": s["set"], "q": s["q"], "offline_rank": o["rank"], "server_answer": answer,
               "offline_top": o["top"][:5]}
        if answer:
            found, how = find_offline(args.offline_base, answer)
            row["found"] = how
            row["offline_doc"] = found
            if found:
                sn, on = name_tags(answer["tags"]), name_tags(found["tags"])
                row["names_missing_offline"] = sorted(set(sn) - set(on))
                row["admins_server"] = answer["admins"]
                row["admins_offline"] = found["admins"]
                row["importance"] = [answer["importance"], found["importance"]]
                skip = set(sn) | {k for k in answer["tags"] if k.startswith("cardinal:") or k.startswith("addr:")}
                row["tags_missing_offline"] = sorted(k for k in answer["tags"] if k not in skip and k not in found["tags"])
                row["kind"] = [answer["kind"], found["kind"]]
                row["distance_km"] = km(answer["center"], found["center"]) if found.get("center") else None
        if args.offline_notagger_base:
            row["offline_notagger_ok"] = evaluate(args.offline_notagger_base, case)["ok"]
        if args.server_notagger_base:
            row["server_notagger_ok"] = evaluate(args.server_notagger_base, case)["ok"]
        out.write(json.dumps(row, ensure_ascii=False) + "\n")
    out.close()


def wof_sample(args):
    """Areas to sample, from the region's stored WOF responses (what the device indexes):
    places whose label point is in Washington, at random, as sample_docs lines."""
    import random
    places = {}
    for file in sorted(Path(args.wof).glob("*.json")):
        for place in json.load(open(file))["places"]:
            places.setdefault(place["id"], place)

    def name(place):
        names = place.get("names") or []
        for lang, tag in (("eng", "preferred"), ("und", "default"), ("eng", "default")):
            for n in names:
                if n[0] == lang and n[1] == tag:
                    return n[3]
        return names[0][3] if names else None

    pool = [p for p in places.values() if not p.get("ancestor_only") and p.get("label") and in_wa(*p["label"]) and name(p)]
    pool.sort(key=lambda p: p["id"])
    random.Random(args.seed).shuffle(pool)
    for place in pool[:args.n]:
        admins = [{"placetype": places[a]["placetype"], "name": name(places[a]), "id": a}
                  for a in place.get("ancestors") or [] if a in places]
        print(json.dumps({"kind": "admin", "id": f"wof:{place['id']}", "name": name(place), "center": place["label"],
                          "placetype": place["placetype"], "admins": admins, "tags": {}}, ensure_ascii=False))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("run", "diagnose", "cases"):
        p = sub.add_parser(name)
        p.add_argument("--pelias", help="a checkout of pelias/acceptance-tests")
        p.add_argument("--sample", help="sample_docs output (JSON lines)")
    sub.choices["run"].add_argument("--base", required=True)
    sub.choices["run"].add_argument("--out", required=True)
    for name in ("run", "diagnose"):
        sub.choices[name].add_argument("--hydrate-base", help="fill pointer results' tags from this server by ID")
    w = sub.add_parser("wof-sample")
    w.add_argument("--wof", required=True)
    w.add_argument("--n", type=int, default=60)
    w.add_argument("--seed", type=int, default=7)
    p = sub.add_parser("compare")
    p.add_argument("server")
    p.add_argument("offline")
    d = sub.choices["diagnose"]
    d.add_argument("server")
    d.add_argument("offline")
    d.add_argument("--offline-base", required=True)
    d.add_argument("--offline-notagger-base")
    d.add_argument("--server-notagger-base")
    d.add_argument("--out", required=True)
    args = parser.parse_args()
    global HYDRATE_BASE
    HYDRATE_BASE = getattr(args, "hydrate_base", None)
    if args.command == "run":
        run(args)
    elif args.command == "compare":
        compare(args)
    elif args.command == "diagnose":
        diagnose(args)
    elif args.command == "wof-sample":
        wof_sample(args)
    else:
        counts = {}
        for c in all_cases(args):
            counts[c["set"]] = counts.get(c["set"], 0) + 1
            if c["set"] == "pelias-wa":
                print(c["id"], json.dumps(c["test"].get("in"), ensure_ascii=False))
        print(counts)


if __name__ == "__main__":
    sys.exit(main())
