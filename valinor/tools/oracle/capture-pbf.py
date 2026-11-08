#!/usr/bin/env python3
"""Captures mainline responses to PBF-encoded requests (see capture.py for JSON requests).

Some options can only be set in a PBF request; in 3.9.1 that includes `pbf_field_selector`,
which asks for the `trip` in the response.
With the trip selected, the response shows the trip *after* Odin,
which updates it in place (turn lanes and short-edge headings),
so these captures are the reference for those updates.

Each case gives the request as a `valhalla.Api` in protobuf text format,
which this script encodes with `protoc`.

Usage:
    capture-pbf.py <corpus.json> <tap-capture-dir> <out-dir> --protos <dir> [--url http://localhost:8002]

Corpus format:
    {"region": "...", "cases": [{"name": "...", "action": "route", "request_text": "options { ... }"}]}

Output layout (the same as capture.py's PBF variant):
    <out-dir>/<case>/pbf/request.txt
    <out-dir>/<case>/pbf/response.1.bin   (and response.1.txt; status in response.status)
    <out-dir>/<case>/pbf/<tap>/*.bin
"""

import argparse
import json
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

from capture import TAPS, decode, tap_files, wait_for_quiet


def encode(protos: Path, text: str) -> bytes:
    result = subprocess.run(
        ["protoc", f"-I{protos}", "--encode=valhalla.Api", "api.proto"],
        input=text.encode(),
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        sys.exit(f"failed to encode request: {result.stderr.decode()}")
    return result.stdout


def send(url: str, action: str, body: bytes) -> tuple[int, bytes]:
    req = urllib.request.Request(
        f"{url}/{action}", data=body, method="POST", headers={"Content-Type": "application/x-protobuf"}
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("tap_dir", type=Path)
    parser.add_argument("out_dir", type=Path)
    parser.add_argument("--url", default="http://localhost:8002")
    parser.add_argument("--protos", type=Path, required=True)
    args = parser.parse_args()

    corpus = json.loads(args.corpus.read_text())
    for case in corpus["cases"]:
        out = args.out_dir / case["name"] / "pbf"
        if out.exists():
            shutil.rmtree(out)
        out.mkdir(parents=True)

        body = encode(args.protos, case["request_text"])
        before = tap_files(args.tap_dir)
        status, response = send(args.url, case["action"], body)
        new = wait_for_quiet(args.tap_dir, before)

        (out / "request.txt").write_text(case["request_text"] + "\n")
        (out / "response.status").write_text(f"{status}\n")
        (out / "response.1.bin").write_bytes(response)
        decode(args.protos, out / "response.1.bin")
        for path in sorted(new):
            (out / path.parent.name).mkdir(exist_ok=True)
            moved = Path(shutil.move(path, out / path.parent.name / path.name))
            if moved.name.endswith(".1.bin"):
                decode(args.protos, moved)

        counts = {tap: len(list((out / tap).glob("*.1.bin"))) for tap in TAPS if (out / tap).exists()}
        print(f"{case['name']}/pbf: HTTP {status}, captured {counts or 'nothing'}")


if __name__ == "__main__":
    main()
