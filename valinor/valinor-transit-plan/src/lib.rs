//! Connects the compiled timetable, GTFS-RT overlay, and router, and builds rider-facing responses.
//!
//! See `docs/transit.md` for the design this crate serves. In outline:
//!
//! - [`TransitState`] owns the compiled timetable and service windows, accepts optional GTFS-RT
//!   bytes per feed, and publishes an immutable [`TransitSnapshot`] that a query pins.
//!   It has no runtime, network access, or clock; `now` is always an argument,
//!   so a server and a phone use it identically.
//! - [`plan`] answers a [`TransitRequest`] over one pinned snapshot, using an
//!   [`AccessEgressSearch`] for stop tables and a [`valinor_transit::TransferSearch`] for walks.
//! - [`PlanResponse`] is the rider-facing JSON answer, with its own endpoint's schema
//!   rather than Valhalla's.
//! - [`CompiledLookup`] is the bridge that lets the GTFS-RT overlay builder see compiled GTFS.

mod error;
mod lookup;
mod plan;
pub mod response;
mod state;
mod window;

#[cfg(test)]
mod scenario_tests;
#[cfg(test)]
mod testing;

pub use error::{PlanError, StateError};
pub use lookup::CompiledLookup;
pub use plan::{
    AccessEgressRequest, AccessEgressSearch, AccessEgressTables, MAX_DEPARTURE_WINDOW_SECONDS,
    TransitOptions, TransitRequest, TravelMode, plan,
};
pub use response::{PlanResponse, Vehicle, VehicleUpdate};
pub use state::{
    FeedUpdate, IngestOutcome, StateConfig, StopLocation, TransitSnapshot, TransitState,
};
