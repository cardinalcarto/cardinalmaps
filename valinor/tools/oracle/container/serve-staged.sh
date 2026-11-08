#!/bin/bash
# Runs inside the pinned mainline Valhalla image (see ../serve-staged.sh).
#
# Starts mainline Valhalla as separate processes (httpd, one proxy per stage, one worker per stage)
# with two extra proxies spliced in so taps outside the container can record the messages
# between Loki and Thor, and between Thor and Odin:
#
#   httpd -> loki -> [tap_thor] -> thor -> [tap_odin] -> odin -> loopback
#
# Each worker reads its upstream and downstream endpoints from its config,
# so each gets its own config with the proxies rewired.
# All sockets live in /ipc, which the host mounts so the taps can reach them.
#
# Usage: serve-staged.sh <config: valhalla.json built by build.sh> <port>
#
# With ODIN=illuvatar, no Odin worker is started; one outside the container serves odin_out.
# With LOKI=valinor, no Loki worker is started; one outside the container serves loki_out.
# With THOR=valinor, no Thor worker is started; one outside the container serves thor_out.
set -euo pipefail

config="$1"
port="$2"
ipc="ipc:///ipc"
cfg_dir="$(mktemp -d)"

python3 -I - "$config" "$cfg_dir" "$ipc" <<'EOF'
import json, sys
base_path, out_dir, ipc = sys.argv[1:4]
base = json.load(open(base_path))
base["httpd"]["service"]["loopback"] = f"{ipc}/loopback"
base["httpd"]["service"]["interrupt"] = f"{ipc}/interrupt"
base["loki"]["service"]["proxy"] = f"{ipc}/loki"
# Loki pushes to thor.service.proxy + "_in"; Thor reads from thor.service.proxy + "_out".
# Thor pushes to odin.service.proxy + "_in"; Odin reads from odin.service.proxy + "_out".
wiring = {
    "loki": {"thor": f"{ipc}/tap_thor", "odin": f"{ipc}/odin"},
    "thor": {"thor": f"{ipc}/thor", "odin": f"{ipc}/tap_odin"},
    "odin": {"thor": f"{ipc}/thor", "odin": f"{ipc}/odin"},
}
for worker, proxies in wiring.items():
    cfg = json.loads(json.dumps(base))
    cfg["thor"]["service"]["proxy"] = proxies["thor"]
    cfg["odin"]["service"]["proxy"] = proxies["odin"]
    json.dump(cfg, open(f"{out_dir}/{worker}.json", "w"), indent=2)
EOF

rm -f /ipc/*

prime_httpd "tcp://*:$port" "$ipc/loki_in" "$ipc/loopback" "$ipc/interrupt" &
for proxy in loki tap_thor thor tap_odin odin; do
	prime_proxyd "$ipc/${proxy}_in" "$ipc/${proxy}_out" &
done
if [ "${LOKI:-mainline}" = mainline ]; then
	valhalla_loki_worker "$cfg_dir/loki.json" &
fi
if [ "${THOR:-mainline}" = mainline ]; then
	valhalla_thor_worker "$cfg_dir/thor.json" &
fi
if [ "${ODIN:-mainline}" = mainline ]; then
	valhalla_odin_worker "$cfg_dir/odin.json" &
fi

# If any process exits, stop everything so a half-working chain is never left running.
wait -n
echo "a mainline process exited; shutting down" >&2
kill 0
