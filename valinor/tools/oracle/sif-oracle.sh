#!/bin/sh
# Runs the sif helper (tools/oracle/cpp/sif.cc; build it with cpp/build-sif.sh) over a tile set.
#
#   tools/oracle/sif-oracle.sh costing <tile_dir> <out_file> [node_stride] [edge_stride] [special_stride] [focus]
#   tools/oracle/sif-oracle.sh conditional <out_file> [count]
#   tools/oracle/sif-oracle.sh speeds <tile_dir> <level> <tile_id> <out_file>
#   tools/oracle/sif-oracle.sh edges <tile_dir> <out_file>
#
# The costings come from valinor-sif/fixtures/oracle/costings.tsv.
# Run from the valinor workspace root.
set -eu
CONTAINER="${CONTAINER:-podman}"
IMAGE="${IMAGE:-localhost/valinor-valhalla-dev:3.9.1}"
mode="$1"
shift
case "$mode" in
costing)
	tiles="$(realpath "$1")"
	out="$2"
	stride="${3:-50}"
	edge_stride="${4:-1}"
	special_stride="${5:-1}"
	focus="${6:-}"
	"$CONTAINER" run --rm -v "$PWD/tools/oracle/cpp:/w:z" -v "$tiles:/tiles:ro,z" \
		-v "$PWD/valinor-sif/fixtures/oracle:/o:ro,z" "$IMAGE" \
		/w/sif costing /tiles /o/costings.tsv "$stride" "$edge_stride" "$special_stride" $focus >"$out"
	;;
conditional)
	out="$1"
	count="${2:-3000}"
	"$CONTAINER" run --rm -v "$PWD/tools/oracle/cpp:/w:z" "$IMAGE" /w/sif conditional "$count" >"$out"
	;;
speeds)
	tiles="$(realpath "$1")"
	"$CONTAINER" run --rm -v "$PWD/tools/oracle/cpp:/w:z" -v "$tiles:/tiles:ro,z" "$IMAGE" \
		/w/sif speeds /tiles "$2" "$3" >"$4"
	;;
edges)
	tiles="$(realpath "$1")"
	"$CONTAINER" run --rm -v "$PWD/tools/oracle/cpp:/w:z" -v "$tiles:/tiles:ro,z" "$IMAGE" \
		/w/sif edges /tiles >"$2"
	;;
*)
	echo "unknown mode $mode" >&2
	exit 2
	;;
esac
