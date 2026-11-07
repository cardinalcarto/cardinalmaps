#!/usr/bin/env bash
# Counts the frames the map misses its deadline on, on a connected Android phone.
#
#   tool/map_bench.sh [--zoom N] [--view LNG,LAT] [--style FILE] [--without casings,labels]
#                     [--defines FILE] [--label NAME]
#
# Builds a profile APK with SPARKLE_BENCH, which moves the camera around a closed square, a
# fixed step per frame, once the map has loaded (see `_benchStep` in sparkle_map.dart). So
# every run starts from the same view and draws the same frames, unlike swipes, which end in
# flings that depend on frame timing. The first of its loops loads tiles and doesn't count.
#
# Missed frames are what to compare: frame times mislead on phones whose clocks scale to
# just keep up. A frame missed its deadline if it took more than one and a half refreshes.
# Without touches the panel may stay at its lower refresh rate; the output says which.
#
# --style names a file in assets/map. --without draws that style less, to see what its
# casings or labels cost; casings at low zooms are all that shows of minor roads.
#
# Set ANDROID_SERIAL if more than one device is connected. --defines is as for
# --dart-define-from-file, and localhost servers in it are reached with `adb reverse`.
set -euo pipefail

cd "$(dirname "$0")/.."
zoom=13 view=-122.335,47.608 style=headway.json without= defines=dart_defines.json label=
while [[ $# -gt 0 ]]; do
  case $1 in
    --zoom) zoom=$2 ;;
    --view) view=$2 ;;
    --style) style=$2 ;;
    --without) without=$2 ;;
    --defines) defines=$2 ;;
    --label) label=$2 ;;
    *) sed -n '2,21p' "$0" | sed 's/^# \?//' >&2; exit 2 ;;
  esac
  shift 2
done
label=${label:-z$zoom-${without:-$style}}

work=$(mktemp -d)
bench_style=assets/map/bench-style.json
trap 'rm -rf "$work"; rm -f "$bench_style"' EXIT

python3 -I - "assets/map/$style" "$bench_style" "$without" <<'PY'
import json, sys
source, target, without = sys.argv[1:]
style = json.load(open(source))
dropped = set(filter(None, without.split(',')))
unknown = dropped - {'casings', 'labels'}
if unknown:
    sys.exit(f'--without: unknown {sorted(unknown)}')
def keep(layer):
    casing = 'casing' in layer['id'] and layer.get('source-layer') == 'transportation'
    return not ('casings' in dropped and casing or 'labels' in dropped and layer['type'] == 'symbol')
style['layers'] = list(filter(keep, style['layers']))
json.dump(style, open(target, 'w'))
PY

python3 -I - "$defines" "$work/defines.json" "$view,$zoom" <<'PY'
import json, sys
source, target, view = sys.argv[1:]
defines = json.load(open(source))
defines.update(CARDINAL_VIEW=view, CARDINAL_STYLE='bench-style.json', SPARKLE_BENCH='true')
json.dump(defines, open(target, 'w'))
PY

flutter build apk --profile --target-platform android-arm64 \
  --dart-define-from-file="$work/defines.json" >"$work/build.log" 2>&1 \
  || { tail -20 "$work/build.log" >&2; exit 1; }

adb install -r build/app/outputs/flutter-apk/app-profile.apk >/dev/null
python3 -I - "$defines" <<'PY' | while read -r port; do adb reverse "tcp:$port" "tcp:$port" >/dev/null; done
import json, sys, urllib.parse
for value in json.load(open(sys.argv[1])).values():
    url = urllib.parse.urlparse(str(value))
    if url.hostname in ('localhost', '127.0.0.1') and url.port:
        print(url.port)
PY

package=com.example.cardinal
adb shell am force-stop "$package"
adb logcat -c
adb shell am start -n "$package/.MainActivity" >/dev/null
for _ in $(seq 1 90); do
  sleep 2
  adb logcat -d | grep -q 'BENCH done' && break
done
adb logcat -d >"$work/log"
grep -q 'BENCH done' "$work/log" || { echo "$label: timed out" >&2; exit 1; }

python3 -I - "$label" "$work/log" <<'PY'
import re, sys
label, path = sys.argv[1:]
text = open(path, errors='replace').read()
loops = re.findall(r'BENCH loop \d+ end \d+ ms worst (\d+) ms missed (\d+) of (\d+) at (\d+) Hz', text)[1:]
missed = [int(l[1]) for l in loops]
frames = sum(int(l[2]) for l in loops)
worst = max(int(l[0]) for l in loops)
print(f'{label}: missed {sum(missed)} of {frames} frames {missed}, worst {worst} ms, at {loops[0][3]} Hz')
PY
