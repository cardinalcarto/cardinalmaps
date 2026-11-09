"""Reads and writes the sprite sheet's PNG, for the tools that draw into it."""
import struct
import zlib


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
