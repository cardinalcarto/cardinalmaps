# Publishing releases

`cardinal-publish` uploads a release of Cardinal's data to an S3-compatible
bucket that clients read over plain HTTPS: the map as one PMTiles archive, the
Valhalla routing tiles loose for phones and as a btrfs image for servers, the
geocoder index as a btrfs image for servers with its models loose for phones,
and a transit build with the
indexes a phone uses to pick feeds. Each kind is released and versioned on its own, and
`latest.json` names the releases clients should use together.

It runs on the machine that holds the data, not in Dagger: the inputs are
hundreds of gigabytes, which Dagger would copy into its engine, and uploading
needs credentials. It lives outside `data_pipeline/` (the Dagger module's
source, which Dagger loads on every call) so its virtualenv doesn't either.

## Bucket layout

```
releases/
  latest.json                       {"map": DATE, "routing": DATE, "transit": DATE, "geocoder": DATE, "poi": SEQ}
  map/DATE/
    planet.pmtiles
    release.json                    size, SHA-256, tile type, zooms, bounds
  routing/DATE/
    tiles/{0,1,2}/NNN/NNN[/NNN].gph named as in the tarball
    index/L0.json                   for each 4° level 0 tile: [{path, bytes, sha256}]
                                    of every level 0 to 2 tile inside it
    routing.btrfs                   the same tiles.tar and valhalla.json in a btrfs image, for servers
    release.json                    Valhalla build ID, source, commits, counts, index cells, image
  geocoder/DATE/
    geocoder.btrfs                  the geocoder's index/, categories/, tagger.model in a btrfs image, for servers
    tagger.model                    the query tagger, for phones
    categorical/LANG.amcats         each language's categorical model (LANG is ISO 639-3), for phones
    release.json                    image and models' sizes and SHA-256s, index schema version, source, commits
  transit/DATE/
    timetables/FEED.timetable
    transfers/MODE.pmtiles          one PMTiles archive per mode: each mode's zoom 14 transfer tiles, as built
    snaps/MODE/FEED_INDEX.bin
    index/X/Y.json                  zoom 6 tiles: feeds with stops there, and their zoom 12 cells
    feeds/FEED.json, feeds.json     feed summaries: names, agencies, license, dates, sizes
    manifest/shared.json            the build manifest's shared fields, feed_count, and feed IDs in order
    manifest/feeds/FEED.json        one feed's manifest entry, with its index
    manifest/manifest.json          the whole manifest, for servers
    realtime.json                   GTFS-RT sources
    release.json                    manifest ID, Valhalla build ID, routing release, counts
  poi/SEQ/                          a POI identity checkpoint, by OSM replication sequence
    delta.bin.zst                   that diff's changes (none for the planet's checkpoint)
    release.json                    checkpoint, checksum, origin, files, and the snapshot it restores from
  poi/snapshots/SEQ/
    versions.bin.zst, pois.bin.zst, status.bin.zst, checkpoints.json
    release.json
```

POI releases are named by sequence rather than date: there's one per daily
OSM diff, and `poi_service/README.md` ("Published tables") describes them.
POI identifiers are stable, so `latest.json`'s `poi` pairs with any map or
geocoder release and isn't checked against them.

`DATE` is `YYYY-MM-DD`; a second release of one kind on the same day is
`YYYY-MM-DD.1`, then `.2`. A release is complete once its `release.json`
exists, and doesn't change after that.

A phone fetches `latest.json`, then the files of those releases. A transit
release is only usable with the routing tiles its walks were computed on, so
its `release.json` records that routing release and Valhalla build ID, and
`latest` refuses to name a transit release whose build ID differs from the
routing release's.

