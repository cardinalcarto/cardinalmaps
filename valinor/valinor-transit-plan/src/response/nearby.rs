//! Nearby departures and a route's detail, built from one pinned snapshot.
//!
//! Departures are found by the departure-range code ([`super::departures`]) with
//! [`Serving::Board`]: from a stop, to ride on, whatever the rider's destination.
//! Each is described as a ride's departures are, so its status, times, headsign, vehicle,
//! accessibility, and alerts mean the same here as on a plan's ride.
//!
//! # Performance
//!
//! A query reads, for each reachable stop, the patterns serving it, per lane a binary search
//! and the rows in the horizon, and the trip each row runs to find its route and direction.
//! Only the listed departures are described.

use core::convert::Infallible;
use std::collections::HashMap;

use serde::Serialize;
use serde_with::skip_serializing_none;
use valinor_gtfs::{GtfsRoute, TimetablePart};
use valinor_gtfs_rt::ScopedId;
use valinor_transit::{RouteFilter, RouteId, Snapshot, StopId, StopTable, TripKey};

use crate::error::Missing;
use crate::nearby::{DirectionKey, NearbyRequest, RouteDetailRequest};
use crate::plan::{
    AccessEgressRequest, AccessEgressSearch, AccessEgressTables, DepartureRange, DirectSearch,
    TransitOptions, TransitRequest,
};
use crate::response::build::{
    Builder, agency_response, gtfs_route_of, realtime_status, route_response,
};
use crate::response::departures::{Candidate, DepartureSearch, Found, Pair, Serving};
use crate::response::vehicle::vehicle_from;
use crate::response::{
    Accessibility, AgencyResponse, AlertResponse, DepartureStatus, RealtimeStatus, RouteResponse,
    StopRef, TransitLeg, Vehicle, Window, is_false,
};
use crate::state::TransitSnapshot;

/// The routes a rider can catch near a place, soonest first.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NearbyResponse {
    /// When the response was generated, in POSIX seconds.
    pub generated_at: i64,
    /// The departures looked for: from the request's time to the end of its horizon.
    pub window: Window,
    /// What real-time data this answer used, and how old it was.
    pub realtime: RealtimeStatus,
    /// Each GTFS route with a departure the rider can catch, by its soonest one.
    pub routes: Vec<NearbyRoute>,
    /// Why `routes` is empty; absent when it isn't.
    pub no_routes_reason: Option<NoRoutesReason>,
    /// Alerts on the listed departures, which their `alerts` index.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alerts: Vec<AlertResponse>,
}

/// Why a nearby response lists no routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoRoutesReason {
    /// No stop is within the walk.
    NoAccess,
    /// Stops are within the walk, but nothing the rider can catch leaves them in the horizon.
    NoService,
}

/// One GTFS route near the rider.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NearbyRoute {
    /// The route's feed; a `route_id` is only unique within its feed.
    pub feed: String,
    /// The route.
    pub route: RouteResponse,
    /// The route's agency.
    pub agency: Option<AgencyResponse>,
    /// Each direction with a departure the rider can catch, in a stable order:
    /// by `direction_id`, then by `terminus_stop_id`.
    pub directions: Vec<NearbyDirection>,
}

/// Which way a direction goes: exactly one of the two is present.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirectionRef {
    /// GTFS `direction_id`, for trips that have one.
    pub direction_id: Option<u32>,
    /// For trips without a `direction_id`, the `stop_id` of their last stop.
    pub terminus_stop_id: Option<String>,
}

impl From<&DirectionKey> for DirectionRef {
    fn from(key: &DirectionKey) -> Self {
        match key {
            DirectionKey::Id(id) => Self {
                direction_id: Some(*id),
                terminus_stop_id: None,
            },
            DirectionKey::Terminus(stop) => Self {
                direction_id: None,
                terminus_stop_id: Some(stop.clone()),
            },
        }
    }
}

