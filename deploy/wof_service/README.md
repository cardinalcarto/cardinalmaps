# Who's On First places on fly.io

An example of serving `wof_service` (see `wof_service/README.md`) on
[fly.io](https://fly.io): the app fetches the Who's On First places of the
regions it downloads from it, to index them on the phone.

Each machine has a volume holding the WOF admin SpatiaLite database
(`whosonfirst-data-admin-*.spatial.db`, about 34 GB). The image,
`cardinal-wof-service`, runs `entrypoint.sh` in front of `wof_service`. On
every start, the entrypoint:

1. Uses the database at `WOF_DB` if it's there and, when `WOF_DB_SHA256` is
   set, a previous download recorded that hash for it (`WOF_DB.sha256`). The
   database is opened read-only and immutable, so it isn't hashed again on
   each start.
2. Otherwise downloads `WOF_DB_URL` (decompressing a `.bz2` as it goes),
   checks its SHA-256 against `WOF_DB_SHA256`, and only then moves it into
   place. A mismatch deletes the download and exits, so the machine fails its
   checks rather than serve the wrong data.

`/health` answers once the database is open.

## Adapting it

Edit `fly.toml`:

- `app`: your fly.io app.
- `[build] image`: `ghcr.io/OWNER/cardinal-wof-service:sha-COMMIT`, for a
  commit on `main` that `images.yml` built. fly pulls it without credentials,
  so the package on ghcr.io must be public.
- `WOF_DB_URL` and `WOF_DB_SHA256`: a copy of the database you control (a
  bucket of yours), and the SHA-256 of the uncompressed file
  (`sha256sum whosonfirst-data-admin-*.spatial.db`). Who's On First's
  "latest" downloads change in place, so don't point machines at one: two
  machines could serve different data, and a restart would replace it.
- `WOF_MAX_AREA`, `WOF_MAX_PLACES`, `WOF_TIMEOUT`: request limits (see
  `wof_service/README.md`). Keep `[[vm]] memory` well above the largest
  response: 1.5 to 3 KB per place uncompressed, so 50,000 places is up to
  150 MB.
- `primary_region`, and the regions you add machines in.

## Deploying

```sh
fly apps create cardinal-wof-example
fly volumes create wof_data --app cardinal-wof-example --region sea --size 50 --count 2
fly deploy --config deploy/wof_service/fly.toml
fly scale count 2 --region sea --config deploy/wof_service/fly.toml
```

The first start downloads the database, so the first deploy takes as long as
that; `wait_timeout` must cover it.

### Moving to another database

Upload the new database, change `WOF_DB_URL` and `WOF_DB_SHA256`, and deploy.
Each machine in turn finds that its file doesn't have the new hash, deletes it,
and downloads the new one. Commit the change, so git records which database
is served. Since the phone stores the places it fetched, a new database only
reaches regions downloaded (or updated) after the change.

## Trying it locally

```sh
cd wof_service
cargo run --release -- --db /path/to/whosonfirst-data-admin-latest.spatial.db
curl -s --compressed 'http://127.0.0.1:3000/v1/places?bbox=-122.44,47.49,-122.23,47.74' | head -c 600
```
