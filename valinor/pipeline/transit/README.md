# Offline transit pipeline: feed fetching

This stage produces the cleaned GTFS feeds that the rest of the transit pipeline compiles.
It runs [Transitous](https://github.com/public-transport/transitous)'s own `src/fetch.py` and `gtfsclean`,
at pinned commits, inside a container built from the [Containerfile](Containerfile).

## Running it

```bash
valinor/pipeline/transit/run.sh --output DIR --region us-wa [--cache DIR] [--jobs N]
```

`run.sh` builds the image and runs `fetch_feeds.py` in it.
The container gets the network, the output directory, and the cache directory, and nothing else.
To run the stage elsewhere, such as in Dagger, invoke the image's entrypoint directly:

```bash
fetch_feeds.py --input /opt/transitous/feeds --output DIR --region us-wa --cache DIR
```

Regions are named one by one (`--region` is repeatable).
There is deliberately no way to ask for all of them.

Downloaded feeds never go in git.

## Pins

The Transitous and `gtfsclean` commits are the two `ARG`s at the top of the Containerfile.
The `transitland-atlas` submodule follows from the Transitous commit.
Moving the Transitous pin may require updating the files in `patches/`, which `git apply` checks at build time.
Python dependencies are pinned in `constraints.txt`.

## What differs from Transitous

The patches in `patches/` change two things, and no others.

- Requests identify Cardinal Maps instead of Transitous.
- The fallback to `gtfsproxy.fale.io` for Transitland Atlas sources is removed.
  It's a proxy hard-coded in Transitous's code, which we treat as Transitous's infrastructure.
  The Mobility Database's own `urls.latest` fallback stays.

Sources that need Transitous's encrypted keys or `use-feed-proxy` are skipped.

## Excluded agencies

[`excluded-sources.txt`](excluded-sources.txt) lists Transitous source names to never fetch.
It is an exact, reviewable list, applied before anything is downloaded.
Excluded sources appear in `skipped` with the reason "excluded".

## Output

In the output directory:

- `feeds/<feed-id>.gtfs.zip`: the cleaned GTFS feed, with `<feed-id>` being `<region>-<source name>`.
- `feeds.json`, sorted by ID:

```json
{
  "transitous_commit": "...",
  "fetched_at": "2026-11-09T00:00:00Z",
  "feeds": [{"id": "...", "region": "...", "name": "...", "file": "feeds/....gtfs.zip",
             "sha256": "...", "bytes": 0, "license": {}, "source": {}}],
  "skipped": [{"id": "...", "reason": "..."}],
  "realtime": [{"id": "...", "name": "...", "url": "..."}]
}
```

`license` holds the license fields of the region file, the Transitland Atlas, and the Mobility Database,
copied verbatim under their own names.
Nothing here interprets them.
`source` is the source entry from the region file, verbatim.
It includes options that `fetch.py` doesn't act on, such as `extend-calendar` and `default-timezone`,
which Transitous applies when importing into MOTIS.

One ID can appear in several lists, since a feed's static and real-time sources share a name.
A feed that fails to download or clean is listed in `skipped` and doesn't stop the others.
The stage exits non-zero when no feed at all succeeds, or when a source resolver is unavailable.

## Running it again

Point `--cache` at the same directory to reuse earlier work (the default is `<output>/.cache`).
For each source, `fetch.py` sends a HEAD request and skips the download when the server's `Last-Modified` isn't newer,
or downloads and compares the bytes when the server doesn't send one.
Feeds with `fetch-interval-days` aren't requested within that interval.
`gtfsclean` is rerun only for changed downloads and for feeds that have no cleaned output yet.
The Mobility Database export is fetched fresh on every run.
Cleaned feeds that are reused aren't checked again for expiry; only a new download is.
`feeds/` and `feeds.json` are rebuilt on every run, so feeds that now fail don't linger.

## Publishing

`feeds` and `feeds.json` are symbolic links through `.current`, which names a directory in `.generations/`.
Each run builds a whole new generation and switches `.current` with one rename,
so a run killed at any point leaves a listing that matches its archives.
Archives are copies, so the cleaned feeds take up space in both the cache and the output.
An output directory with a real `feeds/` or `feeds.json` is refused, rather than replaced.

When a source resolver is unavailable, such as the Mobility Database export not loading,
or when no feed succeeds and earlier feeds exist, the stage exits non-zero and leaves the output as it was.

## Tests

```bash
python3 valinor/pipeline/transit/test_fetch_feeds.py
```
