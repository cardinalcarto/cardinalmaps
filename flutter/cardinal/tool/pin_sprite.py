#!/usr/bin/env python3
"""Draws the selected place's pins into the sprite sheet: `selection_pin`, and
`selection_pin_dark` for dark mode, which is lighter with a dark outline.

    tool/pin_sprite.py

The sheet is bitmaps at two pixels per logical pixel. Running this again redraws the pins
rather than adding others.
"""
import json
import math
import struct
import zlib

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


def read_png(path):
    raw = open(path, "rb").read()
    pos, idat, header = 8, b"", None
    while pos < len(raw):
        size, kind = struct.unpack(">I4s", raw[pos : pos + 8])
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", raw[pos + 8 : pos + 8 + size])
        elif kind == b"IDAT":
            idat += raw[pos + 8 : pos + 8 + size]
        pos += 12 + size
    width, height, depth, color, _, _, interlace = header
    assert (depth, color, interlace) == (8, 6, 0), "expected 8 bit RGBA, not interlaced"
    data = zlib.decompress(idat)
    rows, previous, offset = [], bytearray(width * 4), 0
    for _ in range(height):
        kind, line = data[offset], bytearray(data[offset + 1 : offset + 1 + width * 4])
        offset += 1 + width * 4
        for i in range(len(line)):
            left = line[i - 4] if i >= 4 else 0
            up, corner = previous[i], previous[i - 4] if i >= 4 else 0
            if kind == 1:
                line[i] = (line[i] + left) & 255
            elif kind == 2:
                line[i] = (line[i] + up) & 255
            elif kind == 3:
                line[i] = (line[i] + (left + up) // 2) & 255
            elif kind == 4:
                p = left + up - corner
                a, b, c = abs(p - left), abs(p - up), abs(p - corner)
                line[i] = (line[i] + (left if a <= b and a <= c else up if b <= c else corner)) & 255
        rows.append(line)
        previous = line
    return width, rows


def write_png(path, width, rows):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    filtered = bytearray()
    for row in rows:
        # Filter by the pixel to the left, which suits flat icons.
        filtered.append(1)
        filtered += bytes((row[i] - (row[i - 4] if i >= 4 else 0)) & 255 for i in range(len(row)))
    with open(path, "wb") as out:
        out.write(b"\x89PNG\r\n\x1a\n")
        out.write(chunk(b"IHDR", struct.pack(">IIBBBBB", width, len(rows), 8, 6, 0, 0, 0)))
        out.write(chunk(b"IDAT", zlib.compress(bytes(filtered), 9)))
        out.write(chunk(b"IEND", b""))


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
