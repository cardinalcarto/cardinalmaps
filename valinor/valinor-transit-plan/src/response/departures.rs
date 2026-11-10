//! A ride's other departures: its route between the same two stops, around the journey's own.
//!
//! The route is the rider's: the GTFS routes a trip rides from the boarding stop to the alighting
//! stop, each once in a row. A departure is listed when its trip rides the same ones,
//! so a stay-seated ride lists trips that make the same change, and a ride on one route lists
//! combined trips that are still on that route between the two stops.
//!
//! The timetable is read as routing reads it, from the pinned snapshot alone:
//! every pattern serving the boarding stop and then the alighting stop (the occurrences routing
//! scans), every service day of the window, every lane, and the rules a query of the request's
//! travel mode reads. A departure the overlay removed is found in the schedule's block,
//! which the overlay leaves in place: canceled when real-time data or an alert canceled its trip,
//! and skipped when the trip still runs but no longer serves one of the two stops.
//!
//! # Performance
//!
//! A ride reads the occurrences at its boarding stop, the stop sequences of those patterns up to
//! the alighting stop, and per lane a binary search and the rows in the time bound.
//! Only blocks the overlay changed are read twice. Describing a departure resolves its trip as
//! a leg does, without stop names or a vehicle's position.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use valinor_gtfs::GtfsRoute;
use valinor_gtfs_rt::TripState;
use valinor_transit::{
    DaySlot, LaneRef, Ride, RouteFilter, RouteId, ServiceDate, Snapshot, StopEvent, StopId,
    StopRules, Time, TripId, TripKey,
};

use crate::plan::AccessEgressSearch;
use crate::response::build::{
    Builder, gtfs_route_of, leg_accessibility, stop_headsign, trip_accessibility,
    trip_bikes_allowed, trip_headsign,
};
use crate::response::{AlertResponse, DepartureStatus, RideDeparture, TransitLeg};
use crate::window::calendar_date;

/// A pattern serving a ride's boarding stop and then its alighting stop, at these positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Pair {
    route: RouteId,
    board: u32,
    alight: u32,
}

/// What became of a departure the schedule has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    /// It runs between the two stops, in the snapshot.
    Runs,
    Canceled,
    Skipped,
}

/// One departure between the ride's stops, read from a lane.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    pair: Pair,
    lane: LaneRef,
    row: u32,
    key: TripKey,
    board: StopEvent,
    alight: StopEvent,
    found: Found,
}

impl Candidate {
    /// The order departures are listed in.
    fn order(&self) -> (Time, Time, ServiceDate, TripId, u32) {
        (
            self.board.departure,
            self.alight.arrival,
            self.key.date,
            self.key.trip,
            self.pair.board,
        )
    }
}

/// The GTFS routes a trip rides between two positions, each once in a row, by file.
type RiddenRoutes = Vec<(usize, GtfsRoute)>;

/// What one plan has already worked out about departures, since its journeys share most of them.
#[derive(Debug, Default)]
pub(super) struct DepartureCache {
    /// Each departure described, by its run and its pair of positions.
    described: HashMap<(TripKey, Pair), Rc<Described>>,
    /// The latest departure of a route between two stops on a service date, on the window clock.
    latest: HashMap<(StopId, StopId, RiddenRoutes, ServiceDate), Option<Time>>,
}

/// A departure's leg-independent description.
#[derive(Debug)]
struct Described {
    departure: RideDeparture,
    alerts: Vec<AlertResponse>,
}

