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
use crate::timetable::pairs::{PairRule, TransferPair, TransferPairs};
use crate::timetable::window::NO_CHANGE;
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
    pairs: Vec<(u32, u32, PairRule)>,
    /// Stops where a rider arriving by trip can't board another there.
    no_change: Vec<bool>,
    /// Stops marked as not accessible to a rider in a wheelchair.
    inaccessible: Vec<bool>,
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
        self.no_change.push(false);
        self.inaccessible.push(false);
        Ok(key)
    }

    /// Forbids changing at a stop: a rider arriving there by trip can't board another there.
    ///
    /// # Errors
    ///
    /// If the stop wasn't issued by this builder.
    pub fn forbid_change_at(&mut self, stop: StopKey) -> Result<(), BuildError> {
        *self
            .no_change
            .get_mut(stop.0 as usize)
            .ok_or(BuildError::UnknownStop(stop.0))? = true;
        Ok(())
    }

    /// Marks a stop as not accessible: a rider in a wheelchair can't board or alight there.
    ///
    /// # Errors
    ///
    /// If the stop wasn't issued by this builder.
    pub fn mark_stop_inaccessible(&mut self, stop: StopKey) -> Result<(), BuildError> {
        *self
            .inaccessible
            .get_mut(stop.0 as usize)
            .ok_or(BuildError::UnknownStop(stop.0))? = true;
        Ok(())
    }

    /// Adds a rule for walking from one stop to another.
    ///
    /// # Errors
    ///
    /// If either stop wasn't issued by this builder, or they are the same stop.
    pub fn add_transfer_pair(
        &mut self,
        from: StopKey,
        to: StopKey,
        rule: PairRule,
    ) -> Result<(), BuildError> {
        for stop in [from, to] {
            if stop.0 as usize >= self.stops.len() {
                return Err(BuildError::UnknownStop(stop.0));
            }
        }
        if from == to {
            return Err(BuildError::TransferToSameStop(StopId::new(from.0)));
        }
        self.pairs.push((from.0, to.0, rule));
        Ok(())
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
        let mut no_change = vec![false; self.stops.len()];
        let mut stop_access = vec![true; self.stops.len()];
        for (old, &id) in stop_map.iter().enumerate() {
            min_change[id.index()] = self.stops[old].as_i32();
            no_change[id.index()] = self.no_change[old];
            stop_access[id.index()] = !self.inaccessible[old];
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

        let pairs = self
            .pairs
            .iter()
            .map(|&(from, to, rule)| TransferPair {
                from: stop_map[from as usize],
                to: stop_map[to as usize],
                rule,
            })
            .collect();
        Ok(Timetable {
            stop_map,
            min_change,
            no_change,
            stop_access,
            services: self.services,
            routes,
            pairs,
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
    no_change: Vec<bool>,
    stop_access: Vec<bool>,
    services: Vec<BTreeSet<ServiceDate>>,
    routes: Vec<RouteRecord>,
    pairs: Vec<TransferPair>,
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
        let offsets: Vec<Option<Time>> = days.iter().map(|day| Some(day.offset)).collect();
        materialize_parts(
            &[WindowPart {
                timetable: self,
                first_trip: 0,
                day_offsets: &offsets,
            }],
            days,
        )
    }
}

/// Whatever a window can be materialized from: a [`Timetable`] on the heap, or a borrowed view.
///
/// Numbers are the source's own, from zero. [`materialize_parts`] offsets them.
///
/// # Performance
///
/// Everything here is on the cold path of building a window, not in the route scan.
pub trait TimetableSource {
    /// How many stops there are.
    fn stop_count(&self) -> usize;

    /// How many routes there are.
    fn route_count(&self) -> usize;

    /// The minimum change time at a stop, in seconds.
    fn min_change(&self, stop: usize) -> i32;

    /// Whether a rider arriving at this stop by trip may not board another there.
    ///
    /// False by default: most stops have no such rule.
    fn no_change_at(&self, _stop: usize) -> bool {
        false
    }

    /// Whether a rider in a wheelchair may use this stop (only an explicit `2` says no).
    fn wheelchair_stop_accessible(&self, _stop: usize) -> bool {
        true
    }

    /// The rules for walking between particular stops, in the source's numbering.
    ///
    /// Empty by default: most sources have none.
    fn transfer_pairs(&self) -> Vec<TransferPair> {
        Vec::new()
    }

    /// The stop sequence of a route, in the source's numbering.
    ///
    /// # Errors
    ///
    /// If the route is unknown.
    fn route_stops_of(&self, route: RouteId) -> Result<Vec<StopId>, BuildError>;

    /// The attributes of a route.
    ///
    /// # Errors
    ///
    /// If the route is unknown.
    fn route_attributes_of(&self, route: RouteId) -> Result<RouteAttributes, BuildError>;

    /// The trips of a route that run on a date, resolved to times in a window
    /// where that date starts at `offset` seconds.
    ///
    /// # Errors
    ///
    /// If a shifted time leaves the supported range, or the route is unknown.
    fn resolved_trips_on(
        &self,
        route: RouteId,
        date: ServiceDate,
        offset: Time,
    ) -> Result<Vec<ResolvedTrip>, BuildError>;
}

impl TimetableSource for Timetable {
    fn stop_count(&self) -> usize {
        self.min_change.len()
    }

    fn route_count(&self) -> usize {
        self.routes.len()
    }

    fn min_change(&self, stop: usize) -> i32 {
        self.min_change.get(stop).copied().unwrap_or(0)
    }

    fn no_change_at(&self, stop: usize) -> bool {
        self.no_change.get(stop).copied().unwrap_or(false)
    }

    fn wheelchair_stop_accessible(&self, stop: usize) -> bool {
        self.stop_access.get(stop).copied().unwrap_or(true)
    }

    fn transfer_pairs(&self) -> Vec<TransferPair> {
        self.pairs.clone()
    }

    fn route_stops_of(&self, route: RouteId) -> Result<Vec<StopId>, BuildError> {
        self.routes
            .get(route.index())
            .map(|record| record.stops.clone())
            .ok_or(BuildError::UnknownRoute(route))
    }

    fn route_attributes_of(&self, route: RouteId) -> Result<RouteAttributes, BuildError> {
        self.routes
            .get(route.index())
            .map(|record| record.attributes)
            .ok_or(BuildError::UnknownRoute(route))
    }

    fn resolved_trips_on(
        &self,
        route: RouteId,
        date: ServiceDate,
        offset: Time,
    ) -> Result<Vec<ResolvedTrip>, BuildError> {
        self.resolved_trips(route, date, offset)
    }
}

/// One timetable of a window built from several, in that window's numbering.
///
/// [`materialize_parts`] concatenates its stops and routes after those of the parts before it,
/// so a part's stop number is its own plus the sum of the earlier parts' stops, and likewise for
/// routes. Trips keep the numbers this part's caller gives them, offset by `first_trip`.
#[derive(Clone, Copy)]
pub struct WindowPart<'a> {
    /// The part's own stops, routes, and trips, numbered from zero.
    pub timetable: &'a dyn TimetableSource,
    /// Added to each of this part's trip identifiers, so that parts can't collide.
    pub first_trip: u32,
    /// For each service day of the window, in slot order, where the day starts on the window's
    /// clock for this part, or `None` if this part has no such day.
    ///
    /// This lets parts in different time zones share one clock:
    /// the same service date starts at a different instant in each.
    pub day_offsets: &'a [Option<Time>],
}

/// Lays out the active trips of several timetables as one window for searches.
///
/// Stops, routes, and days are concatenated in `parts` order, in one arena, so the route scan
/// reads one contiguous layout exactly as it does for a single timetable.
/// Nothing about a part is visible after this point.
///
/// `days` gives the date of each slot and is kept as the window's [`BaseWindow::days`];
/// a part's own clock for each slot is in [`WindowPart::day_offsets`].
///
/// # Performance
///
/// Runs once per window, not per query; it evaluates every calendar of every part.
///
/// # Errors
///
/// If there are too many days, a part's `day_offsets` differs from `days` in length,
/// a shifted time leaves the supported range, or a table grows past its `u32` offsets.
#[expect(
    clippy::too_many_lines,
    reason = "one linear concatenation of the parts' tables"
)]
pub fn materialize_parts(
    parts: &[WindowPart<'_>],
    days: &[ServiceDay],
) -> Result<BaseWindow, BuildError> {
    if days.is_empty() {
        return Err(BuildError::NoDays);
    }
    if days.len() > usize::from(u8::MAX) {
        return Err(BuildError::TooManyDays(days.len()));
    }
    for part in parts {
        if part.day_offsets.len() != days.len() {
            return Err(BuildError::PartDayCount {
                expected: days.len(),
                got: part.day_offsets.len(),
            });
        }
    }

    let route_total: usize = parts.iter().map(|part| part.timetable.route_count()).sum();
    let stop_total: usize = parts.iter().map(|part| part.timetable.stop_count()).sum();
    // Checked here so that the per-stop and per-route additions below can't overflow.
    offset_of(route_total)?;
    offset_of(stop_total)?;
    let mut arena = Arena::new();
    let mut directory = Vec::with_capacity(route_total * days.len());
    let mut attributes = Vec::with_capacity(route_total);
    let mut route_stops = Vec::with_capacity(route_total);
    let mut min_change = Vec::with_capacity(stop_total);
    let mut transfer_times = Vec::with_capacity(stop_total);
    let mut pairs = Vec::new();
    let mut stop_access = Vec::with_capacity(stop_total);
    for part in parts {
        let timetable = part.timetable;
        stop_access
            .extend((0..timetable.stop_count()).map(|s| timetable.wheelchair_stop_accessible(s)));
    }
    let mut stop_base = 0_u32;
    let mut route_base = 0_u32;
    let mut stops = Vec::new();
    for part in parts {
        let timetable = part.timetable;
        for local_route in 0..timetable.route_count() {
            let local = RouteId::new(offset_of(local_route)?);
            let route_id = RouteId::new(route_base + offset_of(local_route)?);
            attributes.push(timetable.route_attributes_of(local)?);
            stops.clear();
            stops.extend(
                timetable
                    .route_stops_of(local)?
                    .into_iter()
                    .map(|stop| StopId::new(stop.into_inner() + stop_base)),
            );
            let stops_start = arena.push_stops(&stops)?;
            route_stops.push((stops_start, offset_of(stops.len())?));
            for (slot, day) in days.iter().enumerate() {
                let slot = u8::try_from(slot).map_err(|_| BuildError::TooManyDays(days.len()))?;
                let Some(offset) = part.day_offsets[usize::from(slot)] else {
                    directory.push(BlockRef::EMPTY);
                    continue;
                };
                let mut trips = timetable.resolved_trips_on(local, day.date, offset)?;
                for trip in &mut trips {
                    trip.trip = TripId::new(
                        trip.trip
                            .into_inner()
                            .checked_add(part.first_trip)
                            .ok_or(BuildError::TableTooLarge)?,
                    );
                }
                let block = ResolvedBlock::from_trips(trips)?;
                let index = arena.push_block(
                    route_id,
                    DaySlot::new(slot),
                    stops_start,
                    stops.len(),
                    &block,
                    &stop_access,
                )?;
                directory.push(BlockRef {
                    arena: 0,
                    block: index,
                });
            }
        }
        let stop_count = timetable.stop_count();
        for stop in 0..stop_count {
            let time = timetable.min_change(stop);
            transfer_times.push(time);
            min_change.push(if timetable.no_change_at(stop) {
                NO_CHANGE
            } else {
                time
            });
        }
        pairs.extend(
            timetable
                .transfer_pairs()
                .into_iter()
                .map(|pair| TransferPair {
                    from: StopId::new(pair.from.into_inner() + stop_base),
                    to: StopId::new(pair.to.into_inner() + stop_base),
                    rule: pair.rule,
                }),
        );
        stop_base += offset_of(stop_count)?;
        route_base += offset_of(timetable.route_count())?;
    }

    let pairs = TransferPairs::new(min_change.len(), &pairs)?;
    let index = StopRouteIndex::build(
        min_change.len(),
        route_stops
            .iter()
            .enumerate()
            .map(|(route, &(start, length))| {
                let stops = &arena.stops[start as usize..(start + length) as usize];
                (route, stops)
            }),
    )?;

    Ok(BaseWindow::new(
        arena,
        directory,
        attributes,
        route_stops,
        min_change,
        transfer_times,
        pairs,
        stop_access,
        index,
        days.to_vec(),
    ))
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
    fn parts_share_one_window_with_their_own_clocks() {
        fn one_part(hours_to_day: i32, trip: u32) -> Timetable {
            let mut builder = TimetableBuilder::new();
            let a = builder.add_stop(secs(60)).expect("add");
            let b = builder.add_stop(secs(0)).expect("add");
            let service = builder.add_service([date(10), date(11)]).expect("service");
            let pattern = builder
                .add_pattern(&[a, b], RouteAttributes::NONE)
                .expect("pattern");
            builder
                .add_trip(
                    pattern,
                    TripId::new(trip),
                    service,
                    vec![stop_time(hours_to_day), stop_time(hours_to_day + 600)],
                )
                .expect("trip");
            builder.build().expect("build")
        }
        let first = one_part(0, 0);
        let second = one_part(0, 2);
        // The second part's day 10 starts an hour after the window's origin;
        // its day 11 is outside what it covers.
        let days = [
            ServiceDay {
                date: date(10),
                offset: t(0),
            },
            ServiceDay {
                date: date(11),
                offset: t(86_400),
            },
        ];
        let first_offsets = [Some(t(0)), Some(t(86_400))];
        let second_offsets = [Some(t(3_600)), None];
        let window = materialize_parts(
            &[
                WindowPart {
                    timetable: &first,
                    first_trip: 0,
                    day_offsets: &first_offsets,
                },
                WindowPart {
                    timetable: &second,
                    first_trip: 10,
                    day_offsets: &second_offsets,
                },
            ],
            &days,
        )
        .expect("materialize");

        assert_eq!(window.stop_count(), 4);
        assert_eq!(window.route_count(), 2);
        // Stops of part 2 are numbered after part 1's, and the minimum change times come along.
        assert_eq!(
            window.route_stops(RouteId::new(1)),
            Some(&[StopId::new(2), StopId::new(3)][..])
        );
        assert_eq!(window.min_change, vec![60, 0, 60, 0]);
        // Each part's trips have their own clock and their trip base.
        assert_eq!(
            window.lane_trips_for_test(RouteId::new(0), DaySlot::new(0)),
            vec![(0, vec![0, 0, 600, 600])]
        );
        assert_eq!(
            window.lane_trips_for_test(RouteId::new(1), DaySlot::new(0)),
            vec![(12, vec![3_600, 3_600, 4_200, 4_200])]
        );
        // A day a part doesn't cover has no trips.
        assert!(
            window
                .lane_trips_for_test(RouteId::new(1), DaySlot::new(1))
                .is_empty()
        );
        assert_eq!(
            window.lane_trips_for_test(RouteId::new(0), DaySlot::new(1)),
            vec![(0, vec![86_400, 86_400, 87_000, 87_000])]
        );
    }

    #[test]
    fn a_part_with_the_wrong_day_count_is_an_error() {
        let builder = TimetableBuilder::new();
        let timetable = builder.build().expect("build");
        let offsets = [Some(t(0))];
        let days = [
            ServiceDay {
                date: date(1),
                offset: t(0),
            },
            ServiceDay {
                date: date(2),
                offset: t(86_400),
            },
        ];
        assert_eq!(
            materialize_parts(
                &[WindowPart {
                    timetable: &timetable,
                    first_trip: 0,
                    day_offsets: &offsets,
                }],
                &days
            )
            .err(),
            Some(BuildError::PartDayCount {
                expected: 2,
                got: 1
            }),
        );
    }

    #[test]
    fn no_days_is_an_error() {
        let builder = TimetableBuilder::new();
        let timetable = builder.build().expect("build");
        assert_eq!(timetable.materialize(&[]).err(), Some(BuildError::NoDays));
    }
}
