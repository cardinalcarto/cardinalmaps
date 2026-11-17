#!/bin/sh
# Makes the volume hold the Who's On First database of the release pinned by WOF_RELEASE, and
# runs wof_service on it. See README.md. On every start, in this order:
#
#   1. Fetch the release's release.json; refuse to start, touching nothing, if it's missing or
#      isn't a wof release of that date.
#   2. Drop a database the volume can't vouch for, before anything else uses it. The database
#      is about 34 GB, so unlike the geocoder's entrypoint this one doesn't hash it on every
#      start: DB.sha256, the sidecar, is written only after a download's size and SHA-256 were
#      checked, and the database is trusted when the sidecar holds release.json's SHA-256 and
#      its size is release.json's. Otherwise (no sidecar, another release's, a size that's off)
#      the database and the sidecar are deleted here. A sidecar without a database goes too.
#   3. If the database is still there, it's this release's: serve it.
#   4. Otherwise clear the volume, keeping only a partial download of this same release (and
#      lost+found), and check there's room for the rest.
#   5. Download into DB-RELEASE.partial, resuming it with range requests. A dropped connection
#      or a server error is retried here, resuming each time, with exponential backoff (from
#      WOF_RETRY_DELAY seconds, doubling, at most WOF_RETRY_MAX_DELAY), logging each attempt
#      with the bytes so far and curl's error, up to WOF_DOWNLOAD_ATTEMPTS attempts. Then it
#      exits non-zero, and the next start resumes the partial. A 4xx answer for the file
#      (other than a range the partial already covers) isn't retried.
#   6. Check the complete download's size and SHA-256. A mismatch deletes it and downloads
#      once more from scratch (the partial may have come from an earlier, broken attempt); a
#      second mismatch exits non-zero rather than download 34 GB again and again.
#   7. Only then move the partial to the database's path, and after that write the sidecar,
#      syncing each. An interruption between the two leaves a database without a sidecar,
#      which step 2 deletes on the next start.
#   8. exec wof_service with WOF_DB set to the database, passing the arguments on.
#
# The problems retrying can't fix each exit non-zero with one line saying which: a release.json
# that's missing or wrong, a volume without room, the file refused by the bucket, attempts used
# up, or a download that fails its check twice.
#
# Environment:
#   WOF_RELEASE             the release date, YYYY-MM-DD[.N] (required)
#   WOF_RELEASES_URL        the bucket's public releases URL, e.g. https://BUCKET.t3.storage.dev/releases
#   WOF_VOLUME              where the volume is mounted (default /data)
#   WOF_DOWNLOAD_ATTEMPTS   download attempts per start (default 30)
#   WOF_RETRY_DELAY         seconds before the first retry (default 5)
#   WOF_RETRY_MAX_DELAY     the most seconds between retries (default 300)
# Other WOF_* variables and the arguments are wof_service's (see wof_service --help).
set -eu

log() { echo "wof: $*" >&2; }
die() { log "$*"; exit 1; }

release=${WOF_RELEASE:?set WOF_RELEASE to a wof release date}
base=${WOF_RELEASES_URL:?set WOF_RELEASES_URL to the releases URL of the bucket}
volume=${WOF_VOLUME:-/data}
attempts=${WOF_DOWNLOAD_ATTEMPTS:-30}
first_delay=${WOF_RETRY_DELAY:-5}
max_delay=${WOF_RETRY_MAX_DELAY:-300}

case $release in
  [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] | [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9].[1-9]*) ;;
  *) die "WOF_RELEASE $release isn't YYYY-MM-DD or YYYY-MM-DD.N" ;;
esac
[ -d "$volume" ] || die "no volume at $volume"

prefix="${base%/}/wof/$release"

# 1. The release, checked before anything on the volume changes.
manifest=$(mktemp)
trap 'rm -f "$manifest"' EXIT
curl -fsS --retry 5 --retry-all-errors -o "$manifest" "$prefix/release.json" ||
  die "can't fetch $prefix/release.json; is WOF_RELEASE published?"
kind=$(jq -er .kind "$manifest") || die "$prefix/release.json has no kind"
date=$(jq -er .date "$manifest") || die "$prefix/release.json has no date"
[ "$kind" = wof ] && [ "$date" = "$release" ] ||
  die "$prefix/release.json is a $kind release of $date, not wof $release"
