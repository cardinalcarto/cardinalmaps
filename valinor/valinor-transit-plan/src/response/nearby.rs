//! Nearby departures and a route's detail, built from one pinned snapshot.
//!
//! Departures are found by the departure-range code ([`super::departures`]) with
//! [`Serving::Board`]: from a stop, to ride on, whatever the rider's destination.
//! Each is described as a ride's departures are, so its status, times, headsign, vehicle,
//! accessibility, and alerts mean the same here as on a plan's ride.
//!
//! A route's departures are shown in pages, one per headsign a rider reads at the stop:
//! headsigns whose trips go the same way from a page's stop share it, and pages that share a
//! trip are one; see [`route_pages`].
//!
//! # Performance
//!
//! A query reads, for each reachable stop, the patterns serving it, per lane a binary search
//! and the rows in the horizon, and for each row the trip it runs, to find its route and its
//! headsign at the stop. Only the listed departures are described.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_with::skip_serializing_none;
use valinor_gtfs::{GtfsRoute, TimetablePart};
use valinor_gtfs_rt::ScopedId;
use valinor_transit::{RouteFilter, RouteId, Snapshot, StopId, TripKey};

use crate::error::Missing;
use crate::nearby::{NearbyRequest, RouteDetailRequest};
use crate::plan::DepartureRange;
use crate::response::build::{
    RideDescriber, agency_response, boarding_headsign, encode_polyline6, gtfs_route_of,
    realtime_status, route_response,
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
    /// Each page with a departure the rider can catch, in an order that doesn't depend on
    /// time: by `direction_id` (pages without one last), then by the first of the page's
    /// headsigns in string order (a page without headsigns last).
    pub pages: Vec<NearbyPage>,
}

/// One page of a nearby route: its departures under one headsign, or under several whose trips
/// go the same way from the page's stop, from the stop where the rider catches them soonest.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NearbyPage {
    /// The headsigns this page lists, the most frequent among its departures that run first:
    /// what a route detail asks for it by. Empty for the departures that have no headsign.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub headsigns: Vec<String>,
    /// The `direction_id` of the page's trips that run from `stop`, when they all have the
    /// same one. It orders the pages and says nothing else: pages aren't grouped by it.
    pub direction_id: Option<u32>,
    /// The nearest reachable stop where the rider can catch the page's earliest catchable trip.
    pub stop: StopRef,
    /// The walk to `stop`, in seconds.
    pub walk_seconds: u32,
    /// When that trip leaves `stop`, in POSIX seconds; a route is listed by its pages' soonest.
    pub soonest_departure: i64,
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
    /// The route its vehicle continues as after this trip ends, with riders staying on.
    pub continues_as: Option<ContinuesAs>,
}

/// The trip a vehicle runs next as another route, which a rider can stay on for.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContinuesAs {
    /// The next trip's `route_id`, in the departure's feed.
    pub route_id: String,
    /// That route's `route_short_name`.
    pub short_name: Option<String>,
    /// The next trip's headsign.
    pub headsign: Option<String>,
}

/// One page of a route from one stop, for a detail view.
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
    /// The page, by the headsigns the request gave.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub headsigns: Vec<String>,
    /// The stop.
    pub stop: StopRef,
    /// The walk to the stop the request gave, in seconds.
    pub walk_seconds: u32,
    /// The next departures from the stop under the page's headsigns the rider can be there for.
    pub departures: Vec<StopDeparture>,
    /// The trip asked for, or else the first departure that runs: from the stop to where its
    /// vehicle stops carrying riders, with every stop's times, its shape, vehicle, and alerts,
    /// as a plan's ride. That is the end of its GTFS trip, or, where the vehicle goes on as
    /// further trips of a stay-seated chain, the end of the last of them that still carries
    /// riders, the ride then in `segments` as a plan's stay-seated ride is.
    ///
    /// Absent when nothing runs in the horizon.
    pub ride: Option<TransitLeg>,
    /// The ride's trip before the stop, where the rider can't board it yet: its shape from the
    /// first stop of its GTFS trip to the stop, as polyline6, ending where the ride's begins.
    /// A trip without a shape there is drawn straight from stop to stop.
    ///
    /// Absent without a ride, and when the stop is its trip's first.
    pub shape_to_stop: Option<String>,
    /// Vehicles real-time data has on this route's trips that leave the stop under one of the
    /// page's headsigns, whether or not they have left it yet.
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

