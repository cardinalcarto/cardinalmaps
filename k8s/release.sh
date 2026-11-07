#!/usr/bin/env bash
# Builds Cardinal's data releases and switches the cluster between them. See
# k8s/README.md.
#
#   k8s/release.sh init
#   k8s/release.sh bootstrap-pois PLANET.osm.pbf [REPLICATION_URL]
#   k8s/release.sh build EXTRACT.osm.pbf [--id ID] [--wof-db FILE] [--qrank FILE] [--sources DIR] [--contours CONFIG [--dem DIR]] [--categories DIR] [--tagger FILE]
#   k8s/release.sh promote ID [--query Q]
#   k8s/release.sh list
#   k8s/release.sh prune [--keep N] [--yes]
set -euo pipefail

ns=cardinal
# The data volume's path on the node; see k8s/cardinal/data-volume.yaml.
root=${CARDINAL_DATA:-/var/lib/cardinal}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/.." && pwd)
# This deployment's overlay of k8s/cardinal, which git ignores: the image
# tags it runs and the release it serves.
site=${CARDINAL_SITE:-$here/local}
releases_env=$site/releases.env
# Local end of the database port-forward, clear of a PostgreSQL on the
# build machine itself.
db_port=${CARDINAL_DB_PORT:-5433}

die() { echo "$*" >&2; exit 1; }
usage() { sed -n '5,10p' "$0" | sed 's/^# //' >&2; exit 2; }

valid_id() { [[ $1 =~ ^[a-z0-9][a-z0-9._-]*$ ]]; }

require_site() {
  [[ -f $site/kustomization.yaml ]] || die "$site doesn't exist; run k8s/release.sh init"
}

# The image the overlay deploys for one of the images.yml images.
image() {
  require_site
  local ref
  ref=$(kubectl kustomize "$site" | awk -v n="/$1:" '$1 == "image:" && index($2, n) { print $2; exit }')
  [[ -n $ref ]] || die "the overlay in $site deploys no $1 image"
  echo "$ref"
}

# The release the cluster's tileserver serves, if any.
live_release() {
  local cm
  cm=$(kubectl -n "$ns" get deploy martin \
    -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="MAP_RELEASE")].valueFrom.configMapKeyRef.name}' \
    2>/dev/null) || return 0
  [[ -z $cm ]] || kubectl -n "$ns" get configmap "$cm" -o jsonpath='{.data.MAP_RELEASE}'
}

site_release() { [[ ! -f $releases_env ]] || sed -n 's/^MAP_RELEASE=//p' "$releases_env"; }

# Holds a lock on the releases directory until the script exits, so builds
# and prunes don't run at the same time.
lock_releases() {
  [[ -d $root/releases ]] || die "$root/releases doesn't exist; see k8s/README.md"
  exec 9>"$root/releases/.lock"
  flock -n 9 || die "another build or prune is running"
}

# Forwards $db_port to the POI database until the script exits.
forward_db() {
  kubectl -n "$ns" port-forward svc/pois-rw "$db_port:5432" >/dev/null &
  local pid=$!
  trap "kill $pid 2>/dev/null || true" EXIT
  for _ in $(seq 100); do
    if (exec 3<>"/dev/tcp/127.0.0.1/$db_port") 2>/dev/null; then
      return
    fi
    kill -0 "$pid" 2>/dev/null || die "kubectl port-forward to the POI database failed"
    sleep 0.2
  done
  die "timed out waiting for the POI database port-forward"
}

# The POI database's connection string, with its host replaced by $1.
database_url() {
  kubectl -n "$ns" get secret pois-app -o jsonpath='{.data.uri}' | base64 -d | sed -E "s#@[^/]+/#@$1/#"
}

