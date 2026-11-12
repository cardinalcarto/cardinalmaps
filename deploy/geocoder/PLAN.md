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
- **Data per release:** about 160 GB on disk. The index is ~156 GB, in 143 files.
- **Memory:** about 4 GB is a hard floor (mostly the category models); 16 GB
  leaves room to cache the index.
- **CPU:** every query is single-threaded and CPU-heavy. The dev index took
  63–475 ms at the median; planet latency has never been measured.

## Decisions

1. **One archive per release.** Publish `releases/geocoder/DATE/geocoder.tar.zst`
   (index, categories, tagger, and the release's `manifest.json`) as a single
   multipart upload. `release.json` beside it records the archive's size and
   SHA-256. Samples measured with zstd -3 put the archive at about 110 GB
   (`.store` 80%, `.idx` 79%, `.term` 44%). Per-file objects were considered
   and rejected as more complexity than they're worth. The cost: a failed
   download restarts from the beginning, once per release.
2. **Pulled once per release, not on every start.** The index lives on a fly
   volume. A machine's entrypoint checks whether the volume holds the release
   named by `GEOCODER_RELEASE`. If it doesn't, it streams the archive straight
   onto the volume (`curl | tee >(sha256sum) | zstd -d | tar -x`, nothing
   stored twice), checks the hash, and only then starts airmail. A restart reuses
   what's there.
3. **Blue/green machines, no A/B slots.** Volumes are the main cost, so a
   machine's volume holds one release (~200 GB). A new release gets a new
   machine with a new volume; the old one is stopped once the new one is
   proven, and destroyed later. Two volumes exist only during a rollout and its
   grace period.
4. **Tiles and geocoder release independently.** POI IDs are stable
   (`poi_service`), so there's no pairing check between map and geocoder
   releases. A `/v1/place` 404 says what became of an ID (active, inactive,
   replaced, unknown) from a status file built from the published POI tables
   (`releases/poi/`, see `publish/README.md`). The status file is about 1 GB;
   refreshing it daily replaces it by rename, with no restart
   (`AIRMAIL_POI_STATUS`).
5. **Load balancing is fly's proxy.** Requests go to the nearest region with a
   healthy machine, and within a region by request concurrency limits. Airmail
   is CPU-bound, so the limits should be low (start near soft 8 / hard 32).
   Auto-stop stays off: a cold start re-reads 3 GB of models through the
   volume's bandwidth cap.

## The rollout script

`deploy/geocoder/rollout` (Python, calling `flyctl … --json`). Every step reads
its state from fly (machines tagged with metadata `release=DATE`), never from
local files, so each step can be rerun after a crash.

| Step | What it does |
|---|---|
| `plan --app APP --release DATE` | Read-only: what's running, and exactly which machine IDs each step would touch |
| `start` | Checks that `releases/geocoder/DATE/release.json` exists, creates a volume for the release, and clones the current machine's config with `GEOCODER_RELEASE=DATE`. Touches nothing old. |
| `verify` | On the NEW machine only: `/health` reports release DATE, and smoke queries return known answers (e.g. a landmark's `poi:` ID, an address's city). An unknown or timed-out answer counts as failure. |
| `promote` | Refuses unless `verify` passes at that moment. Then **stops** the old machines one at a time, re-checking before each that a healthy machine on DATE remains. |
| `rollback` | Starts the stopped old machines again, then destroys the new one. |
| `cleanup` | Run by hand a day or so later: destroys the stopped old machines and their volumes. |

### What could take production down, and the safeguard for each

| Failure | Safeguard |
|---|---|
| Removing the old machine before the new one works | `promote` requires a passing `verify` at that moment, and only stops machines; destroying happens in `cleanup`. |
| Selecting the wrong machines | Machines are selected by their `release` tag only. Hard rule: never stop or destroy the last healthy machine. `plan` prints the exact IDs. |
| The wrong app | `--app` is required, with no default. Changing steps print the plan and require typing the app name to confirm. |
| A bad index that passes `/health` | Smoke queries with known answers, and the old machine kept stopped until `cleanup`. |
| A crash partway through | State is read from fly, so a rerun picks up where it stopped. |
| Two rollouts at once | Steps refuse if machines from more than two releases exist. |
| fly API errors | Timeouts and retries; an unknown status counts as unhealthy. |
| Bugs in the script | Every rollout goes to a separate staging app first (one small machine with a Seattle-sized index). Tests use a fake flyctl returning recorded JSON. |

## Work, roughly 2½–3 days

| Piece | Effort |
|---|---|
| Measure first: the planet geocoder in a container limited to 16 GB and 2 CPUs, replaying `airmail/eval/search.tsv` with the app's parameters (prefix and typos on); p50/p99 and queries per second. This sets the machine size. | ½ day |
| `cardinal-publish geocoder`: tar, zstd and one multipart upload, plus `release.json` with the SHA-256. Credentials on the command line, like the other steps. | ½ day |
| `deploy/geocoder/`: image, entrypoint, example `fly.toml`, the rollout script with its tests, README with "Adapting it" | 1 day |
| airmail: graceful shutdown on SIGTERM, the release ID in `/health`. Optional: response compression, a limit on concurrent searches. | ½ day |

## Starting point

- **Sizing:** `performance-2x` with 16 GB, one region, auto-stop off, about $180
  a month per machine plus the volume (from memory of fly's prices; check
  them). Two machines in a region for high availability, if wanted.
- **Volumes:** about 200 GB each, for one release plus headroom. They live on
  one host and don't replicate, so a failed host means a new machine fills from
  the bucket, which takes hours.

## Open items

- **A search failure to check first:** a search against the local planet
  geocoder on :3000 failed with "Connection closed before full header was
  received" (2026-11-12). Not investigated.
- **Fly facts to verify** before relying on them: the volume size limit, volume
  bandwidth caps (they set fill time: ~20 min at ~128 MiB/s, hours at
  ~16 MiB/s), that the proxy doesn't route to machines failing their checks,
  `fly machine clone` flags, and current prices.
- **Licensing:** a public index is a derived database, with ODbL obligations
  for OSM and WOF source licences. Decide how many releases to keep.
