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

Stops, and a query's origin and destination, are snapped to an edge from which the mode's costing
reaches at least 250 nodes, and which at least 250 reach (a route request's default is 50).
A journey leaves and enters each of them, and a smaller pocket that can be entered but not left
passes a smaller bound: by bicycle, SeaTac/Airport's southbound platform landed in the airport
garage, so no ride could end there.

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
The manifest is checked three ways: tiles carry its ID, each timetable and stop snap file must
match its SHA-256, and opening refuses street tiles of a different build.
The manifest ID covers everything a client reads to plan, including every tile's contents and
every snap file's SHA-256, so any rebuild that would plan differently has a new ID,
a tile from another build is refused, and prepared tables (keyed by the ID) are made again.
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
  and is rebuilt and published atomically when that changes. Several threads or processes may
  open a build at once: each writes its own temporary file, renames it into place, and maps the
  file it wrote. Publishing a table removes the ones it supersedes, those of the same feeds
  made from other tiles and those of an earlier manifest, but never another feed set's, which
  another opener may be using. A GTFS-RT new stop goes in a small
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

Each transit leg lists `departures`: the departures of every route from its boarding stop to its
alighting stop, around and including its own, so a client can show the other routes a rider could
take instead (such as "or 7, 14"), what happens if the rider misses a connection, or that the
connection is the night's last, and can re-time the rest of the journey on the one the rider picks.
They are read from the query's pinned snapshot, so they agree with the plan's real-time state.

Only the leg's exact two stops count, not other stops nearby: a departure is any trip that lets the
rider on at the boarding stop and off at the alighting stop, whatever its route. A trip that stays
seated as its vehicle continues as another route counts when it serves both stops, and is described
by the route it boards. Each departure is listed once, however many patterns lead from one stop to the other.
Departures are found as routing finds them: every pattern serving the boarding stop and later the
alighting stop (from each visit to the boarding stop, the first later visit to the alighting stop,
unless the pattern visits the boarding stop again first), every service day of the window,
every lane, and the rules a query of the request's mode reads, so a wheelchair rider sees only
departures they can take and a cyclist only those that allow bikes. Frequency runs are each a
departure.

A request sets the range with `departures_before` and `departures_after` (2 and 6 by default,
at most 10 each; both 0 lists nothing), and the range is bounded in time: 60 minutes before the
leg's own departure and 120 minutes after. Departures are in order of departure from the boarding
stop. Each has:

- `trip_id`; `start_date` when it differs from the leg's trip's; `headsign`, the one a rider
  reads at the boarding stop (its stop headsign, else the trip's), when it differs from the leg's;
  and `route`, the GTFS route it boards in the shape of a leg's `route`, when it isn't the leg's.
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
  for the first ride, the request's departure (the response's `window.earliest`) plus the walk from
  the origin; for a later one, the previous ride's arrival plus the leg's `transfer_seconds`.
  The journey's own is never missed, and a late earlier ride can make a departure that was scheduled
  to connect a missed one.
- `last_of_day` when no later departure of any route between the two stops runs on its service
  date, the connection's last: canceled and skipped departures don't count, and a departure after
  midnight on the previous date's service day can be that day's last. Knowing it takes the whole service date,
  through its last trip (GTFS times can run past 24:00), which the window always holds,
  since it materializes whole service days.
- `alerts`, indices into the leg's `alerts` that apply to this departure, and `departure_alerts`,
  indices into the leg's `departure_alerts`: the alerts on some of its departures that don't apply
  to the leg itself. Alerts match each departure as they match a leg.

Every transit leg after a journey's first ride has `transfer_seconds`: the seconds from the
previous ride's arrival at its alighting stop until the rider can board this one, as routing
counts them for `missed`. After a walk, it is the walk's cost, which includes any minimum transfer
time, so it can be more than the walk leg's `duration_seconds`, which is the walk alone;
after a ride to the same stop, it is the stop's change time. A client that re-times a journey on
another departure of an earlier ride marks as missed the later ride's departures that leave before
that departure's `arrival` plus `transfer_seconds`.

False flags, empty lists, and absent values are left out to keep the JSON small.
Departures added about 11% to a Washington plan's JSON, and finding and describing them cost 0.3 to
1.2 ms a plan on the schedule and 0.8 to 2.5 ms with King County Metro's real-time data,
on plans of 50 to 460 ms, measured with `multi_feed_bench`'s `ride departure ranges` stage
when a leg listed only its own route's departures, three after its own by default.
Those numbers predate listing every route and six after, and haven't been measured again.
A plan finds the patterns between two stops, the latest departure between them each day,
and each departure's description once, however many of its journeys list them.

### Nearby departures

`POST /transit/nearby` answers which routes a rider can catch near a place, under which
headsigns, and when. `POST /transit/route` shows one page of one of them from the stop it used,
for a detail view. The device answers both with the same bytes (`OfflineTransit::nearby` and
`::route`), and the logic is in `valinor-transit-plan` (`nearby.rs` and `response/nearby.rs`);
the server and the bridge only supply the clock and pass bytes through.

A nearby request has a `location`; `time` (`now` by default); `mode`, `walking` (the default) or
`wheelchair`; `max_walk_seconds` (1800); `horizon_seconds` (7200, at most a day);
`max_departures` per page (3, at most 10); and optionally `max_routes`.

- Stops are those the plan's access search reaches: a walk on real streets bounded by Sif's cost,
  with the wheelchair costing and snaps for a wheelchair, not straight-line distance.
  The device's 45-minute limit on walks holds here too.
- Departures are found by the departure-range code, from the same pinned snapshot, as departures
  to ride on from a stop: every pattern serving it at a position some later stop follows (a trip's
  last stop is no departure), every service day of the window, every lane, and the rules the
  mode reads, so a wheelchair rider sees only trips and stops not marked inaccessible.
  Frequency runs are each a departure. Canceled and skipped departures are found in the
  schedule's block as for a range, and listed at their scheduled times; skipped means the trip
  runs but doesn't let riders on at the stop any more.
- They are grouped by GTFS route (an original trip's own on a stay-seated combined trip),
  then into pages. A trip no GTFS route names isn't listed. A row shows one page at a time,
  and a rider swipes between them.
- A departure's `headsign` is the one a rider reads at its stop: the stop's headsign there
  (real-time data's, then the schedule's), else the trip's. A page is one headsign, or several
  that merge. Only departures that run give a page its headsigns. A canceled or skipped
  departure is listed on the page its own headsign is on, and on none if that headsign has no
  departure that runs. `direction_id` doesn't group departures.
- A departure is catchable when it leaves at or after `time` plus the walk to its stop, as
  routing counts a first boarding. A page's `stop` is chosen in two steps, over all of its
  headsigns' departures: first the trip of its earliest catchable departure that runs at any
  reachable stop, then, of the stops where that same trip can still be caught, the one with the
  shortest walk (then the earlier departure, then the lower stop number), with `walk_seconds`.
  A later trip from a nearer stop never beats the earliest trip, and a trip caught only at a
  farther stop is caught there.
  A trip is a run on its service date (`trip_id` and `start_date`), and each run of a frequency
  trip is a trip of its own, since runs that share a `trip_id` are different vehicles.
  A loop's two visits to one stop are one stop, at the earlier departure.
  It lists the next `max_departures` departures that run from that stop within the horizon, in
  order, with the canceled and skipped ones among them listed beside them without counting: those
  up to the last one counted, or to the end of the horizon when fewer run.
  `soonest_departure` is when the trip that chose the stop leaves it.
  A route with no page left isn't listed.
- Two headsigns of one route share a page when, from the stop that page would have, every trip
  of one and every trip of the other that run from it call at the same stops until one of them
  ends: the shorter's stops from there are where the longer's begin (see the rounds below). Each trip is compared to the
  end of its own GTFS trip, so a vehicle that goes on as another route is compared only as far
  as this route goes. A short turn merges with the trips it runs with, as does a trip whose
  vehicle goes on as another route, like King County Metro's 44 to the University District that
  continues as the 43. Headsigns of different routes never merge.
  - Every pair of a page's headsigns must merge, never a chain: if "A" merges with "B" and "B"
    with "C", but "A" not with "C", "A" and "C" are on different pages.
  - The comparison is made from the stop the merged page would have, chosen over all of their
    departures as above, and each headsign needs a departure that runs from that stop. A page
    lists departures from one stop, so headsigns that only go the same way from another stop
    don't merge: when the merged page's earliest trip is caught where one of them doesn't stop,
    they stay apart, each at its own stop.
- Pages that share a trip (a run, by `trip_id` and `start_date`, whether it runs or not) are one
  page, transitively, since they are the same vehicles. A trip's headsign can change along its
  route, as King County Metro's 62 reads "Downtown Seattle Fremont" before Fremont and "Downtown
  Seattle" after it, and each headsign alone would be a page at its own stop listing the same
  trips. The page's stop is chosen over all its departures as above, it lists departures from
  that stop only, and its headsigns are those read there, so a rider at Stone Way sees one page
  under Stone Way's headsign. No trip is listed on two pages of a route. A stay-seated chain's
  trips are each a trip of their own: a 44 that reaches Ballard and goes back as a 44 to the
  University District doesn't join the two directions' pages.
- Every headsign with a departure that runs starts as a page of its own. Then, in rounds until a
  round changes nothing:
  1. Pages are taken in a fixed order, most distinct catchable trips that run first, then by
     their first headsign in string order. Each joins the first page before it that it goes the
     same way as, at the stop the two would then have, or else stays. Two pages go the same way
     when each lists a departure that runs from that stop, every one of their headsigns is there
     (by a departure that runs, or by a trip listed there under another of their headsigns,
     which is the same vehicle), and every pair of trips that run there under different
     headsigns call at the same stops until one ends. A short turn so joins the main page
     rather than a rarer one.
  2. Pages that share a trip become one.

  A union moves a page's stop, from which pages may now go the same way, so the rounds repeat.
  Each round only merges, so they end, and since each round's order depends only on its pages,
  the answer doesn't depend on the order departures were found in.
  - Departures without a headsign never join a page by going the same way, only by sharing a
    trip.
- A page's `headsigns` are its merged headsigns, the one with the most departures that run from
  its stop in the horizon first (then the earliest, then by string); a client shows them as
  "A / B", and asks for the page's detail by them. A page's `direction_id` is that of its trips
  that run from its stop, when they all share one, and is only for ordering and information.
  Each departure has the fields of a range's, less those about an alighting stop:
  `trip_id`, `start_date`, `headsign`, `status`, `departure`, `scheduled_departure`,
  `delay_seconds`, `realtime`, `vehicle_id`, `accessibility` (of its trip and stop), `bikes_allowed`,
  and `alerts`, indices into the response's `alerts`; and `continues_as` when its vehicle goes on
  as another route after its trip, with riders staying on (see "Stay-seated transfers"): the next
  trip's `route_id`, the route's `short_name`, and the next trip's `headsign`. It is read from the
  compiled stay-seated trips, so it is there only where the compiler merged the two trips, and is
  left out when the next trip's stretch lets no one on or off (canceled, replaced, or removed by a
  `NO_SERVICE` alert), when the next trip is the same route's, and on a canceled departure.
- Routes are ordered by their pages' soonest `soonest_departure`. A route's pages are in an order
  that doesn't depend on time, so swiping between them is stable from one refresh to the next:
  by `direction_id`, pages without one last, then by the first of their headsigns in string
  (UTF-8 byte) order, a page without headsigns last.
  A route has its `feed`, `route` (names, type, and colors), and `agency`.
- `realtime` states each feed's age, as a plan's does. With no routes, `no_routes_reason` says
  `no_access` (no stop within the walk) or `no_service`.

A route request names what a nearby page gave: `feed`, `route_id`, `headsigns` (the page's,
empty for a page without any), and `stop_id`; with `time`, `mode`, `walk_seconds` (0),
`horizon_seconds`, `max_departures`, and optionally a departure's `trip_id` and `start_date`.
It answers with the route, `headsigns` as asked, `departures` from the stop of the route's
trips whose headsign there is one of them (or that have none, for an empty list) from `time` plus
`walk_seconds` (`max_departures` counting only those that run, as for nearby), `ride`, a plan's
transit leg on the chosen trip (or the first that runs) from the stop to where its vehicle stops
carrying riders, with every stop's times, its shape as polyline6, vehicle, and alerts;
`shape_to_stop`, the same trip
before the stop, where the rider can't board it yet: its shape from the first stop of its GTFS trip
to the stop, as polyline6, ending where the ride's begins (straight from stop to stop where the
trip has no shape there, or runs on a pattern real-time data gave it; left out without a `ride`,
and when the stop is the trip's first, as for a stay-seated trip's second stretch at the join);
and `vehicles`, those real-time data has on the route's trips that leave the stop under one of the
headsigns, whether or not they have left it yet. A vehicle not matched to a trip of the timetable has no stops to read that from,
and isn't listed. A route, stop, or trip that isn't there is a 404 (`unknown_route`,
`unknown_stop`, or `unknown_trip`); headsigns with nothing in the horizon are an answer with no
departures and no `ride`. A request without `headsigns`, with an empty headsign, or naming one
twice is a 400.

