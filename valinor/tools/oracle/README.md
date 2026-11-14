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
Valinor builds those trips (see "Known divergences" in `docs/routing.md`).

#### Trip building

With a fourth argument, `trip`, the `thor` helper prints mainline's whole Thor→Odin message
for each request instead of the summary (`<id>\tok\t<base64 Api>`, or `<id>\terror\t<code> <message>`).
That is the reference for trip building (`TripLegBuilder`): the `trips_external` test in
`valinor-thor` routes each Loki→Thor message, builds its trip, and compares it with mainline's,
every field the same and floats within a relative 1e-6
(cases whose paths tie at equal cost, which path search may break either way, are listed apart).

The corpora above use mainline's default attributes.
`thor-corpus.py <region> <count> <seed> trip` also uses the options trip building reads:
attribute filters (Ferrostar's `shape_attributes` among them), `elevation_interval`, `recostings`,
and locations on indoor levels with a `search_filter` level,
picked from [`corpus/levels-andorra.tsv`](corpus/levels-andorra.tsv) and
[`corpus/levels-seattle.tsv`](corpus/levels-seattle.tsv)
(a sample of the midpoints of the edges with levels in each region's tiles, and their levels).

```shell
work=/mnt/maps/cardinal/valhalla-oracle/thor
python3 -I tools/oracle/thor-corpus.py andorra 3000 31 trip > "$work/andorra-trip-requests.tsv"
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" \
  -v "$PWD/valhalla-graphtile/fixtures/andorra-tiles:/fx:ro,z" \
  -v /mnt/maps/cardinal/valhalla-oracle/andorra-oracle:/cfg:ro,z -v "$work:/io:z" \
  valinor-valhalla-dev:3.9.1 sh -c '/w/loki /cfg/valhalla.json /fx /io/andorra-trip-requests.tsv \
    > /io/andorra-trip-loki.tsv && /w/thor /cfg/valhalla.json /fx /io/andorra-trip-loki.tsv trip \
    > /io/andorra-trip-trip.tsv'
VALINOR_THOR_ORACLE=$work/andorra-trip VALINOR_THOR_TILES=$PWD/valhalla-graphtile/fixtures/andorra-tiles \
  VALINOR_THOR_CONFIG=/mnt/maps/cardinal/valhalla-oracle/andorra-oracle \
  cargo test --release -p valinor-thor trips_external -- --ignored --nocapture
```

The default-attribute corpora (`andorra`, `predicted`, and `seattle`, with the `trip` helper output
as `<prefix>-trip.tsv`) and the trip corpora (`thor-corpus.py andorra 3000 31 trip`,
`andorra 1500 33 trip` on the predicted tiles, and `seattle 2000 32 trip`) all match:
2862, 1886, and 1953 trips, and 2869, 1442, and 1938 trips, with all of mainline's errors
(the two above among them, and two more in the Andorra trip corpus, which Valinor builds
and the test counts apart). Two Seattle trip-corpus cases have paths that tie at equal cost.
In CI, the `captured_trips` test compares the trips of `valinor-tyr`'s Andorra captures.

### Map matching (Meili)

[`cpp/meili.cc`](cpp/meili.cc) runs mainline's map matcher on trace requests, one JSON object
per line (`{"name", "action", "body"}`, the form of the shared trace corpus in
`/mnt/maps/cardinal/valhalla-oracle/traces`): `ParseApi`, `MapMatcherFactory::Create`, the
measurements Thor's `parse_measurements` makes, and `OfflineMatch` with k = `alternates` + 1 for
`trace_attributes` (1 for `trace_route`), with one factory for the run.
It prints each case's error, or every `MatchResults` (results, segments, and score),
in the text format the `oracle_tests` in `valinor-meili` also produce.
With `columns`, it also prints each column's candidates in mainline's (hash) order,
the paths the top-k search cloned, and the states it removed;
the test replays those candidates, so that every difference that remains is a bug
(or a tie between equal costs), and without the replay counts apart the differences that
mainline's hash order makes (see "Known divergences" in `docs/routing.md`).
`MEILI_DEBUG_STATE=<time>/<id>` prints that state's routes to the next column.

The helper's `walks` mode prints random walks along the graph, which
[`meili-corpus.py`](meili-corpus.py) turns into traces of every kind Meili handles
(clean, noisy, dense enough to interpolate, lingering, with gaps and breakage distances,
point types, times, U-turns, alternates, turn penalty and interpolation options,
outliers and traces off the roads, pedestrian and bicycle costings, and encoded polylines).
`valinor-meili/fixtures/oracle` holds 130 of these over the Andorra fixture tiles, with the
error cases of the shared Andorra corpus (`cut_segments` failures, a negative turn penalty
factor, and negative accuracies and radii), mainline's output for them, and the `meili`
config they use (the oracle build's, with `breakage_distance` customizable);
the `oracle_andorra` test checks them.

```shell
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" valinor-valhalla-dev:3.9.1 /w/build-meili.sh
work=/mnt/maps/cardinal/valhalla-oracle/meili
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" \
  -v "$PWD/valhalla-graphtile/fixtures/andorra-tiles:/fx:ro,z" \
  -v /mnt/maps/cardinal/valhalla-oracle/andorra-oracle:/cfg:ro,z \
  valinor-valhalla-dev:3.9.1 /w/meili /cfg/valhalla.json /fx walks 60 7 > "$work/walks-andorra.jsonl"
python3 -I tools/oracle/meili-corpus.py "$work/walks-andorra.jsonl" 130 11 \
  > valinor-meili/fixtures/oracle/andorra.jsonl
# Append the shared corpus's error cases (see above), then, with the fixture config merged
# into the oracle build's valhalla.json as $work/fixture-config.json:
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" \
  -v "$PWD/valhalla-graphtile/fixtures/andorra-tiles:/fx:ro,z" -v "$work:/cfg:ro,z" \
  -v "$PWD/valinor-meili/fixtures/oracle:/io:z" valinor-valhalla-dev:3.9.1 \
  sh -c '/w/meili /cfg/fixture-config.json /fx match /io/andorra.jsonl columns > /io/andorra-3.9.1.txt'
```

The shared corpora are checked with the ignored `oracle_external` test:

```shell
traces=/mnt/maps/cardinal/valhalla-oracle/traces
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" -v /mnt/maps/cardinal/valhalla-oracle/seattle:/cfg:ro,z \
  -v "$traces:/tr:ro,z" valinor-valhalla-dev:3.9.1 \
  sh -c '/w/meili /cfg/valhalla.json /cfg/tiles match /tr/seattle.jsonl columns' > "$work/seattle-columns.txt"
VALINOR_MEILI_CORPUS=$traces/seattle.jsonl VALINOR_MEILI_EXPECTED=$work/seattle-columns.txt \
  VALINOR_MEILI_CONFIG=/mnt/maps/cardinal/valhalla-oracle/seattle/valhalla.json \
  VALINOR_MEILI_TILES=/mnt/maps/cardinal/valhalla-oracle/seattle/tiles \
  cargo test --release -p valinor-meili oracle_external -- --ignored --nocapture
```

(Andorra likewise, with the fixture tiles and the Andorra build's config, and without
`VALINOR_MEILI_TILES`.)
Of the 350 requests with bodies in each shared corpus, Andorra has 272 cases exactly the same,
52 the same but for a node candidate's edge count, 1 tie, and 24 errors as mainline's;
Seattle has 258, 67, and 1, 3 alternates that differ by candidate order, and 20 errors.
With mainline's candidates replayed, Andorra has 325 exactly the same, and Seattle 327
and 2 ties (each a path through either of two candidates at one node).
In each, one request uses the multimodal costing, which Sif doesn't have
(Loki rejects multimodal traces with error 140 before they reach Thor).
## Map matching (trace_route and trace_attributes)

[`trace-corpus.py`](trace-corpus.py) generates reproducible trace requests as JSON lines
(`{"name", "action", "body"}`), from real routes: it routes with a staged mainline
between random points, resamples the shape at varied intervals, and adds GPS noise.
Its kinds of cases (the case name's suffix) are clean, noisy, sparse, and dense traces
(points closer than `interpolation_distance`), gaps past `breakage_distance`,
exact route shapes for `edge_walk` and `walk_or_snap`, noisy shapes for both,
point types (break, break_through, via, through), point `time`s and `begin_time` with
`durations` (with and without `use_timestamps`), U-turns, loops, GPS spikes, off-road and
very short traces, stationary points, every `trace_options` field, `alternates`, and
`filters`, for auto, pedestrian, and bicycle with costing options, in every format
(JSON, OSRM, and PBF), with `id`, `units`, and `encoded_polyline` or `shape`.
Then come fixed cases: the `service_limits.trace` limits on both sides, the trace option
bounds, and every error the trace path can give (100, 101, 103, 114, 123, 124, 125, 134,
135, 136, 140, 153, 154, 157, 158, 160, 162, 164, 172, 442, 443, 444, and 445),
some in OSRM and PBF format, plus a GET and two PBF request bodies.
The same seed, tiles, and mainline give the same corpus.

The Andorra build is `c0-fixture-build-2/build`, whose tiles are the fixture tiles
(`valhalla-graphtile/fixtures/andorra-tiles.tar`, byte for byte);
both builds' `meili` sections and trace limits are `valhalla_build_config`'s defaults.
Each line is captured once, in its own format; trace_attributes is answered by Thor,
so only trace_route cases have a Thor→Odin message.

```shell
cargo build --release -p valinor-tap
oracle=/mnt/maps/cardinal/valhalla-oracle
tools/oracle/serve-staged.sh $oracle/c0-fixture-build-2/build $oracle/traces/taps-andorra 8471 $oracle/traces/ipc-a &
tools/oracle/serve-staged.sh $oracle/seattle $oracle/traces/taps-seattle 8472 $oracle/traces/ipc-s &
python3 -I tools/oracle/trace-corpus.py andorra 300 1 --url http://localhost:8471 > $oracle/traces/andorra.jsonl
python3 -I tools/oracle/trace-corpus.py seattle 300 2 --url http://localhost:8472 > $oracle/traces/seattle.jsonl
# Routing for the corpus leaves messages in the taps; clear them before capturing.
rm -f $oracle/traces/taps-*/*/*.bin
python3 -I tools/oracle/capture.py $oracle/traces/andorra.jsonl $oracle/traces/taps-andorra \
  $oracle/captures/traces-andorra --url http://localhost:8471 --protos valhalla-proto/proto
python3 -I tools/oracle/capture.py $oracle/traces/seattle.jsonl $oracle/traces/taps-seattle \
  $oracle/captures/traces-seattle --url http://localhost:8472 --protos valhalla-proto/proto
tools/oracle/copy-trace-fixtures.sh
```

Each corpus has 354 cases (300 generated and 54 fixed). In Andorra 304 succeed and 50 fail;
in Seattle 300 and 54. Rerunning a sample of about 90 cases per region gave the same
responses, Loki→Thor messages, and Thor→Odin messages (apart from latency statistics).

[`copy-trace-fixtures.sh`](copy-trace-fixtures.sh) copies the first Andorra case of each
kind and action, and the limit and error cases, under 80 KB each:
`valinor-thor/fixtures/traces/andorra` has the Loki→Thor message of the cases that reached Thor,
with the Thor→Odin message (trace_route) or Thor's response (trace_attributes and errors),
and `valinor-tyr/fixtures/traces/andorra` has every copied case's request and response,
end to end. See the script for the layout. `valinor-thor`'s `traces_andorra` test checks the
Thor fixtures, and `valinor-router`'s `trace_tests` send the end-to-end ones through the router.

Mainline behavior to know when comparing:

- A body with `trace` instead of `shape` fails with 135, even with valid points.
- `use_timestamps` without times is never 159: parsing gives every point a time of -1
  (JSON and PBF alike).
- `trace_options.breakage_distance` only changes Loki's check (172);
  meili's breakage distance isn't customizable in the default config.
  `max_route_distance_factor` and `max_route_time_factor` in `trace_options` are ignored.
- An unknown `date_time.type` is ignored (200).
- trace_attributes with `alternates` fails with 444 when there aren't that many
  distinct matches, though the same trace matches with fewer alternates.
- `edge_walk` on the concatenated shape of a multi-leg route often fails (443),
  and now and then on a single leg's shape (in a check of 40 Seattle routes, 12 of 20
  three-stop routes and 2 of their 56 legs);
  the `edge_walk_exact` cases include both kinds of routes.
  The causes, and Valinor's lenient walk where mainline's fails, are in "Known divergences"
  in `docs/routing.md` and in `valinor-thor`'s `route_matcher`.

### Thor's map matching

`valinor-thor`'s ignored `traces_external` test runs every case of a capture run that reached
Thor (its Loki→Thor message) through `Thor::trace_route` or `Thor::trace_attributes` and
compares the Thor→Odin message (the trip, the options with the shape's correlations,
and the warnings) or Thor's response (`trace_attributes`, and errors) with mainline's:

```shell
VALINOR_THOR_TRACES=/mnt/maps/cardinal/valhalla-oracle/captures/traces-andorra \
  cargo test --release -p valinor-thor traces_external -- --ignored --nocapture
VALINOR_THOR_TRACES=/mnt/maps/cardinal/valhalla-oracle/captures/traces-seattle \
  VALINOR_THOR_TILES=/mnt/maps/cardinal/valhalla-oracle/seattle/tiles \
  VALINOR_THOR_CONFIG=/mnt/maps/cardinal/valhalla-oracle/seattle/valhalla.json \
  cargo test --release -p valinor-thor traces_external -- --ignored --nocapture
```

(`VALINOR_THOR_ONLY` picks cases by name prefix.) Each region has 322 cases that reached Thor.
In Andorra, 139 trace_route messages, 162 trace_attributes responses, and 17 errors are
mainline's; 3 differ by the documented Meili divergence (two OSRM `alternatives_count`s and a
node's zero-length edge); and 1 is a route shape mainline's edge walk fails on (443) and Valinor's
lenient walk walks. In Seattle, 132, 158, and 18 are mainline's; 7 differ by the Meili divergence
(OSRM `alternatives_count`s, alternates, and one candidate's percent along); 1 is the Meili tie
(`seattle-0194-uturn`, exact when mainline's candidates are replayed in `valinor-meili`'s oracle
test); and 6 are shapes mainline's walk fails on (4 with 443, 2 `walk_or_snap` that map match).
Each of those walks gives the same edges as map matching the shape. With the lenient walk turned
off, every one of them is mainline's.

The ground truth for the lenient walk is mainline's own routes: `valinor-thor`'s
`edge_walk_tests` walk a few legs of the Andorra route captures (and one shape joined from two
legs partway along an edge) back to the routes' edges, through the front and Loki, as mainline
walks and as Valinor does. In a one-off run over every route of the Seattle `thor` helper corpus
(2,000 routes; the test that ran it was not kept), of 2,552 single legs that are paths on the
graph, mainline's walk gives 1,944 their routes' edges and 41 other edges (short edges at a
junction that tie within `LL_EPSILON`), and fails on 567; the lenient walk gives 329 of those
their routes' edges and 10 other edges, and fails on 228. Of 349 joined multi-leg shapes,
mainline's walk gives 24 their routes' edges and fails on 323; the lenient walk gives 246 of
those their edges and 6 other edges, and fails on 71. Most of what still fails is the third
cause in `route_matcher`, which stays mainline's. The 213 legs and 271 joined shapes that turn
around partway along an edge (at via and break locations) are no paths on the graph, and 13 legs
go nowhere (one point, repeated); both were counted apart.

### The trace_attributes serializer (Tyr)

Mainline's Thor serializes trace_attributes responses itself
(`thor_worker_t::trace_attributes` calls `tyr::serializeTraceAttributes` with the request,
its strict `AttributesController`, and the map matching results),
so no message between services carries the serializer's input.
[`cpp/trace_attributes.cc`](cpp/trace_attributes.cc) runs Loki (`ParseApi` and `trace`) and then
the steps of `thor_worker_t::trace_attributes`, line for line, in a subclass of the worker
(so it can reach `map_match` and `route_match`), and dumps what the serializer got:
the `Api` (with Thor's `trip` and options), the scores and every `meili::MatchResult`
of each match (as JSON, with numbers as strings of their round-tripping digits),
and the response. It then runs the worker's own `trace_attributes` on the same Loki output
and fails if the responses differ, so the dump is what mainline's serializer saw.
For trace_route it runs Thor's `trace_route`, Odin's `DirectionsBuilder`, and
`tyr::serializeDirections`, and dumps the `Api` Odin hands Tyr (before the PBF serializer
clears it). The helper subclasses `thor_worker_t`, so it is compiled with `ENABLE_SERVICES`,
as libvalhalla is ([`cpp/build-trace.sh`](cpp/build-trace.sh)); without it, the subclass's
vtable has another layout and the worker crashes in `measure_scope_time`.

[`trace-attributes-corpus.py`](trace-attributes-corpus.py) writes 30 requests aimed at the
serializer, from the shapes of captured Andorra routes: every `shape_match` mode
(edge walks on exact shapes, which succeed, and on noisy ones, which fail),
clean and noisy traces, `polyline5`, a discontinuity, a point far from any road,
`alternates`, timestamps and `durations`, break points, strict `include` and `exclude` filters
(with an unknown name), miles with an `id` and `elevation_interval`, bicycles, PBF,
and the same kinds of trace_route requests in OSRM and Valhalla JSON.
The map matching corpora above (their JSON body lines) run through the same helper.
[`copy-trace-attributes-fixtures.py`](copy-trace-attributes-fixtures.py) copies every
targeted case and the first corpus case of each action and kind (leaving out the `limit_`
cases and any over 512 KB) into `valinor-tyr/fixtures/trace-attributes/andorra`
(`api.bin`, `matches.json` for trace_attributes, and `response.json` or `response.bin`),
which `responses_match_mainline` in `valinor-tyr`'s `trace_attributes` tests checks,
and, given `requests.tsv`, each case's `request.json`, which `valinor-router`'s `trace_tests`
send end to end.
The ignored `trace_oracle_external` test checks a whole run.

```shell
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" valinor-valhalla-dev:3.9.1 /w/build-trace.sh
work=/mnt/maps/cardinal/valhalla-oracle/trace-attributes
python3 -I tools/oracle/trace-attributes-corpus.py valinor-tyr/fixtures/captures/andorra \
  > "$work/targeted.tsv"
python3 -I -c 'import json, sys
for line in open(sys.argv[1]):
    d = json.loads(line)
    if isinstance(d.get("body"), dict) and not {"method", "raw_body", "pbf_text"} & d.keys():
        print(d["name"], d["action"], json.dumps(d["body"]), sep="\t")' \
  /mnt/maps/cardinal/valhalla-oracle/traces/andorra.jsonl > "$work/andorra-corpus.tsv"
cat "$work/targeted.tsv" "$work/andorra-corpus.tsv" > "$work/requests.tsv"
podman run --rm -v "$PWD/tools/oracle/cpp:/w:z" \
  -v "$PWD/valhalla-graphtile/fixtures/andorra-tiles:/fx:ro,z" \
  -v /mnt/maps/cardinal/valhalla-oracle/andorra-oracle:/cfg:ro,z -v "$work:/io:z" \
  valinor-valhalla-dev:3.9.1 sh -c '/w/trace_attributes /cfg/valhalla.json /fx /io/requests.tsv \
    > /io/results.tsv'
python3 -I tools/oracle/copy-trace-attributes-fixtures.py "$work/results.tsv" "$work/targeted.tsv" \
  valinor-tyr/fixtures/trace-attributes/andorra "$work/requests.tsv"
VALINOR_TYR_TRACE_ORACLE=$work/results.tsv \
  cargo test -p valinor-tyr trace_oracle_external -- --ignored --nocapture
```

On the Andorra run (30 targeted and 348 corpus requests), 182 trace_attributes responses
(JSON and PBF) and 146 trace_route responses (Valhalla JSON, OSRM, and PBF) match mainline's;
the Seattle corpus (with the Seattle build's tiles and config) gives 160 and 137.
The only difference is one OSRM miles request's approach alert, the documented divergence,
which the test leaves out.

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
  With `LOKI=valinor THOR=valinor`, mainline runs only httpd and Odin.
  On the Andorra, Seattle, Ferrostar, and Odin corpora, every response and Thor→Odin message
  matches mainline's (apart from Thor's statistics, which Valinor doesn't record,
  and the order of a request's excluded edges, which Loki doesn't reproduce),
  except for the costings Sif doesn't have yet (bus, taxi, truck, motorcycle, and motor scooter),
  which are error 107.
- With `LOKI=valinor THOR=valinor ODIN=illuvatar`, mainline runs only httpd.
  Compared with the same corpora served with mainline's Odin, every response matches
  except miles requests in OSRM format. Those differ only in voice `announcement` text,
  which is the documented approach-alert divergence:
  - Andorra: 120 responses, 2 differences.
  - Ferrostar: 7 responses, 1 difference.
  - Odin Andorra: 82 responses, 17 differences.
  - Seattle: 23 responses, 0 differences.
  - Odin Seattle: 90 responses, 18 differences.

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

## The whole router (`valinor-server`)

`valinor-server` runs every stage in one process (see "The router and the server" in
`docs/routing.md`), so it has no taps: capture its responses with an empty tap directory,
and compare them with full mainline's using `compare-runs.py --responses-only --pbf`
(PBF responses are compared apart from their statistics).
The baselines are full mainline captures: `n5-captures/{andorra,seattle}` for the shared corpora,
`ferrostar/captures` for Ferrostar, and `n4/captures/{andorra,seattle}` for the Odin corpora
(with `capture-pbf.py` for their `odin-trips-*` cases).

```shell
cargo build --release -p valinor-server
target/release/valinor-server --config /mnt/maps/cardinal/valhalla-oracle/seattle/valhalla.json \
  --tiles /mnt/maps/cardinal/valhalla-oracle/seattle/tiles.tar --listen 127.0.0.1:8413 &
mkdir -p "$work/taps"
python3 -I tools/oracle/capture.py tools/oracle/corpus/seattle.json "$work/taps" "$work/seattle" \
  --url http://localhost:8413 --protos valhalla-proto/proto
python3 -I tools/oracle/compare-runs.py /mnt/maps/cardinal/valhalla-oracle/n5-captures/seattle \
  "$work/seattle" --protos valhalla-proto/proto --responses-only --pbf
```

The Andorra corpora run on `valhalla-graphtile/fixtures/andorra-tiles.tar` with
`andorra-oracle/valhalla.json`. Every response matches but the documented divergences:

| Corpus | JSON and OSRM | PBF | Differences |
|---|---|---|---|
| Andorra | 120 | 58 | 18 for the six costings Sif lacks (107), 2 approach alerts in miles |
| Ferrostar | 7 | 8 | 1 approach alert in miles (the baseline also has `ferrostar_error_no_data`, no longer in the corpus) |
| Odin Andorra | 82 | 43 | 17 approach alerts in miles |
| Seattle | 23 | 11 | none |
| Odin Seattle | 90 | 48 | 18 approach alerts in miles |

The approach-alert responses differ only in `voiceInstructions`.
Against mainline's `valhalla_service`, the HTTP behavior matches too (status, content type,
GPX disposition, CORS, and bodies for bad methods, unknown paths, and bad bodies),
apart from the PBF content type and the actions Valinor doesn't serve yet
(`height`, `isochrone`, the matrices, `optimized_route`, and `centroid` are 107).
