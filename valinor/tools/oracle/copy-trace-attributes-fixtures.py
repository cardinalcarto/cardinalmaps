#!/usr/bin/env python3
"""Copies a sample of the `trace_attributes` helper's results into `valinor-tyr/fixtures`.

Usage: copy-trace-attributes-fixtures.py <results.tsv> <targeted.tsv> <out-dir>

<results.tsv> is the output of `cpp/trace_attributes.cc`; <targeted.tsv> holds the requests of
`trace-attributes-corpus.py`, all of which are kept. Of the other (corpus) cases,
the first successful one of each action and kind (the name after `<region>-<number>-`)
is kept, so every kind of trace is covered without committing the whole run.
The `limit_` kinds (traces at the service limits, some of them megabytes) are left out,
since they test Loki's checks, not the serializer; so are corpus cases over 512 KB,
in favor of a smaller one of the same kind.
Each case becomes a directory with:

- `api.bin`: the `valhalla.Api` the serializer got (for `trace_route`, Odin's output),
- `matches.json`: the match results and scores (`trace_attributes` only),
- `response.json` or `response.bin`: mainline's response body.

Errors are not copied: they happen before serialization.
"""

import base64
import shutil
import sys
from pathlib import Path


def main():
    results, targeted, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    keep = {line.split("\t", 1)[0] for line in targeted.read_text().splitlines() if line}
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    kinds = set()
    copied = 0
    for line in results.read_text().splitlines():
        fields = line.split("\t")
        case, status = fields[0], fields[1]
        if status != "ok":
            continue
        api, matches, response = (base64.b64decode(f) for f in fields[2:5])
        if case not in keep:
            if "-limit_" in case or len(api) + len(matches) + len(response) > 512 * 1024:
                continue
            parts = case.split("-", 2)
            kind = (bool(matches), parts[2] if len(parts) == 3 else case)
            if kind in kinds:
                continue
            kinds.add(kind)
        directory = out / case
        directory.mkdir()
        (directory / "api.bin").write_bytes(api)
        if matches:
            (directory / "matches.json").write_bytes(matches + b"\n")
        # JSON responses start with `{`; PBF ones are `valhalla.Api` bytes.
        name = "response.json" if response.startswith(b"{") else "response.bin"
        (directory / name).write_bytes(response)
        copied += 1
    print(f"copied {copied} cases", file=sys.stderr)


if __name__ == "__main__":
    main()
