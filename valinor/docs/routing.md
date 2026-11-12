# Routing architecture

This document describes the Valhalla-compatible router we are building on Valinor
for Cardinal Maps.
It is the shared contract for everyone (human or agent) working on a stage,
so changes to it should be deliberate and discussed.

## Goals and constraints

- **Valhalla-compatible API.**
  Requests and responses (Valhalla JSON and OSRM format with banner and voice instructions)
  match mainline closely enough that existing clients, Ferrostar in particular, work unchanged.
- **Pinned to Valhalla 3.9.1.**
  Tile format, protobuf definitions, microservice protocol, narrative locales,
  and the reference oracle all follow 3.9.1.
  Tiles are built by mainline's own tools; we do not write a tile builder.
- **The same router on the server and offline.**
  Both run the stage chain in process through `valinor-router`:
  the server behind an HTTP service (axum),
  and the app through flutter_rust_bridge (see [flutter-api.md](flutter-api.md)).
  There is no HTTP server or ZeroMQ on the device.
- **No panics on decoding paths.**
  Corrupt, truncated, or unexpected tile data is an error, never a crash.
  This matters most offline, where a bad download must not take the app down.
- **Out of scope:** live traffic.
  Static predicted speeds are in scope.
  Transit is separate from these stages; see [transit.md](transit.md).

## Stages

Each stage mirrors a mainline Valhalla service,
so its boundaries are the `valhalla.Api` messages that mainline passes between services.

```text
request ──► front ──► Loki ──► Thor ──► Odin ──► Tyr ──► Valhalla JSON / OSRM format
                        └──── Sif (costing library) ───┘
```

| Stage | Crate | Input | Output |
|---|---|---|---|
| Front | `valinor-tyr` | HTTP request (action + JSON) | `Api` with `options` |
| Loki | `valinor-loki` | `Api` with `options` | `Api` with correlated locations |
| Thor | `valinor-thor` | `Api` with correlated locations | `Api` with `trip` |
| Odin | `illuvatar` | `Api` with `trip` | `Api` with `directions` |
| Tyr | `valinor-tyr` | `Api` with `directions` | serialized response |
| Sif | `valinor-sif` | edges, nodes, and costing options | costs, access, and filters |

`valinor-router` chains the stages in process, for both the server and the app.
`illuvatar-service`, `valinor-loki-service`, and `valinor-thor-service` wrap the same stages
as ZeroMQ workers (via `valhalla-microservice`),
so they can be spliced into mainline's staged chain for the oracle.

Front and Loki share a worker, as in mainline:
the HTTP server hands Loki's worker the raw HTTP request,
which it parses into `options` (front) before correlating locations (Loki).
So the front stage has no ZeroMQ boundary of its own,
and the Loki worker needs its own entry point rather than `WorkerResult::from_stage`.
The front stage's reference output is the `options` in the Loki→Thor capture.

### Stage contract

- A stage is a synchronous function that updates the request in place:
  `fn(&mut Api) -> Result<(), ValhallaError>`.
  This is what mainline's workers do,
  and it means an error is serialized from the same request it happened in:
  its format picks the error body,
  and any warnings or statistics the stage added before failing are kept.
  `WorkerResult::from_stage` adapts a stage to a ZeroMQ worker.
  Stages never use tokio or ZeroMQ; transports live in the wrappers.
- `ValhallaError` carries mainline's numeric error code (for example 171,
  "No suitable edges near location"), the HTTP status, and the message.
  Tyr serializes it exactly as mainline does for the requested format,
  so clients see the same error JSON:
  `error_code`, `error`, `status_code`, and `status` for Valhalla JSON,
  but OSRM-style `code` and `message` (for example `NoSegment`) for OSRM format,
  which is what Ferrostar sees.
- Stages must accept everything mainline's equivalent stage accepts at that boundary,
  including fields they do not use, and pass unknown data through untouched.

## Costing (Sif)

`valinor-sif` is mainline's Sif: the costing options parser (which the front calls)
and the cost model that Loki and Thor call on their hot paths.

