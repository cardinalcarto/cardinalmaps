# PMTiles accelerator

An example of serving Cardinal's published map (see `publish/`) close to its
users. The map is one PMTiles archive in a public bucket; a client could read
it with HTTP range requests, but each tile then costs a directory lookup and a
round trip to the bucket. [`pmtiles serve`](https://docs.protomaps.com/deploy/server)
reads the archive's directories once, caches them, and answers plain z/x/y
requests with TileJSON for any MapLibre client, such as the app's renderer.

`fly.toml` runs the pinned go-pmtiles 1.31.2 image on [fly.io](https://fly.io)
in a few regions. For a release, it serves:

| Path | |
| --- | --- |
| `/planet.json` | TileJSON 3.0.0, with tile URLs under `--public-url` |
| `/planet/{z}/{x}/{y}.mlt` | gzipped MLT tiles (`Content-Encoding: gzip`), 204 where there is no tile |

go-pmtiles adds `vector_layers` to TileJSON only for MVT archives, so this
TileJSON has none; styles name their layers directly, so MapLibre doesn't need
it.

## Adapting it

Edit `fly.toml`:

- `app`, and the same name in `--public-url`: your fly.io app (or the public
  hostname in front of it).
- `--bucket`: your bucket's public URL, followed by the map release's prefix,
  `releases/map/DATE`. Each new map release needs this changed and the app
  deployed again; `latest.json` isn't read here.
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

With a map release in a directory (`cardinal-publish map --target dir:/PATH`)
or a converted `planet.pmtiles` anywhere (on an SELinux host, add `,z` to the
volume's options):

```sh
podman run --rm -p 8080:8080 -v /PATH/releases/map/DATE:/data:ro \
  docker.io/protomaps/go-pmtiles:v1.31.2@sha256:06574f01f55a78f78f887bc7ebf729a5c093c0d6e17d9876300cfcb0758b59d3 \
  serve /data --public-url=http://localhost:8080 '--cors=*'
curl http://localhost:8080/planet.json
```
