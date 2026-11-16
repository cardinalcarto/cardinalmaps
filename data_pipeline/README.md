# Data pipeline

Builds Cardinal's data from an OSM extract:

- an MLT vector tileset from the `planetiler-openmaptiles` fork,
- the airmail geocoder index,
- Valhalla routing tiles, built by mainline Valhalla's own tile builder, for
  Valinor (see `valinor/docs/routing.md`),
- and, run by hand now and then, airmail's per-language categorical search
  models (see `airmail/categorical/README.md`), which a release carries
  along with the query tagger.

POIs in the tiles and the index are first tagged with stable identifiers from a
`poi_service` database (see `poi_service/README.md`).

```
export DAGGER_ENGINE=image+podman://registry.dagger.io/engine:v1.0.0-beta.21
dagger -m data_pipeline api call build --osm=./region.osm.pbf \
  --wof-db=./whosonfirst-data-admin-latest.spatial.db \
  --categories=./categories --tagger=./tagger.model \
  --database-url=env://DATABASE_URL -o out
```

writes a release:

- `out/tiles.mbtiles`;
- `out/geocoder.btrfs`, a btrfs image of everything the geocoder serves
  (see "Images" below): the index as `index/`, and the models it's given as
  `categories/` and `tagger.model`, with `out/geocoder.json`, the index's
  `airmail-index.json`, beside it;
- `out/routing/`, and `out/routing.btrfs`, the same `tiles.tar` and
  `valhalla.json` as an image;

The steps run one after another, since each can use most of a planet build
machine's memory. Omit the first line if you're using docker instead of
podman. `PLANET.md` walks through a planet build from scratch.

- `--categories` and `--tagger` are the categorical search models and the
  query tagger, usually the last release's. How people describe POIs changes
  slowly, so they're retrained by hand: the models with `categorical-models`,
  the tagger with airmail's `tagger train` (see `airmail/BUILD.md`).

- `--database-url` gives POIs their identifiers; without it, POIs have none.
  For a database on this machine, add `--database=tcp://localhost:5432` and
  use host `database` in the URL. A patched extract is reused until the
  database's latest checkpoint changes.
- `--wof-db` is a Who's On First SpatiaLite database covering the extract.
  Every administrative area in it is indexed for search, so for a regional
  deployment use one covering the region, and the planet's for a global one.
  Tiles get the areas POIs are in from it too; see "Differences from
  OpenMapTiles" in `planetiler-openmaptiles/README.md`.
  Point-in-polygon caches built from it are kept in an `airmail-wof-<digest>`
  cache volume, which the engine's garbage collection can reclaim once the
  database is replaced.
- `--qrank=./qrank.csv.gz` ranks POIs with Wikidata entities by their
  Wikipedia articles' page views, from [QRank](https://qrank.wmcloud.org).
  Without it, having an entity counts the same for every POI.
- `--pip-tree=false` uses SpatiaLite lookups instead of an in-memory polygon
  tree, which is slower for the planet but avoids the tree's ~10GB of memory.

- `--routing-speeds` is a default speeds file for the routing tiles; see
  below.

Each output can also be built alone with `tiles`, `index`, `routing-tiles` or
`categorical-models`, which take the same arguments where they apply.

The index and the models read the extract with node locations on its ways,
which `locations-on-ways` writes with `osmium add-locations-to-ways`. For an
extract over 20GB, osmium's index of node locations goes on disk, in the
`osmium-node-index` cache volume, rather than in memory. The tiles read the
extract itself, since planetiler needs every node.

## Routing tiles

`routing-tiles` builds `routing/`: `tiles.tar`, a Valhalla tile extract with
its index (`valhalla_build_extract`), and `valhalla.json`, the configuration to
serve it with, whose `mjolnir.tile_extract` is `tiles.tar` in the same
directory.

```
dagger -m data_pipeline api call routing-tiles --osm=./region.osm.pbf \
  --speeds=./default_speeds.json -o routing
```

It runs mainline Valhalla's tools in the image pinned by
`valinor/tools/oracle/valhalla-image` (3.9.1), the same way Valinor's oracle
builds its tiles: with pedestrian areas, bounding circles, administrative areas
(`routing-admins`) and time zones (`routing-timezones`, built once from the
timezone-boundary-builder release the image names). There are no landmarks,
since mainline's landmarks step makes tiles that route incorrectly (see
`valinor/tools/oracle/README.md`), and elevation only with `--gedtm` (below).
The tiles are built in the `routing-tiles-work` cache volume, which is emptied
afterwards: a planet's are many tens of gigabytes before they're packed into
the tarball, which is about as large again.

