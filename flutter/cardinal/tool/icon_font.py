#!/usr/bin/env -S uv run --script
# /// script
# dependencies = ["fonttools"]
# ///
"""Builds the app's icon font, for icons Flutter's Material font doesn't have: the app's
own, from the Android vector drawables in `tool/icons`, and navigation's maneuver icons,
from Mapbox's directions-icons SVGs in `tool/icons/maneuvers` (see `NOTICE.md`).

    tool/icon_font.py

Writes `assets/fonts/CardinalIcons.ttf`. `lib/home/app_icons.dart` names the app's own
code points; the maneuver icons' are written to `lib/navigation/maneuver_glyphs.dart`,
by name, since there are dozens.
"""
import re
from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib.path import parse_path

ROOT = Path(__file__).parent
OUT = ROOT.parent / "assets/fonts/CardinalIcons.ttf"
MANEUVERS = ROOT / "icons/maneuvers"
DART = ROOT.parent / "lib/navigation/maneuver_glyphs.dart"
EM = 960
# In code point order from 0xE000, as in `AppIcons`. New icons go at the end, so code
# points already in use stay.
ICONS = ["ic_star", "ic_nearby", "ic_search", "ic_bus_railway", "cloud_download_24dp"]
# The maneuver icons follow, from 0xE100, in name order.
MANEUVER_BASE = 0xE100


def outline(data, size):
    """A glyph from SVG path `data` drawn in a `size`-unit square, filling the em."""
    scale = EM / size
    pen = TTGlyphPen(None)
    # The font's y axis points up.
    flipped = TransformPen(Cu2QuPen(pen, 1), (scale, 0, 0, -scale, 0, EM))
    parse_path(data, flipped)
    return pen.glyph()


def drawable(name):
    xml = (ROOT / "icons" / f"{name}.xml").read_text()
    data = " ".join(re.findall(r'android:pathData="([^"]+)"', xml))
    size = float(re.search(r'android:viewportHeight="([\d.]+)"', xml)[1])
    return outline(data, size)


def maneuver(path):
    """A maneuver icon. Each is one black path on a 20 × 20 view box, filled by the
    nonzero rule as TrueType fills. `merge_straight` also holds unfilled guide
    rectangles outside the view box, which are left out with everything but the path."""
    svg = path.read_text()
    box = re.search(r'viewBox="0 0 ([\d.]+) ([\d.]+)"', svg)
    assert box and box[1] == box[2], f"{path.name}: not a square view box"
    assert "fill-rule" not in svg, f"{path.name}: a fill rule"
    paths = re.findall(r'<path[^>]*\sd="([^"]+)"', svg)
    assert len(paths) == 1, f"{path.name}: {len(paths)} paths"
    return outline(paths[0], float(box[1]))


def write_dart(names):
    # Constant, so that release builds can still leave the font's unused glyphs out.
    lines = [
        "// Written by `tool/icon_font.py`; don't edit.",
        "",
        "import 'package:flutter/widgets.dart';",
        "",
        "/// The maneuver icons in the app's icon font, by directions-icons name: the",
        "/// maneuver's type, then its modifier, as `maneuverIconName` makes them.",
        "const maneuverGlyphs = <String, IconData>{",
        *(
            f"  '{n}': IconData(0x{MANEUVER_BASE + i:X}, fontFamily: 'CardinalIcons'),"
            for i, n in enumerate(names)
        ),
        "};",
        "",
    ]
    DART.write_text("\n".join(lines))


def main():
    empty = TTGlyphPen(None).glyph()
    maneuvers = sorted(MANEUVERS.glob("*.svg"))
    names = [p.stem for p in maneuvers]
    glyphs = {
        ".notdef": empty,
        **{name: drawable(name) for name in ICONS},
        **{f"maneuver_{p.stem}": maneuver(p) for p in maneuvers},
    }
    font = FontBuilder(EM, isTTF=True)
    font.setupGlyphOrder(list(glyphs))
    font.setupCharacterMap(
        {
            **{0xE000 + i: name for i, name in enumerate(ICONS)},
            **{MANEUVER_BASE + i: f"maneuver_{n}" for i, n in enumerate(names)},
        }
    )
    font.setupGlyf(glyphs)
    # The bearing is where the outline starts: renderers move the outline to match it.
    table = font.font["glyf"]
    font.setupHorizontalMetrics(
        {name: (EM, getattr(table[name], "xMin", 0)) for name in glyphs}
    )
    font.setupHorizontalHeader(ascent=EM, descent=0)
    font.setupNameTable({"familyName": "CardinalIcons", "styleName": "Regular"})
    font.setupOS2(sTypoAscender=EM, sTypoDescender=0, usWinAscent=EM, usWinDescent=0)
    font.setupPost()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    font.save(OUT)
    write_dart(names)
    print(f"{OUT}: {len(ICONS)} icons, {len(names)} maneuver icons")


if __name__ == "__main__":
    main()
