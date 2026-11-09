# Real-time transit routing

Design for public transit in Valinor.
It does not use Valhalla's multimodal mechanisms or its transit tiles.
Read this together with [routing.md](routing.md) and `AGENTS.md`.

The algorithm is the profile search of Wagner and Zündorf,
"Public Transit Routing with Unrestricted Walking" (ATMOS 2017),
over multimodal multicriteria RAPTOR (MCR).

## Requirements

- Profile queries: all journeys in a departure window,
  Pareto-optimal on arrival time and number of trips.
- Unrestricted transfers on the Valhalla routing graph, with no transitive closure.
- GTFS-RT trip updates, vehicle positions, and service alerts.
- The same router on the server and on the client.
  There is no tokio or network access in the routing crates.
  GTFS-RT reaches the router as optional bytes; without it, routing uses the schedule.
- No panics while decoding a compiled timetable or GTFS-RT.
- Target: p95 under 1 s for a 60-minute window across a metro region.
- Costing is generic over travel modes. It is never hardcoded to pedestrians.
- Park and ride is deferred, but the access and egress design must admit it (below).

## Crates

| Crate | Responsibility |
|---|---|
| `valinor-transit` | Timetable, real-time overlay, MCR forward and backward, profile loop, journeys. |
| `valinor-gtfs` | Compiles GTFS into the timetable file; decodes GTFS-RT into overlay input. |
| `valinor-walk` | Street search over tiles in both directions, stop snapping, access and egress tables. |

`valinor-transit` knows nothing about tiles or GTFS.
It sees streets only through a transfer search trait,
so its tests run on synthetic networks.

## Time

Query times are integer seconds from an origin chosen for the query's window.
`τdep + ε` is `τdep + 1`.

## Profile loop

For an interval `[τmin, τmax]`:

1. Run a forward search at `τmin`.
   Push each Pareto-optimal arrival time into a priority queue.
2. Pop the minimum arrival time and run a backward search from it.
   Each number of trips with a result yields a profile entry.
3. For each distinct new departure time `τdep` from step 2 with `τdep < τmax`,
   run a forward search at `τdep + 1` and push its arrivals, skipping those already seen.
4. Repeat until the queue is empty. Discard entries departing after `τmax`.

The paper leaves step 3 ambiguous.
Brute force decides: see "Testing".

A query's searches all read one pinned real-time snapshot.

## Direction-dependent costs

Costs differ by direction because the world is not flat.
Each directed edge has exactly one cost, and both searches charge it.
A backward search that moves from B to A charges the time of travelling A to B.

Transfer costs are integer seconds per directed edge, rounded once,
so that sums don't depend on the order of evaluation.
Where a stop splits an edge, the two parts sum to the whole edge's seconds.

Transfers search a node-based graph.
A transition cost that depends on the pair of edges can't be charged exactly in both directions,
so only costs that belong to a node or an edge are charged.
Walking times are compared against Thor's to check what this loses.

## Access, egress, and transfers

Access (`s` to every stop) and egress (every stop to `t`) are computed once per profile
as stop tables, which every forward and backward search reuses.
They are time-independent, and being shared, they don't need to mirror anything.
They may use full edge-based Sif, and may be layered in modes (drive, park, walk).

A journey must contain a trip, as `t` is only reachable by a stop's egress entry from round 1 on.
This replaces the paper's pruning at `t` in section 3.1.
Entries slower than the direct journey in the query's mode are removed afterwards.

| Use case | Access | Transfers | Routes | Egress |
|---|---|---|---|---|
| Walk | walk | walk | all | walk |
| Bike on board | bike | bike | `bikes_allowed` | bike |
| Bike and park | bike, park, walk | walk | all | walk |
| Park and ride (deferred) | drive, park, walk | walk | all | walk |

## Rounds

Each round scans routes, then runs a multi-source transfer search seeded by the stops the scan
improved. Both happen in the same round: walking never uses up a round.

Standard RAPTOR rules apply at every stop of a scan:
the current trip can be replaced by an earlier one wherever the previous round's label allows it.
Each stop occurrence on a route is indexed, so loop routes work.
`pickup_type` and `drop_off_type` are honored, and mirrored in backward searches.

A stop has separate labels for arriving by trip and arriving on foot.
Boarding the same stop after arriving by trip requires the minimum change time,
so a zero-length walk can't bypass it.

## Memory layout

RAPTOR is fast because of cache hits on data the prefetcher can predict. Nothing may jeopardize this.

- The route scan reads contiguous blocks of times. There are no per-trip branches, lookups,
  or calendar checks in the scan.
- The compiled file stores service-day-relative times and calendars.
  At load, and at each rollover, the active trips for yesterday, today, and tomorrow
  are materialized into a contiguous window. Other dates are built on demand.
- A route directory gives each route its block, which is in the base window or in the overlay arena.
  Both use the same layout, and the scan is the same code for both.
- Labels are flat arrays indexed by stop and round, cleared by version stamps.
- Stops are numbered so that stops on the same route are close together.

## Real-time overlay

An overlay is an immutable value built from GTFS-RT input and published as an `Arc`.
It holds an arena of route blocks, the directory entries that point into it,
and per-stop additions for new stop patterns.

Each changed route and service day gets a whole replacement block.
If delays make trips overtake, the block is split into non-overtaking lanes,
each laid out and scanned like any route block. An unchanged route has one lane.

| Trip state | Meaning | Effect |
|---|---|---|
| Scheduled | No update | Base times |
| Delayed | Updated times | Times replaced; delays propagate along the trip per GTFS-RT rules |
| Stop skipped | `SKIPPED` stop time | No boarding or alighting at that stop |
| Canceled | Canceled trip | Removed from that service day |
| Added or replaced | New trip or stop pattern | Own pattern in the overlay |
| No service | Active `NO_SERVICE` alert | Removed from routing for the alert's scope |
| Stale | Feed older than its limit | Falls back to Scheduled |

Trips are matched by `trip_id` and `start_date`.
GTFS-RT entity IDs are not stable and are never used as keys.

Service that isn't operating can't be routed on.
Any `NO_SERVICE` alert active at the relevant time removes the service it informs,
at whatever scope it is declared.
Fields within one entity selector are combined with AND, and selectors with OR.
`DETOUR`, `STOP_MOVED`, and other effects don't change routing; they are displayed on the legs.

## Responses

Transit has its own endpoint and JSON, separate from Valhalla's `Api` and `multimodal`.
Walking and cycling legs are built from the search's parent pointers through Thor's `Paths` and
`build_trip`, and narrated by Ilúvatar.

Transit legs carry everything a rider may want, each field optional where feeds may omit it:
scheduled and predicted times, delay, schedule relationship, route names and colors, agency details,
headsign and stop headsigns, every intermediate stop, shape, accessibility and bike flags,
stop names and platforms, the vehicle's ID, position, occupancy, and status,
and alerts with cause, effect, severity, active periods, URL, description, and what they affect.
Each response states the age of its real-time data.
Vehicles can be looked up by ID afterwards.

## Testing

- The profile is compared on randomized synthetic networks against brute force:
  forward MCR at every second of the window, reduced to Pareto-optimal journeys.
- Forward and backward searches must agree on every journey,
  on networks with direction-dependent costs.
- Overlays are tested by the same comparison, including overtaking, skips, and cancellations.
- Each trip state and transition has its own test.
- Real feeds and captures live in `/mnt/maps/cardinal/transit`, never in git.
  Fixtures in git are synthetic.
