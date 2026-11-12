//! Valhalla's whole server in one process: the stage chain for the server and the app.
//!
//! [`Router`] chains the stages described in `docs/routing.md`,
//! front → Loki → Thor (path search, then trip building) → Odin → Tyr,
//! by calling each stage function directly, with no ZeroMQ and no async runtime,
//! and answers each request with what mainline's whole server would send:
//! the body, its status and content type, and mainline's errors in the request's format.
//!
//! - [`Router::open`] opens one or more tile sets (mainline `build_extract` tarballs or tile
//!   directories) as one tile set, refusing sets from different builds ([`tiles`]).
//! - [`Router::handle`] answers an HTTP request (any action);
//!   [`Router::handle_interruptible`] can be interrupted, as mainline's workers can.
//! - [`Router::route`] answers a `route` request body: the one function the app calls
//!   (see `docs/flutter-api.md`).
//!
//! `valinor-server` serves it over HTTP, and Cardinal's `routing_flutter` crate
//! exposes [`Router::route`] to the app.
//!
//! # Concurrency
//!
//! A [`Router`] is [`Send`] and [`Sync`], and each request runs on the caller's thread,
//! so the caller decides how many run at once (a blocking pool on the server,
//! `flutter_rust_bridge`'s worker pool on the device).
//! Tiles are shared through one bounded cache, and path searches take a workspace from a
//! bounded pool ([`RouterConfig::workspaces`]): requests beyond it wait for a workspace.

extern crate alloc;

mod config;
mod pool;
mod router;
pub mod tiles;

#[cfg(test)]
mod capture_tests;

pub use config::{ConfigError, RouterConfig};
pub use http::{Method, StatusCode};
pub use router::{HttpResponse, Interrupted, Router};
pub use tiles::OpenError;
pub use valinor_tyr::front::HttpRequest;
