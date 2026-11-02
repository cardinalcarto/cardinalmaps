# Airmail build docs

The [README](README.md) has a quickstart. This page covers the pieces in more detail.

## Dependencies

- Rust 1.94 or newer.
- clang, pkg-config and zstd's development headers.
- SpatiaLite (`libsqlite3-mod-spatialite` on Debian/Ubuntu, `libspatialite` on Fedora). The indexer loads it as a SQLite extension to read the Who's On First database.

## Data

Put data files in `./data`.

- An OSM extract in PBF format, from [Geofabrik](https://download.geofabrik.de) or [BBBike](https://download.bbbike.org/osm/planet/). Try an extract before building the planet.
- A Who's On First SpatiaLite database covering the same area. Use the planet download if your extract covers more than one country.

## Indexing

```bash
cargo run --release --bin indexer -- \
  --wof-db ./data/whosonfirst-data-admin-latest.spatial.db \
  --index ./data/index \
  --admin-cache ./data/admin-cache \
  load-osm-pbf ./data/australia-oceania-latest.osm.pbf
cargo run --release --bin indexer -- \
  --wof-db ./data/whosonfirst-data-admin-latest.spatial.db \
  --index ./data/index \
  load-wof
```

`load-osm-pbf` indexes the extract's named features and addresses, with their
shapes: nodes, ways, and multipolygon relations. OSM's administrative places
(`place=city`, `boundary=*` and so on) are skipped; `load-wof` indexes every
administrative area in the Who's On First database instead, so use one
covering the area you want to search.

- `--admin-cache` is a redb file caching point-in-polygon lookups and OSM node locations. It's optional, but keeping it between runs speeds imports up a lot.
- `--pip-tree` builds an in-memory R-tree of Who's On First polygons and caches it at the given path. It's faster than SpatiaLite lookups for planet-scale or repeated imports, but uses around 10GB of memory and takes a few minutes to build the first time.
- `load-osm-pbf --nodes-already-cached` skips the node-location pass when the admin cache already has it from a previous run of the same file.
- `load-osm-pbf --ignore <ways|nodes|dense-nodes|relations>` skips element types.
- `--qrank qrank.csv.gz` ([QRank](https://qrank.wmcloud.org)) ranks POIs with Wikidata entities by their Wikipedia articles' page views. Without it, having an entity counts the same for every POI.

An index records its schema version in `airmail-index.json`; the service and
indexer refuse indexes of other versions, so rebuild after upgrading across a
version change.

## Running the service

```bash
cargo run --release --bin airmail_service -- --index ./data/index
```

Then query `http://127.0.0.1:3000/v1/search?q=...`. [airmail_service/openapi.yaml](airmail_service/openapi.yaml) describes the API, which the service also serves at `/openapi.yaml`; `/health` reports whether it's ready. The bind address and CORS origins are set with `--bind`/`AIRMAIL_BIND` and `--cors`/`AIRMAIL_CORS`.

For categorical search ("vegan food", "Apotheke"), pass `--categories` a directory of per-language models (see [categorical/README.md](categorical/README.md)) and add `lang` to requests, e.g. `lang=de-CH`. Without `lang`, or for a language with no model, search matches words only.

## Other tools

- `cargo run --release --bin query -- --index ./data/index [--bbox lng1,lat1,lng2,lat2] [--categories models/ --lang de]` opens an interactive query prompt. With only `--categories`, it shows what the model understands without searching.
- `cargo run --release --bin merge -- --index ./data/index --merge` merges all index segments into one.
- `python3 -I eval/search_eval.py http://127.0.0.1:3000 [--region wa]` scores a running service against the ranking cases in `eval/search.tsv`. It understands the legacy `/search` API too, so a change can be compared against the version before it.

## Containers

`Dockerfile` builds the service and `airmail_indexer/Dockerfile` builds the indexer. `docker-compose.yaml` wires both to `./data`; see the README for the commands.
