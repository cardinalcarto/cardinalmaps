#!/usr/bin/env python3
"""Generates the Thor oracle corpus: route requests that exercise mainline's path search.

Usage: thor-corpus.py <region> <count> <seed> > requests.tsv

Each line is `<id>\t<action>\t<json>`, the input of the `loki` C++ helper (`cpp/loki.cc`),
whose output is the input of the `thor` helper (`cpp/thor.cc`).
The requests are random but reproducible (the seed fixes them):
two to five locations of every type (break, through, via, and break_through),
for the auto, pedestrian (walking, hiking, wheelchair, and blind), and bicycle costings
with their main options, without a time and with depart_at, arrive_by, and invariant times
(at several times of week), with alternates, headings (which leave filtered edges),
exclude locations, and `prioritize_bidirectional`.
Some locations are far from any road or on small islands of the graph,
so some pairs are unroutable.
`current` times aren't used, since mainline's results would depend on when it ran.
"""

import json
import random
import sys

REGIONS = {
    # The Andorra fixture tiles (valhalla-graphtile/fixtures/andorra-tiles).
    "andorra": (1.42, 42.43, 1.78, 42.65),
    # Central Seattle, for the Seattle oracle build.
    "seattle": (-122.42, 47.52, -122.25, 47.70),
}

# Weeks with and without daylight saving time, and a leap day.
DATES = ["2026-11-09", "2026-11-13", "2026-11-15", "2026-07-01", "2026-03-29", "2028-02-29"]
TIMES = ["00:00", "03:15", "07:30", "08:00", "12:00", "16:45", "17:30", "21:59", "23:59"]


def main():
    region, count, seed = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    rng = random.Random(seed)
    minx, miny, maxx, maxy = REGIONS[region]
    centers = [(rng.uniform(minx, maxx), rng.uniform(miny, maxy)) for _ in range(30)]

    def point(spread):
        if rng.random() < 0.2:
            return rng.uniform(minx, maxx), rng.uniform(miny, maxy)
        cx, cy = rng.choice(centers)
        return cx + rng.gauss(0, spread), cy + rng.gauss(0, spread * 0.7)

    def location(spread, kind):
        lon, lat = point(spread)
        loc = {"lat": round(lat, 6), "lon": round(lon, 6)}
        if kind != "break":
            loc["type"] = kind
        r = rng.random
        if r() < 0.15:
            loc["heading"] = rng.randrange(0, 360)
            if r() < 0.5:
                loc["heading_tolerance"] = rng.choice([10, 30, 60])
        if r() < 0.08:
            loc["radius"] = rng.choice([10, 50, 150])
        if r() < 0.08:
            loc["minimum_reachability"] = rng.choice([0, 10, 50])
        if r() < 0.05:
            loc["preferred_side"] = rng.choice(["same", "opposite"])
        if r() < 0.04:
            loc["node_snap_tolerance"] = rng.choice([0, 15, 40])
        if r() < 0.04:
            loc["rank_candidates"] = False
        if r() < 0.04:
            loc["waiting"] = rng.choice([60, 600])
        return loc

    for n in range(count):
        r = rng.random
        costing = rng.choice(["auto", "auto", "auto", "pedestrian", "pedestrian", "bicycle"])
        spread = {"auto": 0.03, "pedestrian": 0.008, "bicycle": 0.015}[costing]
        legs = rng.choice([1, 1, 1, 1, 2, 2, 3, 4])
        kinds = ["break"]
        for _ in range(legs - 1):
            kinds.append(rng.choice(["break", "through", "via", "break_through"]))
        kinds.append("break")
        locations = [location(spread, k) for k in kinds]
        # Some routes start and end close together (trivial, same edge, or connected edges).
        if r() < 0.1:
            a = locations[0]
            locations[-1] = {
                "lat": round(a["lat"] + rng.gauss(0, 0.0004), 6),
                "lon": round(a["lon"] + rng.gauss(0, 0.0004), 6),
            }
        request = {"locations": locations, "costing": costing}

        options = {}
        if costing == "auto":
            if r() < 0.15:
                options["use_highways"] = rng.choice([0.0, 0.2, 1.0])
            if r() < 0.1:
                options["use_tolls"] = rng.choice([0.0, 1.0])
            if r() < 0.1:
                options["shortest"] = True
            if r() < 0.05:
                options["top_speed"] = rng.choice([40, 90])
            if r() < 0.05:
                options["use_distance"] = 0.5
        elif costing == "pedestrian":
            if r() < 0.3:
                options["max_hiking_difficulty"] = rng.choice([1, 3, 6])
            if r() < 0.08:
                options["type"] = rng.choice(["wheelchair", "blind"])
            if r() < 0.1:
                options["walking_speed"] = rng.choice([3.0, 6.5])
            if r() < 0.1:
                options["use_ferry"] = rng.choice([0.0, 1.0])
        else:
            if r() < 0.5:
                options["bicycle_type"] = rng.choice(["road", "hybrid", "cross", "mountain"])
            if r() < 0.2:
                options["use_roads"] = rng.choice([0.0, 0.5, 1.0])
            if r() < 0.2:
                options["use_hills"] = rng.choice([0.0, 1.0])
        if options:
            request["costing_options"] = {costing: options}

        roll = r()
        if roll < 0.3:
            pass
        else:
            kind = 1 if roll < 0.6 else (2 if roll < 0.85 else 3)
            request["date_time"] = {
                "type": kind,
                "value": f"{rng.choice(DATES)}T{rng.choice(TIMES)}",
            }
            if r() < 0.1:
                request["prioritize_bidirectional"] = True
        if r() < 0.12:
            request["alternates"] = rng.choice([1, 2, 3])
        if r() < 0.05:
            lon, lat = point(spread)
            request["exclude_locations"] = [{"lat": round(lat, 6), "lon": round(lon, 6)}]

        action = "optimized_route" if r() < 0.03 and len(locations) > 2 else "route"
        if action == "optimized_route":
            for loc in request["locations"]:
                loc.pop("type", None)
            request.pop("date_time", None)
            request.pop("alternates", None)
        print(f"{region}-{n:05d}\t{action}\t{json.dumps(request, separators=(',', ':'))}")


if __name__ == "__main__":
    main()
