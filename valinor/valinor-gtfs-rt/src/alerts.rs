//! Service alerts: what riders are shown, and which of them can't be routed on.
//!
//! Every alert is carried for display in [`crate::rider::RiderData`].
//! Only `NO_SERVICE` changes routing; it removes what it informs, for the times it is active.
//! `DETOUR`, `STOP_MOVED`, and the other effects never do.
//!
//! The work alerts cause is the product of their selectors and the trips they might reach,
//! both under a feed's control, so all of it draws on one [`AlertBudget`].

use std::collections::{HashMap, HashSet};

use valinor_transit::{DaySlot, RouteId, StopEvent, StopId, TripId};

use crate::date::{CalendarDate, parse_start_time};
use crate::feed::{FeedId, ScopedId};
use crate::lookup::{DayClock, RouteInfo, ScheduledTrip, StaticLookup, TripQuery};
use crate::proto::alert::Effect;
use crate::proto::{Alert, EntitySelector, TimeRange};

/// A half-open time range: active at `t` when `start <= t < end`, an absent bound being infinite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Period {
    /// The first active moment, in POSIX seconds.
    pub start: Option<i64>,
    /// The first moment no longer active, in POSIX seconds.
    pub end: Option<i64>,
}

impl Period {
    pub(crate) fn from_range(range: &TimeRange) -> Self {
        Self {
            // A bound past `i64` is effectively forever: it must not become an open start.
            start: range
                .start
                .filter(|&start| start != 0)
                .map(|start| i64::try_from(start).unwrap_or(i64::MAX)),
            end: range
                .end
                .filter(|&end| end != 0)
                .map(|end| i64::try_from(end).unwrap_or(i64::MAX)),
        }
    }

    /// Whether this period overlaps the inclusive interval `from..=to`.
    #[inline]
    pub(crate) fn overlaps(&self, from: i64, to: i64) -> bool {
        self.start.is_none_or(|start| start <= to) && self.end.is_none_or(|end| from < end)
    }
}

/// When an alert changes service: its `impact_period`, else the older `active_period`.
///
/// `impact_period` says when the service is affected,
/// and `communication_period` is only when to tell riders.
#[allow(deprecated)]
pub(crate) fn service_periods(alert: &Alert) -> Vec<Period> {
    let ranges = if alert.impact_period.is_empty() {
        &alert.active_period
    } else {
        &alert.impact_period
    };
    ranges.iter().map(Period::from_range).collect()
}

/// How much selector matching one build may do, in single checks, across all feeds.
///
/// Far beyond any real feed (King County's 121 selectors cost a few thousand checks), and
/// small enough to finish quickly on a phone. Each feed gets an equal share of it;
/// see [`crate::overlay`]'s budgets.
pub(crate) const ALERT_WORK_LIMIT: u64 = 50_000_000;

/// Counts down the work one feed's alerts may cause in one build.
///
/// When it runs out that feed's remaining alerts stop applying, and the build reports it:
/// a hostile feed degrades its own routing rather than hanging the device or anyone else's.
#[derive(Debug)]
pub(crate) struct AlertBudget {
    remaining: u64,
    exhausted: bool,
}

impl AlertBudget {
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            remaining: limit,
            exhausted: false,
        }
    }

    /// Spends `work`, returning whether there was enough.
    #[inline]
    fn charge(&mut self, work: usize) -> bool {
        let work = u64::try_from(work).unwrap_or(u64::MAX);
        if self.remaining < work {
            self.remaining = 0;
            self.exhausted = true;
            return false;
        }
        self.remaining -= work;
        true
    }

    /// Whether any work was refused.
    pub(crate) fn exhausted(&self) -> bool {
        self.exhausted
    }
}

/// The run a selector names on one service day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NamedRun {
    trip: TripId,
    /// The original trip's stretch, when the selector names one trip of a combined run.
    segment: Option<(u32, u32)>,
}

