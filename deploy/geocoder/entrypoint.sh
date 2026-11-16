#!/bin/sh
# Makes the volume hold the geocoder release pinned by GEOCODER_RELEASE, mounts its image, and
# runs airmail on it. See README.md and PLAN.md (decision 3). On every start:
#
#   1. Fetch the release's release.json; refuse to start, touching nothing, if it's missing,
#      isn't a geocoder release of that date, or has an index schema this airmail can't read.
#   2. If the volume holds geocoder.btrfs with the release's size and SHA-256, use it.
#   3. Otherwise clear the volume, keeping only a partial download of this same release.
#   4. Download the image, resuming a partial download, and check its size and SHA-256 before
#      it becomes geocoder.btrfs. A mismatch deletes the download and exits non-zero.
#   5. Mount it read-only and exec airmail on its index/, categories/ and tagger.model.
#
# Environment:
#   GEOCODER_RELEASE        the release date, YYYY-MM-DD[.N] (required)
#   GEOCODER_RELEASES_URL   the bucket's public releases URL, e.g. https://BUCKET.t3.storage.dev/releases
#   GEOCODER_VOLUME         where the volume is mounted (default /data)
#   GEOCODER_MOUNT          where the image is mounted (default /srv/geocoder)
#   GEOCODER_SCHEMA_FILE    holds the index schema version this airmail reads
#                           (default /etc/airmail/schema-version, written when the image is built)
# Arguments are passed on to airmail_service.
set -eu

log() { echo "geocoder: $*" >&2; }
die() { log "$*"; exit 1; }

release=${GEOCODER_RELEASE:?set GEOCODER_RELEASE to a geocoder release date}
base=${GEOCODER_RELEASES_URL:?set GEOCODER_RELEASES_URL to the releases URL of the bucket}
volume=${GEOCODER_VOLUME:-/data}
mountpoint=${GEOCODER_MOUNT:-/srv/geocoder}
schema_file=${GEOCODER_SCHEMA_FILE:-/etc/airmail/schema-version}

case $release in
  [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] | [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9].[1-9]*) ;;
  *) die "GEOCODER_RELEASE $release isn't YYYY-MM-DD or YYYY-MM-DD.N" ;;
esac
[ -d "$volume" ] || die "no volume at $volume"

prefix="${base%/}/geocoder/$release"
image="$volume/geocoder.btrfs"
partial="$volume/geocoder-$release.partial"

# 1. The release, checked before anything on the volume changes.
manifest=$(mktemp)
trap 'rm -f "$manifest"' EXIT
curl -fsS --retry 5 --retry-all-errors -o "$manifest" "$prefix/release.json" ||
  die "can't fetch $prefix/release.json; is GEOCODER_RELEASE published?"
kind=$(jq -er .kind "$manifest") || die "$prefix/release.json has no kind"
date=$(jq -er .date "$manifest") || die "$prefix/release.json has no date"
[ "$kind" = geocoder ] && [ "$date" = "$release" ] ||
  die "$prefix/release.json is a $kind release of $date, not geocoder $release"
schema=$(jq -er .schema_version "$manifest") || die "$prefix/release.json has no schema_version"
file=$(jq -er .image.file "$manifest") || die "$prefix/release.json names no image"
bytes=$(jq -er .image.bytes "$manifest") || die "$prefix/release.json gives no image size"
sha256=$(jq -er .image.sha256 "$manifest") || die "$prefix/release.json gives no image SHA-256"
readable=$(cat "$schema_file") || die "can't read $schema_file"
[ "$schema" = "$readable" ] ||
  die "geocoder $release has index schema $schema, but this airmail reads schema $readable"

size_of() { stat -c %s "$1"; }
sha_of() { sha256sum "$1" | cut -d' ' -f1; }

# 2. The image already there, if it's this release's.
ready=0
if [ -f "$image" ]; then
  if [ "$(size_of "$image")" = "$bytes" ]; then
    log "checking the SHA-256 of $image ($bytes bytes)"
    [ "$(sha_of "$image")" = "$sha256" ] && ready=1
  fi
  [ $ready = 1 ] || log "$image isn't geocoder $release's image"
fi

if [ $ready = 0 ]; then
  # 3. Clear the volume, but keep a partial download of this release, and the filesystem's own
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
    die "geocoder $release's image is $bytes bytes, $have of them downloaded, but $volume has $free free"

  # 4. Download, resuming, then check before the image is used.
  if [ "$have" -lt "$bytes" ]; then
    log "downloading $prefix/$file ($bytes bytes, from byte $have)"
    curl -fsS --retry 10 --retry-all-errors -C - -o "$partial" "$prefix/$file" ||
      die "downloading $prefix/$file failed; the next start resumes it"
  fi
  if [ "$(size_of "$partial")" != "$bytes" ] || [ "$(sha_of "$partial")" != "$sha256" ]; then
    rm -f "$partial"
    die "the download of $prefix/$file doesn't match release.json's size and SHA-256; deleted it"
  fi
  mv "$partial" "$image"
  sync
fi

# 5. Mount and serve.
mkdir -p "$mountpoint"
mount -o loop,ro "$image" "$mountpoint" || die "can't mount $image at $mountpoint"
log "serving geocoder $release from $mountpoint"
rm -f "$manifest"
trap - EXIT
exec airmail_service --index "$mountpoint/index" --categories "$mountpoint/categories" \
  --tagger "$mountpoint/tagger.model" "$@"
