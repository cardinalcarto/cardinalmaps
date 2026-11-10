//! What the overlay builder needs to know about the static timetable.
//!
//! Compiled GTFS implements [`StaticLookup`]; this crate never reads GTFS files.
//! Trips are matched by `trip_id` and `start_date` only.
//! GTFS-RT entity IDs are unstable (King County prefixes them with a timestamp),
//! so nothing here keys on them.

use valinor_transit::{DaySlot, RouteId, StopEvent, StopId, Time, TripId};

use crate::date::CalendarDate;
use crate::feed::FeedId;

/// One day of the window that the overlay is built over, in slot order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowDay {
    /// The calendar date the day runs.
    pub date: CalendarDate,
    /// When the day's service-day clock starts, in POSIX seconds (noon minus twelve hours).
    ///
    /// Combined with the window's [`valinor_transit::ServiceDay::offset`],
    /// this converts between GTFS-RT's POSIX times and window times.
    pub start_unix: i64,
}

/// A static trip being looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TripQuery<'a> {
    /// The trip identifier from the trip descriptor.
    pub trip_id: &'a str,
    /// The service date of the run.
    pub date: CalendarDate,
    /// The run's start in seconds from the day's start, from `start_time` when the feed gives one.
    ///
    /// This distinguishes runs of a `frequencies.txt` trip;
    /// other implementations may ignore it.
    pub start_time: Option<u32>,
}

/// A static trip on one service day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TripRef {
    /// The trip, as identified to the timetable.
    pub trip: TripId,
    /// The stop pattern the trip belongs to.
    pub route: RouteId,
    /// The day slot of this run.
    pub day: DaySlot,
    /// The first departure, in window time.
    pub first_departure: Time,
    /// The last arrival, in window time.
    pub last_arrival: Time,
    /// The descriptor's `start_time`, for a run of an `exact_times=0` frequency trip.
    ///
    /// Such a run is a template laid out by the compiler, and two real vehicles can resolve
    /// to the same one, so the run alone does not identify a vehicle. `None` for any other run.
    pub headway_start: Option<u32>,
    /// For an original trip that is part of a combined stay-seated trip,
    /// its inclusive `(first, last)` positions in that trip.
    pub segment: Option<(u32, u32)>,
}

/// One original trip's stretch of a combined stay-seated trip, in a schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledSegment {
    /// The first position of this trip's stretch.
    pub first: u32,
    /// The last position of this trip's stretch, inclusive.
    pub last: u32,
    /// The GTFS `route_id` of the original trip.
    pub route_id: String,
    /// The GTFS `direction_id`, when given.
    pub direction_id: Option<u32>,
}

/// The GTFS identity of a stop pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteInfo<'a> {
    /// The feed the route belongs to.
    pub feed: &'a FeedId,
    /// The GTFS `route_id`.
    pub route_id: &'a str,
    /// The GTFS `agency_id`.
    pub agency_id: Option<&'a str>,
    /// The GTFS `route_type`.
    pub route_type: i32,
}

/// One scheduled trip of a route on a day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTrip {
    /// The trip, as identified to the timetable.
    pub trip: TripId,
    /// The GTFS `direction_id`, when the trip has one.
    pub direction_id: Option<u32>,
    /// One event per stop of the route, in window time.
    pub events: Vec<StopEvent>,
    /// The GTFS `stop_sequence` of each stop, parallel to `events`.
    ///
    /// GTFS-RT addresses a trip's stops by these, and they needn't be consecutive.
    pub stop_sequences: Vec<u32>,
    /// For a combined stay-seated trip, each original trip's stretch; empty for any other trip.
    pub segments: Vec<ScheduledSegment>,
}

/// The static timetable, as seen by the overlay builder.
///
/// A timetable covering several feeds resolves identifiers within the given [`FeedId`],
/// so identifiers of different agencies never collide.
pub trait StaticLookup {
    /// The days of the window in slot order, one per [`valinor_transit::BaseWindow::days`] entry.
    fn window_days(&self) -> &[WindowDay];

