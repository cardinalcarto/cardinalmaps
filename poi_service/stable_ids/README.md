# Measuring the stable-ID rules

OSM element IDs are not stable identities for places: when a business closes,
mappers often retag the same node or way for whatever opens there next. The POI
service (`poi_service/src/rules.rs`) decides whether two versions of an element
describe the **same** place or a **different** one, keeping an identifier
across edits but issuing a new one when a place is replaced. This directory
builds labelled pairs from real OSM edits for reviewing and measuring those
rules.

Deleting a POI and recreating it under a new OSM ID is deliberately out of
scope; that churn is accepted.

## The rules

`same_place_label` in `stable_ids/poi_tags.py` mirrors the Rust rules for named
places (`UNNAMED_POI_TAGS` is only in Rust, and this tooling doesn't run in
production):

- **Same:** the name is unchanged, respelled (typos, punctuation, added or
  removed words), or switched to one of the place's translations or other
  names, and the brand didn't change.
- **Different:** the name changed entirely, which is how mappers usually
  record a new business reusing the old element, or the name changed and the
  brand switched to a different one (matching `brand:wikidata` counts as the
  same brand).

When the rules change, change both implementations and check they agree: dump
pairs as JSON Lines of `{"a": tags, "b": tags, "same": same_place_label(a, b)}`
and run `RULES_PARITY_FILE=pairs.jsonl cargo test -- --ignored` in
`poi_service`.

## Running

Requires [uv](https://docs.astral.sh/uv/) and about 200 GB of free disk (the
planet is ~95 GB).

```sh
./run_pipeline.sh 261005 21   # planet date (YYMMDD), days of diffs
```

or step by step:

| Step | Command | Output |
| --- | --- | --- |
| Download planet | `./download_planet.sh 261005` | `data/planet-261005.osm.pbf` |
| Extract POIs | `uv run stable-ids-extract` | `data/pois-*.parquet` (with locations) |
| Download diffs | `uv run stable-ids-diffs` | `data/diffs/*.osc.gz`, `data/changes-*.parquet` |
| Build pairs | `uv run stable-ids-pairs` | `data/pairs-*.parquet` |

Each command takes `--help`. Diffs default to
`https://planet.openstreetmap.org/replication/day/`; for a quick test, use a
Geofabrik regional extract with `--server` set to that region's `-updates/` URL.
