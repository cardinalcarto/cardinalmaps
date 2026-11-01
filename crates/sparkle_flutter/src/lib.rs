//! sparkle for Flutter: the map, its tiles and its frames as `sparkle_harness` has them,
//! drawn by the Dart side with Flutter GPU from the commands [`render`] records. Dart
//! reaches it through flutter_rust_bridge ([`api`]).

pub mod api;
mod app;
mod frb_generated;
mod gpu;
mod render;
