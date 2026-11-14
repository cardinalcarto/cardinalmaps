#!/usr/bin/env python3
"""Generates trace requests for the Meili oracle from random walks along the graph.

Usage: meili-corpus.py <walks.jsonl> <count> <seed> > corpus.jsonl

<walks.jsonl> is the output of the `meili` helper's `walks` mode (`cpp/meili.cc`):
road-following polylines for the auto, pedestrian, and bicycle modes.
Each output line is `{"name", "action", "body"}`, as in the shared trace corpus,
a `trace_route` or `trace_attributes` request made from a walk, of one of these kinds
(cycling through them, so a small corpus covers each):

- clean: points every 15 to 40 meters with a meter or two of noise;
- noisy: more noise, with `gps_accuracy` and `search_radius` options;
- dense: points closer than the interpolation distance, which get interpolated;
- linger: a trace that stops for a while, with times (interpolated points set the time the
  trace leaves a match point);
- gap: a stretch of the walk left out, sometimes with a `breakage_distance`;
- types: `break`, `through`, `via`, and `break_through` points;
- times: timestamps from a speed, some too fast for the roads (`max_route_time_factor`);
- uturn: the walk and then back along it;
- alternates: `trace_attributes` with 1 to 3 alternates;
- options: `turn_penalty_factor` and `interpolation_distance`, and a pedestrian or bicycle
  costing on any walk;
- outlier: a point far from any road (a column without candidates);
- offroad: every point far from roads (error 443);
- polyline: the shape as an encoded polyline.

The traces are random but reproducible (the seed fixes them).
"""

import json
import math
import random
import sys

KINDS = [
    "clean", "noisy", "dense", "linger", "gap", "types", "times", "uturn",
    "alternates", "options", "outlier", "offroad", "polyline",
]
METERS_PER_DEGREE = 111_000.0


def distance(a, b):
    lat = math.radians((a[1] + b[1]) / 2)
    dx = (b[0] - a[0]) * METERS_PER_DEGREE * math.cos(lat)
    dy = (b[1] - a[1]) * METERS_PER_DEGREE
    return math.hypot(dx, dy)


def resample(shape, spacing, limit):
    """Points every `spacing` meters along the shape, up to `limit` meters."""
    out = [shape[0]]
    travelled = 0.0
    since = 0.0
    for a, b in zip(shape, shape[1:]):
        d = distance(a, b)
        t = 0.0
        while d - t >= spacing - since:
            t += spacing - since
            f = t / d
            out.append((a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f))
            since = 0.0
            travelled += spacing
            if travelled >= limit:
                return out
        since += d - t
    out.append(shape[-1])
    return out


def jitter(rng, p, meters):
    lat = math.radians(p[1])
    return (
        p[0] + rng.gauss(0, meters) / (METERS_PER_DEGREE * math.cos(lat)),
        p[1] + rng.gauss(0, meters) / METERS_PER_DEGREE,
    )


def encode(points, precision=1e6):
    """A polyline6 encoding of (lng, lat) points."""
    out = []
    last = (0, 0)
    for lng, lat in points:
        cur = (round(lat * precision), round(lng * precision))
        for v, w in zip(cur, last):
            d = v - w
            d = ~(d << 1) if d < 0 else d << 1
            while d >= 0x20:
                out.append(chr((0x20 | (d & 0x1F)) + 63))
                d >>= 5
            out.append(chr(d + 63))
        last = cur
    return "".join(out)


def location(p):
    return {"lat": round(p[1], 6), "lon": round(p[0], 6)}


def make(rng, walk, kind):
    mode = walk["mode"]
    shape = [tuple(p) for p in walk["shape"]]
    costing = mode
    body = {"costing": costing}
    action = "trace_route"
    spacing = rng.uniform(15, 40)
    limit = rng.uniform(400, 1500)
    noise = rng.uniform(0.5, 2.0)
    times = None
    if kind == "noisy":
        noise = rng.uniform(5, 15)
        body["trace_options"] = {"gps_accuracy": rng.choice([5, 10, 20]), "search_radius": rng.choice([15, 30, 50])}
    elif kind == "dense":
        spacing = rng.uniform(2, 8)
        limit = rng.uniform(150, 400)
    pts = resample(shape, spacing, limit)
    if kind == "uturn":
        pts = pts + pts[-2::-1]
    pts = [jitter(rng, p, noise) for p in pts]
    if kind == "linger":
        i = len(pts) // 2
        pts = pts[:i] + [jitter(rng, pts[i], 1.0) for _ in range(rng.randrange(3, 8))] + pts[i:]
        times = []
        t = 1_700_000_000 + rng.randrange(10_000)
        for j, p in enumerate(pts):
            t += rng.uniform(15, 30) if 0 < j and abs(j - i) > 4 else rng.uniform(1, 3)
            times.append(round(t, 1))
    elif kind == "times":
        speed = rng.choice([1.5, 5, 12, 30, 200])
        t = 1_700_000_000 + rng.randrange(10_000)
        times = [t]
        for a, b in zip(pts, pts[1:]):
            t += distance(a, b) / speed
            times.append(round(t, 1))
    elif kind == "gap" and len(pts) > 8:
        i = rng.randrange(2, len(pts) - 5)
        j = min(len(pts) - 2, i + rng.randrange(3, 30))
        pts = pts[:i] + pts[j:]
        if rng.random() < 0.6:
            body.setdefault("trace_options", {})["breakage_distance"] = rng.choice([100, 300, 1000])
    elif kind == "outlier" and len(pts) > 4:
        i = rng.randrange(1, len(pts) - 1)
        lat = math.radians(pts[i][1])
        pts[i] = (pts[i][0] + 400 / (METERS_PER_DEGREE * math.cos(lat)), pts[i][1] + 300 / METERS_PER_DEGREE)
    elif kind == "offroad":
        pts = [(p[0] + 0.03, p[1] + 0.03) for p in pts[:5]]
    if kind == "options":
        opts = body.setdefault("trace_options", {})
        opts["turn_penalty_factor"] = rng.choice([0, 50, 200, 500])
        if rng.random() < 0.5:
            opts["interpolation_distance"] = rng.choice([1, 20, 50])
        body["costing"] = rng.choice(["auto", "pedestrian", "bicycle"])
    if kind == "alternates":
        action = "trace_attributes"
        body["alternates"] = rng.choice([1, 2, 3])
    elif rng.random() < 0.25:
        action = "trace_attributes"
    if kind == "polyline":
        body["encoded_polyline"] = encode(pts)
        return action, body
    shape_points = [location(p) for p in pts]
    if times:
        for loc, t in zip(shape_points, times):
            loc["time"] = t
    if kind == "types":
        for loc in shape_points[1:-1]:
            if rng.random() < 0.3:
                loc["type"] = rng.choice(["break", "through", "via", "break_through"])
    if rng.random() < 0.15:
        shape_points[rng.randrange(len(shape_points))]["radius"] = rng.choice([5, 25, 80])
    body["shape"] = shape_points
    return action, body


def main():
    walks_path, count, seed = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    rng = random.Random(seed)
    walks = [json.loads(line) for line in open(walks_path) if line.strip()]
    for n in range(count):
        kind = KINDS[n % len(KINDS)]
        walk = walks[n % len(walks)]
        action, body = make(rng, walk, kind)
        print(json.dumps({"name": f"meili-{n:04d}-{kind}", "action": action, "body": body}))


if __name__ == "__main__":
    main()
