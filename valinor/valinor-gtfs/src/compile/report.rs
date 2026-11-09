//! What compiling a feed found: counts and problems worth a human's attention.

use std::collections::BTreeMap;

/// Why a trip was left out of the timetable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DropReason {
    /// The `trip_id` repeats one seen earlier.
    DuplicateTripId,
    /// `route_id` isn't in `routes.txt`.
    UnknownRoute,
    /// `service_id` is in neither calendar file.
    UnknownService,
    /// A `stop_id` isn't in `stops.txt`.
    UnknownStop,
    /// A stop can't be boarded (a station, or a stop without coordinates).
    UnboardableStop,
    /// Two rows of the trip share a `stop_sequence`.
    DuplicateStopSequence,
    /// A time couldn't be parsed.
    BadTime,
    /// The trip's first or last stop has no time, so the rest can't be interpolated.
    MissingEndpointTimes,
    /// Times go backward along the trip.
    TimesOutOfOrder,
    /// A time is too large to represent.
    TimeOutOfRange,
    /// The trip has fewer than two stops.
    TooFewStops,
}

/// Counts for one feed, the first thing to look at after a compile.
///
/// `Debug` output is meant for logs and comparison with independently computed figures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedReport {
    /// The feed's ID.
    pub feed_id: String,
    /// Rows of `agency.txt`.
    pub agencies: usize,
    /// Rows of `routes.txt` that were kept.
    pub routes: usize,
    /// Rows of `stops.txt`.
    pub stops_listed: usize,
    /// Timetable stops: rows that riders can board.
    pub boardable_stops: usize,
    /// Boardable stops used by at least one kept trip.
    pub stops_used_by_trips: usize,
    /// Service IDs defined in the calendar files.
    pub services_listed: usize,
    /// Service IDs used by at least one kept trip.
    pub services_used: usize,
    /// Rows of `trips.txt`.
    pub trips_listed: usize,
    /// Trips kept from `trips.txt`, before expansion.
    pub trips_kept: usize,
    /// Timetable trips after expanding `frequencies.txt`.
    pub trip_runs: usize,
    /// Runs created by frequency expansion.
    pub frequency_runs: usize,
    /// Rows of `stop_times.txt`.
    pub stop_times_listed: usize,
    /// Total stop events of kept trips, after expansion.
    pub stop_events: usize,
    /// Patterns: trips with the same route, stop sequence, and attributes.
    pub patterns: usize,
    /// Patterns in which some stop appears twice.
    pub loop_patterns: usize,
    /// Trips (before expansion) whose last arrival is at or after 24:00.
    pub trips_past_midnight: usize,
    /// Latest time of any stop event, in seconds from the start of the service day.
    pub latest_time: i32,
    /// Stop times that were interpolated.
    pub interpolated_stop_times: usize,
    /// Rows with a stop headsign.
    pub stop_headsigns: usize,
    /// Distinct shapes used.
    pub shapes: usize,
    /// Shape points stored.
    pub shape_points: usize,
    /// Rows of `transfers.txt` the core can express as a minimum change time.
    pub transfers_applied: usize,
    /// Rows of `transfers.txt` the core can't express.
    pub transfers_ignored: usize,
    /// Trips left out, by reason.
    pub dropped_trips: BTreeMap<DropReason, usize>,
    /// How many non-fatal problems were found.
    pub warning_count: usize,
    /// The first [`WARNING_LIMIT`] problems.
    pub warnings: Vec<String>,
}

/// The most warnings a report keeps text for.
pub const WARNING_LIMIT: usize = 200;

impl FeedReport {
    pub(super) fn new(feed_id: String) -> Self {
        Self {
            feed_id,
            agencies: 0,
            routes: 0,
            stops_listed: 0,
            boardable_stops: 0,
            stops_used_by_trips: 0,
            services_listed: 0,
            services_used: 0,
            trips_listed: 0,
            trips_kept: 0,
            trip_runs: 0,
            frequency_runs: 0,
            stop_times_listed: 0,
            stop_events: 0,
            patterns: 0,
            loop_patterns: 0,
            trips_past_midnight: 0,
            latest_time: 0,
            interpolated_stop_times: 0,
            stop_headsigns: 0,
            shapes: 0,
            shape_points: 0,
            transfers_applied: 0,
            transfers_ignored: 0,
            dropped_trips: BTreeMap::new(),
            warning_count: 0,
            warnings: Vec::new(),
        }
    }

    pub(super) fn warn(&mut self, text: impl FnOnce() -> String) {
        self.warning_count += 1;
        if self.warnings.len() < WARNING_LIMIT {
            self.warnings.push(text());
        }
    }

    pub(super) fn dropped(&mut self, reason: DropReason) {
        *self.dropped_trips.entry(reason).or_insert(0) += 1;
    }
}
