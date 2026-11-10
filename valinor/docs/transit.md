# Real-time transit routing

Design for public transit in Valinor.
It does not use Valhalla's multimodal mechanisms or its transit tiles.
Read this together with [routing.md](routing.md) and `AGENTS.md`.

The algorithm is the profile search of Wagner and Zündorf,
"Public Transit Routing with Unrestricted Walking" (ATMOS 2017),
over multicriteria RAPTOR.
Transfers are read from a table of walks between nearby stops, unlike that paper:
searching the street network during a query cost too much time and memory (see "Transfers").

## Requirements

- Profile queries: all journeys in a departure window,
  Pareto-optimal on arrival time and number of trips.
- A leave-now query for the first answer, with the profile as the slower "more routes".
- Transfers within a bound, each costed by Sif the way any route on the graph is,
  and at least as long as the transfer times of its two stops.
- GTFS-RT trip updates, vehicle positions, and service alerts.
- The same router on the server and on the client.
  There is no tokio or network access in the routing crates.
  GTFS-RT reaches the router as optional bytes; without it, routing uses the schedule.
- No panics while decoding a compiled timetable or GTFS-RT.
- Target: p95 under 1 s for a first answer across a metro region.
- Costing is generic over travel modes. It is never hardcoded to pedestrians.
- Park and ride is deferred, but the access and egress design must admit it (below).

## Crates

| Crate | Responsibility |
|---|---|
| `valinor-transit` | Timetable, real-time overlay, MCR forward and backward, profile loop, journeys. |
| `valinor-gtfs` | Compiles GTFS into the timetable file, and numbers several files as one. |
| `valinor-gtfs-rt` | Decodes GTFS-RT and builds real-time overlays. |
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
3. For each distinct new departure time `τdep` from step 2, at any number of trips,
   with `τdep < τmax`, run a forward search at `τdep + 1` and push its arrivals,
   skipping those already seen.
4. Repeat until the queue is empty. Discard entries departing after `τmax`.

Step 3 must run for the new departures of every number of trips.
Forward searches only for the earliest, the latest, or the fewest-trips departure lose journeys;
the brute-force test (see "Testing") confirms this rule and fails each of those.

A query's searches all read one pinned real-time snapshot.

## Direction-dependent costs

Costs differ by direction because the world is not flat.
Each directed edge has exactly one cost, and both searches charge it.
A backward search that moves from B to A charges the time of travelling A to B.

Forward and backward searches read the same table entry for a directed pair of stops,
so they agree by construction.

## Leave now

A leave-now query runs one forward search at the requested time,
then one backward search per arrival, limited to the number of trips that forward search used.
Each result is the earliest arrival for that many trips and the latest departure that still makes it.
"I missed it" is the same query, one second after the departure of the journey that was missed.

## Access, egress, and transfers

Access (`s` to every stop) and egress (every stop to `t`) are computed once per profile
as stop tables, which every forward and backward search reuses.
They are time-independent, and being shared, they don't need to mirror anything.
They may use full edge-based Sif, and may be layered in modes (drive, park, walk).

A journey must contain a trip, as `t` is only reachable by a stop's egress entry from round 1 on.
This replaces the paper's pruning at `t` in section 3.1.
Entries slower than the direct journey in the query's mode are removed afterwards.

Access reaches a first boarding directly, and egress leaves from the last alighting stop.
A journey is access, trips joined by walks, and egress.

## Transfers

A table holds, for each stop, the stops within a bound (15 minutes by default) for a travel mode.
Each entry is one walk, found by the same edge-based Sif search as access and egress:
the route minimizes Sif's cost, and the entry is that route's seconds.
A cyclist's transfer takes the bike path the costing prefers, even when a busier road is quicker.

A rider never walks twice in a row, so the table needs no closure.
It stores only times; a transfer in a result is routed again with the same search to draw it.
A table depends on stop positions and the street graph, not on the timetable,
so a new real-time stop only needs its own walks added.

Sif's cost and seconds are different quantities, so cutting a cost-ordered search off by seconds
lets a costlier route that fits take a stop whose cheapest route doesn't.
Table searches (access, egress, transfers, GTFS-RT new stops, and draws alike) are bounded by cost
alone: `max_cost = max_seconds * factor(mode)`, with factors covering the 95th percentile of
measured whole-route cost over time (walking 1.2, cycling 3.6).
Labels over the bound are dropped, and a stop is only offered a route within it,
so exactly the routes of cost at most `max_cost` survive, whichever direction the search runs.
A stop's entry is its cheapest route, reported if its seconds are within `max_seconds`.
A draw searches with the table's limit and stops once its target stop's route is final.

