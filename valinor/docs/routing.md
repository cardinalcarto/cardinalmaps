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
  The server is an HTTP service.
  Offline (on device), the same stage chain runs over ZeroMQ `ipc://` sockets.
  There is no HTTP server on the device.
- **No panics on decoding paths.**
  Corrupt, truncated, or unexpected tile data is an error, never a crash.
  This matters most offline, where a bad download must not take the app down.
- **Out of scope:** transit routing and live traffic.
  Static predicted speeds are in scope.

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

`valinor-router` chains the stages in process.
`illuvatar-service` and the other service binaries wrap stages as ZeroMQ workers
(via `valhalla-microservice`), which serves both the server deployment and offline mode.

### Stage contract

- A stage is a synchronous function taking an `Api` message and returning
  `Result<Api, ValhallaError>`.
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
| Loki | The set of candidate edges per location (graph id, side of street), and errors | `percent_along` within 1e-6; distances within 0.01 m; candidate order may differ only among equal distances |
| Thor (path search) | Errors | Same edge sequence wherever costs are not tied. Otherwise the total cost must be within 0.1% of mainline's and never higher, with length and time within 0.5%. Alternates: same count, each meeting the same bounds. |
| Thor (trip building) | Every `trip` field, given the same edge path | Floating point values within a relative 1e-6 |
| Odin | Maneuver types, order, instruction strings, and verbal and banner instructions, given the same `trip` | Lengths and times within a relative 1e-6 |
| Tyr | Response JSON after parsing (key order is irrelevant), given the same `directions` | Floating point values within a relative 1e-6; encoded polylines must decode to the same coordinates at their precision |

When a difference is a mainline bug, document it next to the test that covers it,
and match mainline's behavior only if a client could depend on it.
