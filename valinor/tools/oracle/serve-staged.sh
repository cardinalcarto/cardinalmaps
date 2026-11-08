#!/bin/bash
# Serves mainline Valhalla in separate stages with valinor-tap recording between them.
#
# Usage: serve-staged.sh <build-dir> <capture-dir> [port] [ipc-dir]
#
#   build-dir    output of `build-tiles.sh tiles` (holds valhalla.json, tiles.tar, ...)
#   capture-dir  taps write to <capture-dir>/loki-thor and <capture-dir>/thor-odin
#   port         HTTP port on localhost (default 8002)
#   ipc-dir      host directory for the ZeroMQ sockets shared with the container
#                (default: a fresh temp dir; Unix socket paths must stay under ~100 bytes)
#
# Runs in the foreground until interrupted, then stops the container and both taps.
# Build the tap first: cargo build --release -p valinor-tap
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
image="$(cat "$here/valhalla-image")"
runtime="${CONTAINER:-podman}"
tap="$here/../../target/release/valinor-tap"

build="$(cd "$1" && pwd)"
mkdir -p "$2"
capture="$(cd "$2" && pwd)"
port="${3:-8002}"
ipc="${4:-$(mktemp -d)}"
mkdir -p "$ipc"
ipc="$(cd "$ipc" && pwd)"
name="valinor-oracle-$port"

[ -x "$tap" ] || {
	echo "missing $tap; run: cargo build --release -p valinor-tap" >&2
	exit 1
}

pids=()
cleanup() {
	trap - EXIT INT TERM
	for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
	"$runtime" rm -f -t 2 "$name" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

"$runtime" rm -f "$name" >/dev/null 2>&1 || true
# build.sh writes configs with paths under /out, so the build dir is mounted there.
"$runtime" run -d --name "$name" -p "127.0.0.1:$port:$port" \
	-v "$here/container:/oracle:ro,z" \
	-v "$build:/out:ro,z" \
	-v "$ipc:/ipc:z" \
	"$image" /oracle/serve-staged.sh /out/valhalla.json "$port" >/dev/null

# Wait for the proxies the taps connect to.
for _ in $(seq 60); do
	[ -S "$ipc/tap_thor_out" ] && [ -S "$ipc/tap_odin_out" ] && [ -S "$ipc/thor_in" ] && [ -S "$ipc/odin_in" ] && break
	sleep 0.5
done

"$tap" --upstream "ipc://$ipc/tap_thor_out" --downstream "ipc://$ipc/thor_in" \
	--out-dir "$capture/loki-thor" &
pids+=($!)
"$tap" --upstream "ipc://$ipc/tap_odin_out" --downstream "ipc://$ipc/odin_in" \
	--out-dir "$capture/thor-odin" &
pids+=($!)

for _ in $(seq 60); do
	curl -fs "localhost:$port/status" >/dev/null && break
	sleep 0.5
done
curl -fs "localhost:$port/status" >/dev/null || {
	echo "mainline did not come up; container logs:" >&2
	"$runtime" logs "$name" >&2
	exit 1
}
echo "ready: http://localhost:$port (captures in $capture)"

# Exit (and clean up) if the container or either tap stops.
"$runtime" wait "$name" >/dev/null &
pids+=($!)
wait -n
echo "a component exited; shutting down" >&2
