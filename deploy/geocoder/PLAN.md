# Plan: the geocoder on fly.io

Status: planned, not started. This records what was decided, so the work can pick
up later. Like the rest of `deploy/`, it describes an example deployment: app
names, buckets and hostnames are placeholders.

## What runs

- **One process: `airmail_service`** (`airmail/Dockerfile`), port 3000. The app
  uses only `GET /v1/search` and `GET /v1/place`. `GET /health` answers only once
  the index has loaded, so it doubles as the readiness check.
- **At startup** it memory-maps `index/` (tantivy) and reads `categories/*.amcats`
  (65 languages, ~3 GB) and `tagger.model` (~0.1 GB) into memory. At query time
  it only pages in index files.
- **Not needed to serve:** Postgres, the WOF databases, `poi_service`, and the
  POI updater. Those are build-time inputs only.
- **Data per release:** one image, `geocoder.btrfs`. The planet image of
  2026-11-15 is 152 GB. It holds 197 files, among them a 147 GB index, as
  97.9 GiB of zstd data inside a 141 GiB filesystem (see "Open items").
- **Memory:** about 4 GB is a hard floor (mostly the category models); 16 GB
  leaves room to cache the index.
- **CPU:** every query is single-threaded and CPU-heavy. The dev index took
  63–475 ms at the median; planet latency has never been measured.

## Decisions

1. **One image per release.** `cardinal-publish geocoder` (done) publishes
   `releases/geocoder/DATE/geocoder.btrfs`, the read-only btrfs image that
   `data_pipeline`'s `build` makes: the index (`index/`), the categorical
   models (`categories/`) and the query tagger (`tagger.model`), compressed
   with zstd. It goes up as a single multipart upload, and `release.json`
   beside it records the image's size and SHA-256 and the index's schema
   version. A published release never changes, so a date names exactly one
   image.
2. **The release is pinned in `fly.toml`.** `GEOCODER_RELEASE = "DATE"` in
   `[env]`, committed, so git records which index production serves.
   Changing the pin and running `fly deploy` is the only way to move a
   machine to another release.
3. **The entrypoint makes the volume match the pin, then mounts it.** On every
   start:
   1. Fetch `releases/geocoder/$GEOCODER_RELEASE/release.json`. Refuse to
      start if it's missing or its schema version isn't the one this
      airmail reads.
   2. **Hash check:** if the volume holds `geocoder.btrfs`, hash it and
      compare its size and SHA-256 with `release.json`. On a match, go to
      step 5.
   3. **Clear the volume:** delete everything on it except a partial
      download of this same release (named after the release, so it can be
      told apart).
   4. Download the image, resuming any partial download with range
      requests, check its size and SHA-256 against `release.json`, and only
      then move it to `geocoder.btrfs`. A mismatch deletes the download and
      exits non-zero, so the machine fails its checks instead of serving.
   5. Mount it read-only (`mount -o loop,ro`) and exec airmail on its
      `index/`, `categories/` and `tagger.model`. The image stays compressed
      on the volume, and the kernel decompresses pages as airmail maps them.
4. **Rollouts are fly's rolling deploy, in place.** Machines with volumes can
   only use the `rolling` strategy (`canary` and `bluegreen` refuse them), and
   a deploy can't move a machine to a different volume. So each machine
   restarts on its own volume, one at a time. When the pin changed, the
   entrypoint clears the volume and refills it from the bucket, and the
   machine is out of rotation until `/health` passes. fly waits for each
   machine's checks before starting the next, and a failing machine stops
   the deploy, leaving the rest on the old release. Consequences:
   - **No downtime needs at least two machines** in each region that serves
     traffic. With one, a rollout is an outage lasting the refill.
   - **The deploy's wait has to outlast a refill:** `--wait-timeout` and the
     health check's grace period sized from measured volume bandwidth.
   - **Rollback is the same:** pin the old date and deploy. It downloads the
     old image again, since the volume holds one release.
   - **Volumes hold one release**, about 200 GB for the 152 GB image plus
     room for the partial download that replaces it (which only exists after
     the old image is deleted).
