#!/usr/bin/env python3
"""Generates the map matching corpus: trace_route and trace_attributes requests.

Usage: trace-corpus.py <region> <count> <seed> [--url http://localhost:8002] > <region>.jsonl

Each line is `{"name": ..., "action": "trace_route"|"trace_attributes", "body": {...}}`.
A few lines describe one exact HTTP request instead, with capture.py's fields
(`method`, `raw_body`, `content_type`, `pbf_text`); their `body` is the JSON that is sent,
or null.

Traces come from real routes: the generator routes with mainline (a staged server,
see serve-staged.sh) between random points, decodes the shape, resamples it at varied
intervals, and adds GPS noise. The same seed, tiles, and mainline give the same corpus.

Every kind of case in KINDS is made at least once; the rest are drawn by weight,
so most cases are ordinary traces. Then come the fixed limit and error cases.
"""

import argparse
import json
import math
import random
import sys
import urllib.error
import urllib.request

REGIONS = {
    # The Andorra fixture tiles (valhalla-graphtile/fixtures/andorra-tiles).
    "andorra": (1.42, 42.43, 1.78, 42.65),
    # Central Seattle, for the Seattle oracle build.
    "seattle": (-122.42, 47.52, -122.25, 47.70),
}
# Route lengths (km) for each costing, as (min, max) of the offset between the ends.
SPANS = {"auto": (1.0, 12.0), "pedestrian": (0.2, 2.5), "bicycle": (0.8, 6.0)}
COSTING_OPTIONS = {
    "auto": [{"use_highways": 0.2}, {"use_tolls": 0.0}, {"shortest": True}, {"top_speed": 60},
             {"use_living_streets": 0.9, "service_penalty": 50}],
    "pedestrian": [{"walking_speed": 3.5}, {"use_hills": 0.1}, {"max_hiking_difficulty": 4},
                   {"type": "wheelchair"}, {"walkway_factor": 0.5, "sidewalk_factor": 0.6}],
    "bicycle": [{"bicycle_type": "Mountain"}, {"bicycle_type": "Road", "use_roads": 0.2},
                {"cycling_speed": 25}, {"use_hills": 0.9}, {"bicycle_type": "Hybrid"}],
}
ATTRIBUTES = [
    "edge.names", "edge.id", "edge.way_id", "edge.length", "edge.speed", "edge.road_class",
    "edge.use", "edge.begin_shape_index", "edge.end_shape_index", "edge.begin_heading",
    "edge.end_heading", "edge.traversability", "edge.surface", "edge.travel_mode",
    "edge.speed_limit", "edge.density", "edge.forward", "edge.elevation", "edge.mean_elevation",
    "node.type", "node.elapsed_time", "node.intersecting_edge.begin_heading",
    "node.intersecting_edge.road_class", "matched.point", "matched.type", "matched.edge_index",
    "matched.distance_along_edge", "matched.distance_from_trace_point",
    "matched.begin_route_discontinuity", "matched.end_route_discontinuity", "shape",
    "confidence_score", "raw_score", "osm_changeset", "admin.country_code", "admin.state_code",
    "shape_attributes.time", "shape_attributes.length", "shape_attributes.speed",
    "edge.tagged_names", "not.an.attribute",
]
# 2026-11-09 08:00 UTC, a Monday.
EPOCH = 1794211200


# Geometry -------------------------------------------------------------------------------

def decode(encoded):
    """Decodes a polyline6 string to [(lon, lat)] as integer micro-degrees."""
    points, index, lat, lon = [], 0, 0, 0
    while index < len(encoded):
        deltas = []
        for _ in range(2):
            shift = result = 0
            while True:
                b = ord(encoded[index]) - 63
                index += 1
                result |= (b & 0x1F) << shift
                shift += 5
                if b < 0x20:
                    break
            deltas.append(~(result >> 1) if result & 1 else result >> 1)
        lat += deltas[0]
        lon += deltas[1]
        points.append((lon, lat))
    return points


def encode(points):
    """Encodes [(lon, lat)] integer micro-degrees as polyline6."""
    out, plat, plon = [], 0, 0
    for lon, lat in points:
        for delta in (lat - plat, lon - plon):
            value = ~(delta << 1) if delta < 0 else delta << 1
            while value >= 0x20:
                out.append(chr((0x20 | (value & 0x1F)) + 63))
                value >>= 5
            out.append(chr(value + 63))
        plat, plon = lat, lon
    return "".join(out)


