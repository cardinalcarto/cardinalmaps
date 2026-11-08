#!/usr/bin/env python3
"""Generates the Loki oracle corpus: requests that exercise mainline's location search.

Usage: loki-corpus.py <region> <count> <seed> > requests.tsv

Each line is `<id>\t<action>\t<json>`, the input of the `loki` C++ helper (`cpp/loki.cc`).
The requests are random but reproducible (the seed fixes them):
locations across the region with every search option Loki reads
(radius, reachability, headings, preferred side, street side tolerances, node snapping,
search cutoffs, search filters, levels, layers, and display coordinates),
for route, locate (verbose or not), trace_route, trace_attributes, and optimized_route,
with exclude locations and exclude polygons, and requests that break the service limits.
Only the auto, pedestrian, and bicycle costings are used, since those are the ones Valinor has.
"""

import json
import math
import random
import sys

REGIONS = {
    # The Andorra fixture tiles (valhalla-graphtile/fixtures/andorra-tiles).
    "andorra": (1.42, 42.43, 1.78, 42.65),
    # Central Seattle, for the Seattle oracle build.
    "seattle": (-122.42, 47.52, -122.25, 47.70),
}

COSTINGS = ["auto", "auto", "pedestrian", "bicycle"]


def main():
    region, count, seed = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    rng = random.Random(seed)
    minx, miny, maxx, maxy = REGIONS[region]
    # Clusters near which most points fall, so that locations are near roads and each other.
    centers = [(rng.uniform(minx, maxx), rng.uniform(miny, maxy)) for _ in range(40)]

    def point(spread=0.01):
        if rng.random() < 0.15:
            return rng.uniform(minx, maxx), rng.uniform(miny, maxy)
        cx, cy = rng.choice(centers)
        return cx + rng.gauss(0, spread), cy + rng.gauss(0, spread * 0.7)

    def location(spread=0.01):
        lon, lat = point(spread)
        loc = {"lat": round(lat, 6), "lon": round(lon, 6)}
        r = rng.random
        if r() < 0.3:
            loc["radius"] = rng.choice([0, 5, 10, 25, 50, 100, 200, 300])
        if r() < 0.25:
            if r() < 0.5:
                loc["minimum_reachability"] = rng.choice([0, 1, 5, 10, 30, 50, 80, 150])
            else:
                if r() < 0.7:
                    loc["minimum_inbound_reachability"] = rng.choice([0, 3, 20, 60, 120])
                if r() < 0.7:
                    loc["minimum_outbound_reachability"] = rng.choice([0, 3, 20, 60, 120])
        if r() < 0.25:
            loc["heading"] = rng.randrange(0, 360)
            if r() < 0.5:
                loc["heading_tolerance"] = rng.choice([0, 10, 30, 45, 90, 180])
        if r() < 0.2:
            loc["preferred_side"] = rng.choice(["same", "opposite", "either"])
            if r() < 0.5:
                loc["street_side_tolerance"] = rng.choice([0, 1, 5, 10, 30])
            if r() < 0.4:
                loc["street_side_max_distance"] = rng.choice([5, 20, 100, 1000, 5000])
            if r() < 0.3:
                loc["street_side_cutoff"] = rng.choice(
                    ["motorway", "trunk", "primary", "secondary", "tertiary", "unclassified",
                     "residential", "service_other"])
        if r() < 0.15:
            loc["node_snap_tolerance"] = rng.choice([0, 1, 5, 15, 40])
        if r() < 0.12:
            loc["search_cutoff"] = rng.choice([10, 50, 200, 1000, 5000, 50000])
        if r() < 0.1:
            loc["display_lat"] = round(loc["lat"] + rng.gauss(0, 0.0003), 6)
            loc["display_lon"] = round(loc["lon"] + rng.gauss(0, 0.0003), 6)
        if r() < 0.05:
            loc["preferred_layer"] = rng.choice([-1, 0, 1])
        if r() < 0.2:
            f = {}
            for key in ["exclude_tunnel", "exclude_bridge", "exclude_ramp", "exclude_closures",
                        "exclude_toll", "exclude_ferry"]:
                if r() < 0.3:
                    f[key] = r() < 0.7
            classes = ["motorway", "trunk", "primary", "secondary", "tertiary", "unclassified",
                       "residential", "service_other"]
            if r() < 0.3:
                f["min_road_class"] = rng.choice(classes)
            if r() < 0.3:
                f["max_road_class"] = rng.choice(classes)
            if r() < 0.15:
                f["level"] = rng.choice([0, 1, -1, 2, 0.5])
                if r() < 0.5:
                    loc["search_cutoff"] = rng.choice([100, 300, 900, 1500])
            loc["search_filter"] = f
        return loc

    def costing_fields(req):
        costing = rng.choice(COSTINGS)
        req["costing"] = costing
        options = {}
        if costing == "pedestrian" and rng.random() < 0.2:
            options["type"] = rng.choice(["wheelchair", "blind", "foot"])
        if costing == "pedestrian" and rng.random() < 0.1:
            options["max_hiking_difficulty"] = rng.randrange(1, 7)
        if costing == "bicycle" and rng.random() < 0.3:
            options["bicycle_type"] = rng.choice(["Road", "Hybrid", "Cross", "Mountain"])
        if costing == "auto" and rng.random() < 0.1:
            options["exclude_tolls"] = True
        if options:
            req["costing_options"] = {costing: options}

    def exclusions(req):
        if rng.random() < 0.15:
            req["exclude_locations"] = [location(0.01) for _ in range(rng.randrange(1, 4))]
        if rng.random() < 0.12:
            rings = []
            for _ in range(rng.randrange(1, 3)):
                cx, cy = point(0.01)
                n = rng.randrange(3, 8)
                size = rng.choice([0.0005, 0.002, 0.005, 0.012])
                ring = []
                for i in range(n):
                    a = 2 * math.pi * i / n + rng.uniform(-0.2, 0.2)
                    rad = size * rng.uniform(0.5, 1.0)
                    ring.append([round(cx + rad * math.cos(a), 6), round(cy + rad * math.sin(a) * 0.7, 6)])
                if rng.random() < 0.5:
                    ring.append(ring[0])
                if rng.random() < 0.3:
                    ring.reverse()
                rings.append(ring)
            req["exclude_polygons"] = rings

    out = []
    for i in range(count):
        kind = rng.random()
        req = {}
        if kind < 0.55:
            action = "route"
            n = rng.choice([2, 2, 2, 3, 4])
            locs = [location() for _ in range(n)]
            for loc in locs[1:-1]:
                if rng.random() < 0.4:
                    loc["type"] = rng.choice(["through", "via", "break_through", "break"])
            req["locations"] = locs
            costing_fields(req)
            exclusions(req)
        elif kind < 0.72:
            action = "locate"
            req["locations"] = [location() for _ in range(rng.choice([1, 1, 2, 3]))]
            if rng.random() < 0.85:
                costing_fields(req)
            req["verbose"] = rng.random() < 0.5
        elif kind < 0.84:
            action = rng.choice(["trace_route", "trace_attributes"])
            lon, lat = point()
            shape = []
            for _ in range(rng.randrange(2, 12)):
                lon += rng.gauss(0, 0.0007)
                lat += rng.gauss(0, 0.0005)
                shape.append({"lat": round(lat, 6), "lon": round(lon, 6)})
            req["shape"] = shape
            costing_fields(req)
            req["shape_match"] = rng.choice(["map_snap", "walk_or_snap", "edge_walk"])
            if rng.random() < 0.3:
                req["locations"] = [location(), location()]
            if rng.random() < 0.2:
                req["gps_accuracy"] = rng.choice([5, 50, 150])
            if rng.random() < 0.2:
                req["search_radius"] = rng.choice([10, 50, 150])
            if rng.random() < 0.15:
                req["breakage_distance"] = rng.choice([1, 100, 2000])
            if rng.random() < 0.2:
                req["alternates"] = rng.choice([0, 1, 3])
            exclusions(req)
        elif kind < 0.94:
            action = "optimized_route"
            req["locations"] = [location() for _ in range(rng.choice([2, 3, 4, 5]))]
            costing_fields(req)
            exclusions(req)
        else:
            # Requests that break the service limits.
            action = "route"
            limit = rng.choice(["locations", "distance", "excludes", "one", "polygon"])
            if limit == "locations":
                req["locations"] = [location() for _ in range(rng.choice([21, 51]))]
            elif limit == "distance":
                req["locations"] = [location(), {"lat": 48.0 + rng.random(), "lon": 2.0}]
            elif limit == "one":
                req["locations"] = [location()]
            else:
                req["locations"] = [location(), location()]
            costing_fields(req)
            if limit == "excludes":
                req["exclude_locations"] = [location() for _ in range(51)]
            if limit == "polygon":
                cx, cy = point()
                big = rng.choice([0.05, 0.2])
                req["exclude_polygons"] = [[[cx - big, cy - big], [cx + big, cy - big],
                                            [cx + big, cy + big], [cx - big, cy + big]]]
        out.append(f"{region}-{i:05d}\t{action}\t{json.dumps(req, separators=(',', ':'))}")
    print("\n".join(out))


if __name__ == "__main__":
    main()
