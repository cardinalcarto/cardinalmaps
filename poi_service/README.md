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

## Container image

`Dockerfile` builds the `cardinal-poi-service` image, which
`.github/workflows/images.yml` publishes on pushes to `main`. See
`k8s/README.md` for an example deployment.

## Tests

`cargo test` runs the unit tests. `matches_python_labels` (ignored by default)
checks the rules against `stable_ids`' Python implementation; see
`stable_ids/README.md`.
