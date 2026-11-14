//! Errors from the state owner and the query API.

use thiserror::Error;
use valinor_transit::{BuildError, SearchError};

/// Why the state owner could not do what was asked.
#[derive(Debug, Error)]
pub enum StateError {
    /// The compiled timetable was rejected while being loaded or materialized.
    #[error("load the compiled timetable: {0}")]
    Timetable(#[from] valinor_gtfs::DecodeError),
    /// The compiled timetables can't be loaded together.
    #[error("number the compiled timetables together: {0}")]
    Set(#[from] valinor_gtfs::SetError),
    /// A service window could not be laid out.
    #[error("materialize a service window: {0}")]
    Window(#[from] BuildError),
    /// The overlay could not be built from the timetable and feeds.
    ///
    /// Feed content never causes this; only an internal disagreement between tables does.
    #[error("build a real-time overlay: {0}")]
    Overlay(#[from] valinor_gtfs_rt::OverlayError),
    /// A moment falls outside every date the compiled timetable covers.
    #[error("find a service date in the compiled timetable for POSIX time {0}")]
    DateNotCovered(i64),
    /// GTFS-RT bytes arrived for a feed that isn't in the compiled timetable.
    #[error("accept real-time data for unknown feed {0:?}")]
    UnknownFeed(String),
    /// Stops couldn't be snapped to the street network.
    #[error("snap stops to the street network: {0}")]
    Snap(#[from] valinor_walk::WalkError),
    /// A request needs more service days than one window may hold.
    #[error("build a window of {0} service days")]
    WindowTooLong(usize),
}

/// Why a query failed.
#[derive(Debug, Error)]
pub enum PlanError<T: core::error::Error + 'static, A: core::error::Error + 'static> {
    /// The request itself is invalid.
    #[error("use request: {0}")]
    InvalidRequest(&'static str),
    /// The timetable state couldn't serve the query.
    #[error(transparent)]
    State(#[from] StateError),
    /// The access and egress search failed.
    #[error("compute access and egress tables: {0}")]
    AccessEgress(#[source] A),
    /// The router failed.
    #[error("run a transit search: {0}")]
    Search(SearchError<T>),
    /// The caller's interrupt asked the query to stop.
    #[error("finish a query that was interrupted")]
    Interrupted,
    /// A time in the request is too far from the service window to be represented.
    #[error("represent a time of {0} seconds from the window's origin")]
    TimeOutOfRange(i64),
    /// Something the request names by ID isn't in the timetable or the pinned real-time data.
    #[error("find the {0} the request names")]
    NotFound(Missing),
}

/// What a request named that couldn't be found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// A route, by its feed and `route_id`.
    Route,
    /// A stop, by its feed and `stop_id`.
    Stop,
    /// A trip, by its `trip_id` and `start_date`, among the stop's departures.
    Trip,
}

impl Missing {
    /// The name used in messages and in the JSON error's `kind`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Stop => "stop",
            Self::Trip => "trip",
        }
    }
}

impl core::fmt::Display for Missing {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}