/// Builds a nearby response from the reachable stops and their walks, in seconds.
///
/// `None` if `interrupt` asked to stop.
pub(crate) fn build_nearby(
    snapshot: &TransitSnapshot,
    request: &NearbyRequest,
    reachable: &[(StopId, u32)],
    interrupt: &dyn Fn() -> bool,
) -> Option<NearbyResponse> {
    // Nearby departures list no ride's other departures.
    let rides = RideDescriber::new(snapshot, request.mode, DepartureRange::NONE);
    let (routes, alerts) = rides.nearby_routes(request, reachable, interrupt)?;
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
    RideDescriber::new(snapshot, request.mode, DepartureRange::NONE).route_detail(request)
}

/// A departure from one reachable stop, and what its trip is there.
#[derive(Debug, Clone, Copy)]
struct Seen {
    stop: StopId,
    walk: u32,
    candidate: Candidate,
    /// The position of the boarded GTFS trip's last stop on the candidate's pattern.
    end: u32,
    /// The boarded GTFS trip's `direction_id`.
    direction: Option<u32>,
    /// The headsign a rider reads at the stop, as an index into its route's
    /// [`RouteDepartures::headsigns`].
    headsign: usize,
}

impl Seen {
    fn runs(&self) -> bool {
        self.candidate.found == Found::Runs
    }
}

/// Every departure of one GTFS route of one file from the reachable stops.
#[derive(Debug, Default)]
struct RouteDepartures {
    /// The distinct headsigns its departures have, in order of first appearance; `None` for
    /// those without one.
    headsigns: Vec<Option<String>>,
    seen: Vec<Seen>,
}

impl RouteDepartures {
    /// The index of `headsign` in [`Self::headsigns`], adding it if it's new.
    fn intern(&mut self, headsign: Option<String>) -> usize {
        if let Some(index) = self.headsigns.iter().position(|known| *known == headsign) {
            return index;
        }
        self.headsigns.push(headsign);
        self.headsigns.len() - 1
    }
}

/// A route's page: some of its headsigns, and the departure that chose its stop.
#[derive(Debug)]
struct Page<'s> {
    /// Indices into [`RouteDepartures::headsigns`].
    members: Vec<usize>,
    chosen: &'s Seen,
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

/// How many of some departures, in order of departure, to list for `max` that run: up to
/// and including the `max`-th that runs, so canceled and skipped ones within that span are
/// listed beside them and don't count; all of them when fewer than `max` run.
fn listed_span<'a>(departures: impl Iterator<Item = &'a Candidate>, max: u8) -> usize {
    let mut runs = 0;
    let mut span = 0;
    for candidate in departures {
        if runs == max {
            break;
        }
        span += 1;
        if candidate.found == Found::Runs {
            runs += 1;
        }
    }
    span
}

/// Where a route's departures are caught: the earliest trip that runs the rider can catch at
/// any reachable stop, at the nearest stop where that trip can still be caught; then the
/// earlier departure, then the lower stop number. `None` if nothing that runs can be caught.
///
/// A trip is a run on its service date, so each run of a frequency trip is a trip of its own,
/// and a loop's two visits to one stop are one stop, at its earlier departure.
fn chosen_boarding<'s>(seen: impl Iterator<Item = &'s Seen> + Clone) -> Option<&'s Seen> {
    // Canceled and skipped departures are listed, but can't be caught.
    let running = seen.filter(|s| s.runs());
    let earliest = running
        .clone()
        .min_by_key(|s| (s.candidate.board.departure, s.walk, s.stop))?
        .candidate
        .key;
    running
        .filter(|s| s.candidate.key == earliest)
        .min_by_key(|s| (s.walk, s.candidate.board.departure, s.stop))
}

/// Whether two trips call at the same stops, from where both are boarded, until one of them
/// ends: one's stops from there are where the other's begin.
fn same_until_one_ends(a: &[StopId], b: &[StopId]) -> bool {
    let shared = a.len().min(b.len());
    a[..shared] == b[..shared]
}

