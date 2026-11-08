# Mainline Valhalla oracle

Tools for producing reference data with mainline Valhalla,
which Valinor is tested against.
Everything runs in the pinned container image named in [`valhalla-image`](valhalla-image)
(currently Valhalla 3.9.1), via rootless podman (or `CONTAINER=docker`).

## Tile builds

[`build-tiles.sh`](build-tiles.sh) builds a tile set the same way the Cardinal pipeline does:
admins, timezones, elevation, pedestrian areas, and bounding circles,
plus a tarball extract with a live traffic skeleton.

Landmarks are left out for now.
In 3.9.1, running `valhalla_add_landmarks` produces tiles that route incorrectly:
on the Andorra extract, Loki snaps a location in Andorra la Vella to an edge 5.4 km away
and an 8.07 km route comes back as 0.049 km.
Leaving out only that step fixes it;
pedestrian areas, elevation, and bounding circles are not involved.

```shell
tools/oracle/build-tiles.sh tiles <pbf> <out-dir> <minx,miny,maxx,maxy>
tools/oracle/build-tiles.sh predicted <out-dir> <csv-dir>
```

## Graph tile fixtures

[`make-graphtile-fixtures.sh`](make-graphtile-fixtures.sh) regenerates
`valhalla-graphtile/fixtures` from the pinned Andorra extract in `fixtures/source`
(checked against a SHA-256 before use).

* `andorra-tiles` and `andorra-tiles.tar` are the tile set.
* `andorra-tiles-with-traffic` is tile `0/003/015` after `valhalla_add_predicted_traffic`
  with the rows in [`predicted-speeds`](predicted-speeds),
  which match the `add_predicted_speeds` builder test.
* `andorra-traffic.tar` is the live traffic extract
  with edge `0/3015/42` set to 32 kph by [`set-live-speed.py`](set-live-speed.py).
  That script writes the mainline layout directly so the fixture does not depend on Valinor.

Note that adding predicted speeds changes the tileset build id in *every* tile header,
since mainline derives it from all tile hashes in the set.

The source extract is OpenStreetMap data from Geofabrik,
© OpenStreetMap contributors, available under the Open Database License.

## Predicted speed references

`valhalla-graphtile/fixtures/predicted-speeds-reference` holds mainline's values
for the predicted speed codec and for speed selection:

* `profiles.txt` and `decoded.bin` come from mainline's Python bindings
  (`compress_speed_buckets`, `encode_compressed_speeds`, and `decompress_speed_bucket`)
  via [`predicted-speeds-reference.py`](predicted-speeds-reference.py).
  The input profiles come from a small integer generator
  that the Valinor test reimplements, so they aren't stored.
* `get-speed.txt` is `GraphTile::GetSpeed` on `andorra-tiles-with-traffic`,
  from the C++ helper's `speeds` mode (below).

```shell
podman run --rm -v "$PWD/tools/oracle:/oracle:ro,z" \
  -v "$PWD/valhalla-graphtile/fixtures/predicted-speeds-reference:/out:z" \
  "$(cat tools/oracle/valhalla-image)" python3 -I /oracle/predicted-speeds-reference.py /out
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" -v "$PWD/valhalla-graphtile/fixtures:/fx:ro,z" \
  valinor-valhalla-dev:3.9.1 /w/oracle /fx/andorra-tiles-with-traffic speeds \
  > valhalla-graphtile/fixtures/predicted-speeds-reference/get-speed.txt
```

Mainline's float results depend on the platform's `cosf`,
so Valinor's tests compare bit for bit only on Linux with glibc, as these were made.

## Modified tiles

`valhalla-graphtile/fixtures/builder-modified` holds level 2 tile `2/000/763/927`
(which has edge bins and bounding circles)
after Valinor's `GraphTileBuilder` changes speeds on five of its edges
(see the `builder` tests; set `VALINOR_UPDATE_FIXTURES=1` to rewrite it).
`mainline-compare.txt` is the C++ helper's `compare` mode,
which reads the original and the modified tile through every mainline accessor
(header, nodes, transitions, edges, edge info, signs, lanes, restrictions, bins, circles, and speeds)
and prints what differs.
The builder tests check that only the intended values differ
and that mainline's speeds for the changed edges match Valinor's.

```shell
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" -v "$PWD/valhalla-graphtile/fixtures:/fx:ro,z" \
  valinor-valhalla-dev:3.9.1 /w/oracle compare /fx/andorra-tiles /fx/builder-modified 2 763927 \
  > valhalla-graphtile/fixtures/builder-modified/mainline-compare.txt
```

## Linking against mainline (C++)

[`cpp/`](cpp) holds a small C++ program that links mainline's `libvalhalla`
to print reference values for tile fixtures (shortcut members, opposing edges, edge bins,
bounding circles, edge info, and so on) straight from mainline's own readers.
Extend it when a test needs a value mainline computes.

