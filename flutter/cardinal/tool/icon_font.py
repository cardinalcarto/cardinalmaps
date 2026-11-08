#!/usr/bin/env -S uv run --script
# /// script
# dependencies = ["fonttools"]
# ///
"""Builds the app's icon font from the Android vector drawables in `tool/icons`, for icons
Flutter's Material font doesn't have.

    tool/icon_font.py

Writes `assets/fonts/CardinalIcons.ttf`; `lib/app_icons.dart` names its code points.
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
EM = 960
# In code point order from 0xE000, as in `AppIcons`.
ICONS = ["ic_star", "ic_nearby", "ic_search", "ic_bus_railway", "cloud_download_24dp"]


def glyph(name):
    xml = (ROOT / "icons" / f"{name}.xml").read_text()
    data = " ".join(re.findall(r'android:pathData="([^"]+)"', xml))
    size = float(re.search(r'android:viewportHeight="([\d.]+)"', xml)[1])
    scale = EM / size
    pen = TTGlyphPen(None)
    # The font's y axis points up.
    flipped = TransformPen(Cu2QuPen(pen, 1), (scale, 0, 0, -scale, 0, EM))
    parse_path(data, flipped)
    return pen.glyph()


def main():
    empty = TTGlyphPen(None).glyph()
    glyphs = {".notdef": empty, **{name: glyph(name) for name in ICONS}}
    font = FontBuilder(EM, isTTF=True)
    font.setupGlyphOrder(list(glyphs))
    font.setupCharacterMap({0xE000 + i: name for i, name in enumerate(ICONS)})
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
    print(f"{OUT}: {len(ICONS)} icons")


if __name__ == "__main__":
    main()