/// The departure that chooses the stop of a page of some of a route's headsigns, as
/// [`chosen_boarding`] chooses one over all their departures.
fn page_stop<'s>(route: &'s RouteDepartures, members: &[usize]) -> Option<&'s Seen> {
    chosen_boarding(route.seen.iter().filter(|s| members.contains(&s.headsign)))
}

/// Whether, from `stop`, every pair of trips that run there under two different headsigns of
/// `members` calls at the same stops until one of them ends (each GTFS trip to its own last
/// stop, so a stay-seated vehicle's next trip isn't compared). Headsigns without a departure
/// that runs from `stop` aren't compared.
fn same_way_from(
    snapshot: &Snapshot,
    route: &RouteDepartures,
    members: &[usize],
    stop: StopId,
) -> bool {
    // Each member's distinct stop sequences from the stop, to its trips' ends.
    let mut itineraries: Vec<Vec<&[StopId]>> = vec![Vec::new(); members.len()];
    for seen in route.seen.iter().filter(|s| s.stop == stop && s.runs()) {
        let Some(member) = members.iter().position(|&m| m == seen.headsign) else {
            continue;
        };
        let Some(stops) = snapshot
            .route_stops(seen.candidate.pair.route)
            .and_then(|stops| stops.get(seen.candidate.pair.board as usize..=seen.end as usize))
        else {
            return false;
        };
        if !itineraries[member].contains(&stops) {
            itineraries[member].push(stops);
        }
    }
    itineraries.iter().enumerate().all(|(index, one)| {
        itineraries[index + 1..].iter().all(|other| {
            one.iter()
                .all(|a| other.iter().all(|b| same_until_one_ends(a, b)))
        })
    })
}

/// What [`route_pages`] reads of each of a route's headsigns.
struct HeadsignTrips {
    /// The distinct trips that run under each headsign, at any reachable stop.
    running: Vec<HashSet<TripKey>>,
    /// The distinct trips under each headsign, whether they run or not.
    all: Vec<HashSet<TripKey>>,
}

impl HeadsignTrips {
    fn of(route: &RouteDepartures) -> Self {
        let mut running = vec![HashSet::new(); route.headsigns.len()];
        let mut all = vec![HashSet::new(); route.headsigns.len()];
        for seen in &route.seen {
            all[seen.headsign].insert(seen.candidate.key);
            if seen.runs() {
                running[seen.headsign].insert(seen.candidate.key);
            }
        }
        Self { running, all }
    }

    /// How many distinct trips run under any of `members`.
    fn running_count(&self, members: &[usize]) -> usize {
        let mut trips: HashSet<TripKey> = HashSet::new();
        for &member in members {
            trips.extend(&self.running[member]);
        }
        trips.len()
    }

    /// Whether two sets of headsigns have a trip in common, whether it runs or not.
    fn share_a_trip(&self, a: &[usize], b: &[usize]) -> bool {
        a.iter().any(|&one| {
            b.iter()
                .any(|&other| !self.all[one].is_disjoint(&self.all[other]))
        })
    }
}

/// The pages `a` and `b` of a route make together when their headsigns go the same way, or
/// `None` when they don't.
///
/// They do when, from the stop the two would have together, each of them lists a departure
/// that runs, every headsign of either is there (by a departure that runs, or by a trip that
/// runs there under another of their headsigns, which is the same vehicle), and every pair of
/// trips that run there under two different headsigns go the same way ([`same_way_from`]).
/// Departures without a headsign go the same way as nothing.
fn joined_pages(
    snapshot: &Snapshot,
    route: &RouteDepartures,
    trips: &HeadsignTrips,
    (a, b): (&[usize], &[usize]),
) -> Option<Vec<usize>> {
    let named = |members: &[usize]| members.iter().all(|&m| route.headsigns[m].is_some());
    if !named(a) || !named(b) {
        return None;
    }
    let members: Vec<usize> = a.iter().chain(b).copied().collect();
    let stop = page_stop(route, &members)?.stop;
    let there: Vec<&Seen> = route
        .seen
        .iter()
        .filter(|s| s.stop == stop && s.runs() && members.contains(&s.headsign))
        .collect();
    let listed_trips: HashSet<TripKey> = there.iter().map(|s| s.candidate.key).collect();
    let lists = |page: &[usize]| there.iter().any(|s| page.contains(&s.headsign));
    let is_there = |member: usize| {
        there.iter().any(|s| s.headsign == member) || !trips.all[member].is_disjoint(&listed_trips)
    };
    (lists(a)
        && lists(b)
        && members.iter().all(|&m| is_there(m))
        && same_way_from(snapshot, route, &members, stop))
    .then_some(members)
}

