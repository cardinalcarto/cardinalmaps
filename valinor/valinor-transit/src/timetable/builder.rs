//! The in-memory builder that `valinor-gtfs` targets.
//!
//! The builder takes stops, calendars, patterns, and trips in the caller's own handles.
//! Building renumbers stops so that stops on the same route are close together,
//! which keeps the label reads of a route scan on neighboring cache lines.

use std::collections::BTreeSet;

use crate::error::BuildError;
use crate::ids::{
    DaySlot, RouteId, Seconds, ServiceDate, ServiceDay, ServiceId, StopId, Time, TripId,
};
use crate::timetable::arena::{Arena, BlockRef, offset_of};
use crate::timetable::lanes::{ResolvedBlock, ResolvedTrip, StopEvent, StopRules};
use crate::timetable::window::{BaseWindow, RouteAttributes, StopRouteIndex};

/// A stop as the builder knows it; [`Timetable::stop_id`] gives its final number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StopKey(u32);

/// A pattern added to the builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PatternKey(u32);

/// One stop of a trip, in seconds from the start of its service day.
///
/// Times past midnight are ordinary larger values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopTime {
    /// When the vehicle arrives.
    pub arrival: Time,
    /// When the vehicle departs.
    pub departure: Time,
    /// Whether riders may board and alight here.
    pub rules: StopRules,
}

#[derive(Debug, Clone)]
struct TripRecord {
    pattern: u32,
    trip: TripId,
    service: ServiceId,
    events: Vec<StopTime>,
}

#[derive(Debug, Clone)]
struct PatternRecord {
    stops: Vec<u32>,
    attributes: RouteAttributes,
}

/// Builds a [`Timetable`].
#[derive(Debug, Default)]
pub struct TimetableBuilder {
    stops: Vec<Seconds>,
    services: Vec<BTreeSet<ServiceDate>>,
    patterns: Vec<PatternRecord>,
    trips: Vec<TripRecord>,
}

