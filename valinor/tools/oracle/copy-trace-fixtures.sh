#!/bin/sh
# Copies a small, representative subset of the captured Andorra trace cases
# (trace-corpus.py, captured with capture.py) into crate fixtures.
#
# Usage: copy-trace-fixtures.sh [captures-dir]
#
# The captures dir defaults to /mnt/maps/cardinal/valhalla-oracle/captures/traces-andorra.
# The subset is the first case of each kind and action (in name order), and every
# limit and error case, whose captured files total under 80 KB.
#
# valinor-thor/fixtures/traces/andorra/<case>/ (cases that reached Thor):
#   request.json    the HTTP request (action and body; see capture.py),
#                   and request.bin for PBF request bodies
#   loki-thor.bin   the Loki→Thor Api, Thor's input
#   thor-odin.bin   the Thor→Odin Api, Thor's output for trace_route
#   response.json   the response (or response.bin and response.status for PBF),
#                   which Thor serializes itself for trace_attributes and for errors
# valinor-tyr/fixtures/traces/andorra/<case>/ (every case, end to end):
#   request.json (and request.bin), and response.json or response.bin and response.status
#
# The messages are derived from OpenStreetMap data,
# © OpenStreetMap contributors, available under the Open Database License.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
captures="${1:-/mnt/maps/cardinal/valhalla-oracle/captures/traces-andorra}"
thor="$here/../../valinor-thor/fixtures/traces/andorra"
tyr="$here/../../valinor-tyr/fixtures/traces/andorra"

rm -rf "$thor" "$tyr"
mkdir -p "$thor" "$tyr"
python3 -I - "$captures" "$thor" "$tyr" <<'PY'
import json
import shutil
import sys
from pathlib import Path

captures, thor, tyr = (Path(p) for p in sys.argv[1:4])
LIMIT = 80_000


def frame(directory):
    frames = list(directory.glob("*.1.bin"))
    if len(frames) != 1:
        sys.exit(f"expected exactly one frame-1 message in {directory}")
    return frames[0]


def responses(fmt_dir):
    return [p for p in (fmt_dir / "response.json", fmt_dir / "response.1.bin", fmt_dir / "response.status") if p.exists()]


def copy_request(fmt_dir, dest):
    """request.json, and request.bin for PBF request bodies."""
    for name in ("request.json", "request.bin"):
        if (fmt_dir / name).exists():
            shutil.copy(fmt_dir / name, dest)


def copy_response(fmt_dir, dest):
    for path in responses(fmt_dir):
        if path.name == "response.json":
            response = json.loads(path.read_text())
            (dest / path.name).write_text(json.dumps(response, separators=(",", ":"), ensure_ascii=False) + "\n")
        else:
            shutil.copy(path, dest / path.name.replace(".1.bin", ".bin"))


seen = set()
counts = {"thor": 0, "tyr": 0}
for case in sorted(p for p in captures.iterdir() if p.is_dir()):
    # Each trace case is captured in exactly one format.
    (fmt_dir,) = [p for p in case.iterdir() if p.is_dir()]
    action = json.loads((fmt_dir / "request.json").read_text())["action"]
    parts = case.name.split("-", 2)
    kind = parts[2] if parts[1].isdigit() else case.name
    taps = [frame(fmt_dir / t) for t in ("loki-thor", "thor-odin") if (fmt_dir / t).is_dir()]
    size = sum(p.stat().st_size for p in [fmt_dir / "request.json", *taps, *responses(fmt_dir)])
    if (kind, action) in seen or size > LIMIT:
        continue
    seen.add((kind, action))

    dest = tyr / case.name
    dest.mkdir()
    copy_request(fmt_dir, dest)
    copy_response(fmt_dir, dest)
    counts["tyr"] += 1
    if (fmt_dir / "loki-thor").is_dir():
        dest = thor / case.name
        dest.mkdir()
        copy_request(fmt_dir, dest)
        shutil.copy(frame(fmt_dir / "loki-thor"), dest / "loki-thor.bin")
        if (fmt_dir / "thor-odin").is_dir():
            shutil.copy(frame(fmt_dir / "thor-odin"), dest / "thor-odin.bin")
        else:
            copy_response(fmt_dir, dest)
        counts["thor"] += 1
print(f"copied {counts['thor']} cases for valinor-thor and {counts['tyr']} for valinor-tyr")
PY
du -sh "$thor" "$tyr"
