//! Compiles static GTFS feeds into a single file for `valinor-transit`.
//!
//! See `docs/transit.md` for the design this crate serves. In outline:
//!
//! - [`compile`] turns GTFS feeds (zip archives or directories) into the bytes of one file.
//! - [`CompiledTimetable::parse`] loads such a file without copying it, validating every byte,
//!   so a truncated, corrupt, or wrong-version file is an error rather than a panic.
//! - [`CompiledTimetable::to_timetable`] gives a [`valinor_transit::Timetable`],
//!   and [`CompiledTimetable::service_window`] gives the service days to materialize,
//!   using UTC offsets resolved when the file was compiled.
//! - [`TimetableSet`] numbers several compiled files as one, leaving each file as it is.
//! - [`CompiledTimetable`] also answers the questions a response layer and GTFS-RT need,
//!   by feed-scoped GTFS identifiers.

#[cfg(feature = "compile")]
mod compile;
mod error;
pub(crate) mod format;

pub use error::{DecodeError, LoadError};
pub use format::reader::{
    Accessibility, AgencyIndex, AgencyInfo, CompiledTimetable, FeedInfo, GtfsRoute, LatLon,
    LoadedTimetable, LocationId, LocationType, PatternInfo, RouteInfo, ServiceWindow, ShapeSlice,
    StopInfo, TripInfo, TripRun, civil_date_of, parse_service_date, service_date_of,
};
pub use format::set::{SetError, TimetablePart, TimetableSet};
pub use format::{DEFAULT_TRANSFER_SECONDS, FORMAT_VERSION, MAGIC};

#[cfg(feature = "compile")]
pub use compile::{
    CompiledOutput, DirectorySource, DropReason, FeedId, FeedIdError, FeedInput, FeedReport,
    GtfsSource, InMemorySource, WARNING_LIMIT, ZipSource, compile,
};
#[cfg(feature = "compile")]
pub use error::CompileError;

#[cfg(test)]
mod regression_tests;
#[cfg(test)]
mod scenario_tests;