- `Costing` is the interface (mainline's `DynamicCost` virtual methods):
  `allowed` and `allowed_reverse` (access, U-turns, simple turn restrictions,
  exclusions, and access restrictions at a time), `edge_cost` and `partial_edge_cost`,
  `transition_cost` and `transition_cost_reverse`, `edge_filter` and `node_allowed`
  (Loki's filters), and the A* factor.
  `DynamicCost` holds what every costing shares, including complex restrictions
  (`restricted`), hierarchy limits, and user avoid edges.
- `AutoCost`, `PedestrianCost` (with hiking, wheelchair, and blind pedestrians),
  and `BicycleCost` implement it; `AnyCosting` picks one from the request's options
  and dispatches with a `match`.
  Other costings return `CostingError::NotImplemented` until they are added;
  each is a new type implementing `Costing` and a new `AnyCosting` variant.
- `Label` is what costing reads about the predecessor edge.
  Thor implements it over its own edge labels, or embeds `EdgeLabel`;
  the `label` module documents where each value comes from.
- `TimeInfo` is mainline's time along a route, and `recost_forward` recosts a known path
  (as Thor does for bidirectional searches).
  Mainline's times are 27 seconds late (it counts leap seconds and then reads the count
  as Unix time); `TimeInfo` reproduces that, since it shifts every time-dependent cost.
- Live traffic is out of scope: no edge is closed, and live speeds are never used.
  Conditional speed limits aren't costed (mainline 3.9.1 only reports them).

Results match mainline bit for bit: see `tools/oracle/README.md` for the oracles
(a C++ helper that runs mainline's costings, and the captured trips).

## Location correlation (Loki)

`valinor-loki` is mainline's Loki: the search for the edges near each location
(`loki::Search`, with mainline's reach expansions) and the actions that check a request
against the service limits before they search (route and centroid, locate,
the matrix actions, the trace actions, and status).

- `Loki` holds the configuration (`LokiConfig`, the Loki part of `valhalla.json`),
  the tile provider, and the tile set's `ConnectivityMap` (for error 170).
  Each action is a stage, `fn(&mut Api) -> Result<_, ValhallaError>`;
  `/locate` returns its response body (serialized by `valinor_tyr::tyr::serialize_locate`).
- `handle_http_request` is the worker's entry point: front and Loki share a worker,
  so it parses the server's raw HTTP request (with `valinor-tyr`'s front) and then runs Loki.
  `valinor-loki-service` wraps it as a ZeroMQ worker (with `valhalla-microservice`'s
  `build_http`); an offline router can call it directly.
- Thor gets each location's `correlation`: the candidate `edges` and the `filtered_edges`
  (ruled out by heading, preferred side, or layer) as `PathEdge`s, with graph ID,
  `percent_along` in the edge's direction, the correlated point, its distance,
  side of street, `begin_node` and `end_node`, reach in both directions, heading, names,
  and bounding circle. Exclude locations and polygons become the costing's `exclude_edges`.
- Tiles are shared through `valhalla_graphtile::tile_provider::CachingTileProvider`,
  a bounded, thread-safe cache of parsed tiles (no async runtime, so it runs on device),
  which Thor can use too.
- Costings Sif doesn't have yet, linear cost features (`cost_factor_lines`), isochrones,
  expansion, heights, transit availability, tiles, and verbose status are error 107.

Results are equivalent to mainline's, within the tolerances below,
not byte for byte: where mainline's order is a container's (equal candidates after its
`std::sort`, snapped nodes, excluded edges, its reach queue), ours is a deterministic one.
What is reproduced on purpose is what decides the result
(see the module docs of `search`, `reach`, and `worker`):
the float and double arithmetic of `midgard` (a node snap is an exact comparison),
the reach counts, and the double bucket queue of the exact reach expansion.
Bounding circles skip edges only when a circle grown by 1 cm is out of reach,
so that a different `cosf` can never make us skip an edge that mainline keeps.

## Path search (Thor)

`valinor-thor` is mainline's Thor for `route` requests, up to trip building
(`thor_worker_t::route` and `route_action.cc`).

- `Thor::route` scores Loki's candidates (`adjust_locations`: filtered candidates are used
  only if nothing else is, with warning 215, and every distance becomes a score),
  then routes each pair of locations with the algorithm mainline picks:
  bidirectional A* by default; time-dependent forward A* when the origin has a time
  (`depart_at`, `current`) and is within `max_timedep_distance` of the destination,
  or when any origin and destination candidates are the same or connected edges;
  time-dependent reverse A* when the destination has a time (`arrive_by`);
  warnings 402 and 214 when a time is dropped for distance.
  A failed search is retried with relaxed hierarchy limits, the filtered candidates,
  destination-only edges, and no not-thru pruning (warning 401),
  and so is a pedestrian route with a ferry.
  A route that fails at a through location on a small island of the graph
  is retried with only highly reachable candidates; then error 442.
- Segments are joined into legs at through and via locations
  (through locations continue on the edge they arrived on, without a U-turn),
  with mainline's edge trimming and `leg_shape_index`es, and each location gets the time
  the route reaches it (`DateTime::offset_date`, which is 27 seconds late and cut to the minute).
  Thor picks each search's time zone from its location's candidate edges, as mainline does,
  and passes it to Sif's `TimeInfo`.
- The result is `Paths`: per route (the best, then alternates) and per leg, mainline's
  `PathInfo` for each edge (elapsed and transition costs, mode, restriction),
  the algorithms, the edge trimming, and the leg's locations as trip building sees them.
  Trip building (mainline's `TripLegBuilder`) turns them into `TripLeg`s.
- A `Workspace` keeps the searches' storage (labels, queues, edge status) between requests,
  so the searches don't allocate as they go; a search can be interrupted
  (`route_interruptible`, checked every 5000 iterations as in mainline).
- `valinor-thor-service` wraps it as a ZeroMQ worker.

The searches follow mainline step by step, because what decides between nearly equal paths
is part of the result: the double bucket queue (unit buckets, last in first out within one),
single precision costs and heuristics, the hierarchy limits used up in step by both directions,
shortcuts taken in sync, the connection threshold (`threshold_delta`) and reach-based pruning,
complex restrictions across the meeting point, and the recost of bidirectional paths.
What isn't reproduced is container order that can't change a result
(the alternates' sort among equal costs, hash tables).
On the oracle corpora (see `tools/oracle/README.md`), every path, cost, error, warning,
and location time matches mainline exactly.
`optimized_route` (which needs mainline's `CostMatrix` and its `Optimizer`) is not here yet.

## Trip building (Thor)

`Thor::build_trip` is mainline's `TripLegBuilder` and the parts of `route_action.cc` that assemble
the `trip`: a route per path (the best, then each alternate) and a `TripLeg` per leg.

- Which attributes are written follows mainline's `AttributesController`: its defaults,
  the request's `filters` (`include` and `exclude`; unknown names are ignored),
  and `elevation_interval` for elevation. Ferrostar's `shape_attributes` filter gives
  per-point times, lengths, speeds, and speed limits.
- Each leg gets its locations (each keeping only the candidate edge the path uses,
  with the projected point and side of street at the ends, and each intermediate location's
  `leg_shape_index` and `distance_from_leg_origin`), a node per edge with the edge's attributes
  and the edges that meet the path there (on every level the node connects to),
  the shape with the partial edges at the ends and at intermediate locations trimmed,
  level changes, admins, the bounding box, `osm_changeset`, the summary flags,
  and `recostings`.
- Trip building asks the costing only for edge speeds and travel types;
  edge speeds by source come from `select_speed` without live traffic,
  so faded speeds (which need live traffic) are never written.
- Not here: landmarks (Valinor's tile builds have none), transit and bike share nodes,
  incidents, and traffic cuts of the shape (none without live traffic).

Mainline's arithmetic is followed step by step where it decides a value
(single precision trim distances and lengths, the polyline's rounding, headings),
and so are a few results that look odd but that Odin or clients see:
a destination without intermediate locations before it gets the `leg_shape_index` and distance
of the end of the leg's first edge (it is compared with each edge index, as intermediate
locations are); an `arrive_by` intermediate location at a node gets the start of its edge;
`edge.speed_type` is on by default (mainline's table lists it twice, and the first wins);
a message mainline creates is created even when every field in it is filtered out
(a sign, the cost of a node); and a shape point's time over the part of an edge
with no length is 0 (mainline converts an infinite time to an integer, which gives 0 on x86-64;
Ferrostar sums these into its annotations).
On the oracle corpora (see `tools/oracle/README.md`), every trip matches mainline's exactly,
apart from paths that tie at equal cost.

## The router and the server

`valinor-router` is mainline's whole server in one process.
`Router::handle` takes an HTTP request (method, target, content type, and body)
and runs the front and Loki (`valinor_loki::handle_request`),
Thor's worker (path search with a pooled `Workspace`, then trip building),
and Odin's (narration and Tyr), as the staged chain does, and answers with what mainline sends:
the status, content type, GPX disposition, and body, with errors in the request's format.
Actions Valinor doesn't have yet are error 107, as in the Thor worker.
`Router::route` is `POST /route` with a body, the one function the app calls.

- **Tile sets.** `Router::open` takes one or more `build_extract` tarballs or tile directories
  as one tile set: the first set that has a tile supplies it, and a tile no set has is
  the edge of the graph, as in mainline. Sets from different builds are refused:
  the build ID in each tile header's `checksum` must be the same in every tile read,
  which is every tile of a set of up to 4096 tiles (any region) and 4096 spread across a
  larger one (a planet), reading only headers. Opening a tarball maps it and reads its index.
- **Tile handles.** Tarball tiles are validated on first use and kept as references into the
  map, which the stages copy freely; an `Arc` per lookup, or a shared LRU's lock, made
  concurrent routes slower than one (see `valinor-router`'s `tiles` module).
- **Concurrency.** `Router` is `Send` and `Sync`; requests run on the caller's threads,
  and path searches take workspaces from a bounded pool (`RouterConfig::workspaces`).
- **Interrupts.** `handle_interruptible` stops path search and trip building when asked
  (every 5000 iterations, as mainline); Loki and Odin, which take milliseconds, don't check.

`valinor-server` serves the router over HTTP (axum), at Valhalla's paths with GET `?json=`
and POST: `valinor-server --config <valhalla.json> [--tiles <set>...] --listen <addr:port>`.
Without `--tiles`, it opens `mjolnir.tile_extract` relative to the config's directory.
Routing runs on a blocking pool behind a semaphore sized to the workspaces,
with a request timeout (504, which interrupts the search, as a client going away does)
and a body limit (413). `/status` doesn't wait for a worker, since it is the health probe.
It adds `Access-Control-Allow-Origin: *` to every response and answers methods
`prime_server` doesn't parse with its bare 501, as mainline's server does.

`valinor-router`'s `capture_tests` send every Andorra capture in `valinor-tyr/fixtures` through
the router; the end-to-end runs of `valinor-server` are in `tools/oracle/README.md`.

## The oracle

Reference data comes from mainline 3.9.1 running in stages,
with `valinor-tap` recording the messages between them.
See [`tools/oracle`](../tools/oracle/README.md).

- Tile builds: `tools/oracle/build-tiles.sh` (admins, timezones, elevation, pedestrian areas,
  and bounding circles; landmarks are currently left out because mainline's
  landmarks step produces tiles that route incorrectly).
- Staged serving with taps: `tools/oracle/serve-staged.sh`.
- Captures: `tools/oracle/capture.py` with the corpora in `tools/oracle/corpus`.
  Each case holds the request, the final response,
  and the Loki→Thor and Thor→Odin `Api` messages (raw and decoded).
  Captured data lives outside the repo;
  copy the cases a crate needs into that crate's `fixtures/` directory.

The Loki→Thor capture includes the parsed `options`,
so it is also the reference for the front stage (JSON → `options`).

## Match tolerances

"Matches mainline" means the following, per stage,
given the same input message at that boundary.

| Stage | Must match exactly | Tolerances |
|---|---|---|
| Front | Every `options` field mainline sets, including defaults it fills in | None |
| Loki | The set of candidate edges per location (graph id, side of street), and errors | `percent_along` within 1e-6; distances within 0.01 m; candidate order may differ only among equal distances (in practice every field matches, including reach, headings, and order) |
| Thor (path search) | Errors | Same edge sequence wherever costs are not tied. Otherwise the total cost must be within 0.1% of mainline's and never higher, with length and time within 0.5%. Alternates: same count, each meeting the same bounds. |
| Thor (trip building) | Every `trip` field, given the same edge path | Floating point values within a relative 1e-6 |
| Odin | Maneuver types, order, instruction strings, and verbal and banner instructions, given the same `trip` | Lengths and times within a relative 1e-6 |
| Tyr | Response JSON after parsing (key order is irrelevant), given the same `directions` | Floating point values within a relative 1e-6; encoded polylines must decode to the same coordinates at their precision |

When a difference is a mainline bug, document it next to the test that covers it,
and match mainline's behavior only if a client could depend on it.

## Known divergences from mainline

Deliberate differences, each documented next to the test that covers it.

| Behavior | Mainline 3.9.1 | Valinor | Why |
|---|---|---|---|
| Fractional levels (`kLevels` tagged values) | `decode_levels` divides by 10 for any positive precision, so 0.25 decodes as 2.5, and narrative uses that 10 as the decimal places ("Level 1.5000000000") | Decodes `value / 10^precision` (`Levels::ranges`, so Loki's level filter too), and narrative uses the level's decimal places ("Level 1.5", "Level 0.25") | Wrong floors break indoor routing and instructions; no client can depend on it. Tested against mainline's own decoding in `valhalla-graphtile`'s `section_oracle` and in `illuvatar`'s `trip.rs`. |
| Trace ends given as locations, closest to the end of an edge | `locations_from_shape` takes the side of the street from a segment that ends one past the end of the edge's shape: an out of bounds read, which varies between runs | Uses the edge's last segment | Undefined behavior can't be reproduced. The oracle test skips these sides (see `reads_past_end` in `valinor-loki`). |
| Order of locations that tie in Loki's search | Groups of locations in the same bin are ordered by the address of the bin's tile in memory | Ordered by tile ID | Addresses can't be reproduced. The order only matters through the reach cache when several locations visit the same edges. |
| OSRM approach voice alerts in miles | Passes kilometers to the alert, so a kilometer figure is announced as miles | Uses the request's units | A wrong spoken distance helps no one (Ferrostar plays these). |
| PBF response content type | `application/json` (the format is read after `options` is cleared) | `application/x-protobuf` | Clients decoding PBF need the right type. |
| Landmarks in tiles | `valhalla_add_landmarks` drops edge bins and bounding circles but keeps their header offsets | Landmarks are left out of builds; the reader rejects such tiles | See `tools/oracle/README.md`. |
| Out-of-range table reads in costing | Auto reads past `kSurfaceFactor` for impassable edges (which it never allows, but can cost as an origin or destination); bicycle reads past its speed penalty and surface factor tables for edges faster than 252 kph or with impassable surfaces | Uses the last entry (auto treats impassable like `kPath`) | The reads are undefined behavior with no value to match. Documented in `valinor-sif`'s `auto.rs` and `bicycle.rs`. |
| Two intermediate locations at one node | When two through or via locations in a row snap to the same node, the segment between them is empty and merged away, so both get the same edge index; `CopyLocations` takes an equal index for an unset one and fails with 499 "leg_shape_index not set for intermediate location" (4 of about 11,000 generated routes, all with two vias, such as `andorra-01959`) | Builds the trip, with both locations at the node's shape index and distance (`trip/leg.rs`); mainline's Odin and Tyr serve that trip | A client gets an error for a routable request. Tested by `builds_trips_with_two_vias_at_one_node`; the oracle tests count these cases apart. |