The table bounds which journeys exist: a transfer past the bound isn't considered.
Measured on King County Metro, about 91% of the profile entries of a 15-minute table match a search
of the street network during the query, at about one hundredth of the search time.

| Use case | Access | Transfers | Routes | Egress |
|---|---|---|---|---|
| Walk | walk | walk | all | walk |
| Bike on board | bike | bike | `bikes_allowed` | bike |
| Wheelchair | wheelchair | wheelchair | not `wheelchair_accessible=2` | wheelchair |
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
Boarding the same stop after arriving by trip requires the stop's transfer time,
so a zero-length walk can't bypass it.
A walk joins two different stops, so it never leads from a stop back to itself.

### Transfer times

This follows MOTIS (nigiri).
Each stop has a transfer time of 2 minutes,
unless a `transfers.txt` row with the same from and to stop sets one;
the smallest of several rows wins.
As in nigiri, `min_transfer_time` is read from a row of any type except 3 and 4,
whether or not it names a route or a trip; a row with no `min_transfer_time` sets nothing.
A station's row covers each of its stops, as GTFS says.

A transfer at the same stop costs that stop's transfer time.
A walk between two stops costs the longest of the walk and both stops' transfer times.
Such a row between two different stops is a floor on that walk, the first row winning, as in
nigiri: nigiri would add a footpath for such a row even with no other way to get there,
but a transfer here comes from a walk on the street network, so a row with no walk in the table
has nothing to apply to. The transfer stage counts those rows in the manifest's totals
(`pair_rules_without_walk`).

Tables keep real walk seconds; the longer value applies when a search reads a walk,
so a walk drawn in a response still takes as long as it really does,
and what's left of the transfer's cost is a wait to board, shown as the next ride's `wait_seconds`.
Nothing here depends on the request: there is no per-request slack.

### Wheelchair

`wheelchair` is a third travel mode, using Sif's pedestrian `wheelchair` type for access, egress,
and transfer tiles and stop snaps, built offline by the same stages as walking and cycling.
Only what a feed explicitly marks inaccessible is excluded:
a stop with `wheelchair_boarding=2` (a stop's empty value inherits its parent station's,
as GTFS says), and a trip with `wheelchair_accessible=2`. Unknown is allowed, and no feed is
special-cased. An excluded stop can't be boarded, alighted, or changed at.
Trips with and without `wheelchair_accessible=2` go in separate patterns, as `bikes_allowed` does,
so a route filter tests one attribute. A stop's exclusion is one more pair of bits in each lane
position's rule byte, set when the lane is laid out, and the scan reads the bits its query chose
with the same test as before, so a query on foot or by bicycle pays nothing.

Forward and backward searches mirror the wheelchair bits exactly as they mirror pickup and drop-off.
A build made without wheelchair tables refuses a wheelchair request as invalid,
rather than answering that nothing is reachable.

A table search's cost bound for a wheelchair is `1.2`, the same as walking;
measured over 5,338 real wheelchair routes in Washington, whole-route cost over time has
p50 1.03, p95 1.175, p99 2.2 (walking p95 1.176, cycling 3.49 over the same sample).

### Forbidden transfers

`transfer_type=3` is honored at stop level. Between two different stops, that walk doesn't exist.
At one stop, a rider arriving by trip can't board another trip there; walking to another stop
and boarding there is still possible, because a rider boards on foot.
A station in a row covers each of its stops, and every pair of them.
Rows naming a route or a trip can't be expressed cheaply (a route or trip check on every
boarding would be a per-trip branch in the scan); they are counted in the compile report
as `transfers_forbidden_qualified`, and otherwise ignored.

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
  They are sized from the pinned snapshot's stop count, which includes an overlay's new stops.
- Pickup and drop-off rules are one flag per stop position of a lane,
  so trips differing in them are in different lanes.
- Stops are numbered so that stops on the same route are close together.

## Several timetables at once

