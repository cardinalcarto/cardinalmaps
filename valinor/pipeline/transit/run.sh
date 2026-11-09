#!/usr/bin/env bash
# Builds the fetch container and runs it for the named regions.
# See README.md. Usage:
#
#   run.sh --output DIR --region us-wa [--region ...] [--cache DIR] [--jobs N]
set -euo pipefail

usage() {
  echo "usage: $0 --output DIR --region REGION [--region REGION ...] [--cache DIR] [--jobs N]" >&2
  exit 2
}

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
image=cardinal-transit-fetch
output=""
cache=""
regions=()
extra=()

while (($# > 0)); do
  case "$1" in
    --output)
      (($# >= 2)) || usage
      output=$2
      shift 2
      ;;
    --cache)
      (($# >= 2)) || usage
      cache=$2
      shift 2
      ;;
    --region)
      (($# >= 2)) || usage
      regions+=(--region "$2")
      shift 2
      ;;
    --jobs)
      (($# >= 2)) || usage
      extra+=(--jobs "$2")
      shift 2
      ;;
    *)
      usage
      ;;
  esac
done

[[ -n $output && ${#regions[@]} -gt 0 ]] || usage

mkdir -p -- "$output"
output=$(cd -- "$output" && pwd)
volumes=(--volume "$output:/output:z")
if [[ -n $cache ]]; then
  mkdir -p -- "$cache"
  cache=$(cd -- "$cache" && pwd)
  volumes+=(--volume "$cache:/cache:z")
  extra+=(--cache /cache)
fi

podman build --tag "$image" --file "$here/Containerfile" "$here"

# The third-party code in here gets the network and these two directories, and nothing else.
podman run --rm \
  --read-only --tmpfs /tmp \
  --cap-drop=all --security-opt=no-new-privileges \
  "${volumes[@]}" \
  "$image" \
  --input /opt/transitous/feeds \
  --output /output \
  "${regions[@]}" \
  "${extra[@]}"