/// One informed entity, compiled against the timetable: all fields present must match.
#[derive(Debug)]
struct CompiledSelector {
    agency: Option<String>,
    route: Option<String>,
    route_type: Option<i32>,
    direction: Option<u32>,
    /// Per day, the run this selector names; `None` when the selector names no trip.
    trip: Option<HashMap<DaySlot, NamedRun>>,
    /// The static patterns of the trip this selector names.
    trip_routes: Vec<RouteId>,
    /// The stops this selector names, with a station's platforms; `None` when it names no stop.
    stops: Option<HashSet<StopId>>,
}

/// A `NO_SERVICE` alert compiled against the timetable.
#[derive(Debug)]
pub(crate) struct NoServiceAlert {
    /// The alert's position in the rider store.
    pub(crate) rider_index: usize,
    feed: FeedId,
    periods: Vec<Period>,
    selectors: Vec<CompiledSelector>,
}

/// Compiles `alert` if it removes service, resolving its selectors against the timetable.
///
/// `resolve_stop` finds both static and overlay stops,
/// and `added` finds the trips the overlay has added.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_no_service<L: StaticLookup + ?Sized>(
    alert: &Alert,
    feed: &FeedId,
    rider_index: usize,
    lookup: &L,
    clocks: &[DayClock],
    added: &HashMap<(ScopedId, CalendarDate), (TripId, DaySlot)>,
    resolve_stop: &dyn Fn(&str) -> Option<StopId>,
    invalid_selectors: &mut usize,
    budget: &mut AlertBudget,
) -> Option<NoServiceAlert> {
    if alert.effect() != Effect::NoService {
        return None;
    }
    let mut selectors = Vec::with_capacity(alert.informed_entity.len());
    for selector in &alert.informed_entity {
        if !budget.charge(1) {
            break;
        }
        match compile_selector(selector, feed, lookup, clocks, added, resolve_stop, budget) {
            Some(compiled) => selectors.push(compiled),
            None => *invalid_selectors += 1,
        }
    }
    Some(NoServiceAlert {
        rider_index,
        feed: feed.clone(),
        periods: service_periods(alert),
        selectors,
    })
}