/// Orders headsigns by string, a missing one last.
fn headsign_order(a: Option<&str>, b: Option<&str>) -> core::cmp::Ordering {
    (a.is_none(), a).cmp(&(b.is_none(), b))
}

/// A route's pages: its departures' headsigns, merged where they go the same way, and where
/// they share a trip.
///
/// Only headsigns of departures that run are paged, each starting as a page of its own.
/// Then, until nothing changes:
///
/// 1. Pages are taken in a fixed order, most distinct trips that run first, then by their
///    first headsign in string order, and each joins the first page before it that it goes
///    the same way as ([`joined_pages`]), at the stop the two would then have; or else stays.
///    Every pair of headsigns is compared, so two that each go the same way as a third but not
///    as each other stay apart.
/// 2. Pages that share a trip, running or not, become one, transitively: a trip whose
///    headsign changes along its route is one vehicle, which is on one page.
///
/// Each round only merges pages, so it ends; and since the order in each round depends only
/// on the pages, not on the order departures were found in, so does the answer. Departures
/// without a headsign never join a page by going the same way.
fn route_pages<'s>(snapshot: &Snapshot, route: &'s RouteDepartures) -> Vec<Page<'s>> {
    let trips = HeadsignTrips::of(route);
    let mut pages: Vec<Vec<usize>> = (0..route.headsigns.len())
        .filter(|&headsign| !trips.running[headsign].is_empty())
        .map(|headsign| vec![headsign])
        .collect();
    let first_headsign = |members: &[usize]| {
        members
            .iter()
            .filter_map(|&m| route.headsigns[m].as_deref())
            .min()
    };
    loop {
        let before = pages.len();
        pages.sort_by(|a, b| {
            trips
                .running_count(b)
                .cmp(&trips.running_count(a))
                .then_with(|| headsign_order(first_headsign(a), first_headsign(b)))
        });
        let mut joined: Vec<Vec<usize>> = Vec::with_capacity(pages.len());
        for page in pages {
            let merged = joined.iter().enumerate().find_map(|(index, earlier)| {
                joined_pages(snapshot, route, &trips, (earlier, &page)).map(|m| (index, m))
            });
            match merged {
                Some((index, members)) => joined[index] = members,
                None => joined.push(page),
            }
        }
        pages = union_sharing_trips(&trips, joined);
        if pages.len() == before {
            break;
        }
    }
    pages
        .into_iter()
        .filter_map(|members| {
            let chosen = page_stop(route, &members)?;
            Some(Page { members, chosen })
        })
        .collect()
}

