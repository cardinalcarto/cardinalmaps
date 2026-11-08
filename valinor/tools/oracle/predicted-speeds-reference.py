#!/usr/bin/env python3
"""Writes mainline reference values for Valinor's predicted speed codec.

Runs inside the pinned mainline image (see valhalla-image), using mainline's Python bindings:

    podman run --rm -v "$PWD/tools/oracle:/oracle:ro,z" \
        -v "$PWD/valhalla-graphtile/fixtures/predicted-speeds-reference:/out:z" \
        "$(cat tools/oracle/valhalla-image)" \
        python3 -I /oracle/predicted-speeds-reference.py /out

The inputs are not stored: they come from a small integer generator
that the Valinor test reimplements exactly (see `reference` in predicted_speeds.rs),
so keep the two in step.
Speed profiles are integers in hundredths of a kph, divided by 100 in f32,
which is exact to reproduce in any language.

Writes two files:

profiles.txt
    One line per profile: `<name> <base64>`.
    For speed profiles, the base64 is mainline's encode_compressed_speeds
    of mainline's compress_speed_buckets of the speeds.
    For coefficient profiles (coefficients rather than speeds:
    generated ones, and the one from mainline's tests),
    it is mainline's encoding of the coefficients.
decoded.bin
    For the first DECODED_PROFILES lines of profiles.txt, in order:
    mainline's decompress_speed_bucket of the profile's coefficients
    for every bucket of the week, as 2016 little-endian f32 values.
"""

import struct
import sys
import warnings

import numpy as np

with warnings.catch_warnings():
    # valhalla.utils is deprecated in favor of other modules, but is what 3.9.1 ships.
    warnings.simplefilter("ignore", DeprecationWarning)
    from valhalla import utils

BUCKETS = 2016
COEFFICIENTS = 200
GENERATED_PROFILES = 61
DECODED_PROFILES = 24
MASK = (1 << 64) - 1
MAINLINE_TEST = (
    "BcUACQAEACEADP/7/9sAGf/fAAwAGwARAAX/+AAAAB0AFAAS//AAF//+ACwACQAqAAAAKP/6AEMABABsAAsBBAAq/rcA"
    "AP+bAAz/zv/s/9MABf/Y//X/8//9/+wACf/P//EADv/8//L//P/y//H////7AAwAFf/5//oADgAZAAQAFf/3/+8AB//y"
    "AB8ABv/0AAUAEf/8//QAFAAG//b////j//v//QAT//7/+f/kABMABwABAAv/6//8//cAEwAAABT/8v/6//wAAAAQ//cA"
    "CwAFAAT/1//sAAEADAABAAYAE//9AAn/7gAH/+AAFQAB//4AC//o/+gAE//+AAAAFf/l//kABP/+//kACAAG//cAHv/q"
    "AB0AAv/4/+v/+wALAAMABP/3AAT/8wAIAAr/9wAK//j/+wAEAAD/+P/8//v/8f/2//L//AALAAcABgAG//gAAv/5AAoA"
    "Hv//AAcAFf/zABD/7AAUAAv/7v/8AAgACAAN//0ADP/iABD/9f/3//7/+P/3AAQADP//AAMABw=="
)
KINDS = [
    "uniform",
    "walk",
    "steps",
    "daily",
    "spikes",
    "kph",
    "coefficients",
    "small-coefficients",
]


class Rng:
    """splitmix64."""

    def __init__(self, seed: int):
        self.state = seed & MASK

    def next(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & MASK
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK
        return z ^ (z >> 31)

    def below(self, n: int) -> int:
        return self.next() % n


def clamp(v: int, lo: int, hi: int) -> int:
    return max(lo, min(hi, v))


def hundredths(kind: str, index: int) -> list[int]:
    """A speed profile in hundredths of a kph."""
    rng = Rng(0x5EED0000 + index)
    out = []
    if kind == "uniform":
        out = [rng.below(25001) for _ in range(BUCKETS)]
    elif kind == "walk":
        v = rng.below(12001)
        for _ in range(BUCKETS):
            v = clamp(v + rng.below(401) - 200, 0, 25000)
            out.append(v)
    elif kind == "steps":
        remaining = 0
        level = 0
        for _ in range(BUCKETS):
            if remaining == 0:
                remaining = 1 + rng.below(288)
                level = rng.below(25001)
            out.append(level)
            remaining -= 1
    elif kind == "daily":
        base = rng.below(8001)
        amp = rng.below(8001)
        phase = rng.below(288)
        out = [base + amp * abs((i + phase) % 288 - 144) // 144 for i in range(BUCKETS)]
    elif kind == "spikes":
        base = rng.below(10001)
        out = [rng.below(25001) if rng.below(20) == 0 else base for _ in range(BUCKETS)]
    elif kind == "kph":
        k = rng.below(131)
        for _ in range(BUCKETS):
            k = clamp(k + rng.below(7) - 3, 0, 140)
            out.append(k * 100)
    else:
        raise ValueError(kind)
    return out


def coefficients(kind: str, index: int) -> list[int]:
    rng = Rng(0x5EED0000 + index)
    if kind == "coefficients":
        out = []
        for _ in range(COEFFICIENTS):
            v = rng.next() & 0xFFFF
            out.append(v - 0x10000 if v >= 0x8000 else v)
        return out
    if kind == "small-coefficients":
        return [rng.below(12001)] + [rng.below(2001) - 1000 for _ in range(COEFFICIENTS - 1)]
    raise ValueError(kind)


def profiles():
    """(name, hundredths or None, coefficients or None) for every profile."""
    yield "zero", [0] * BUCKETS, None
    yield "max", [25000] * BUCKETS, None
    yield "sawtooth", [(i % 100) * 100 for i in range(BUCKETS)], None
    # The profile in mainline's predicted speed decoding test (test/predictive_traffic.cc),
    # without the stray leading byte that fixture has.
    # Edge 0/3015/42 of the andorra-tiles-with-traffic fixture carries it.
    yield "mainline-test", None, [int(c) for c in utils.decode_compressed_speeds(MAINLINE_TEST)]
    for index in range(GENERATED_PROFILES):
        kind = KINDS[index % len(KINDS)]
        name = f"{kind}-{index}"
        if kind.endswith("coefficients"):
            yield name, None, coefficients(kind, index)
        else:
            yield name, hundredths(kind, index), None


def main(out_dir: str) -> None:
    lines = []
    decoded = bytearray()
    for n, (name, speeds, coeffs) in enumerate(profiles()):
        if speeds is not None:
            array = np.array(speeds, dtype=np.float32) / np.float32(100.0)
            assert array.dtype == np.float32
            coeffs = [int(c) for c in utils.compress_speed_buckets(array)]
        encoded = utils.encode_compressed_speeds(coeffs)
        assert [int(c) for c in utils.decode_compressed_speeds(encoded)] == coeffs
        lines.append(f"{name} {encoded}\n")
        if n < DECODED_PROFILES:
            for bucket in range(BUCKETS):
                decoded += struct.pack("<f", utils.decompress_speed_bucket(coeffs, bucket))

    with open(f"{out_dir}/profiles.txt", "w") as f:
        f.write("# Mainline 3.9.1 reference values for Valinor's predicted speed codec.\n")
        f.write("# Generated by tools/oracle/predicted-speeds-reference.py; do not edit.\n")
        f.writelines(lines)
    with open(f"{out_dir}/decoded.bin", "wb") as f:
        f.write(decoded)


if __name__ == "__main__":
    main(sys.argv[1])