/// Compiles one selector; `None` if it is empty or invalid, and so informs nothing.
///
/// Invalid means:
/// - `direction_id` without `route_id`, which the specification forbids;
/// - a trip descriptor whose `start_date` or `start_time` can't be parsed,
///   since it would otherwise silently widen to every run;
/// - a trip descriptor without a `trip_id` that doesn't give the route, direction, start date,
///   and start time that together identify one run.
fn compile_selector<L: StaticLookup + ?Sized>(
    selector: &EntitySelector,
    feed: &FeedId,
    lookup: &L,
    clocks: &[DayClock],
    added: &HashMap<(ScopedId, CalendarDate), (TripId, DaySlot)>,
    resolve_stop: &dyn Fn(&str) -> Option<StopId>,
    budget: &mut AlertBudget,
) -> Option<CompiledSelector> {
    if selector.direction_id.is_some() && selector.route_id.is_none() {
        return None;
    }
    // An empty descriptor says nothing, and is the same as none at all.
    let descriptor = selector.trip.as_ref().filter(|d| {
        d.trip_id.is_some()
            || d.route_id.is_some()
            || d.direction_id.is_some()
            || d.start_time.is_some()
            || d.start_date.is_some()
    });
    let mut trip_routes = Vec::new();
    let trip = match descriptor {
        None => None,
        Some(descriptor) => {
            let date = match descriptor.start_date.as_deref() {
                Some(text) => Some(CalendarDate::parse(text)?),
                None => None,
            };
            let start_time = match descriptor.start_time.as_deref() {
                Some(text) => Some(parse_start_time(text)?),
                None => None,
            };
            let days = if let Some(id) = descriptor.trip_id.as_deref() {
                let mut days = HashMap::new();
                for clock in clocks {
                    if date.is_some_and(|d| d != clock.date) {
                        continue;
                    }
                    let query = TripQuery {
                        trip_id: id,
                        date: clock.date,
                        start_time,
                    };
                    if let Some(&(trip, day)) = added.get(&(ScopedId::new(feed, id), clock.date)) {
                        // An added or replaced trip stands in for any static one of that name.
                        let segment = None;
                        days.insert(day, NamedRun { trip, segment });
                    } else if let Some(found) = lookup.trip(feed, &query) {
                        let run = NamedRun {
                            trip: found.trip,
                            segment: found.segment,
                        };
                        days.insert(found.day, run);
                        trip_routes.push(found.route);
                    }
                }
                days
            } else {
                let run = OneRun {
                    route_id: descriptor.route_id.as_deref()?,
                    direction: descriptor.direction_id?,
                    date: date?,
                    start_time: start_time?,
                };
                find_run(&run, feed, lookup, clocks, &mut trip_routes, budget)
            };
            Some(days)
        }
    };
    let stops = selector.stop_id.as_deref().map(|stop_id| {
        let mut set = HashSet::new();
        if let Some(stop) = resolve_stop(stop_id) {
            set.insert(stop);
        }
        set.extend(lookup.child_stops(feed, stop_id));
        set
    });
    // Each field is optional, and the trip descriptor's own route and direction count too.
    let route = selector
        .route_id
        .clone()
        .or_else(|| descriptor.and_then(|d| d.route_id.clone()));
    let direction = selector
        .direction_id
        .or_else(|| descriptor.and_then(|d| d.direction_id));
    let compiled = CompiledSelector {
        agency: selector.agency_id.clone(),
        route,
        route_type: selector.route_type,
        direction,
        trip,
        trip_routes,
        stops,
    };
    (compiled.agency.is_some()
        || compiled.route.is_some()
        || compiled.route_type.is_some()
        || compiled.trip.is_some()
        || compiled.stops.is_some())
    .then_some(compiled)
}

/// The fields that, without a `trip_id`, identify a single run of a trip.
struct OneRun<'a> {
    route_id: &'a str,
    direction: u32,
    date: CalendarDate,
    start_time: u32,
}

/// Finds the scheduled trip a [`OneRun`] names.
///
/// A combined run is found by each original trip's own route, direction, and start;
/// such a selector names that stretch.
fn find_run<L: StaticLookup + ?Sized>(
    run: &OneRun<'_>,
    feed: &FeedId,
    lookup: &L,
    clocks: &[DayClock],
    routes: &mut Vec<RouteId>,
    budget: &mut AlertBudget,
) -> HashMap<DaySlot, NamedRun> {
    let mut days = HashMap::new();
    let Some(clock) = clocks.iter().copied().find(|c| c.date == run.date) else {
        return days;
    };
    let patterns = lookup
        .route_patterns(feed, run.route_id)
        .into_iter()
        .chain(lookup.combined_patterns_of_route(feed, run.route_id));
    for pattern in patterns {
        let block = lookup.scheduled_block(pattern, clock.slot);
        let work: usize = block.iter().map(|trip| 1 + trip.segments.len()).sum();
        if !budget.charge(work) {
            break;
        }
        for trip in block {
            let departs_at = |position: usize| {
                trip.events
                    .get(position)
                    .map(|event| clock.relative(event.departure.into_inner()))
            };
            let named = if trip.segments.is_empty() {
                (trip.direction_id == Some(run.direction)
                    && departs_at(0) == Some(i64::from(run.start_time)))
                .then_some(NamedRun {
                    trip: trip.trip,
                    segment: None,
                })
            } else {
                trip.segments
                    .iter()
                    .find(|segment| {
                        segment.route_id == run.route_id
                            && segment.direction_id == Some(run.direction)
                            && departs_at(segment.first as usize) == Some(i64::from(run.start_time))
                    })
                    .map(|segment| NamedRun {
                        trip: trip.trip,
                        segment: Some((segment.first, segment.last)),
                    })
            };
            if let Some(named) = named {
                days.insert(clock.slot, named);
                routes.push(pattern);
            }
        }
    }
    days
}

