#!/bin/bash
# Cuts OpenGeoHub's GEDTM, one large GeoTIFF, into 1x1 degree GeoTIFFs named like
# GEDTM_N047W123.tif that `topo` can read, skipping cells with no data. Reruns skip cells
# already done.
#
#   tile_gedtm.sh SOURCE.tif OUT_DIR [WEST SOUTH EAST NORTH] [JOBS]
#
# SOURCE can be remote, as /vsicurl/https://... The region is in whole degrees.
#
# Each tile is 3601x3601, with pixel centers from one whole degree to the next, sharing its
# edges with its neighbors, like SRTM. The source's values are meters, despite its scale tag
# of 0.1 (Mount Rainier's summit reads 4402.8, and its Zenodo record says a scale of 1), so
# they're kept as they are, without the tag.
set -euo pipefail

if [[ $# -lt 2 ]]; then
  sed -n '2,12p' "$0" | sed 's/^# \?//' >&2
  exit 1
fi
source=$1 out=$2 west=${3:--180} south=${4:--65} east=${5:-180} north=${6:-85} jobs=${7:-8}
mkdir -p "$out"

# The pixel at column 4, row 4 of v1.2 is centered on (-180, 85). Refuse any other layout.
geo=$(gdalinfo -nomd "$source" | grep -E '^(Origin|Pixel Size|Size is)')
expected='Size is 1296009, 540009
Origin = (-180.001249999999999,85.001249999999999)
Pixel Size = (0.000277777777778,-0.000277777777778)'
if [[ $geo != "$expected" ]]; then
  echo "unexpected layout, which this script's offsets would get wrong:" >&2
  echo "$geo" >&2
  exit 1
fi

cut_tile() {
  local source=$1 out=$2 lon=$3 lat=$4
  local ns=N ew=E
  ((lat < 0)) && ns=S
  ((lon < 0)) && ew=W
  local name
  name=$(printf 'GEDTM_%s%03d%s%03d' $ns ${lat#-} $ew ${lon#-})
  [[ -e $out/$name.tif || -e $out/$name.empty ]] && return 0
  local column=$(((lon + 180) * 3600 + 4)) row=$(((85 - lat - 1) * 3600 + 4))
  local tmp=$out/$name.partial
  gdal_translate -q -of GTiff -srcwin $column $row 3601 3601 -a_scale 1 -a_offset 0 \
    -co COMPRESS=DEFLATE -co PREDICTOR=3 -co TILED=NO "$source" "$tmp"
  # No min and max means every pixel is no data, like open ocean.
  if gdalinfo -mm "$tmp" 2>/dev/null | grep -q 'Computed Min/Max'; then
    mv "$tmp" "$out/$name.tif"
  else
    rm -f "$tmp"
    : >"$out/$name.empty"
  fi
}
export -f cut_tile

for ((lat = south; lat < north; lat++)); do
  for ((lon = west; lon < east; lon++)); do
    printf '%s\0%s\0%s\0%s\0' "$source" "$out" $lon $lat
  done
done | xargs -0 -n 4 -P "$jobs" bash -c 'cut_tile "$@"' _

echo "$(find "$out" -name '*.tif' | wc -l) tiles with data, $(find "$out" -name '*.empty' | wc -l) empty"