/// One direction of a nearby route, from the stop where the rider catches it soonest.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NearbyDirection {
    /// Which way.
    #[serde(flatten)]
    pub direction: DirectionRef,
    /// The distinct headsigns of `departures`, the most frequent first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub headsigns: Vec<String>,
    /// The nearest reachable stop where the rider can catch this direction's earliest catchable
    /// trip.
    pub stop: StopRef,
    /// The walk to `stop`, in seconds.
    pub walk_seconds: u32,
    /// The next departures from `stop` the rider can be there for, in order of departure.
    pub departures: Vec<StopDeparture>,
}

/// One departure from a stop.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StopDeparture {
    /// `trip_id`, shared by every run of a frequency trip.
    pub trip_id: String,
    /// `start_date`, as `YYYYMMDD`.
    pub start_date: Option<String>,
    /// The headsign at the stop: its `stop_headsign`, or else `trip_headsign`.
    pub headsign: Option<String>,
    /// Whether it runs, and whether real-time data has seen it.
    pub status: DepartureStatus,
    /// When it leaves the stop, as routing would use it (the prediction when there is one),
    /// in POSIX seconds; the scheduled time for a canceled or skipped departure.
    pub departure: i64,
    /// Scheduled departure from the stop, in POSIX seconds.
    pub scheduled_departure: Option<i64>,
    /// The departure's delay from the schedule at the stop, when known.
    pub delay_seconds: Option<i32>,
    /// Whether `departure` came from real-time data; absent when it didn't.
    #[serde(skip_serializing_if = "is_false")]
    pub realtime: bool,
    /// The vehicle's ID, when real-time data names one.
    pub vehicle_id: Option<String>,
    /// Whether the trip and the stop are accessible:
    /// `inaccessible` if either says no, `accessible` if both say yes, and otherwise `unknown`.
    pub accessibility: Accessibility,
    /// `bikes_allowed`: `yes`, `no`, or absent when unknown.
    pub bikes_allowed: Option<&'static str>,
    /// Indices into the response's `alerts` that apply to this departure.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alerts: Vec<usize>,
}

/// One route and direction from one stop, for a detail view.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RouteDetailResponse {
    /// When the response was generated, in POSIX seconds.
    pub generated_at: i64,
    /// The departures looked for: from the request's time to the end of its horizon.
    pub window: Window,
    /// What real-time data this answer used, and how old it was.
    pub realtime: RealtimeStatus,
    /// The route's feed.
    pub feed: String,
    /// The route.
    pub route: RouteResponse,
    /// The route's agency.
    pub agency: Option<AgencyResponse>,
    /// Which way, as the request asked.
    #[serde(flatten)]
    pub direction: DirectionRef,
    /// The distinct headsigns of `departures`, the most frequent first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub headsigns: Vec<String>,
    /// The stop.
    pub stop: StopRef,
    /// The walk to the stop the request gave, in seconds.
    pub walk_seconds: u32,
    /// The next departures from the stop in this direction the rider can be there for.
    pub departures: Vec<StopDeparture>,
    /// The trip asked for, or else the first departure that runs: from the stop to the end
    /// of its trip, with every stop's times, its shape, vehicle, and alerts, as a plan's ride.
    ///
    /// Absent when nothing runs in the horizon.
    pub ride: Option<TransitLeg>,
    /// Vehicles real-time data has on this route's trips in this direction.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vehicles: Vec<Vehicle>,
    /// Alerts on the listed departures, which their `alerts` index.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alerts: Vec<AlertResponse>,
}

/// Why a route-detail request couldn't be answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteLookupError {
    /// The request's feed isn't loaded.
    UnknownFeed(String),
    /// The route or stop it names isn't in the feed, or the trip isn't among the stop's.
    NotFound(Missing),
}

/// A response with no routes, for a request whose walk reaches no stop.
pub(crate) fn empty_nearby(snapshot: &TransitSnapshot, request: &NearbyRequest) -> NearbyResponse {
    NearbyResponse {
        generated_at: request.now,
        window: window(request.time, request.horizon_seconds),
        realtime: realtime_status(snapshot, request.now),
        routes: Vec::new(),
        no_routes_reason: Some(NoRoutesReason::NoAccess),
        alerts: Vec::new(),
    }
}

