//! The offline geocoder on the device, reached from Dart through flutter_rust_bridge
//! ([`api`]).
//!
//! The phone builds its search index from the map tiles it downloaded and the Who's On First
//! places fetched for its regions: [`extract`] reads a z14 MLT tile's features (POIs,
//! addresses, streets, named areas and labels) into [`extract::ExtractedFeature`]s, [`merge`]
//! joins the pieces of streets and areas that tiles cut, [`region`] runs tiles through both
//! into a [`region::FeatureSink`], [`documents`] makes airmail documents of the features with
//! their areas from [`wof`], and [`build`] writes them into an index. [`search`] searches it.
//!
//! This library has no SQLite: the basemap is read through sparkle_flutter's, which writes it
//! (two copies of SQLite in one process break each other's locks; see `offline_map::mbtiles`),
//! and its tiles come in from Dart.

pub mod api;
pub mod build;
pub mod documents;
pub mod extract;
mod frb_generated;
pub mod merge;
pub mod region;
pub mod search;
pub mod wof;