impl<A: AccessEgressSearch> Builder<'_, A> {
    /// The departures of a ride's route between its stops, and the alerts on them that aren't
    /// the leg's own.
    ///
    /// `ready` is the earliest the rider can be at the boarding stop, in POSIX seconds.
    pub(super) fn ride_departures(
        &self,
        ride: &Ride,
        ready: i64,
        leg: &TransitLeg,
    ) -> (Vec<RideDeparture>, Vec<AlertResponse>) {
        let snapshot: &Snapshot = &self.snapshot.snapshot;
        let filter = self.request.mode.route_filter();
        let range = self.request.options.departures;
        let mut routes = RiddenRoutes::new();
        self.ridden_routes(
            ride.trip,
            ride.route,
            ride.board_position,
            ride.alight_position,
            &mut routes,
        );

        let own = Candidate {
            pair: Pair {
                route: ride.route,
                board: ride.board_position,
                alight: ride.alight_position,
            },
            lane: ride.lane,
            row: ride.row,
            key: ride.trip,
            board: snapshot
                .lane_event(ride.lane, ride.row, ride.board_position)
                .unwrap_or(StopEvent {
                    arrival: ride.board_time,
                    departure: ride.board_time,
                    rules: StopRules::REGULAR,
                }),
            alight: snapshot
                .lane_event(ride.lane, ride.row, ride.alight_position)
                .unwrap_or(StopEvent {
                    arrival: ride.alight_time,
                    departure: ride.alight_time,
                    rules: StopRules::REGULAR,
                }),
            found: Found::Runs,
        };
        // A trip that no GTFS route names has no route to list others of,
        // nor to say whether it is that route's last.
        let pairs = if routes.is_empty() {
            Vec::new()
        } else {
            pairs_for(snapshot, filter, ride)
        };
        let mut candidates = if routes.is_empty() {
            Vec::new()
        } else {
            let board = ride.board_time.into_inner();
            let seconds = |count: u8, seconds: u32| {
                if count == 0 {
                    0
                } else {
                    i32::try_from(seconds).unwrap_or(i32::MAX)
                }
            };
            let bounds = (
                board.saturating_sub(seconds(range.before, range.before_seconds)),
                board.saturating_add(seconds(range.after, range.after_seconds)),
            );
            self.candidates(&pairs, filter, bounds, &own, &routes)
        };
        candidates.sort_unstable_by_key(Candidate::order);
        let at = candidates.partition_point(|c| c.order() < own.order());
        candidates.insert(at, own);
        let first = at.saturating_sub(usize::from(range.before));
        let last = (at + usize::from(range.after)).min(candidates.len() - 1);
        let chosen = &candidates[first..=last];

        let latest = self.latest_by_date(chosen, ride, &pairs, filter, &routes);
        let mut pool: Vec<AlertResponse> = Vec::new();
        let departures = chosen
            .iter()
            .map(|candidate| {
                let is_own = candidate.key == own.key && candidate.pair == own.pair;
                let last_of_day = candidate.found == Found::Runs
                    && latest.iter().any(|&(date, time)| {
                        date == candidate.key.date && time == candidate.board.departure
                    });
                let mut departure = if is_own {
                    own_departure(leg)
                } else {
                    self.describe(ride, candidate, leg, &mut pool)
                };
                departure.missed = departure.departure < ready;
                departure.last_of_day = last_of_day;
                departure
            })
            .collect();
        (departures, pool)
    }

    /// Every departure in `bounds` (window clock) of `pairs` whose trip rides `routes`,
    /// other than the ride's own.
    fn candidates(
        &self,
        pairs: &[Pair],
        filter: RouteFilter,
        bounds: (i32, i32),
        own: &Candidate,
        routes: &[(usize, GtfsRoute)],
    ) -> Vec<Candidate> {
        let snapshot: &Snapshot = &self.snapshot.snapshot;
        let mut out = Vec::new();
        let mut scratch = RiddenRoutes::new();
        // Every run seen running between the stops, so the schedule's copy isn't listed again.
        let mut runs: HashSet<TripKey> = HashSet::from([own.key]);
        for &pair in pairs {
            for day in day_slots(snapshot.days().len()) {
                for lane in snapshot.block_lanes(pair.route, day).unwrap_or_default() {
                    if !serves(snapshot, lane, pair, filter) {
                        continue;
                    }
                    for row in rows_between(snapshot, lane, pair.board, bounds) {
                        let Some(key) = snapshot.trip_key(lane, row) else {
                            continue;
                        };
                        if key == own.key && pair == own.pair {
                            continue;
                        }
                        self.ridden_routes(key, pair.route, pair.board, pair.alight, &mut scratch);
                        if scratch != routes {
                            continue;
                        }
                        let (Some(board), Some(alight)) = (
                            snapshot.lane_event(lane, row, pair.board),
                            snapshot.lane_event(lane, row, pair.alight),
                        ) else {
                            continue;
                        };
                        runs.insert(key);
                        out.push(Candidate {
                            pair,
                            lane,
                            row,
                            key,
                            board,
                            alight,
                            found: Found::Runs,
                        });
                    }
                }
            }
        }

        out.extend(self.removed(pairs, filter, bounds, routes, &runs));
        out
    }

    /// The schedule's departures in `bounds` that the overlay took away from between the two
    /// stops, from the blocks it changed: canceled, or skipped at one of the stops.
    ///
    /// `runs` are the runs found running between the stops, which aren't listed again.
    fn removed(
        &self,
        pairs: &[Pair],
        filter: RouteFilter,
        bounds: (i32, i32),
        routes: &[(usize, GtfsRoute)],
        runs: &HashSet<TripKey>,
    ) -> Vec<Candidate> {
        let snapshot: &Snapshot = &self.snapshot.snapshot;
        let mut out = Vec::new();
        let mut scratch = RiddenRoutes::new();
        let mut listed: HashSet<(TripKey, u32)> = HashSet::new();
        for &pair in pairs {
            for day in day_slots(snapshot.days().len()) {
                if !snapshot.block_changed(pair.route, day) {
                    continue;
                }
                let Some(base_lanes) = snapshot.base_block_lanes(pair.route, day) else {
                    continue;
                };
                // Built on first use: where each of the block's trips is now.
                let mut current: Option<HashMap<TripId, LaneRef>> = None;
                for lane in base_lanes {
                    if !serves(snapshot, lane, pair, filter) {
                        continue;
                    }
                    for row in rows_between(snapshot, lane, pair.board, bounds) {
                        let Some(key) = snapshot.trip_key(lane, row) else {
                            continue;
                        };
                        if runs.contains(&key) || !listed.insert((key, pair.board)) {
                            continue;
                        }
                        self.ridden_routes(key, pair.route, pair.board, pair.alight, &mut scratch);
                        if scratch != routes {
                            continue;
                        }
                        let found = match self.snapshot.rider.trip_state(key) {
                            TripState::Canceled | TripState::NoService => Found::Canceled,
                            _ => {
                                let here = current
                                    .get_or_insert_with(|| {
                                        trips_of_block(snapshot, pair.route, day)
                                    })
                                    .get(&key.trip)
                                    .copied();
                                // The pattern the overlay moved a replaced trip to.
                                let moved_to = self
                                    .snapshot
                                    .rider
                                    .trip_by_key(key)
                                    .and_then(|details| details.route);
                                match (here, moved_to) {
                                    // It still runs between the stops, but outside the bounds.
                                    (Some(lane), _) if serves(snapshot, lane, pair, filter) => {
                                        continue;
                                    }
                                    (None, Some(route))
                                        if pairs.iter().any(|other| other.route == route) =>
                                    {
                                        continue;
                                    }
                                    // It runs, but no longer lets riders on or off at one of the
                                    // stops, or runs a pattern that doesn't serve both.
                                    _ => Found::Skipped,
                                }
                            }
                        };
                        let (Some(board), Some(alight)) = (
                            snapshot.lane_event(lane, row, pair.board),
                            snapshot.lane_event(lane, row, pair.alight),
                        ) else {
                            continue;
                        };
                        out.push(Candidate {
                            pair,
                            lane,
                            row,
                            key,
                            board,
                            alight,
                            found,
                        });
                    }
                }
            }
        }
        out
    }

    /// The latest departure, on the window clock, of each service date of the running departures
    /// in `chosen`, over the whole day: every one of `pairs`, whatever the bounds.
    fn latest_by_date(
        &self,
        chosen: &[Candidate],
        ride: &Ride,
        pairs: &[Pair],
        filter: RouteFilter,
        routes: &[(usize, GtfsRoute)],
    ) -> Vec<(ServiceDate, Time)> {
        let snapshot: &Snapshot = &self.snapshot.snapshot;
        let mut dates: Vec<ServiceDate> = chosen
            .iter()
            .filter(|c| c.found == Found::Runs)
            .map(|c| c.key.date)
            .collect();
        dates.sort_unstable();
        dates.dedup();
        if dates.is_empty() || routes.is_empty() {
            return Vec::new();
        }
        let mut scratch = RiddenRoutes::new();
        let mut out = Vec::with_capacity(dates.len());
        for date in dates {
            // A route between two stops has one latest departure a day, whichever ride asks.
            let cache_key = (ride.board_stop, ride.alight_stop, routes.to_vec(), date);
            if let Some(latest) = self.departure_cache.borrow().latest.get(&cache_key) {
                out.extend(latest.map(|latest| (date, latest)));
                continue;
            }
            let Some(day) = snapshot
                .days()
                .iter()
                .position(|day| day.date == date)
                .and_then(|slot| u8::try_from(slot).ok())
                .map(DaySlot::new)
            else {
                continue;
            };
            let mut latest: Option<Time> = None;
            for &pair in pairs {
                for lane in snapshot.block_lanes(pair.route, day).unwrap_or_default() {
                    if !serves(snapshot, lane, pair, filter) {
                        continue;
                    }
                    // Rows run in departure order, so the last that rides the route is the latest.
                    let count = snapshot.lane_trip_count(lane).unwrap_or(0);
                    for row in (0..count).rev() {
                        let (Some(key), Some(board)) = (
                            snapshot.trip_key(lane, row),
                            snapshot.lane_event(lane, row, pair.board),
                        ) else {
                            continue;
                        };
                        if latest.is_some_and(|latest| board.departure <= latest) {
                            break;
                        }
                        self.ridden_routes(key, pair.route, pair.board, pair.alight, &mut scratch);
                        if scratch != routes {
                            continue;
                        }
                        latest = Some(board.departure);
                        break;
                    }
                }
            }
            self.departure_cache
                .borrow_mut()
                .latest
                .insert(cache_key, latest);
            if let Some(latest) = latest {
                out.push((date, latest));
            }
        }
        out
    }

    /// The GTFS routes a run rides between two positions of a pattern, into `out`.
    ///
    /// A combined trip on its own pattern rides the original trips its positions span;
    /// any other trip rides its own route, and a trip only real-time data knows rides the route
    /// its update names. Empty when nothing names a route.
    fn ridden_routes(
        &self,
        key: TripKey,
        route: RouteId,
        board: u32,
        alight: u32,
        out: &mut RiddenRoutes,
    ) {
        out.clear();
        let set = &self.snapshot.data.set;
        if let Some((part, local)) = set.trip_part(key.trip)
            && let Some(info) = part.compiled().trip(local)
        {
            if part.global_route(info.pattern) == route {
                for segment in part.compiled().trip_segments(local) {
                    if segment.first_position <= alight && board <= segment.last_position {
                        let entry = (part.index(), segment.route);
                        if out.last() != Some(&entry) {
                            out.push(entry);
                        }
                    }
                }
            }
            if out.is_empty() {
                out.push((part.index(), info.route));
            }
            return;
        }
        let details = self.snapshot.rider.trip_by_key(key);
        let part = details
            .and_then(|d| d.feed.as_ref())
            .and_then(|feed| set.part_of_feed(feed.as_str()))
            .or_else(|| set.route_part(route).map(|(part, _)| part));
        if let Some(part) = part
            && let Some(gtfs) = gtfs_route_of(set, part, None, details)
        {
            out.push((part.index(), gtfs));
        }
    }

    /// Describes a departure other than the journey's own.
    /// Describes a departure other than the journey's own, for one leg.
    ///
    /// A departure's description depends only on its run and its two stops, so the journeys of
    /// one plan share it; only what relates it to a leg is worked out per leg.
    fn describe(
        &self,
        ride: &Ride,
        candidate: &Candidate,
        leg: &TransitLeg,
        pool: &mut Vec<AlertResponse>,
    ) -> RideDeparture {
        let key = (candidate.key, candidate.pair);
        let cached = self.departure_cache.borrow().described.get(&key).cloned();
        let described = cached.unwrap_or_else(|| {
            let described = Rc::new(self.described(ride, candidate, leg));
            self.departure_cache
                .borrow_mut()
                .described
                .insert(key, Rc::clone(&described));
            described
        });
        let mut alerts = Vec::new();
        let mut departure_alerts = Vec::new();
        for alert in &described.alerts {
            if let Some(index) = leg.alerts.iter().position(|own| own.alert == *alert) {
                if !alerts.contains(&index) {
                    alerts.push(index);
                }
                continue;
            }
            let index = pool
                .iter()
                .position(|known| known == alert)
                .unwrap_or_else(|| {
                    pool.push(alert.clone());
                    pool.len() - 1
                });
            if !departure_alerts.contains(&index) {
                departure_alerts.push(index);
            }
        }
        let mut departure = described.departure.clone();
        // What the leg already says isn't repeated.
        if departure.start_date == leg.trip.start_date {
            departure.start_date = None;
        }
        let leg_headsign = leg
            .stops
            .first()
            .and_then(|visit| visit.headsign.as_ref())
            .or(leg.trip.headsign.as_ref());
        if departure.headsign.as_ref() == leg_headsign {
            departure.headsign = None;
        }
        departure.alerts = alerts;
        departure.departure_alerts = departure_alerts;
        departure
    }

    /// What a departure is, apart from any leg: every field but the leg-relative ones,
    /// and the alerts that apply to it.
    fn described(&self, ride: &Ride, candidate: &Candidate, leg: &TransitLeg) -> Described {
        let snapshot: &Snapshot = &self.snapshot.snapshot;
        let run = Ride {
            route: candidate.pair.route,
            lane: candidate.lane,
            row: candidate.row,
            trip: candidate.key,
            board_stop: ride.board_stop,
            board_position: candidate.pair.board,
            board_time: candidate.board.departure,
            alight_stop: ride.alight_stop,
            alight_position: candidate.pair.alight,
            alight_time: candidate.alight.arrival,
        };
        let view = self.ride_view(&run);
        let context = self.visit_context(&view, run.trip);
        let (board_index, alight_index) =
            (run.board_position as usize, run.alight_position as usize);
        let board_live = self.live_at(run.trip, board_index);
        let board = self.visit_times(&context, board_index, board_live, candidate.board);
        // What a rider waiting at the boarding stop reads on the vehicle, as a leg shows it:
        // the stop's headsign there, or the trip's.
        let headsign =
            stop_headsign(&context, board_index, board_live).or_else(|| trip_headsign(&view));
        let alight = self.visit_times(
            &context,
            alight_index,
            self.live_at(run.trip, alight_index),
            candidate.alight,
        );
        let runs = candidate.found == Found::Runs;
        // A departure that doesn't run is shown at its schedule; nothing else about it is live.
        let realtime =
            runs && (board.predicted_departure.is_some() || alight.predicted_arrival.is_some());
        let status = match candidate.found {
            Found::Runs if realtime => DepartureStatus::Live,
            Found::Runs => DepartureStatus::Scheduled,
            Found::Canceled => DepartureStatus::Canceled,
            Found::Skipped => DepartureStatus::Skipped,
        };
        let vehicle_id = if runs {
            self.ride_vehicle(&view, &run)
                .and_then(|(chosen, reference)| {
                    chosen
                        .map(|details| &details.vehicle)
                        .or(reference)
                        .and_then(|vehicle| vehicle.id.clone())
                })
        } else {
            None
        };
        let route_stops = snapshot.route_stops(run.route).unwrap_or(&[]);
        let (board_unix, alight_unix) = (board.departure, alight.arrival);
        let alerts = self
            .ride_alerts(&view, &run, route_stops, (board_unix, alight_unix))
            .into_iter()
            .map(|found| found.alert)
            .collect();
        Described {
            departure: RideDeparture {
                trip_id: view.trip_id.clone(),
                start_date: calendar_date(run.trip.date).map(|date| date.to_string()),
                headsign,
                status,
                departure: board_unix,
                arrival: alight_unix,
                scheduled_departure: board.scheduled.map(|(_, departure)| departure),
                scheduled_arrival: alight.scheduled.map(|(arrival, _)| arrival),
                delay_seconds: board.departure_delay.filter(|_| runs),
                realtime,
                vehicle_id,
                // The pair's two stops are the leg's: the same positions of the same pattern.
                accessibility: leg_accessibility(
                    trip_accessibility(&view),
                    leg.stops.first().map(|visit| visit.stop.accessibility),
                    leg.stops.last().map(|visit| visit.stop.accessibility),
                ),
                bikes_allowed: trip_bikes_allowed(&view),
                suggested: false,
                missed: false,
                last_of_day: false,
                alerts: Vec::new(),
                departure_alerts: Vec::new(),
            },
            alerts,
        }
    }
}

