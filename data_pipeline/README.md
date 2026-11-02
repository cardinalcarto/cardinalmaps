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
