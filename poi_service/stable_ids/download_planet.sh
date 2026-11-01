#!/usr/bin/env bash
# Download a dated planet.osm.pbf snapshot from planet.openstreetmap.org and
# verify it against the published MD5. Resumable: re-run after an interruption.
#
# Usage: ./download_planet.sh [YYMMDD] [output-dir]
#
# The date must be one of the weekly snapshots listed at
# https://planet.openstreetmap.org/pbf/ (or its per-year archive directories).
set -euo pipefail

date="${1:-261005}"
outdir="${2:-$(dirname "$0")/data}"
name="planet-${date}.osm.pbf"
mkdir -p "$outdir"
cd "$outdir"

base="https://planet.openstreetmap.org/pbf"
url=""
for candidate in "$base/$name" "$base/20${date:0:2}/$name"; do
  if curl -fsIL "$candidate" >/dev/null; then
    url="$candidate"
    break
  fi
done
if [[ -z "$url" ]]; then
  echo "Could not find $name on planet.openstreetmap.org" >&2
  exit 1
fi

curl -fsSL "$url.md5" -o "$name.md5"

if [[ -f "$name" ]] && md5sum --quiet -c "$name.md5" 2>/dev/null; then
  echo "$name already downloaded and verified"
  exit 0
fi

echo "Downloading $url"
curl -fL --retry 20 --retry-all-errors --retry-delay 10 -C - -o "$name" "$url"

echo "Verifying checksum"
md5sum -c "$name.md5"