/// The journey's own departure, from its leg, so the two always agree.
fn own_departure(leg: &TransitLeg) -> RideDeparture {
    let (first, last) = (leg.stops.first(), leg.stops.last());
    let realtime = first.is_some_and(|visit| visit.predicted_departure.is_some())
        || last.is_some_and(|visit| visit.predicted_arrival.is_some());
    RideDeparture {
        trip_id: leg.trip.trip_id.clone(),
        start_date: None,
        headsign: None,
        status: if realtime {
            DepartureStatus::Live
        } else {
            DepartureStatus::Scheduled
        },
        departure: leg.departure,
        arrival: leg.arrival,
        scheduled_departure: first.and_then(|visit| visit.scheduled_departure),
        scheduled_arrival: last.and_then(|visit| visit.scheduled_arrival),
        delay_seconds: leg.delay_seconds,
        realtime,
        vehicle_id: leg.vehicle.as_ref().and_then(|vehicle| vehicle.id.clone()),
        accessibility: leg.accessibility,
        bikes_allowed: leg.trip.bikes_allowed,
        suggested: true,
        missed: false,
        last_of_day: false,
        alerts: (0..leg.alerts.len()).collect(),
        departure_alerts: Vec::new(),
    }
}

/// Every pattern a query with `filter` may ride from the ride's boarding stop to its alighting
/// stop, by the occurrences routing scans, including the ride's own.
///
/// From each visit to the boarding stop, the first later visit to the alighting stop pairs
/// with it, unless the pattern visits the boarding stop again first: a rider boards there instead.
fn pairs_for(snapshot: &Snapshot, filter: RouteFilter, ride: &Ride) -> Vec<Pair> {
    let mut pairs = Vec::new();
    for (route, board) in snapshot.route_occurrences(ride.board_stop) {
        if !snapshot
            .route_attributes(route)
            .is_some_and(|attributes| filter.allows(attributes))
        {
            continue;
        }
        let Some(stops) = snapshot.route_stops(route) else {
            continue;
        };
        let later = stops.iter().enumerate().skip(board as usize + 1);
        let alight = later
            .take_while(|(_, stop)| **stop != ride.board_stop)
            .find(|(_, stop)| **stop == ride.alight_stop)
            .and_then(|(position, _)| u32::try_from(position).ok());
        if let Some(alight) = alight {
            pairs.push(Pair {
                route,
                board,
                alight,
            });
        }
    }
    let own = Pair {
        route: ride.route,
        board: ride.board_position,
        alight: ride.alight_position,
    };
    if !pairs.contains(&own) {
        pairs.push(own);
    }
    pairs
}

