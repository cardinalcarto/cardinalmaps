#!/usr/bin/env bash
# Server index for the offline/server comparison, as production builds it (data_pipeline
# Index): airmail_indexer (offline-geocoder code, SCHEMA_VERSION 8, s2cap) on the bench
# extract (synthetic poi_ids) with way locations and QRank, then every Who's On First
# place in the DB (load-wof), merged to one segment.
set -euo pipefail
D=${BENCH_DIR:-/mnt/maps/cardinal/offline-geocoder/bench-quality}
OUT=${1:-$D/server-index-q}
BIN=$D/target-airmail/release
WOF=/mnt/maps/cardinal/wof/whosonfirst-data-admin-latest.spatial.db
QRANK=/mnt/maps/cardinal/qrank/qrank.csv.gz
rm -rf $OUT $OUT-admin-cache
echo "== index $(date +%s)"
/usr/bin/time -v nice -n 10 $BIN/indexer --wof-db $WOF --index $OUT --admin-cache $OUT-admin-cache --qrank $QRANK load-osm-pbf $D/washington-bench-locations.osm.pbf
rm -rf $OUT-admin-cache
du -sb $OUT
echo "== load-wof $(date +%s)"
/usr/bin/time -v nice -n 10 $BIN/indexer --wof-db $WOF --index $OUT load-wof
echo "== merge $(date +%s)"
/usr/bin/time -v nice -n 10 $BIN/merge --index $OUT --merge
echo "== done $(date +%s)"
du -sb $OUT
