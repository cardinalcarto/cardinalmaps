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
#
# Environment:
#   PASSTHROUGH=loki-thor|thor-odin
#                runs valhalla-microservice's passthrough example at that boundary instead of a tap
#                (build it with: cargo build --release -p valhalla-microservice --example passthrough)
#   ODIN=illuvatar
#                runs illuvatar-service in place of mainline's Odin worker
#                (build it with: cargo build --release -p illuvatar-service)
#   LOKI=valinor
#                runs valinor-loki-service in place of mainline's Loki worker,
#                with the build's valhalla.json and tiles.tar
#                (build it with: cargo build --release -p valinor-loki-service)
#   THOR=valinor
#                runs valinor-thor-service in place of mainline's Thor worker,
#                with the build's valhalla.json and tiles.tar
#                (build it with: cargo build --release -p valinor-thor-service)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
image="$(cat "$here/valhalla-image")"
runtime="${CONTAINER:-podman}"
tap="$here/../../target/release/valinor-tap"
passthrough="$here/../../target/release/examples/passthrough"
illuvatar="$here/../../target/release/illuvatar-service"
valinor_loki="$here/../../target/release/valinor-loki-service"
valinor_thor="$here/../../target/release/valinor-thor-service"
splice="${PASSTHROUGH:-}"
odin="${ODIN:-mainline}"
loki="${LOKI:-mainline}"
thor="${THOR:-mainline}"
case "$splice" in
	"" | loki-thor | thor-odin) ;;
	*)
		echo "PASSTHROUGH must be loki-thor or thor-odin, not $splice" >&2
		exit 1
		;;
esac
case "$odin" in
	mainline | illuvatar) ;;
	*)
		echo "ODIN must be mainline or illuvatar, not $odin" >&2
		exit 1
		;;
esac
case "$loki" in
	mainline | valinor) ;;
	*)
		echo "LOKI must be mainline or valinor, not $loki" >&2
		exit 1
		;;
esac
case "$thor" in
	mainline | valinor) ;;
	*)
		echo "THOR must be mainline or valinor, not $thor" >&2
		exit 1
		;;
esac

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
[ -z "$splice" ] || [ -x "$passthrough" ] || {
	echo "missing $passthrough; run: cargo build --release -p valhalla-microservice --example passthrough" >&2
	exit 1
}
[ "$odin" = mainline ] || [ -x "$illuvatar" ] || {
	echo "missing $illuvatar; run: cargo build --release -p illuvatar-service" >&2
	exit 1
}
[ "$loki" = mainline ] || [ -x "$valinor_loki" ] || {
	echo "missing $valinor_loki; run: cargo build --release -p valinor-loki-service" >&2
	exit 1
}
[ "$thor" = mainline ] || [ -x "$valinor_thor" ] || {
	echo "missing $valinor_thor; run: cargo build --release -p valinor-thor-service" >&2
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
	-e "ODIN=$odin" \
	-e "LOKI=$loki" \
	-e "THOR=$thor" \
	"$image" /oracle/serve-staged.sh /out/valhalla.json "$port" >/dev/null

# Wait for the proxies the taps connect to.
for _ in $(seq 60); do
	[ -S "$ipc/tap_thor_out" ] && [ -S "$ipc/tap_odin_out" ] && [ -S "$ipc/thor_in" ] && [ -S "$ipc/odin_in" ] && break
	sleep 0.5
done

# Starts the tap, or the passthrough worker if it is spliced in here.
boundary() {
	local name="$1" upstream="$2" downstream="$3"
	if [ "$splice" = "$name" ]; then
		"$passthrough" --upstream "$upstream" --downstream "$downstream" \
			--loopback "ipc://$ipc/loopback" --interrupt "ipc://$ipc/interrupt" &
	else
		"$tap" --upstream "$upstream" --downstream "$downstream" --out-dir "$capture/$name" &
	fi
	pids+=($!)
}
boundary loki-thor "ipc://$ipc/tap_thor_out" "ipc://$ipc/thor_in"
boundary thor-odin "ipc://$ipc/tap_odin_out" "ipc://$ipc/odin_in"
if [ "$loki" = valinor ]; then
	"$valinor_loki" --config "$build/valhalla.json" --tiles "$build/tiles.tar" \
		--upstream-socket-endpoint "ipc://$ipc/loki_out" \
		--downstream-socket-endpoint "ipc://$ipc/tap_thor_in" \
		--loopback-socket-endpoint "ipc://$ipc/loopback" \
		--interrupt-socket-endpoint "ipc://$ipc/interrupt" &
	pids+=($!)
fi
if [ "$thor" = valinor ]; then
	"$valinor_thor" --config "$build/valhalla.json" --tiles "$build/tiles.tar" \
		--upstream-socket-endpoint "ipc://$ipc/thor_out" \
		--downstream-socket-endpoint "ipc://$ipc/tap_odin_in" \
		--loopback-socket-endpoint "ipc://$ipc/loopback" \
		--interrupt-socket-endpoint "ipc://$ipc/interrupt" &
	pids+=($!)
fi
if [ "$odin" = illuvatar ]; then
	"$illuvatar" --upstream-socket-endpoint "ipc://$ipc/odin_out" \
		--loopback-socket-endpoint "ipc://$ipc/loopback" \
		--interrupt-socket-endpoint "ipc://$ipc/interrupt" &
	pids+=($!)
fi

for _ in $(seq 60); do
	curl -fs "localhost:$port/status" >/dev/null && break
	sleep 0.5
done
curl -fs "localhost:$port/status" >/dev/null || {
	echo "mainline did not come up; container logs:" >&2
	"$runtime" logs "$name" >&2
	exit 1
}
echo "ready: http://localhost:$port (captures in $capture${splice:+; passthrough at $splice}; loki: $loki; thor: $thor; odin: $odin)"

# Exit (and clean up) if the container or either tap stops.
"$runtime" wait "$name" >/dev/null &
pids+=($!)
wait -n
echo "a component exited; shutting down" >&2
