#!/usr/bin/env python3
"""Compares two capture.py runs over the same corpus.

Every case and format in the first run must be in the second, with the same HTTP status
and the same response body (compared as parsed JSON, so key order does not matter).
Where both runs recorded a Thor→Odin message, the messages must match too,
except for the latency statistics that Loki and Thor record in `info`,
which vary from run to run.
Messages whose bytes differ are decoded with `protoc` to compare them field by field.

Usage:
    compare-runs.py <baseline-out-dir> <other-out-dir> --protos <dir with the pinned .proto files>
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

# A latency statistic in protoc's text format: the key line, then its value line.
LATENCY = re.compile(r'(key: "[^"]*latency_ms"\n\s*value: )[-0-9.e+]+')


def thor_odin(case_dir: Path) -> Path | None:
    messages = sorted((case_dir / "thor-odin").glob("*.1.bin"))
    if len(messages) > 1:
        sys.exit(f"{case_dir}: expected at most one Thor→Odin message, found {len(messages)}")
    return messages[0] if messages else None


def decode(protos: Path, path: Path) -> str:
    with path.open("rb") as f:
        result = subprocess.run(
            ["protoc", f"-I{protos}", "--decode=valhalla.Api", "api.proto"],
            stdin=f,
            capture_output=True,
            check=False,
        )
    if result.returncode != 0:
        sys.exit(f"failed to decode {path}: {result.stderr.decode()}")
    return result.stdout.decode()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("other", type=Path)
    parser.add_argument("--protos", type=Path, required=True)
    args = parser.parse_args()

    differences = []
    responses = messages = identical = 0
    for response in sorted(args.baseline.glob("*/*/response.json")):
        case_dir = response.parent
        relative = case_dir.relative_to(args.baseline)
        other_dir = args.other / relative
        other_response = other_dir / "response.json"
        if not other_response.exists():
            differences.append(f"{relative}: missing from {args.other}")
            continue

        if json.loads(response.read_text()) != json.loads(other_response.read_text()):
            differences.append(f"{relative}: responses differ")
        responses += 1

        a, b = thor_odin(case_dir), thor_odin(other_dir)
        if a is None or b is None:
            if (a is None) != (b is None):
                differences.append(f"{relative}: only one run has a Thor→Odin message")
            continue
        messages += 1
        if a.read_bytes() == b.read_bytes():
            identical += 1
            continue
        text_a = LATENCY.sub(r"\1<latency>", decode(args.protos, a))
        text_b = LATENCY.sub(r"\1<latency>", decode(args.protos, b))
        if text_a != text_b:
            differences.append(f"{relative}: Thor→Odin messages differ beyond latency statistics")

    for difference in differences:
        print(difference)
    print(
        f"compared {responses} responses and {messages} Thor→Odin messages "
        f"({identical} byte-identical, the rest equal except for latency statistics): "
        f"{len(differences)} differences"
    )
    sys.exit(1 if differences or responses == 0 else 0)


if __name__ == "__main__":
    main()
