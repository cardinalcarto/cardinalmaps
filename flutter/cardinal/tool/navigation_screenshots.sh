#!/usr/bin/env bash
# Takes screenshots of the navigation screen on the Linux desktop, in a simulated drive
# through downtown Seattle, for reviewing how it looks.
#
#   tool/navigation_screenshots.sh OUT_DIR [--bucket DIR] [--port N] [--defines FILE]
#                                          [--shots SET]
#
# Runs `tool/screenshots/navigation_screenshots.dart`, which says what it shows and
# writes, with `flutter drive` in profile mode on the desktop (`-d linux`), never on a
# phone or emulator. It needs the Seattle region downloaded in the desktop app.
#
# --defines is as for --dart-define-from-file (default dart_defines.json); the drive is
# simulated, and with --bucket, a `cardinal-publish --target dir:` release bucket is
# served on 127.0.0.1:--port (default 18090) as the app's release URL, so the app
# checks for updates against the data it has. --shots picks which screenshots: banner
# (the default), progress, buttons, steps, status or guidance, as the Dart file lists
# them.
#
# Nothing is tapped or typed into the desktop: the app's own code moves it. It adds one
# place to the app's store and deletes it by its ID at the end.
set -euo pipefail

cd "$(dirname "$0")/.."
usage() { sed -n '2,20p' "$0" | sed 's/^# \?//' >&2; exit 2; }
[[ $# -ge 1 && $1 != --* ]] || usage
out=$(realpath -m "$1")
shift
bucket= port=18090 defines=dart_defines.json shots=banner
while [[ $# -gt 0 ]]; do
  [[ $# -ge 2 ]] || usage
  case $1 in
    --bucket) bucket=$(realpath "$2") ;;
    --port) port=$2 ;;
    --defines) defines=$2 ;;
    --shots) shots=$2 ;;
    *) usage ;;
  esac
  shift 2
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

python3 -I - "$defines" "$work/defines.json" "$out" "$bucket" "$port" "$shots" <<'PY'
import json, sys
source, target, out, bucket, port, shots = sys.argv[1:]
defines = json.load(open(source))
defines["CARDINAL_SIMULATE_NAVIGATION"] = "true"
defines["CARDINAL_SCREENSHOTS_DIR"] = out
defines["CARDINAL_SCREENSHOTS_SET"] = shots
if bucket:
    defines["CARDINAL_SCREENSHOTS_BUCKET"] = bucket
    defines["CARDINAL_RELEASES_URL"] = f"http://127.0.0.1:{port}/releases"
json.dump(defines, open(target, "w"))
PY

mkdir -p "$out"
flutter drive --profile -d linux \
  --driver test_driver/integration_test.dart \
  --target tool/screenshots/navigation_screenshots.dart \
  --dart-define-from-file "$work/defines.json"
