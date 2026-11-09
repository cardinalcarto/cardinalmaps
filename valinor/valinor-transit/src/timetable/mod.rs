//! Timetables: what compiled GTFS becomes, and the windows that searches read.
//!
//! [`TimetableBuilder`] takes patterns, trips, stop times, and calendars,
//! and produces a [`Timetable`], which holds service-day-relative times.
//! [`Timetable::materialize`] then resolves calendars for some service days and lays their
//! active trips out as contiguous blocks in a [`BaseWindow`].

mod arena;
mod builder;
mod lanes;
mod window;

pub(crate) use arena::{Arena, BlockRef, INFINITY, Lane, offset_of};
pub use builder::{PatternKey, StopKey, StopTime, Timetable, TimetableBuilder};
pub use lanes::{ResolvedBlock, ResolvedLane, ResolvedTrip, StopEvent, StopRules};
pub(crate) use window::RouteOccurrence;
pub use window::{BaseWindow, RouteAttributes, StopRouteIndex};