On a phone, transit files go where the manifest names them: `snaps/MODE/N.bin`
is stored at `transfers/snaps/MODE/N.bin`, everything else at the same path. A
phone takes the transfer tiles it needs out of `transfers/MODE.pmtiles` by range
requests, and stores each loose at `transfers/MODE/14/X/Y.bin.zst`, as the build
wrote it, so the loader reads the same layout either way.
The phone writes `transfers/manifest.json` as `manifest/shared.json` without
`feed_ids`, plus `feeds`: its own feeds' entries from `manifest/feeds/`, as
published (each with its `index`), in increasing index order. Transfer tiles
address stops by a feed's index, which the loader takes from these entries.
This needs the loader on the `offline-publish-feed-index` branch; the loader
before it needs every position filled, with `{"id": ID, "stop_count": 0}` for
the feeds the phone doesn't have. See `feed-index` in
`valinor/docs/transit.md`.

### Headers

| Objects | `Content-Type` | `Content-Encoding` | `Cache-Control` |
| --- | --- | --- | --- |
| `*.json` indexes, summaries, manifests | `application/json` | `gzip` | 1 week |
| `release.json` | `application/json` | none | 5 minutes |
| `latest.json` | `application/json` | none | 1 minute |
| `*.pmtiles` | `application/vnd.pmtiles` | none | 1 week |
| `*.gph`, `*.timetable`, `*.bin`, `*.model`, `*.amcats`, `*.btrfs`, `poi/**.zst` | `application/octet-stream` | none | 1 week |
| `poi/snapshots/*/checkpoints.json` | `application/json` | none | 1 week |

