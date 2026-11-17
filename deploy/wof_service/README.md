# Who's On First places on fly.io

An example of serving `wof_service` (see `wof_service/README.md`) on
[fly.io](https://fly.io): the app fetches the Who's On First places of the
regions it downloads from it, to index them on the phone. It serves the wof
releases `cardinal-publish wof` puts in the public bucket (see
`publish/README.md`), the way `deploy/geocoder` serves geocoder releases.

Each machine has a volume, and serves the one release that `fly.toml` pins
(`WOF_RELEASE`): the WOF admin SpatiaLite database, about 34 GB. The image,
`cardinal-wof-service-flyio`, is the `cardinal-wof-service` image of the same
commit with `entrypoint.sh` in front of `wof_service`.
`.github/workflows/images.yml` publishes both. On every start, the entrypoint:

1. Fetches the release's `release.json`. It refuses to start, touching
   nothing, if the release is missing or isn't a wof release of that date.
2. Before anything else uses the database on the volume, deletes it unless its
   sidecar (`DB.sha256`) holds the release's SHA-256 and its size is the
   release's. The sidecar is written only after a download's size and SHA-256
   were checked, so a database left by an interrupted or unchecked download,
   or another release's, is never served. Unlike the geocoder's 152 GB image,
   the database isn't hashed on every start: the size and sidecar vouch for it,
   so starts are quick and machines can stop when idle.
3. If the database is still there, serves it.
4. Otherwise, clears the volume (keeping a partial download of the same
   release), checks there's room, and downloads the database, uncompressed.
5. Checks the download's size and SHA-256, and only then moves it into place
   and writes the sidecar.

`/health` answers once the database is open, so fly's proxy sends a machine
nothing until then.

### Interrupted downloads

A dropped connection or server error doesn't restart the machine. The
entrypoint retries within the same start, resuming the partial download with
a range request each time, waiting 5 seconds, then twice as long after each
failure, up to 5 minutes (`WOF_RETRY_DELAY`, `WOF_RETRY_MAX_DELAY`). Each
attempt is logged with the bytes downloaded so far and curl's error. After 30
attempts (`WOF_DOWNLOAD_ATTEMPTS`, about two hours) it exits non-zero with a
"gave up downloading" line; the partial stays on the volume and the next start
resumes it.

A complete download that fails its SHA-256 check is deleted and logged in
capitals, and downloaded once more from scratch (the partial may have come from
an earlier broken attempt). A second mismatch exits non-zero rather than
download 34 GB again and again: the bucket's file doesn't match its
`release.json`.

The other failures that retrying can't fix also exit non-zero at once, each
with one line saying which: no `release.json` (or another kind or date), not
enough room on the volume, or the bucket refusing the file (a 4xx answer).
fly then restarts the machine, which fails the same way until the cause is
fixed, so look at `fly logs` when a deploy stalls.

## Publishing a release

On the machine that holds the database (uncompressed; `bunzip2` a `.bz2`
download first):

```sh
cd publish
uv run cardinal-publish wof --target s3://BUCKET --access-key-id ID --secret-access-key KEY \
  --database /path/to/whosonfirst-data-admin-latest.spatial.db --date 2026-11-17 \
  --source whosonfirst-data-admin-latest
```

uploads it as `releases/wof/DATE/whosonfirst-data-admin.spatial.db`, then
`release.json` with its size and SHA-256. Pick the date you downloaded it from
Who's On First: their "latest" file changes in place, and a release mustn't.
`latest.json` doesn't name wof releases; servers pin one here.

## Adapting it

Edit `fly.toml`:

- `app`: your fly.io app.
- `[build] image`: `ghcr.io/OWNER/cardinal-wof-service-flyio:sha-COMMIT`, for
  a commit on `main` that `images.yml` built. fly pulls it without
  credentials, so the `cardinal-wof-service-flyio` package on ghcr.io must be
  public.
- `WOF_RELEASES_URL`: your bucket's public URL, followed by `releases`.
- `WOF_RELEASE`: the release to serve, a date under `releases/wof/` in the
  bucket.
- `WOF_MAX_AREA`, `WOF_MAX_PLACES`, `WOF_TIMEOUT`: request limits (see
  `wof_service/README.md`). Keep `[[vm]] memory` well above the largest
  response: 1.5 to 3 KB per place uncompressed, so 50,000 places is up to
  150 MB.
- `wait_timeout`: long enough for a machine to download and hash the database
  on your volumes, with retries.
- `primary_region`, and the regions you add machines in.

## Deploying

Each machine needs its own volume, large enough for one database (the old one
is deleted before the next downloads): 50 GB leaves room to grow.

```sh
fly apps create cardinal-wof-example
fly volumes create wof_data --app cardinal-wof-example --region sea --size 50 --count 2
fly deploy --config deploy/wof_service/fly.toml
fly scale count 2 --region sea --config deploy/wof_service/fly.toml
```

The first start downloads the database, so the first deploy takes as long as
that; `wait_timeout` must cover it.

### Moving to another release

Publish the new database as a new release, change `WOF_RELEASE` in `fly.toml`,
and deploy. Commit the change, so git records which release is served.

The deploy is rolling: one machine at a time restarts, deletes its database
(its sidecar holds the old release's hash), downloads the new one, and must
pass `/health` before the next one starts. A machine that fails stops the
deploy, leaving the others on the old release. Rolling back is the same with
the old date, and downloads the old database again.

Since the phone stores the places it fetched, a new release only reaches
regions downloaded (or updated) after the change.

## Testing

```sh
python3 deploy/wof_service/test_entrypoint.py
```

runs the entrypoint against a local HTTP bucket, with `wof_service` faked. It
needs curl and jq. The tests cover an empty volume; a recorded database (not
downloaded or hashed again); a database without a sidecar, with another
release's, or truncated, and a sidecar without a database (each deleted
first); resuming a partial download; dropped connections and server errors
retried; giving up after the attempts, and the next start resuming; a missing
file; a corrupt download retried once, and giving up on a second; a missing
release, another kind, and a malformed date (the volume untouched); and a
volume without room.

To try the image itself, build it on top of a local `cardinal-wof-service`:

```sh
podman build -t cardinal-wof-service wof_service
podman build -t cardinal-wof-service-flyio -f deploy/wof_service/Dockerfile \
  --build-arg WOF_IMAGE=cardinal-wof-service .
podman run --rm -p 3000:3000 -v /PATH/TO/VOLUME:/data \
  -e WOF_RELEASE=DATE -e WOF_RELEASES_URL=https://BUCKET.t3.storage.dev/releases \
  cardinal-wof-service-flyio
curl -s --compressed 'http://127.0.0.1:3000/v1/places?bbox=-122.44,47.49,-122.23,47.74' | head -c 600
```

A local directory written by `cardinal-publish wof --target dir:/PATH`,
served over HTTP, works as `WOF_RELEASES_URL` too.