impl TimetableBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a stop with its minimum change time.
    ///
    /// # Errors
    ///
    /// If there are more stops than a `u32` can number.
    pub fn add_stop(&mut self, min_change: Seconds) -> Result<StopKey, BuildError> {
        let key = StopKey(offset_of(self.stops.len())?);
        self.stops.push(min_change);
        Ok(key)
    }

    /// Adds a calendar: the dates on which its trips run.
    ///
    /// # Errors
    ///
    /// If there are more calendars than a `u32` can number.
    pub fn add_service(
        &mut self,
        dates: impl IntoIterator<Item = ServiceDate>,
    ) -> Result<ServiceId, BuildError> {
        let id = ServiceId::new(offset_of(self.services.len())?);
        self.services.push(dates.into_iter().collect());
        Ok(id)
    }

    /// Adds a pattern: a stop sequence and attributes.
    ///
    /// A pattern's trips may differ in their pickup and drop-off rules
    /// and may overtake each other; lanes account for both.
    /// Trips whose attributes differ belong in different patterns.
    ///
    /// # Errors
    ///
    /// If `stops` has fewer than two stops or contains a stop this builder did not issue.
    pub fn add_pattern(
        &mut self,
        stops: &[StopKey],
        attributes: RouteAttributes,
    ) -> Result<PatternKey, BuildError> {
        if stops.len() < 2 {
            return Err(BuildError::PatternTooShort(stops.len()));
        }
        for stop in stops {
            if stop.0 as usize >= self.stops.len() {
                return Err(BuildError::UnknownStop(stop.0));
            }
        }
        let key = PatternKey(offset_of(self.patterns.len())?);
        self.patterns.push(PatternRecord {
            stops: stops.iter().map(|stop| stop.0).collect(),
            attributes,
        });
        Ok(key)
    }

    /// Adds a trip of a pattern, with one [`StopTime`] per stop.
    ///
    /// # Errors
    ///
    /// If the pattern or service is unknown, the stop count is wrong,
    /// or the times do not move forward along the trip.
    pub fn add_trip(
        &mut self,
        pattern: PatternKey,
        trip: TripId,
        service: ServiceId,
        times: Vec<StopTime>,
    ) -> Result<(), BuildError> {
        let record = self
            .patterns
            .get(pattern.0 as usize)
            .ok_or(BuildError::UnknownPattern(pattern.0))?;
        if service.index() >= self.services.len() {
            return Err(BuildError::UnknownService(service));
        }
        if times.len() != record.stops.len() {
            return Err(BuildError::WrongStopCount {
                trip,
                expected: record.stops.len(),
                got: times.len(),
            });
        }
        check_times(trip, &times)?;
        self.trips.push(TripRecord {
            pattern: pattern.0,
            trip,
            service,
            events: times,
        });
        Ok(())
    }

    /// Numbers the stops and finishes the timetable.
    ///
    /// # Errors
    ///
    /// If the stop or route tables exceed `u32` offsets.
    pub fn build(self) -> Result<Timetable, BuildError> {
        // Number stops along the busiest patterns first, so the stops a route scan touches
        // tend to be neighbors in the label arrays.
        let mut pattern_order: Vec<usize> = (0..self.patterns.len()).collect();
        let mut trips_per_pattern = vec![0_usize; self.patterns.len()];
        for trip in &self.trips {
            trips_per_pattern[trip.pattern as usize] += 1;
        }
        pattern_order.sort_by_key(|&p| core::cmp::Reverse(trips_per_pattern[p]));

        let mut new_id: Vec<Option<StopId>> = vec![None; self.stops.len()];
        let mut next = 0_u32;
        for &p in &pattern_order {
            for &stop in &self.patterns[p].stops {
                if new_id[stop as usize].is_none() {
                    new_id[stop as usize] = Some(StopId::new(next));
                    next += 1;
                }
            }
        }
        // Stops used by no pattern come last.
        let mut stop_map = Vec::with_capacity(self.stops.len());
        for id in &mut new_id {
            let assigned = if let Some(existing) = *id {
                existing
            } else {
                let fresh = StopId::new(next);
                next += 1;
                *id = Some(fresh);
                fresh
            };
            stop_map.push(assigned);
        }

        let mut min_change = vec![0_i32; self.stops.len()];
        for (old, &id) in stop_map.iter().enumerate() {
            min_change[id.index()] = self.stops[old].as_i32();
        }

        let mut routes = Vec::with_capacity(self.patterns.len());
        let mut route_trips: Vec<Vec<TripRecord>> = vec![Vec::new(); self.patterns.len()];
        for trip in self.trips {
            let pattern = trip.pattern as usize;
            route_trips[pattern].push(trip);
        }
        for (pattern, trips) in self.patterns.into_iter().zip(route_trips) {
            let stops: Vec<StopId> = pattern
                .stops
                .iter()
                .map(|&old| stop_map[old as usize])
                .collect();
            routes.push(RouteRecord {
                stops,
                attributes: pattern.attributes,
                trips,
            });
        }

        let index = StopRouteIndex::build(
            min_change.len(),
            routes
                .iter()
                .enumerate()
                .map(|(r, route)| (r, route.stops.as_slice())),
        )?;

        Ok(Timetable {
            stop_map,
            min_change,
            services: self.services,
            routes,
            index,
        })
    }
}

fn check_times(trip: TripId, times: &[StopTime]) -> Result<(), BuildError> {
    for (position, time) in times.iter().enumerate() {
        if time.arrival > time.departure {
            return Err(BuildError::TripTimesOutOfOrder { trip, position });
        }
        if let Some(next) = times.get(position + 1)
            && time.departure > next.arrival
        {
            return Err(BuildError::TripTimesOutOfOrder { trip, position });
        }
    }
    Ok(())
}

#[derive(Debug)]
struct RouteRecord {
    stops: Vec<StopId>,
    attributes: RouteAttributes,
    trips: Vec<TripRecord>,
}

/// Compiled service: stops, routes, trips with service-day-relative times, and calendars.
///
/// No search reads this. A search reads the [`BaseWindow`] made from it by [`Self::materialize`].
#[derive(Debug)]
pub struct Timetable {
    /// From builder order to the final stop number.
    stop_map: Vec<StopId>,
    min_change: Vec<i32>,
    services: Vec<BTreeSet<ServiceDate>>,
    routes: Vec<RouteRecord>,
    index: StopRouteIndex,
}

impl Timetable {
    /// The final number of a stop.
    #[inline]
    pub fn stop_id(&self, key: StopKey) -> StopId {
        self.stop_map[key.0 as usize]
    }

    /// How many stops there are.
    pub fn stop_count(&self) -> usize {
        self.stop_map.len()
    }

    /// How many routes there are.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// The stop sequence of a route.
    pub fn route_stops(&self, route: RouteId) -> Option<&[StopId]> {
        self.routes.get(route.index()).map(|r| r.stops.as_slice())
    }

