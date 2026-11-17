#!/bin/sh
# Runs wof_service on the database at WOF_DB, first downloading it if it isn't there:
#
#   1. If WOF_DB exists and WOF_DB_SHA256 is unset, or WOF_DB.sha256 (written after a checked
#      download) holds WOF_DB_SHA256, serve it. The file is opened read-only and never changes,
#      so it's hashed once, not on every start.
#   2. Otherwise, if WOF_DB_URL is set, download it to WOF_DB.partial, decompressing a URL that
#      ends in .bz2, check its SHA-256 against WOF_DB_SHA256 if that's set, and only then move
#      it to WOF_DB. A mismatch deletes the download and exits non-zero.
#   3. Otherwise exit non-zero.
#
# Environment:
#   WOF_DB          the database's path (default /data/whosonfirst-data-admin-latest.spatial.db)
#   WOF_DB_URL      where to download it from, if it's missing (optional)
#   WOF_DB_SHA256   the SHA-256 of the uncompressed database (optional, recommended)
# Other WOF_* variables and the arguments are wof_service's (see wof_service --help).
set -eu

log() { echo "wof_service: $*" >&2; }
die() { log "$*"; exit 1; }

db=${WOF_DB:-/data/whosonfirst-data-admin-latest.spatial.db}
url=${WOF_DB_URL:-}
sha256=${WOF_DB_SHA256:-}
export WOF_DB="$db"

ready=0
if [ -f "$db" ]; then
  if [ -z "$sha256" ] || [ "$(cat "$db.sha256" 2>/dev/null)" = "$sha256" ]; then
    ready=1
  else
    log "$db isn't the database with SHA-256 $sha256"
  fi
fi

if [ $ready = 0 ]; then
  [ -n "$url" ] || die "no database at $db, and WOF_DB_URL is unset"
  mkdir -p "$(dirname "$db")"
  rm -f "$db" "$db.sha256" "$db.partial"
  log "downloading $url to $db"
  case $url in
    *.bz2)
      curl -fsS --retry 10 --retry-all-errors "$url" | bzip2 -dc > "$db.partial" ||
        { rm -f "$db.partial"; die "downloading $url failed"; } ;;
    *)
      curl -fsS --retry 10 --retry-all-errors -o "$db.partial" "$url" ||
        { rm -f "$db.partial"; die "downloading $url failed"; } ;;
  esac
  got=$(sha256sum "$db.partial" | cut -d' ' -f1)
  if [ -n "$sha256" ] && [ "$got" != "$sha256" ]; then
    rm -f "$db.partial"
    die "the download's SHA-256 is $got, not $sha256; deleted it"
  fi
  mv "$db.partial" "$db"
  echo "$got" > "$db.sha256"
  sync
  log "downloaded $db (SHA-256 $got)"
fi

exec wof_service "$@"
