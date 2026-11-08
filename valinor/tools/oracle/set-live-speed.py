#!/usr/bin/env python3
"""Sets a live traffic speed on one edge of a Valhalla traffic.tar, in place.

This deliberately does not use Valinor:
the result is a test fixture for Valinor's traffic reader,
so it is written from the layout in mainline's baldr/traffictile.h (3.9.1).

Layout of each traffic tile inside the tar:
    TrafficTileHeader (32 bytes: tile_id u64, last_update u64,
                       directed_edge_count u32, traffic_tile_version u32, 2 x u32 spare)
    directed_edge_count x TrafficSpeed (8 bytes each, little endian bitfield, LSB first):
        overall_encoded_speed:7, encoded_speed1:7, encoded_speed2:7, encoded_speed3:7,
        breakpoint1:8, breakpoint2:8, congestion1:6, congestion2:6, congestion3:6,
        has_incidents:1, spare:1

The edge gets a single speed over its full length,
the way a typical external traffic feed writes it:
overall and first-segment speeds set, other segments unknown, breakpoint1 = 255.

Usage: set-live-speed.py <traffic.tar> <level>/<tile>/<edge index> <speed kph>
"""

import struct
import sys
import tarfile

HEADER_SIZE = 32
SPEED_SIZE = 8
UNKNOWN_SPEED_RAW = (1 << 7) - 1
# Tile levels use 3-digit path groups; level 2 tile ids need more digits than levels 0 and 1.
LEVEL_DIGITS = {0: 6, 1: 6, 2: 9}


def tile_member_name(level: int, tile: int) -> str:
    digits = f"{tile:0{LEVEL_DIGITS[level]}d}"
    groups = [digits[i : i + 3] for i in range(0, len(digits), 3)]
    return f"{level}/" + "/".join(groups) + ".gph"


def encode_single_speed(kph: int) -> int:
    if kph % 2 != 0 or not 0 < kph <= (UNKNOWN_SPEED_RAW - 1) * 2:
        raise ValueError(f"speed must be an even kph value in (0, 252], got {kph}")
    encoded = kph // 2
    fields = [
        (encoded, 7),  # overall_encoded_speed
        (encoded, 7),  # encoded_speed1
        (UNKNOWN_SPEED_RAW, 7),  # encoded_speed2
        (UNKNOWN_SPEED_RAW, 7),  # encoded_speed3
        (255, 8),  # breakpoint1: the first segment covers the whole edge
        (0, 8),  # breakpoint2
        (0, 6),  # congestion1 (unknown)
        (0, 6),  # congestion2
        (0, 6),  # congestion3
        (0, 1),  # has_incidents
        (0, 1),  # spare
    ]
    value = 0
    shift = 0
    for field, bits in fields:
        assert field < (1 << bits)
        value |= field << shift
        shift += bits
    assert shift == 64
    return value


def main() -> None:
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    tar_path, edge_id, kph = sys.argv[1], sys.argv[2], int(sys.argv[3])
    level, tile, index = (int(part) for part in edge_id.split("/"))
    name = tile_member_name(level, tile)

    with tarfile.open(tar_path, "r:") as tar:
        member = tar.getmember(name)
        data_offset = member.offset_data
        size = member.size

    with open(tar_path, "r+b") as f:
        f.seek(data_offset)
        _tile_id, _last_update, edge_count, _version = struct.unpack("<QQII", f.read(24))
        if index >= edge_count:
            sys.exit(f"edge index {index} out of range: tile {name} has {edge_count} edges")
        speed_offset = HEADER_SIZE + index * SPEED_SIZE
        if speed_offset + SPEED_SIZE > size:
            sys.exit(f"tile {name} is truncated ({size} bytes)")
        f.seek(data_offset + speed_offset)
        f.write(struct.pack("<Q", encode_single_speed(kph)))
    print(f"set {edge_id} in {name} to {kph} kph")


if __name__ == "__main__":
    main()
