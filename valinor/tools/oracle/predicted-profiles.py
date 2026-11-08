#!/usr/bin/env python3
"""Writes varied, generated speed CSVs for `build-tiles.sh predicted`, for costing tests.

Runs inside the pinned mainline image (see valhalla-image), using mainline's Python bindings
to compress the profiles:

    tools/oracle/sif-oracle.sh edges <tile_dir> edges.tsv
    podman run --rm -v "$PWD/tools/oracle:/oracle:ro,z" -v "$work:/work:z" \\
        "$(cat tools/oracle/valhalla-image)" \\
        python3 -I /oracle/predicted-profiles.py /work/edges.tsv /work/csv

`edges.tsv` lists every edge (`l/t/i`, speed, use, and shortcut flag), as the sif helper's
`edges` mode prints it. One CSV per tile is written under the output directory
(`<level>/<tile / 1000>/<tile % 1000>.csv`), with rows of
`<edge>,<free flow kph>,<constrained kph>[,<base64 profile>]`
(0 means no free flow or constrained speed; boost's tokenizer drops empty fields).

Each edge gets a kind from a hash of its ID, so a rebuild gives the same tiles:
no data (10%), flow speeds only (15%), or a profile (75%) of one of these shapes,
relative to the edge's speed:

- commute: weekday rush hour dips to 40%, nights at 110%;
- flat: a constant between 50% and 120%;
- congested: between 1 and 8 kph all week;
- fast: up to 160% (capped at 160 kph), above most top speeds;
- noisy: a random walk between 20% and 130%.

About half of the profiled edges also get flow speeds.
Shortcuts get nothing.
"""

import os
import sys
import warnings

import numpy as np

with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    from valhalla import utils

BUCKETS = 2016
BUCKET_MINUTES = 5
MASK = (1 << 64) - 1


def mix(value: int) -> int:
    """SplitMix64, as a hash of an edge ID."""
    value = (value + 0x9E3779B97F4A7C15) & MASK
    value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & MASK
    value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & MASK
    return value ^ (value >> 31)


class Rng:
    def __init__(self, seed: int):
        self.state = seed

    def next(self) -> int:
        self.state = mix(self.state)
        return self.state

    def uniform(self, lo: float, hi: float) -> float:
        return lo + (hi - lo) * (self.next() % 1_000_000) / 1_000_000


def profile(kind: str, speed: float, rng: Rng) -> np.ndarray:
    speeds = np.empty(BUCKETS, dtype=np.float32)
    if kind == "commute":
        for b in range(BUCKETS):
            day, minute = divmod(b * BUCKET_MINUTES, 24 * 60)
            hour = minute / 60
            weekday = 1 <= day <= 5  # the week starts on Sunday
            if weekday and (7 <= hour < 9 or 16 <= hour < 18):
                factor = 0.4
            elif hour < 6 or hour >= 22:
                factor = 1.1
            else:
                factor = 0.85
            speeds[b] = speed * factor
    elif kind == "flat":
        speeds[:] = speed * rng.uniform(0.5, 1.2)
    elif kind == "congested":
        for b in range(BUCKETS):
            speeds[b] = rng.uniform(1.0, 8.0)
    elif kind == "fast":
        speeds[:] = min(speed * rng.uniform(1.2, 1.6), 160.0)
    else:  # noisy
        value = speed * 0.8
        for b in range(BUCKETS):
            value = min(max(value + rng.uniform(-2.0, 2.0), speed * 0.2), speed * 1.3)
            speeds[b] = value
    return speeds


def main(edges_path: str, out_dir: str) -> None:
    rows: dict[tuple[int, int], list[str]] = {}
    kinds = ["commute", "flat", "congested", "fast", "noisy"]
    with open(edges_path) as edges:
        for line in edges:
            edge, speed, _use, shortcut = line.rstrip("\n").split("\t")
            if shortcut == "1":
                continue
            level, tile, index = (int(p) for p in edge.split("/"))
            rng = Rng(mix((level << 50) | (tile << 25) | index))
            choice = rng.next() % 100
            if choice < 10:
                continue
            speed = max(float(speed), 5.0)
            free_flow = int(min(speed * rng.uniform(0.9, 1.2), 160))
            constrained = int(max(speed * rng.uniform(0.4, 0.9), 1))
            if choice < 25:
                row = f"{edge},{free_flow},{constrained}"
            else:
                kind = kinds[rng.next() % len(kinds)]
                coefficients = [int(c) for c in utils.compress_speed_buckets(profile(kind, speed, rng))]
                encoded = utils.encode_compressed_speeds(coefficients)
                if rng.next() % 2 == 0:
                    free_flow = constrained = 0
                row = f"{edge},{free_flow},{constrained},{encoded}"
            rows.setdefault((level, tile), []).append(row)

    for (level, tile), lines in sorted(rows.items()):
        directory = os.path.join(out_dir, str(level), f"{tile // 1000:03d}")
        if level == 2:
            directory = os.path.join(out_dir, str(level), f"{tile // 1000000:03d}", f"{tile // 1000 % 1000:03d}")
        os.makedirs(directory, exist_ok=True)
        with open(os.path.join(directory, f"{tile % 1000:03d}.csv"), "w") as f:
            f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
