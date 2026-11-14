# On-device routing API

How the Cardinal app calls the router offline, through flutter_rust_bridge.
It is built: `crates/routing_flutter` (the bindings over `valinor-router`)
and the `flutter/routing` plugin, whose test routes a Ferrostar body over the Andorra fixture.
The app doesn't use it yet.

## How Ferrostar asks for routes

Ferrostar's core (Rust) does routing through a `RouteAdapter`,
a request generator paired with a response parser:

- `ValhallaHttpRequestGenerator::generate_request(user_location, waypoints)`
  returns `RouteRequest::HttpPost { url, headers, body }`.
  The body is Valhalla route JSON:
  `format: "osrm"`, `banner_instructions` and `voice_instructions`,
  `filters` including `shape_attributes.{speed, speed_limit, time, length}` (for annotations),
  `costing`, and `locations`.
  The first location carries the user's `heading` and a `street_side_tolerance`,
  and the others are `break`, `via`, `through`, or `break_through`.
  The app's `options_json` is merged shallowly on top.
- `OsrmResponseParser::parse_response(bytes)` turns the OSRM-format response into `Route`s.

Online, the platform sends that POST to `/routing/route` on our server.
Ferrostar uses the same generator to reroute.

## The proposal: one function, the same bytes as online

Offline, the app takes the body the same generator produces,
hands it to the router on the device, and gives the response bytes to the same parser.
The device then sees exactly what the server would have returned,
and online and offline routing differ only in transport.

```rust
/// Opaque to Dart. Thread-safe: concurrent requests are fine.
pub struct OfflineRouter { /* a valinor-router */ }

impl OfflineRouter {
    /// Opens tile sets (mainline `build_extract` tarballs or tile directories).
    /// Fails if they don't all come from one build (the build ID in each tile's checksum).
    pub fn open(tile_sets: Vec<String>) -> Result<OfflineRouter, OpenError>;

    /// A Valhalla `route` request body, as Ferrostar's generator makes it.
    /// Not `#[frb(sync)]`: flutter_rust_bridge runs it on its worker pool.
    pub fn route(&self, body: Vec<u8>) -> RouterResponse;
}

pub struct RouterResponse {
    /// The HTTP status the server would have sent (200, or 400 for an error).
    pub status: u16,
    /// OSRM-format JSON: a route, or an OSRM-style error such as
    /// `{"code": "NoRoute", "message": ...}`.
    pub body: Vec<u8>,
}
```

- **Errors.** Routing errors are response bodies, not Dart exceptions,
  exactly as they are over HTTP, so Ferrostar's parser handles them the same way.
  Only `open` returns a `Result` (missing files, mixed builds, unreadable tiles).
- **Just `route`.** Ferrostar uses nothing else.
  `valinor-router` will serve every action, because the server needs them,
  so `locate`, `trace_route`, or `status` can be exposed later
  when a feature needs one, as a `request(action, body)` variant.
- **Snapping like transit.** On the device, a location's edge must reach 250 nodes in and out
  (Loki's default is 50), the bound transit uses for its stops and plan endpoints,
  so street and transit directions start and end in the same places.
  At 50, Cal Anderson Park's point lands on a park path a bicycle can't enter or leave,
  and no bicycle route reaches the park, in mainline or valinor.
- **No async runtime.** The stages are synchronous functions.
  flutter_rust_bridge's thread pool supplies the concurrency, so tokio never runs on the device.
- **Panics.** The router is built not to panic.
  flutter_rust_bridge's `catch_unwind` is only a backstop:
  it turns an escaped panic into a Dart exception for non-sync functions,
  but not under `panic = "abort"`.

## Decided: the router runs in process

On the device and on the server, `valinor-router` calls the stage functions directly,
in one process, with no ZeroMQ.
iOS doesn't let apps start child processes,
so mainline's separate worker processes can't run there.
Since Android 10, apps can't run executables from their writable storage either.
Connecting threads with ZeroMQ inside one process would only add
serialization at every stage boundary,
and it would put the `zeromq` crate and tokio in the app.
Parity with the server holds at the stage level,
because the ZeroMQ workers wrap these same stage functions.
The server (axum) calls `valinor-router` the same way,
so the device and the server share one code path.
The ZeroMQ workers remain for splicing stages into the mainline oracle.

## Decided: where the bindings live and how regions work

- The flutter_rust_bridge bindings are a crate in Cardinal's `crates/` workspace,
  which depends on `valinor-router` by path.
- Offline regions are all cut from one planet build,
  and a device opens all of its regions as one router.
  The router refuses to mix builds (the build ID in each tile's checksum).
  When one region is updated, the others need the same build.

## As built

- `OpenError` has a `kind` (`NoTileSets`, `Unreadable`, `NoBuildId`, or `MixedBuilds`)
  and a `message` naming the file and tile; Dart sees it as an exception from `open`.
  `MixedBuilds` is the one that means "download the other regions again".
- `open` is not `#[frb(sync)]` either: it maps each tarball, reads its index,
  and reads up to 4096 tile headers per set for the build check (every tile of a region).