/// The earliest page of `index`'s set, in a union-find over pages.
fn root(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

/// Pages made one wherever two share a trip, transitively.
fn union_sharing_trips(trips: &HeadsignTrips, pages: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
    // Union-find over the pages, each pointing at the earliest page of its set.
    let mut parent: Vec<usize> = (0..pages.len()).collect();
    for one in 0..pages.len() {
        for other in one + 1..pages.len() {
            if trips.share_a_trip(&pages[one], &pages[other]) {
                let (a, b) = (root(&mut parent, one), root(&mut parent, other));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut unions: Vec<Vec<usize>> = vec![Vec::new(); pages.len()];
    for (index, page) in pages.into_iter().enumerate() {
        let at = root(&mut parent, index);
        unions[at].extend(page);
    }
    unions.retain(|page| !page.is_empty());
    unions
}

/// A page's headsigns, the most departures that run from its stop in the horizon first, then
/// the earliest, then by string.
fn ranked_headsigns(route: &RouteDepartures, page: &Page<'_>) -> Vec<String> {
    let mut ranked: Vec<(&str, usize, &Seen)> = Vec::new();
    for seen in route
        .seen
        .iter()
        .filter(|s| s.stop == page.chosen.stop && s.runs() && page.members.contains(&s.headsign))
    {
        let Some(headsign) = route.headsigns[seen.headsign].as_deref() else {
            continue;
        };
        match ranked.iter_mut().find(|(known, _, _)| *known == headsign) {
            Some((_, count, first)) => {
                *count += 1;
                if seen.candidate.order() < first.candidate.order() {
                    *first = seen;
                }
            }
            None => ranked.push((headsign, 1, seen)),
        }
    }
    ranked.sort_by(|(a, a_count, a_first), (b, b_count, b_first)| {
        b_count
            .cmp(a_count)
            .then_with(|| a_first.candidate.order().cmp(&b_first.candidate.order()))
            .then_with(|| a.cmp(b))
    });
    ranked
        .into_iter()
        .map(|(headsign, _, _)| headsign.to_owned())
        .collect()
}

/// The `direction_id` all of a page's trips that run from its stop share, if they do.
fn shared_direction(route: &RouteDepartures, page: &Page<'_>) -> Option<u32> {
    let mut directions = route
        .seen
        .iter()
        .filter(|s| s.stop == page.chosen.stop && s.runs() && page.members.contains(&s.headsign))
        .map(|s| s.direction);
    let first = directions.next()??;
    directions.all(|d| d == Some(first)).then_some(first)
}

/// The order of a route's pages: by `direction_id`, none last, then by the first of their
/// headsigns in string order, none last.
fn page_order(a: &NearbyPage, b: &NearbyPage) -> core::cmp::Ordering {
    fn first(page: &NearbyPage) -> Option<&str> {
        page.headsigns.iter().min().map(String::as_str)
    }
    (a.direction_id.is_none(), a.direction_id)
        .cmp(&(b.direction_id.is_none(), b.direction_id))
        .then_with(|| headsign_order(first(a), first(b)))
}

/// What a trip is where a rider boards it: its file, GTFS route, `direction_id`, and the
/// positions of its first and last stops on the pattern.
struct Boarded {
    part: usize,
    route: GtfsRoute,
    direction: Option<u32>,
    first: usize,
    end: usize,
}

impl RideDescriber<'_> {
    /// The GTFS trip a run is on `pattern`, at the original trip `segment` picks on a combined
    /// stay-seated trip.
    ///
    /// The original trip's route, direction, and positions, on its own pattern; on a pattern the
    /// overlay moved it to, the trip's route and direction and that whole pattern. A trip only
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
                first: segment.map_or(0, |s| s.first_position as usize),
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
            first: 0,
            end: pattern_end,
        })
    }

    /// The GTFS trip a departure boards at a pair's boarding.
    fn boarded_at(&self, key: TripKey, pair: Pair) -> Option<Boarded> {
        self.boarded(key, pair.route, &|segment| {
            segment.first_position <= pair.board && pair.board <= segment.last_position
        })
    }

    /// What a rider waiting at `position` of `pattern` reads on a run's vehicle, as its
    /// departure from there says: the stop's headsign there, or else its trip's.
    fn headsign_at(&self, key: TripKey, pattern: RouteId, position: u32) -> Option<String> {
        let view = self.trip_view(key, pattern, (position, position));
        let context = self.visit_context(&view, key);
        let index = position as usize;
        boarding_headsign(&view, &context, index, self.live_at(key, index))
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
        let filter = self.mode.route_filter();
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
            continues_as: self.continues_as(candidate),
        }
    }

    /// The route a departure's vehicle continues as after its trip, with riders staying on:
    /// the next original trip of a stay-seated combined trip, when it is another route's and
    /// still lets riders on or off somewhere. `None` for a canceled departure, whose vehicle
    /// doesn't run.
    fn continues_as(&self, candidate: &Candidate) -> Option<ContinuesAs> {
        if candidate.found == Found::Canceled {
            return None;
        }
        let (part, local) = self.set().trip_part(candidate.key.trip)?;
        let segments = part.compiled().trip_segments(local);
        let board = candidate.pair.board;
        let index = segments
            .iter()
            .position(|s| s.first_position <= board && board <= s.last_position)?;
        let (boarded, next) = (segments.get(index)?, segments.get(index + 1)?);
        if next.route == boarded.route || !self.carries_riders(candidate, next) {
            return None;
        }
        let route = part.compiled().route(next.route)?;
        let headsign = self
            .snapshot
            .rider
            .trip_details_named(candidate.key, next.gtfs_id)
            .and_then(|details| details.headsign.clone())
            .or_else(|| next.headsign.map(str::to_owned));
        Some(ContinuesAs {
            route_id: route.gtfs_id.to_owned(),
            short_name: route.short_name.map(str::to_owned),
            headsign,
        })
    }

    /// Whether a departure's vehicle carries riders on `segment`, an original trip of its
    /// stay-seated combined trip: whether it lets anyone on or off there.
    ///
    /// A canceled or replaced stretch, or one a `NO_SERVICE` alert removed, lets no one on or
    /// off, so the vehicle doesn't go on as that trip.
    fn carries_riders(
        &self,
        candidate: &Candidate,
        segment: &valinor_gtfs::TripSegmentInfo<'_>,
    ) -> bool {
        let snapshot = &self.snapshot.snapshot;
        (segment.first_position..=segment.last_position).any(|position| {
            snapshot
                .lane_event(candidate.lane, candidate.row, position)
                .is_some_and(|event| event.rules.pickup || event.rules.drop_off)
        })
    }

    /// Where a departure's vehicle stops carrying riders, as a position of its pattern:
    /// the end of its GTFS trip, or on a stay-seated combined trip, the end of the last of the
    /// original trips from the boarded one on that carries riders, each in turn.
    ///
    /// The compiler's chain cap and rules (`block_id`, and `transfer_type` 4 and 5) made the
    /// combined trip, so every original trip on it is one a rider can stay on for.
    fn chain_end(&self, candidate: &Candidate, boarded: &Boarded) -> usize {
        let Some((part, local)) = self.set().trip_part(candidate.key.trip) else {
            return boarded.end;
        };
        let segments = part.compiled().trip_segments(local);
        let board = candidate.pair.board;
        let Some(index) = segments
            .iter()
            .position(|s| s.first_position <= board && board <= s.last_position)
        else {
            return boarded.end;
        };
        // The boarded trip carries riders, since the departure boards it.
        let last = segments
            .iter()
            .skip(index + 1)
            .take_while(|segment| self.carries_riders(candidate, segment))
            .last()
            .or_else(|| segments.get(index))
            .map_or(boarded.end, |segment| segment.last_position as usize);
        // A moved trip keeps its stretches' positions, on a pattern that holds them all;
        // should it not, the ride ends with the pattern.
        let pattern_end = self
            .snapshot
            .snapshot
            .route_stops(candidate.pair.route)
            .map_or(boarded.end, |stops| stops.len().saturating_sub(1));
        last.min(pattern_end)
    }

    /// Every departure from the reachable stops in the horizon, by GTFS route of its file.
    ///
    /// `None` if `interrupt` asked to stop.
    fn reachable_departures(
        &self,
        request: &NearbyRequest,
        reachable: &[(StopId, u32)],
        interrupt: &dyn Fn() -> bool,
    ) -> Option<HashMap<(usize, GtfsRoute), RouteDepartures>> {
        let end = request
            .time
            .saturating_add(i64::from(request.horizon_seconds));
        let mut by_route: HashMap<(usize, GtfsRoute), RouteDepartures> = HashMap::new();
        for &(stop, walk) in reachable {
            if interrupt() {
                return None;
            }
            // As routing counts a first boarding: at the stop by the time plus the walk.
            let ready = request.time.saturating_add(i64::from(walk));
            for candidate in self.stop_departures(stop, (ready, end), &mut |_, _| true) {
                let Some(boarded) = self.boarded_at(candidate.key, candidate.pair) else {
                    continue;
                };
                let Ok(trip_end) = u32::try_from(boarded.end) else {
                    continue;
                };
                let headsign =
                    self.headsign_at(candidate.key, candidate.pair.route, candidate.pair.board);
                let route = by_route.entry((boarded.part, boarded.route)).or_default();
                let headsign = route.intern(headsign);
                route.seen.push(Seen {
                    stop,
                    walk,
                    candidate,
                    end: trip_end,
                    direction: boarded.direction,
                    headsign,
                });
            }
        }
        Some(by_route)
    }

    /// A page as JSON, with its departures' alerts in `pool`.
    fn nearby_page(
        &self,
        route: &RouteDepartures,
        page: &Page<'_>,
        max_departures: u8,
        pool: &mut Vec<AlertResponse>,
    ) -> NearbyPage {
        let (stop, walk) = (page.chosen.stop, page.chosen.walk);
        // Canceled and skipped departures are listed under their own headsign's page.
        let mut listed: Vec<&Candidate> = route
            .seen
            .iter()
            .filter(|s| s.stop == stop && page.members.contains(&s.headsign))
            .map(|s| &s.candidate)
            .collect();
        listed.sort_unstable_by_key(|c| c.order());
        listed.truncate(listed_span(listed.iter().copied(), max_departures));
        let stop_ref = self.stop_ref(stop);
        let departures = listed
            .into_iter()
            .map(|c| self.stop_departure(stop, stop_ref.accessibility, c, pool))
            .collect();
        NearbyPage {
            headsigns: ranked_headsigns(route, page),
            direction_id: shared_direction(route, page),
            stop: stop_ref,
            walk_seconds: walk,
            // The route's order reads the trip as the rider catches it, at the chosen stop.
            soonest_departure: self.origin()
                + i64::from(page.chosen.candidate.board.departure.into_inner()),
            departures,
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
        let by_route = self.reachable_departures(request, reachable, interrupt)?;
        let mut pool = Vec::new();
        let mut routes: Vec<(i64, NearbyRoute)> = Vec::new();
        for ((part, route), departures) in by_route {
            let Some(part) = self.set().parts().nth(part) else {
                continue;
            };
            let Some(info) = part.compiled().route(route) else {
                continue;
            };
            let mut pages: Vec<NearbyPage> = route_pages(&self.snapshot.snapshot, &departures)
                .iter()
                .map(|page| self.nearby_page(&departures, page, request.max_departures, &mut pool))
                .collect();
            if pages.is_empty() {
                continue;
            }
            pages.sort_by(page_order);
            let soonest = pages
                .iter()
                .map(|page| page.soonest_departure)
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
                    pages,
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

    /// One page of a route from one stop.
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

        let of_route = |boarded: Option<Boarded>| {
            boarded.is_some_and(|boarded| boarded.part == part.index() && boarded.route == route)
        };
        // A page without headsigns is the departures that have none.
        let on_page = |headsign: Option<String>| match headsign {
            Some(headsign) => request.headsigns.contains(&headsign),
            None => request.headsigns.is_empty(),
        };
        let ready = request.time.saturating_add(i64::from(request.walk_seconds));
        let end = request
            .time
            .saturating_add(i64::from(request.horizon_seconds));
        let mut found = self.stop_departures(stop, (ready, end), &mut |key, pair| {
            of_route(self.boarded_at(key, pair))
        });
        found.retain(|c| on_page(self.headsign_at(c.key, c.pair.route, c.pair.board)));
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
        let (ride, shape_to_stop) = chosen
            .and_then(|candidate| self.detail_ride(stop, candidate, ready))
            .unzip();

        let stop_ref = self.stop_ref(stop);
        let mut pool = Vec::new();
        let departures: Vec<StopDeparture> = found
            .iter()
            .take(listed_span(found.iter(), request.max_departures))
            .map(|c| self.stop_departure(stop, stop_ref.accessibility, c, &mut pool))
            .collect();
        let vehicles =
            self.route_vehicles(info.gtfs_id, &request.feed, &|key, pattern, trip_id| {
                let Some(boarded) =
                    self.boarded(key, pattern, &|segment| Some(segment.gtfs_id) == trip_id)
                else {
                    return false;
                };
                if boarded.part != part.index() || boarded.route != route {
                    return false;
                }
                let Some(stops) = self.snapshot.snapshot.route_stops(pattern) else {
                    return false;
                };
                // The trip leaves the stop under one of the page's headsigns: at a visit of its own
                // GTFS trip that a later stop of that trip follows.
                let visits = stops.get(boarded.first..boarded.end).unwrap_or(&[]);
                visits.iter().enumerate().any(|(offset, &visit)| {
                    visit == stop
                        && u32::try_from(boarded.first + offset)
                            .is_ok_and(|position| on_page(self.headsign_at(key, pattern, position)))
                })
            });
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
            headsigns: request.headsigns.clone(),
            stop: stop_ref,
            walk_seconds: request.walk_seconds,
            departures,
            ride,
            shape_to_stop: shape_to_stop.flatten(),
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

    /// The ride a detail shows on a departure from `stop`, for a rider ready there at `ready`,
    /// and its trip's shape before the stop.
    fn detail_ride(
        &self,
        stop: StopId,
        candidate: &Candidate,
        ready: i64,
    ) -> Option<(TransitLeg, Option<String>)> {
        let ride = self.ride_to_end(stop, candidate)?;
        let board = self.origin() + i64::from(candidate.board.departure.into_inner());
        Some((
            self.transit_leg(&ride, board - ready, &|_| {}),
            self.shape_to_stop(candidate),
        ))
    }

    /// A departure's trip before the stop it boards at: its shape from its GTFS trip's first
    /// stop to there, as polyline6.
    ///
    /// The trip's shape where it has one there, read as a ride's is; otherwise, or on a pattern
    /// real-time data moved the trip to, straight from stop to stop, as a client draws a ride
    /// without a shape from its stops (it has none of the stops before this one to draw from).
    /// `None` when it boards at its trip's first stop, or fewer than two of the stops are placed.
    fn shape_to_stop(&self, candidate: &Candidate) -> Option<String> {
        let pair = candidate.pair;
        let boarded = self.boarded_at(candidate.key, pair)?;
        let board = pair.board as usize;
        if board <= boarded.first {
            return None;
        }
        // Static positions are the trip's own only on its own pattern.
        let shaped = self
            .set()
            .trip_part(candidate.key.trip)
            .and_then(|(part, local)| {
                let info = part.compiled().trip(local)?;
                (part.global_route(info.pattern) == pair.route)
                    .then(|| {
                        part.compiled()
                            .shape_points_between(local, boarded.first, board)
                    })
                    .flatten()
            });
        let points = match shaped {
            Some(points) => points,
            None => self
                .snapshot
                .snapshot
                .route_stops(pair.route)?
                .get(boarded.first..=board)?
                .iter()
                .filter_map(|&stop| self.stop_coordinate(stop))
                .collect(),
        };
        (points.len() >= 2).then(|| encode_polyline6(&points))
    }

    /// A departure ridden from `stop` to where its vehicle stops carrying riders: the last stop
    /// of its GTFS trip, or of the last trip of its stay-seated chain that does (see
    /// [`Self::chain_end`]), so a ride on a combined trip is one ride in segments, as a plan's.
    fn ride_to_end(&self, stop: StopId, candidate: &Candidate) -> Option<valinor_transit::Ride> {
        let pair = candidate.pair;
        let boarded = self.boarded_at(candidate.key, pair)?;
        let last = self.chain_end(candidate, &boarded);
        let end = u32::try_from(last).ok()?;
        let alight_stop = *self.snapshot.snapshot.route_stops(pair.route)?.get(last)?;
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

    /// Vehicles on a route's trips that `wanted` keeps, by the run each is on, the pattern it
    /// runs, and its `trip_id`, in order of ID.
    ///
    /// A vehicle real-time data doesn't match to a trip of the timetable has no stops to be
    /// read from, so it isn't listed.
    fn route_vehicles(
        &self,
        route_id: &str,
        feed: &str,
        wanted: &dyn Fn(TripKey, RouteId, Option<&str>) -> bool,
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
                pattern.is_some_and(|pattern| wanted(key, pattern, vehicle.trip_id.as_deref()))
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

/// Keeps the alerts of `pool` that a listed departure still indexes, renumbering them.
fn compact_alerts(routes: &mut [NearbyRoute], pool: Vec<AlertResponse>) -> Vec<AlertResponse> {
    let mut renumbered: Vec<Option<usize>> = vec![None; pool.len()];
    let mut kept: Vec<usize> = Vec::new();
    for departure in routes
        .iter_mut()
        .flat_map(|route| &mut route.pages)
        .flat_map(|page| &mut page.departures)
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