    /// The trips of a route that run on a date, resolved to times in a window
    /// where that date starts at `offset` seconds.
    ///
    /// This is what an overlay builder starts from when it applies delays to a route.
    ///
    /// # Errors
    ///
    /// If a shifted time leaves the supported range, or the route is unknown.
    pub fn resolved_trips(
        &self,
        route: RouteId,
        date: ServiceDate,
        offset: Time,
    ) -> Result<Vec<ResolvedTrip>, BuildError> {
        let record = self
            .routes
            .get(route.index())
            .ok_or(BuildError::UnknownRoute(route))?;
        let mut trips = Vec::new();
        for trip in &record.trips {
            if !self.services[trip.service.index()].contains(&date) {
                continue;
            }
            let events = trip
                .events
                .iter()
                .map(|event| {
                    Ok(StopEvent {
                        arrival: shifted(event.arrival, offset)?,
                        departure: shifted(event.departure, offset)?,
                        rules: event.rules,
                    })
                })
                .collect::<Result<Vec<_>, BuildError>>()?;
            trips.push(ResolvedTrip {
                trip: trip.trip,
                events,
            });
        }
        Ok(trips)
    }

    /// Lays out the active trips of `days` as a window for searches.
    ///
    /// This is the only place calendars are evaluated:
    /// the route scan then reads contiguous blocks without looking at any trip.
    ///
    /// # Errors
    ///
    /// If there are too many days, or a shifted time leaves the supported range.
    pub fn materialize(&self, days: &[ServiceDay]) -> Result<BaseWindow, BuildError> {
        if days.is_empty() {
            return Err(BuildError::NoDays);
        }
        if days.len() > usize::from(u8::MAX) {
            return Err(BuildError::TooManyDays(days.len()));
        }

        let mut arena = Arena::new();
        let mut directory = Vec::with_capacity(self.routes.len() * days.len());
        let mut attributes = Vec::with_capacity(self.routes.len());
        let mut route_stops = Vec::with_capacity(self.routes.len());
        for (route_index, route) in self.routes.iter().enumerate() {
            let route_id = RouteId::new(offset_of(route_index)?);
            attributes.push(route.attributes);
            let stops_start = arena.push_stops(&route.stops)?;
            route_stops.push((stops_start, offset_of(route.stops.len())?));
            for (slot, day) in days.iter().enumerate() {
                let slot = u8::try_from(slot).map_err(|_| BuildError::TooManyDays(days.len()))?;
                let trips = self.resolved_trips(route_id, day.date, day.offset)?;
                let block = ResolvedBlock::from_trips(trips)?;
                let index = arena.push_block(
                    route_id,
                    DaySlot::new(slot),
                    stops_start,
                    route.stops.len(),
                    &block,
                )?;
                directory.push(BlockRef {
                    arena: 0,
                    block: index,
                });
            }
        }

        Ok(BaseWindow::new(
            arena,
            directory,
            attributes,
            route_stops,
            self.min_change.clone(),
            self.index.clone(),
            days.to_vec(),
        ))
    }
}

