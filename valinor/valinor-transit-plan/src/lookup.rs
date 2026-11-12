//! Implements [`StaticLookup`] over the loaded timetables, so the overlay builder can see them.
//!
//! Feeds are found by identifier, so each answer comes from the file that has the feed;
//! numbers are the set's. See [`valinor_gtfs::TimetableSet`].

use valinor_gtfs_rt::{
    FeedId, RouteInfo, ScheduledSegment, ScheduledTrip, StaticLookup, TripQuery, TripRef, WindowDay,
};
use valinor_transit::{DaySlot, RouteId, StopId, Time, TimetableSource};

use crate::state::{TimetableData, TransitSnapshot};
use crate::window::WindowData;

/// The compiled timetables and the service window, as the overlay builder sees them.
#[derive(Debug, Clone, Copy)]
pub struct CompiledLookup<'a> {
    data: &'a TimetableData,
    window: &'a WindowData,
}

impl<'a> CompiledLookup<'a> {
    /// The lookup for a snapshot's timetables and window.
    pub fn of(snapshot: &'a TransitSnapshot) -> Self {
        Self::new(&snapshot.data, &snapshot.window)
    }

    pub(crate) fn new(data: &'a TimetableData, window: &'a WindowData) -> Self {
        Self { data, window }
    }

    fn feed_id(&self, name: &str) -> Option<&FeedId> {
        self.data.feeds.iter().find(|feed| feed.as_str() == name)
    }

    /// A service-day-relative time on this window's clock for `offset`.
    fn window_time(offset: Time, seconds: i32) -> Option<Time> {
        Time::try_new(offset.into_inner().checked_add(seconds)?).ok()
    }
}

