# Data pipeline

Builds Cardinal's data from an OSM extract:

- an MLT vector tileset from the `planetiler-openmaptiles` fork,
- the airmail geocoder index,
- airmail's per-language categorical search models (see
  `airmail/categorical/README.md`).

POIs in the tiles and the index are first tagged with stable identifiers from a
`poi_service` database (see `poi_service/README.md`).

```
export DAGGER_ENGINE=image+podman://registry.dagger.io/engine:v1.0.0-beta.21
dagger -m data_pipeline api call build --osm=./region.osm.pbf \
  --wof-db=./whosonfirst-data-admin-latest.spatial.db \
  --database-url=env://DATABASE_URL -o out
```

writes `out/tiles.mbtiles`, `out/index/` and `out/categories/`. The steps run
one after another, since each can use most of a planet build machine's memory.
Omit the first line if you're using docker instead of podman.

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

Each output can also be built alone with `tiles`, `index` or
`categorical-models`, which take the same arguments where they apply. The
categorical models are trained from the unpatched extract, so they aren't
retrained when only the database changes.

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
