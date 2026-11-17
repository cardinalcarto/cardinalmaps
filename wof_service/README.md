# wof_service

Serves the [Who's On First](https://whosonfirst.org) places in a bounding box,
with everything the airmail indexer reads of each, so the phone can index the
admin areas of the map region it downloaded (see the offline geocoder plan,
contract 3). It reads the WOF admin SpatiaLite database
(`whosonfirst-data-admin-latest.spatial.db`) with plain SQLite: no
mod_spatialite. Geometries are parsed from SpatiaLite's blob format in
`src/spatialite.rs`.

```sh
cargo run --release -- --db /path/to/whosonfirst-data-admin-latest.spatial.db
```

Options (also as environment variables): `--db` (`WOF_DB`), `--bind`
(`WOF_BIND`, default `127.0.0.1:3000`), and the limits below. The database is
opened immutable: nothing may write to it while the service runs.

`deploy/wof_service` is an example deployment on fly.io.

## API

### `GET /health`

`ok` once the database is open and has the tables and R-trees queries use.

### `GET /v1/places?bbox=west,south,east,north`

Every place the airmail indexer would index (`airmail_indexer/src/load_wof.rs`)
whose indexed shape meets the box, and every ancestor of one.

- The indexer's rules: not a `planet`, `timezone`, `marketarea` or `campus`;
  at least one name that isn't an abbreviation; and a shape, which is its
  `boundary` if it has one, else its label point.
- A boundary meets the box if it intersects it, tested exactly against the
  database's `shard` pieces of it. A place without a boundary is in the box if
  its label point is.
- Ancestors (the `hierarchy` table, every branch) that aren't in the box
  themselves are included with `"ancestor_only": true` and no geometry, so
  the client can always resolve hierarchy names, populations and spoken
  languages.

`west` may be greater than `east`, for a box across the antimeridian.

Optional parameters:

- `tolerance`: simplification tolerance relative to each shape's size, from
  `0.00001` to `0.05`; default `0.001`. See "Simplification".
- `max_vertices`: the most vertices of each shape, from 16 to 100,000;
  default 1000.

Responses are JSON, compressed with gzip or zstd per `Accept-Encoding`, with
`Cache-Control: public, max-age=86400`. Places are sorted by ID:

```json
{
  "bbox": [-122.3215, 47.6145, -122.3205, 47.6155],
  "tolerance": 0.001,
  "max_vertices": 1000,
  "places": [
    {
      "id": 85882415,
      "placetype": "neighbourhood",
      "names": [["eng", "preferred", false, "Capitol Hill"], ["ell", "variant", false, "Λόφος του Καπιτωλίου"],
                ["und", "default", false, "Capitol Hill"], ...],
      "ancestors": [101730401, 102086191, 85688623, 85633793, 102191575],
      "country_id": 85633793,
      "country": "US",
      "population": 3701,
      "label": [-122.31496, 47.624761],
      "bbox": [-122.330603, 47.612764, -122.292436, 47.639717],
      "geometry": {"type": "Polygon", "coordinates": [[[-122.322881, 47.639717], ...]]}
    },
    {
      "id": 102191583, "placetype": "continent", "names": [...], "ancestors": [...],
      "label": [...], "bbox": [...], "geometry": null, "ancestor_only": true
    },
    ...
  ]
}
```

Each place:

| Field | |
| --- | --- |
| `id` | WOF ID |
| `placetype` | WOF placetype |
| `names` | every name, as `[lang, tag, abbr, name]`: `lang` is ISO 639-3 or `und`; `tag` is `default`, `preferred`, `variant` or `colloquial`; `abbr` whether it's an abbreviation. The client filters (the indexer searches every tag but `colloquial`, and keeps variants apart) |
| `ancestors` | IDs of the places containing it, every branch, not itself, in the database's order |
| `country_id` | the first of itself and its ancestors that's a country (the indexer's `COUNTRIES` list); its `lang_x_spoken` are the languages the indexer adds ancestor names in. Absent if none |
| `country` | `alpha2`, else `wof:country`, as stored (the indexer normalizes it with `iso_country_code`). Absent if neither |
| `population`, `population_rank` | `wof:population` (truncated to an integer) and `wof:population_rank`, if present |
| `lang_x_spoken` | `wof:lang_x_spoken`, split on commas, if present (countries) |
| `label` | `[lon, lat]`: the first of its `label_position`, `reversegeo_position` and `centroid`, as the indexer chooses. Absent if it has none |
| `bbox` | `[west, south, east, north]` of its unsimplified boundary, else of its label point |
| `geometry` | its boundary, simplified, as a GeoJSON geometry (Polygon or MultiPolygon); `null` for a place that's only a point, and for `ancestor_only` places |
| `ancestor_only` | `true` on ancestors that aren't in the box; absent otherwise |