# Creates the overlay, pinning the images to $1 (default main).
cmd_init() {
  [[ $# -le 1 ]] || usage
  local tag=${1:-main} name
  [[ ! -e $site ]] || die "$site already exists"
  mkdir -p "$site"
  {
    echo "# This deployment of k8s/cardinal. Created by \`k8s/release.sh init\`; git"
    echo "# ignores it. Set newTag to the image tags to run, then apply with"
    echo "# \`kubectl apply -k ${site#"$repo/"}\`."
    echo "apiVersion: kustomize.config.k8s.io/v1beta1"
    echo "kind: Kustomization"
    echo "resources:"
    echo "  - $(realpath --relative-to="$site" "$here/cardinal")"
    echo "configMapGenerator:"
    echo "  # Written by \`k8s/release.sh promote\`."
    echo "  - name: map-release"
    echo "    behavior: replace"
    echo "    envs: [releases.env]"
    echo "images:"
    for name in $(awk '$1 == "newName:" { print $2 }' "$here/cardinal/kustomization.yaml"); do
      echo "  - name: $name"
      echo "    newTag: $tag"
    done
  } >"$site/kustomization.yaml"
  echo "MAP_RELEASE=none" >"$releases_env"
  echo "Created $site. Apply it with:"
  echo "  kubectl apply -k ${site#"$repo/"}"
}

cmd_bootstrap_pois() {
  [[ $# -ge 1 && $# -le 2 ]] || usage
  local planet url=${2:-https://planet.openstreetmap.org/replication/day/}
  [[ -f $1 ]] || die "not a file: $1"
  planet=$(realpath "$1")
  [[ $planet == "$root/planets/"* ]] || die "the planet must be in $root/planets, which the job mounts"
  if kubectl -n "$ns" get job poi-bootstrap >/dev/null 2>&1; then
    die "job/poi-bootstrap already exists; delete it first if it's finished"
  fi
  PLANET=${planet#"$root/planets/"} REPLICATION_URL=$url POI_SERVICE_IMAGE=$(image cardinal-poi-service) \
    envsubst '$PLANET $REPLICATION_URL $POI_SERVICE_IMAGE' <"$here/jobs/poi-bootstrap.yaml" | kubectl apply -f - >/dev/null
  echo "Started job/poi-bootstrap. Follow it with:"
  echo "  kubectl -n $ns logs -f job/poi-bootstrap"
  echo "poi-updater picks up from the planet once it finishes."
}

cmd_build() {
  local extract='' id='' wof=$root/wof/whosonfirst-data-admin-latest.spatial.db qrank=$root/qrank/qrank.csv.gz
  local contours='' extra_args=() categories='' tagger=''
  while [[ $# -gt 0 ]]; do
    case $1 in
      --id) id=$2; shift 2 ;;
      --wof-db) wof=$2; shift 2 ;;
      --qrank) qrank=$2; shift 2 ;;
      --sources) extra_args+=(--sources="$(realpath "$2")"); shift 2 ;;
      # Reads AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY if its DEMs are in a bucket.
      --contours) contours=$2; shift 2 ;;
      # DEM rasters, mounted at /dem for a --contours configuration that reads them from there.
      --dem) extra_args+=(--dem="$(realpath "$2")"); shift 2 ;;
      # Categorical search models and the query tagger, trained by hand; by
      # default the live release's.
      --categories) categories=$2; shift 2 ;;
      --tagger) tagger=$2; shift 2 ;;
      -*) usage ;;
      *) [[ -z $extract ]] || usage; extract=$1; shift ;;
    esac
  done
  [[ -n $extract ]] || usage
  [[ -f $extract ]] || die "not a file: $extract"
  if [[ -n $contours ]]; then
    [[ -f $contours ]] || die "not a file: $contours"
    extra_args+=(--contour-config="$(realpath "$contours")")
    [[ -z ${AWS_ACCESS_KEY_ID:-} ]] ||
      extra_args+=(--s3-access-key-id=env://AWS_ACCESS_KEY_ID --s3-secret-access-key=env://AWS_SECRET_ACCESS_KEY)
  fi
  [[ -f $wof ]] || die "no Who's On First database at $wof; pass --wof-db"
  if [[ -z $categories || -z $tagger ]]; then
    local live
    live=$(live_release)
    [[ -n $live && $live != none ]] || die "no live release to take models from; pass --categories and --tagger"
    categories=${categories:-$root/releases/$live/categories}
    tagger=${tagger:-$root/releases/$live/tagger.model}
  fi
  compgen -G "$categories/*.amcats" >/dev/null || die "no categorical models (*.amcats) in $categories"
  [[ -f $tagger ]] || die "no query tagger at $tagger"
  categories=$(realpath "$categories")
  tagger=$(realpath "$tagger")
  extra_args+=(--categories="$categories" --tagger="$tagger")
  extract=$(realpath "$extract")
  wof=$(realpath "$wof")
  local qrank_args=()
  if [[ -f $qrank ]]; then
    qrank_args=(--qrank="$(realpath "$qrank")")
  else
    echo "No QRank at $qrank; POIs' Wikidata entities will all count the same." >&2
  fi
  command -v dagger >/dev/null || die "dagger isn't installed"
  command -v podman >/dev/null || die "podman isn't installed"
  lock_releases
  forward_db

  # Without diffs up to the extract's timestamp, the database doesn't know
  # some of its POI versions, and those POIs would get no identifiers.
  echo "Checking the POI database is caught up to $(basename "$extract")..."
  local covers extract_ts sequence checkpoint_ts
  covers=$(DATABASE_URL=$(database_url "127.0.0.1:$db_port") podman run --rm --pull=newer --network=host \
    --security-opt label=disable -e DATABASE_URL -v "$extract:/input.osm.pbf:ro" \
    "$(image cardinal-poi-service)" covers --input=/input.osm.pbf)
  read -r extract_ts sequence checkpoint_ts <<<"$covers"

  if [[ -z $id ]]; then
    # planet-261005.osm.pbf -> planet-20261005, washington-latest.osm.pbf -> washington-20261101
    id=$(basename "$extract" .osm.pbf | sed -E 's/-(latest|[0-9]{6,8})$//')-$(date -u -d "$extract_ts" +%Y%m%d)
  fi
  valid_id "$id" || die "invalid release ID: $id"
  local dest=$root/releases/$id partial=$root/releases/.$id.partial
  [[ ! -e $dest ]] || die "$dest already exists; pass --id"
  rm -rf "$partial"

  echo "Building $id from $(basename "$extract") with POI identifiers at diff $sequence..."
  (cd "$repo" && DATABASE_URL=$(database_url "database:$db_port") dagger -m data_pipeline api call build \
    --osm="$extract" --wof-db="$wof" \
    --database-url=env://DATABASE_URL --database="tcp://localhost:$db_port" \
    "${qrank_args[@]}" "${extra_args[@]}" -o "$partial")
  [[ -s $partial/tiles.mbtiles ]] || die "the build has no tiles.mbtiles; left in $partial"
  [[ -f $partial/index/meta.json ]] || die "the build has no geocoder index; left in $partial"
  [[ -f $partial/tagger.model ]] || die "the build has no query tagger; left in $partial"
  local schema_version
  schema_version=$(jq -er .schema_version "$partial/index/airmail-index.json") ||
    die "the geocoder index has no schema version; left in $partial"
  # Pods run as nobody.
  chmod -R a+rX "$partial"

  local commit dirty=false
  commit=$(git -C "$repo" rev-parse HEAD)
  [[ -z $(git -C "$repo" status --porcelain) ]] || dirty=true
  jq -n \
    --arg id "$id" \
    --arg built_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg path "$extract" \
    --argjson size "$(stat -c %s "$extract")" \
    --arg extract_ts "$extract_ts" \
    --argjson sequence "$sequence" \
    --arg checkpoint_ts "$checkpoint_ts" \
    --arg commit "$commit" \
    --argjson dirty "$dirty" \
    --argjson schema_version "$schema_version" \
    --argjson qrank "$([[ ${#qrank_args[@]} -gt 0 ]] && echo true || echo false)" \
    --arg categories "$categories" \
    --arg tagger "$tagger" \
    '{id: $id, built_at: $built_at,
      extract: {path: $path, size: $size, replication_timestamp: $extract_ts},
      poi_checkpoint: {sequence: $sequence, timestamp: $checkpoint_ts},
      index: {schema_version: $schema_version, qrank: $qrank},
      models: {categories: $categories, tagger: $tagger},
      git: {commit: $commit, dirty: $dirty}}' >"$partial/manifest.json"
  chmod a+r "$partial/manifest.json"
  mv "$partial" "$dest"
  echo "Built $dest. Serve it with:"
  echo "  k8s/release.sh promote $id"
}