fn window(time: i64, horizon_seconds: u32) -> Window {
    Window {
        earliest: time,
        latest: time.saturating_add(i64::from(horizon_seconds)),
    }
}

/// The access search a nearby builder holds: the walks were found before it, and none is drawn.
struct NoWalks;

impl AccessEgressSearch for NoWalks {
    type Error = Infallible;
    type Walk = ();

    fn tables(
        &mut self,
        request: &AccessEgressRequest<'_>,
        _interrupt: &dyn Fn() -> bool,
    ) -> Result<AccessEgressTables, Infallible> {
        let stops = request.snapshot.snapshot().stop_count();
        Ok(AccessEgressTables {
            access: StopTable::new(stops),
            egress: StopTable::new(stops),
            direct: DirectSearch::NotSearched,
        })
    }
}

/// The plan request a builder reads, for a query at one place: its mode and time.
///
/// A builder reads a request's mode for the rules it reads and its departure range only when
/// it lists a ride's departures, which nothing here does.
fn builder_request(
    location: valinor_gtfs::LatLon,
    time: i64,
    now: i64,
    mode: crate::plan::TravelMode,
) -> TransitRequest {
    TransitRequest {
        options: TransitOptions {
            departures: DepartureRange::NONE,
            ..TransitOptions::default()
        },
        mode,
        ..TransitRequest::new(location, location, time, now)
    }
}

/// Builds a nearby response from the reachable stops and their walks, in seconds.
///
/// `None` if `interrupt` asked to stop.
pub(crate) fn build_nearby(
    snapshot: &TransitSnapshot,
    request: &NearbyRequest,
    reachable: &[(StopId, u32)],
    interrupt: &dyn Fn() -> bool,
) -> Option<NearbyResponse> {
    let plan_request = builder_request(request.location, request.time, request.now, request.mode);
    let mut walks = NoWalks;
    let builder = Builder::new(snapshot, &plan_request, &mut walks);
    let (routes, alerts) = builder.nearby_routes(request, reachable, interrupt)?;
    Some(NearbyResponse {
        generated_at: request.now,
        window: window(request.time, request.horizon_seconds),
        realtime: realtime_status(snapshot, request.now),
        no_routes_reason: routes.is_empty().then_some(NoRoutesReason::NoService),
        routes,
        alerts,
    })
}

/// Builds a route-detail response.
pub(crate) fn build_route_detail(
    snapshot: &TransitSnapshot,
    request: &RouteDetailRequest,
) -> Result<RouteDetailResponse, RouteLookupError> {
    // There is no place: the stop is the rider's, and the walk to it is given.
    let location = valinor_gtfs::LatLon { lat: 0.0, lon: 0.0 };
    let plan_request = builder_request(location, request.time, request.now, request.mode);
    let mut walks = NoWalks;
    let builder = Builder::new(snapshot, &plan_request, &mut walks);
    builder.route_detail(request)
}

/// Which way a trip goes from a stop, as grouping keys it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Way {
    Id(u32),
    /// The trip's last stop.
    Terminus(StopId),
}

/// A GTFS route of one file, and which way its trip goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Group {
    part: usize,
    route: GtfsRoute,
    way: Way,
}

/// A direction to list: its key, its soonest departure that runs, and the direction itself.
type ListedDirection = (DirectionKey, i64, NearbyDirection);

/// A departure from one reachable stop.
#[derive(Debug, Clone, Copy)]
struct Seen {
    stop: StopId,
    walk: u32,
    candidate: Candidate,
}

