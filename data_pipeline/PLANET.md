# Building Cardinal's data from scratch

*Check date: November 14, 2026*

This guide reproduces every data artifact Cardinal serves, starting from a
clone of this repository and an empty disk. Follow it in order. Each section
says what it produces, how long it took on the planet, and how to check the
result before moving on.

## What you'll build

| Artifact | Built by | Used by |
|---|---|---|
| POI identity database (PostgreSQL) | `poi_service bootstrap` and `update` | the build, to give POIs stable IDs |
| Categorical search models (`categories/*.amcats`) | `categorical-models` (Dagger) | the geocoder |
| Query tagger (`tagger.model`) | airmail `tagger train` | the geocoder |
| Contour lines (`contours.osm.pbf`) | `contours` (Dagger) | the map tiles |
| Map tiles (`tiles.mbtiles`, then `planet.pmtiles`) | `build` (Dagger), then `cardinal-publish map-convert` | the tileserver, the app |
| Geocoder image: index, categorical models and tagger (`geocoder.btrfs`, with `geocoder.json`) | `build` (Dagger) | the geocoder |
| Routing tiles (`routing/tiles.tar`, `valhalla.json`) | `build` (Dagger) | the app offline (published loose), transit |
| Routing tiles as a btrfs image (`routing.btrfs`) | `build` (Dagger) | Valinor on servers |
| Transit timetables and transfers | `valinor-transit-build` | Valinor, the app offline |
| Published releases in a bucket | `cardinal-publish` | the app |

The order matters:

1. The POI database comes before the build.
2. The search models come before the build, the first time only.
3. Contours come before the map tiles.
4. Routing tiles come before transit.
5. Routing is published before transit.

Cardinal's published map includes contour lines (the Cardinal Topo layers),
so to reproduce it, don't skip the contour step.

## 1. Requirements

### Machine

The reference build machine is a Framework Desktop with 128 GB of RAM and 32 cores. It is configured with 64 GB of zram. Less RAM may
work with the memory settings below scaled down, but that's left as an exercise for the reader.

| Peak memory | Step |
|---|---|
| about 36 GiB of heap, inside 47 GiB | map tiles (Planetiler) |
| about 65 GB | transit transfers |
| about 10 GB extra | geocoder polygon tree (`--pip-tree=false` avoids it, but is slower) |

### Disk

Put everything on one fast 4 TB disk formatted with a copy-on-write filesystem. Dagger keeps redundant copies of everything inside the engine.

| What | Size |
|---|---|
| Planet `.osm.pbf` | 95 GB |
| Who's On First database | 34 GB |
| GEDTM | 432 GB |
| GEDTM cut into 1° tiles, for contours | 279 GB |
| `contours.osm.pbf` | 149 GB |
| `tiles.mbtiles` | 363 GB, plus 775 GB of Planetiler sort data while building |
| Geocoder index | 146 GB uncompressed, inside the engine only. Its image, `geocoder.btrfs`, is smaller. |
| `routing/tiles.tar` | 107 GB, plus about as much in graph tiles, and 150–220 GB of elevation tiles, while building |
| `routing.btrfs` | the same tiles again, compressed |
| Transit build | about 25 GB, plus the fetch cache |
| `planet.pmtiles` | 324 GB, plus temporary files the size of the MBTiles |

You can delete intermediate files as you go. Each section says what's safe
to delete.

### Software

