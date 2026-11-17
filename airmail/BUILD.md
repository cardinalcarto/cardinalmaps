# Airmail build docs

The [README](README.md) has a quickstart. This page covers the pieces in more detail.

## Dependencies

- Rust 1.94 or newer.
- clang, pkg-config and zstd's development headers.
- SpatiaLite (`libsqlite3-mod-spatialite` on Debian/Ubuntu, `libspatialite` on Fedora). The indexer loads it as a SQLite extension to read the Who's On First database.

## Data

Put data files in `./data`.

- An OSM extract in PBF format, from [Geofabrik](https://download.geofabrik.de) or [BBBike](https://download.bbbike.org/osm/planet/). Try an extract before building the planet. Its ways need their nodes' locations, which [osmium](https://osmcode.org/osmium-tool/) adds: `osmium add-locations-to-ways --keep-member-nodes -o extract-low.osm.pbf extract.osm.pbf`. For the planet, keep the index on disk with `-i dense_file_array,nodes.idx`, since in memory it needs about 110GB. The indexer refuses a file without them.
- A Who's On First SpatiaLite database covering the same area. Use the planet download if your extract covers more than one country.

## Indexing

```bash
cargo run --release --bin indexer -- \
  --wof-db ./data/whosonfirst-data-admin-latest.spatial.db \
  --index ./data/index \
  --admin-cache ./data/admin-cache \
  load-osm-pbf ./data/australia-oceania-latest-low.osm.pbf
cargo run --release --bin indexer -- \
  --wof-db ./data/whosonfirst-data-admin-latest.spatial.db \
  --index ./data/index \
  load-wof
```

`load-osm-pbf` indexes the extract's named features and addresses, with their
shapes: nodes, ways, and multipolygon relations. Named roads and paths become
streets, their ways merged where they're near each other, and addresses' postcodes
become postcodes, shaped by where their addresses are. Houses in
`associatedStreet` relations get the relation's street. OSM's administrative
places (`place=city`, `boundary=*` and so on) are skipped; `load-wof` indexes
every administrative area in the Who's On First database instead, so use one
covering the area you want to search.

- `--admin-cache` is a redb file holding the coordinates of the ways multipolygons are made of while parsing. It's optional, and in a temporary directory without it.
- `--pip-tree` builds an in-memory R-tree of Who's On First polygons and caches it at the given path. It's faster than SpatiaLite lookups for planet-scale or repeated imports, but uses around 10GB of memory and takes a few minutes to build the first time.
- `load-osm-pbf --ignore <ways|nodes|dense-nodes|relations>` skips element types.
- `--qrank qrank.csv.gz` ([QRank](https://qrank.wmcloud.org)) ranks POIs with Wikidata entities by their Wikipedia articles' page views. Without it, having an entity counts the same for every POI.

An index records its schema version in `airmail-index.json`; the service and
indexer refuse indexes of other versions, so rebuild after upgrading across a
version change.

## Running the service

```bash
cargo run --release --bin airmail_service -- --index ./data/index
```

Then query `http://127.0.0.1:3000/v1/search?q=...`, adding `focus=lon,lat` to rank places near a point higher, and look results up again by ID with `/v1/place?id=...`. [airmail_service/openapi.yaml](airmail_service/openapi.yaml) describes the API, which the service also serves at `/openapi.yaml`; `/health` reports whether it's ready. The bind address and CORS origins are set with `--bind`/`AIRMAIL_BIND` and `--cors`/`AIRMAIL_CORS`.

For categorical search ("vegan food", "Apotheke"), pass `--categories` a directory of per-language models (see [categorical/README.md](categorical/README.md)) and add `lang` to requests, e.g. `lang=de-CH`. Without `lang`, or for a language with no model, search matches words only.

With `--tagger`, a query tagger model (see below) reads which words of a query are what: what's sought, a road, a house number, a postcode, or areas by band (neighbourhood, locality, county, region, country). Search then also looks for each likely interpretation that puts what's sought in areas, and the category model reads only the words of what's sought. Without it, search finds area names at the end of queries itself.

## The query tagger

`airmail_tagger` is a linear-chain CRF after libpostal's address parser, with features from the index's gazetteer of areas (by band, matched exactly, as the beginning of a word being typed, or with typos, how well known and how far from the focus). It's trained on queries made from an index's places:

```bash
cargo run --release --bin tagger -- train --index ./data/index-wa --index ./data/index-ch --out tagger.model
cargo run --release --bin tagger -- eval --index ./data/index-wa --model tagger.model eval/tagger.tsv
cargo run --release --bin tagger -- tag --index ./data/index-wa --model tagger.model "vegan pizza near capitol hill"
```

Training holds a tenth of the places out and reports accuracy on (up to 100,000 of) their queries by tag; `eval` scores a model on the hand-labelled queries in `eval/tagger.tsv`. A model works with any index of the same schema, whose gazetteer it uses.

Training uses every `--stride`th document (20 by default; 1 for all), with areas sampled apart in proportion to their population. Examples are featurized on every core and streamed to temporary files beside `--out`, read back a shuffled bucket at a time, so memory goes to the sampled documents and the feature table rather than the examples: on a 250-million-document planet index, stride 20 makes about 14 million examples and takes about an hour on 32 cores. More data helps more than more `--epochs`: past about five, the model learns the generator's habits rather than queries'.

A model stores its features in an FST, with 16-bit weights, so it can be read without unpacking; `tagger convert <model> <out>` rewrites an older model in the current format, and with `--min-weight` drops its smaller weights. Most of a model's size buys little accuracy: a 105 MB planet model pruned at 2.0 is 71 MB (42 MB compressed) and loses about 0.1 points of held-out accuracy. Pruning changes how confident a model is, so run `tagger calibrate` on the result with the arguments it was trained with.

## Other tools

- `cargo run --release --bin query -- --index ./data/index [--bbox lng1,lat1,lng2,lat2] [--categories models/ --lang de]` opens an interactive query prompt. With only `--categories`, it shows what the model understands without searching.
- `cargo run --release --bin merge -- --index ./data/index --merge` merges all index segments into one.
- `python3 -I eval/search_eval.py http://127.0.0.1:3000 [--region wa] [--jobs N]` scores a running service against the ranking cases in `eval/search.tsv`. It understands the legacy `/search` API too, so a change can be compared against the version before it.
- `python3 -I eval/pelias_acceptance.py http://127.0.0.1:3000 path/to/acceptance-tests [--failures] [--jobs N]` runs [Pelias's acceptance tests](https://github.com/pelias/acceptance-tests) (clone them first) against `/v1/search`, translating their inputs and expectations; its docstring lists the approximations. They're worldwide, so they mean most against a planet index.

- `cargo run --release --bin spatial_bench -- sample|run|compare ...` searches sampled queries in boxes of several sizes and compares two indexes' results and latencies (see its doc comment).

## On phones

The `airmail` crate (search, `DocumentBuilder`, the writer, categorical inference) and `airmail_tagger` build for Android and iOS with `airmail`'s default features off, which drops tokio and tantivy's columnar zstd (a C dependency no field uses). Without `tokio`, use `lookup_blocking` and `num_docs_blocking`; `AirmailIndex::writer_with` takes the indexing memory and threads, and `Storage::Pointers`, with which a document with a stable POI ID stores only a pointer to the tile feature it came from (`SearchHit::pointer`). Documents are made from tile features' tags with `airmail::osm::document` and `street_document`, from Who's On First records with `airmail::wof::admin_document`, and postcodes with `airmail::postcodes::Postcodes`, as the indexer does.

```bash
cargo check -p airmail -p airmail_tagger --no-default-features --target aarch64-linux-android
cargo check -p airmail -p airmail_tagger --no-default-features --target aarch64-apple-ios
```

## Containers

`Dockerfile` builds the service and `airmail_indexer/Dockerfile` builds the indexer. `docker-compose.yaml` wires both to `./data`; see the README for the commands.