Each feed keeps its own compiled file, memory-mapped as it is, one per feed.
Stops, routes, and trips are numbered across files: a file's number plus that file's base.
`TimetableSet` does this numbering and sends a number back to its file;
adding or removing a feed rewrites no file.

Only the materialized window is concatenated, by `valinor_transit::materialize_parts`.
The window has one arena and one layout, and the scan never sees a file.
Nothing per-file is in the hot path; rider-facing lookups (stop names, trips, routes, agencies,
alerts) find their file by number on the cold path.

Each file keeps its own service-day clock. A window's slots are service dates,
and the date `D` starts at a different instant in each time zone,
so a file's slot `i` has its own offset from the window's origin, the earliest start of any file.
A file that doesn't cover a date simply has no service in that slot,
so feeds in other time zones coexist and files covering different date ranges can be loaded
together. A window for a moment is the union of yesterday, today, and tomorrow in each file.

Overlays are built across all loaded feeds.
A feed has real-time data or none, feeds are independent,
and a stale or broken feed degrades only itself.
New overlay stops number after every file's stops.

A build is loaded from a directory (`TransitBuild::load`), optionally for a subset of its feeds.
The manifest is checked three ways: tiles carry its ID, each timetable must match its SHA-256,
and opening refuses street tiles of a different build.
Transfers come only from tiles; none are built at open, and any tile not on disk is left out,
so an offline area needn't include a large agency's whole network:
a stop in a tile that wasn't downloaded just has no walks from it.

Opening a build maps almost everything rather than copying it:

- Timetables are read from their mapped files for each window; a heap copy of a file's trips
  is only made by tools and tests.
- Tiles are for the network. The first open (or `TransitBuild::prepare`, as an install step)
  assembles one flat file per travel mode under `prepared/`, holding each direction's
  offsets, stop numbers, and seconds in the loaded feeds' stop numbering, and maps it.
  A table is keyed by the manifest, the loaded feeds, the bound, and which tiles are present,
  and is rebuilt and published atomically when that changes. A GTFS-RT new stop goes in a small
  in-memory side table, since a mapped file can't change. Keep tile files if a new area might
  be added later: a rebuild reads only tiles still on disk, unless no tile of the mode remains.
- Stop snaps are saved by the offline build, one file per feed and mode, bound to the street
  build, and read at open rather than computed. A snap whose tile isn't on the device, as when
  an offline area holds only part of the network, acts as an unsnapped stop.

## Real-time overlay

An overlay is an immutable value built from GTFS-RT input and published as an `Arc`.
It holds an arena of route blocks, the directory entries that point into it,
and per-stop additions for new stop patterns.
Stop IDs number base stops first and then an overlay's new stops.
New stops are snapped to the street network when the overlay is built,
and their walks are added to the snapshot's transfer tables by two searches per new stop:
forward from it for walks leaving it, and backward from it for walks from every other stop into
it, with the same costing as the full build. A transfer drawn for a result is routed by the search
that found it, so a walk from a base stop to a new stop is drawn with that backward search.

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
| New stop | GTFS-RT `Stop`, or `replacement_stops`, not in the static feed | Stop appended to the snapshot's stop numbering; added patterns may use it |
| No service | Active `NO_SERVICE` alert | Removed from routing for the alert's scope |
| Stale | Feed older than its limit | Falls back to Scheduled |

Trips are matched by `trip_id` and `start_date`.
GTFS-RT entity IDs are not stable and are never used as keys.

Service that isn't operating can't be routed on.
Any `NO_SERVICE` alert active at the relevant time removes the service it informs,
at whatever scope it is declared.
Fields within one entity selector are combined with AND, and selectors with OR.
`DETOUR`, `STOP_MOVED`, and other effects don't change routing; they are displayed on the legs.

## Stay-seated transfers

A vehicle that continues as a new trip is one ride, not a transfer. In the compiler, consecutive
trips of a `block_id` merge into one trip on a combined stop pattern when the first's last stop is
the same stop as the next's first and the wait there is at most 10 minutes (`--seated-wait-seconds`,
600 by default). A chain holds at most 8 trips by default; what the cap cuts is counted.
A `transfer_type=4` row merges its two trips whatever the wait (up to a sanity limit of two hours,
as nigiri has) and whatever their stops, and takes precedence over `block_id`: a block with any trip
in a type 4 row doesn't merge by `block_id`. A `transfer_type=5` row forbids its pair's merge.
Trips also merge only when both run that day, and only when their bike and wheelchair rules agree,
because a pattern has one of each. A merged trip's `wheelchair_accessible` is `2` if its trips are,
`1` only if all of them are, and unknown otherwise; each original trip keeps its own value too.