/// Every pattern a query with `filter` may board at `stop`, at each position some later stop
/// follows, so a trip ending there isn't a departure.
fn boardings(snapshot: &Snapshot, filter: RouteFilter, stop: StopId) -> Vec<Pair> {
    snapshot
        .route_occurrences(stop)
        .filter(|&(route, board)| {
            snapshot
                .route_attributes(route)
                .is_some_and(|attributes| filter.allows(attributes))
                && snapshot
                    .route_stops(route)
                    .is_some_and(|stops| (board as usize) + 1 < stops.len())
        })
        .map(|(route, board)| Pair {
            route,
            board,
            alight: board,
        })
        .collect()
}

/// A POSIX time on the window clock, saturating at its ends.
fn window_clock(origin: i64, unix: i64) -> i32 {
    i32::try_from(unix.saturating_sub(origin)).unwrap_or(if unix < origin {
        i32::MIN
    } else {
        i32::MAX
    })
}

/// The distinct headsigns of some departures, the most frequent first, then in order of
/// departure.
fn ranked_headsigns(departures: &[StopDeparture]) -> Vec<String> {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for headsign in departures.iter().filter_map(|d| d.headsign.as_deref()) {
        match counted.iter_mut().find(|(seen, _)| *seen == headsign) {
            Some((_, count)) => *count += 1,
            None => counted.push((headsign, 1)),
        }
    }
    // A stable sort keeps first appearances first among equals.
    counted.sort_by_key(|&(_, count)| core::cmp::Reverse(count));
    counted
        .into_iter()
        .map(|(headsign, _)| headsign.to_owned())
        .collect()
}

/// What a trip is where a rider boards it: its file, GTFS route, `direction_id`, and the
/// position of its last stop on the pattern.
struct Boarded {
    part: usize,
    route: GtfsRoute,
    direction: Option<u32>,
    end: usize,
}

