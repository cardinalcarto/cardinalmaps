//! Real-time public transit routing: the timetable, overlay, and transfer search interfaces.
//!
//! See `docs/transit.md` for the design this crate implements.
//! The searches that read these structures arrive in the next commit.

#![allow(dead_code, unused_imports)]

mod error;
mod ids;
mod overlay;
mod snapshot;
mod stop_table;
mod timetable;
mod transfer;

pub use error::{BuildError, QueryError, SearchError};
pub use ids::{
    DaySlot, Direction, LaneRef, RouteId, SECONDS_LIMIT, Seconds, ServiceDate, ServiceDay,
    ServiceId, StopId, TIME_LIMIT, Time, TripId, TripKey,
};
pub use overlay::OverlayBuilder;
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