def meters(a, b):
    """Haversine distance in meters between (lon, lat) points in degrees."""
    lon1, lat1, lon2, lat2 = map(math.radians, (*a, *b))
    h = math.sin((lat2 - lat1) / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin((lon2 - lon1) / 2) ** 2
    return 6371000 * 2 * math.asin(math.sqrt(h))


def offset(p, east, north):
    """Moves a (lon, lat) point by meters east and north."""
    lat = p[1] + north / 111320
    return (p[0] + east / (111320 * math.cos(math.radians(p[1]))), lat)


class Path:
    """A route's shape in degrees, with cumulative distance (m) and time (s) at each point."""

    def __init__(self, points, times):
        self.points = points
        self.dist = [0.0]
        for a, b in zip(points, points[1:]):
            self.dist.append(self.dist[-1] + meters(a, b))
        self.times = times

    @property
    def length(self):
        return self.dist[-1]

    def at(self, d):
        """The point and time at distance d along the path."""
        lo, hi = 0, len(self.dist) - 1
        while hi - lo > 1:
            mid = (lo + hi) // 2
            lo, hi = (mid, hi) if self.dist[mid] <= d else (lo, mid)
        span = self.dist[hi] - self.dist[lo]
        f = 0.0 if span == 0 else min(1.0, max(0.0, (d - self.dist[lo]) / span))
        a, b = self.points[lo], self.points[hi]
        t = self.times[lo] + f * (self.times[hi] - self.times[lo])
        return (a[0] + f * (b[0] - a[0]), a[1] + f * (b[1] - a[1])), t

    def resample(self, rng, interval, jitter=0.3):
        """Points every `interval` meters (varied by +-jitter), always including both ends."""
        out, d = [], 0.0
        while d < self.length:
            out.append(self.at(d))
            d += max(0.5, interval * (1 + rng.uniform(-jitter, jitter)))
        out.append(self.at(self.length))
        return out

    def reversed(self):
        total = self.times[-1]
        return Path(self.points[::-1], [total - t for t in self.times[::-1]])

    def then(self, other):
        start = self.times[-1]
        return Path(self.points + other.points[1:], self.times + [start + t for t in other.times[1:]])


def window(rng, samples, most):
    """A contiguous run of the samples, most of the time no longer than `most` points."""
    if len(samples) <= most or rng.random() < 0.1:
        return samples
    n = rng.randrange(max(2, most // 10), most + 1)
    start = rng.randrange(len(samples) - n + 1)
    return samples[start:start + n]


# Mainline -------------------------------------------------------------------------------

def post(url, action, body):
    req = urllib.request.Request(f"{url}/{action}", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=120) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError:
        return None


def leg_path(leg):
    """A leg's shape with times spread over each maneuver by distance."""
    raw = decode(leg["shape"])
    points = [(lon / 1e6, lat / 1e6) for lon, lat in raw]
    times = [0.0] * len(points)
    for m in leg["maneuvers"]:
        b, e = m["begin_shape_index"], m["end_shape_index"]
        seg = [meters(points[i], points[i + 1]) for i in range(b, e)]
        total = sum(seg) or 1.0
        for i in range(b, e):
            times[i + 1] = times[i] + m["time"] * seg[i - b] / total
    return raw, Path(points, times)


class Router:
    def __init__(self, url, region, rng):
        self.url, self.bbox, self.rng = url, REGIONS[region], rng

    def random_point(self):
        minx, miny, maxx, maxy = self.bbox
        return (self.rng.uniform(minx, maxx), self.rng.uniform(miny, maxy))

    def route(self, costing, stops=2, back=False):
        """A routable path between random points: (raw polyline6 points, Path, request)."""
        lo, hi = SPANS[costing]
        for _ in range(50):
            start = self.random_point()
            locs = [start]
            for _ in range(stops - 1):
                km = self.rng.uniform(lo, hi) / math.sqrt(stops - 1)
                angle = self.rng.uniform(0, 2 * math.pi)
                locs.append(offset(locs[-1], 1000 * km * math.cos(angle), 1000 * km * math.sin(angle)))
            if back:
                locs.append(start)
            request = {"locations": [{"lon": round(x, 6), "lat": round(y, 6)} for x, y in locs],
                       "costing": costing}
            response = post(self.url, "route", request)
            if not response:
                continue
            raw, path = [], None
            for leg in response["trip"]["legs"]:
                leg_raw, leg_p = leg_path(leg)
                raw = raw + leg_raw[1:] if raw else leg_raw
                path = leg_p if path is None else path.then(leg_p)
            if path.length > 50:
                return raw, path, request
        sys.exit(f"no route found for {costing} after 50 tries")


# Case building --------------------------------------------------------------------------

class Builder:
    def __init__(self, router, rng):
        self.router, self.rng = router, rng

    def costing(self, body, costing=None):
        r = self.rng
        costing = costing or r.choices(["auto", "pedestrian", "bicycle"], [50, 25, 25])[0]
        body["costing"] = costing
        if r.random() < 0.35:
            body["costing_options"] = {costing: dict(r.choice(COSTING_OPTIONS[costing]))}
        return costing

    def noisy(self, samples, sigma):
        return [(offset(p, self.rng.gauss(0, sigma), self.rng.gauss(0, sigma)), t) for p, t in samples]

    def shape(self, body, samples, encoded=None, times=None):
        """Writes the samples as `shape` (with times if asked) or as `encoded_polyline`."""
        if encoded is None:
            encoded = self.rng.random() < 0.4 and times is None
        if encoded:
            body["encoded_polyline"] = encode([(round(p[0] * 1e6), round(p[1] * 1e6)) for p, _ in samples])
        else:
            body["shape"] = [{"lat": round(p[1], 6), "lon": round(p[0], 6)} for p, _ in samples]
            if times == "points":
                for point, (_, t) in zip(body["shape"], samples):
                    point["time"] = round(EPOCH + t, 1)

    def common(self, body, action):
        """The response options: format, units, id, language, directions."""
        r = self.rng
        if action == "trace_route":
            body["format"] = r.choices(["json", "osrm", "pbf"], [60, 25, 15])[0]
            if r.random() < 0.15:
                body["units"] = "miles"
            if r.random() < 0.1:
                body["language"] = r.choice(["ca-ES", "fr-FR", "es-ES", "de-DE"])
            if r.random() < 0.08:
                body["directions_type"] = r.choice(["none", "maneuvers"])
            if body["format"] == "osrm" and r.random() < 0.5:
                body.update(banner_instructions=True, voice_instructions=True)
        else:
            body["format"] = r.choices(["json", "pbf"], [85, 15])[0]
            if r.random() < 0.15:
                body["units"] = "miles"
            if r.random() < 0.1:
                body["shape_format"] = r.choice(["polyline5", "geojson"])
            if r.random() < 0.1:
                body["elevation_interval"] = r.choice([10, 30, 100])
        if r.random() < 0.2:
            body["id"] = f"trace-{r.randrange(10000)}"
        return body

    def trace(self, action=None, costing=None, interval=(10, 30), sigma=(0, 3), stops=2, back=False,
              match="map_snap", times=None, most=600):
        """An ordinary trace along a fresh route."""
        r = self.rng
        action = action or r.choice(["trace_route", "trace_attributes"])
        body = {}
        costing = self.costing(body, costing)
        raw, path, _ = self.router.route(costing, stops, back)
        samples = self.noisy(window(r, path.resample(r, r.uniform(*interval)), most), r.uniform(*sigma))
        self.shape(body, samples, times=times)
        body["shape_match"] = match
        return action, body, samples, raw, path

    # Each kind returns (action, body); `common` is applied afterwards.

    def clean(self):
        return self.trace(interval=(8, 30), sigma=(0, 3))[:2]

    def noisy_trace(self):
        action, body, *_ = self.trace(interval=(15, 60), sigma=(5, 20))
        if self.rng.random() < 0.6:
            body["trace_options"] = {"gps_accuracy": self.rng.choice([10, 15, 25]),
                                     "search_radius": self.rng.choice([30, 50, 75])}
        return action, body

    def sparse(self):
        return self.trace(interval=(120, 500), sigma=(0, 8))[:2]

    def dense(self):
        """Points closer than interpolation_distance, which meili interpolates."""
        action, body, *_ = self.trace(interval=(1, 6), sigma=(0, 2), most=2000)
        if self.rng.random() < 0.4:
            body["trace_options"] = {"interpolation_distance": self.rng.choice([0, 3, 25, 60])}
        return action, body

    def gap(self):
        """A trace with a hole longer than breakage_distance (a discontinuity)."""
        r = self.rng
        action, body, samples, *_ = self.trace(costing="auto", interval=(15, 40), sigma=(0, 4))
        breakage = r.choice([150, 300, 600, None])
        hole = (breakage or 2000) * r.uniform(1.3, 2.5)
        start = r.randrange(1, max(2, len(samples) // 2))
        kept, d, end = samples[:start], 0.0, start
        while end < len(samples) - 1 and d < hole:
            d += meters(samples[end][0], samples[end + 1][0])
            end += 1
        kept += samples[end:]
        body.pop("shape", None), body.pop("encoded_polyline", None)
        self.shape(body, kept)
        if breakage:
            body["trace_options"] = {"breakage_distance": breakage}
        return action, body

    def exact(self, match):
        """The exact shape of a route, as edge_walk expects."""
        r = self.rng
        action = r.choice(["trace_route", "trace_attributes"])
        body = {}
        costing = self.costing(body)
        raw, *_ = self.router.route(costing, r.choice([2, 2, 3]))
        if r.random() < 0.6:
            body["encoded_polyline"] = encode(raw)
        else:
            body["shape"] = [{"lat": lat / 1e6, "lon": lon / 1e6} for lon, lat in raw]
        body["shape_match"] = match
        return action, body

    def edge_walk_exact(self):
        return self.exact("edge_walk")

    def walk_or_snap_exact(self):
        return self.exact("walk_or_snap")

    def walk_or_snap_noisy(self):
        return self.trace(interval=(10, 40), sigma=(3, 12), match="walk_or_snap")[:2]

    def edge_walk_noisy(self):
        return self.trace(interval=(10, 40), sigma=(3, 12), match="edge_walk")[:2]

    def types(self):
        """trace_route with break, break_through, via, and through points."""
        r = self.rng
        _, body, samples, *_ = self.trace("trace_route", interval=(20, 60), sigma=(0, 4))
        body.pop("encoded_polyline", None)
        self.shape(body, samples, encoded=False)
        for point in body["shape"][1:-1]:
            if r.random() < 0.15:
                point["type"] = r.choice(["break", "break_through", "via", "through"])
        if r.random() < 0.3:
            body["shape"][0]["type"] = r.choice(["break", "break_through"])
        return "trace_route", body

    def timestamps(self):
        """Times on the points, as point `time`s or as begin_time and durations."""
        r = self.rng
        style = r.choice(["points", "durations"])
        action, body, samples, *_ = self.trace(interval=(10, 50), sigma=(0, 8),
                                               times="points" if style == "points" else None)
        if style == "durations":
            body["begin_time"] = EPOCH + r.randrange(86400)
            body["durations"] = [round(b[1] - a[1], 1) for a, b in zip(samples, samples[1:])]
        body["use_timestamps"] = r.random() < 0.6
        return action, body

    def uturn(self):
        r = self.rng
        action = r.choice(["trace_route", "trace_attributes"])
        body = {}
        costing = self.costing(body)
        _, path, _ = self.router.route(costing)
        # Go out part of the way and come back along the same streets.
        cut = r.uniform(0.3, 1.0) * path.length
        fine = path.resample(r, 3, 0)
        out = Path([p for p, _ in fine], [t for _, t in fine])
        n = max(2, sum(1 for d in out.dist if d <= cut))
        out = Path(out.points[:n], out.times[:n])
        both = out.then(out.reversed())
        self.shape(body, self.noisy(both.resample(r, r.uniform(8, 30)), r.uniform(0, 4)))
        body["shape_match"] = "map_snap"
        return action, body

    def loop(self):
        return self.trace(interval=(15, 40), sigma=(0, 5), stops=3, back=True)[:2]

    def outliers(self):
        """A few points far off the road (GPS spikes)."""
        r = self.rng
        action, body, samples, *_ = self.trace(interval=(15, 40), sigma=(0, 4))
        for _ in range(r.randrange(1, 4)):
            i = r.randrange(1, len(samples) - 1) if len(samples) > 2 else 0
            p, t = samples[i]
            samples[i] = (offset(p, r.choice([-1, 1]) * r.uniform(80, 400), r.uniform(-200, 200)), t)
        body.pop("shape", None), body.pop("encoded_polyline", None)
        self.shape(body, samples)
        return action, body

    def off_road(self):
        """A trace with no roads nearby (often an error)."""
        r = self.rng
        action = r.choice(["trace_route", "trace_attributes"])
        body = {}
        self.costing(body)
        start = self.router.random_point()
        angle = r.uniform(0, 2 * math.pi)
        pts = [offset(start, 30 * i * math.cos(angle), 30 * i * math.sin(angle)) for i in range(r.randrange(3, 20))]
        self.shape(body, self.noisy([(p, 10.0 * i) for i, p in enumerate(pts)], 3))
        body["shape_match"] = r.choice(["map_snap", "walk_or_snap"])
        body["trace_options"] = {"search_radius": r.choice([5, 20, 50])}
        return action, body

    def short(self):
        """Two or three points a few meters apart."""
        r = self.rng
        action = r.choice(["trace_route", "trace_attributes"])
        body = {}
        costing = self.costing(body)
        _, path, _ = self.router.route(costing)
        d = r.uniform(0, max(0.0, path.length - 50))
        span = r.uniform(2, 40)
        samples = [path.at(d + span * k / 2) for k in range(r.choice([2, 3]))]
        self.shape(body, self.noisy(samples, r.uniform(0, 2)))
        body["shape_match"] = r.choice(["map_snap", "walk_or_snap"])
        return action, body

    def stationary(self):
        """Repeated points while stopped, then moving on."""
        r = self.rng
        action, body, samples, *_ = self.trace(interval=(10, 30), sigma=(0, 2), times="points")
        i = r.randrange(len(samples))
        p, t = samples[i]
        dwell = [(offset(p, r.gauss(0, 1.5), r.gauss(0, 1.5)) if r.random() < 0.5 else p, t + k) for k in range(1, r.randrange(3, 12))]
        later = [(q, s + len(dwell)) for q, s in samples[i + 1:]]
        self.shape(body, samples[:i + 1] + dwell + later, encoded=False, times="points")
        return action, body

    def options(self):
        """Every trace option, in random combinations and at both ends of their ranges."""
        r = self.rng
        action, body, *_ = self.trace(interval=(10, 60), sigma=(0, 10))
        choices = {
            "search_radius": [0, 10, 25, 100],
            "gps_accuracy": [1, 4.07, 10, 50, 100],
            "breakage_distance": [100, 500, 2000, 5000],
            "interpolation_distance": [0, 5, 20, 100],
            "turn_penalty_factor": [0, 50, 200, 500],
            "max_route_distance_factor": [1, 3, 10],
            "max_route_time_factor": [1, 3, 10],
        }
        keys = r.sample(sorted(choices), r.randrange(1, len(choices) + 1))
        body["trace_options"] = {k: r.choice(choices[k]) for k in keys}
        return action, body

    def alternates(self):
        """trace_attributes with alternate matches (map_snap, at most 100 points)."""
        r = self.rng
        _, body, samples, *_ = self.trace("trace_attributes", interval=(15, 60), sigma=(4, 20))
        if len(samples) > 100:
            body.pop("shape", None), body.pop("encoded_polyline", None)
            self.shape(body, samples[:r.randrange(10, 101)])
        body["alternates"] = r.choice([1, 2, 3, 3, 5])
        return "trace_attributes", body

    def filters(self):
        r = self.rng
        _, body, *_ = self.trace("trace_attributes", interval=(10, 40), sigma=(0, 6))
        body["filters"] = {"action": r.choice(["include", "exclude"]),
                           "attributes": r.sample(ATTRIBUTES, r.randrange(1, 12))}
        return "trace_attributes", body


KINDS = {  # kind: weight
    "clean": 18, "noisy_trace": 18, "sparse": 6, "dense": 6, "gap": 5, "edge_walk_exact": 6,
    "walk_or_snap_exact": 4, "walk_or_snap_noisy": 4, "edge_walk_noisy": 2, "types": 4,
    "timestamps": 6, "uturn": 3, "loop": 3, "outliers": 4, "off_road": 2, "short": 3,
    "stationary": 2, "options": 6, "alternates": 4, "filters": 5,
}


# Limits and errors ----------------------------------------------------------------------

def fixed_cases(builder):
    """Service limits and requests mainline rejects, as (kind, action, body, http fields)."""
    r = builder.rng
    router = builder.router

    def base(action="trace_route", costing="auto"):
        _, path, _ = router.route(costing)
        samples = path.resample(r, 15)[:40]
        body = {"costing": costing, "shape_match": "map_snap"}
        builder.shape(body, samples, encoded=False)
        return body

    def dense(n):
        """n points along a route (a long, tight resampling) for the shape limits."""
        _, path, _ = router.route("auto")
        step = path.length / (n - 1)
        body = {"costing": "auto", "shape_match": "map_snap"}
        builder.shape(body, [path.at(step * i) for i in range(n)], encoded=True)
        return body

    def zigzag(km):
        """A trace whose summed point-to-point distance is about `km` kilometers."""
        _, path, _ = router.route("auto")
        a, b = path.points[0], path.at(min(1500, path.length))[0]
        legs = int(km * 1000 / max(meters(a, b), 1)) + 1
        return [(a, b)[i % 2] for i in range(legs + 1)]

    out = []
    add = lambda kind, action, body, **http: out.append((kind, action, body, http))

    # Limits (service_limits.trace and the trace option bounds).
    add("limit_max_shape", "trace_attributes", dense(16000))
    add("limit_over_max_shape", "trace_route", dense(16001))
    body = dense(100)
    body["alternates"] = 3
    add("limit_alternates_shape", "trace_attributes", body)
    body = dense(101)
    body["alternates"] = 1
    add("limit_over_alternates_shape", "trace_attributes", body)
    body = dense(150)
    body["alternates"] = 0
    add("limit_no_alternates_long_shape", "trace_attributes", body)
    body = dense(101)
    body.update(alternates=2, shape_match="walk_or_snap")
    add("limit_alternates_shape_walk_or_snap", "trace_attributes", body)
    for match, km in (("map_snap", 199), ("map_snap", 201), ("edge_walk", 900), ("edge_walk", 1001)):
        body = {"costing": "auto", "shape_match": match}
        builder.shape(body, [(p, 0) for p in zigzag(km)], encoded=True)
        add(f"limit_distance_{match}_{km}km", "trace_route", body)
    for key, value in (("gps_accuracy", 100), ("gps_accuracy", 100.5), ("gps_accuracy", -1),
                       ("search_radius", 100), ("search_radius", 101), ("search_radius", -5),
                       ("turn_penalty_factor", -1), ("breakage_distance", 0)):
        body = base()
        body["trace_options"] = {key: value}
        add(f"limit_{key}_{value}".replace("-", "neg").replace(".", "_"), "trace_route", body)
    body = base("trace_attributes")
    body["alternates"] = 10
    add("limit_alternates_clamped", "trace_attributes", body)

    # Errors.
    add("error_100_bad_json", "trace_route", None, raw_body='{"shape": [')
    add("error_101_put", "trace_route", base(), method="PUT")
    add("get_json_param", "trace_attributes", base(), method="GET")
    add("error_103_bad_pbf", "trace_route", None, raw_body="not a protobuf", content_type="application/x-protobuf")
    add("error_114_no_shape", "trace_route", {"costing": "auto"})
    add("error_114_empty_polyline", "trace_attributes", {"costing": "auto", "encoded_polyline": ""})
    body = base()
    body["shape"] = body["shape"][:1]
    add("error_123_one_point", "trace_route", body)
    body = base("trace_attributes")
    body.pop("costing")
    add("error_124_no_costing", "trace_attributes", body)
    body = base()
    body["costing"] = "hovercraft"
    add("error_125_unknown_costing", "trace_route", body)
    body = base()
    del body["shape"][1]["lon"]
    add("error_134_bad_shape_point", "trace_route", body)
    body = base()
    body["trace"] = body.pop("shape")
    # Mainline fails with 135 for any `trace`, even valid points like these.
    add("trace_key_valid_points", "trace_route", body)
    del body["trace"][0]["lat"]
    add("error_135_bad_trace_point", "trace_route", body)
    body = base()
    body["durations"] = [1.0] * len(body["shape"])
    add("error_136_durations", "trace_attributes", body)
    body = base()
    body["costing"] = "multimodal"
    add("error_140_multimodal", "trace_route", body)
    body = base("trace_attributes")
    body["exclude_locations"] = [dict(body["shape"][0]) for _ in range(51)]
    add("error_157_exclude_locations", "trace_attributes", body)
    body = base()
    body["use_timestamps"] = True
    # JSON shape points always carry a time, so this isn't 159 (see the PBF case below).
    add("use_timestamps_without_times", "trace_route", body)
    body = base()
    body["date_time"] = {"type": 1, "value": "yesterday"}
    add("error_162_bad_date", "trace_route", body)
    body = base()
    body["date_time"] = {"type": 1, "value": "2026-11-09T08:00"}
    add("date_time_depart_at", "trace_route", body)
    body = base()
    body["date_time"] = {"type": 1}
    add("error_160_missing_date", "trace_route", body)
    body = base()
    body["date_time"] = {"type": 9, "value": "2026-11-09T08:00"}
    add("date_time_unknown_type", "trace_route", body)
    body = base("trace_attributes")
    body["shape_format"] = "polyline7"
    add("error_164_shape_format", "trace_attributes", body)
    body = {"costing": "auto", "shape": base()["shape"][:1]}
    p = body["shape"][0]
    body["shape"] += [{"lat": p["lat"] + 0.03 * k, "lon": p["lon"]} for k in (1, 2)]
    add("error_172_breakage", "trace_route", body)
    body = base()
    body["shape_match"] = "best_effort"
    add("error_445_shape_match", "trace_attributes", body)
    # Errors from matching: edge_walk on a shape off the route (443),
    # map_snap with no candidates (443 for trace_route, 444 for trace_attributes),
    # and a trace whose points can't be joined (442).
    for action in ("trace_route", "trace_attributes"):
        body = base(action)
        for point in body["shape"]:
            point["lat"] += 0.0004
        body["shape_match"] = "edge_walk"
        add(f"error_edge_walk_off_route_{action}", action, body)
        body = base(action)
        for point in body["shape"]:
            point["lat"] += 0.002
        body["trace_options"] = {"search_radius": 0}
        add(f"error_no_candidates_{action}", action, body)
        body = base(action, costing="pedestrian")
        body["trace_options"] = {"max_route_distance_factor": 0, "breakage_distance": 100000}
        body["shape"] = body["shape"][:: max(1, len(body["shape"]) // 3)]
        add(f"sparse_long_hops_{action}", action, body)
    # Errors in the other formats.
    for fmt in ("osrm", "pbf"):
        body = base()
        body["shape"] = body["shape"][:1]
        body["format"] = fmt
        add(f"error_123_one_point_{fmt}", "trace_route", body)
    body = base("trace_attributes")
    body.update(costing="hovercraft", format="pbf")
    add("error_125_unknown_costing_pbf", "trace_attributes", body)
    body = base("trace_attributes")
    body["format"] = "osrm"
    add("trace_attributes_osrm_format", "trace_attributes", body)
    # PBF request bodies. Mainline gives every point a time of -1 when parsing,
    # so use_timestamps without times is never 159, here or in JSON.
    shape = " ".join(f"shape {{ ll {{ lat: {pt['lat']} lng: {pt['lon']} }} }}" for pt in base()["shape"][:30])
    options = f"action: trace_route costing_type: auto_ format: pbf shape_match: map_snap {shape}"
    add("pbf_request", "trace_route", None, pbf_text=f"options {{ {options} }}")
    add("use_timestamps_without_times_pbf", "trace_route", None, pbf_text=f"options {{ {options} use_timestamps: true }}")
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("region", choices=sorted(REGIONS))
    parser.add_argument("count", type=int, help="number of generated (non-fixed) cases")
    parser.add_argument("seed", type=int)
    parser.add_argument("--url", default="http://localhost:8002")
    args = parser.parse_args()

    rng = random.Random(args.seed)
    builder = Builder(Router(args.url, args.region, rng), rng)
    kinds = list(KINDS)
    order = kinds + rng.choices(kinds, [KINDS[k] for k in kinds], k=max(0, args.count - len(kinds)))
    for i, kind in enumerate(order):
        action, body = getattr(builder, kind)()
        builder.common(body, action)
        print(json.dumps({"name": f"{args.region}-{i:04d}-{kind}", "action": action, "body": body}))
    for kind, action, body, http in fixed_cases(builder):
        print(json.dumps({"name": f"{args.region}-{kind}", "action": action, "body": body, **http}))


if __name__ == "__main__":
    main()
