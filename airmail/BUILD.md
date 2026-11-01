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
```

- `--admin-cache` is a redb file caching point-in-polygon lookups and OSM node locations. It's optional, but keeping it between runs speeds imports up a lot.
- `--pip-tree` builds an in-memory R-tree of Who's On First polygons and caches it at the given path. It's faster than SpatiaLite lookups for planet-scale or repeated imports, but uses around 10GB of memory and takes a few minutes to build the first time.
- `load-osm-pbf --nodes-already-cached` skips the node-location pass when the admin cache already has it from a previous run of the same file.
- `load-osm-pbf --ignore <ways|nodes|dense-nodes>` skips element types.

## Running the service

```bash
cargo run --release --bin airmail_service -- --index ./data/index
```

Then query `http://127.0.0.1:3000/search?q=...`. The bind address and CORS origins are set with `--bind`/`AIRMAIL_BIND` and `--cors`/`AIRMAIL_CORS`.

For categorical search ("vegan food", "Apotheke"), pass `--categories` a directory of per-language models (see [categorical/README.md](categorical/README.md)) and add `lang` to requests, e.g. `lang=de-CH`. Without `lang`, or for a language with no model, search matches words only.

## Other tools

- `cargo run --release --bin query -- --index ./data/index [--bbox lng1,lat1,lng2,lat2] [--categories models/ --lang de]` opens an interactive query prompt. With only `--categories`, it shows what the model understands without searching.
- `cargo run --release --bin merge -- --index ./data/index --merge` merges all index segments into one.

## Containers

`Dockerfile` builds the service and `airmail_indexer/Dockerfile` builds the indexer. `docker-compose.yaml` wires both to `./data`; see the README for the commands.