    /// When `feed`'s own service day of slot `slot` starts, in POSIX seconds.
    ///
    /// Files loaded together can each have their own service-day clock, in different time zones:
    /// the same date starts at a different instant in each. A lookup over one clock can use this
    /// default, which is the day of [`Self::window_days`]; a feed that doesn't cover a date
    /// may answer with that same value, since no trip of it can be found on that date anyway.
    fn feed_day_start_unix(&self, _feed: &FeedId, slot: DaySlot) -> i64 {
        self.window_days()
            .get(usize::from(slot.into_inner()))
            .map_or(0, |day| day.start_unix)
    }

    /// Finds a trip of `feed` on one service day.
    ///
    /// Returns `None` when the trip doesn't exist or doesn't run on that date.
    fn trip(&self, feed: &FeedId, query: &TripQuery<'_>) -> Option<TripRef>;

    /// The stop patterns that a GTFS `route_id` has, in any order.
    fn route_patterns(&self, feed: &FeedId, route_id: &str) -> Vec<RouteId>;

    /// The GTFS identity of a stop pattern.
    fn route_info(&self, route: RouteId) -> Option<RouteInfo<'_>>;

    /// The GTFS identity of a route by its `route_id`, for one original trip of a combined one.
    fn route_info_by_id(&self, _feed: &FeedId, _route_id: &str) -> Option<RouteInfo<'_>> {
        None
    }

    /// Finds a static stop.
    fn stop(&self, feed: &FeedId, stop_id: &str) -> Option<StopId>;

    /// The GTFS `stop_id` of a static stop, for rider-facing data.
    fn stop_gtfs_id(&self, feed: &FeedId, stop: StopId) -> Option<&str>;

    /// The platforms of a station, when `stop_id` names a parent station.
    ///
    /// A selector that names a station covers its platforms.
    fn child_stops(&self, _feed: &FeedId, _stop_id: &str) -> Vec<StopId> {
        Vec::new()
    }

    /// The scheduled trips of a route on one day, in any order.
    ///
    /// Trips must carry window times, exactly as the base window was built from them.
    fn scheduled_block(&self, route: RouteId, day: DaySlot) -> Vec<ScheduledTrip>;

    /// The first [`TripId`] value not used by any static trip.
    ///
    /// Added trips are numbered from here so they can't collide with static ones.
    /// Their numbers are only meaningful within the overlay that issued them.
    fn first_unused_trip_id(&self) -> u32;
}

/// Maps POSIX time to window time on one service day.
///
/// All GTFS-RT arithmetic goes through this, so a run with `start_date` yesterday
/// resolves correctly however far past midnight it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DayClock {
    pub(crate) date: CalendarDate,
    pub(crate) slot: DaySlot,
    offset: i64,
    start_unix: i64,
}

impl DayClock {
    pub(crate) fn new(day: WindowDay, slot: DaySlot, offset: Time) -> Self {
        Self {
            date: day.date,
            slot,
            offset: i64::from(offset.into_inner()),
            start_unix: day.start_unix,
        }
    }

    /// This clock moved to a feed whose service day starts at `start_unix` instead.
    ///
    /// The window's clock is unchanged: both the day's start and its offset move together,
    /// so window times and POSIX times keep converting identically.
    pub(crate) fn at_feed_start(self, start_unix: i64) -> Self {
        Self {
            offset: self
                .offset
                .saturating_add(start_unix.saturating_sub(self.start_unix)),
            start_unix,
            ..self
        }
    }

    /// The window seconds of a POSIX moment on this day's clock, unchecked for range.
    ///
    /// Feeds can carry any 64-bit time; saturating here means a nonsense time
    /// falls out of range when it reaches [`Time::try_new`], rather than overflowing.
    #[inline]
    pub(crate) fn window_seconds(self, unix: i64) -> i64 {
        unix.saturating_sub(self.start_unix)
            .saturating_add(self.offset)
    }

    /// When this day's service-day clock starts, in POSIX seconds.
    #[inline]
    pub(crate) fn start_unix(self) -> i64 {
        self.start_unix
    }

    /// Seconds from the start of this service day to a window time on it.
    #[inline]
    pub(crate) fn relative(self, seconds: i32) -> i64 {
        i64::from(seconds) - self.offset
    }

    /// The POSIX moment of a window time on this day's clock.
    #[inline]
    pub(crate) fn unix(self, seconds: i32) -> i64 {
        i64::from(seconds) - self.offset + self.start_unix
    }
}
