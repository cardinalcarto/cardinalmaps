# Valinor tap

A pass-through worker for a mainline Valhalla service chain.
It sits between two `prime_proxyd` proxies,
records every message that flows through,
and forwards it downstream byte for byte.

This is how we capture reference data at internal stage boundaries
(for example the `Api` message Thor hands to Odin),
which mainline's HTTP API never returns.
See [`tools/oracle`](../tools/oracle) for the scripts that run mainline in stages with taps in between.

## Why raw bytes

The tap deliberately does not decode the protobuf payload.
Re-encoding through generated code would silently drop any field the code doesn't know about,
and the reference data must not depend on Valinor's own (possibly outdated) message definitions.

## Output

Each message becomes one file in the output directory:
`<sequence>-<request id>.<frame>.bin`,
where frame 0 is `prime_server`'s 12-byte request info struct
and frames 1 and up are the payload (for Valhalla, a single serialized `valhalla.Api`).

```shell
valinor-tap \
  --upstream ipc:///ipc/tap_odin_out \
  --downstream ipc:///ipc/odin_in \
  --out-dir captures/thor-to-odin
```
