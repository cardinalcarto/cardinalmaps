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

writes a release: `out/tiles.mbtiles`, `out/index/`, `out/routing/`, and the models it's given
as `out/categories/` and `out/tagger.model`. The steps run one after another,
since each can use most of a planet build machine's memory. Omit the first
line if you're using docker instead of podman.

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
`valinor/tools/oracle/README.md`), and no elevation, since Valhalla reads only
its own `.hgt` tiles and the pipeline has none. The tiles are built in the
`routing-tiles-work` cache volume, which is emptied afterwards: a planet's are
many tens of gigabytes before they're packed into the tarball, which is about
as large again.

- `--speeds` sets `mjolnir.default_speeds_config`: default speeds by country or
  subdivision, density and road class, for edges without a `maxspeed`. Without
  it, mainline's built-in speeds by road class apply. The project doesn't ship
  one. [OpenStreetMapSpeeds](https://github.com/OpenStreetMapSpeeds/schema)'
  `default_speeds.json` is in this format; its repository is MIT licensed,
  but the speeds were measured from Mapillary GPS traces matched to
  OpenStreetMap, under Mapillary's terms, so check that its terms suit your
  deployment before you use it.

Routing tiles read the extract without POI identifiers, so `build` can reuse
them from the engine's cache when only the POI database has changed.

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
