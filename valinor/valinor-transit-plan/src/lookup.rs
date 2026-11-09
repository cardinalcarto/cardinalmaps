//! Implements [`StaticLookup`] over a compiled timetable, so the overlay builder can see it.

use valinor_gtfs::CompiledTimetable;
use valinor_gtfs_rt::{
    FeedId, RouteInfo, ScheduledTrip, StaticLookup, TripQuery, TripRef, WindowDay,
};
use valinor_transit::{DaySlot, RouteId, ServiceDay, StopId, Time, Timetable};

/// The compiled timetable and the service window, as the overlay builder sees them.
///
/// `days` and `service_days` describe the same window in the same order: the first says when each
/// day starts in POSIX seconds, the second how far each day is along the window's clock.
#[derive(Debug, Clone, Copy)]
pub struct CompiledLookup<'a> {
    compiled: &'a CompiledTimetable<'a>,
    timetable: &'a Timetable,
    days: &'a [WindowDay],
    service_days: &'a [ServiceDay],
    feeds: &'a [FeedId],
}

impl<'a> CompiledLookup<'a> {
    /// A lookup over one window of one compiled timetable.
    ///
    /// `feeds` must hold a [`FeedId`] for each feed in `compiled`, so routes can be attributed.
    pub fn new(
        compiled: &'a CompiledTimetable<'a>,
        timetable: &'a Timetable,
        days: &'a [WindowDay],
        service_days: &'a [ServiceDay],
        feeds: &'a [FeedId],
    ) -> Self {
        Self {
            compiled,
            timetable,
            days,
            service_days,
            feeds,
        }
    }

    fn feed_id(&self, name: &str) -> Option<&FeedId> {
        self.feeds.iter().find(|feed| feed.as_str() == name)
    }

    /// A service-day-relative time on this window's clock for `offset`.
    fn window_time(offset: Time, seconds: i32) -> Option<Time> {
        Time::try_new(offset.into_inner().checked_add(seconds)?).ok()
    }
}

impl StaticLookup for CompiledLookup<'_> {
    fn window_days(&self) -> &[WindowDay] {
        self.days
    }

    /// Resolves a run of a GTFS trip on one service date.
    ///
    /// `exact_times=0` frequency trips resolve to the nearest grid run, by the descriptor's
    /// `start_time` (see [`CompiledTimetable::trip_run_for_start_time`]).
    /// Two vehicles can land on one run, which is why rider data is keyed by `start_time`
    /// for those runs rather than by the run this returns.
    fn trip(&self, feed: &FeedId, query: &TripQuery<'_>) -> Option<TripRef> {
        let slot = self.days.iter().position(|day| day.date == query.date)?;
        let day = self.service_days.get(slot)?;
        let start_time = query.start_time.and_then(|time| i32::try_from(time).ok());
        let run = self.compiled.trip_run_for_start_time(
            feed.as_str(),
            query.trip_id,
            day.date,
            start_time,
        )?;
        let info = self.compiled.trip(run.trip)?;
        let (_, first_departure) = info.time_at(0)?;
        let (last_arrival, _) = info.time_at(info.stop_count().checked_sub(1)?)?;
        Some(TripRef {
            trip: run.trip,
            route: info.pattern,
            day: DaySlot::new(u8::try_from(slot).ok()?),
            first_departure: Self::window_time(day.offset, first_departure)?,
            last_arrival: Self::window_time(day.offset, last_arrival)?,
        })
    }

    fn route_patterns(&self, feed: &FeedId, route_id: &str) -> Vec<RouteId> {
        self.compiled
            .route_by_gtfs_id(feed.as_str(), route_id)
            .and_then(|route| self.compiled.route_patterns(route))
            .unwrap_or_default()
    }

    fn route_info(&self, route: RouteId) -> Option<RouteInfo<'_>> {
        let pattern = self.compiled.pattern(route)?;
        let info = self.compiled.route(pattern.route)?;
        Some(RouteInfo {
            feed: self.feed_id(info.feed)?,
            route_id: info.gtfs_id,
            agency_id: info
                .agency
                .and_then(|agency| self.compiled.agency(agency))
                .and_then(|agency| agency.gtfs_id),
            route_type: i32::from(info.route_type),
        })
    }

    fn stop(&self, feed: &FeedId, stop_id: &str) -> Option<StopId> {
        self.compiled.stop_by_gtfs_id(feed.as_str(), stop_id)
    }

    fn stop_gtfs_id(&self, feed: &FeedId, stop: StopId) -> Option<&str> {
        self.compiled
            .stop(stop)
            .filter(|info| info.feed == feed.as_str())
            .map(|info| info.gtfs_id)
    }

    fn child_stops(&self, feed: &FeedId, stop_id: &str) -> Vec<StopId> {
        let Some(location) = self.compiled.location_by_gtfs_id(feed.as_str(), stop_id) else {
            return Vec::new();
        };
        self.compiled
            .stops_at(location)
            .unwrap_or_default()
            .into_iter()
            .filter(|stop| self.stop_gtfs_id(feed, *stop) != Some(stop_id))
            .collect()
    }

    fn scheduled_block(&self, route: RouteId, day: DaySlot) -> Vec<ScheduledTrip> {
        let Some(service_day) = self.service_days.get(usize::from(day.into_inner())) else {
            return Vec::new();
        };
        let Ok(trips) = self
            .timetable
            .resolved_trips(route, service_day.date, service_day.offset)
        else {
            return Vec::new();
        };
        trips
            .into_iter()
            .map(|trip| {
                let info = self.compiled.trip(trip.trip);
                let stop_count = trip.events.len();
                ScheduledTrip {
                    trip: trip.trip,
                    direction_id: info.and_then(|i| i.direction).map(u32::from),
                    stop_sequences: (0..stop_count)
                        .map(|position| {
                            info.and_then(|i| i.stop_sequence_at(position))
                                // Sequences are required by the compiler; positions keep the
                                // array parallel if a file is somehow missing one.
                                .unwrap_or(u32::try_from(position).unwrap_or(u32::MAX))
                        })
                        .collect(),
                    events: trip.events,
                }
            })
            .collect()
    }

    fn first_unused_trip_id(&self) -> u32 {
        // Every static trip's ID is its row number in the file.
        u32::try_from(self.compiled.trip_count()).unwrap_or(u32::MAX)
    }
}
