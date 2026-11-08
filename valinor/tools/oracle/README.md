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

### Costing (Sif)

[`cpp/sif.cc`](cpp/sif.cc) builds mainline's costings with its own option parsing and
`CostFactory`, for each costing in
[`valinor-sif/fixtures/oracle/costings.tsv`](../../valinor-sif/fixtures/oracle/costings.tsv)
(31 option sets across auto, pedestrian, and bicycle, including Thor's bidirectional
and multimodal settings), and dumps their results over a tile set:
edge costs (without a time and at several times of week), Loki's edge and node filters,
`Allowed` and `AllowedReverse` at several times, `TransitionCost` and `TransitionCostReverse`,
`TurnType`, `Restricted` along matching and diverging paths for every complex restriction,
and the A* factors and hierarchy limits.
Each line holds the inputs and mainline's result (floats as bits),
and the `oracle_tests` in `valinor-sif` repeat each call and compare exactly.
A second mode dumps `is_conditional_active` for generated time domains,
times, and time zones.
[`sif-oracle.sh`](sif-oracle.sh) runs it; see the comments at the top of both files.

```shell
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" valinor-valhalla-dev:3.9.1 /w/build-sif.sh
tools/oracle/sif-oracle.sh costing valhalla-graphtile/fixtures/andorra-tiles \
  valinor-sif/fixtures/oracle/andorra-sample.txt 5000 200 150
tools/oracle/sif-oracle.sh costing valinor-sif/fixtures/seattle-tiles \
  valinor-sif/fixtures/oracle/seattle-restrictions.txt 1000000000 1000000000 1 restrictions
tools/oracle/sif-oracle.sh costing valinor-sif/fixtures/andorra-predicted-tiles \
  valinor-sif/fixtures/oracle/andorra-predicted-sample.txt 1000000000 17 1000000000
tools/oracle/sif-oracle.sh conditional valinor-sif/fixtures/oracle/conditional.txt 3000
```

The committed dumps are samples (the strides are the last arguments):
the Andorra fixture tiles have no timed or local-traffic access restrictions
for these costings, so `seattle-tiles` holds one level 0 tile from the Seattle build,
whose timed access and complex restrictions are in `America/Los_Angeles`.
Larger dumps (every edge and every 50th node of the fixture tiles is about 580 MB)
are checked with the ignored test, in release mode:

```shell
tools/oracle/sif-oracle.sh costing valhalla-graphtile/fixtures/andorra-tiles /tmp/full.txt 50 1 1
VALINOR_SIF_DUMP=/tmp/full.txt cargo test --release -p valinor-sif full_dump -- --ignored
```

`VALINOR_SIF_TILES` points the test at another tile set
(the focus `elevators`, for example, checks the elevator level penalties in Seattle).

#### Captured trips

The trips in the captures are a second reference:
each node records the elapsed cost and time and the transition cost
along the path mainline chose.
`valinor-sif`'s trip tests recost each leg with `recost_forward`
(splitting legs at through locations, as mainline does) and compare.
The Andorra cases in `valinor-tyr/fixtures/captures` run against the fixture tiles;
other capture sets run with the ignored test:

```shell
VALINOR_SIF_CAPTURES=/mnt/maps/cardinal/valhalla-oracle/captures/seattle \
VALINOR_SIF_TILES=/mnt/maps/cardinal/valhalla-oracle/seattle/tiles \
  cargo test --release -p valinor-sif captured_trips -- --ignored --nocapture
```

Paths from bidirectional searches, which mainline recosts too, must match exactly;
see `trip_tests.rs` for the tolerances on time-dependent (unidirectional) searches.

#### Predicted speeds

None of the oracle builds has predicted speeds on many edges,
so `andorra-predicted-tiles` comes from the fixture tiles with generated, varied profiles
(rush hours, flat, congested, faster than the edge, and noisy;
see [`predicted-profiles.py`](predicted-profiles.py)),
which `valinor-sif` checks at several times of week:

