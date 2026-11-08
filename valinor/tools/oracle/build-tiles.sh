#!/bin/sh
# Builds Valhalla tiles with the pinned mainline image.
#
# Usage:
#   build-tiles.sh tiles <pbf> <out-dir> <bbox: minx,miny,maxx,maxy>
#   build-tiles.sh predicted <out-dir> <csv-dir>
#
# Paths may be relative; they are resolved on the host and mounted into the container.
# Requires podman (rootless is fine); set CONTAINER=docker to use docker instead.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
image="$(cat "$here/valhalla-image")"
runtime="${CONTAINER:-podman}"

abs() {
	mkdir -p "$1"
	(cd "$1" && pwd)
}

cmd="$1"
shift

case "$cmd" in
tiles)
	pbf_dir="$(cd "$(dirname "$1")" && pwd)"
	pbf_name="$(basename "$1")"
	out="$(abs "$2")"
	bbox="$3"
	# :z (shared label), not :Z, so several containers can use the same directories under SELinux.
	"$runtime" run --rm \
		-v "$here/container:/oracle:ro,z" \
		-v "$pbf_dir:/input:ro,z" \
		-v "$out:/out:z" \
		"$image" /oracle/build.sh tiles "/input/$pbf_name" /out "$bbox"
	;;
predicted)
	out="$(abs "$1")"
	csv="$(cd "$2" && pwd)"
	"$runtime" run --rm \
		-v "$here/container:/oracle:ro,z" \
		-v "$csv:/csv:ro,z" \
		-v "$out:/out:z" \
		"$image" /oracle/build.sh predicted /out /csv
	;;
*)
	echo "usage: $0 tiles <pbf> <out-dir> <bbox> | predicted <out-dir> <csv-dir>" >&2
	exit 2
	;;
esac
