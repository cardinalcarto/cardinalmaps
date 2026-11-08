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
    <out-dir>/<case>/<format>/request.bin         (the body of `pbf_text` cases)
    <out-dir>/<case>/<format>/response.json       (status code in response.status)
    <out-dir>/<case>/pbf/response.1.bin           (PBF responses, errors included: a serialized Api)
    <out-dir>/<case>/<format>/loki-thor/*.bin     (raw frames; frame 1 is the Api message)
    <out-dir>/<case>/<format>/thor-odin/*.bin

Ordinary cases are POSTed as JSON, once per format the action supports.
A case can instead describe one exact HTTP request with these fields,
in which case only that request is sent:

    "method": "GET"        GET /<action>?json=<url-encoded request> (or any other method,
                           which sends the request as a POST body would be sent)
    "query": {"k": "v"}    extra query parameters, url-encoded and appended in order
    "raw_body": "..."      a body sent verbatim instead of the JSON `request`
    "pbf_text": "..."      a valhalla.Api in protobuf text format, encoded with protoc
                           (needs --protos) and POSTed as application/x-protobuf
    "format": "json"       the format directory to capture into
                           (default: the request's `format`, or pbf for `pbf_text`, or json)

The request.json of such a case also records the exact request under "http":
its method, target (path and query), content type, and body
(as a string, or "body_file": "request.bin" for PBF bodies).
"""

import argparse
import json
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

TAPS = ("loki-thor", "thor-odin")
# Ferrostar's Valhalla adapter asks for OSRM-format responses with these extensions.
OSRM_OPTIONS = {"format": "osrm", "banner_instructions": True, "voice_instructions": True}
ROUTE_LIKE = {"route", "optimized_route", "trace_route"}


def tap_files(tap_dir: Path) -> set[Path]:
    return {p for tap in TAPS for p in (tap_dir / tap).glob("*.bin")}


SPECIAL_FIELDS = ("method", "query", "raw_body", "pbf_text")
PBF_MIME = "application/x-protobuf"


def send(url: str, action: str, request: dict) -> tuple[int, bytes]:
    return send_http(url, "POST", f"/{action}", None, json.dumps(request).encode())


def send_http(url: str, method: str, target: str, content_type: str | None, body: bytes | None) -> tuple[int, bytes]:
    headers = {"Content-Type": content_type} if content_type else {}
    req = urllib.request.Request(f"{url}{target}", data=body, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=120) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as e:
        # Valhalla reports request errors with 4xx statuses and a JSON body; those are reference data too.
        return e.code, e.read()


def encode(protos: Path, text: str) -> bytes:
    result = subprocess.run(
        ["protoc", f"-I{protos}", "--encode=valhalla.Api", "api.proto"],
        input=text.encode(),
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        sys.exit(f"failed to encode pbf_text: {result.stderr.decode()}")
    return result.stdout


def special_request(case: dict, protos: Path | None) -> tuple[str, dict, str, str, str | None, bytes | None]:
    """The format dir, the recorded HTTP request, and what to send for a case with SPECIAL_FIELDS."""
    action = case["action"]
    request = case.get("request")
    method = case.get("method", "POST")
    query = "&".join(
        f"{urllib.parse.quote(k, safe='')}={urllib.parse.quote(str(v), safe='')}"
        for k, v in case.get("query", {}).items()
    )
    content_type = None
    http: dict = {"method": method}
    if "pbf_text" in case:
        if protos is None:
            sys.exit(f"{case['name']}: pbf_text cases need --protos")
        body = encode(protos, case["pbf_text"])
        content_type = PBF_MIME
        http["body_file"] = "request.bin"
        default_format = "pbf"
    else:
        if "raw_body" in case:
            body = case["raw_body"].encode()
        elif method == "GET":
            body = None
            json_param = "json=" + urllib.parse.quote(json.dumps(request), safe="")
            query = f"{json_param}&{query}" if query else json_param
        else:
            body = json.dumps(request).encode()
        if body is not None:
            http["body"] = body.decode()
        default_format = (request or {}).get("format", "json")
    target = f"/{action}" + (f"?{query}" if query else "")
    http["target"] = target
    http["content_type"] = content_type
    return case.get("format", default_format), http, method, target, content_type, body


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
        if any(field in case for field in SPECIAL_FIELDS):
            capture_special(case, args)
            continue
        variants = {"json": case["request"]}
        if action in ROUTE_LIKE:
            variants["osrm"] = {**case["request"], **OSRM_OPTIONS}
            # Mainline's Odin builds directions and serializes the response in one process,
            # so its `directions` output never crosses a tapped boundary.
            # A PBF response carries it (with `info`), which makes it Odin's reference output.
            variants["pbf"] = {**case["request"], "format": "pbf"}

        for fmt, request in variants.items():
            out = fresh_dir(args.out_dir / case["name"] / fmt)
            before = tap_files(args.tap_dir)
            status, body = send(args.url, action, request)
            new = wait_for_quiet(args.tap_dir, before)
            (out / "request.json").write_text(json.dumps({"action": action, "request": request}, indent=2) + "\n")
            record(case["name"], fmt, out, status, body, new, args)


def fresh_dir(out: Path) -> Path:
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    return out


def capture_special(case: dict, args: argparse.Namespace) -> None:
    """Sends the one exact HTTP request a case describes (see SPECIAL_FIELDS)."""
    fmt, http, method, target, content_type, body = special_request(case, args.protos)
    out = fresh_dir(args.out_dir / case["name"] / fmt)
    before = tap_files(args.tap_dir)
    status, response = send_http(args.url, method, target, content_type, body)
    new = wait_for_quiet(args.tap_dir, before)
    recorded = {"action": case["action"], "request": case.get("request"), "http": http}
    if "pbf_text" in case:
        recorded["pbf_text"] = case["pbf_text"]
        (out / "request.bin").write_bytes(body or b"")
    (out / "request.json").write_text(json.dumps(recorded, indent=2) + "\n")
    record(case["name"], fmt, out, status, response, new, args)


def record(name: str, fmt: str, out: Path, status: int, body: bytes, new: set[Path], args: argparse.Namespace) -> None:
    """Writes a response and moves the tapped messages for it into `out`."""
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
            sys.exit(f"{name}/{fmt}: non-JSON response (status {status}): {body[:200]!r}")
        (out / "response.json").write_text(json.dumps({"status": status, "body": parsed}, indent=2) + "\n")

    for path in sorted(new):
        tap = path.parent.name
        (out / tap).mkdir(exist_ok=True)
        moved = Path(shutil.move(path, out / tap / path.name))
        if args.protos and moved.name.endswith(".1.bin"):
            decode(args.protos, moved)

    counts = {tap: len(list((out / tap).glob("*.1.bin"))) for tap in TAPS if (out / tap).exists()}
    print(f"{name}/{fmt}: HTTP {status}, captured {counts or 'nothing'}")


if __name__ == "__main__":
    main()