```shell
work=/mnt/maps/cardinal/valhalla-oracle/andorra-predicted
mkdir -p "$work/csv" && cp -r valhalla-graphtile/fixtures/andorra-tiles "$work/tiles"
cp /mnt/maps/cardinal/valhalla-oracle/andorra-oracle/valhalla.json "$work/"
tools/oracle/sif-oracle.sh edges "$work/tiles" "$work/edges.tsv"
podman run --rm -v "$PWD/tools/oracle:/oracle:ro,z" -v "$work:/work:z" \
  valinor-valhalla-dev:3.9.1 python3 -I /oracle/predicted-profiles.py /work/edges.tsv /work/csv
tools/oracle/build-tiles.sh predicted "$work" "$work/csv"
```

The committed tiles are `0/003/015` and `2/000/763/927` of `$work/tiles-predicted`.
The profiles are a hash of each edge ID, so a rebuild gives the same tiles.

#### Truck speeds

No fixture edge has a truck speed below its speed,
so `select_speed`'s truck cap was never compared with mainline.
`valhalla-graphtile/fixtures/builder-truck` is the level 2 tile `2/000/763/927`
with truck speeds set by `GraphTileBuilder::with_truck_speed`
(half the speed, ten above it, or none, in turn; see the `builder` tests),
and `mainline-truck-speeds.txt` is mainline's `GetSpeed` for trucks on it
for several masks and times:

```shell
tools/oracle/sif-oracle.sh speeds valhalla-graphtile/fixtures/builder-truck 2 763927 \
  valhalla-graphtile/fixtures/builder-truck/mainline-truck-speeds.txt
```

The Containerfile adds a compiler and development headers to the pinned image.
(Its `FROM` line names the tag; keep it in step with [`valhalla-image`](valhalla-image).)

### Loki

[`cpp/loki.cc`](cpp/loki.cc) runs mainline's own `loki_worker_t` on a list of requests:
for each, `ParseApi` and the action (route, locate, the matrix actions, or the trace actions),
then `cleanup()`, as the worker does for each HTTP request.
It prints the `Api` that Loki sends to Thor (or the `/locate` response,
or the error and the request as it was when it failed), base64-encoded.
[`loki-corpus.py`](loki-corpus.py) generates reproducible random requests
that use every option Loki reads (radius, reachability, headings, preferred side,
street side tolerances, node snapping, search cutoffs, search filters, levels, layers,
display coordinates, exclude locations and polygons, verbose locate)
and requests that break the service limits.

`valinor-loki/fixtures/oracle/andorra` holds 2,000 of these over the Andorra fixture tiles,
which the `oracle_andorra` test checks.
Larger sets, and other regions, are checked with the ignored `oracle_external` test:

```shell
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" valinor-valhalla-dev:3.9.1 /w/build-loki.sh
python3 -I tools/oracle/loki-corpus.py andorra 2000 2 > requests.tsv
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" \
  -v "$PWD/valhalla-graphtile/fixtures/andorra-tiles:/fx:ro,z" \
  -v /mnt/maps/cardinal/valhalla-oracle/andorra-oracle:/cfg:ro,z -v "$PWD:/io:z" \
  valinor-valhalla-dev:3.9.1 sh -c '/w/loki /cfg/valhalla.json /fx /io/requests.tsv > /io/results.tsv'

# Seattle (tiles from the oracle build):
python3 -I tools/oracle/loki-corpus.py seattle 2000 3 > requests.tsv
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" -v /mnt/maps/cardinal/valhalla-oracle/seattle:/cfg:ro,z \
  -v "$PWD:/io:z" valinor-valhalla-dev:3.9.1 \
  sh -c '/w/loki /cfg/valhalla.json /cfg/tiles /io/requests.tsv > /io/results.tsv'
VALINOR_LOKI_ORACLE=$PWD VALINOR_LOKI_TILES=/mnt/maps/cardinal/valhalla-oracle/seattle/tiles.tar \
  cargo test --release -p valinor-loki oracle_external -- --ignored --nocapture
```

