#!/bin/sh
# Runs inside the pinned mainline Valhalla image (see ../valhalla-image).
# Invoke it through ../build-tiles.sh rather than directly.
#
# Subcommands:
#   tiles <pbf> <out-dir> <bbox>
#       Builds a complete tile set the same way the Cardinal pipeline does:
#       admins, timezones, elevation, landmarks, pedestrian areas, and bounding circles,
#       then a tarball extract with a live traffic skeleton.
#   predicted <out-dir> <csv-dir>
#       Copies <out-dir>/tiles to <out-dir>/tiles-predicted
#       and applies valhalla_add_predicted_traffic with the CSVs in <csv-dir>.
set -eu

cmd="$1"
shift

case "$cmd" in
tiles)
	pbf="$1"
	out="$2"
	bbox="$3"
	mkdir -p "$out/tiles" "$out/elevation"
	valhalla_build_config \
		--mjolnir-tile-dir "$out/tiles" \
		--mjolnir-tile-extract "$out/tiles.tar" \
		--mjolnir-traffic-extract "$out/traffic.tar" \
		--mjolnir-timezone "$out/timezones.sqlite" \
		--mjolnir-admin "$out/admins.sqlite" \
		--mjolnir-landmarks "$out/landmarks.sqlite" \
		--mjolnir-pedestrian-areas True \
		--mjolnir-data-processing-build-bounding-circles True \
		--additional-data-elevation "$out/elevation" \
		>"$out/valhalla.json"
	valhalla_build_timezones >"$out/timezones.sqlite"
	valhalla_build_admins -c "$out/valhalla.json" "$pbf"
	valhalla_build_elevation -c "$out/valhalla.json" -b "$bbox"
	valhalla_build_landmarks -c "$out/valhalla.json" "$pbf"
	valhalla_build_tiles -c "$out/valhalla.json" "$pbf"
	valhalla_add_landmarks -c "$out/valhalla.json"
	valhalla_build_extract -c "$out/valhalla.json" --with-traffic --overwrite
	;;
predicted)
	out="$1"
	csv="$2"
	rm -rf "$out/tiles-predicted"
	cp -r "$out/tiles" "$out/tiles-predicted"
	valhalla_add_predicted_traffic -c "$out/valhalla.json" \
		-i "{\"mjolnir\":{\"tile_dir\":\"$out/tiles-predicted\"}}" \
		"$csv"
	;;
*)
	echo "unknown subcommand: $cmd" >&2
	exit 2
	;;
esac