A merged trip keeps each original trip's positions as they were, so the last stop of one and the
first stop of the next are two consecutive positions of the combined pattern.
A rider can't board at the former or alight at the latter, so staying on is the only way through.
Each original trip keeps its `trip_id`, route, headsign, stop sequences, and shape, per segment
in the file. GTFS-RT applies to the original trip's stretch, read against that trip's own schedule
whatever order the updates come in. A delay moves that stretch, and since the next stretch is the
same vehicle, a stretch with no update of its own (or its stops before its first update) takes the
previous stretch's arrival delay, keeping its scheduled running times; an early arrival waits.
Times stay in order across the join, since the vehicle can't leave before it has arrived.
A cancellation or a `NO_SERVICE` alert for one original trip leaves its stretch unusable,
so a ride can still board on another stretch and end at the join; alert selectors match each
stretch by its own trip, route, agency, route type, direction, and start time.
A `REPLACEMENT` (or `NEW`, or `ADDED`) of one original trip runs as a trip of its own, and its
stretch of the merged trip is unusable, because nothing says the vehicle stays the same.
A `DUPLICATED` copy of one original trip copies only that trip's stops.
An `assigned_stop_id` moves the whole merged trip to an added pattern, its stretches kept.
A response shows one ride with `segments`, and the join as one stop.
Its `trip` fields are those of the original trip boarded, its state covers the stretches ridden,
and its accessibility the least accessible of them. A stretch with no shape of its own is drawn
straight from stop to stop.

## Responses

Transit has its own endpoint and JSON, separate from Valhalla's `Api` and `multimodal`.
Walking and cycling legs are built from the search's parent pointers through Thor's `Paths` and
`build_trip`, and narrated by Ilúvatar.

Every stop and every leg, in every response and for every mode, carries `accessibility`:
`accessible`, `inaccessible`, or `unknown`.
A stop's is its GTFS `wheelchair_boarding`, a child's empty value inheriting its parent station's.
A transit leg's covers its trip and its boarding and alighting stops: `inaccessible` if any says so,
`accessible` if all do, otherwise `unknown`. A walk or ride to a stop states what's known of it:
`accessible` for a leg a wheelchair search routed on streets that allow one, and `unknown` otherwise.

Transit legs carry everything a rider may want, each field optional where feeds may omit it:
scheduled and predicted times, delay, schedule relationship, route names and colors, agency details,
headsign and stop headsigns, every intermediate stop, shape, accessibility and bike flags,
stop names and platforms, the vehicle's ID, position, occupancy, and status,
and alerts with cause, effect, severity, active periods, URL, description, and what they affect.
Each response states the age of its real-time data.
Vehicles can be looked up by ID afterwards.

### Departure ranges

Each transit leg lists `departures`: the departures of its route from its boarding stop to its
alighting stop, around and including its own, so a client can show what happens if the rider misses
a connection, or that the connection is the night's last. They are read from the query's pinned
snapshot, so they agree with the plan's real-time state.

The route is the rider's: the GTFS routes a trip rides between the two stops, each once in a row.
A departure is listed when its trip rides the same ones. A ride that stays seated as its vehicle
continues as another route lists trips that make the same change between the two stops, and a ride
on one route lists combined trips while they are still on that route, each departure once.
Departures are found as routing finds them: every pattern serving the boarding stop and later the
alighting stop (from each visit to the boarding stop, the first later visit to the alighting stop,
unless the pattern visits the boarding stop again first), every service day of the window,
every lane, and the rules a query of the request's mode reads, so a wheelchair rider sees only
departures they can take and a cyclist only those that allow bikes. Frequency runs are each a
departure. A trip no GTFS route names, such as an added trip without a `route_id`, lists only itself.

A request sets the range with `departures_before` and `departures_after` (2 and 3 by default,
at most 10 each; both 0 lists nothing), and the range is bounded in time: 60 minutes before the
leg's own departure and 120 minutes after. Departures are in order of departure from the boarding
stop. Each has:

