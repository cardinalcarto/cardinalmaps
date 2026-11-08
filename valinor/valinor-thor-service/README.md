# Valinor Thor service

A replacement for mainline Valhalla's Thor worker,
built on [`valinor-thor`](../valinor-thor) and [`valhalla-microservice`](../valhalla-microservice).

It takes the requests Loki has correlated, finds their paths,
and forwards them to Odin, or answers the client itself with an error
(442 when no path is found, as mainline does).

Trip building (mainline's `TripLegBuilder`) isn't written yet,
so for now a route that path search finds is answered with error 107 (not implemented);
the other actions mainline's Thor serves (`optimized_route`, matrices, isochrones,
map matching, expansion, and centroids) are error 107 too,
and status requests go on to Odin, as in mainline.

```shell
valinor-thor-service --config valhalla.json --tiles tiles.tar \
  --upstream-socket-endpoint ipc:///tmp/thor_out \
  --downstream-socket-endpoint ipc:///tmp/odin_in \
  --loopback-socket-endpoint ipc:///tmp/loopback \
  --interrupt-socket-endpoint ipc:///tmp/interrupt
```

`--config` is a mainline `valhalla.json`, of which Thor reads its search parameters
and service limits (see `valinor_thor::ThorConfig`); without one, mainline's defaults apply.
`--tiles` is a tile extract or a tile directory.
Parsed tiles are cached (`--tile-cache-size` of them) across requests,
and the searches' storage is kept between requests.
Searches stop when the server interrupts their request.

Every flag can also be set by an environment variable of the same name
(`TILES`, `UPSTREAM_SOCKET_ENDPOINT`, and so on), and logging follows `RUST_LOG`.