impl<A: AccessEgressSearch> Builder<'_, A> {
    /// The GTFS trip a run is on `pattern`, at the original trip `segment` picks on a combined
    /// stay-seated trip.
    ///
    /// The original trip's route, direction, and end, on its own pattern; on a pattern the
    /// overlay moved it to, the trip's route and direction and that pattern's end. A trip only
    /// real-time data knows is what its update says.
    fn boarded(
        &self,
        key: TripKey,
        pattern: RouteId,
        segment: &dyn Fn(&valinor_gtfs::TripSegmentInfo<'_>) -> bool,
    ) -> Option<Boarded> {
        let set = self.set();
        let details = self.snapshot.rider.trip_by_key(key);
        let pattern_end = self
            .snapshot
            .snapshot
            .route_stops(pattern)?
            .len()
            .checked_sub(1)?;
        if let Some((part, local)) = set.trip_part(key.trip)
            && let Some(info) = part.compiled().trip(local)
        {
            let segment = (part.global_route(info.pattern) == pattern)
                .then(|| {
                    part.compiled()
                        .trip_segments(local)
                        .into_iter()
                        .find(|found| segment(found))
                })
                .flatten();
            return Some(Boarded {
                part: part.index(),
                route: segment.map_or(info.route, |s| s.route),
                direction: segment
                    .map_or(info.direction, |s| s.direction)
                    .map(u32::from)
                    .or_else(|| details.and_then(|d| d.direction_id)),
                end: segment.map_or(pattern_end, |s| s.last_position as usize),
            });
        }
        let details = details?;
        let part: TimetablePart<'_> = details
            .feed
            .as_ref()
            .and_then(|feed| set.part_of_feed(feed.as_str()))
            .or_else(|| set.route_part(pattern).map(|(part, _)| part))?;
        Some(Boarded {
            part: part.index(),
            route: gtfs_route_of(set, part, None, Some(details))?,
            direction: details.direction_id,
            end: pattern_end,
        })
    }

    /// The route and direction of a departure at a pair's boarding.
    fn group_of(&self, key: TripKey, pair: Pair) -> Option<Group> {
        let boarded = self.boarded(key, pair.route, &|segment| {
            segment.first_position <= pair.board && pair.board <= segment.last_position
        })?;
        self.group(pair.route, &boarded)
    }

    fn group(&self, pattern: RouteId, boarded: &Boarded) -> Option<Group> {
        let way = match boarded.direction {
            Some(id) => Way::Id(id),
            None => Way::Terminus(
                *self
                    .snapshot
                    .snapshot
                    .route_stops(pattern)?
                    .get(boarded.end)?,
            ),
        };
        Some(Group {
            part: boarded.part,
            route: boarded.route,
            way,
        })
    }

    /// A way as the JSON names it: the terminus by its `stop_id`.
    fn direction_key(&self, way: Way) -> Option<DirectionKey> {
        match way {
            Way::Id(id) => Some(DirectionKey::Id(id)),
            Way::Terminus(stop) => self
                .stop_gtfs_id_of(stop)
                .map(|id| DirectionKey::Terminus(id.to_owned())),
        }
    }

    /// Every departure from `stop` the rider can be there for, in the horizon: those that run,
    /// and those the overlay canceled or made skip it. `accepts` chooses among them.
    fn stop_departures(
        &self,
        stop: StopId,
        (ready, end): (i64, i64),
        accepts: &mut dyn FnMut(TripKey, Pair) -> bool,
    ) -> Vec<Candidate> {
        if ready > end {
            return Vec::new();
        }
        let filter = self.request.mode.route_filter();
        let pairs = boardings(&self.snapshot.snapshot, filter, stop);
        if pairs.is_empty() {
            return Vec::new();
        }
        let origin = self.origin();
        self.candidates(
            &DepartureSearch {
                pairs: &pairs,
                filter,
                serving: Serving::Board,
                bounds: (window_clock(origin, ready), window_clock(origin, end)),
            },
            None,
            accepts,
        )
    }

    /// A departure from `stop` as JSON, with its alerts in `pool`.
    fn stop_departure(
        &self,
        stop: StopId,
        access: Accessibility,
        candidate: &Candidate,
        pool: &mut Vec<AlertResponse>,
    ) -> StopDeparture {
        let described = self.described((stop, stop), candidate, (Some(access), Some(access)));
        let mut alerts = Vec::new();
        for alert in described.alerts {
            let index = pool
                .iter()
                .position(|known| *known == alert)
                .unwrap_or_else(|| {
                    pool.push(alert);
                    pool.len() - 1
                });
            if !alerts.contains(&index) {
                alerts.push(index);
            }
        }
        let departure = described.departure;
        StopDeparture {
            trip_id: departure.trip_id,
            start_date: departure.start_date,
            headsign: departure.headsign,
            status: departure.status,
            departure: departure.departure,
            scheduled_departure: departure.scheduled_departure,
            delay_seconds: departure.delay_seconds,
            realtime: departure.realtime,
            vehicle_id: departure.vehicle_id,
            accessibility: departure.accessibility,
            bikes_allowed: departure.bikes_allowed,
            alerts,
        }
    }

    /// The nearby routes, soonest first, and the alerts their departures index.
    ///
    /// `None` if `interrupt` asked to stop.
    fn nearby_routes(
        &self,
        request: &NearbyRequest,
        reachable: &[(StopId, u32)],
        interrupt: &dyn Fn() -> bool,
    ) -> Option<(Vec<NearbyRoute>, Vec<AlertResponse>)> {
        let end = request
            .time
            .saturating_add(i64::from(request.horizon_seconds));
        let mut groups: HashMap<Group, Vec<Seen>> = HashMap::new();
        for &(stop, walk) in reachable {
            if interrupt() {
                return None;
            }
            // As routing counts a first boarding: at the stop by the time plus the walk.
            let ready = request.time.saturating_add(i64::from(walk));
            let mut keyed: Vec<(Group, Candidate)> = Vec::new();
            for candidate in self.stop_departures(stop, (ready, end), &mut |_, _| true) {
                if let Some(group) = self.group_of(candidate.key, candidate.pair) {
                    keyed.push((group, candidate));
                }
            }
            for (group, candidate) in keyed {
                groups.entry(group).or_default().push(Seen {
                    stop,
                    walk,
                    candidate,
                });
            }
        }

        let origin = self.origin();
        let mut pool = Vec::new();
        // Per route of a file: its directions, each with its soonest departure that runs.
        let mut by_route: HashMap<(usize, GtfsRoute), Vec<ListedDirection>> = HashMap::new();
        for (group, seen) in groups {
            let Some(chosen) = chosen_boarding(&seen) else {
                continue;
            };
            let Some(key) = self.direction_key(group.way) else {
                continue;
            };
            let (stop, walk) = (chosen.stop, chosen.walk);
            // The route's order reads the trip as the rider catches it, at the chosen stop.
            let soonest = origin + i64::from(chosen.candidate.board.departure.into_inner());
            let mut listed: Vec<&Candidate> = seen
                .iter()
                .filter(|s| s.stop == stop)
                .map(|s| &s.candidate)
                .collect();
            listed.sort_unstable_by_key(|c| c.order());
            listed.truncate(usize::from(request.max_departures));
            let stop_ref = self.stop_ref(stop);
            let departures: Vec<StopDeparture> = listed
                .into_iter()
                .map(|c| self.stop_departure(stop, stop_ref.accessibility, c, &mut pool))
                .collect();
            let direction = NearbyDirection {
                direction: DirectionRef::from(&key),
                headsigns: ranked_headsigns(&departures),
                stop: stop_ref,
                walk_seconds: walk,
                departures,
            };
            by_route
                .entry((group.part, group.route))
                .or_default()
                .push((key, soonest, direction));
        }

        let mut routes: Vec<(i64, NearbyRoute)> = Vec::new();
        for ((part, route), mut directions) in by_route {
            let Some(part) = self.set().parts().nth(part) else {
                continue;
            };
            let Some(info) = part.compiled().route(route) else {
                continue;
            };
            directions.sort_by(|a, b| a.0.cmp(&b.0));
            let soonest = directions
                .iter()
                .map(|&(_, soonest, _)| soonest)
                .min()
                .unwrap_or(i64::MAX);
            routes.push((
                soonest,
                NearbyRoute {
                    feed: info.feed.to_owned(),
                    route: route_response(info),
                    agency: info
                        .agency
                        .and_then(|agency| part.compiled().agency(agency))
                        .map(agency_response),
                    directions: directions.into_iter().map(|(_, _, d)| d).collect(),
                },
            ));
        }
        routes.sort_by(|(a_soonest, a), (b_soonest, b)| {
            (a_soonest, &a.feed, &a.route.route_id).cmp(&(b_soonest, &b.feed, &b.route.route_id))
        });
        let mut routes: Vec<NearbyRoute> = routes.into_iter().map(|(_, route)| route).collect();
        if let Some(max) = request.max_routes {
            routes.truncate(usize::try_from(max).unwrap_or(usize::MAX));
        }
        // Only alerts on departures still listed; renumbered in order of first use.
        let pool = compact_alerts(&mut routes, pool);
        Some((routes, pool))
    }

    /// One route and direction from one stop.
    fn route_detail(
        &self,
        request: &RouteDetailRequest,
    ) -> Result<RouteDetailResponse, RouteLookupError> {
        let set = self.set();
        let part = set
            .part_of_feed(&request.feed)
            .ok_or_else(|| RouteLookupError::UnknownFeed(request.feed.clone()))?;
        let compiled = part.compiled();
        let route = compiled
            .route_by_gtfs_id(&request.feed, &request.route_id)
            .ok_or(RouteLookupError::NotFound(Missing::Route))?;
        let info = compiled
            .route(route)
            .ok_or(RouteLookupError::NotFound(Missing::Route))?;
        let stop = compiled
            .stop_by_gtfs_id(&request.feed, &request.stop_id)
            .map(|stop| part.global_stop(stop))
            .or_else(|| {
                self.snapshot
                    .new_stops
                    .iter()
                    .find(|new| {
                        new.id.feed.as_str() == request.feed && *new.id.id == *request.stop_id
                    })
                    .map(|new| new.stop)
            })
            .ok_or(RouteLookupError::NotFound(Missing::Stop))?;

        let wanted = |group: Option<Group>| {
            group.is_some_and(|group| {
                group.part == part.index()
                    && group.route == route
                    && self.direction_key(group.way).as_ref() == Some(&request.direction)
            })
        };
        let ready = request.time.saturating_add(i64::from(request.walk_seconds));
        let end = request
            .time
            .saturating_add(i64::from(request.horizon_seconds));
        let mut found = self.stop_departures(stop, (ready, end), &mut |key, pair| {
            wanted(self.group_of(key, pair))
        });
        found.sort_unstable_by_key(Candidate::order);

        let chosen = match &request.trip {
            Some(choice) => Some(
                found
                    .iter()
                    .find(|candidate| self.is_trip(stop, candidate, choice))
                    .ok_or(RouteLookupError::NotFound(Missing::Trip))?,
            ),
            None => found.iter().find(|c| c.found == Found::Runs),
        };
        let ride = chosen.and_then(|candidate| {
            let ride = self.ride_to_end(stop, candidate)?;
            let board = self.origin() + i64::from(candidate.board.departure.into_inner());
            Some(self.transit_leg(&ride, board - ready))
        });

        let stop_ref = self.stop_ref(stop);
        let mut pool = Vec::new();
        let departures: Vec<StopDeparture> = found
            .iter()
            .take(usize::from(request.max_departures))
            .map(|c| self.stop_departure(stop, stop_ref.accessibility, c, &mut pool))
            .collect();
        let vehicles = self.route_vehicles(info.gtfs_id, &request.feed, &|group| wanted(group));
        Ok(RouteDetailResponse {
            generated_at: request.now,
            window: window(request.time, request.horizon_seconds),
            realtime: realtime_status(self.snapshot, request.now),
            feed: info.feed.to_owned(),
            route: route_response(info),
            agency: info
                .agency
                .and_then(|agency| compiled.agency(agency))
                .map(agency_response),
            direction: DirectionRef::from(&request.direction),
            headsigns: ranked_headsigns(&departures),
            stop: stop_ref,
            walk_seconds: request.walk_seconds,
            departures,
            ride,
            vehicles,
            alerts: pool,
        })
    }

    /// Whether a departure from `stop` is the run a request chose.
    fn is_trip(
        &self,
        stop: StopId,
        candidate: &Candidate,
        choice: &crate::nearby::TripChoice,
    ) -> bool {
        let date = crate::window::calendar_date(candidate.key.date).map(|date| date.to_string());
        if choice
            .start_date
            .as_ref()
            .is_some_and(|wanted| date.as_ref() != Some(wanted))
        {
            return false;
        }
        let run = boarding_ride(stop, candidate);
        self.ride_view(&run).trip_id == choice.trip_id
    }

    /// A departure ridden from `stop` to the last stop of its GTFS trip.
    fn ride_to_end(&self, stop: StopId, candidate: &Candidate) -> Option<valinor_transit::Ride> {
        let pair = candidate.pair;
        let boarded = self.boarded(candidate.key, pair.route, &|segment| {
            segment.first_position <= pair.board && pair.board <= segment.last_position
        })?;
        let end = u32::try_from(boarded.end).ok()?;
        let alight_stop = *self
            .snapshot
            .snapshot
            .route_stops(pair.route)?
            .get(boarded.end)?;
        let alight = self
            .snapshot
            .snapshot
            .lane_event(candidate.lane, candidate.row, end)?;
        Some(valinor_transit::Ride {
            alight_stop,
            alight_position: end,
            alight_time: alight.arrival,
            ..boarding_ride(stop, candidate)
        })
    }

    /// Vehicles on a route's trips that `wanted` keeps, by the trip each is running,
    /// in order of ID.
    ///
    /// A vehicle real-time data doesn't match to a trip of the timetable has no direction,
    /// so it isn't listed.
    fn route_vehicles(
        &self,
        route_id: &str,
        feed: &str,
        wanted: &dyn Fn(Option<Group>) -> bool,
    ) -> Vec<Vehicle> {
        let Some(feed) = self.snapshot.data.feeds.iter().find(|f| f.as_str() == feed) else {
            return Vec::new();
        };
        let rider = &self.snapshot.rider;
        let mut vehicles: Vec<_> = rider
            .vehicles_on_route(&ScopedId::new(feed, route_id))
            .into_iter()
            .filter(|vehicle| {
                let Some(key) = vehicle.trip_key else {
                    return false;
                };
                let pattern = self
                    .set()
                    .trip_part(key.trip)
                    .and_then(|(part, local)| {
                        part.compiled()
                            .trip(local)
                            .map(|info| part.global_route(info.pattern))
                    })
                    .or_else(|| rider.trip_by_key(key).and_then(|d| d.route));
                let trip_id = vehicle.trip_id.as_deref();
                pattern
                    .and_then(|pattern| {
                        let boarded = self
                            .boarded(key, pattern, &|segment| Some(segment.gtfs_id) == trip_id)?;
                        self.group(pattern, &boarded)
                    })
                    .is_some_and(|group| wanted(Some(group)))
            })
            .collect();
        vehicles.sort_by(|a, b| {
            (&a.vehicle.id, &a.vehicle.label).cmp(&(&b.vehicle.id, &b.vehicle.label))
        });
        vehicles
            .into_iter()
            .map(|vehicle| vehicle_from(self.snapshot, Some(vehicle), None))
            .collect()
    }
}

