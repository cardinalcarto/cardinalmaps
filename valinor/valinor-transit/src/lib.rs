//! Real-time public transit routing, following
//! Wagner and Zündorf, "Public Transit Routing with Unrestricted Walking" (ATMOS 2017).
//!
//! See `docs/transit.md` for the design this crate implements.
//! In outline:
//!
//! - [`TimetableBuilder`] builds a [`Timetable`] of service-day-relative trips, and
//!   [`Timetable::materialize`] lays the trips of some service days out in a [`BaseWindow`].
//! - [`OverlayBuilder`] replaces, removes, and adds service in an immutable [`Snapshot`].
//!   A [`TransitQuery`] pins one `Arc<Snapshot>`.
//! - [`TransferSearch`] is how a search walks between stops. This crate knows nothing about tiles.
//! - [`Router`] runs forward and backward multimodal RAPTOR searches,
//!   and combines them into a [`Profile`] of journeys.

mod error;
mod ids;
mod journey;
mod overlay;
mod profile;
mod raptor;
mod snapshot;
mod stop_table;
mod timetable;
mod transfer;

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod scenario_tests;
#[cfg(test)]
mod stress_tests;
#[cfg(test)]
mod testing;

pub use error::{BuildError, QueryError, SearchError};
pub use ids::{
    DaySlot, Direction, LaneRef, RouteId, SECONDS_LIMIT, Seconds, ServiceDate, ServiceDay,
    ServiceId, StopId, TIME_LIMIT, Time, TripId, TripKey,
};
pub use journey::{Journey, Leg, Ride};
pub use overlay::OverlayBuilder;
pub use profile::{Profile, ProfileEntry};
pub use raptor::{MAX_TRIPS, RoundResult, Router, SearchRun, TransitQuery};
pub use snapshot::Snapshot;
pub use stop_table::{RouteFilter, StopTable};
pub use timetable::{
    BaseWindow, PatternKey, ResolvedBlock, ResolvedLane, ResolvedTrip, RouteAttributes, StopEvent,
    StopKey, StopRouteIndex, StopRules, StopTime, Timetable, TimetableBuilder,
};
pub use transfer::{
    Clock, TransferArrival, TransferOutcome, TransferRequest, TransferSearch, TransferSeed,
    TransferSink,
};