Where the vehicle goes on as further trips of a stay-seated combined trip (see "Stay-seated
transfers"), the `ride` goes on with it, as a plan's ride does: one ride in `segments`, each
original trip with its `trip_id`, route, headsign, and the range of the ride's `stops` it covers,
the joins one stop, every stop with its own trip's times and real-time data, and the shape across
them all. A 44 to the University District that continues as the 43 to Capitol Hill so shows, before
the rider boards, that it gets them to Capitol Hill. The ride goes on from the boarded trip through
each next trip that still lets riders on or off somewhere, so it ends where `continues_as` would
stop: at the end of the last trip before one that is canceled, replaced, or removed by a
`NO_SERVICE` alert. The compiler's chain cap and rules (`block_id`, and `transfer_type` 4 and 5)
made the chain, so every trip on it is one a rider can stay on for; a later trip of the same route
is part of the ride as well. The ride's `route`, `trip`, and `departures` stay the row's, whatever
routes the later segments are, and `shape_to_stop` is still only the boarded trip's own.
On Seattle's build, the 44 from N 46th St & Phinney Ave N at 09:49 on a Tuesday rides 9 stops to
U District Station as the 44 and 27 more as the 43, to Boren Ave & E Yesler Way: its detail grows
from 9.0 KB of JSON to 23.3 KB, the ride from 5.1 KB to 19.4 KB.

Measured with `nearby_bench` on Seattle's seven feeds (the 2026-11-12 planet build) on a desktop,
warm, a 30-minute walk takes 4 to 35 ms, nearly all of it the access table:
15 ms from Fremont (11 routes), 22 to 24 ms downtown (50 to 70 routes), and 28 to 32 ms on
Capitol Hill. Departures take under 3 ms, also with King County Metro's real-time data
(a Monday 10:18 capture). The process's first query takes about 110 ms, reading tiles from disk.
A response is 8 to 130 KB of JSON.
Paging by headsign, with pages that share a trip made one, takes departures from 0.1 to 1.8 ms
to 0.1 to 3.6 ms (the most downtown and on Capitol Hill), mostly reading each departure's
headsign. There are about as many pages as there were directions, and the JSON is 1 to 9%
larger (8 to 134 KB), for `soonest_departure` and `continues_as`; measured warm with the
planet's street tiles on 2026-11-13.

## Offline build

`valinor-transit-build` runs the offline stages, each a plain command with explicit
input and output directories. They run only on a large machine; timetables and transfers are
never built on a device.

```text
valinor-transit-build compile --input <fetch-output> --output <build-dir>
valinor-transit-build transfers --compiled <build-dir> --street-tiles <tiles> --output <build-dir>
valinor-transit-build feed-index --build <build-dir> --output <staging-dir>
```

`compile` writes one timetable per feed to `timetables/`.
A feed that fails is recorded in `compile.json` and doesn't stop the others;
an unchanged archive (by SHA-256) is not recompiled.

`transfers` loads the stop lists of all feeds and computes every walk with the same search as
`TransferTable::build`. Results are tiled at zoom 14 by the stop the walk starts at,
so a tile can walk to stops of any feed, and one mode's tiles are at
`transfers/<mode>/14/<x>/<y>.bin.zst`.
Walks no plan can use are left out (`dominance` in `valinor-transit-build`): a walk to a stop
`T` goes when, on every pattern a rider could board at `T`, a walk to a stop upstream catches
every trip `T` would, strictly sooner, under the planner's own walk costs (transfer times,
`transfers.txt` floors, forbidden pairs) and, for wheelchairs, only where a wheelchair may
board. It is exact for every plan (checked by `compare_builds` in `valinor-transit-plan`);
the manifest counts the walks left out per tile and mode, and `--no-prune-dominated` keeps
them all.
`transfers/manifest.json` lists the feeds in index order, with the tiles each feed's stops need
and its saved stop snaps (`transfers/snaps/<mode>/<feed index>.bin`),
since clients choose which feeds to download, and `transfers/unsnapped-<mode>.jsonl` the stops
with no walks. A run is staged, and appears only by one rename when everything has succeeded.

