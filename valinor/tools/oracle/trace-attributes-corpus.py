#!/usr/bin/env python3
"""Writes the map matching requests for the `trace_attributes` oracle helper.

Usage: trace-corpus.py <valinor-tyr/fixtures/captures/andorra> > requests.tsv

Each line is `<id>\t<action>\t<json>`, as `cpp/trace_attributes.cc` reads them.
The traces come from the shapes of captured Andorra routes (decoded from their Valhalla JSON
responses), so they run on the fixture tiles:
the exact shapes (for `edge_walk`), points taken along them (clean traces),
the same points moved by a fixed pseudo-random offset (noisy traces),
and traces with a jump between two routes (a discontinuity) or a point far from any road.
Everything is deterministic, so the output is the same on every run.
"""

import json
import math
import random
import sys
from pathlib import Path


def decode(encoded, precision=6):
    """Decodes a Valhalla polyline into (lat, lon) pairs."""
    factor = 10.0**precision
    points, index, lat, lon = [], 0, 0, 0
    while index < len(encoded):
        values = []
        for _ in range(2):
            shift, result = 0, 0
            while True:
                byte = ord(encoded[index]) - 63
                index += 1
                result |= (byte & 0x1F) << shift
                shift += 5
                if byte < 0x20:
                    break
            values.append(~(result >> 1) if result & 1 else result >> 1)
        lat += values[0]
        lon += values[1]
        points.append((lat / factor, lon / factor))
    return points


def encode(points, precision=6):
    """Encodes (lat, lon) pairs as a Valhalla polyline."""
    factor = 10.0**precision
    out, prev_lat, prev_lon = [], 0, 0
    for lat, lon in points:
        ilat, ilon = round(lat * factor), round(lon * factor)
        for delta in (ilat - prev_lat, ilon - prev_lon):
            value = ~(delta << 1) if delta < 0 else delta << 1
            while value >= 0x20:
                out.append(chr((0x20 | (value & 0x1F)) + 63))
                value >>= 5
            out.append(chr(value + 63))
        prev_lat, prev_lon = ilat, ilon
    return "".join(out)


def route_shape(captures, case):
    response = json.loads((captures / case / "json" / "response.json").read_text())
    shape = []
    for leg in response["body"]["trip"]["legs"]:
        points = decode(leg["shape"])
        shape.extend(points if not shape else points[1:])
    return shape


def meters(a, b):
    lat = math.radians((a[0] + b[0]) / 2)
    return math.hypot((a[0] - b[0]) * 111_320, (a[1] - b[1]) * 111_320 * math.cos(lat))


def resample(shape, spacing):
    """Points every `spacing` meters along a shape, with its ends."""
    out, carried = [shape[0]], 0.0
    for a, b in zip(shape, shape[1:]):
        length = meters(a, b)
        position = spacing - carried
        while position < length:
            t = position / length
            out.append((a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t))
            position += spacing
        carried = (carried + length) % spacing
    if out[-1] != shape[-1]:
        out.append(shape[-1])
    return out


def noisy(points, meters_off, seed):
    rng = random.Random(seed)
    out = []
    for lat, lon in points:
        angle = rng.uniform(0, 2 * math.pi)
        distance = rng.uniform(0, meters_off)
        dlat = distance * math.sin(angle) / 111_320
        dlon = distance * math.cos(angle) / (111_320 * math.cos(math.radians(lat)))
        out.append((lat + dlat, lon + dlon))
    return out


def shape_json(points, times=None):
    shape = []
    for i, (lat, lon) in enumerate(points):
        point = {"lat": round(lat, 6), "lon": round(lon, 6)}
        if times is not None:
            point["time"] = times[i]
        shape.append(point)
    return shape