cmd_promote() {
  local id='' query=park
  while [[ $# -gt 0 ]]; do
    case $1 in
      --query) query=$2; shift 2 ;;
      -*) usage ;;
      *) [[ -z $id ]] || usage; id=$1; shift ;;
    esac
  done
  [[ -n $id ]] || usage
  valid_id "$id" || die "invalid release ID: $id"
  [[ -f $root/releases/$id/manifest.json ]] || die "$id isn't a finished release in $root/releases"
  # The geocoder won't start without them.
  [[ -f $root/releases/$id/tagger.model ]] || die "$id has no tagger.model"
  compgen -G "$root/releases/$id/categories/*.amcats" >/dev/null || die "$id has no categorical models in categories/"

  local previous
  previous=$(live_release)
  require_site
  echo "MAP_RELEASE=$id" >"$releases_env"
  kubectl apply -k "$site" >/dev/null
  echo "Rolling out $id..."
  for deploy in martin geocoder; do
    kubectl -n "$ns" rollout status "deploy/$deploy" --timeout=600s >/dev/null || {
      echo "deploy/$deploy didn't become ready; the previous pods are still serving." >&2
      [[ -z $previous ]] || echo "Go back with: k8s/release.sh promote $previous" >&2
      exit 1
    }
  done

  local host
  host=$(kubectl -n "$ns" get ingress cardinal -o jsonpath='{.status.loadBalancer.ingress[0].hostname}')
  if [[ -z $host ]]; then
    echo "Serving $id. The ingress has no hostname yet, so the public URL wasn't checked."
  else
    curl -fsS -m 20 "https://$host/tiles/catalog" | jq -e '.tiles.basemap' >/dev/null ||
      die "https://$host/tiles/catalog doesn't list basemap"
    curl -fsS -m 20 -G "https://$host/geocoder/v1/search" --data-urlencode "q=$query" | jq -e '.results | length > 0' >/dev/null ||
      die "https://$host/geocoder/v1/search found nothing for \"$query\"; try another --query"
    echo "Serving $id at https://$host/tiles/basemap and https://$host/geocoder/v1/search"
  fi
}