The manifest ID (`manifest::manifest_id`) hashes the street build, tile format, zoom, bounds,
each feed's archive, timetable, snap files, and tile list, and each tile's content digest:
the SHA-256 of the decompressed tile with its manifest ID and checksum fields as zeros.
Since tiles carry the ID, they're written in two passes: the searches keep each tile
uncompressed with no ID and record its digest, and once the ID is known each tile is given it,
compressed, and written. Names, licenses, statistics, and compressed sizes aren't covered.
A build is byte-identical across reruns and thread counts.

Stops are addressed as `(feed index, stop number)` where the feed index is the position
in the manifest. A client decodes the tiles of its feeds, and
`transfer_tile::assemble_transfer_table` gathers them into one `TransferTable` in the
client's own stop numbering; walks to feeds it doesn't have are left out.

`feed-index` writes what a client fetches to choose and install feeds, for publishing
beside the build (see `feed_index`). Its `index/<x>/<y>.json` lists, for each zoom 6 tile, the
feeds with stops there and the zoom 12 cells of those stops, read from the timetables;
`feeds/<id>.json` and `feeds.json` summarize feeds (agencies, license, service dates, and
download sizes). `manifest/shared.json` is the manifest without its feeds and global tile list,
with `feed_count` and `feed_ids` in index order, and `manifest/feeds/<id>.json` is one feed's
entry with its `index`. The loader reads only the scalar fields and the loaded feeds' entries,
so a client opens the shared fields (less `feed_ids`) with `feeds` holding just its own
entries, in increasing index order. A feed's `index` is what transfer tiles address its stops
by; without one, it is the feed's position in `feeds`. Indices are given by every feed or by
none, and must be strictly increasing and below `feed_count`.

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