/// Whether a lane lets a query with `filter` board at the pair's boarding and alight at its end.
fn serves(snapshot: &Snapshot, lane: LaneRef, pair: Pair, filter: RouteFilter) -> bool {
    snapshot
        .lane_rules(lane, pair.board, filter)
        .is_some_and(|rules| rules.pickup)
        && snapshot
            .lane_rules(lane, pair.alight, filter)
            .is_some_and(|rules| rules.drop_off)
}

/// The rows of a lane leaving `position` within `bounds`, inclusive, on the window clock.
///
/// A lane's trips never overtake, so its departures at every position are in row order.
fn rows_between(
    snapshot: &Snapshot,
    lane: LaneRef,
    position: u32,
    (low, high): (i32, i32),
) -> impl Iterator<Item = u32> + '_ {
    let count = snapshot.lane_trip_count(lane).unwrap_or(0);
    let departs = move |row: u32| {
        snapshot
            .lane_event(lane, row, position)
            .map_or(i32::MAX, |event| event.departure.into_inner())
    };
    let (mut first, mut end) = (0, count);
    while first < end {
        let middle = first + (end - first) / 2;
        if departs(middle) < low {
            first = middle + 1;
        } else {
            end = middle;
        }
    }
    (first..count).take_while(move |&row| departs(row) <= high)
}

/// Where each trip of a route's block on a day is now, by lane.
fn trips_of_block(snapshot: &Snapshot, route: RouteId, day: DaySlot) -> HashMap<TripId, LaneRef> {
    let mut trips = HashMap::new();
    for lane in snapshot.block_lanes(route, day).unwrap_or_default() {
        for row in 0..snapshot.lane_trip_count(lane).unwrap_or(0) {
            if let Some(key) = snapshot.trip_key(lane, row) {
                trips.insert(key.trip, lane);
            }
        }
    }
    trips
}

fn day_slots(days: usize) -> impl Iterator<Item = DaySlot> {
    (0..days).filter_map(|day| u8::try_from(day).ok().map(DaySlot::new))
}
