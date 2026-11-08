#!/usr/bin/env python3
"""Generates the Thor oracle corpus: route requests that exercise mainline's path search.

Usage: thor-corpus.py <region> <count> <seed> [trip] > requests.tsv

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

With `trip`, the requests also use the options that trip building reads
(the `thor` helper's `trip` mode compares whole trips):
attribute filters (`include` and `exclude`, Ferrostar's shape attributes among them),
`elevation_interval`, `recostings`, and locations on indoor levels with a `search_filter`
level, from `corpus/levels-<region>.tsv` (midpoints of edges with levels in the region's
tiles, and their levels). Their IDs are `<region>-trip-<n>`.
"""

import json
import random
import sys
from pathlib import Path

REGIONS = {
    # The Andorra fixture tiles (valhalla-graphtile/fixtures/andorra-tiles).
    "andorra": (1.42, 42.43, 1.78, 42.65),
    # Central Seattle, for the Seattle oracle build.
    "seattle": (-122.42, 47.52, -122.25, 47.70),
}

# Weeks with and without daylight saving time, and a leap day.
DATES = ["2026-11-09", "2026-11-13", "2026-11-15", "2026-07-01", "2026-03-29", "2028-02-29"]
TIMES = ["00:00", "03:15", "07:30", "08:00", "12:00", "16:45", "17:30", "21:59", "23:59"]

# Attributes for filters: those trip building reads, the categories it checks,
# some it never writes, and a name that isn't an attribute.
ATTRIBUTES = [
    "edge.names", "edge.length", "edge.speed", "edge.speed_type", "edge.speeds_faded",
    "edge.speeds_non_faded", "edge.road_class", "edge.begin_heading", "edge.end_heading",
    "edge.begin_shape_index", "edge.end_shape_index", "edge.traversability", "edge.use",
    "edge.toll", "edge.unpaved", "edge.tunnel", "edge.bridge", "edge.roundabout",
    "edge.internal_intersection", "edge.drive_on_right", "edge.surface", "edge.sign.exit_number",
    "edge.sign.exit_branch", "edge.sign.exit_toward", "edge.sign.junction_name",
    "edge.travel_mode", "edge.vehicle_type", "edge.pedestrian_type", "edge.bicycle_type",
    "edge.id", "edge.way_id", "edge.begin_osm_node_id", "edge.weighted_grade",
    "edge.max_upward_grade", "edge.max_downward_grade", "edge.mean_elevation", "edge.elevation",
    "edge.lane_count", "edge.lane_connectivity", "edge.cycle_lane", "edge.bicycle_network",
    "edge.sac_scale", "edge.shoulder", "edge.sidewalk", "edge.density", "edge.speed_limit",
    "edge.conditional_speed_limits", "edge.truck_speed", "edge.truck_route", "edge.default_speed",
    "edge.destination_only", "edge.is_urban", "edge.tagged_values", "edge.indoor",
    "edge.landmarks", "edge.country_crossing", "edge.forward", "edge.levels",
    "edge.traffic_signal", "edge.hov_type", "edge.curvature", "edge.lit",
    "node.intersecting_edge.begin_heading", "node.intersecting_edge.from_edge_name_consistency",
    "node.intersecting_edge.to_edge_name_consistency", "node.intersecting_edge.driveability",
    "node.intersecting_edge.cyclability", "node.intersecting_edge.walkability",
    "node.intersecting_edge.use", "node.intersecting_edge.road_class",
    "node.intersecting_edge.lane_count", "node.intersecting_edge.sign_info", "node.elapsed_time",
    "node.admin_index", "node.type", "node.traffic_signal", "node.fork", "node.time_zone",
    "node.transition_time", "node.elevation", "osm_changeset", "admin.country_code",
    "admin.country_text", "admin.state_code", "admin.state_text", "shape", "incidents",
    "shape_attributes.time", "shape_attributes.length", "shape_attributes.speed",
    "shape_attributes.speed_limit", "shape_attributes.closure", "shape_attributes.congestion",
    "not.an.attribute",
]
ADMINS = ["admin.country_code", "admin.country_text", "admin.state_code", "admin.state_text"]
FERROSTAR = ["shape_attributes.speed", "shape_attributes.speed_limit", "shape_attributes.time",
             "shape_attributes.length"]


def read_levels(region):
    """The level points of a region: (lon, lat, [levels])."""
    path = Path(__file__).parent / "corpus" / f"levels-{region}.tsv"
    points = []
    if path.exists():
        for line in path.read_text().splitlines():
            lon, lat, levels = line.split("\t")
            values = []
            for part in levels.split(","):
                values.extend(float(v) for v in part.split(".."))
            points.append((float(lon), float(lat), values))
    return points


def trip_options(rng, request, levels):
    """Adds the options trip building reads to a request."""
    r = rng.random
    if levels and r() < 0.25:
        # Indoor routes: locations near edges with levels, filtered to one of them.
        request["costing"] = "pedestrian"
        request.pop("costing_options", None)
        lon, lat, values = rng.choice(levels)
        nearby = sorted(levels, key=lambda p: (p[0] - lon) ** 2 + (p[1] - lat) ** 2)[1:8]
        for i, loc in enumerate(request["locations"]):
            if i in (0, len(request["locations"]) - 1) or r() < 0.3:
                plon, plat, pvalues = (lon, lat, values) if i == 0 else rng.choice(nearby or levels)
                loc["lon"] = round(plon + rng.gauss(0, 0.00003), 6)
                loc["lat"] = round(plat + rng.gauss(0, 0.00003), 6)
                if r() < 0.8:
                    loc["search_filter"] = {"level": rng.choice(pvalues)}
                loc.pop("heading", None)
    roll = r()
    if roll < 0.15:
        request["filters"] = {"action": "include", "attributes": list(FERROSTAR)}
    elif roll < 0.35:
        request["filters"] = {
            "action": "include",
            "attributes": rng.sample(ATTRIBUTES, rng.randrange(1, 8)),
        }
    elif roll < 0.55:
        attributes = rng.sample(ATTRIBUTES, rng.randrange(1, 12))
        if r() < 0.3:
            attributes += ADMINS
        request["filters"] = {"action": "exclude", "attributes": attributes}
    if r() < 0.2:
        request["elevation_interval"] = rng.choice([5, 30, 100])
    if r() < 0.12:
        recostings = []
        for name, costing in rng.sample(
            [("car", "auto"), ("walk", "pedestrian"), ("bike", "bicycle")], rng.randrange(1, 3)
        ):
            recosting = {"costing": costing, "name": name}
            if costing == "auto" and r() < 0.3:
                recosting["use_highways"] = 0.0
            recostings.append(recosting)
        request["recostings"] = recostings


def main():
    region, count, seed = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    trip = len(sys.argv) > 4 and sys.argv[4] == "trip"
    levels = read_levels(region) if trip else []
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

        if trip:
            trip_options(rng, request, levels)

        action = "optimized_route" if r() < 0.03 and len(locations) > 2 else "route"
        if action == "optimized_route":
            for loc in request["locations"]:
                loc.pop("type", None)
            request.pop("date_time", None)
            request.pop("alternates", None)
        name = f"{region}-trip-{n:05d}" if trip else f"{region}-{n:05d}"
        print(f"{name}\t{action}\t{json.dumps(request, separators=(',', ':'))}")


if __name__ == "__main__":
    main()