/// Shifts a service-day-relative time onto a window's clock.
fn shifted(time: Time, offset: Time) -> Result<Time, BuildError> {
    let sum = i64::from(time.into_inner()) + i64::from(offset.into_inner());
    i32::try_from(sum)
        .ok()
        .and_then(|sum| Time::try_new(sum).ok())
        .ok_or(BuildError::TimeOutOfRange(sum))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(seconds: i32) -> Time {
        Time::try_new(seconds).expect("in range")
    }

    fn secs(seconds: u32) -> Seconds {
        Seconds::try_new(seconds).expect("in range")
    }

    fn date(d: u32) -> ServiceDate {
        ServiceDate::new(d)
    }

    fn stop_time(time: i32) -> StopTime {
        StopTime {
            arrival: t(time),
            departure: t(time),
            rules: StopRules::REGULAR,
        }
    }

    #[test]
    fn stops_are_numbered_along_busy_patterns_first() {
        let mut builder = TimetableBuilder::new();
        let stops: Vec<StopKey> = (0..5)
            .map(|_| builder.add_stop(secs(0)).expect("add"))
            .collect();
        let service = builder.add_service([date(1)]).expect("service");
        // The quiet pattern, added first, runs on stops 0 and 4.
        let quiet = builder
            .add_pattern(&[stops[0], stops[4]], RouteAttributes::NONE)
            .expect("pattern");
        // The busy pattern runs on stops 1 to 3.
        let busy = builder
            .add_pattern(&[stops[1], stops[2], stops[3]], RouteAttributes::NONE)
            .expect("pattern");
        builder
            .add_trip(
                quiet,
                TripId::new(0),
                service,
                vec![stop_time(0), stop_time(10)],
            )
            .expect("trip");
        for id in 1..4 {
            builder
                .add_trip(
                    busy,
                    TripId::new(id),
                    service,
                    vec![stop_time(0), stop_time(10), stop_time(20)],
                )
                .expect("trip");
        }
        let timetable = builder.build().expect("build");
        let numbers: Vec<u32> = stops
            .iter()
            .map(|&s| timetable.stop_id(s).into_inner())
            .collect();
        assert_eq!(&numbers[1..4], &[0, 1, 2]);
    }

    #[test]
    fn trips_must_match_their_pattern() {
        let mut builder = TimetableBuilder::new();
        let a = builder.add_stop(secs(0)).expect("add");
        let b = builder.add_stop(secs(0)).expect("add");
        let service = builder.add_service([date(1)]).expect("service");
        let pattern = builder
            .add_pattern(&[a, b], RouteAttributes::NONE)
            .expect("pattern");
        assert_eq!(
            builder.add_trip(pattern, TripId::new(5), service, vec![stop_time(0)]),
            Err(BuildError::WrongStopCount {
                trip: TripId::new(5),
                expected: 2,
                got: 1,
            }),
        );
        assert_eq!(
            builder.add_trip(
                pattern,
                TripId::new(6),
                service,
                vec![stop_time(10), stop_time(5)]
            ),
            Err(BuildError::TripTimesOutOfOrder {
                trip: TripId::new(6),
                position: 0,
            }),
        );
    }

    #[test]
    fn short_patterns_are_rejected() {
        let mut builder = TimetableBuilder::new();
        let a = builder.add_stop(secs(0)).expect("add");
        assert_eq!(
            builder.add_pattern(&[a], RouteAttributes::NONE),
            Err(BuildError::PatternTooShort(1)),
        );
    }

    #[test]
    fn materialize_selects_active_trips_and_shifts_them() {
        let mut builder = TimetableBuilder::new();
        let a = builder.add_stop(secs(0)).expect("add");
        let b = builder.add_stop(secs(0)).expect("add");
        let weekday = builder.add_service([date(1), date(2)]).expect("service");
        let sunday = builder.add_service([date(3)]).expect("service");
        let pattern = builder
            .add_pattern(&[a, b], RouteAttributes::NONE)
            .expect("pattern");
        builder
            .add_trip(
                pattern,
                TripId::new(1),
                weekday,
                vec![stop_time(100), stop_time(200)],
            )
            .expect("trip");
        builder
            .add_trip(
                pattern,
                TripId::new(2),
                sunday,
                vec![stop_time(150), stop_time(250)],
            )
            .expect("trip");
        let timetable = builder.build().expect("build");
        let window = timetable
            .materialize(&[
                ServiceDay {
                    date: date(1),
                    offset: t(0),
                },
                ServiceDay {
                    date: date(2),
                    offset: t(86_400),
                },
                ServiceDay {
                    date: date(3),
                    offset: t(172_800),
                },
            ])
            .expect("materialize");
        // Trip 1 on day 2 is on the window's clock.
        let trips = window.lane_trips_for_test(RouteId::new(0), DaySlot::new(1));
        assert_eq!(trips, vec![(1, vec![86_500, 86_500, 86_600, 86_600])]);
        let trips = window.lane_trips_for_test(RouteId::new(0), DaySlot::new(2));
        assert_eq!(trips, vec![(2, vec![172_950, 172_950, 173_050, 173_050])]);
    }

    #[test]
    fn inactive_dates_get_the_empty_block() {
        let mut builder = TimetableBuilder::new();
        let a = builder.add_stop(secs(0)).expect("add");
        let b = builder.add_stop(secs(0)).expect("add");
        let service = builder.add_service([date(9)]).expect("service");
        let pattern = builder
            .add_pattern(&[a, b], RouteAttributes::NONE)
            .expect("pattern");
        builder
            .add_trip(
                pattern,
                TripId::new(1),
                service,
                vec![stop_time(0), stop_time(10)],
            )
            .expect("trip");
        let window = builder
            .build()
            .expect("build")
            .materialize(&[ServiceDay {
                date: date(1),
                offset: t(0),
            }])
            .expect("materialize");
        assert!(
            window
                .lane_trips_for_test(RouteId::new(0), DaySlot::new(0))
                .is_empty()
        );
    }

    #[test]
    fn no_days_is_an_error() {
        let builder = TimetableBuilder::new();
        let timetable = builder.build().expect("build");
        assert_eq!(timetable.materialize(&[]).err(), Some(BuildError::NoDays));
    }
}