file=$(jq -er .database.file "$manifest") || die "$prefix/release.json names no database"
bytes=$(jq -er .database.bytes "$manifest") || die "$prefix/release.json gives no database size"
sha256=$(jq -er .database.sha256 "$manifest") || die "$prefix/release.json gives no database SHA-256"
case $file in
  */* | .* | "") die "$prefix/release.json names the database $file, not a file name" ;;
esac

db="$volume/$file"
sidecar="$db.sha256"
partial="$volume/$file-$release.partial"

size_of() { stat -c %s "$1"; }
sha_of() { sha256sum "$1" | cut -d' ' -f1; }

# 2. Delete a database whose sidecar doesn't vouch for it as this release's, first of all.
if [ -e "$db" ] || [ -e "$sidecar" ]; then
  recorded=$(cat "$sidecar" 2>/dev/null || true)
  if [ ! -f "$db" ]; then
    log "$sidecar has no database beside it; deleting it"
    rm -f "$sidecar"
  elif [ "$recorded" != "$sha256" ] || [ "$(size_of "$db")" != "$bytes" ]; then
    if [ -z "$recorded" ]; then
      log "$db has no recorded SHA-256 (an unchecked or interrupted download); deleting it"
    else
      log "$db isn't wof $release's database (recorded SHA-256 $recorded); deleting it"
    fi
    rm -f "$db" "$sidecar"
    sync
  fi
fi

# 3. Serve a database that's still there: its sidecar says it's this release's.
if [ ! -f "$db" ]; then
  # 4. Clear the volume, but keep a partial download of this release, and the filesystem's own
  # lost+found.
  find "$volume" -mindepth 1 -maxdepth 1 ! -name "$(basename "$partial")" ! -name lost+found \
    -exec rm -rf {} +
  have=0
  [ -f "$partial" ] && have=$(size_of "$partial")
  if [ "$have" -gt "$bytes" ]; then
    rm -f "$partial"
    have=0
  fi
  free=$(( $(df -Pk "$volume" | awk 'NR == 2 { print $4 }') * 1024 ))
  [ $(( bytes - have )) -le "$free" ] ||
    die "wof $release's database is $bytes bytes, $have of them downloaded, but $volume has $free free"

  # 5. Download, resuming, retrying with backoff.
  download() {
    attempt=1
    delay=$first_delay
    while :; do
      have=0
      [ -f "$partial" ] && have=$(size_of "$partial")
      [ "$have" -lt "$bytes" ] || return 0
      log "downloading $prefix/$file, attempt $attempt of $attempts: from byte $have of $bytes"
      status=0
      code=$(curl -fsS -C - -w '%{http_code}' -o "$partial" "$prefix/$file") || status=$?
      [ $status = 0 ] && return 0
      have=0
      [ -f "$partial" ] && have=$(size_of "$partial")
      case $code in
        4[0-9][0-9])
          [ "$code" = 416 ] ||
            die "the bucket answered HTTP $code for $prefix/$file; is the release complete?" ;;
      esac
      if [ "$have" -gt "$bytes" ]; then
        log "the download is $have bytes, more than release.json's $bytes; starting it over"
        rm -f "$partial"
      fi
      log "attempt $attempt failed (curl exit $status, HTTP ${code:-none}) with $have of $bytes bytes"
      [ "$attempt" -lt "$attempts" ] ||
        die "gave up downloading $prefix/$file after $attempts attempts, with $have of $bytes bytes; the next start resumes it"
      log "retrying in ${delay}s"
      sleep "$delay"
      attempt=$((attempt + 1))
      delay=$((delay * 2))
      [ "$delay" -le "$max_delay" ] || delay=$max_delay
    done
  }

  # 6. Check the complete download; one fresh download if it doesn't match.
  checks=0
  while :; do
    download
    got_bytes=$(size_of "$partial")
    log "checking the SHA-256 of the download ($got_bytes bytes)"
    if [ "$got_bytes" = "$bytes" ] && [ "$(sha_of "$partial")" = "$sha256" ]; then
      break
    fi
    rm -f "$partial"
    sync
    checks=$((checks + 1))
    [ $checks -lt 2 ] ||
      die "the download of $prefix/$file failed its size and SHA-256 check again; deleted it and giving up. Check the bucket's file against release.json"
    log "THE DOWNLOAD OF $prefix/$file DOESN'T MATCH release.json'S SIZE AND SHA-256; deleted it, downloading it once more from scratch"
  done

  # 7. Only a checked download becomes the database, and only then gets its sidecar.
  mv "$partial" "$db"
  sync
  echo "$sha256" > "$sidecar.tmp"
  mv "$sidecar.tmp" "$sidecar"
  sync
  log "downloaded wof $release's database to $db"
fi

# 8. Serve.
log "serving wof $release from $db"
rm -f "$manifest"
trap - EXIT
export WOF_DB="$db"
exec wof_service "$@"
