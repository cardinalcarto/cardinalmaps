# On-device routing API (proposal)

How the Cardinal app calls the router offline, through flutter_rust_bridge.
This is a proposal: nothing here is built yet,
and the open questions at the end need decisions before it is.

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
pub struct OfflineRouter { /* valinor-router over a CachingTileProvider */ }

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
- **No async runtime.** The stages are synchronous functions.
  flutter_rust_bridge's thread pool supplies the concurrency, so tokio never runs on the device.
- **Panics.** The router is built not to panic.
  flutter_rust_bridge's `catch_unwind` is only a backstop:
  it turns an escaped panic into a Dart exception for non-sync functions,
  but not under `panic = "abort"`.

## Open questions

1. **ZeroMQ on the device.** We decided offline mode would talk to the router
   over ZeroMQ `ipc://`. With flutter_rust_bridge, the router can run in process instead:
   `valinor-router` calls the stage functions directly,
   and those are the same functions the ZeroMQ workers wrap.
   That keeps parity at the stage level,
   and avoids putting the `zeromq` crate and tokio
   (which `valhalla-microservice` depends on) in the app.
   **Recommendation:** run in process on the device, and keep ZeroMQ for the server.
   This reverses an earlier decision, so it's yours to make.
2. **Where the bindings live.** The app should have one flutter_rust_bridge crate for routing,
   since a second one carries a second bridge runtime. That crate will most likely be
   wherever Ferrostar's core gets bound for Flutter (Ferrostar has no Flutter SDK).
   If Ferrostar is bound in Rust next to the router,
   the generator → router → parser handoff can happen entirely in Rust,
   as a local route provider, and Dart never handles the blobs.
   **Recommendation:** keep `valinor-router` a plain library in `valinor/`,
   and decide where the bindings go as part of the Ferrostar integration.
3. **Several offline regions.** Tiles where two regions meet only agree
   if both regions come from the same build.
   Every build restamps the build ID in every tile, so the ID is a reliable test.
   **Recommendation:** cut regions from one planet build,
   open all of a device's regions as one router (a tile can come from any of them),
   and refuse to mix builds.
   When a region is updated, the others need the same build.
4. **Cancellation.** Ferrostar may supersede a reroute while it is still running.
   Loki doesn't check interrupts yet, and offline routes over region-sized graphs should be fast,
   so v1 has no cancellation. Adding a cancel token to `route` later is a compatible change.
