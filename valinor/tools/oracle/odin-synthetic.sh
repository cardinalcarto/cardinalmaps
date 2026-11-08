#!/bin/sh
# Regenerates illuvatar's synthetic Odin fixtures: hand-built trips run through mainline's Odin.
#
# Usage: odin-synthetic.sh
#
# Writes the inputs (illuvatar/fixtures/synthetic/<case>/thor-odin.bin) from the cases in
# illuvatar/src/synthetic.rs, then runs mainline's DirectionsBuilder on each with the `odin` helper
# (tools/oracle/cpp/odin.cc, built by tools/oracle/cpp/build.sh in the dev image),
# writing the reference (response.bin: the Api with directions and the updated trip,
# or "error: <what>" if mainline throws).
# Cases whose names end in _markup run with phoneme markup enabled.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
root="$here/../.."
runtime="${CONTAINER:-podman}"
image="${DEV_IMAGE:-localhost/valinor-valhalla-dev:3.9.1}"

[ -x "$here/cpp/odin" ] || {
	echo "missing $here/cpp/odin; run: $runtime run --rm -v \"\$PWD/tools/oracle/cpp:/w:z\" $image /w/build.sh" >&2
	exit 1
}

cargo test --manifest-path "$root/Cargo.toml" -p illuvatar --lib -- --ignored --exact synthetic::write_synthetic_inputs

plain=""
markup=""
for case_dir in "$root"/illuvatar/fixtures/synthetic/*; do
	name="$(basename "$case_dir")"
	pair="/fx/$name/thor-odin.bin /fx/$name/response.bin"
	case "$name" in
		*_markup) markup="$markup $pair" ;;
		*) plain="$plain $pair" ;;
	esac
done

# shellcheck disable=SC2086 # the pairs are meant to split into arguments
"$runtime" run --rm -v "$here/cpp:/w:ro,z" -v "$root/illuvatar/fixtures/synthetic:/fx:z" "$image" \
	sh -c "/w/odin $plain && if [ -n '$markup' ]; then /w/odin --markup $markup; fi"
