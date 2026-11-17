#!/usr/bin/env bash
# Takes a screenshot of offline search's results in the app on the Linux desktop, over a
# search index built on the desktop, for reviewing how they look.
#
#   tool/offline_search_screenshot.sh OUT_DIR --index DIR --basemap FILE
#       [--query TEXT] [--name NAME] [--view lng,lat,zoom] [--defines FILE]
#
# The index is one `offline_index build` made (crates/geocoder_flutter/examples), and the
# basemap the MBTiles file it was built from, which results' features are read from.
# Runs `tool/screenshots/offline_search_screenshot.dart` with `flutter drive` in profile
# mode on the desktop (`-d linux`), never on a phone or emulator, over a scratch offline
# folder whose `search/current.json` points at them. --defines is as for
# --dart-define-from-file (default: none); downloads are off (no CARDINAL_RELEASES_URL),
# so the app searches only that index. Writes OUT_DIR/NAME.png (default
# offline-search.png), of --query (default "coffee") near --view (default downtown
# Seattle).
set -euo pipefail

cd "$(dirname "$0")/.."
usage() { sed -n '2,16p' "$0" | sed 's/^# \?//' >&2; exit 2; }
[[ $# -ge 1 && $1 != --* ]] || usage
out=$(realpath -m "$1")
shift
index= basemap= query=coffee name=offline-search view=-122.3375,47.6100,15 defines=
while [[ $# -gt 0 ]]; do
  [[ $# -ge 2 ]] || usage
  case $1 in
    --index) index=$(realpath "$2") ;;
    --basemap) basemap=$(realpath "$2") ;;
    --query) query=$2 ;;
    --name) name=$2 ;;
    --view) view=$2 ;;
    --defines) defines=$2 ;;
    *) usage ;;
  esac
  shift 2
done
[[ -n $index && -n $basemap ]] || usage

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/offline/search" "$out"

python3 -I - "$defines" "$work/defines.json" "$work/offline" "$index" "$basemap" \
  "$out" "$query" "$name" "$view" <<'PY'
import json, sys
source, target, root, index, basemap, out, query, name, view = sys.argv[1:]
defines = json.load(open(source)) if source else {}
defines.pop("CARDINAL_RELEASES_URL", None)
defines.update({
    "CARDINAL_OFFLINE_ROOT": root,
    "CARDINAL_VIEW": view,
    "CARDINAL_SCREENSHOTS_DIR": out,
    "CARDINAL_SCREENSHOTS_QUERY": query,
    "CARDINAL_SCREENSHOTS_NAME": name,
})
json.dump(defines, open(target, "w"))
json.dump({"dir": index, "sources": {"basemap": basemap}},
          open(f"{root}/search/current.json", "w"))
PY

flutter drive --profile -d linux \
  --driver test_driver/integration_test.dart \
  --target tool/screenshots/offline_search_screenshot.dart \
  --dart-define-from-file "$work/defines.json"
