#!/bin/bash
# Checks that splicing valhalla-microservice's passthrough worker into a staged mainline chain
# changes nothing a client or Odin can see.
#
# Usage: check-passthrough.sh <build-dir> <corpus.json> <work-dir> [boundary] [port]
#
#   build-dir  output of `build-tiles.sh tiles` for the corpus's region
#   corpus     a corpus file from corpus/
#   work-dir   where the two runs are captured (<work-dir>/baseline and <work-dir>/passthrough)
#   boundary   where to splice the passthrough: loki-thor (default) or thor-odin
#   port       HTTP port on localhost (default 8012)
#
# Runs the corpus against plain staged mainline, then again with the passthrough in place of
# the tap at the boundary, and compares the two with compare-runs.py.
# Build first:
#   cargo build --release -p valinor-tap
#   cargo build --release -p valhalla-microservice --example passthrough
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
build="$1"
corpus="$2"
work="$3"
boundary="${4:-loki-thor}"
port="${5:-8012}"

server=""
stop() {
	if [ -n "$server" ]; then
		kill "$server" 2>/dev/null || true
		wait "$server" 2>/dev/null || true
		server=""
	fi
}
trap stop EXIT INT TERM

# Serves mainline (with PASSTHROUGH set as given) and captures the corpus into <work>/<name>.
run() {
	local name="$1" splice="$2"
	rm -rf "${work:?}/$name" "${work:?}/$name-taps"
	mkdir -p "$work/$name"
	PASSTHROUGH="$splice" "$here/serve-staged.sh" "$build" "$work/$name-taps" "$port" \
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
		--url "http://localhost:$port"
	stop
}

run baseline ""
run passthrough "$boundary"
python3 -I "$here/compare-runs.py" "$work/baseline" "$work/passthrough" \
	--protos "$here/../../valhalla-proto/proto"
