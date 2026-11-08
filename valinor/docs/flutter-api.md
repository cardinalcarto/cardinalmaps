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

## Open questions

1. **Cancellation.** Ferrostar may supersede a reroute while it is still running.
   Loki doesn't check interrupts yet, and offline routes over region-sized graphs should be fast,
   so v1 has no cancellation. Adding a cancel token to `route` later is a compatible change.
