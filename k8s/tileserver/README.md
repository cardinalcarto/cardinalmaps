# Tileserver

[Martin](https://maplibre.org/martin/) serves the `.mbtiles` files in the `tiles` volume, exposed publicly with Tailscale Funnel at `https://tiles.tail5db9f8.ts.net`.

Deploy with kustomize so everything lands in the `tiles` namespace:

```sh
kubectl apply -k k8s/tileserver
```

`kubectl apply -f k8s/tileserver/` puts the deployment, volume, service and ingress in `default` instead, and the second ingress takes the `tiles` hostname.

## Copying tilesets into the volume

```sh
k8s/tileserver/load-tiles.sh seattle.mbtiles [more.mbtiles ...]
```

Each file becomes a tileset named after it (`seattle.mbtiles` is served at `/seattle`), replacing any existing one of that name. The script:

1. starts a temporary loader pod ([loader.yaml](loader.yaml)) that mounts the volume read-write, since Martin mounts it read-only;
2. copies each file in as `<name>.mbtiles.tmp`, checks the size matches, then renames it, so a partial copy never replaces a tileset that's being served;
3. deletes the loader pod (also on failure);
4. restarts Martin, which only reads the directory at startup;
5. checks each tileset appears in the public `/catalog`.

To remove a tileset by hand:

```sh
kubectl apply -f k8s/tileserver/loader.yaml
kubectl -n tiles wait --for=condition=Ready pod/tiles-loader
kubectl -n tiles exec tiles-loader -- rm /tiles/<name>.mbtiles
kubectl -n tiles delete pod tiles-loader
kubectl -n tiles rollout restart deploy/martin
```

## Notes

- `kubectl cp` streams the file through the API server, which is fine for city or region extracts but slow for very large tilesets.
- Copied files keep the local owner and mode. Martin runs as uid 65534, so they need to be world-readable (`0644`, the default).
- Martin labels tiles `application/x-protobuf` even though they're MLT. Clients must not pick a decoder from the response's content type.