- `trip_id`; `start_date` when it differs from the leg's trip's; and `headsign`, the one a rider
  reads at the boarding stop (its stop headsign, else the trip's), when it differs from the leg's.
- `status`: `scheduled`, `live` (its times are predictions), `canceled` (a cancellation or a
  `NO_SERVICE` alert removed the trip), or `skipped` (it runs, but doesn't let riders on at the
  boarding stop or off at the alighting stop any more, or runs a pattern that doesn't serve both).
  Canceled and skipped departures are found in the schedule's block, which an overlay leaves beside
  its own, and are shown at their scheduled times.
- `departure` and `arrival` as routing would use them (the predictions when there are any),
  `scheduled_departure` and `scheduled_arrival`, `delay_seconds` at the boarding stop,
  and `realtime` when either time came from real-time data.
- `vehicle_id` when real-time data names one, `accessibility` as a leg's is worked out,
  and `bikes_allowed`.
- `suggested` on the journey's own departure, whose fields are the leg's.
- `missed` when it leaves the boarding stop before the rider can be there, as routing counts it:
  for the first ride, the request's departure plus the walk from the origin; after a walk, the last
  alighting plus the walk's cost (which includes any minimum transfer time); and after a ride to the
  same stop, the last alighting plus the stop's change time. The journey's own is never missed,
  and a late earlier ride can make a departure that was scheduled to connect a missed one.
- `last_of_day` when no later departure of the route between the two stops runs on its service
  date: canceled and skipped departures don't count, and a departure after midnight on the
  previous date's service day can be that day's last. Knowing it takes the whole service date,
  through its last trip (GTFS times can run past 24:00), which the window always holds,
  since it materializes whole service days.
- `alerts`, indices into the leg's `alerts` that apply to this departure, and `departure_alerts`,
  indices into the leg's `departure_alerts`: the alerts on some of its departures that don't apply
  to the leg itself. Alerts match each departure as they match a leg.

False flags, empty lists, and absent values are left out to keep the JSON small.
Departures add about 11% to a Washington plan's JSON. Finding and describing them costs 0.3 to
1.2 ms a plan on the schedule and 0.8 to 2.5 ms with King County Metro's real-time data,
on plans of 50 to 460 ms, measured with `multi_feed_bench`'s `ride departure ranges` stage.
A plan describes each departure once, however many of its journeys list it.

## Offline build

`valinor-transit-build` runs the offline stages, each a plain command with explicit
input and output directories. They run only on a large machine; timetables and transfers are
never built on a device.

```text
valinor-transit-build compile --input <fetch-output> --output <build-dir>
valinor-transit-build transfers --compiled <build-dir> --street-tiles <tiles> --output <build-dir>
```

`compile` writes one timetable per feed to `timetables/`.
A feed that fails is recorded in `compile.json` and doesn't stop the others;
an unchanged archive (by SHA-256) is not recompiled.

`transfers` loads the stop lists of all feeds and computes every walk with the same search as
`TransferTable::build`. Results are tiled at zoom 12 by the stop the walk starts at,
so a tile can walk to stops of any feed, and one mode's tiles are at
`transfers/<mode>/12/<x>/<y>.bin.zst`.
`transfers/manifest.json` lists the feeds in index order, with the tiles each feed's stops need
and its saved stop snaps (`transfers/snaps/<mode>/<feed index>.bin`),
since clients choose which feeds to download, and `transfers/unsnapped-<mode>.jsonl` the stops
with no walks. A run is staged, and appears only by one rename when everything has succeeded.

Stops are addressed as `(feed index, stop number)` where the feed index is the position
in the manifest. A client decodes the tiles of its feeds, and
`transfer_tile::assemble_transfer_table` gathers them into one `TransferTable` in the
client's own stop numbering; walks to feeds it doesn't have are left out.

## Testing

- The profile is compared on randomized synthetic networks against brute force:
  forward MCR at every second of the window, reduced to Pareto-optimal journeys.
- Forward and backward searches must agree on every journey,
  on networks with direction-dependent costs.
- Transfer table entries must match what the same Sif search finds when a result is drawn.
- Overlays are tested by the same comparison, including overtaking, skips, and cancellations.
- Each trip state and transition has its own test.
- Real feeds and captures live in `/mnt/maps/cardinal/transit`, never in git.
  Fixtures in git are synthetic.