- Two routes run at once on a device; more calls wait.
  Each running route keeps its search storage, which grows with the largest search it ran.
- The router's configuration is mainline's defaults (service limits included),
  since a device has no `valhalla.json`.
- `valhalla-graphtile` builds with no special `--cfg` (cargokit's Android builds set their
  own `RUSTFLAGS`, which replace a cargo config's).
- iOS links the static library through cargokit's `build_pod.sh` and `-force_load`
  (see `flutter/routing/ios/routing.podspec`), as `flutter/sparkle` does; it can't be
  built on Linux, so it is untested.

## Open questions

1. **Cancellation.** Ferrostar may supersede a reroute while it is still running.
   Loki doesn't check interrupts yet, and offline routes over region-sized graphs should be fast,
   so v1 has no cancellation. Adding a cancel token to `route` later is a compatible change.

## Public transit on the device

Transit planning runs through `api::transit::OfflineTransit`, beside the router above:

```rust
impl OfflineTransit {
    /// A transit build directory (from `valinor-transit-build`) over the same tile sets;
    /// `feeds` chooses which of its feeds to load, or `None` for every feed that compiled.
    /// Takes a few seconds: every stop is snapped for walking and for cycling.
    pub fn open(build: String, feeds: Option<Vec<String>>, tile_sets: Vec<String>) -> Result<OfflineTransit, TransitOpenError>;

    /// JSON request bytes (the body of the server's `POST /transit/plan`),
    /// with whatever GTFS-RT bytes the app holds, per feed.
    /// `cancel` lets a screen abandon the plan; past a 30-second deadline it answers 504.
    pub fn plan(
        &self,
        request: Vec<u8>,
        feeds: Vec<TransitFeed>,
        cancel: &TransitCancel,
    ) -> TransitResponse;

    /// The body of the server's `POST /transit/nearby`: the routes a rider can catch near a
    /// place, in each direction, and when. Feeds, cancellation, and the deadline are as for `plan`.
    pub fn nearby(&self, request: Vec<u8>, feeds: Vec<TransitFeed>, cancel: &TransitCancel) -> TransitResponse;

    /// The body of the server's `POST /transit/route`: one route and direction from one stop.
    pub fn route(&self, request: Vec<u8>, feeds: Vec<TransitFeed>, cancel: &TransitCancel) -> TransitResponse;

    /// A vehicle's latest state, as the server's vehicle lookup returns it.
    pub fn vehicle(&self, feed: String, id: String) -> TransitResponse;
}
```

- The app fetches GTFS-RT itself and passes the bytes in;
  the routing crates never reach the network, and a feed that doesn't decode degrades to the
  schedule and is reported in the response.
  A feed omitted from a call stays as it was until it ages out.
- Requests and responses are the server's JSON, from `valinor-transit-plan`.
  A response has `journeys`, each with access, transit, transfer, and egress legs;
  walking and cycling legs carry a `walk` with a polyline6 `shape`, distance, duration,
  and `maneuvers`, and each response states every feed's age in `realtime`.
- A walking leg next to a moved stop has `stop_moved`: `STOP_MOVED` alerts give the
  temporary location only as prose, so show that text beside the walk.
  The transit leg's alert also names the affected stop in `moved_stops`.
- Not `#[frb(sync)]`: planning runs on flutter_rust_bridge's worker pool.
- A plan with no journeys says why in `no_journeys_reason` (`no_access`, `no_egress`,
  `no_service`, or `slower_than_direct`), and `direct` reports the direct search's outcome.
- `nearby` answers the screen of nearby routes: per GTFS route, its pages, one per headsign or
  per set of headsigns whose trips go the same way, each with the reachable stop to use and its
  walk, and the next departures there; routes come soonest first. A row's `feed`, `route_id`, a
  page's `headsigns`, and its stop's `stop_id` are what `route` takes for the detail view, which
  adds a trip's stops from that stop on with its shape, and the vehicles on the page's trips.
  See "Nearby departures" in `transit.md` for both.
- The device refuses access or egress walks above 45 minutes with a 400,
  and a timetable must be replaced by renaming, never rewritten in place, while open.
On first open (or `TransitBuild::prepare`) a flat transfer table is assembled under the build's
`prepared/`, so that directory must be writable.
