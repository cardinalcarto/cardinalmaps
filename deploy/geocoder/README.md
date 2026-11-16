# Geocoder on fly.io

An example of serving Cardinal's geocoder on [fly.io](https://fly.io), from the
geocoder releases `cardinal-publish geocoder` puts in the public bucket (see
`publish/README.md`). `PLAN.md` records why it works this way.

Each machine has a volume, and serves the one release that `fly.toml` pins
(`GEOCODER_RELEASE`). The image, `cardinal-airmail-flyio`, is the `cardinal-airmail`
image of the same commit with `entrypoint.sh` in front of `airmail_service`.
`.github/workflows/images.yml` publishes both. On every start, the entrypoint:

1. Fetches the release's `release.json`. It refuses to start, touching nothing,
   if the release is missing, or if its index schema isn't the one this airmail
   reads (`/etc/airmail/schema-version`, taken from the source when the image is
   built).
2. If the volume holds `geocoder.btrfs` with the release's size and SHA-256,
   uses it. Hashing the planet's 152 GB image takes a while (see "Timing").
3. Otherwise, clears the volume and downloads the image. It resumes a partial
   download of the same release with range requests, and checks the size and
   SHA-256 before the download becomes `geocoder.btrfs`. A mismatch deletes
   the download and exits, so the machine fails its checks rather than serve.
4. Mounts the image read-only (`mount -o loop,ro`), then execs `airmail_service`
   on the `index/`, `categories/` and `tagger.model` inside it. The image stays
   compressed on the volume, and the kernel decompresses pages as airmail reads
   them.

`/health` answers once the index and models are loaded, so fly's proxy sends a
machine nothing until then.

## Adapting it

Edit `fly.toml`:

- `app`: your fly.io app.
- `[build] image`: `ghcr.io/OWNER/cardinal-airmail-flyio:sha-COMMIT`, for a commit on
  `main` that `images.yml` built. fly pulls it without credentials, so the
  `cardinal-airmail-flyio` package on ghcr.io must be public (the package's
  settings, "Change visibility").
- `GEOCODER_RELEASES_URL`: your bucket's public URL, followed by `releases`.
- `GEOCODER_RELEASE`: the release to serve, a date under
  `releases/geocoder/` in the bucket.
- `wait_timeout` and the check's `grace_period`: long enough for a machine to
  download and hash an image on your volumes (see "Timing").
- `[[vm]]`: the machine size. 16 GB leaves room to cache the index around the
  models' 4 GB; searches are CPU-bound, one core each.
- `primary_region`, and the regions you add machines in.

## Deploying

Each machine needs its own volume, large enough for one image plus the next
release's download: about 200 GB for the planet. Create two per region, so
that a rollout always leaves one machine serving:

```sh
fly apps create cardinal-geocoder-example
fly volumes create geocoder_data --app cardinal-geocoder-example --region sea --size 200 --count 2
fly deploy --config deploy/geocoder/fly.toml
fly scale count 2 --region sea --config deploy/geocoder/fly.toml
```

The first start downloads the image, so the first deploy takes as long as
that.

### Moving to another release

Change `GEOCODER_RELEASE` in `fly.toml` and deploy. Commit the change, so git
records which release is served.

The deploy is rolling: one machine at a time restarts, replaces its image, and
must pass `/health` before the next one starts. A machine that fails stops
the deploy, leaving the others on the old release.

Rolling back is the same, with the old date. Because a volume holds only one
release, the old image is downloaded again.

Try a release on a separate staging app before production: search for a few
things with known answers, and check that each comes back.

### Moving to another airmail

Change `[build] image` to a newer commit's `cardinal-airmail-flyio`. It must read the
pinned release's schema, and the entrypoint refuses otherwise.

When a change bumps the schema, change the image and `GEOCODER_RELEASE`
together, to a release built at that commit.

## Timing

A start that finds its image hashes the whole image. A start after a release
change also downloads it. Both are bound by the volume's bandwidth, so measure
it before choosing `wait_timeout` and the grace period. At about 128 MiB/s,
hashing 152 GB takes about 20 minutes.

## Unverified

These have not been tried on fly.io yet:

- loop mounts inside a fly machine;
- a rolling deploy keeping each machine on its own volume;
- the longest `wait_timeout` fly accepts;
- volume bandwidth.

## Testing

```sh
python3 deploy/geocoder/test_entrypoint.py
```

runs the entrypoint against a local HTTP bucket, with `mount` and
`airmail_service` faked. It needs curl and jq. The tests cover:

- an empty volume;
- a matching image;
- another release's image, and clearing the volume;
- resuming a partial download;
- a corrupt download;
- a missing release;
- another schema;
- a malformed date.

To try the image itself, build it on top of a local `cardinal-airmail`. Loop
mounts need a privileged container:

```sh
podman build -t cardinal-airmail airmail
podman build -t cardinal-airmail-flyio -f deploy/geocoder/Dockerfile --build-arg AIRMAIL_IMAGE=cardinal-airmail .
podman run --rm --privileged -p 3000:3000 -v /PATH/TO/VOLUME:/data \
  -e GEOCODER_RELEASE=DATE -e GEOCODER_RELEASES_URL=https://BUCKET.t3.storage.dev/releases \
  cardinal-airmail-flyio
```
