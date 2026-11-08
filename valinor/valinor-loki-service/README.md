# Valinor Loki service

A drop-in replacement for mainline Valhalla's Loki worker,
built on [`valinor-loki`](../valinor-loki) and [`valhalla-microservice`](../valhalla-microservice).

As in mainline, the front and Loki share a worker:
the HTTP server hands it each raw HTTP request,
which it parses into `options` (the front, in `valinor-tyr`),
checks against the service limits, and correlates to the graph.
It forwards the result to Thor, or answers the client itself
(for `/locate`, and for every error).

```shell
valinor-loki-service --config valhalla.json --tiles tiles.tar \
  --upstream-socket-endpoint ipc:///tmp/loki_out \
  --downstream-socket-endpoint ipc:///tmp/thor_in \
  --loopback-socket-endpoint ipc:///tmp/loopback \
  --interrupt-socket-endpoint ipc:///tmp/interrupt
```

`--config` is a mainline `valhalla.json`, of which Loki reads its actions, defaults,
and service limits (see `valinor_loki::LokiConfig`); without one, mainline's defaults apply.
`--tiles` is a tile extract or a tile directory.
Parsed tiles are cached (`--tile-cache-size` of them) across requests.

Every flag can also be set by an environment variable of the same name
(`TILES`, `UPSTREAM_SOCKET_ENDPOINT`, and so on), and logging follows `RUST_LOG`.
