#!/bin/sh
# Copies the captured cases that valinor-tyr tests against into valinor-tyr/fixtures/captures.
#
# Usage: copy-tyr-fixtures.sh [captures-dir]
#
# The captures dir is what capture.py writes
# (default: /mnt/maps/cardinal/valhalla-oracle/captures).
# For each <region>/<case>, this writes:
#   trip.bin                 an Api holding only the Thor→Odin trip
#                            (identical across formats; the script checks)
#   directions.bin           an Api holding only Odin's directions, from the PBF response
#   <format>/request.json    the HTTP request (action and body; see capture.py for the
#                            "http" record of cases that send one exact request)
#   <format>/request.bin     the body of a PBF request (pbf_text cases)
#   <format>/loki-thor.bin   the Loki→Thor Api (the front's reference options, plus Loki's additions)
#   <format>/thor-odin.bin   the Thor→Odin Api without its trip (Tyr's options and info)
#   <format>/response.json   the response, re-serialized compactly ({"status": …, "body": …})
#   pbf/response.bin         the PBF response body, and pbf/response.status its HTTP status
# Splitting the trip and directions out keeps one copy of each per case,
# which keeps the fixtures small.
# The messages are derived from OpenStreetMap data,
# © OpenStreetMap contributors, available under the Open Database License.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
captures="${1:-/mnt/maps/cardinal/valhalla-oracle/captures}"
out="$here/../../valinor-tyr/fixtures/captures"

rm -rf "$out"
mkdir -p "$out"
python3 -I - "$captures" "$out" <<'PY'
import json
import sys
from pathlib import Path

captures, out = Path(sys.argv[1]), Path(sys.argv[2])


def varint(data, pos):
    shift = value = 0
    while True:
        byte = data[pos]
        pos += 1
        value |= (byte & 0x7F) << shift
        if byte < 0x80:
            return value, pos
        shift += 7


def fields(data):
    """Splits a protobuf message into (field number, raw bytes of the whole field) pairs."""
    pos = 0
    while pos < len(data):
        start = pos
        key, pos = varint(data, pos)
        wire = key & 7
        if wire == 0:
            _, pos = varint(data, pos)
        elif wire == 1:
            pos += 8
        elif wire == 2:
            length, pos = varint(data, pos)
            pos += length
        elif wire == 5:
            pos += 4
        else:
            sys.exit(f"unsupported wire type {wire}")
        yield key >> 3, data[start:pos]


def keep(data, numbers):
    return b"".join(raw for number, raw in fields(data) if number in numbers)


def frame(directory):
    frames = list(directory.glob("*.1.bin"))
    if len(frames) != 1:
        sys.exit(f"expected exactly one frame-1 message in {directory}")
    return frames[0].read_bytes()


OPTIONS, TRIP, DIRECTIONS, INFO = 1, 2, 3, 20

for case in sorted(p for p in captures.glob("*/*") if p.is_dir()):
    dest = out / case.relative_to(captures)
    trips = set()
    for fmt_dir in sorted(p for p in case.iterdir() if p.is_dir()):
        fmt_out = dest / fmt_dir.name
        fmt_out.mkdir(parents=True)
        (fmt_out / "request.json").write_bytes((fmt_dir / "request.json").read_bytes())
        if (fmt_dir / "request.bin").is_file():
            (fmt_out / "request.bin").write_bytes((fmt_dir / "request.bin").read_bytes())
        if (fmt_dir / "loki-thor").is_dir():
            (fmt_out / "loki-thor.bin").write_bytes(frame(fmt_dir / "loki-thor"))
        if (fmt_dir / "thor-odin").is_dir():
            api = frame(fmt_dir / "thor-odin")
            trips.add(keep(api, {TRIP}))
            (fmt_out / "thor-odin.bin").write_bytes(keep(api, {OPTIONS, INFO}))
        if (fmt_dir / "response.json").is_file():
            response = json.loads((fmt_dir / "response.json").read_text())
            (fmt_out / "response.json").write_text(
                json.dumps(response, separators=(",", ":"), ensure_ascii=False) + "\n"
            )
        if (fmt_dir / "response.1.bin").is_file():
            body = (fmt_dir / "response.1.bin").read_bytes()
            (fmt_out / "response.bin").write_bytes(body)
            status = (fmt_dir / "response.status").read_text()
            (fmt_out / "response.status").write_text(status)
            directions = keep(body, {DIRECTIONS})
            # A pbf_field_selector request may leave the directions out.
            if status.strip() == "200" and directions:
                (dest / "directions.bin").write_bytes(directions)
    if len(trips) > 1:
        sys.exit(f"{case}: the trip differs between formats")
    if trips:
        (dest / "trip.bin").write_bytes(trips.pop())
PY