```shell
podman build -t valinor-valhalla-dev:3.9.1 tools/oracle/cpp
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" valinor-valhalla-dev:3.9.1 /w/build.sh
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" -v "$PWD/valhalla-graphtile/fixtures:/fx:ro,z" \
  valinor-valhalla-dev:3.9.1 /w/oracle /fx/andorra-tiles
```

A second program, `records`, prints every directed edge, node, and node transition accessor
for a sample of each fixture tile (pass `all` for every record).
Its output is the expected-values fixture `valhalla-graphtile/fixtures/mainline-3.9.1-records.txt`:

```shell
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" -v "$PWD/valhalla-graphtile/fixtures:/fx:ro,z" \
  valinor-valhalla-dev:3.9.1 /w/records /fx/andorra-tiles \
  > valhalla-graphtile/fixtures/mainline-3.9.1-records.txt
```

### Variable-length tile sections

[`cpp/sections.cc`](cpp/sections.cc) dumps every edge info (names, tagged values,
linguistic records, levels, and elevation), sign, turn lane, access restriction,
complex restriction, lane connectivity record, and admin in a tile directory,
decoded by mainline's readers.
With `--synthetic`, it instead builds the records the fixtures lack
(landmarks, conditional speed limits, OSM node IDs, fractional levels,
timed complex restrictions, lane connectivity, and so on) with mainline's encoders,
and prints their bytes followed by mainline's decoding.
The `section_oracle` tests in `valhalla-graphtile` format the same dumps with Valinor's decoders.

```shell
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" valinor-valhalla-dev:3.9.1 /w/build-sections.sh
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" -v "$PWD/valhalla-graphtile/fixtures:/fx:ro,z" \
  valinor-valhalla-dev:3.9.1 /w/sections /fx/andorra-tiles > sections.txt
python3 tools/oracle/sections-fixture.py sections.txt \
  > valhalla-graphtile/fixtures/oracle/sections-3.9.1.txt
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" valinor-valhalla-dev:3.9.1 /w/sections --synthetic \
  > valhalla-graphtile/fixtures/oracle/sections-synthetic-3.9.1.txt
```

The full dump of the fixture tiles is several megabytes,
so [`sections-fixture.py`](sections-fixture.py) condenses it to a line count and digest
per tile and kind of line, plus a sample of the lines.
When the test reports a difference, it writes Valinor's dump to a temporary file
to diff against `sections.txt`.

The Containerfile adds a compiler and development headers to the pinned image.
(Its `FROM` line names the tag; keep it in step with [`valhalla-image`](valhalla-image).)

## Message fixtures

[`copy-message-fixtures.sh`](copy-message-fixtures.sh) copies the captured
Loki→Thor and Thor→Odin messages (see `capture.py`) into crate fixtures:
the `valhalla.Api` frames go to `valhalla-proto/fixtures/captures`,
and prime_server's 12-byte request info frames go to `valhalla-microservice/fixtures/captures`,
both as `<region>/<case>/<format>/<boundary>`.
The captures default to `/mnt/maps/cardinal/valhalla-oracle/captures`;
pass a different directory as the first argument.
The messages are derived from OpenStreetMap data,
© OpenStreetMap contributors, available under the Open Database License.

## Splicing in Valinor workers

[`serve-staged.sh`](serve-staged.sh) runs mainline as separate processes,
with taps recording the Loki→Thor and Thor→Odin messages.
Two environment variables swap Valinor components into that chain:

- `PASSTHROUGH=loki-thor` (or `thor-odin`) runs `valhalla-microservice`'s
  [`passthrough`](../../valhalla-microservice/examples/passthrough.rs) example
  in place of the tap at that boundary.
  It decodes every job and forwards the re-encoded `Api` downstream,
  so it exercises the worker protocol (advertising, forwarding, and interrupts)
  and our message definitions against mainline.
- `ODIN=illuvatar` runs `illuvatar-service` in place of mainline's Odin worker.

[`check-passthrough.sh`](check-passthrough.sh) runs a corpus twice,
once through plain staged mainline and once with the passthrough spliced in,
and compares the runs with [`compare-runs.py`](compare-runs.py):
every response must be the same,
and so must every Thor→Odin message apart from Loki's and Thor's latency statistics.
It needs `protoc` on the `PATH` to compare messages field by field.

```shell
cargo build --release -p valinor-tap
cargo build --release -p valhalla-microservice --example passthrough
tools/oracle/check-passthrough.sh <build-dir> tools/oracle/corpus/andorra.json <work-dir>
```

On the Andorra build and corpus, all 41 responses and all 34 Thor→Odin messages match.