- `--speeds` sets `mjolnir.default_speeds_config`: default speeds by country or
  subdivision, density and road class, for edges without a `maxspeed`. Without
  it, mainline's built-in speeds by road class apply. The project doesn't ship
  one. [OpenStreetMapSpeeds](https://github.com/OpenStreetMapSpeeds/schema)'
  `default_speeds.json` is in this format; its repository is MIT licensed,
  but the speeds were measured from Mapillary GPS traces matched to
  OpenStreetMap, under Mapillary's terms, so check that its terms suit your
  deployment before you use it.
- `--gedtm` gives edges elevation (`additional_data.elevation`): heights along
  them and their grades, which bicycle costing weighs (`use_hills`) and routes
  and `trace_attributes` report (`elevation_interval`, `edge.mean_elevation`,
  grades). It's where GDAL, in the build's container, reads
  [GEDTM](https://zenodo.org/records/14900180) v1.2, the bare earth model the
  topographic tiles use, from a server that answers range requests.
  `--gedtm-service` is that server, if it's on this machine. Without
  `--gedtm`, edges have no elevation. `build` takes both too.

### Elevation

Mainline Valhalla reads elevation only from its own `.hgt` tiles, 1x1 degree
at 1 arc second. With `--gedtm`, the build stops before the elevation stage,
cuts the cells under the graph tiles and in the extract's bounding box (from
its header) out of GEDTM (`routing_elevation.py`), and finishes. The tiles are
cut with the alignment and units of `topo_pipeline/scripts/tile_gedtm.sh`,
which refuses any other layout than v1.2's, and are gzipped. Cells with no
data, like open ocean, are skipped, as are cells outside GEDTM's 65S to 85N,
whose edges get no elevation. They're cut into the `routing-tiles-work` cache
volume beside the graph tiles and deleted with them. Only the build needs
them: the tile builder samples them into each edge, so the release doesn't
carry them, and `height` requests, which read `.hgt` tiles when they're
served, aren't answered.

GEDTM is one file of 432GB, and isn't passed in as a file: a file passed to
Dagger is copied into the engine's store, and stays there until garbage
collection reclaims it. Instead GDAL reads the blocks it needs over HTTP
(`/vsicurl/`), from a server for the file on this machine, which the build
reaches as host `gedtm`. Any file server that answers range requests will do,
such as Caddy's, in rootless podman:

```
podman run -d --rm --name gedtm-http -p 127.0.0.1:8080:8080 \
  -v /path/to/gedtm:/srv:ro,z docker.io/library/caddy:2.11.7-alpine \
  caddy file-server --root /srv --listen :8080
dagger -m data_pipeline api call routing-tiles --osm=./region.osm.pbf \
  --gedtm=/vsicurl/http://gedtm:8080/gedtm_rf_m_30m_s_20060101_20151231_go_epsg.4326.3855_v1.2.tif \
  --gedtm-service=tcp://localhost:8080 -o routing
podman stop gedtm-http
```

The cut tiles are 6 to 16MB each. Washington's 47 (of 52 cells in its
bounding box) are 0.55GB, cut in 10 seconds, beside 2.2GB of graph tiles. GEDTM
has data in about 18,600 cells, so a planet's would be around 150 to 220GB,
cut in about an hour, in the cache volume for the length of the build.

The engine's cache keys a run on the URL, not on what's behind it, so after
replacing the file behind the same URL, build with a different one, such as
another port.

GEDTM is under CC BY 4.0, so a deployment that shows routes' elevation needs
to credit it; see "Data credit" in `topo_pipeline/README.md`.

Routing tiles read the extract without POI identifiers, so `build` can reuse
them from the engine's cache when only the POI database has changed.

## Images

Servers get the geocoder index and the routing tiles as read-only btrfs
filesystem images, whose files are compressed with zstd. Each is one object
to store in the bucket and to download onto a machine, smaller than its
files, and mounted as it is (`mount -o loop,ro`). The kernel decompresses
pages as they're read, so the files can still be memory-mapped.

`btrfs-image --dir=DIR` makes one from any directory, with `mkfs.btrfs
--rootdir --compress zstd:LEVEL --shrink` (btrfs-progs 6.14, pinned), which
needs no privileges, and checks it with `btrfs check`. `build` makes two:

- `routing.btrfs` holds `tiles.tar` and `valhalla.json` at its root, so
  Valinor maps the tarball as a whole. Loose tiles would take one mapping
  each, and the planet's 209,841 are more than Linux's default
  `vm.max_map_count`. `routing/` is exported too, since the phone's loose
  tiles are published from `tiles.tar`, and transit's walks are computed on
  it.
- `geocoder.btrfs` holds the index as `index/`, with the categorical models
  as `categories/` and the query tagger as `tagger.model`, so a server needs
  only the image. None of them is exported loose.

`--image-compression-level` (1 to 15, default 3) sets the zstd level.
Images aren't byte-reproducible, since btrfs records when one was made.
`btrfs restore IMAGE DIR` copies an image's files out without mounting it.

## Cardinal Topo

`topo-tiles` builds the topographic tileset: everything in `tiles`, plus contour lines in
both metric and imperial (see `topo_pipeline/README.md`).

```
dagger -m data_pipeline api call topo-tiles --osm=./region.osm.pbf \
  --dem=./dem --config=./topo.toml --bbox=-125,45,-120,50 -o topo.mbtiles
```

`build` takes `--contour-config` to include the contours in its tiles, and `--s3-access-key-id`
and `--s3-secret-access-key` if its DEMs are in a bucket. Or it takes `--contours`, a file made
earlier by `contours`, so a planet's DEMs needn't stay in the engine for the whole build.

The steps are also available alone as `contours` and `append-contours`. Finished contour
chunks stay in the `topo-chunks` cache volume, which the engine's garbage collection can
reclaim; a rerun recomputes what's missing or changed.
