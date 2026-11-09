//! Errors from building timetables, overlays, and from running searches.

use thiserror::Error;

use crate::ids::{DaySlot, RouteId, ServiceId, StopId, TripId};

/// Why a timetable or an overlay block could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BuildError {
    /// A stop handle was not issued by this builder.
    #[error("use unknown stop {0}")]
    UnknownStop(u32),
    /// A pattern handle was not issued by this builder.
    #[error("use unknown pattern {0}")]
    UnknownPattern(u32),
    /// A service was not issued by this builder.
    #[error("use unknown service {0}")]
    UnknownService(ServiceId),
    /// A pattern has fewer than two stops, so nothing can be ridden on it.
    #[error("build a pattern with {0} stops")]
    PatternTooShort(usize),
    /// A trip has a different number of stop events from its pattern.
    #[error("match trip {trip} ({got} stop events) to a pattern of {expected} stops")]
    WrongStopCount {
        /// The trip with the wrong number of events.
        trip: TripId,
        /// How many stops the pattern has.
        expected: usize,
        /// How many events the trip gave.
        got: usize,
    },
    /// A trip's times don't move forward along the trip,
    /// or its arrival at a stop is after its departure.
    #[error("order the times of trip {trip} at position {position}")]
    TripTimesOutOfOrder {
        /// The trip in question.
        trip: TripId,
        /// The stop position where the order broke.
        position: usize,
    },
    /// Two trips in the same lane overtake one another.
    #[error("keep the trips of a lane from overtaking at position {position}")]
    LaneOvertakes {
        /// The stop position where the overtake happens.
        position: usize,
    },
    /// A shifted time left the supported range.
    #[error("represent a time out of range: {0}")]
    TimeOutOfRange(i64),
    /// A table grew past what its `u32` offsets can address.
    #[error("index more than 2^32 timetable entries")]
    TableTooLarge,
    /// The window has more days than a [`DaySlot`] can name.
    #[error("materialize a window of {0} days")]
    TooManyDays(usize),
    /// A window was asked for no days.
    #[error("materialize a window with no days")]
    NoDays,
    /// A route doesn't exist.
    #[error("use unknown route {0}")]
    UnknownRoute(RouteId),
    /// A service day slot doesn't exist in the window.
    #[error("use unknown day slot {0}")]
    UnknownDay(DaySlot),
    /// A stop doesn't exist in the timetable.
    #[error("use unknown stop {0}")]
    UnknownStopId(StopId),
}

/// Why a query was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QueryError {
    /// A stop table was built for a different number of stops.
    #[error("use a stop table of {got} stops with a timetable of {expected} stops")]
    StopTableSize {
        /// Stops in the snapshot.
        expected: usize,
        /// Stops in the table.
        got: usize,
    },
    /// `max_trips` is outside of what the search supports.
    #[error("limit a search to {0} trips")]
    MaxTrips(u8),
    /// The window ends before it starts.
    #[error("search a window that ends before it starts")]
    EmptyWindow,
}

/// Why a search stopped without a result.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SearchError<E> {
    /// The caller's interrupt asked the search to stop.
    #[error("finish a search that was interrupted")]
    Interrupted,
    /// The query is invalid.
    #[error(transparent)]
    Query(#[from] QueryError),
    /// The transfer search failed.
    #[error("run a transfer search: {0}")]
    Transfer(E),
}
