//! A ride's other departures: every route's between the same two stops, around the journey's own.
//!
//! A departure is any trip that lets the rider on at the ride's boarding stop and off at its
//! alighting stop, those exact two stops and no others nearby, whatever its route:
//! what a rider could take instead, or after missing the journey's own. A trip that stays seated
//! as its vehicle continues as another route is one, described by the route it boards.
//! Each is listed once, and `last_of_day` is about all of them: the connection's last.
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
//! a leg does, without stop names or a vehicle's position. A plan finds the patterns between two
//! stops, the latest departure between them each day, and each departure's description once,
//! however many of its journeys ride between them.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use valinor_gtfs::GtfsRoute;
use valinor_gtfs_rt::TripState;
use valinor_transit::{
    DaySlot, LaneRef, Ride, RouteFilter, RouteId, ServiceDate, Snapshot, StopEvent, StopId,
    StopRules, Time, TripId, TripKey,
};

use crate::response::build::{
    RideDescriber, boarding_headsign, leg_accessibility, route_response, trip_accessibility,
    trip_bikes_allowed,
};
use crate::response::{Accessibility, AlertResponse, DepartureStatus, RideDeparture, TransitLeg};
use crate::window::calendar_date;

/// A pattern serving a ride's boarding stop and then its alighting stop, at these positions.
///
/// A departure from a stop to wherever the rider likes ([`Serving::Board`]) has its alighting
/// at its boarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Pair {
    pub(super) route: RouteId,
    pub(super) board: u32,
    pub(super) alight: u32,
}

/// What a listed departure must let a rider do at its pair's positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Serving {
    /// Board at the boarding position and alight at the alighting one: a ride's departures.
    BoardAndAlight,
    /// Board at the boarding position, to ride on: a stop's departures.
    ///
    /// The caller pairs only positions some later stop follows, so a trip ending there isn't one.
    Board,
}

/// Where departures are looked for: patterns at positions, the rules read, and a time bound.
#[derive(Debug, Clone, Copy)]
pub(super) struct DepartureSearch<'p> {
    pub(super) pairs: &'p [Pair],
    pub(super) filter: RouteFilter,
    pub(super) serving: Serving,
    /// The departures from the boarding position to list, inclusive, on the window clock.
    pub(super) bounds: (i32, i32),
}

/// What became of a departure the schedule has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Found {
    /// It runs between the two stops, in the snapshot.
    Runs,
    Canceled,
    Skipped,
}

/// One departure between the ride's stops, read from a lane.
#[derive(Debug, Clone, Copy)]
pub(super) struct Candidate {
    pub(super) pair: Pair,
    pub(super) lane: LaneRef,
    pub(super) row: u32,
    pub(super) key: TripKey,
    pub(super) board: StopEvent,
    /// The event at the alighting position; for [`Serving::Board`], the boarding's departure.
    pub(super) alight: StopEvent,
    pub(super) found: Found,
}

impl Candidate {
    /// The order departures are listed in.
    pub(super) fn order(&self) -> (Time, Time, ServiceDate, TripId, u32) {
        (
            self.board.departure,
            self.alight.arrival,
            self.key.date,
            self.key.trip,
            self.pair.board,
        )
    }

    /// Whether this is the last departure of its service date that runs, given the `latest`
    /// departure of each date as [`RideDescriber::latest_by_date`] found them.
    pub(super) fn is_last_of_day(&self, latest: &[(ServiceDate, Time)]) -> bool {
        self.found == Found::Runs
            && latest
                .iter()
                .any(|&(date, time)| date == self.key.date && time == self.board.departure)
    }
}

/// What one plan has already worked out about departures, since its journeys share most of them.
#[derive(Debug, Default)]
pub(super) struct DepartureCache {
    /// Each departure described, by its run and its pair of positions.
    described: HashMap<(TripKey, Pair), Rc<Described>>,
    /// The patterns serving one stop and then another, as [`pairs_for`] finds them.
    pairs: HashMap<(StopId, StopId), Rc<[Pair]>>,
    /// The latest departure of any route between two stops on a service date, on the window clock.
    latest: HashMap<(StopId, StopId, ServiceDate), Option<Time>>,
}

/// A departure's leg-independent description.
#[derive(Debug)]
pub(super) struct Described {
    /// Its `route` is the one it boards, which a leg on the same route leaves out.
    pub(super) departure: RideDeparture,
    /// The GTFS route it boards, by file, which tells it from the leg's.
    pub(super) gtfs_route: Option<(usize, GtfsRoute)>,
    pub(super) alerts: Vec<AlertResponse>,
}

