#!/usr/bin/env python3
"""Captures reference data from a staged mainline Valhalla (see serve-staged.sh).

For every case in a corpus file, and for each response format the case's action supports,
this sends the request, then moves the messages the taps recorded for it
into the case's output directory, alongside the request and the final response.

Requests are sent one at a time, so the files that appear in the tap directories
while a request is in flight belong to that request.

Usage:
    capture.py <corpus.json> <tap-capture-dir> <out-dir> [--url http://localhost:8002]
               [--protos <dir with the pinned .proto files>]

With --protos and `protoc` on the PATH, each captured `valhalla.Api` message
also gets a decoded text version (`*.txt`) for humans and agents to read.

Output layout:
    <out-dir>/<case>/<format>/request.json
    <out-dir>/<case>/<format>/response.json       (status code in response.status)
    <out-dir>/<case>/pbf/response.1.bin           (PBF responses, errors included: a serialized Api)
    <out-dir>/<case>/<format>/loki-thor/*.bin     (raw frames; frame 1 is the Api message)
    <out-dir>/<case>/<format>/thor-odin/*.bin
"""

import argparse
import json
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

TAPS = ("loki-thor", "thor-odin")
# Ferrostar's Valhalla adapter asks for OSRM-format responses with these extensions.
OSRM_OPTIONS = {"format": "osrm", "banner_instructions": True, "voice_instructions": True}
ROUTE_LIKE = {"route", "optimized_route", "trace_route"}


def tap_files(tap_dir: Path) -> set[Path]:
    return {p for tap in TAPS for p in (tap_dir / tap).glob("*.bin")}


def send(url: str, action: str, request: dict) -> tuple[int, bytes]:
    body = json.dumps(request).encode()
    req = urllib.request.Request(f"{url}/{action}", data=body, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=120) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as e:
        # Valhalla reports request errors with 4xx statuses and a JSON body; those are reference data too.
        return e.code, e.read()


def decode(protos: Path, path: Path) -> None:
    with path.open("rb") as f:
        result = subprocess.run(
            ["protoc", f"-I{protos}", "--decode=valhalla.Api", "api.proto"],
            stdin=f,
            capture_output=True,
            check=False,
        )
    if result.returncode != 0:
        sys.exit(f"failed to decode {path}: {result.stderr.decode()}")
    path.with_suffix(".txt").write_bytes(result.stdout)


def wait_for_quiet(tap_dir: Path, before: set[Path]) -> set[Path]:
    """Taps write before forwarding, but give stragglers (e.g. retries) a moment to land."""
    seen = tap_files(tap_dir) - before
    for _ in range(10):
        time.sleep(0.05)
        now = tap_files(tap_dir) - before
        if now == seen:
            return now
        seen = now
    return seen


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("tap_dir", type=Path)
    parser.add_argument("out_dir", type=Path)
    parser.add_argument("--url", default="http://localhost:8002")
    parser.add_argument("--protos", type=Path)
    args = parser.parse_args()

    corpus = json.loads(args.corpus.read_text())
    names = [case["name"] for case in corpus["cases"]]
    if len(names) != len(set(names)):
        sys.exit("case names must be unique")

    for case in corpus["cases"]:
        action = case["action"]
        variants = {"json": case["request"]}
        if action in ROUTE_LIKE:
            variants["osrm"] = {**case["request"], **OSRM_OPTIONS}
            # Mainline's Odin builds directions and serializes the response in one process,
            # so its `directions` output never crosses a tapped boundary.
            # A PBF response carries it (with `info`), which makes it Odin's reference output.
            variants["pbf"] = {**case["request"], "format": "pbf"}

        for fmt, request in variants.items():
            out = args.out_dir / case["name"] / fmt
            if out.exists():
                shutil.rmtree(out)
            out.mkdir(parents=True)

            before = tap_files(args.tap_dir)
            status, body = send(args.url, action, request)
            new = wait_for_quiet(args.tap_dir, before)

            (out / "request.json").write_text(json.dumps({"action": action, "request": request}, indent=2) + "\n")
            if fmt == "pbf":
                # PBF responses are a serialized `valhalla.Api`, errors included
                # (those carry only `info`, with the error and its statistic).
                (out / "response.status").write_text(f"{status}\n")
                response = out / "response.1.bin"
                response.write_bytes(body)
                if args.protos:
                    decode(args.protos, response)
            else:
                try:
                    parsed = json.loads(body)
                except json.JSONDecodeError:
                    sys.exit(f"{case['name']}/{fmt}: non-JSON response (status {status}): {body[:200]!r}")
                (out / "response.json").write_text(json.dumps({"status": status, "body": parsed}, indent=2) + "\n")

            for path in sorted(new):
                tap = path.parent.name
                (out / tap).mkdir(exist_ok=True)
                moved = Path(shutil.move(path, out / tap / path.name))
                if args.protos and moved.name.endswith(".1.bin"):
                    decode(args.protos, moved)

            counts = {tap: len(list((out / tap).glob("*.1.bin"))) for tap in TAPS if (out / tap).exists()}
            print(f"{case['name']}/{fmt}: HTTP {status}, captured {counts or 'nothing'}")


if __name__ == "__main__":
    main()