Errors are `{"error": "..."}` with 400 for a malformed request, 413 for one
over a limit, and 500 for a database failure.

## Limits

Requests are bounded so a huge bbox fails fast rather than tying up a machine
or its memory:

| Limit | Default | Option |
| --- | --- | --- |
| bbox area | 100 square degrees | `--max-area`, `WOF_MAX_AREA` |
| places in a response, ancestors included | 50,000 | `--max-places`, `WOF_MAX_PLACES` |
| time per request | 30 s | `--timeout`, `WOF_TIMEOUT` |
| requests answered at once (the rest wait) | CPU count | `--concurrency`, `WOF_CONCURRENCY` |

Each fails with 413. A Washington-state bbox is 28 square degrees and 4,706
places; a 10° by 10° box over France and Germany is 184,765 places (493 MB of
JSON) and fails after 16 s, so a client covering a large region should split it into
boxes of a few degrees.

## Simplification

The tolerance is relative: a fraction of the larger side of each shape's
bounding box, applied with Douglas-Peucker. A fixed tolerance in degrees can't
suit both a microhood a few hundred metres across and a country, while a
relative one keeps every shape equally recognizable when it fills the screen:
at the default of 1/1000, a place drawn 1000 pixels wide is off by at most a
pixel. For containment the error grows with the place: about 25 m on
Seattle's boundary, a few metres for its neighbourhoods, under a kilometre for
Washington state. Rings (islands, holes) smaller than the tolerance are
dropped. Coordinates are rounded to 6 decimals (about 10 cm).

Shapes that still have more than `max_vertices` vertices (default 1000, the
same cap as `airmail::geometry::MAX_VERTICES`, which the phone's index
applies anyway) are simplified with doubling tolerances until they fit, and
then lose their smallest parts. In practice that's only continents, empires
and large countries: in a Washington bbox, the median shape has 81 vertices
and the 95th percentile 283.

## Performance

Measured with curl on the 2026-11 planet database (34 GB) on a local NVMe
disk. "First" is the first request for the box after the service started, with
the database's pages partly cached; the others are repeats. Compressing adds
the difference between the columns.

| bbox | places | JSON | gzip | zstd | first | repeat | repeat, gzip | repeat, zstd |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Seattle `-122.44,47.49,-122.23,47.74` | 231 | 667 KB | 175 KB | 163 KB | 245 ms | 27 ms | 50 ms | 38 ms |
| Washington `-124.85,45.54,-116.91,49.01` | 4,706 | 6.9 MB | 1.6 MB | 1.6 MB | 2.6 s | 255 ms | 430 ms | 330 ms |
| Lyon `4.79,45.70,4.90,45.81` | 109 | 435 KB | 100 KB | 77 KB | 118 ms | 16 ms | 23 ms | 17 ms |

The Washington response holds every place under Washington in the hierarchy
(3,912), plus parts of Oregon, Idaho and British Columbia.

## Tests

`cargo test`: unit tests of the blob parser, simplification and bbox parsing,
and `tests/api.rs`, which builds a tiny database with the real one's tables
and R-trees and checks the API against it.