# Finished releases, newest first, as "built_at id" lines.
releases() {
  local manifest
  for manifest in "$root"/releases/*/manifest.json; do
    [[ -e $manifest ]] || continue
    jq -r '"\(.built_at) \(.id)"' "$manifest"
  done | sort -r
}

cmd_list() {
  [[ $# -eq 0 ]] || usage
  local live built id
  live=$(live_release)
  while read -r built id; do
    printf '%s  %s%s\n' "$built" "$id" "$([[ $id == "$live" ]] && echo '  (live)')"
  done < <(releases)
}

cmd_prune() {
  local keep=2 yes=false
  while [[ $# -gt 0 ]]; do
    case $1 in
      --keep) keep=$2; shift 2 ;;
      --yes) yes=true; shift ;;
      *) usage ;;
    esac
  done
  [[ $keep =~ ^[0-9]+$ ]] || usage
  lock_releases
  local live pinned n=0 built id doomed=()
  live=$(live_release)
  pinned=$(site_release)
  while read -r built id; do
    n=$((n + 1))
    if [[ $n -gt $keep && $id != "$live" && $id != "$pinned" ]]; then
      doomed+=("$root/releases/$id")
    fi
  done < <(releases)
  # Holding the lock means no build is writing these.
  for partial in "$root"/releases/.*.partial; do
    [[ -e $partial ]] && doomed+=("$partial")
  done
  if [[ ${#doomed[@]} -eq 0 ]]; then
    echo "Nothing to prune."
    return
  fi
  printf 'Deleting:\n'
  printf '  %s\n' "${doomed[@]}"
  if ! $yes; then
    read -r -p "Continue? [y/N] " answer
    [[ $answer == [yY] ]] || exit 1
  fi
  rm -rf "${doomed[@]}"
}

[[ $# -ge 1 ]] || usage
command=$1
shift
case $command in
  init) cmd_init "$@" ;;
  bootstrap-pois) cmd_bootstrap_pois "$@" ;;
  build) cmd_build "$@" ;;
  promote) cmd_promote "$@" ;;
  list) cmd_list "$@" ;;
  prune) cmd_prune "$@" ;;
  *) usage ;;
esac