The Seattle build has a level 2 tile that only holds edge bins (2/792229),
whose header corner mainline never sets;
it is now a graph tile fixture (`valhalla-graphtile/fixtures/bin-only`).

### Thor

[`cpp/thor.cc`](cpp/thor.cc) runs mainline's own `thor_worker_t` (`route` and `optimized_route`)
on the `ok` lines of the `loki` helper's output, which are the `Api` messages Loki sends to Thor,
followed by `cleanup()`, as the worker does for each job.
For each request it prints a summary: the error (code and message),
or the warnings, each location's time after Thor (`date_time`, offset, and zone name),
and, for each route and leg, the algorithms and the path edges
(graph ID, the elapsed time and cost at the end of the edge, and the transition onto it,
read back from the trip that `TripLegBuilder` made).
[`thor-corpus.py`](thor-corpus.py) generates reproducible random route requests for it:
auto, pedestrian (walking, hiking, wheelchair, and blind), and bicycle with their main options,
two to five locations of every type, no time, `depart_at`, `arrive_by`, and `invariant` times
(at several times of week), alternates, headings, exclude locations,
`prioritize_bidirectional`, and pairs that can't be routed.

`valinor-thor/fixtures/oracle` holds a sample of the Andorra corpus over the fixture tiles
(every error and alternate, and every twelfth other case, with compact references),
which the `oracle_andorra` test checks.
Whole corpora, and other regions, are checked with the ignored `oracle_external` test,
which also writes the sample (`VALINOR_THOR_FIXTURE`):

```shell
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" valinor-valhalla-dev:3.9.1 /w/build-loki.sh
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" valinor-valhalla-dev:3.9.1 /w/build-thor.sh
work=/mnt/maps/cardinal/valhalla-oracle/thor
python3 -I tools/oracle/thor-corpus.py andorra 3000 11 > "$work/andorra-requests.tsv"
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" \
  -v "$PWD/valhalla-graphtile/fixtures/andorra-tiles:/fx:ro,z" \
  -v /mnt/maps/cardinal/valhalla-oracle/andorra-oracle:/cfg:ro,z -v "$work:/io:z" \
  valinor-valhalla-dev:3.9.1 sh -c '/w/loki /cfg/valhalla.json /fx /io/andorra-requests.tsv \
    > /io/andorra-loki.tsv && /w/thor /cfg/valhalla.json /fx /io/andorra-loki.tsv > /io/andorra-thor.tsv'
VALINOR_THOR_ORACLE=$work/andorra VALINOR_THOR_TILES=valhalla-graphtile/fixtures/andorra-tiles \
  VALINOR_THOR_CONFIG=/mnt/maps/cardinal/valhalla-oracle/andorra-oracle \
  VALINOR_THOR_FIXTURE=valinor-thor/fixtures/oracle/andorra VALINOR_THOR_STRIDE=12 \
  cargo test --release -p valinor-thor oracle_external -- --ignored --nocapture
```

The same goes for Seattle (`thor-corpus.py seattle 2000 12`, with the Seattle build's tiles
and config) and for time-dependent costs over predicted speeds
(`thor-corpus.py andorra 2000 13` on `andorra-predicted/tiles-predicted`,
which is too large to commit).
On these, every path, cost, error, warning, and location time matches mainline exactly:
4110 legs and 87 errors in Andorra, 2678 legs and 81 errors with predicted speeds,
and 2836 legs and 15 errors in Seattle, with 25, 8, and 36 requests with alternates.
Two requests fail in mainline's trip building, after path search
(`leg_shape_index not set for intermediate location`); the test lists them.

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

## Odin fixtures

Odin (`illuvatar`) is tested against mainline's PBF responses,
which carry the `directions` that mainline's Odin built from the captured Thor→Odin message.
Besides the shared corpora, two corpora exist for it:

- `corpus/odin-andorra.json` and `corpus/odin-seattle.json` (made with `capture.py`):
  every mainline locale on a short Andorra route with roundabouts and on a Seattle highway route
  with exits and forks (alternating kilometers and miles),
  plus a ferry, an elevator, a numbered exit, a cycle trail, a long hike, blind pedestrians,
  combined roundabout entry and exit, a language alias, and the `maneuvers` and `none`
  directions types.
