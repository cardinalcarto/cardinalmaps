#!/usr/bin/env bash
# Reproduce the labelled edit pairs used to measure the stable-ID rules.
#
# Usage: ./run_pipeline.sh [planet YYMMDD] [days of diffs]
set -euo pipefail
cd "$(dirname "$0")"

date="${1:-261005}"
days="${2:-21}"

./download_planet.sh "$date" data
uv run stable-ids-extract --planet "data/planet-$date.osm.pbf" \
  --pbf-out "data/pois-$date.osm.pbf" --out "data/pois-$date.parquet"
uv run stable-ids-diffs --planet "data/planet-$date.osm.pbf" --days "$days" \
  --diff-dir data/diffs --out "data/changes-$date-${days}d.parquet"
uv run stable-ids-pairs --pois "data/pois-$date.parquet" \
  --changes "data/changes-$date-${days}d.parquet" --out "data/pairs-$date-${days}d.parquet"
