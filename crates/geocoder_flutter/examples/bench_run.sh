#!/usr/bin/env bash
# Runs the quality comparison against the four running services:
#   3930 airmail_service (tagger + eng model), 3931 the same without the tagger,
#   3920 offline_serve (tagger + eng model),   3921 the same without the tagger.
set -euo pipefail
D=${BENCH_DIR:-/mnt/maps/cardinal/offline-geocoder/bench-quality}
H=$(dirname "$(readlink -f "$0")")/bench_quality.py
SETS="--pelias $D/pelias-src/acceptance-tests --sample $D/sample.jsonl"
cd $D
python3 -I -B $H run --base http://127.0.0.1:3930 --out $D/results/server $SETS
python3 -I -B $H run --base http://127.0.0.1:3920 --out $D/results/offline --hydrate-base http://127.0.0.1:3930 $SETS
python3 -I -B $H compare $D/results/server $D/results/offline > $D/results/compare.txt
python3 -I -B $H diagnose $D/results/server $D/results/offline $SETS --hydrate-base http://127.0.0.1:3930 \
  --offline-base http://127.0.0.1:3920 --offline-notagger-base http://127.0.0.1:3921 \
  --server-notagger-base http://127.0.0.1:3931 --out $D/results/diagnosis.jsonl
echo done
