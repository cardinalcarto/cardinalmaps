//! Valhalla-compatible costing: mainline's Sif.
//!
//! See `docs/routing.md` for where this sits in the stage chain:
//! Loki and Thor call it to decide which edges a travel mode may use
//! and what it costs to traverse them.
//!
//! - [`costing_options`] parses a request's costing options and fills in mainline's defaults.
//! - [`Costing`] is the interface Loki and Thor call (mainline's `DynamicCost`),
//!   implemented by [`AutoCost`], [`PedestrianCost`], and [`BicycleCost`],
//!   with [`AnyCosting`] to pick one at run time.
//!   [`DynamicCost`] holds what they share, including complex restrictions
//!   ([`DynamicCost::restricted`]) and hierarchy limits.
//! - [`Label`] is what costing reads about the predecessor edge,
//!   which Thor's edge labels provide (see [`label`] for the contract).
//! - [`datetime`] resolves time zones and evaluates timed restrictions.
//! - [`json`] is the rapidjson-like document requests are read from.
//!
//! Live traffic is out of scope: no edge is ever closed, and live speeds are never used.
//! Predicted speeds are used when a route has a time.

#![expect(
    clippy::similar_names,
    reason = "costing reads `tile`s and `time`s side by side, as mainline names them"
)]

extern crate alloc;

mod auto;
mod bicycle;
mod cost;
mod costing;
pub mod costing_options;
pub mod datetime;
mod dynamic;
pub mod json;
pub mod label;
mod pedestrian;

#[cfg(test)]
mod oracle_tests;

pub use auto::AutoCost;
pub use bicycle::{BicycleCost, BicycleType};
pub use cost::{Cost, InternalTurn, SHORT_INTERNAL_LENGTH, TravelMode};
pub use costing::{AnyCosting, Costing, CostingError, EdgeCostResult};
pub use dynamic::{
    DisallowMask, DynamicCost, EdgeAccess, INVALID_RESTRICTION, TimeContext, UNLIMITED_TRANSITIONS,
    stop_expanding, stop_expanding_bidirectional,
};
pub use label::{EdgeLabel, Label, LabelState, LabelStore};
pub use pedestrian::{PedestrianCost, PedestrianType};