/// What a trip-level question needs of an alert's selector.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TripIdentity<'a> {
    pub(crate) feed: &'a FeedId,
    pub(crate) route: Option<RouteInfo<'a>>,
    pub(crate) direction: Option<u32>,
    pub(crate) trip: TripId,
    pub(crate) day: DaySlot,
    /// The original trip's stretch, when this asks about one trip of a combined run.
    pub(crate) segment: Option<(u32, u32)>,
}

/// What an alert does to a trip.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AlertEffect {
    /// The whole trip is removed.
    pub(crate) remove: bool,
    /// Positions that lose their stop.
    pub(crate) stops_removed: Vec<usize>,
}

impl NoServiceAlert {
    /// The feed this alert came from.
    pub(crate) fn feed(&self) -> &FeedId {
        &self.feed
    }

    /// Whether this alert is active in the inclusive interval of POSIX times.
    fn active(&self, from: i64, to: i64) -> bool {
        self.periods.is_empty() || self.periods.iter().any(|p| p.overlaps(from, to))
    }

    /// Stop patterns this alert may affect, and the days it may affect them.
    ///
    /// The result is deliberately broad; [`Self::effect_on`] decides per trip, per event.
    pub(crate) fn candidate_routes<L: StaticLookup + ?Sized>(
        &self,
        lookup: &L,
        base_routes: u32,
        routes_of_stop: &mut dyn FnMut(StopId) -> Vec<RouteId>,
    ) -> Vec<RouteId> {
        let mut out = HashSet::new();
        for selector in &self.selectors {
            if let Some(route_id) = &selector.route {
                out.extend(lookup.route_patterns(&self.feed, route_id));
                // A combined trip's pattern is its first original trip's route, not this one.
                out.extend(lookup.combined_patterns_of_route(&self.feed, route_id));
                out.extend(selector.trip_routes.iter().copied());
                continue;
            }
            if selector.trip.is_some() {
                out.extend(selector.trip_routes.iter().copied());
                continue;
            }
            if let Some(stops) = &selector.stops {
                for &stop in stops {
                    out.extend(routes_of_stop(stop));
                }
                continue;
            }
            // Agency, route type, or direction alone: check every route.
            for raw in 0..base_routes {
                let route = RouteId::new(raw);
                let on_this_route = |info: RouteInfo<'_>| {
                    info.feed == &self.feed && selector.route_level_matches(info)
                };
                // A combined trip's parts can differ from its pattern's route in agency or type.
                if lookup.route_info(route).is_some_and(on_this_route)
                    || lookup
                        .combined_routes_of_pattern(route)
                        .into_iter()
                        .any(on_this_route)
                {
                    out.insert(route);
                }
            }
        }
        let mut routes: Vec<RouteId> = out.into_iter().collect();
        routes.sort_unstable();
        routes
    }

    /// Evaluates this alert for one trip at its actual (real-time) event times.
    pub(crate) fn effect_on(
        &self,
        identity: TripIdentity<'_>,
        stops: &[StopId],
        events: &[StopEvent],
        clock: DayClock,
        budget: &mut AlertBudget,
    ) -> AlertEffect {
        let mut effect = AlertEffect::default();
        if identity.feed != &self.feed {
            return effect;
        }
        let (Some(first), Some(last)) = (events.first(), events.last()) else {
            return effect;
        };
        let from = clock.unix(first.departure.into_inner());
        let to = clock.unix(last.arrival.into_inner());
        for selector in &self.selectors {
            if !budget.charge(1) {
                break;
            }
            if !selector.trip_level_matches(identity) {
                continue;
            }
            match &selector.stops {
                Some(selected) => {
                    if !budget.charge(stops.len()) {
                        break;
                    }
                    for (position, (stop, event)) in stops.iter().zip(events).enumerate() {
                        if selected.contains(stop)
                            && self.active(
                                clock.unix(event.arrival.into_inner()),
                                clock.unix(event.departure.into_inner()),
                            )
                        {
                            effect.stops_removed.push(position);
                        }
                    }
                }
                None => {
                    if self.active(from, to) {
                        effect.remove = true;
                    }
                }
            }
        }
        effect.stops_removed.sort_unstable();
        effect.stops_removed.dedup();
        effect
    }
}