- `corpus/odin-trips-andorra.json` and `corpus/odin-trips-seattle.json`
  (made with [`capture-pbf.py`](capture-pbf.py), which sends PBF requests):
  routes whose responses also carry the `trip`,
  since `pbf_field_selector` can only be set in a PBF request.
  That trip has Odin's in-place updates (turn lanes and short-edge headings).

Rarer maneuvers and phrases (building entrances, levels, parking, bike share, every kind of sign,
blind pedestrians, phoneme markup, very long lengths, and Odin's errors) come from hand-built trips
in `illuvatar/src/synthetic.rs`.
[`odin-synthetic.sh`](odin-synthetic.sh) writes them to `illuvatar/fixtures/synthetic`
and runs them through mainline's own `DirectionsBuilder` with the `odin` helper in [`cpp/`](cpp)
(build it first with `cpp/build.sh`, as above), so the references still come from mainline.

[`copy-illuvatar-fixtures.sh`](copy-illuvatar-fixtures.sh) copies the captured cases into
`illuvatar/fixtures`, keeping the synthetic cases:

```shell
tools/oracle/copy-illuvatar-fixtures.sh /mnt/maps/cardinal/valhalla-oracle/captures <odin-captures-dir>
```

## Valinor-tyr fixtures

[`copy-tyr-fixtures.sh`](copy-tyr-fixtures.sh) copies whole captured cases
(requests, stage messages, and responses) into `valinor-tyr/fixtures/captures`,
keeping one trip and one set of directions per case to save space.
Its fixtures currently come from a capture of the full corpora in
`/mnt/maps/cardinal/valhalla-oracle/n5-captures`;
the cases that are also in the shared captures matched them exactly,
apart from latency statistics, so those files were left as they were.
Fifteen Andorra cases whose first capture missed the tapped messages
were captured again (their responses were unchanged) and copied into that directory.

The `ferrostar_*` cases come from `corpus/ferrostar-andorra.json`. Each sends the exact body
Ferrostar's `ValhallaHttpRequestGenerator` builds:
- the OSRM format with banner and voice instructions;
- the `shape_attributes` filter that turns on annotations;
- a `street_side_tolerance` (and sometimes a `heading`) on the start;
- typed waypoints;
- options merged on top.

Every case is an exact request, so each is sent twice: once as is,
and once as a `_pbf` copy with `format: pbf` for the directions.
They were captured into `/mnt/maps/cardinal/valhalla-oracle/ferrostar/captures`.
The `_pbf` copy's `pbf` directory was then moved into its case
(`ferrostar/merged`), and the cases were copied in with this script's logic.

## Corpus cases

[`capture.py`](capture.py) POSTs each case's `request` as JSON,
once per format its action supports
(JSON; for route-like actions, also OSRM format with Ferrostar's options, and PBF).
A case can instead describe one exact HTTP request,
which is sent once, into the format directory named by its `format` field
(or by the request's `format`, or `pbf` for PBF bodies, or `json`):

- `"method": "GET"` sends `GET /<action>?json=<url-encoded request>`;
  any other method (such as `PUT`) sends the JSON body with that method.
- `"query": {...}` appends url-encoded query parameters, in order.
- `"raw_body": "..."` sends a body verbatim, for example malformed JSON.
- `"pbf_text": "..."` is a `valhalla.Api` in protobuf text format,
  encoded with `protoc` (so `--protos` is required)
  and POSTed as `application/x-protobuf`.
  The encoded body is saved as `request.bin`.

The `request.json` of these cases records the exact request under `http`
(its method, target, content type, and body, or `body_file` for PBF bodies),
which the front's tests replay.

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
- `THOR=valinor` runs `valinor-thor-service` in place of mainline's Thor worker,
  with the build's `valhalla.json` and `tiles.tar`.
  Until trip building is written it answers routes it finds with error 107,
  so only its errors (such as 442) can be compared with mainline's end to end.

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
