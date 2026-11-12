# POI service

Tracks every OSM POI and its latest tags in PostgreSQL, and gives each one an
identifier (`poi_id`) that survives edits but changes when the element is
reused for a different place, such as a new business moving into a shop.

## Identifiers

A POI is an element with a `name` or `brand` and any
[top-level key](https://wiki.openstreetmap.org/wiki/Top-level_tag) that isn't
about transportation infrastructure, such as `amenity`, `shop`, `building`,
`natural`, `place` or `public_transport`, or a specific tag such as
`highway=bus_stop` (`POI_KEYS` and `POI_TAGS` in `src/rules.rs` list them). Some
tags, like `amenity=drinking_water` and `amenity=charging_station`, make an
element a POI with no name at all (`UNNAMED_POI_TAGS`).
Each POI version belongs to an identity:

- A new POI starts a new identity.
- A POI version keeps its element's identity if the rules in `src/rules.rs`
  say it's the same place as the element's last POI version (similar name, or
  one of its translations or other names, and no brand switch alongside a
  rename, or with no name, the same `UNNAMED_POI_TAGS` tags). Otherwise it
  starts a new one. A name added to a nameless POI keeps its identity.
- Versions that are deleted or aren't POIs (e.g. `disused:shop`) don't end an
  identity: if the element becomes a POI again, it's compared against its last
  POI version.

Identities are numbered sequentially from 1, so they fit in 32 bits for a
long time. Every deployment that bootstraps from the same planet and applies
the same daily diffs with the same ruleset assigns the same numbers:

- Bootstrap numbers POIs in planet order (type, then ID).
- Each daily diff numbers its new identities in (type, ID, version) order.
  Daily diffs include every version of an element, not just the latest.
- Only daily diffs are applied, because how changes are grouped into diffs
  affects the numbering.

`checkpoints` records a running SHA-256 over every (type, ID, version,
poi_id) assigned, after the planet and after each diff, so deployments can
confirm they agree. Changing the rules' behaviour, including the Unicode data
they use, needs a new `RULESET_VERSION`; the service refuses to update a
database built with another one.

## Tables

| Table | Contents |
| --- | --- |
| `pois` | One row per element that has been a POI: its latest version, whether that version is a POI (`active`), and the tags and `poi_id` of its last POI version |
| `poi_versions` | `(osm_type, osm_id, version, poi_id)` for every POI version since the planet |
| `origin` | The planet timestamp, ruleset and replication URL the identifiers depend on |
| `checkpoints` | Each applied replication sequence, the next `poi_id`, and the checksum |
| `poi_changes` | Each diff's new `pois` rows and each element's identity before it, for `export` |

`poi_versions.sequence` records which diff added each version (NULL for the
planet's), and `checkpoints.journaled` whether a diff's changes are in
`poi_changes`; diffs applied before that migration can't be exported.

## Running

The planet must include element metadata (versions), as
planet.openstreetmap.org's files do. `stable_ids/download_planet.sh`
downloads one.

```sh
export DATABASE_URL=postgres://user@host/pois
cargo run --release -- bootstrap --planet planet-261005.osm.pbf
cargo run --release -- update --follow
```

`update` applies daily diffs from the planet's date until caught up, and with
`--follow` keeps polling for new ones. For a quick test, bootstrap from a
Geofabrik extract with `--replication-url` set to its `-updates/` URL.

## Adding identifiers to an extract

```sh
cargo run --release -- patch --input planet.osm.pbf --output planet-ids.osm.pbf
```

copies the file, adding `cardinal:poi_id=<poi_id>` and
`cardinal:version=<version>` to every element whose exact version is in
`poi_versions`, and replacing any such tags already there. Since each `poi_id`
belongs to a single element, the OSM version tells consumers (tiles, the
geocoder) whether a POI's data is fresher than what they cached. All known
versions are loaded into memory first (about 24 bytes each), so PostgreSQL
isn't queried while the file is rewritten. Blocks without a known POI version
are copied unchanged. The input must include element metadata, and POIs whose
version the database hasn't seen (for example, if the input is newer than the
database) are counted and left without an identifier.

`checkpoint` prints the latest checkpoint's sequence and checksum. They change
whenever the identifiers `patch` would add can, so builds use them to tell
whether an earlier patched file is still current.

`covers --input extract.osm.pbf` fails unless the database has applied diffs
up to the file's replication timestamp, so that `patch` can identify all of
its POIs.

## Published tables

The database stays on the build host. What it knows is published to the
public bucket under `releases/poi/`, so anyone can restore it, patch extracts
or answer ID status without PostgreSQL or an OSM planet:

```sh
poi_service export --staging poi-staging     # after each `update`
uv run cardinal-publish poi --target s3://BUCKET ... --staging poi-staging --latest
```

`export` reads the database in one read-only transaction, so it can run
while `update` does. It writes a release for every checkpoint after the
newest one in `--staging`, and a snapshot of the latest checkpoint when given
`--snapshot`, when `--staging` has none, or when the newest is
`--snapshot-days` (7) older. Each release is written under a temporary name
and renamed once complete, so a rerun after a failure starts over cleanly;
releases never change once written. `export` checks each delta against its
checkpoint before writing it, and reads each snapshot back and checks it (see
`import`) before marking it complete. `cardinal-publish` (`publish/README.md`)
uploads them and prunes old ones.

```
releases/poi/
  SEQ/release.json                 every checkpoint, by OSM replication sequence
  SEQ/delta.bin.zst                what that diff changed (none for the planet's)
  snapshots/SEQ/release.json
  snapshots/SEQ/versions.bin.zst   all of poi_versions, with the sequence that added each row
  snapshots/SEQ/pois.bin.zst       all of pois, with tags
  snapshots/SEQ/status.bin.zst     a fixed-size record per poi_id, for lookups
  snapshots/SEQ/checkpoints.json   every checkpoint row
```

`release.json` gives the checkpoint (`sequence`, `timestamp`, `next_poi_id`,
`checksum`), what it follows (`previous`, for a delta), the origin
(`planet_timestamp`, `ruleset`, `replication_url`), each file's size and
SHA-256, row counts, and `snapshot`: the snapshot a restore up to this
checkpoint starts from. To restore checkpoint S, take snapshot
`release(S).snapshot`, then apply the deltas after it, one per sequence, up
to S. No listing is needed: `latest.json`'s `poi` names the newest checkpoint.

### Formats

Every file is checked by its SHA-256 in `release.json`. The row files are a
zstd frame (level 15, with zstd's content checksum) of a header (an 8-byte
magic, `CPOIVERS`, `CPOIPOIS` or `CPOIDLTA`, and a little-endian u32 format
version, 1) and rows as LEB128 varints. Rows are in (type, ID, version) order,
in three sections (nodes, ways, relations), each ended by a 0; a row starts
with its ID's delta from the previous row's in the section plus one, then
its version, then its `poi_id` as a zigzag delta from the previous row's.
Each table ends with its row count (u64). Identifiers were numbered in key
order, so most of these deltas take a byte each, and a table can be streamed
in order without an index.

- `versions.bin.zst`: after the header, the planet checkpoint's sequence
  (i64); each row then has the sequence that added it as a varint, 0 for the
  planet's and otherwise its distance from the planet's. That's what lets a
  restore recompute every checkpoint's checksum.
- `pois.bin.zst`: each row has an active byte (0 or 1), then its tags: a
  count, then each key and value as a length and UTF-8 bytes, in key order.
- `delta.bin.zst`: after the header, the previous checkpoint and this one
  (sequence, timestamp in microseconds, `next_poi_id`, 32-byte checksum),
  this one's `applied_at` (µs), then the diff's new `poi_versions` rows and
  its new `pois` rows. Each `pois` row also has the element's identity before
  the diff (0 if new; otherwise a zigzag delta from its `poi_id`, plus one),
  so the status table can be updated from deltas alone.

A delta is exactly what `update` stored for the diff, which is everything a
restore needs to replay it: the upserted `pois` rows (with tags and active
flags), the inserted `poi_versions` rows and the checkpoint row.

`status.bin` (published as `status.bin.zst`; `poi_service status` writes it
uncompressed for the geocoder to memory-map) is a 128-byte header (the magic
`CPOISTAT`, u32 format 1, u32 record size 8, then i64s: sequence, timestamp
in seconds, `next_poi_id`, record count, then the 32-byte checksum, then
zeros) and then one little-endian u64 per `poi_id` below `next_poi_id`,
starting at 0, so a lookup is one read at `128 + 8 × poi_id`:

| Bits | Field |
| --- | --- |
| 0-1 | state: 0 unassigned, 1 active (its element is a POI), 2 inactive (deleted or not a POI; may come back), 3 replaced (its element has a newer identity) |
| 2-3 | OSM type: 0 node, 1 way, 2 relation |
| 4-23 | the last OSM version with this identity (up to 1,048,575; the planet's highest is 6,165) |
| 24-63 | OSM ID (up to 1.1 × 10¹²; the planet's highest is 1.4 × 10¹⁰) |

`export` refuses a value that doesn't fit, which would need format 2.

Why these: the tables are tens of millions of sorted rows of small integers,
which delta varints shrink to about a byte a field before zstd, without a
dependency like Parquet's; tags are mostly repeated keys, which zstd
handles. A `pg_dump` would tie readers to PostgreSQL and isn't byte-stable.
The status table is fixed-size and indexed by `poi_id` so the geocoder can
map it and look up any ID without parsing or building anything.

Sizes for the planet (68,185,562 POIs, October 2026), against 29 GB in
PostgreSQL: `versions.bin.zst` 90 MB, `pois.bin.zst` 1.65 GB,
`status.bin.zst` 178 MB (545 MB uncompressed), so a snapshot is 1.9 GB.
A day's delta is 2 to 6 MB (median 2.3 MB for 55,000 changed elements), and
four weeks of deltas plus the snapshots they restore from is about 4 GB.
`poi_changes` grows the database by about 20 MB a day; it's only needed until
a diff is exported.

Measured on the build host: a snapshot export takes 6 to 9 minutes (mostly
zstd) and 2.2 GB of memory, a day's delta about a second; importing the
planet snapshot takes 8 minutes and 1.6 GB, and a delta about 5 seconds.

### Reading them without PostgreSQL

```sh
poi_service fetch --url https://BUCKET-HOST/releases --dir poi-releases
poi_service patch --releases poi-releases --input extract.osm.pbf --output extract-ids.osm.pbf
poi_service covers --releases poi-releases --input extract.osm.pbf
poi_service status --releases poi-releases --output poi-status.bin
DATABASE_URL=... poi_service import --releases poi-releases
```

`fetch` downloads the releases that reach the checkpoint `latest.json` names
(or `--sequence`), keeping files it already has. `patch --releases` gives
byte-identical output to `patch` from the database at the same checkpoint.
`status` writes the geocoder's status file (the snapshot's, updated by each
delta since), replacing `--output` by a rename. `import` restores an empty
database from the snapshot, then applies each delta, or brings a database up
to date with the deltas after its checkpoint; the restored database is
identical to the original, row for row, except `poi_changes` rows from
before the snapshot.

Everything that reads releases checks each file's SHA-256, that each delta
follows the checkpoint before it, and that its rows give its checksum and
counter. `import` also checks a snapshot completely before loading it: its
versions table against every checkpoint's checksum (one SHA-256 per
checkpoint, since a diff's rows are in key order within the whole table) and
counter, each `pois` row against its element's versions, and the status file
against both.

## Container image

`Dockerfile` builds the `cardinal-poi-service` image, which
`.github/workflows/images.yml` publishes on pushes to `main`. See
`k8s/README.md` for an example deployment.

## Tests

`cargo test` runs the unit tests. `matches_python_labels` (ignored by default)
checks the rules against `stable_ids`' Python implementation; see
`stable_ids/README.md`.
