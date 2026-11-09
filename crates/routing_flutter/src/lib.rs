//! Offline routing for Flutter: `valinor-router` in the app's process,
//! reached from Dart through flutter_rust_bridge ([`api`]).
//!
//! The app hands [`api::router::OfflineRouter::route`] the same Valhalla `route` body that
//! Ferrostar's request generator makes for the server, and gets back the bytes the server
//! would have sent (see `valinor/docs/flutter-api.md`).
//! Public transit is planned the same way, through [`api::transit::OfflineTransit`].

pub mod api;
mod frb_generated;