5. **Tiles and geocoder release independently.** POI IDs are stable
   (`poi_service`), so there's no pairing check between map and geocoder
   releases. A `/v1/place` 404 says what became of an ID (active, inactive,
   replaced, unknown) from a status file built from the published POI tables
   (`releases/poi/`, see `publish/README.md`). The status file is about 1 GB;
   refreshing it daily replaces it by rename, with no restart
   (`AIRMAIL_POI_STATUS`).
6. **Load balancing is fly's proxy.** Requests go to the nearest region with a
   healthy machine, and within a region by request concurrency limits. Airmail
   is CPU-bound, so the limits should be low (start near soft 8 / hard 32).
   Auto-stop stays off: every start hashes the whole image (decision 3).

### What could take production down, and the safeguard for each

| Failure | Safeguard |
|---|---|
| A corrupt or wrong image on a volume | The hash check against `release.json` on every start; airmail never sees an image that doesn't match. |
| A pin naming a release that doesn't exist, or another schema | The entrypoint refuses before touching the volume, so the old image stays and the rolling deploy stops at the first machine. |
| A bad index that passes `/health` | Every new release goes to a separate staging app first, and is checked there with smoke queries with known answers (a landmark's `poi:` ID, an address's city) before production's pin changes. |
| All machines refilling at once | `rolling` only, never `immediate`; at least two machines per region. |
| A refill longer than the deploy waits | The wait timeout and grace period set from measured bandwidth; if exceeded, the deploy stops with the remaining machines still on the old release. |
| An interrupted download | Resumed by range requests on the next start; only a hash-checked download becomes `geocoder.btrfs`. |

## Work, roughly 2 days

| Piece | Effort |
|---|---|
| Measure first: the planet geocoder in a container limited to 16 GB and 2 CPUs, replaying `airmail/eval/search.tsv` with the app's parameters (prefix and typos on); p50/p99 and queries per second. This sets the machine size. | ½ day |
| `deploy/geocoder/`: image, entrypoint (decision 3) with tests against a local bucket (`--target dir:`-style fixtures: match, mismatch, partial download, missing release, wrong schema), example `fly.toml`, smoke-query script, README with "Adapting it" | 1 day |
| airmail: graceful shutdown on SIGTERM, the release ID in `/health`. Optional: response compression, a limit on concurrent searches. | ½ day |

`cardinal-publish geocoder` is already done.

## Starting point

- **Sizing:** `performance-2x` with 16 GB, auto-stop off, about $180 a month
  per machine plus the volume (from memory of fly's prices; check them). Two
  machines per region, for rollouts without downtime.
- **Volumes:** about 200 GB each. They live on one host and don't replicate,
  so a failed host means a new machine fills from the bucket, which takes as
  long as a rollout's refill.

## Open items

- **A search failure to check first:** a search against the local planet
  geocoder on :3000 failed with "Connection closed before full header was
  received" (2026-11-12). Not investigated.
- **Startup time:** hashing 152 GB on every start takes about 20 minutes at
  ~128 MiB/s of volume bandwidth, and hours at ~16 MiB/s; a refill adds the
  download. Measure both on a real volume before choosing timeouts.
- **Image slack:** `mkfs.btrfs --shrink` left a 141 GiB filesystem for
  97.9 GiB of data. Shrinking it to fit would cut downloads, hashing and
  volume size by about a third.
- **Fly facts to verify** before relying on them: that a rolling deploy keeps
  each machine's volume (the docs imply it but don't say), that loop mounts
  work in fly machines, the volume size limit, volume bandwidth, the longest
  wait `fly deploy` allows, that the proxy doesn't route to machines failing
  their checks, and current prices.
- **Licensing:** a public index is a derived database, with ODbL obligations
  for OSM and WOF source licences. Decide how many releases to keep.
