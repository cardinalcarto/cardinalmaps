#!/bin/sh
# Copies Odin's captured inputs and reference outputs into illuvatar/fixtures.
#
# Usage: copy-illuvatar-fixtures.sh [captures-dir...]
#
# Each captures dir is what capture.py writes
# (default: /mnt/maps/cardinal/valhalla-oracle/captures).
# For every <region>/<case> with a PBF-format capture, this copies
#   <region>/<case>/pbf/thor-odin/NNNNNN-N.1.bin  ->  illuvatar/fixtures/<region>/<case>/thor-odin.bin
#   <region>/<case>/pbf/response.1.bin           ->  illuvatar/fixtures/<region>/<case>/response.bin
# The Thor->Odin message is Odin's input (options and trip);
# the PBF response carries the directions mainline's Odin built from it.
# Cases without a Thor->Odin message (errors raised before Thor, and locate) are skipped.
# With several captures dirs, later ones add cases (and replace cases of the same name).
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
out="$here/../../illuvatar/fixtures"

if [ "$#" -eq 0 ]; then
  set -- /mnt/maps/cardinal/valhalla-oracle/captures
fi

rm -rf "$out"
mkdir -p "$out"

for captures in "$@"; do
  for case_dir in "$captures"/*/*; do
    [ -d "$case_dir/pbf/thor-odin" ] || continue
    [ -f "$case_dir/pbf/response.1.bin" ] || continue
    set -- "$case_dir"/pbf/thor-odin/*.1.bin
    if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
      echo "expected exactly one frame-1 message in $case_dir/pbf/thor-odin" >&2
      exit 1
    fi
    region="$(basename "$(dirname "$case_dir")")"
    name="$(basename "$case_dir")"
    mkdir -p "$out/$region/$name"
    cp "$1" "$out/$region/$name/thor-odin.bin"
    cp "$case_dir/pbf/response.1.bin" "$out/$region/$name/response.bin"
  done
done

cat > "$out/README.md" <<'EOF'
# Odin fixtures

Copied by `tools/oracle/copy-illuvatar-fixtures.sh` from the mainline Valhalla 3.9.1 captures.
Each case holds the Thor→Odin `Api` message (`thor-odin.bin`, Odin's input)
and mainline's PBF response (`response.bin`), whose `directions` are the reference output.

The messages are derived from OpenStreetMap data,
© OpenStreetMap contributors, available under the Open Database License.
EOF
