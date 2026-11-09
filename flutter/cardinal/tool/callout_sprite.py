#!/usr/bin/env python3
"""Draws the route callout into the sprite sheet: `route_callout`, a dark rounded rectangle
with an arrow under it, which `icon-text-fit` stretches around a route's travel time.

    tool/callout_sprite.py

The sheet is bitmaps at two pixels per logical pixel. Running this again redraws it. Only
the bands to either side of the arrow stretch across, and one through the middle of the
body stretches down, so the corners and the centered arrow keep their shape.
"""
import json

from sprite_sheet import read_png, write_png

SHEET = "assets/map/sprites@2x"
NAME = "route_callout"
WIDTH, BODY, HEIGHT = 64, 48, 60
RADIUS = 16
ARROW_HALF = 10
COLOR = (0x24, 0x24, 0x29)
# Beside the selected place's pins.
LEFT, TOP = 124, 808
SAMPLES = 4


def inside(x, y):
    """Whether a point is in the rounded rectangle or the arrow under it."""
    if y >= BODY:
        return abs(x - WIDTH / 2) <= ARROW_HALF * (HEIGHT - y) / (HEIGHT - BODY)
    dx = max(RADIUS - x, x - (WIDTH - RADIUS), 0)
    dy = max(RADIUS - y, y - (BODY - RADIUS), 0)
    return dx * dx + dy * dy <= RADIUS * RADIUS


def pixel(px, py):
    """The callout's color at a pixel, with alpha from how much of it is covered."""
    covered = sum(
        inside(px + (sx + 0.5) / SAMPLES, py + (sy + 0.5) / SAMPLES)
        for sy in range(SAMPLES)
        for sx in range(SAMPLES)
    )
    return (*COLOR, covered * 255 // (SAMPLES * SAMPLES)) if covered else (0, 0, 0, 0)


meta = json.load(open(SHEET + ".json"))
sheet_width, rows = read_png(SHEET + ".png")
assert LEFT + WIDTH <= sheet_width and TOP + HEIGHT <= len(rows)
for y in range(HEIGHT):
    for x in range(WIDTH):
        rows[TOP + y][(LEFT + x) * 4 : (LEFT + x) * 4 + 4] = bytes(pixel(x, y))
meta[NAME] = {
    # Where the text goes: the body, clear of its corners and the arrow.
    "content": [16, 12, 48, 36],
    "height": HEIGHT,
    "pixelRatio": 2,
    "stretchX": [[17, 21], [43, 47]],
    "stretchY": [[22, 26]],
    "width": WIDTH,
    "x": LEFT,
    "y": TOP,
}
write_png(SHEET + ".png", sheet_width, rows)
with open(SHEET + ".json", "w") as out:
    json.dump(dict(sorted(meta.items())), out, separators=(",", ":"))
