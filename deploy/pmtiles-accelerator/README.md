# PMTiles accelerator

An example of serving Cardinal's published map (see `publish/`) close to its
users. The map is one PMTiles archive in a public bucket; a client could read
it with HTTP range requests, but each tile then costs a directory lookup and a
round trip to the bucket. [`pmtiles serve`](https://docs.protomaps.com/deploy/server)
reads the archive's directories once, caches them, and answers plain z/x/y
requests with TileJSON for any MapLibre client, such as the app's renderer.

`fly.toml` runs the pinned go-pmtiles 1.31.2 image on [fly.io](https://fly.io)
in a few regions. It serves every map release in the bucket, each under its
date:

| Path | |
| --- | --- |
| `/DATE/planet.json` | TileJSON 3.0.0, with tile URLs under `--public-url` |
| `/DATE/planet/{z}/{x}/{y}.mlt` | gzipped MLT tiles (`Content-Encoding: gzip`), 204 where there is no tile |

A release that isn't in the bucket answers 404.

Serving every release, not just the latest, is deliberate. A client pins one
map release and asks for its tiles until it moves to another all at once, so a
cache never mixes tiles of two releases (which shows as seams at tile edges),
and a downloaded offline basemap's missing tiles come from the same release.
Since each release's tile URLs include its date, a tile URL's contents never
change. A release can be removed from the bucket only once no client still uses
it.

go-pmtiles adds `vector_layers` to TileJSON only for MVT archives, so this
TileJSON has none; styles name their layers directly, so MapLibre doesn't need
it.

## Adapting it

Edit `fly.toml`:

- `app`, and the same name in `--public-url`: your fly.io app (or the public
  hostname in front of it).
- `--bucket`: your bucket's public URL, followed by `releases/map`. New map
  releases are served as soon as they're published, with no deploy;
  `latest.json` isn't read here.
- The health check's `path`: the TileJSON of a release that stays in the bucket.
  Move it to a newer release before deleting the one it names.
- `--cors`: the origins allowed to fetch tiles from a browser (`*` allows any).
- `--cache-size`: megabytes of directories and tiles kept in memory; keep it
  below the machine's memory.
- `primary_region` and the regions below: where your users are.

Then deploy once, and add a machine in each other region:

```sh
fly launch --copy-config --no-deploy   # or: fly apps create cardinal-tiles-example
fly deploy
fly scale count 5 --region sea,iad,fra,sin,syd
```

## Trying it locally

With map releases in a directory (`cardinal-publish map --target dir:/PATH`),
or converted archives laid out as `releases/map/DATE/planet.pmtiles` (on an
SELinux host, add `,z` to the volume's options):

```sh
podman run --rm -p 8080:8080 -v /PATH:/data:ro \
  docker.io/protomaps/go-pmtiles:v1.31.2@sha256:06574f01f55a78f78f887bc7ebf729a5c093c0d6e17d9876300cfcb0758b59d3 \
  serve / --bucket=file:///data/releases/map --public-url=http://localhost:8080 '--cors=*'
curl http://localhost:8080/DATE/planet.json
```
