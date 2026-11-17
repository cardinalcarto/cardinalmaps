//! The offline geocoder on the device, reached from Dart through flutter_rust_bridge
//! ([`api`]).
//!
//! The phone builds its search index from the map tiles it downloaded: [`extract`] reads a
//! z14 MLT tile's features (POIs, addresses, streets, named areas and labels) into
//! [`extract::ExtractedFeature`]s, [`merge`] joins the pieces of streets and areas that tiles
//! cut, and [`region`] runs a region's tiles through both into a [`region::FeatureSink`]
//! (airmail's document builder, once it's plugged in).
//!
//! This library has no SQLite: the basemap is read through sparkle_flutter's, which writes it
//! (two copies of SQLite in one process break each other's locks; see `offline_map::mbtiles`),
//! and its tiles come in from Dart.

pub mod api;
pub mod extract;
mod frb_generated;
pub mod merge;
pub mod region;