| Tool | Version | For |
|---|---|---|
| podman (or docker) | any recent | Dagger's engine, transit feed fetching, PostgreSQL |
| [Dagger CLI](https://docs.dagger.io/install) | currently v1.0.0-beta.21, as in `data_pipeline/dagger-module.toml` | the main build |
| Rust, via [rustup](https://rustup.rs) | stable | `poi_service`, the tagger, `valinor-transit-build`, `cardinal-pack` |
| [uv](https://docs.astral.sh/uv/) | any recent | `cardinal-publish` |
| GDAL (`gdalinfo`, `gdal_translate`) | 3.8 or newer | cutting GEDTM for contours |
| curl, bunzip2, md5sum, git, sqlite3 | any | downloads and checks |
| btrfs-progs | 6.x | only to inspect the images the build makes (Dagger packs them itself) |
| [osmium-tool](https://osmcode.org/osmium-tool/) | any recent | only to inspect files |

## 2. Paths

Every command in this guide uses these variables. Put them in a file, such as
`$HOME/cardinal-env.sh`, and `source` it in every shell you use.

```sh
# A clean checkout of this repository: no uncommitted changes, so you know
# what went into each artifact. A git worktree works well.
export CARDINAL_SRC=$HOME/src/cardinal
# Inputs and outputs, on the big disk. Should be outside $CARDINAL_SRC.
export CARDINAL_DATA=/data/cardinal
export CARDINAL_OUT=$CARDINAL_DATA/out

# The planet snapshot, as YYMMDD: a weekly snapshot from
# https://planet.openstreetmap.org/pbf/.
export PLANET_DATE=261109
# The date the release is published under, as YYYY-MM-DD.
export RELEASE_DATE=$(date +%F)

export PLANET_PBF=$CARDINAL_DATA/planets/planet-$PLANET_DATE.osm.pbf
export WOF_DB=$CARDINAL_DATA/wof/whosonfirst-data-admin-latest.spatial.db
export QRANK=$CARDINAL_DATA/qrank/qrank.csv.gz
export SPEEDS=$CARDINAL_DATA/speeds/default_speeds.json
export SOURCES=$CARDINAL_DATA/sources
export GEDTM_DIR=$CARDINAL_DATA/gedtm
export GEDTM_FILE=gedtm_rf_m_30m_s_20060101_20151231_go_epsg.4326.3855_v1.2.tif
export DEM_DIR=$CARDINAL_DATA/dem
export MODELS=$CARDINAL_DATA/models
export TRANSIT_OUT=$CARDINAL_DATA/transit/planet-$RELEASE_DATE

# The POI database (section 5).
export POI_DB_PASSWORD=choose-a-password
export POI_DB_DATA=$CARDINAL_DATA/postgres
```

Create the directories:

```sh
mkdir -p $CARDINAL_DATA/{planets,wof,qrank,speeds,sources,gedtm,dem,models,postgres,transit} $CARDINAL_OUT
```

`planets/`, `wof/`, `qrank/`, `speeds/` and `releases/` match the layout
`k8s/release.sh` expects under `CARDINAL_DATA`, if you later use the example
deployment.

Keep data and build output out of the checkout, even in gitignored
directories. Dagger reads the whole workspace on every call, so a large tree
makes every call slow and breaks `dagger generate`.

## 3. Download the inputs

### Upstream versions

Two inputs are only published as "latest":

- **Who's On First** changes daily.
- **QRank** changes weekly.

Downloading them today gives different files from the ones Cardinal's
current data was built with. As a result, the geocoder index and the tiles'
POI areas won't match Cardinal's byte for byte, though they should be close.

### OpenStreetMap planet (required)

Download the planet from planet.openstreetmap.org itself. The POI service
needs element versions, which some mirrors and extracts strip. The script
resumes after an interruption and checks the published MD5:

```sh
$CARDINAL_SRC/poi_service/stable_ids/download_planet.sh $PLANET_DATE $CARDINAL_DATA/planets
```

### Who's On First (required)

Download the planet SpatiaLite database. It's about 9.8 GB compressed and
34 GB uncompressed:

```sh
curl -fL https://data.geocode.earth/wof/dist/spatial/whosonfirst-data-admin-latest.spatial.db.bz2 \
  | bunzip2 > $WOF_DB
```

Every administrative area in it is indexed for search. The tiles also use it,
to say which areas POIs are in.

### QRank (recommended)

QRank ranks POIs by the page views of their Wikipedia articles. Without it,
every POI with a Wikidata entity counts the same.

```sh
curl -fL -o $QRANK https://qrank.toolforge.org/download/qrank.csv.gz
```

### Default speeds (recommended)

These are speeds for roads without a `maxspeed`, by country, density and road
class. Without this file, Valhalla's built-in speeds by road class are used.
Cardinal uses OpenStreetMapSpeeds':

```sh
curl -fL -o $SPEEDS https://raw.githubusercontent.com/OpenStreetMapSpeeds/schema/master/default_speeds.json
```

### GEDTM (required for contours and routing elevation)

[GEDTM30](https://codeberg.org/openlandmap/GEDTM30) v1.2 is OpenGeoHub's
global bare-earth elevation model at 1 arc second, under CC BY 4.0. It's one
432,407,644,715-byte GeoTIFF.

The 30 m file isn't on Zenodo, which has only the 240 m versions. It's on
OpenGeoHub's server, which supports resuming:

```sh
curl -fL -C - --retry 20 --retry-all-errors -o $GEDTM_DIR/$GEDTM_FILE \
  https://s3.opengeohub.org/global/dtm/v1.2/$GEDTM_FILE
stat -c %s $GEDTM_DIR/$GEDTM_FILE    # 432407644715
```

The tools refuse any layout other than v1.2's. Maps and apps that show
contours or route elevation must credit GEDTM; see "Data credit" in
`topo_pipeline/README.md`.

### Planetiler's auxiliary sources (recommended)

These are water polygons, Natural Earth and lake centerlines. Without
`--sources`, Planetiler downloads them on every build, and they change
upstream. Downloading them once pins them:

```sh
cd $SOURCES
curl -fLO https://osmdata.openstreetmap.de/download/water-polygons-split-3857.zip
curl -fLO https://dev.maptiler.download/geodata/omt/natural_earth_vector.sqlite.zip
curl -fLO https://dev.maptiler.download/geodata/omt/lake_centerline.shp.zip
```

These are the URLs Planetiler itself downloads from (`OpenMapTilesMain.java`
in `planetiler-openmaptiles/`). The last two are MapTiler's mirrors of Natural
Earth and `acalcutt/osm-lakelines`.

Cardinal's current map was built without `--sources`, so these files were
downloaded at build time.

## 4. Set up Dagger

### Engine

Run Dagger's engine yourself. That way you control where it keeps its state
(it must be on the big disk) and how much memory it can use:

```sh
mkdir -p $CARDINAL_DATA/dagger
podman run -d --name cardinal-dagger-engine --privileged \
  -v $CARDINAL_DATA/dagger:/var/lib/dagger \
  registry.dagger.io/engine:v1.0.0-beta.21
# Cap the engine below the host's RAM, with no swap, so a runaway step fails
# instead of taking the machine down.
podman update --memory 85g --memory-swap 85g cardinal-dagger-engine
export DAGGER_ENGINE=container+podman://cardinal-dagger-engine
```

Add the `DAGGER_ENGINE` line to your env file. With docker, use
`docker run` and `container+docker://`.

### Memory and Planetiler's heap

Inside Dagger, the JVM sizes its heap from the host's RAM, not from the
engine's limit. `--max-ram-percentage` is therefore a share of the whole
machine.

On the reference machine
`--max-ram-percentage=38` gives a 47 GiB heap. The default, 50, gave a 67 GiB
heap inside a 72 GiB cap, which leaves too little for the rest. Scale the
percentage so that the percentage times host RAM sits well under your
engine's cap.

### Check it works

Run every `dagger` command from `$CARDINAL_SRC`:

```sh
cd $CARDINAL_SRC
dagger -m data_pipeline api call valhalla-image
```

This prints the pinned Valhalla image, and confirms the CLI, the engine and
the module all load.

## 5. The POI database

### How POI identifiers reach the build

Tiles and the geocoder give each POI a stable identifier, which survives
edits but changes when a different place takes over the element (a new shop
in an old shop's node). Identifiers come from the `poi_service` PostgreSQL
database. `poi_service/README.md` explains how they're assigned.

The build never queries the database for each POI. Instead, its first step,
`Patch` in `data_pipeline/main.go`, works in three steps:

1. Builds `poi_service` from this repository, inside Dagger.
2. Runs `poi_service checkpoint` against the database. This prints the
   latest applied replication sequence and checksum. The database changes
   as diffs are applied, which Dagger can't see, so this runs on every build.
   The result keys the next step's cache, which means the patch is redone
   only when the database has moved on.
3. Runs `poi_service patch`. This loads every known `(element, version) →
   poi_id` into memory (about 24 bytes each), then copies the extract,
   adding two tags to every element whose exact version the database has:
   - `cardinal:poi_id`, the identifier;
   - `cardinal:version`, the OSM version, so clients can tell whether
     cached data is stale.

   It also tags every POI with `cardinal:importance`, as airmail ranks it
   (from QRank, given `--qrank`, and the POI's area). Areas need node
   locations, so osmium adds them to the extract's ways first (as for the
   geocoder index), and the patch writes a patched copy of that file too,
   which the index reads.

The patched extract then feeds:

- **the tiles:** `Poi.java` in `planetiler-openmaptiles/` reads the tags;
- **the geocoder index:** `airmail/airmail/src/index.rs` reads them;
- **the contours,** when used: they're appended to the patched file.

Routing tiles are built from the original, unpatched extract. A change in
the POI database alone therefore never triggers a routing rebuild.

The build is given the database as `--database-url`, a Dagger secret. If the
database runs on the build machine, also pass `--database=tcp://localhost:5432`.
That binds the host's port into the build as the host `database`, so the URL
must use `database` as its host name:

```sh
export DATABASE_URL=postgres://cardinal:$POI_DB_PASSWORD@database:5432/pois       # for dagger
export POI_DATABASE_URL=postgres://cardinal:$POI_DB_PASSWORD@localhost:5432/pois  # for poi_service on the host
```

Without `--database-url`, there's no patch step, and POIs have no
identifiers. Everything else still builds.

### Start PostgreSQL

```sh
podman run -d --name cardinal-pois \
  -e POSTGRES_USER=cardinal -e POSTGRES_PASSWORD=$POI_DB_PASSWORD -e POSTGRES_DB=pois \
  -p 127.0.0.1:5432:5432 -v $POI_DB_DATA:/var/lib/postgresql:Z \
  docker.io/library/postgres:18
```

The example deployment runs PostgreSQL 18.6. `poi_service` creates its
tables (migrations) the first time it connects.

### Build poi_service

```sh
cd $CARDINAL_SRC/poi_service
cargo build --release
export POI_SERVICE=$CARDINAL_SRC/poi_service/target/release/poi_service
```

### Fill it

Choose one of the two options below.

#### Option A: bootstrap from the planet

This gives you your own identifier history, numbered the same as every other
deployment that starts from the same planet with the same ruleset.

```sh
DATABASE_URL=$POI_DATABASE_URL $POI_SERVICE bootstrap --planet $PLANET_PBF
```

On the planet, this loads about 56 million POIs in about 6 minutes, and logs
`bootstrapped; checksum …`. Daily diffs start after the planet's replication
sequence (5136 for `261005`).

To bring the database past the planet date, apply daily diffs from
planet.openstreetmap.org:

```sh
DATABASE_URL=$POI_DATABASE_URL $POI_SERVICE update            # until caught up
DATABASE_URL=$POI_DATABASE_URL $POI_SERVICE update --follow   # keep polling
```

You don't need to apply diffs to build from the planet itself. The patch only
uses versions up to the extract's date, and diffs only add later versions.

Identifiers depend on the planet, the replication URL and the ruleset
(`RULESET_VERSION` in `poi_service/src/rules.rs`). Changing any of them means
a new, empty database.

#### Option B: restore Cardinal's published identifiers

This gives you the same identifiers as Cardinal's own deployment, at its
latest checkpoint. Restore an empty database from the public bucket:

```sh
$POI_SERVICE fetch --url https://BUCKET-HOST/releases --dir $CARDINAL_DATA/poi-releases
DATABASE_URL=$POI_DATABASE_URL $POI_SERVICE import --releases $CARDINAL_DATA/poi-releases
```

`BUCKET-HOST` is the public host of the bucket the app reads releases from.
A snapshot is about 1.9 GB, and imports in about 8 minutes.

### Check it

```sh
DATABASE_URL=$POI_DATABASE_URL $POI_SERVICE checkpoint
DATABASE_URL=$POI_DATABASE_URL $POI_SERVICE covers --input $PLANET_PBF
```

- **`checkpoint`** prints the sequence and checksum. Two databases that
  agree on both have assigned exactly the same identifiers. To compare with
  Cardinal's, look at the published POI release with the same sequence
  (`releases/poi/SEQ/release.json`).
- **`covers`** fails unless the database has applied diffs up to the
  extract's timestamp. If it fails, POIs newer than the database would get
  no identifier. The Dagger build doesn't run this check, so run it yourself
  before every build.

## 6. Search models (first release only)

The geocoder needs two models that are trained by hand now and then, not with
every release:

- the per-language categorical models, which read queries like "pizza",
  "vegan" or "Bäckerei";
- the query tagger, which reads which words of a query are what.

`build` requires both, and packs them into `geocoder.btrfs`. If you kept
the previous release's models in `$MODELS`, reuse them and skip this
section. To get them back out of a published image, run
`btrfs restore geocoder.btrfs DIR`, which copies out `categories/` and
`tagger.model` along with the index.

### Categorical models

```sh
cd $CARDINAL_SRC
dagger -m data_pipeline api call categorical-models \
  --osm=$PLANET_PBF --wof-db=$WOF_DB -o $MODELS/categories
ls $MODELS/categories | wc -l
```

The step adds node locations to ways (osmium), exports each POI's names with
the languages spoken where it is, and trains one model per qualifying
language. Training runs one language after another, and each finished
language is cached, so a rerun resumes.

Cardinal's current set has 66 languages, each about 45 MB. Defaults:
`--min-pois=50000`, `--min-cuisine=1000`. See `airmail/categorical/README.md`.

The models don't depend on POI identifiers, so the unpatched planet is fine.

### Query tagger

The tagger trains on a geocoder index, so build one first. Without a POI
database, `build` reuses this exact index from Dagger's cache later. With a
database, the index is rebuilt from the patched extract, and this one is only
for training.

```sh
cd $CARDINAL_SRC
dagger -m data_pipeline api call index \
  --osm=$PLANET_PBF --wof-db=$WOF_DB --qrank=$QRANK -o $CARDINAL_OUT/index-for-tagger
cd $CARDINAL_SRC/airmail
cargo run --release --bin tagger -- train \
  --index $CARDINAL_OUT/index-for-tagger --out $MODELS/tagger.model
cargo run --release --bin tagger -- eval \
  --index $CARDINAL_OUT/index-for-tagger --model $MODELS/tagger.model eval/tagger.tsv
```

Training on the planet uses every 20th document by default (`--stride`),
about 14 million examples. That takes about an hour on 32 cores, and the
model is about 105 MB. `airmail/BUILD.md` ("The query tagger") covers pruning and
calibrating it.

Afterwards, delete `$CARDINAL_OUT/index-for-tagger`.

## 7. Contour lines

Contours are generated on their own, before the build. A planet's DEM tiles
are hundreds of gigabytes, and passing contours in keeps the DEM out of the
engine for the rest of the build.

**Cut GEDTM into 1° tiles.** On the planet, this makes 19,351 tiles, 279 GB,
in about 18 minutes:

```sh
mkdir -p $DEM_DIR/gedtm
$CARDINAL_SRC/topo_pipeline/scripts/tile_gedtm.sh $GEDTM_DIR/$GEDTM_FILE $DEM_DIR/gedtm -180 -65 180 85 16
```

The arguments after the output directory are west, south, east, north and
the number of jobs. Reruns skip tiles already done.

**Write the configuration.** This is the configuration Cardinal's planet
contours were made with:

```sh
cat > $CARDINAL_DATA/topo.toml <<'EOF'
# GEDTM v1.2 tiles from topo_pipeline/scripts/tile_gedtm.sh, mounted at /dem.
[contours]
cache_gb = 16.0

[[source]]
name = "gedtm"
dir = "/dem"
suffix = ".tif"
arcsec = 1.0
median = true
sigma = 1.2
EOF
```

Settings it doesn't name take their defaults. `topo_pipeline/topo.example.toml`
documents all of them.

**Generate.** On the planet, this makes 331,500 chunks and a 149 GB file, in
about 67 minutes:

```sh
cd $CARDINAL_SRC
dagger -m data_pipeline api call contours \
  --dem=$DEM_DIR --config=$CARDINAL_DATA/topo.toml -o $CARDINAL_OUT/contours.osm.pbf
```

Finished chunks stay in the `topo-chunks` cache volume, so a rerun only
computes what's missing.

**Check it** (optional). `topo seams` checks that every line ending on a
chunk boundary meets one on the other side. See `topo_pipeline/README.md`.

Afterwards, you can delete `$DEM_DIR`. It takes about 18 minutes to cut
again.

## 8. Build tiles, index and routing tiles

### Serve GEDTM for routing elevation

The build reads only the parts of GEDTM it needs, over HTTP, instead of
copying all 432 GB into the engine. Serve the file with any server that
supports range requests, on a port of your choice:

```sh
podman run -d --rm --name gedtm-http -p 127.0.0.1:8471:8080 \
  -v $GEDTM_DIR:/srv:ro,z docker.io/library/caddy:2.11.7-alpine \
  caddy file-server --root /srv --listen :8080
curl -sI http://127.0.0.1:8471/$GEDTM_FILE | grep -i content-length
```

Keep it up until the build finishes. Dagger's cache is keyed on the URL, not
the file, so if you replace the file, serve it on another port.

### Run the build

```sh
cd $CARDINAL_SRC
DATABASE_URL=$POI_DATABASE_URL $POI_SERVICE covers --input $PLANET_PBF   # section 5
dagger -m data_pipeline api call build \
  --osm=$PLANET_PBF \
  --wof-db=$WOF_DB \
  --qrank=$QRANK \
  --categories=$MODELS/categories \
  --tagger=$MODELS/tagger.model \
  --database-url=env://DATABASE_URL --database=tcp://localhost:5432 \
  --contours=$CARDINAL_OUT/contours.osm.pbf \
  --sources=$SOURCES \
  --routing-speeds=$SPEEDS \
  --gedtm=/vsicurl/http://gedtm:8471/$GEDTM_FILE --gedtm-service=tcp://localhost:8471 \
  --max-ram-percentage=38 \
  --args=--nodemap-type=array,--storage=mmap,--force \
  -o $CARDINAL_OUT/release
podman stop gedtm-http
```

The planet needs these flags:

- **`--nodemap-type=array --storage=mmap`:** Planetiler's node map for the
  planet: an array, memory-mapped from disk.
- **`--force`:** gets past Planetiler's pre-flight memory check. The check
  estimates heap from the input's size, and with contours most of the input
  is contour ways, which need no relation state. The real peak was 36 of
  47 GiB. If the heap really runs out, Java says `OutOfMemoryError`.
- **`--max-ram-percentage=38`:** see section 4.

### What it does

The steps run one at a time, since each can use most of the machine:

1. **Patch:** adds POI identifiers (section 5).
2. **Append contours:** splices `contours.osm.pbf` into the patched planet,
   in the node, way and relation order Planetiler needs. It also turns on
   the topo layers' extra options (`--transportation_z13_paths`,
   `--waterway_z12_streams`).
3. **Tiles:** the `planetiler-openmaptiles` fork writes `tiles.mbtiles`, as
   gzipped MLT tiles. POIs get the areas they're in from Who's On First.
4. **Geocoder index:** airmail's indexer loads the POIs and addresses from
   the patched copy with node locations on its ways that step 1 wrote (osmium
   adds them before the patch; its node index goes on disk for the planet),
   then every Who's On First area. It writes `index/`.
5. **Routing tiles:** mainline Valhalla (the image pinned in
   `valinor/tools/oracle/valhalla-image`) builds graph tiles from the
   **unpatched** planet, with pedestrian areas, bounding circles,
   administrative areas and time zones. With GEDTM, the build stops before
   the elevation stage, cuts `.hgt` tiles under the graph
   (`routing_elevation.py`, about an hour), adds elevation and grades to
   every edge, and packs `routing/tiles.tar` with `routing/valhalla.json`.
6. **Images:** packs two read-only btrfs filesystem images for servers, with
   their files compressed with zstd (`BtrfsImage` in `main.go`):
   - `routing.btrfs` holds `tiles.tar` and `valhalla.json` at its root.
     Valinor maps the tarball whole: one mapping, where loose tiles would
     need one per tile.
   - `geocoder.btrfs` holds everything the geocoder serves: the index as
     `index/`, plus the `categories/` and `tagger.model` you passed in.

The output is `$CARDINAL_OUT/release/`:

| Path | What |
|---|---|
| `tiles.mbtiles` | the map tiles |
| `geocoder.btrfs` | `index/`, `categories/` and `tagger.model` inside the image |
| `geocoder.json` | the index's `airmail-index.json`, giving its schema version without mounting the image |
| `routing/tiles.tar`, `routing/valhalla.json` | the routing tiles, for the loose phone upload and transit |
| `routing.btrfs` | the same two files in an image, for servers |

The index and the models aren't exported loose; keep your copies of the
models in `$MODELS` for the next release. To look inside an image without mounting
it, use `btrfs restore IMAGE DIR` (from btrfs-progs), which copies its files
out.

**How servers use the images.** A server downloads an image and mounts it
read-only:

```sh
sudo mount -o loop,ro geocoder.btrfs /srv/geocoder
```

The kernel decompresses pages as they're read, so the files can be
memory-mapped as usual. The image stays compressed on disk and in transit.

`--image-compression-level` sets the zstd level, from 1 to 15 (default 3).
Higher levels make smaller images but take longer to build. Reads cost
about the same at any level.

Images aren't byte-reproducible, because btrfs records when it was made. To
compare two builds, compare what the images hold, not the images' hashes.

Each step can also run on its own, with the same flags where they apply:
`tiles`, `index`, `routing-tiles`, `topo-tiles`. `data_pipeline/README.md`
documents every flag. A failed `build` reruns its steps from Dagger's cache
where their inputs haven't changed. Steps marked "never cache", including
the patch, still check the database each time.

### What to expect

From Cardinal's planet build, for checking yours:

| Output | Result |
|---|---|
| `tiles.mbtiles` | 363 GB, 3 h 35 min (pass 1: 6.5 min, pass 2: 1 h 39 min, sort: 11 min, archive: 1 h 37 min). 52 billion nodes, 38 billion of them from contours. Largest tile: 1.9 MB. |
| Geocoder index | 146 GB before packing, schema 7. 217.3 million places, 1.56 million postcodes, 5.23 million Who's On First places. OSM load: 78 min; Who's On First load: 12 min. |
| `routing/tiles.tar` | 107 GB, 209,841 tiles, one Valhalla build ID |

The images' planet sizes and packing times haven't been measured yet.

Check the results:

```sh
sqlite3 $CARDINAL_OUT/release/tiles.mbtiles "select name, value from metadata where name in ('format','minzoom','maxzoom')"
tar tf $CARDINAL_OUT/release/routing/tiles.tar | grep -c '\.gph$'
cat $CARDINAL_OUT/release/geocoder.json
btrfs filesystem show $CARDINAL_OUT/release/geocoder.btrfs
btrfs filesystem show $CARDINAL_OUT/release/routing.btrfs
```

`btrfs filesystem show` and `btrfs restore` work on an image file without
root.

`btrfs filesystem show` prints how much data the image holds. Even a tiny
image is about 100 MB, because btrfs reserves fixed space for its metadata.

To serve the routing tiles locally and try some routes, see `valinor/` and
`k8s/README.md`.

### Building with the example deployment instead

If you run the example k3s deployment, `k8s/release.sh build` wraps this
step. It checks `covers`, port-forwards to the cluster's POI database, runs
`build` into `$CARDINAL_DATA/releases/<id>`, and writes a `manifest.json`.
`k8s/release.sh promote <id>` then switches the tileserver, geocoder and
router to it.

It passes `--contour-config` and `--dem` rather than a contours file, and no
GEDTM. Add those yourself for a planet with contours and elevation. See
`k8s/README.md`.

`k8s/release.sh` and the example geocoder deployment still expect a loose
`index/` in the release, which `build` no longer writes. Until they're
updated for `geocoder.btrfs`, build planet releases with the commands above.

## 9. Transit

Transit is built outside Dagger, from GTFS feeds that
[Transitous](https://github.com/public-transport/transitous) lists, at a
pinned commit. It needs `routing/tiles.tar` from section 8, because the walks
between stops are routed on it. For background, see `valinor/docs/transit.md`
("Offline build") and `valinor/pipeline/transit/README.md`.

### Build the tool

```sh
cd $CARDINAL_SRC/valinor
cargo build --release -p valinor-transit-build
export TRANSIT_BUILD=$CARDINAL_SRC/valinor/target/release/valinor-transit-build
mkdir -p $TRANSIT_OUT/logs
```

### Choose regions

On purpose, there's no "all regions" option. A region is the name of a file
in Transitous's `feeds/` directory, at the commit pinned by
`TRANSITOUS_COMMIT` in `valinor/pipeline/transit/Containerfile`. Cardinal's
planet build uses all of them (143 when its current transit data was built):

```sh
commit=$(sed -n 's/^ARG TRANSITOUS_COMMIT=//p' $CARDINAL_SRC/valinor/pipeline/transit/Containerfile)
git clone https://github.com/public-transport/transitous $CARDINAL_DATA/transitous
git -C $CARDINAL_DATA/transitous checkout $commit
ls $CARDINAL_DATA/transitous/feeds/*.json | xargs -n1 basename | sed 's/\.json$//' | tr '\n' ' ' \
  > $TRANSIT_OUT/regions.txt
```

`valinor/pipeline/transit/excluded-sources.txt` lists sources that are never
fetched.

### Run

The three stages take about 4 hours on the planet, which outlasts most
terminal sessions. Put them in a script and run it as a systemd user unit:

```sh
cat > $TRANSIT_OUT/run.sh <<EOF
#!/bin/bash
set -euo pipefail
regions=(); for r in \$(cat $TRANSIT_OUT/regions.txt); do regions+=(--region "\$r"); done
$CARDINAL_SRC/valinor/pipeline/transit/run.sh --output $TRANSIT_OUT --cache $TRANSIT_OUT/cache \
  --jobs 4 "\${regions[@]}" > $TRANSIT_OUT/logs/fetch.log 2>&1
$TRANSIT_BUILD compile --input $TRANSIT_OUT --output $TRANSIT_OUT > $TRANSIT_OUT/logs/compile.log 2>&1
$TRANSIT_BUILD transfers --compiled $TRANSIT_OUT \
  --street-tiles $CARDINAL_OUT/release/routing/tiles.tar --output $TRANSIT_OUT > $TRANSIT_OUT/logs/transfers.log 2>&1
$TRANSIT_BUILD feed-index --build $TRANSIT_OUT --output $TRANSIT_OUT/feed-index > $TRANSIT_OUT/logs/feed-index.log 2>&1
echo done
EOF
chmod +x $TRANSIT_OUT/run.sh
systemd-run --user --unit=cardinal-transit -p MemoryMax=80G $TRANSIT_OUT/run.sh
journalctl --user -u cardinal-transit -f
```

The script stops at the first failing stage. A feed that fails to download
or compile is recorded and skipped, without stopping the stage.

The transfers stage peaks at about 65 GB, so give the unit more than 64G.

| Stage | Planet result |
|---|---|
| Fetch (in podman) | 68 min. 2,170 feeds fetched, 1,299 skipped (GBFS, sources needing Transitous's feed proxy or keys, expired). |
| Compile | 2 h 12 min, 27 GB peak. 2,165 compiled, 5 failed, listed in `compile.json`. 13 GB of timetables, 4.29 million stops. |
| Transfers | 62 min, 65 GB peak. Zoom 14 tiles, 6.5 GB. |
| Feed index | 1.5 min. |

Feeds change daily, so your counts will differ somewhat from these.

The fetch cache (`$TRANSIT_OUT/cache`) makes the next fetch incremental; keep
it, or point the next build's `--cache` at it.

## 10. Publish

`cardinal-publish` uploads each kind of data (map, routing, transit, POI) as
its own dated release to an S3-compatible bucket that clients read over plain
HTTPS. The bucket must allow public reads. `publish/README.md` describes the
layout and every flag.

```sh
export S3_BUCKET=your-bucket
export S3_ENDPOINT=https://t3.storage.dev     # Tigris; change for another provider
export BUILD_COMMIT=$(git -C $CARDINAL_SRC rev-parse HEAD)
read -rs S3_KEY_ID; read -rs S3_SECRET; export S3_KEY_ID S3_SECRET
target=(--target s3://$S3_BUCKET --endpoint $S3_ENDPOINT --access-key-id $S3_KEY_ID --secret-access-key $S3_SECRET)
cd $CARDINAL_SRC/publish
```

To try any step without a bucket, use `--target dir:$CARDINAL_DATA/bucket-test`
instead. `--dry-run` checks everything but uploads nothing.

Every step can be rerun. It skips objects already in the bucket and resumes
an interrupted upload. Once a release is complete, it never changes. To
publish different data on the same day, it goes up as `DATE.1`.

### Routing (first)

```sh
uv run cardinal-publish routing "${target[@]}" \
  --tar $CARDINAL_OUT/release/routing/tiles.tar --image $CARDINAL_OUT/release/routing.btrfs \
  --date $RELEASE_DATE --source planet-$PLANET_DATE --build-commit $BUILD_COMMIT
```

This uploads two things:

- **loose tiles, for phones:** it reads the tarball once, without extracting
  it, and uploads 209,841 tiles plus their indexes, in about 15 minutes at
  1 Gbit/s;
- **`routing.btrfs`, for servers:** one resumable multipart upload.

The release's `release.json` records the image's size and SHA-256.

### Map

First convert the MBTiles to one PMTiles archive. Its temporary files and its
output are each about the size of the input, so put `--tmpdir` on another
disk if one disk can't hold both:

```sh
uv run cardinal-publish map-convert --mbtiles $CARDINAL_OUT/release/tiles.mbtiles \
  --out $CARDINAL_OUT/planet.pmtiles --tmpdir $CARDINAL_OUT/pmtiles-tmp --mlt-header
uv run cardinal-publish map "${target[@]}" \
  --pmtiles $CARDINAL_OUT/planet.pmtiles --date $RELEASE_DATE --source planet-$PLANET_DATE
```

The planet archive is 324 GB.

`--mlt-header` is required. go-pmtiles doesn't recognise MLT, and without the
flag the archive's tile type is unknown and it can't be served.

### Transit

```sh
(cd $CARDINAL_SRC/crates && cargo build --release -p offline_map --bin cardinal-pack)
uv run cardinal-publish transit "${target[@]}" \
  --build $TRANSIT_OUT --staging $TRANSIT_OUT/feed-index --work $TRANSIT_OUT-archives \
  --date $RELEASE_DATE --routing-date $RELEASE_DATE --build-commit $BUILD_COMMIT
```

`--routing-date` must name a published routing release with the same
Valhalla build ID that the transfers were computed on. If it doesn't, the
step fails.

### Geocoder

```sh
uv run cardinal-publish geocoder "${target[@]}" \
  --image $CARDINAL_OUT/release/geocoder.btrfs --metadata $CARDINAL_OUT/release/geocoder.json \
  --date $RELEASE_DATE --source planet-$PLANET_DATE --build-commit $BUILD_COMMIT
```

This uploads `geocoder.btrfs` as one resumable multipart upload, under
`releases/geocoder/DATE/`. Its `release.json` records:

- the image's size and SHA-256;
- the index's schema version, so a server can tell whether its geocoder can
  read the release before downloading it.

Only servers use geocoder releases, so `latest.json` doesn't name them.

### POI identities

Run this only if you bootstrapped your own database (option A in section 5):

```sh
DATABASE_URL=$POI_DATABASE_URL $POI_SERVICE export --staging $CARDINAL_DATA/poi-staging
uv run cardinal-publish poi "${target[@]}" --staging $CARDINAL_DATA/poi-staging --latest
```

Run both again after each `update` to publish that day's diff.
`cardinal-publish poi-prune` removes old releases.

### Point clients at the release

```sh
uv run cardinal-publish latest "${target[@]}" \
  --map $RELEASE_DATE --routing $RELEASE_DATE --transit $RELEASE_DATE
```

This writes `releases/latest.json`, which apps read to pick their releases.

Keep old map releases in the bucket while any client might still use them.
Apps pin one map release and keep reading tiles from it.

## 11. Serving

Not covered here:

- **Map tiles:** `deploy/pmtiles-accelerator/` serves every map release in
  the bucket from a few regions with go-pmtiles. For a single server,
  Martin can serve `tiles.mbtiles` directly (`k8s/`).
- **Geocoder:** `airmail_service` serves a mounted `geocoder.btrfs`: its
  `index/`, `categories/` and `tagger.model`. The tiles and the index it serves must come from the same
  build, since they agree on POI identifiers. See `k8s/README.md`.
- **Routing:** `valinor-server` serves `tiles.tar` with its `valhalla.json`,
  from a mounted `routing.btrfs` or from `routing/`. See `k8s/README.md` and
  `valinor/docs/routing.md`.
- **The example deployment:** `k8s/README.md` sets up all three behind one
  hostname, plus the POI database and its daily updater.

## 12. Try a region first

Every step works on any extract. A [Geofabrik](https://download.geofabrik.de)
state or country goes through the same code paths in minutes rather than
hours. Washington, with contours and all the planet flags, took 35 minutes
end to end.

```sh
export PLANET_PBF=$CARDINAL_DATA/planets/washington-latest.osm.pbf
curl -fL -o $PLANET_PBF https://download.geofabrik.de/north-america/us/washington-latest.osm.pbf
```

What changes for a region:

- **POI database:** a Geofabrik extract isn't a planet, so bootstrap a
  separate test database with its updates URL:
  `bootstrap --planet $PLANET_PBF --replication-url https://download.geofabrik.de/north-america/us/washington-updates/`.
  Or build without `--database-url`.
- **Who's On First:** the planet database works for any extract, and
  indexes every area in it.
- **GEDTM:** pass the region's whole-degree bounds to `tile_gedtm.sh`
  (`-125 45 -116 50` for Washington). For contours, pass `--bbox=W,S,E,N` to
  `contours`. You can skip downloading GEDTM entirely: both
  `tile_gedtm.sh` and `--gedtm` accept
  `/vsicurl/https://s3.opengeohub.org/global/dtm/v1.2/$GEDTM_FILE` and read
  only what they need, in which case leave out `--gedtm-service`.
- **Memory flags:** leave out `--max-ram-percentage` and `--args` if you
  like; the defaults suit an extract.
- **Transit:** pass only the regions it covers, such as `--region us-wa`.

## 13. When things go wrong

- **A step's log** is in Dagger's output. Run long builds under
  `systemd-run --user` or `tmux`, with output to a file, so a dropped session
  doesn't cancel them.
- **Out of disk:** Dagger fails the step. Prune the cache (below), or
  delete intermediate files you no longer need, and rerun.
- **GEDTM reads stalling** during "cutting elevation tiles": the cutter sets
  GDAL HTTP timeouts and retries, but check that `gedtm-http` is still up.
- **Planetiler's pre-flight check** fails with "temporary profile storage"
  on the planet: pass `--force` (section 8).
- **`covers` fails:** the extract is newer than the POI database. Run
  `poi_service update` until it passes.
- **The geocoder refuses an index:** it reads only the index schema it was
  built for. Serve an index with the geocoder image from the same commit.

## 14. Cleaning up

Dagger keeps copies of every input and intermediate file in its engine.
Once your outputs are exported, reclaim the space:

```sh
dagger -m core api call engine local-cache prune
```

The next build then imports the planet again.