def main():
    captures = Path(sys.argv[1])
    town = route_shape(captures, "auto_town")  # 8 km through Andorra la Vella and Encamp
    catalan = route_shape(captures, "auto_catalan_miles")  # 5 km
    walk = route_shape(captures, "pedestrian_town")  # 1.5 km on foot
    hike = route_shape(captures, "hiking_default_difficulty")  # 4 km, far from town
    three = route_shape(captures, "auto_three_legs")  # 11.6 km, two legs

    town_clean = resample(town, 60)
    town_noisy = noisy(town_clean, 20, 1)
    walk_clean = resample(walk, 25)
    walk_noisy = noisy(walk_clean, 12, 2)
    catalan_noisy = noisy(resample(catalan, 50), 25, 3)
    # Alternates are limited to 100 shape points (`max_alternates_shape`).
    catalan_sparse = noisy(resample(catalan, 100), 15, 5)
    # A jump of several kilometers between the first half of the town route and the hike:
    # farther than meili's breakage distance (2 km), so the path breaks there.
    jump = resample(town[: len(town) // 3], 60) + resample(hike, 60)[:20]
    # A point 400 m off any road, in the middle of the walk.
    middle = len(walk_clean) // 2
    off_road = walk_clean[:middle] + [(42.5160, 1.5300)] + walk_clean[middle:]

    cases = []

    def add(case_id, action, request):
        cases.append((case_id, action, request))

    auto = {"costing": "auto"}
    # Exact route shapes, as edge walking expects them.
    add("edge_walk", "trace_attributes",
        {**auto, "encoded_polyline": encode(town), "shape_match": "edge_walk"})
    add("edge_walk_two_legs", "trace_attributes",
        {**auto, "encoded_polyline": encode(three), "shape_match": "edge_walk"})
    add("walk_or_snap_exact", "trace_attributes",
        {**auto, "encoded_polyline": encode(catalan), "shape_match": "walk_or_snap"})
    add("edge_walk_noisy_fails", "trace_attributes",
        {**auto, "shape": shape_json(town_noisy), "shape_match": "edge_walk"})
    add("walk_or_snap_noisy", "trace_attributes",
        {**auto, "shape": shape_json(town_noisy), "shape_match": "walk_or_snap"})
    # Map matching.
    add("map_snap_clean", "trace_attributes",
        {**auto, "shape": shape_json(town_clean), "shape_match": "map_snap"})
    add("map_snap_noisy", "trace_attributes",
        {**auto, "shape": shape_json(catalan_noisy), "shape_match": "map_snap"})
    add("map_snap_polyline5", "trace_attributes",
        {**auto, "encoded_polyline": encode(town_noisy, 5), "shape_format": "polyline5",
         "shape_match": "map_snap"})
    add("pedestrian_noisy", "trace_attributes",
        {"costing": "pedestrian", "shape": shape_json(walk_noisy), "shape_match": "map_snap"})
    add("discontinuity", "trace_attributes",
        {**auto, "shape": shape_json(jump), "shape_match": "map_snap"})
    add("off_road_point", "trace_attributes",
        {"costing": "pedestrian", "shape": shape_json(off_road), "shape_match": "map_snap",
         "trace_options": {"search_radius": 30}})
    add("alternates", "trace_attributes",
        {"costing": "pedestrian", "shape": shape_json(walk_noisy), "shape_match": "map_snap",
         "alternates": 2})
    add("alternates_auto", "trace_attributes",
        {**auto, "shape": shape_json(catalan_sparse), "shape_match": "map_snap", "alternates": 3})
    add("timestamps", "trace_attributes",
        {**auto, "shape": shape_json(town_clean, [1_700_000_000 + 6 * i
                                                   for i in range(len(town_clean))]),
         "shape_match": "map_snap", "use_timestamps": True,
         "trace_options": {"search_radius": 40, "gps_accuracy": 10, "breakage_distance": 3000,
                           "interpolation_distance": 20, "turn_penalty_factor": 100}})
    add("durations", "trace_attributes",
        {**auto, "shape": shape_json(town_clean), "shape_match": "map_snap",
         "begin_time": 1_700_000_000, "durations": [5.5] * (len(town_clean) - 1),
         "use_timestamps": True})
    add("breaks", "trace_attributes",
        {**auto, "shape_match": "map_snap",
         "shape": [dict(p, type=("break" if i in (0, 40, len(town_clean) - 1) else "via"))
                   for i, p in enumerate(shape_json(town_clean))]})
    add("filters_include", "trace_attributes",
        {**auto, "shape": shape_json(town_noisy), "shape_match": "map_snap",
         "filters": {"action": "include",
                     "attributes": ["edge.names", "edge.id", "edge.length", "edge.way_id",
                                    "edge.speed", "node.elapsed_time", "node.type",
                                    "matched.point", "matched.type", "matched.edge_index",
                                    "shape", "confidence_score", "admin.country_code",
                                    "shape_attributes.time", "shape_attributes.length",
                                    "shape_attributes.speed", "shape_attributes.speed_limit",
                                    "not.an.attribute"]}})
    add("filters_exclude", "trace_attributes",
        {**auto, "shape": shape_json(town_noisy), "shape_match": "map_snap",
         "filters": {"action": "exclude",
                     "attributes": ["edge.names", "node.intersecting_edge.begin_heading",
                                    "matched.distance_from_trace_point", "shape", "raw_score",
                                    "osm_changeset"]}})
    add("filters_include_edge_walk", "trace_attributes",
        {**auto, "encoded_polyline": encode(town), "shape_match": "edge_walk",
         "filters": {"action": "include", "attributes": ["edge.way_id", "matched.type"]}})
    add("miles_id_elevation", "trace_attributes",
        {**auto, "shape": shape_json(town_noisy), "shape_match": "map_snap", "units": "miles",
         "id": "trace \"one\"", "elevation_interval": 30})
    add("bicycle_hike", "trace_attributes",
        {"costing": "bicycle", "shape": shape_json(noisy(resample(hike, 40), 10, 4)),
         "shape_match": "map_snap"})
    add("pbf", "trace_attributes",
        {**auto, "shape": shape_json(catalan_sparse), "shape_match": "map_snap", "format": "pbf",
         "alternates": 1})
    add("pbf_filtered", "trace_attributes",
        {**auto, "shape": shape_json(jump), "shape_match": "map_snap", "format": "pbf",
         "filters": {"action": "include", "attributes": ["matched.point", "matched.type",
                                                         "matched.begin_route_discontinuity",
                                                         "raw_score"]}})

    # trace_route: OSRM (matchings and tracepoints) and Valhalla JSON.
    add("route_osrm_noisy", "trace_route",
        {**auto, "shape": shape_json(town_noisy), "shape_match": "map_snap", "format": "osrm"})
    add("route_osrm_discontinuity", "trace_route",
        {**auto, "shape": shape_json(jump), "shape_match": "map_snap", "format": "osrm"})
    add("route_osrm_off_road", "trace_route",
        {"costing": "pedestrian", "shape": shape_json(off_road), "shape_match": "map_snap",
         "format": "osrm", "trace_options": {"search_radius": 30}})
    add("route_osrm_breaks", "trace_route",
        {**auto, "shape_match": "map_snap", "format": "osrm",
         "shape": [dict(p, type=("break" if i in (0, 40, len(town_clean) - 1) else "via"))
                   for i, p in enumerate(shape_json(town_clean))]})
    add("route_osrm_edge_walk", "trace_route",
        {**auto, "encoded_polyline": encode(catalan), "shape_match": "edge_walk",
         "format": "osrm"})
    add("route_json_noisy", "trace_route",
        {**auto, "shape": shape_json(walk_noisy), "shape_match": "map_snap"})
    add("route_json_discontinuity", "trace_route",
        {**auto, "shape": shape_json(jump), "shape_match": "map_snap", "units": "miles",
         "id": "jump"})

    for case_id, action, request in cases:
        print(f"{case_id}\t{action}\t{json.dumps(request, separators=(',', ':'))}")


if __name__ == "__main__":
    main()
