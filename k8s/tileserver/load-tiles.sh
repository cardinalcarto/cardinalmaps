#!/usr/bin/env bash
# Copies .mbtiles files into the tileserver's volume and restarts Martin.
#
#   k8s/tileserver/load-tiles.sh seattle.mbtiles [more.mbtiles ...]
#
# Each file is served under its name without the extension, replacing any
# existing tileset of that name.
set -euo pipefail

ns=tiles
pod=tiles-loader
here=$(cd "$(dirname "$0")" && pwd)

if [[ $# -eq 0 ]]; then
  echo "usage: $0 FILE.mbtiles [FILE.mbtiles ...]" >&2
  exit 2
fi
for f in "$@"; do
  [[ -f $f ]] || { echo "not a file: $f" >&2; exit 1; }
  [[ $f == *.mbtiles ]] || { echo "not an .mbtiles file: $f" >&2; exit 1; }
done

cleanup() { kubectl -n "$ns" delete pod "$pod" --ignore-not-found --wait=false >/dev/null; }
trap cleanup EXIT

echo "Starting loader pod..."
kubectl apply -f "$here/loader.yaml" >/dev/null
kubectl -n "$ns" wait --for=condition=Ready "pod/$pod" --timeout=120s >/dev/null

for f in "$@"; do
  name=$(basename "$f")
  echo "Copying $name..."
  # Copy under a name Martin ignores, then rename, so a partial copy never
  # replaces a tileset that's being served.
  kubectl -n "$ns" cp "$f" "$pod:/tiles/$name.tmp"
  want=$(wc -c <"$f" | tr -d ' ')
  got=$(kubectl -n "$ns" exec "$pod" -- sh -c "wc -c < '/tiles/$name.tmp'" | tr -d ' ')
  if [[ $want != "$got" ]]; then
    echo "size mismatch for $name: local $want bytes, copied $got" >&2
    kubectl -n "$ns" exec "$pod" -- rm -f "/tiles/$name.tmp"
    exit 1
  fi
  kubectl -n "$ns" exec "$pod" -- mv "/tiles/$name.tmp" "/tiles/$name"
done

cleanup
echo "Restarting Martin..."
kubectl -n "$ns" rollout restart deploy/martin >/dev/null
kubectl -n "$ns" rollout status deploy/martin --timeout=180s >/dev/null

host=$(kubectl -n "$ns" get ingress martin -o jsonpath='{.status.loadBalancer.ingress[0].hostname}')
if [[ -z $host ]]; then
  echo "Loaded. The ingress has no hostname yet, so the public URL wasn't checked."
  exit 0
fi
catalog=$(curl -fsS -m 20 "https://$host/catalog") || {
  echo "Loaded, but https://$host/catalog didn't respond." >&2
  exit 1
}
for f in "$@"; do
  id=$(basename "$f" .mbtiles)
  if grep -q "\"$id\":" <<<"$catalog"; then
    echo "Serving https://$host/$id"
  else
    echo "$id is missing from https://$host/catalog" >&2
    exit 1
  fi
done
