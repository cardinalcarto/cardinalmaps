#!/bin/bash
# Checks that valinor-loki-service in place of mainline's Loki worker changes nothing a client,
# Thor, or Odin can see.
#
# Usage: check-loki.sh <build-dir> <corpus.json> <work-dir> [port]
#
#   build-dir  output of `build-tiles.sh tiles` for the corpus's region
#   corpus     a corpus file from corpus/
#   work-dir   where the two runs are captured (<work-dir>/baseline and <work-dir>/valinor)
#   port       HTTP port on localhost (default 8022)
#
# Runs the corpus against plain staged mainline, then again with LOKI=valinor
# (see serve-staged.sh), and compares the two with compare-runs.py.
# Build first:
#   cargo build --release -p valinor-tap
#   cargo build --release -p valinor-loki-service
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
build="$1"
corpus="$2"
work="$3"
port="${4:-8022}"

server=""
stop() {
	if [ -n "$server" ]; then
		kill "$server" 2>/dev/null || true
		wait "$server" 2>/dev/null || true
		server=""
	fi
}
trap stop EXIT INT TERM

# Serves mainline (with LOKI set as given) and captures the corpus into <work>/<name>.
run() {
	local name="$1" loki="$2"
	rm -rf "${work:?}/$name" "${work:?}/$name-taps"
	mkdir -p "$work/$name"
	LOKI="$loki" "$here/serve-staged.sh" "$build" "$work/$name-taps" "$port" \
		>"$work/$name.log" 2>&1 &
	server=$!
	for _ in $(seq 120); do
		grep -q '^ready:' "$work/$name.log" && break
		kill -0 "$server" 2>/dev/null || {
			cat "$work/$name.log" >&2
			exit 1
		}
		sleep 0.5
	done
	grep -q '^ready:' "$work/$name.log" || {
		echo "$name: mainline did not come up" >&2
		cat "$work/$name.log" >&2
		exit 1
	}
	python3 -I "$here/capture.py" "$corpus" "$work/$name-taps" "$work/$name" \
		--url "http://localhost:$port" --protos "$here/../../valhalla-proto/proto"
	stop
}

run baseline mainline
run valinor valinor
python3 -I "$here/compare-runs.py" "$work/baseline" "$work/valinor" \
	--protos "$here/../../valhalla-proto/proto"