impl RideDescriber<'_> {
    /// The departures between a ride's two stops, of any route, and the alerts on them that
    /// aren't the leg's own.
    ///
    /// `ready` is the earliest the rider can be at the boarding stop, in POSIX seconds.
    pub(super) fn ride_departures(
        &self,
        ride: &Ride,
        ready: i64,
        leg: &TransitLeg,
    ) -> (Vec<RideDeparture>, Vec<AlertResponse>) {
        let snapshot: &Snapshot = &self.snapshot.snapshot;
        let filter = self.mode.route_filter();
        let range = self.departures;

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
        let (pairs, own_paired) = self.connection_pairs(filter, ride);
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
        // Every trip that lets the rider on at the one stop and off at the other is a departure,
        // whatever its route.
        let mut candidates = self.candidates(
            &DepartureSearch {
                pairs: &pairs,
                filter,
                serving: Serving::BoardAndAlight,
                bounds,
            },
            Some((own.key, own.pair)),
            &mut |_, _| true,
        );
        candidates.sort_unstable_by_key(Candidate::order);
        let at = candidates.partition_point(|c| c.order() < own.order());
        candidates.insert(at, own);
        let first = at.saturating_sub(usize::from(range.before));
        let last = (at + usize::from(range.after)).min(candidates.len() - 1);
        let chosen = &candidates[first..=last];

        let latest = self.latest_by_date(
            chosen,
            (ride.board_stop, ride.alight_stop),
            &pairs,
            filter,
            own_paired,
        );
        // The route the leg boards, which a departure on it doesn't repeat.
        let own_route = self.ride_view(ride).gtfs_route;
        let mut pool: Vec<AlertResponse> = Vec::new();
        let departures = chosen
            .iter()
            .map(|candidate| {
                let is_own = candidate.key == own.key && candidate.pair == own.pair;
                let last_of_day = candidate.is_last_of_day(&latest);
                let mut departure = if is_own {
                    own_departure(leg)
                } else {
                    self.describe(ride, candidate, leg, own_route, &mut pool)
                };
                departure.missed = departure.departure < ready;
                departure.last_of_day = last_of_day;
                departure
            })
            .collect();
        (departures, pool)
    }

    /// Every departure of the search whose trip `accepts`, but `skip`'s run at its boarding:
    /// those that run, and those the overlay took away, each once.
    ///
    /// A run boarded at one position is one departure, however many of the pairs it alights at:
    /// the first pair listed for it wins.
    pub(super) fn candidates(
        &self,
        search: &DepartureSearch<'_>,
        skip: Option<(TripKey, Pair)>,
        accepts: &mut dyn FnMut(TripKey, Pair) -> bool,
    ) -> Vec<Candidate> {
        let snapshot: &Snapshot = &self.snapshot.snapshot;
        let mut out = Vec::new();
        // Every run seen running between the stops, so the schedule's copy isn't listed again.
        let mut runs: HashSet<TripKey> = skip.iter().map(|&(key, _)| key).collect();
        // Every run listed, by where it is boarded.
        let mut listed: HashSet<(TripKey, u32)> =
            skip.iter().map(|&(key, pair)| (key, pair.board)).collect();
        for &pair in search.pairs {
            for day in day_slots(snapshot.days().len()) {
                for lane in snapshot.block_lanes(pair.route, day).unwrap_or_default() {
                    if !serves(snapshot, lane, pair, search.filter, search.serving) {
                        continue;
                    }
                    for row in rows_between(snapshot, lane, pair.board, search.bounds) {
                        let Some(key) = snapshot.trip_key(lane, row) else {
                            continue;
                        };
                        if listed.contains(&(key, pair.board)) || !accepts(key, pair) {
                            continue;
                        }
                        let (Some(board), Some(alight)) = (
                            snapshot.lane_event(lane, row, pair.board),
                            alight_event(snapshot, lane, row, pair, search.serving),
                        ) else {
                            continue;
                        };
                        listed.insert((key, pair.board));
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

        out.extend(self.removed(search, accepts, &runs));
        out
    }

    /// The schedule's departures of the search that the overlay took away from between the two
    /// stops, from the blocks it changed: canceled, or skipped at one of the stops.
    ///
    /// `runs` are the runs found running between the stops, which aren't listed again.
    fn removed(
        &self,
        search: &DepartureSearch<'_>,
        accepts: &mut dyn FnMut(TripKey, Pair) -> bool,
        runs: &HashSet<TripKey>,
    ) -> Vec<Candidate> {
        let snapshot: &Snapshot = &self.snapshot.snapshot;
        let (filter, serving) = (search.filter, search.serving);
        let mut out = Vec::new();
        let mut listed: HashSet<(TripKey, u32)> = HashSet::new();
        for &pair in search.pairs {
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
                    if !serves(snapshot, lane, pair, filter, serving) {
                        continue;
                    }
                    for row in rows_between(snapshot, lane, pair.board, search.bounds) {
                        let Some(key) = snapshot.trip_key(lane, row) else {
                            continue;
                        };
                        if runs.contains(&key) || !listed.insert((key, pair.board)) {
                            continue;
                        }
                        if !accepts(key, pair) {
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
                                    (Some(lane), _)
                                        if serves(snapshot, lane, pair, filter, serving) =>
                                    {
                                        continue;
                                    }
                                    (None, Some(route))
                                        if search
                                            .pairs
                                            .iter()
                                            .any(|other| other.route == route) =>
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
                            alight_event(snapshot, lane, row, pair, serving),
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
    /// in `chosen`, over the whole day: every one of `pairs`, whatever the bounds and the route.
    ///
    /// `stops` are the boarding and alighting stops the pairs lead between.
    /// `shared` says `pairs` are the two stops' alone, which any ride between them would read,
    /// so what is found is kept for them; a ride's own pair beyond those is its own to read.
    pub(super) fn latest_by_date(
        &self,
        chosen: &[Candidate],
        (board_stop, alight_stop): (StopId, StopId),
        pairs: &[Pair],
        filter: RouteFilter,
        shared: bool,
    ) -> Vec<(ServiceDate, Time)> {
        let snapshot: &Snapshot = &self.snapshot.snapshot;
        let mut dates: Vec<ServiceDate> = chosen
            .iter()
            .filter(|c| c.found == Found::Runs)
            .map(|c| c.key.date)
            .collect();
        dates.sort_unstable();
        dates.dedup();
        let mut out = Vec::with_capacity(dates.len());
        for date in dates {
            // Two stops have one latest departure between them a day, whichever ride asks:
            // the plan's travel mode, and so its rules, is the same for all of them.
            let cache_key = (board_stop, alight_stop, date);
            if shared && let Some(latest) = self.departure_cache.borrow().latest.get(&cache_key) {
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
                    if !serves(snapshot, lane, pair, filter, Serving::BoardAndAlight) {
                        continue;
                    }
                    // Rows run in departure order, so the last one is the lane's latest.
                    let count = snapshot.lane_trip_count(lane).unwrap_or(0);
                    let lane_latest = (0..count).rev().find_map(|row| {
                        snapshot.trip_key(lane, row)?;
                        snapshot
                            .lane_event(lane, row, pair.board)
                            .map(|board| board.departure)
                    });
                    latest = latest.max(lane_latest);
                }
            }
            if shared {
                self.departure_cache
                    .borrow_mut()
                    .latest
                    .insert(cache_key, latest);
            }
            if let Some(latest) = latest {
                out.push((date, latest));
            }
        }
        out
    }

    /// Every pattern a query with `filter` may ride from the ride's boarding stop to its
    /// alighting stop, as [`pairs_for`] finds them, worked out once per plan for each two stops,
    /// and whether they include the ride's own pair.
    fn connection_pairs(&self, filter: RouteFilter, ride: &Ride) -> (Rc<[Pair]>, bool) {
        let key = (ride.board_stop, ride.alight_stop);
        let own = Pair {
            route: ride.route,
            board: ride.board_position,
            alight: ride.alight_position,
        };
        let cached = self.departure_cache.borrow().pairs.get(&key).cloned();
        let pairs = cached.unwrap_or_else(|| {
            let pairs: Rc<[Pair]> = pairs_for(&self.snapshot.snapshot, filter, key).into();
            self.departure_cache
                .borrow_mut()
                .pairs
                .insert(key, Rc::clone(&pairs));
            pairs
        });
        // The ride's own may be on a pattern the stops' occurrences don't pair, such as one
        // that visits the boarding stop again before the alighting stop.
        if pairs.contains(&own) {
            (pairs, true)
        } else {
            (pairs.iter().copied().chain([own]).collect(), false)
        }
    }

    /// Describes a departure other than the journey's own, for one leg.
    ///
    /// A departure's description depends only on its run and its two stops, so the journeys of
    /// one plan share it; only what relates it to a leg is worked out per leg.
    ///
    /// `own_route` is the GTFS route the leg boards, by file.
    fn describe(
        &self,
        ride: &Ride,
        candidate: &Candidate,
        leg: &TransitLeg,
        own_route: Option<(usize, GtfsRoute)>,
        pool: &mut Vec<AlertResponse>,
    ) -> RideDeparture {
        let key = (candidate.key, candidate.pair);
        let cached = self.departure_cache.borrow().described.get(&key).cloned();
        let described = cached.unwrap_or_else(|| {
            // The pair's two stops are the leg's: the same positions of the same pattern.
            let described = Rc::new(self.described(
                (ride.board_stop, ride.alight_stop),
                candidate,
                (
                    leg.stops.first().map(|visit| visit.stop.accessibility),
                    leg.stops.last().map(|visit| visit.stop.accessibility),
                ),
            ));
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
        if described.gtfs_route == own_route {
            departure.route = None;
        }
        departure.alerts = alerts;
        departure.departure_alerts = departure_alerts;
        departure
    }

    /// What a departure is, apart from any leg: every field but the leg-relative ones,
    /// and the alerts that apply to it.
    ///
    /// `stops` are the boarding and alighting stops, and `access` what is known of a wheelchair
    /// at each; a stop's departures ([`Serving::Board`]) give its boarding stop for both.
    pub(super) fn described(
        &self,
        (board_stop, alight_stop): (StopId, StopId),
        candidate: &Candidate,
        (board_access, alight_access): (Option<Accessibility>, Option<Accessibility>),
    ) -> Described {
        let snapshot: &Snapshot = &self.snapshot.snapshot;
        let run = Ride {
            route: candidate.pair.route,
            lane: candidate.lane,
            row: candidate.row,
            trip: candidate.key,
            board_stop,
            board_position: candidate.pair.board,
            board_time: candidate.board.departure,
            alight_stop,
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
        let headsign = boarding_headsign(&view, &context, board_index, board_live);
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
                route: view.route.map(route_response),
                status,
                departure: board_unix,
                arrival: alight_unix,
                scheduled_departure: board.scheduled.map(|(_, departure)| departure),
                scheduled_arrival: alight.scheduled.map(|(arrival, _)| arrival),
                delay_seconds: board.departure_delay.filter(|_| runs),
                realtime,
                vehicle_id,
                accessibility: leg_accessibility(
                    trip_accessibility(&view),
                    board_access,
                    alight_access,
                ),
                bikes_allowed: trip_bikes_allowed(&view),
                suggested: false,
                missed: false,
                last_of_day: false,
                alerts: Vec::new(),
                departure_alerts: Vec::new(),
            },
            alerts,
            gtfs_route: view.gtfs_route,
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
        route: None,
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

/// Every pattern a query with `filter` may ride from one stop to another, by the occurrences
/// routing scans, whatever its route.
///
/// From each visit to the boarding stop, the first later visit to the alighting stop pairs
/// with it, unless the pattern visits the boarding stop again first: a rider boards there instead.
pub(super) fn pairs_for(
    snapshot: &Snapshot,
    filter: RouteFilter,
    (board_stop, alight_stop): (StopId, StopId),
) -> Vec<Pair> {
    let mut pairs = Vec::new();
    for (route, board) in snapshot.route_occurrences(board_stop) {
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
            .take_while(|(_, stop)| **stop != board_stop)
            .find(|(_, stop)| **stop == alight_stop)
            .and_then(|(position, _)| u32::try_from(position).ok());
        if let Some(alight) = alight {
            pairs.push(Pair {
                route,
                board,
                alight,
            });
        }
    }
    pairs
}

/// Whether a lane lets a query with `filter` board at the pair's boarding, and for a ride's
/// departures, alight at its end.
fn serves(
    snapshot: &Snapshot,
    lane: LaneRef,
    pair: Pair,
    filter: RouteFilter,
    serving: Serving,
) -> bool {
    snapshot
        .lane_rules(lane, pair.board, filter)
        .is_some_and(|rules| rules.pickup)
        && match serving {
            Serving::BoardAndAlight => snapshot
                .lane_rules(lane, pair.alight, filter)
                .is_some_and(|rules| rules.drop_off),
            Serving::Board => true,
        }
}

/// A candidate's event at its alighting: for a stop's departures, which have none, the
/// boarding's departure, so a departure's alerts are matched at that moment alone.
fn alight_event(
    snapshot: &Snapshot,
    lane: LaneRef,
    row: u32,
    pair: Pair,
    serving: Serving,
) -> Option<StopEvent> {
    match serving {
        Serving::BoardAndAlight => snapshot.lane_event(lane, row, pair.alight),
        Serving::Board => snapshot
            .lane_event(lane, row, pair.board)
            .map(|event| StopEvent {
                arrival: event.departure,
                ..event
            }),
    }
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
