#!/bin/sh
# Copies the captured stage-boundary messages into crate fixture directories.
#
# Usage: copy-message-fixtures.sh [captures-dir]
#
# The captures dir is what capture.py writes
# (default: /mnt/maps/cardinal/valhalla-oracle/captures).
# Each <region>/<case>/<format>/<boundary>/NNNNNN-N.<frame>.bin file is copied to
#   valhalla-proto/fixtures/captures/<region>/<case>/<format>/<boundary>.bin        (frame 1, the Api)
#   valhalla-microservice/fixtures/captures/<region>/<case>/<format>/<boundary>.info.bin
#                                                       (frame 0, prime_server's request info)
# Only the first request of each case is copied;
# capture.py sends one request per case, so there is never more than one.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
captures="${1:-/mnt/maps/cardinal/valhalla-oracle/captures}"
proto_out="$here/../../valhalla-proto/fixtures/captures"
micro_out="$here/../../valhalla-microservice/fixtures/captures"

rm -rf "$proto_out" "$micro_out"

cd "$captures"
for dir in */*/*/*; do
  [ -d "$dir" ] || continue
  case "$dir" in
    */loki-thor | */thor-odin) ;;
    *) continue ;;
  esac
  set -- "$dir"/*.1.bin
  if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
    echo "expected exactly one frame-1 message in $dir" >&2
    exit 1
  fi
  api="$1"
  info="${api%.1.bin}.0.bin"
  parent="$(dirname "$dir")"
  boundary="$(basename "$dir")"
  mkdir -p "$proto_out/$parent" "$micro_out/$parent"
  cp "$api" "$proto_out/$parent/$boundary.bin"
  cp "$info" "$micro_out/$parent/$boundary.info.bin"
done