impl CompiledSelector {
    fn route_level_matches(&self, info: RouteInfo<'_>) -> bool {
        // `routes.txt` may omit `agency_id` when a feed has one agency,
        // and then any agency this feed's alert names can only be that one.
        self.agency.as_deref().is_none_or(|agency| {
            info.agency_id
                .is_none_or(|route_agency| route_agency == agency)
        }) && self
            .route
            .as_deref()
            .is_none_or(|route| route == info.route_id)
            && self
                .route_type
                .is_none_or(|route_type| route_type == info.route_type)
    }

    fn trip_level_matches(&self, identity: TripIdentity<'_>) -> bool {
        if self
            .direction
            .is_some_and(|d| identity.direction != Some(d))
        {
            return false;
        }
        if self.trip.as_ref().is_some_and(|days| {
            !days.get(&identity.day).is_some_and(|run| {
                run.trip == identity.trip
                    && (run.segment.is_none() || run.segment == identity.segment)
            })
        }) {
            return false;
        }
        match identity.route {
            Some(info) => self.route_level_matches(info),
            // An added trip with no known route can only match on what it has.
            None => self.agency.is_none() && self.route.is_none() && self.route_type.is_none(),
        }
    }
}

/// The identity of a scheduled trip, for alert matching.
pub(crate) fn scheduled_identity<'a>(
    feed: &'a FeedId,
    trip: &ScheduledTrip,
    route: Option<RouteInfo<'a>>,
    day: DaySlot,
) -> TripIdentity<'a> {
    TripIdentity {
        feed,
        route,
        direction: trip.direction_id,
        trip: trip.trip,
        day,
        segment: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_are_half_open() {
        let period = Period {
            start: Some(10),
            end: Some(20),
        };
        assert!(period.overlaps(10, 10));
        assert!(period.overlaps(19, 19));
        assert!(!period.overlaps(20, 20));
        assert!(!period.overlaps(5, 9));
        assert!(period.overlaps(5, 25));
    }

    #[test]
    fn open_ended_periods_extend_forever() {
        let open_start = Period {
            start: None,
            end: Some(10),
        };
        assert!(open_start.overlaps(-1000, -1000));
        assert!(!open_start.overlaps(10, 10));
        let open_end = Period {
            start: Some(10),
            end: None,
        };
        assert!(open_end.overlaps(10, 10));
        assert!(!open_end.overlaps(5, 9));
    }

    #[test]
    fn a_start_beyond_i64_is_never_rather_than_always() {
        let period = Period::from_range(&TimeRange {
            start: Some(u64::MAX),
            end: None,
        });
        assert!(!period.overlaps(1_000, 2_000));
    }

    #[test]
    fn an_end_beyond_i64_is_effectively_open() {
        let period = Period::from_range(&TimeRange {
            start: Some(10),
            end: Some(u64::MAX),
        });
        assert!(period.overlaps(1_000, 2_000));
    }

    #[test]
    fn the_budget_refuses_work_once_spent() {
        let mut budget = AlertBudget::new(ALERT_WORK_LIMIT);
        assert!(budget.charge(usize::try_from(ALERT_WORK_LIMIT).expect("64-bit only")));
        assert!(!budget.exhausted());
        assert!(!budget.charge(1));
        assert!(budget.exhausted());
    }

    #[test]
    fn zero_bounds_count_as_absent() {
        let period = Period::from_range(&TimeRange {
            start: Some(0),
            end: Some(0),
        });
        assert_eq!(
            period,
            Period {
                start: None,
                end: None
            }
        );
    }
}
