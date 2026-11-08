//! Valhalla-compatible costing: mainline's Sif.
//!
//! See `docs/routing.md` for where this sits in the stage chain:
//! Loki and Thor call it to decide which edges a travel mode may use
//! and what it costs to traverse them.
//!
//! - [`costing_options`] parses a request's costing options and fills in mainline's defaults.
//! - [`json`] is the rapidjson-like document those options (and the rest of a request) are read from.

extern crate alloc;

pub mod costing_options;
pub mod json;
