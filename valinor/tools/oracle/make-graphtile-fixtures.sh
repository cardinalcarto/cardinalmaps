#!/bin/sh
# Regenerates the valhalla-graphtile test fixtures from the pinned Andorra extract
# using the pinned mainline Valhalla image.
#
# Usage: make-graphtile-fixtures.sh [work-dir]
#
# The work dir holds intermediate build products (admins, timezones, elevation, ...)
# and defaults to a fresh temporary directory.
#
# Produces, under valhalla-graphtile/fixtures:
#   andorra-tiles/              the tile directory
#   andorra-tiles.tar           the tarball extract of the same tiles
#   andorra-tiles-with-traffic/ tile 0/003/015 after valhalla_add_predicted_traffic
#                               (the rows in predicted-speeds/ match the add_predicted_speeds test)
#   andorra-traffic.tar         the live traffic extract, with edge 0/3015/42 set to 32 kph
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
fixtures="$here/../../valhalla-graphtile/fixtures"
pbf="$fixtures/source/andorra-2026-11-07.osm.pbf"
pbf_sha256="8885ab9f90e41900cdf52c939211a84b0c44fe4a337333345011b69d562a4204"
bbox="1.40,42.42,1.79,42.66"

work="${1:-$(mktemp -d)}"
mkdir -p "$work"
echo "work dir: $work"

echo "$pbf_sha256  $pbf" | sha256sum -c -

"$here/build-tiles.sh" tiles "$pbf" "$work/build" "$bbox"
"$here/build-tiles.sh" predicted "$work/build" "$here/predicted-speeds"

rm -rf "$fixtures/andorra-tiles" "$fixtures/andorra-tiles-with-traffic"
cp -r "$work/build/tiles" "$fixtures/andorra-tiles"
cp "$work/build/tiles.tar" "$fixtures/andorra-tiles.tar"
mkdir -p "$fixtures/andorra-tiles-with-traffic/0/003"
cp "$work/build/tiles-predicted/0/003/015.gph" "$fixtures/andorra-tiles-with-traffic/0/003/015.gph"
cp "$work/build/traffic.tar" "$fixtures/andorra-traffic.tar"
python3 -I "$here/set-live-speed.py" "$fixtures/andorra-traffic.tar" 0/3015/42 32

echo "fixtures updated in $fixtures"
