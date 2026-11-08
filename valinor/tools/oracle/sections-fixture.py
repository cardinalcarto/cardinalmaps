#!/usr/bin/env python3
"""Condenses the output of `cpp/sections <tile_dir>` into a graph tile test fixture.

The full dump of the Andorra fixtures is several megabytes,
so the fixture keeps, for each tile and kind of line,
the line count and an MD5 digest of the lines,
plus a sample of the lines themselves (so that a mismatch points somewhere useful).
The `section_oracle` test in valhalla-graphtile formats the same dump with Valinor's decoders,
groups it the same way, and compares.

Usage: sections-fixture.py <sections output> > valhalla-graphtile/fixtures/oracle/sections-3.9.1.txt
"""

import hashlib
import sys

# Groups up to this size are kept whole.
SMALL_GROUP = 400
# For larger groups, keep this many leading lines and then every STRIDE-th line.
LEADING = 40
STRIDE = 997
# Nested edge info lines that are too common to sample individually.
COMMON_NESTED = ("  name ", "  linguistic", "  layer ", "  tag 1 ")


def groups(lines):
    """Groups lines by (tile, kind), in order of first appearance.

    A line indented by one space starts a group named by its first token;
    a line indented by two spaces belongs to the group of the line before it.
    """
    result = {}
    tile = None
    kind = None
    for line in lines:
        # Mainline logs (for example "No signs found") go to stdout with a timestamp.
        if not line.startswith(" ") and not line.startswith("tile "):
            continue
        if line.startswith("tile "):
            tile = line.split(" ", 1)[1]
            continue
        if not line.startswith("  "):
            kind = line.split(" ", 2)[1]
        result.setdefault((tile, kind), []).append(line)
    return result


def main():
    with open(sys.argv[1], encoding="utf-8", errors="surrogateescape") as f:
        lines = f.read().split("\n")
    print("# Mainline Valhalla 3.9.1's decoding of the variable-length sections of the fixture tiles,")
    print("# condensed by tools/oracle/sections-fixture.py from the output of tools/oracle/cpp/sections.")
    print("# group<TAB>tile<TAB>kind<TAB>line count<TAB>MD5 of the lines (each ending in a newline)")
    print("# sample<TAB>tile<TAB>kind<TAB>index in the group<TAB>line")
    for (tile, kind), group in groups(lines).items():
        digest = hashlib.md5("".join(l + "\n" for l in group).encode("utf-8", "surrogateescape"))
        print(f"group\t{tile}\t{kind}\t{len(group)}\t{digest.hexdigest()}")
        for i, line in enumerate(group):
            keep = (
                len(group) <= SMALL_GROUP
                or i < LEADING
                or i % STRIDE == 0
                or (line.startswith("  ") and not line.startswith(COMMON_NESTED))
            )
            if keep:
                print(f"sample\t{tile}\t{kind}\t{i}\t{line}")


if __name__ == "__main__":
    main()
