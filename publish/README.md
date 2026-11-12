# Publishing releases

`cardinal-publish` uploads a release of Cardinal's data to an S3-compatible
bucket that clients read over plain HTTPS: the map as one PMTiles archive, the
Valhalla routing tiles loose, and a transit build with the indexes a phone uses
to pick feeds. Each kind is released and versioned on its own, and
`latest.json` names the releases clients should use together.

It runs on the machine that holds the data, not in Dagger: the inputs are
hundreds of gigabytes, which Dagger would copy into its engine, and uploading
needs credentials. It lives outside `data_pipeline/` (the Dagger module's
source, which Dagger loads on every call) so its virtualenv doesn't either.

## Bucket layout

```
releases/
  latest.json                       {"map": DATE, "routing": DATE, "transit": DATE}
  map/DATE/
    planet.pmtiles
    release.json                    size, SHA-256, tile type, zooms, bounds
  routing/DATE/
    tiles/{0,1,2}/NNN/NNN[/NNN].gph named as in the tarball
    index/L0.json                   for each 4° level 0 tile: [{path, bytes, sha256}]
                                    of every level 0 to 2 tile inside it
    release.json                    Valhalla build ID, source, commits, counts, index cells
  transit/DATE/
    timetables/FEED.timetable
    transfers/MODE/12/X/Y.bin.zst
    snaps/MODE/FEED_INDEX.bin
    index/X/Y.json                  zoom 6 tiles: feeds with stops there, and their zoom 12 cells
    feeds/FEED.json, feeds.json     feed summaries: names, agencies, license, dates, sizes
    manifest/shared.json            the build manifest's shared fields and feed IDs in order
    manifest/feeds/FEED.json        one feed's manifest entry, with its index
    manifest/manifest.json          the whole manifest, for servers
    realtime.json                   GTFS-RT sources
    release.json                    manifest ID, Valhalla build ID, routing release, counts
```

`DATE` is `YYYY-MM-DD`; a second release of one kind on the same day is
`YYYY-MM-DD.1`, then `.2`. A release is complete once its `release.json`
exists, and doesn't change after that.

A phone fetches `latest.json`, then the files of those releases. A transit
release is only usable with the routing tiles its walks were computed on, so
its `release.json` records that routing release and Valhalla build ID, and
`latest` refuses to name a transit release whose build ID differs from the
routing release's.

On a phone, transit files go where the manifest names them: `snaps/MODE/N.bin`
is stored at `transfers/snaps/MODE/N.bin`, everything else at the same path.
The phone writes `transfers/manifest.json` from `manifest/shared.json` (without
`feed_ids`) with a `feeds` list holding, at each position of `feed_ids`, that
feed's entry from `manifest/feeds/` (without `index`) if it has the feed, or
`{"id": ID, "stop_count": 0}` if it doesn't. Transfer tiles address stops by a
feed's position, so positions must be kept. See `feed-index` in
`valinor/docs/transit.md`.

### Headers

| Objects | `Content-Type` | `Content-Encoding` | `Cache-Control` |
| --- | --- | --- | --- |
| `*.json` indexes, summaries, manifests | `application/json` | `gzip` | 1 week |
| `release.json` | `application/json` | none | 5 minutes |
| `latest.json` | `application/json` | none | 1 minute |
| `*.pmtiles` | `application/vnd.pmtiles` | none | 1 week |
| `*.bin.zst` | `application/zstd` | none | 1 week |
| `*.gph`, `*.timetable`, `*.bin` | `application/octet-stream` | none | 1 week |

JSON that a phone fetches is stored gzipped with `Content-Encoding: gzip`, so
HTTP clients (Dart's included) inflate it transparently: transit indexes and
summaries shrink 3 to 7 times and routing index files about 2.5 times (their
SHA-256s don't compress). `release.json` and `latest.json` are small and read by
people too, so they're plain. Transfer tiles are zstd that the app unpacks
itself, so they don't claim a `Content-Encoding`. Every object also carries
`x-amz-meta-sha256`, the SHA-256 of its stored bytes.

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
  --tar routing/tiles.tar --date 2026-11-11 --source planet-261005 --build-commit COMMIT
```

reads `tiles.tar` (a `valhalla_build_extract` tarball, as `data_pipeline`'s
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
uv run cardinal-publish transit --target s3://BUCKET --access-key-id ID --secret-access-key KEY \
  --build transit/BUILD --staging transit/BUILD-feed-index \
  --date 2026-11-11 --routing-date 2026-11-11 --build-commit COMMIT
```

`feed-index` (in `valinor/valinor-transit-build`) writes the indexes, summaries,
split manifest, and `realtime.json` for a finished build. `transit` publishes
them with every timetable, snap file, and transfer tile the manifest names,
checking each against it. `--routing-date` must be a published routing release
with the build ID the transfers were computed on.

### Latest

```sh
uv run cardinal-publish latest --target s3://BUCKET --access-key-id ID --secret-access-key KEY \
  --map 2026-11-11 --routing 2026-11-11 --transit 2026-11-11
```

points `latest.json` at those releases; a kind left out keeps its current one.

## Testing

```sh
uv run pytest
```

runs each step against a temporary directory and an in-process S3 (moto).
