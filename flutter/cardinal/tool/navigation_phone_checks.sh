#!/usr/bin/env bash
# Builds a profile APK that runs on-device navigation checks on a phone.
#
#   tool/navigation_phone_checks.sh OUT_APK CHECKS [--defines FILE] [--seconds N]
#
# CHECKS is a comma-separated list of `latency`, `voice` and `screenoff`, as
# `tool/nav_phone/navigation_phone_checks.dart` describes them. `voice` builds with
# CARDINAL_SIMULATE_NAVIGATION=true and `screenoff` without, so they can't share a
# build. --defines is as for --dart-define-from-file (default dart_defines.json);
# --seconds is how long the screen-off trip runs (default 300).
#
# It only builds. Install the APK with `adb install -r OUT_APK`, which keeps the app's
# data and offline regions, launch it (`adb shell am start -n
# com.cardinalcarto.cardinal/.MainActivity`), and read `adb logcat -s flutter` for the
# `navphone:` lines until `navphone: done`. Never run it through `flutter drive`, which
# uninstalls the app when it ends. Afterwards, install a plain profile build again.
set -euo pipefail

cd "$(dirname "$0")/.."
usage() { sed -n '2,16p' "$0" | sed 's/^# \?//' >&2; exit 2; }
[[ $# -ge 2 && $1 != --* ]] || usage
out=$(realpath -m "$1")
checks=$2
shift 2
defines=dart_defines.json seconds=300
while [[ $# -gt 0 ]]; do
  [[ $# -ge 2 ]] || usage
  case $1 in
    --defines) defines=$2 ;;
    --seconds) seconds=$2 ;;
    *) usage ;;
  esac
  shift 2
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

python3 -I - "$defines" "$work/defines.json" "$checks" "$seconds" <<'PY'
import json, sys
source, target, checks, seconds = sys.argv[1:]
wanted = set(checks.split(","))
if not wanted <= {"latency", "voice", "screenoff"} or {"voice", "screenoff"} <= wanted:
    sys.exit(f"checks are latency, voice and screenoff, voice and screenoff apart: {checks}")
defines = json.load(open(source))
defines["CARDINAL_NAV_PHONE_CHECKS"] = checks
defines["CARDINAL_NAV_PHONE_SECONDS"] = seconds
if "voice" in wanted:
    defines["CARDINAL_SIMULATE_NAVIGATION"] = "true"
else:
    defines.pop("CARDINAL_SIMULATE_NAVIGATION", None)
json.dump(defines, open(target, "w"))
PY

flutter build apk --profile \
  --target tool/nav_phone/navigation_phone_checks.dart \
  --dart-define-from-file "$work/defines.json"
mkdir -p "$(dirname "$out")"
cp build/app/outputs/flutter-apk/app-profile.apk "$out"
echo "built $out"
