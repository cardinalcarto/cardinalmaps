#!/usr/bin/env python3
"""Draws the selected place's pins into the sprite sheet: `selection_pin`, and
`selection_pin_dark` for dark mode, which is lighter with a dark outline.

    tool/pin_sprite.py

The sheet is bitmaps at two pixels per logical pixel. Running this again redraws the pins
rather than adding others.
"""
import json
import math

from sprite_sheet import read_png, write_png

SHEET = "assets/map/sprites@2x"
WIDTH, HEIGHT = 60, 76
# Name, then the colors of its body, and of its outline and dot.
PINS = [
    ("selection_pin", (0xD9, 0x30, 0x25), (0xFF, 0xFF, 0xFF)),
    ("selection_pin_dark", (0xF2, 0x8B, 0x82), (0x20, 0x21, 0x24)),
]
# The head of the pin and where its point is, in pixels.
CENTER_X, CENTER_Y, RADIUS = 30.0, 26.0, 20.0
TIP_Y = 69.0
OUTLINE = 4.0
DOT = 8.0
SAMPLES = 4


def in_pin(x, y, center_y, radius, tip_y):
    """Whether a point is in a circle with lines from it down to a point below."""
    dx, dy = x - CENTER_X, y - center_y
    if dx * dx + dy * dy <= radius * radius:
        return True
    # The tangents from the tip touch the circle where the triangle below begins.
    distance = tip_y - center_y
    if y > tip_y or y < center_y + radius * radius / distance:
        return False
    half_width = (tip_y - y) * radius / math.sqrt(distance * distance - radius * radius)
    return abs(dx) <= half_width


def pixel(px, py, body, trim):
    """The pin's color at a pixel, averaged over samples, with alpha."""
    r = g = b = a = 0
    for sy in range(SAMPLES):
        for sx in range(SAMPLES):
            x, y = px + (sx + 0.5) / SAMPLES, py + (sy + 0.5) / SAMPLES
            if (x - CENTER_X) ** 2 + (y - CENTER_Y) ** 2 <= DOT * DOT:
                color = trim
            elif in_pin(x, y, CENTER_Y, RADIUS, TIP_Y):
                color = body
            elif in_pin(x, y, CENTER_Y, RADIUS + OUTLINE, TIP_Y + OUTLINE * 1.6):
                color = trim
            else:
                continue
            r, g, b, a = r + color[0], g + color[1], b + color[2], a + 1
    if a == 0:
        return (0, 0, 0, 0)
    return (r // a, g // a, b // a, a * 255 // (SAMPLES * SAMPLES))


meta = json.load(open(SHEET + ".json"))
sheet_width, rows = read_png(SHEET + ".png")
assert WIDTH * len(PINS) <= sheet_width
first = PINS[0][0]
if first in meta:
    top = meta[first]["y"]
else:
    top = len(rows)
    rows.extend(bytearray(sheet_width * 4) for _ in range(HEIGHT))
for index, (name, body, trim) in enumerate(PINS):
    left = index * (WIDTH + 2)
    for y in range(HEIGHT):
        for x in range(WIDTH):
            rows[top + y][(left + x) * 4 : (left + x) * 4 + 4] = bytes(pixel(x, y, body, trim))
    meta[name] = {"height": HEIGHT, "pixelRatio": 2, "width": WIDTH, "x": left, "y": top}
write_png(SHEET + ".png", sheet_width, rows)
with open(SHEET + ".json", "w") as out:
    json.dump(dict(sorted(meta.items())), out, separators=(",", ":"))