/// A departure as a ride boarding at `stop` and alighting there, for describing it.
fn boarding_ride(stop: StopId, candidate: &Candidate) -> valinor_transit::Ride {
    valinor_transit::Ride {
        route: candidate.pair.route,
        lane: candidate.lane,
        row: candidate.row,
        trip: candidate.key,
        board_stop: stop,
        board_position: candidate.pair.board,
        board_time: candidate.board.departure,
        alight_stop: stop,
        alight_position: candidate.pair.alight,
        alight_time: candidate.alight.arrival,
    }
}

/// Where a direction is caught: the earliest trip the rider can catch at any reachable stop,
/// at the nearest stop where that trip can still be caught; then the earlier departure, then
/// the lower stop number. `None` if nothing that runs can be caught.
///
/// A trip is a run on its service date, so each run of a frequency trip is a trip of its own,
/// and a loop's two visits to one stop are one stop, at its earlier departure.
fn chosen_boarding(seen: &[Seen]) -> Option<&Seen> {
    // Canceled and skipped departures are listed, but can't be caught.
    let running = || seen.iter().filter(|s| s.candidate.found == Found::Runs);
    let earliest = running()
        .min_by_key(|s| (s.candidate.board.departure, s.walk, s.stop))?
        .candidate
        .key;
    running()
        .filter(|s| s.candidate.key == earliest)
        .min_by_key(|s| (s.walk, s.candidate.board.departure, s.stop))
}

/// Keeps the alerts of `pool` that a listed departure still indexes, renumbering them.
fn compact_alerts(routes: &mut [NearbyRoute], pool: Vec<AlertResponse>) -> Vec<AlertResponse> {
    let mut renumbered: Vec<Option<usize>> = vec![None; pool.len()];
    let mut kept: Vec<usize> = Vec::new();
    for departure in routes
        .iter_mut()
        .flat_map(|route| &mut route.directions)
        .flat_map(|direction| &mut direction.departures)
    {
        for index in &mut departure.alerts {
            let old = *index;
            let Some(slot) = renumbered.get_mut(old) else {
                continue;
            };
            *index = *slot.get_or_insert_with(|| {
                kept.push(old);
                kept.len() - 1
            });
        }
    }
    let mut pool: Vec<Option<AlertResponse>> = pool.into_iter().map(Some).collect();
    kept.into_iter()
        .filter_map(|old| pool.get_mut(old).and_then(Option::take))
        .collect()
}
