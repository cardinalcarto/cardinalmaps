//! Service alerts: what riders are shown, and which of them can't be routed on.
//!
//! Every alert is carried for display in [`crate::rider::RiderData`].
//! Only `NO_SERVICE` changes routing; it removes what it informs, for the times it is active.
//! `DETOUR`, `STOP_MOVED`, and the other effects never do.

use std::collections::{HashMap, HashSet};

use valinor_transit::{DaySlot, RouteId, StopEvent, StopId, TripId};

use crate::date::CalendarDate;
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
            start: range
                .start
                .filter(|&start| start != 0)
                .and_then(|start| i64::try_from(start).ok()),
            end: range
                .end
                .filter(|&end| end != 0)
                .and_then(|end| i64::try_from(end).ok()),
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

/// One informed entity, compiled against the timetable: all fields present must match.
#[derive(Debug)]
struct CompiledSelector {
    agency: Option<String>,
    route: Option<String>,
    route_type: Option<i32>,
    direction: Option<u32>,
    /// Per day, the trip this selector names; `None` when the selector names no trip.
    trip: Option<HashMap<DaySlot, TripId>>,
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
pub(crate) fn compile_no_service<L: StaticLookup + ?Sized>(
    alert: &Alert,
    feed: &FeedId,
    rider_index: usize,
    lookup: &L,
    clocks: &[DayClock],
    added: &HashMap<(ScopedId, CalendarDate), (TripId, DaySlot)>,
    resolve_stop: &dyn Fn(&str) -> Option<StopId>,
) -> Option<NoServiceAlert> {
    if alert.effect() != Effect::NoService {
        return None;
    }
    let selectors = alert
        .informed_entity
        .iter()
        .filter_map(|selector| {
            compile_selector(selector, feed, lookup, clocks, added, resolve_stop)
        })
        .collect();
    Some(NoServiceAlert {
        rider_index,
        feed: feed.clone(),
        periods: service_periods(alert),
        selectors,
    })
}

/// Compiles one selector; `None` if it has no fields, which informs nothing.
fn compile_selector<L: StaticLookup + ?Sized>(
    selector: &EntitySelector,
    feed: &FeedId,
    lookup: &L,
    clocks: &[DayClock],
    added: &HashMap<(ScopedId, CalendarDate), (TripId, DaySlot)>,
    resolve_stop: &dyn Fn(&str) -> Option<StopId>,
) -> Option<CompiledSelector> {
    let trip_descriptor = selector.trip.as_ref();
    let mut trip_routes = Vec::new();
    let trip = trip_descriptor
        .and_then(|d| d.trip_id.as_deref())
        .map(|id| {
            let date = trip_descriptor
                .and_then(|d| d.start_date.as_deref())
                .and_then(CalendarDate::parse);
            let mut days = HashMap::new();
            for clock in clocks {
                if date.is_some_and(|d| d != clock.date) {
                    continue;
                }
                let query = TripQuery {
                    trip_id: id,
                    date: clock.date,
                    start_time: None,
                };
                if let Some(found) = lookup.trip(feed, &query) {
                    days.insert(found.day, found.trip);
                    trip_routes.push(found.route);
                } else if let Some(&(trip, day)) = added.get(&(ScopedId::new(feed, id), clock.date))
                {
                    days.insert(day, trip);
                }
            }
            days
        });
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
        .or_else(|| trip_descriptor.and_then(|d| d.route_id.clone()));
    let direction = selector
        .direction_id
        .or_else(|| trip_descriptor.and_then(|d| d.direction_id));
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
        || compiled.direction.is_some()
        || compiled.trip.is_some()
        || compiled.stops.is_some())
    .then_some(compiled)
}

/// What a trip-level question needs of an alert's selector.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TripIdentity<'a> {
    pub(crate) feed: &'a FeedId,
    pub(crate) route: Option<RouteInfo<'a>>,
    pub(crate) direction: Option<u32>,
    pub(crate) trip: TripId,
    pub(crate) day: DaySlot,
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
                if lookup.route_info(route).is_some_and(|info| {
                    info.feed == &self.feed && selector.route_level_matches(info)
                }) {
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
            if !selector.trip_level_matches(identity) {
                continue;
            }
            match &selector.stops {
                Some(selected) => {
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
        self.agency
            .as_deref()
            .is_none_or(|agency| Some(agency) == info.agency_id)
            && self
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
        if self
            .trip
            .as_ref()
            .is_some_and(|days| days.get(&identity.day) != Some(&identity.trip))
        {
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