JSON that a phone fetches is stored gzipped with `Content-Encoding: gzip`, so
HTTP clients (Dart's included) inflate it transparently: transit indexes and
summaries shrink 3 to 7 times and routing index files about 2.5 times (their
SHA-256s don't compress). `release.json` and `latest.json` are small and read by
people too, so they're plain. Transfer tiles are zstd that the app unpacks
itself, so they don't claim a `Content-Encoding`. Every object sent in a single
`PUT` also carries `x-amz-meta-sha256`, the SHA-256 of its stored bytes; those
sent as multipart uploads (PMTiles archives, btrfs images, POI snapshot files of
256 MiB or more) don't, since they're hashed as they're sent, and their
`release.json` records it instead. A phone reads the geocoder's models' sizes and
SHA-256s from a `HEAD`, so those always go up in a single `PUT`.

## Running

Requires [uv](https://docs.astral.sh/uv/). `uv run` creates a virtualenv in
`publish/.venv` on first use; to keep it out of the tree, set
`UV_PROJECT_ENVIRONMENT` to a directory elsewhere.

Every step but `map-convert` takes a target:

- `--target s3://BUCKET` with `--access-key-id` and `--secret-access-key`.
  The endpoint is Tigris's (`https://t3.storage.dev`) unless `--endpoint` says
  otherwise. Credentials on the command line show in `ps` and in your shell's
  history; in bash, a command starting with a space is left out of history when
  `HISTCONTROL` includes `ignorespace`.
- `--target dir:/PATH` writes the same layout to a local directory, with each
  object's headers in `/PATH/.meta/`. Use it to try a step out.

`--prefix` (default `releases`) is where in the bucket releases go. `--dry-run`
reads and checks everything and lists what's in the bucket, but uploads
nothing. `--jobs` (default 32) is how many uploads run at once.

Steps are rerunnable. An object is skipped when the bucket has it at the same
size and with the same SHA-256, which this machine remembers in
`$XDG_STATE_HOME/cardinal-publish/` (or `--state`) and which the object's own
`sha256` header also says. An interrupted step picks up where it stopped. Once
a release is complete, running its step again only checks that the inputs
still give the same release, and fails if they don't: publish those as the next
release of the day instead. A step also fails before writing `release.json` if
the release's prefix holds objects it didn't make, such as tiles left by an
interrupted run with other inputs.

### Routing

```sh
uv run cardinal-publish routing --target s3://BUCKET --access-key-id ID --secret-access-key KEY \
  --tar release/routing/tiles.tar --image release/routing.btrfs \
  --date 2026-11-11 --source planet-261005 --build-commit COMMIT
```

With `--image routing.btrfs` (the image `data_pipeline`'s `build` writes
beside `routing/`), the release also holds the image, uploaded as one
resumable multipart upload of 128 MiB parts (`--part-size-mib`) once the
tiles are up, and `release.json` records its `image` (file, bytes, SHA-256).
A rerun of a release published with an image needs `--image` too, to check
it.

The step reads `tiles.tar` (a `valhalla_build_extract` tarball, as `data_pipeline`'s
`routing-tiles` writes) once in order, without extracting it: every member is
checked against `index.bin` and its own tile header, hashed, and uploaded. All
tiles must carry one build ID, which goes into `release.json`. Reading and
hashing runs at about 1.3 GB/s, so a planet (107 GB, 209,841 tiles) is bound by
the upload: about 15 minutes at 1 Gbit/s, two and a half hours at 100 Mbit/s.

### Map

```sh
uv run cardinal-publish map-convert --mbtiles release/tiles.mbtiles --out maps/planet.pmtiles \
  --tmpdir /other-disk/pmtiles-tmp --mlt-header
uv run cardinal-publish map --target s3://BUCKET --access-key-id ID --secret-access-key KEY \
  --pmtiles maps/planet.pmtiles --date 2026-11-11 --source planet-261005
```

`map-convert` runs go-pmtiles' `convert`, pinned to 1.31.2: unless
`--pmtiles-bin` names a binary, the release archive is downloaded into
`$XDG_CACHE_HOME/cardinal-publish/` and checked against the SHA-256 pinned in
`mapfile.py`. Its temporary files and its output are each about the size of the
input, so put `--tmpdir` on another disk than `--out` when there isn't room for
both (for a planet, 363 GB each). It refuses to start if they won't fit, and
writes the output under a temporary name, so an existing `--out` is a finished
conversion and a rerun keeps it.

go-pmtiles 1.31.2 doesn't recognize MLT in MBTiles metadata, so it writes the
archive with an unknown tile type and no tile compression, although the tiles
are gzipped MLT. `pmtiles serve` can't serve that: its TileJSON gives tile URLs
without an extension, which it answers 404 to. `--mlt-header` sets the header's
tile type to MLT and compression to the MBTiles metadata's (gzip); without it,
an MLT input is refused before converting, and `map` refuses an archive whose
tile type is unknown.

`map` uploads the archive as one multipart upload of 128 MiB parts
(`--part-size-mib`), up to 8 at once. The file is read in order to hash it;
when a previous run was interrupted, parts the bucket already holds with the
same MD5 aren't sent again.

### Transit

```sh
valinor-transit-build feed-index --build transit/BUILD --output transit/BUILD-feed-index
(cd ../crates && cargo build --release -p offline_map --bin cardinal-pack)
uv run cardinal-publish transit --target s3://BUCKET --access-key-id ID --secret-access-key KEY \
  --build transit/BUILD --staging transit/BUILD-feed-index --work transit/BUILD-archives \
  --date 2026-11-11 --routing-date 2026-11-11 --build-commit COMMIT
```

`feed-index` (in `valinor/valinor-transit-build`) writes the indexes, summaries,
split manifest, and `realtime.json` for a finished build. `transit` publishes
them with every timetable and snap file the manifest names, checking each
against it, and the transfer tiles as one PMTiles archive per mode:
`cardinal-pack transfers` (in `crates/offline_map`; `--pack-bin` names another
build of it) packs each mode's tiles, as the build wrote them, into `--work`
(about their size; a planet's three modes are 1.5 GB and pack in about 15
seconds), checking every tile the manifest lists is there at its size, and each
archive goes up as one resumable multipart upload. `release.json` records each
archive's size, SHA-256, and tile count under `transfer_archives`.
`--routing-date` must be a published routing release with the build ID the
transfers were computed on.

### Geocoder

```sh
uv run cardinal-publish geocoder --target s3://BUCKET --access-key-id ID --secret-access-key KEY \
  --image release/geocoder.btrfs --metadata release/geocoder.json \
  --tagger models/tagger.model --categories models/categories \
  --date 2026-11-14 --source planet-261005 --build-commit COMMIT
```

uploads the geocoder index's btrfs image as one resumable multipart upload,
checked first for btrfs's superblock. `release.json` records the image's size
and SHA-256, and the index's schema version from `geocoder.json` (the index's
`airmail-index.json`, which `build` writes beside the image): a geocoder only
reads indexes of its own schema, so a server can check before it downloads.

It also uploads, loose, the models a phone's offline search downloads:
`--tagger` as `tagger.model`, and every `LANG.amcats` in `--categories` (a
directory of only those) as `categorical/LANG.amcats`. Pass the models `build`
was given as its `--tagger` and `--categories`, the ones inside the image. A
phone reads each model's size and SHA-256 from a `HEAD`, so each goes up as one
`PUT` with `x-amz-meta-sha256`: models are tens to a hundred MB, read whole on
each worker (8 at once), and an interrupted run sends only those that didn't
make it. `release.json` lists them under `tagger` and `categorical` (by
language), with their sizes and SHA-256s.

A server downloads the image and mounts it read-only
(`mount -o loop,ro geocoder.btrfs DIR`), then serves `DIR/index` with the
models beside it, `DIR/categories/` and `DIR/tagger.model`.

Either part can be published alone: `--image` and `--metadata` for servers, or
`--tagger` (with `--categories`, or without for a release a phone uses with no
categorical model). A rerun of a complete release needs the parts it was
published with, to check them. Phones download the models of the release
`latest.json`'s `geocoder` names (`latest --geocoder DATE`), which must have a
tagger; servers are pointed at a release by hand and don't read `latest.json`.

### POI identities

```sh
poi_service export --staging poi-staging
uv run cardinal-publish poi --target s3://BUCKET --access-key-id ID --secret-access-key KEY \
  --staging poi-staging --latest
```

uploads every complete release `poi_service export` staged that the bucket
doesn't have: snapshots first, then checkpoints in order, each one's files
before its `release.json`. Each checkpoint must follow one that's staged or
published, and restore from a snapshot that is. A release already published
is only compared with the staged one, and a difference fails the step: the
staging directory belongs to another database, or history changed, which
never should. Snapshot files of 256 MiB or more go up as resumable multipart
uploads. `--latest` then points `latest.json`'s `poi` at the newest
checkpoint.

```sh
uv run cardinal-publish poi-prune --target s3://BUCKET --access-key-id ID --secret-access-key KEY \
  --keep-days 28 --dry-run
```

deletes checkpoint releases more than `--keep-days` (28) older than the
newest, except those between a kept checkpoint and the snapshot it restores
from, then every snapshot that none of the remaining checkpoints restores
from. With weekly snapshots, that keeps up to five weeks of deltas (2 to 6 MB
each) and about five snapshots (1.9 GB each for the planet). It never deletes the newest checkpoint, the one `latest.json` names, a
snapshot newer than every checkpoint (one whose checkpoint is still being
uploaded), or a prefix without a `release.json`. Each release's
`release.json` goes first, so an interrupted prune leaves incomplete releases
rather than broken ones. Uploads never delete anything. `--dry-run` lists
what it would delete; `--staging DIR` prunes a local staging directory the
same way instead of the bucket.

### Latest

```sh
uv run cardinal-publish latest --target s3://BUCKET --access-key-id ID --secret-access-key KEY \
  --map 2026-11-11 --routing 2026-11-11 --transit 2026-11-11 --geocoder 2026-11-14
```

points `latest.json` at those releases; a kind left out keeps its current one.
A geocoder release must have phone models (a `tagger` in its `release.json`).
`--poi SEQ` names a POI identity checkpoint.

## Testing

```sh
uv run pytest
```

runs each step against a temporary directory and an in-process S3 (moto).