impl StaticLookup for CompiledLookup<'_> {
    fn window_days(&self) -> &[WindowDay] {
        &self.window.days
    }

    /// The feed's own service-day start, when its file covers this slot's date.
    fn feed_day_start_unix(&self, feed: &FeedId, slot: DaySlot) -> i64 {
        let slot = usize::from(slot.into_inner());
        self.data
            .set
            .part_of_feed(feed.as_str())
            .and_then(|part| {
                self.window
                    .part_days
                    .get(part.index())?
                    .get(slot)
                    .copied()?
            })
            .map(|day| day.start_unix)
            .or_else(|| self.window.days.get(slot).map(|day| day.start_unix))
            .unwrap_or(0)
    }

    /// Resolves a run of a GTFS trip on one service date.
    ///
    /// `exact_times=0` frequency trips resolve to the nearest grid run, by the descriptor's
    /// `start_time` (see [`valinor_gtfs::CompiledTimetable::trip_run_for_start_time`]).
    /// Two vehicles can land on one run, which is why rider data is keyed by `start_time`
    /// for those runs rather than by the run this returns.
    fn trip(&self, feed: &FeedId, query: &TripQuery<'_>) -> Option<TripRef> {
        let slot = self
            .window
            .days
            .iter()
            .position(|day| day.date == query.date)?;
        let date = *self.window.dates.get(slot)?;
        let part = self.data.set.part_of_feed(feed.as_str())?;
        let day = (*self.window.part_days.get(part.index())?.get(slot)?)?;
        let start_time = query.start_time.and_then(|time| i32::try_from(time).ok());
        let compiled = part.compiled();
        let run =
            compiled.trip_run_for_start_time(feed.as_str(), query.trip_id, date, start_time)?;
        let info = compiled.trip(run.trip)?;
        // A combined trip's run is found under each original trip, as that trip's stretch.
        let (first_position, last_position) = run
            .segment
            .map_or((0, info.stop_count().checked_sub(1)?), |(first, last)| {
                (first as usize, last as usize)
            });
        let (_, first_departure) = info.time_at(first_position)?;
        let (last_arrival, _) = info.time_at(last_position)?;
        Some(TripRef {
            trip: part.global_trip(run.trip),
            route: part.global_route(info.pattern),
            day: DaySlot::new(u8::try_from(slot).ok()?),
            first_departure: Self::window_time(day.offset, first_departure)?,
            last_arrival: Self::window_time(day.offset, last_arrival)?,
            headway_start: run.headway_based.then_some(query.start_time).flatten(),
            segment: run.segment,
        })
    }

    fn route_patterns(&self, feed: &FeedId, route_id: &str) -> Vec<RouteId> {
        let Some(part) = self.data.set.part_of_feed(feed.as_str()) else {
            return Vec::new();
        };
        part.compiled()
            .route_by_gtfs_id(feed.as_str(), route_id)
            .and_then(|route| part.compiled().route_patterns(route))
            .unwrap_or_default()
            .into_iter()
            .map(|route| part.global_route(route))
            .collect()
    }

    fn route_info(&self, route: RouteId) -> Option<RouteInfo<'_>> {
        let (part, local) = self.data.set.route_part(route)?;
        let compiled = part.compiled();
        let pattern = compiled.pattern(local)?;
        let info = compiled.route(pattern.route)?;
        Some(RouteInfo {
            feed: self.feed_id(info.feed)?,
            route_id: info.gtfs_id,
            agency_id: info
                .agency
                .and_then(|agency| compiled.agency(agency))
                .and_then(|agency| agency.gtfs_id),
            route_type: i32::from(info.route_type),
        })
    }

    fn route_info_by_id(&self, feed: &FeedId, route_id: &str) -> Option<RouteInfo<'_>> {
        let part = self.data.set.part_of_feed(feed.as_str())?;
        let compiled = part.compiled();
        let route = compiled.route_by_gtfs_id(feed.as_str(), route_id)?;
        let info = compiled.route(route)?;
        Some(RouteInfo {
            feed: self.feed_id(info.feed)?,
            route_id: info.gtfs_id,
            agency_id: info
                .agency
                .and_then(|agency| compiled.agency(agency))
                .and_then(|agency| agency.gtfs_id),
            route_type: i32::from(info.route_type),
        })
    }

    fn combined_patterns_of_route(&self, feed: &FeedId, route_id: &str) -> Vec<RouteId> {
        let Some(part) = self.data.set.part_of_feed(feed.as_str()) else {
            return Vec::new();
        };
        part.compiled()
            .route_by_gtfs_id(feed.as_str(), route_id)
            .map(|route| self.data.seated().patterns_of_route(part.index(), route))
            .unwrap_or_default()
    }

    fn combined_routes_of_pattern(&self, pattern: RouteId) -> Vec<RouteInfo<'_>> {
        self.data
            .seated()
            .routes_of_pattern(pattern)
            .iter()
            .filter_map(|&(part, route)| {
                let compiled = self.data.set.parts().nth(part)?.compiled();
                let info = compiled.route(route)?;
                Some(RouteInfo {
                    feed: self.feed_id(info.feed)?,
                    route_id: info.gtfs_id,
                    agency_id: info
                        .agency
                        .and_then(|agency| compiled.agency(agency))
                        .and_then(|agency| agency.gtfs_id),
                    route_type: i32::from(info.route_type),
                })
            })
            .collect()
    }

    fn stop(&self, feed: &FeedId, stop_id: &str) -> Option<StopId> {
        let part = self.data.set.part_of_feed(feed.as_str())?;
        part.compiled()
            .stop_by_gtfs_id(feed.as_str(), stop_id)
            .map(|stop| part.global_stop(stop))
    }

    fn stop_gtfs_id(&self, feed: &FeedId, stop: StopId) -> Option<&str> {
        let (part, local) = self.data.set.stop_part(stop)?;
        part.compiled()
            .stop(local)
            .filter(|info| info.feed == feed.as_str())
            .map(|info| info.gtfs_id)
    }

    fn child_stops(&self, feed: &FeedId, stop_id: &str) -> Vec<StopId> {
        let Some(part) = self.data.set.part_of_feed(feed.as_str()) else {
            return Vec::new();
        };
        let compiled = part.compiled();
        let Some(location) = compiled.location_by_gtfs_id(feed.as_str(), stop_id) else {
            return Vec::new();
        };
        compiled
            .stops_at(location)
            .unwrap_or_default()
            .into_iter()
            .map(|stop| part.global_stop(stop))
            .filter(|stop| self.stop_gtfs_id(feed, *stop) != Some(stop_id))
            .collect()
    }

    fn scheduled_block(&self, route: RouteId, day: DaySlot) -> Vec<ScheduledTrip> {
        let Some((part, local)) = self.data.set.route_part(route) else {
            return Vec::new();
        };
        let slot = usize::from(day.into_inner());
        let Some(Some(part_day)) = self
            .window
            .part_days
            .get(part.index())
            .and_then(|days| days.get(slot).copied())
        else {
            // This file has no such service day, so it has no trips.
            return Vec::new();
        };
        let Some(date) = self.window.dates.get(slot) else {
            return Vec::new();
        };
        let Ok(trips) = part
            .compiled()
            .resolved_trips_on(local, *date, part_day.offset)
        else {
            return Vec::new();
        };
        let compiled = part.compiled();
        trips
            .into_iter()
            .map(|trip| {
                let info = compiled.trip(trip.trip);
                let stop_count = trip.events.len();
                ScheduledTrip {
                    trip: part.global_trip(trip.trip),
                    direction_id: info.and_then(|i| i.direction).map(u32::from),
                    segments: compiled
                        .trip_segments(trip.trip)
                        .into_iter()
                        .map(|segment| ScheduledSegment {
                            trip_id: segment.gtfs_id.to_owned(),
                            first: segment.first_position,
                            last: segment.last_position,
                            route_id: compiled
                                .route(segment.route)
                                .map(|route| route.gtfs_id.to_owned())
                                .unwrap_or_default(),
                            direction_id: segment.direction.map(u32::from),
                            headsign: segment.headsign.map(str::to_owned),
                        })
                        .collect(),
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
        // Every static trip's ID is below the set's total.
        u32::try_from(self.data.set.trip_count()).unwrap_or(u32::MAX)
    }
}
