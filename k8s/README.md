# Example deployment

This directory is an example of hosting Cardinal's backend: a tileserver and
a geocoder serving data built by `data_pipeline/`, and the POI database that
gives POIs their stable identifiers. Adapt it to your own cluster; nothing
else in the project depends on how it's deployed.

It assumes:

- a single-node [k3s](https://k3s.io/) cluster, for its Helm controller and
  `local-path` storage;
- the [Tailscale Kubernetes operator](https://tailscale.com/kb/1236/kubernetes-operator)
  with Funnel, to expose the services publicly;
- that data builds run on the node itself, so they can write releases straight
  into the volume the services read.

Everything runs in the `cardinal` namespace, behind one public hostname:

| Path | Service |
| --- | --- |
| `/tiles/basemap/{z}/{x}/{y}` | [Martin](https://maplibre.org/martin/) serving the release's `tiles.mbtiles` (TileJSON at `/tiles/basemap`) |
| `/geocoder/search?q=…` | `airmail_service` serving the release's `index/` and `categories/` |
| `/routing/…` | reserved for a routing engine |

A Caddy proxy ([cardinal/proxy/Caddyfile](cardinal/proxy/Caddyfile)) routes
the paths, so clients only need one base URL. Behind it, a
[CloudNativePG](https://cloudnative-pg.io/) database tracks POI identifiers,
and `poi-updater` applies OSM's daily diffs to it.

## Adapting it

| What | Where |
| --- | --- |
| Data directory | [cardinal/data-volume.yaml](cardinal/data-volume.yaml); set `CARDINAL_DATA` for `release.sh` if it isn't `/var/lib/cardinal` |
| Images | `images` in [cardinal/kustomization.yaml](cardinal/kustomization.yaml) |
| Public hostname and exposure | [cardinal/proxy/ingress.yaml](cardinal/proxy/ingress.yaml); replace it with your own ingress if you don't use Tailscale |
| Database storage and memory | [cardinal/poi/database.yaml](cardinal/poi/database.yaml) |
| Storage class | `local-path` in [cardinal/poi/database.yaml](cardinal/poi/database.yaml) |

The data directory is a `hostPath` volume, which only suits a single node.
On a cluster with more, replace it with a `local` volume whose node affinity
names the build node, or put the releases on shared storage.

## Images

[.github/workflows/images.yml](../.github/workflows/images.yml) publishes
`cardinal-poi-service` and `cardinal-airmail` to the GitHub Container
Registry of the repository's owner on pushes to `main` that change
`poi_service/` or `airmail/`, tagged `sha-<short commit>` and `main`. To
deploy a build, set `newTag` in the kustomization to its `sha-` tag and
apply. `release.sh` uses the same images.

## Data releases

A release is a directory built from one OSM extract by the data pipeline
(`data_pipeline/README.md`), with POIs tagged from the POI database:

```
/var/lib/cardinal/
  planets/                 extracts, including the one the POI database was bootstrapped from
  wof/                     Who's On First databases for builds
  releases/<id>/           tiles.mbtiles, index/, categories/, manifest.json
```

The tileserver and geocoder serve the release named by `MAP_RELEASE` in
[cardinal/releases.env](cardinal/releases.env). Both must serve the same
release, since the tiles and the index agree on POI identifiers. Changing it
rolls both deployments, and each new pod is ready before the old one stops.

```sh
k8s/release.sh build /var/lib/cardinal/planets/planet-261102.osm.pbf
k8s/release.sh promote planet-20261102
```

`build`:

1. checks that the POI database has applied diffs up to the extract's
   timestamp (`poi_service covers`), since POIs it hasn't seen would get no
   identifiers;
2. runs the pipeline's `build` against the database, through a port-forward
   on `localhost:5433` (`CARDINAL_DB_PORT`), into `releases/.<id>.partial`;
3. checks the outputs, writes `manifest.json` (extract, POI checkpoint, git
   commit), and renames the directory to `releases/<id>`.

The ID defaults to the extract's name and date (`planet-20261102`,
`washington-20261101`); pass `--id` to choose another. `--wof-db` defaults to
`wof/whosonfirst-data-admin-latest.spatial.db` in the data directory. Set
`DAGGER_ENGINE` if the pipeline's README says you need to. Only one build or
prune runs at a time.

`promote` only accepts finished releases (with a `manifest.json`). It writes
`releases.env`, applies the kustomization, waits for the rollout, and checks
that `/tiles/catalog` lists `basemap` and that `/geocoder/search` finds
something for `--query` (default `park`). Commit `releases.env` afterwards
if you keep your deployment's state in git. To roll back, promote the
previous release.

`list` shows finished releases and which one is live. `prune --keep N`
deletes all but the newest N (default 2), never the live or committed one,
plus partial builds, after asking.

## Setting up

Once:

```sh
sudo mkdir -p /var/lib/cardinal/{planets,wof,releases}
sudo chown -R "$USER": /var/lib/cardinal
kubectl apply -f k8s/operators/cloudnative-pg.yaml
kubectl -n kube-system wait --for=condition=complete job/helm-install-cloudnative-pg --timeout=300s
kubectl -n cnpg-system rollout status deploy/cloudnative-pg
kubectl apply -k k8s/cardinal
```

Martin and the geocoder fail until a release is promoted, and `poi-updater`
until the POI database is bootstrapped.

### Bootstrapping the POI database

The POI database starts from a planet with element metadata (see
`poi_service/README.md`), which must be in the data directory's `planets/`:

```sh
k8s/release.sh bootstrap-pois /var/lib/cardinal/planets/planet-261005.osm.pbf
kubectl -n cardinal logs -f job/poi-bootstrap
```

For a quick test, use a Geofabrik extract and its updates:

```sh
k8s/release.sh bootstrap-pois /var/lib/cardinal/planets/washington-latest.osm.pbf \
  https://download.geofabrik.de/north-america/us/washington-updates/
```

Identifiers depend on the planet and replication URL, so changing either
means a new database: delete the `pois` cluster (which deletes its volume),
apply the kustomization again, and bootstrap.

## Notes

- Martin labels tiles `application/x-protobuf` even though they're MLT.
  Clients must not pick a decoder from the response's content type.
- The database isn't backed up. Losing it costs a bootstrap, not
  correctness: the same planet and diffs give the same identifiers. See
  CloudNativePG's documentation to add backups to object storage.
