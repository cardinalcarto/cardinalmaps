//! Nearby departures: which routes a rider can catch from where they are, under which
//! headsigns, and when; and one of those routes in detail, from the stop a nearby row's page
//! uses.
//!
//! See `docs/transit.md` ("Nearby departures"). In outline:
//!
//! - Stops are found by the access search a plan uses ([`AccessEgressSearch::access_table`]),
//!   a walk on real streets bounded by Sif's cost, honoring a wheelchair, not straight lines.
//! - Departures are found as a ride's departure range finds them, from the same pinned
//!   snapshot, so canceled and skipped departures, frequency runs, and service days that run
//!   past midnight are handled as they are there.
//! - Departures are grouped by GTFS route, then into pages by the headsign a rider reads at the
//!   stop. Two headsigns of a route share a page when, from the page's stop, every pair of
//!   their trips calls at the same stops until one of them ends.
//! - A departure is catchable when it leaves at or after the request's time plus the walk to its
//!   stop, as routing counts a first boarding. A page's stop is the nearest of those where
//!   its earliest catchable trip can still be caught.
//!
//! Like [`crate::plan`], this has no clock: `now` is always an argument.

use core::error::Error;

use valinor_gtfs::LatLon;
use valinor_transit::StopId;

use crate::error::{PlanError, StateError};
use crate::plan::{
    AccessEgressSearch, AccessRequest, MAX_DEPARTURE_WINDOW_SECONDS, MAX_RANGE_DEPARTURES,
    PlanStage, TravelMode, coordinates_are_valid,
};
use crate::response::{
    NearbyResponse, RouteDetailResponse, RouteLookupError, build_nearby, build_route_detail,
    empty_nearby,
};
use crate::state::{TransitSnapshot, TransitState};

/// How long a walk to a stop may be by default, in seconds.
///
/// Longer than a plan's typical walk, because not everywhere has good transit nearby.
pub const DEFAULT_NEARBY_WALK_SECONDS: u32 = 1800;

/// How far ahead departures are looked for by default, in seconds.
pub const DEFAULT_HORIZON_SECONDS: u32 = 2 * 3600;

/// How many departures each page lists by default.
pub const DEFAULT_NEARBY_DEPARTURES: u8 = 3;

/// A nearby-departures question.
#[derive(Debug, Clone, Copy)]
pub struct NearbyRequest {
    /// Where the rider is.
    pub location: LatLon,
    /// When the rider sets off, in POSIX seconds.
    pub time: i64,
    /// The moment the query runs, in POSIX seconds.
    pub now: i64,
    /// [`TravelMode::Walking`] or [`TravelMode::Wheelchair`].
    pub mode: TravelMode,
    /// The longest walk to a stop, in seconds.
    pub max_walk_seconds: u32,
    /// How far after `time` departures are listed, in seconds; at most a day.
    pub horizon_seconds: u32,
    /// How many departures that run each page lists; 1 to [`MAX_RANGE_DEPARTURES`].
    /// Canceled and skipped ones among them are listed too, and don't count.
    pub max_departures: u8,
    /// How many routes to list, soonest first; all when `None`.
    pub max_routes: Option<u32>,
}

impl NearbyRequest {
    /// A request on foot with the default walk, horizon, and departure count, and every route.
    pub fn new(location: LatLon, time: i64, now: i64) -> Self {
        Self {
            location,
            time,
            now,
            mode: TravelMode::Walking,
            max_walk_seconds: DEFAULT_NEARBY_WALK_SECONDS,
            horizon_seconds: DEFAULT_HORIZON_SECONDS,
            max_departures: DEFAULT_NEARBY_DEPARTURES,
            max_routes: None,
        }
    }
}

/// A trip a route-detail request asks to see, by the IDs a departure gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripChoice {
    /// `trip_id`.
    pub trip_id: String,
    /// `start_date`, as `YYYYMMDD`; any of the stop's runs of the trip when `None`.
    pub start_date: Option<String>,
}

/// One page of a route from one stop, by the identifiers a nearby response gave.
#[derive(Debug, Clone)]
pub struct RouteDetailRequest {
    /// The route's feed.
    pub feed: String,
    /// `route_id`.
    pub route_id: String,
    /// The page, by its `headsigns`: the departures listed are those whose headsign at the stop
    /// is one of these, or for a page without headsigns, those without one.
    pub headsigns: Vec<String>,
    /// The stop, by its `stop_id` in the route's feed.
    pub stop_id: String,
    /// When the rider sets off, in POSIX seconds.
    pub time: i64,
    /// The moment the query runs, in POSIX seconds.
    pub now: i64,
    /// [`TravelMode::Walking`] or [`TravelMode::Wheelchair`].
    pub mode: TravelMode,
    /// The walk to the stop, in seconds: departures before `time` plus this are not listed.
    pub walk_seconds: u32,
    /// How far after `time` departures are listed, in seconds; at most a day.
    pub horizon_seconds: u32,
    /// How many departures that run to list, as for [`NearbyRequest::max_departures`].
    pub max_departures: u8,
    /// The trip whose stops to show; the first departure that runs when `None`.
    pub trip: Option<TripChoice>,
}

fn check_mode<T: Error + 'static, A: Error + 'static>(
    mode: TravelMode,
) -> Result<(), PlanError<T, A>> {
    match mode {
        TravelMode::Walking | TravelMode::Wheelchair => Ok(()),
        TravelMode::Cycling => Err(PlanError::InvalidRequest(
            "nearby departures are for a rider on foot or in a wheelchair",
        )),
    }
}

/// Checks the horizon and departure count, and returns the end of the horizon.
fn check_horizon<T: Error + 'static, A: Error + 'static>(
    time: i64,
    horizon_seconds: u32,
    max_departures: u8,
) -> Result<i64, PlanError<T, A>> {
    if horizon_seconds == 0 || horizon_seconds > MAX_DEPARTURE_WINDOW_SECONDS {
        return Err(PlanError::InvalidRequest(
            "horizon_seconds must be more than zero and at most a day",
        ));
    }
    if max_departures == 0 || max_departures > MAX_RANGE_DEPARTURES {
        return Err(PlanError::InvalidRequest(
            "max_departures must be one to ten",
        ));
    }
    time.checked_add(i64::from(horizon_seconds))
        .ok_or(PlanError::InvalidRequest("the time is out of range"))
}

/// Checks a nearby request and returns the end of its horizon.
pub(crate) fn validated_nearby<T: Error + 'static, A: Error + 'static>(
    request: &NearbyRequest,
) -> Result<i64, PlanError<T, A>> {
    check_mode(request.mode)?;
    if !coordinates_are_valid(request.location) {
        return Err(PlanError::InvalidRequest(
            "a coordinate is not a valid latitude and longitude",
        ));
    }
    if request.max_routes == Some(0) {
        return Err(PlanError::InvalidRequest("max_routes must be at least one"));
    }
    check_horizon(
        request.time,
        request.horizon_seconds,
        request.max_departures,
    )
}

/// Checks a route-detail request and returns the end of its horizon.
pub(crate) fn validated_route<T: Error + 'static, A: Error + 'static>(
    request: &RouteDetailRequest,
) -> Result<i64, PlanError<T, A>> {
    check_mode(request.mode)?;
    if request.walk_seconds > MAX_DEPARTURE_WINDOW_SECONDS {
        return Err(PlanError::InvalidRequest("walk_seconds is more than a day"));
    }
    // A response never names an empty headsign, and a page names each of its own once.
    if request.headsigns.iter().any(String::is_empty) {
        return Err(PlanError::InvalidRequest("a headsign is never empty"));
    }
    if request
        .headsigns
        .iter()
        .enumerate()
        .any(|(i, headsign)| request.headsigns[..i].contains(headsign))
    {
        return Err(PlanError::InvalidRequest("headsigns names one twice"));
    }
    check_horizon(
        request.time,
        request.horizon_seconds,
        request.max_departures,
    )
}

/// Answers a nearby request over one pinned snapshot, pinning it first.
///
/// # Errors
///
/// If the request is invalid, `interrupt` asks to stop, or the access search fails.
pub fn nearby<T, A>(
    state: &TransitState,
    access: &mut A,
    request: &NearbyRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<NearbyResponse, PlanError<T, A::Error>>
where
    T: Error + 'static,
    A: AccessEgressSearch,
{
    let latest = validated_nearby(request)?;
    let snapshot = state.pin_for_departure(request.now, request.time, latest)?;
    nearby_pinned(&snapshot, access, request, interrupt)
}

/// Answers a nearby request over a snapshot the caller has already pinned.
///
/// # Errors
///
/// If the request is invalid, `interrupt` asks to stop, or the access search fails.
pub fn nearby_pinned<T, A>(
    snapshot: &TransitSnapshot,
    access: &mut A,
    request: &NearbyRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<NearbyResponse, PlanError<T, A::Error>>
where
    T: Error + 'static,
    A: AccessEgressSearch,
{
    validated_nearby::<T, A::Error>(request)?;
    let table = access
        .access_table(
            &AccessRequest {
                origin: request.location,
                mode: request.mode,
                snapshot,
            },
            interrupt,
        )
        .map_err(PlanError::AccessEgress)?;
    if interrupt() {
        // An interrupted search returns an empty table, which isn't "nothing nearby".
        return Err(PlanError::Interrupted);
    }
    if table.is_empty() {
        return Ok(empty_nearby(snapshot, request));
    }
    access.enter_stage(PlanStage::Departures);
    let reachable: Vec<(StopId, u32)> = table
        .reachable()
        .map(|(stop, seconds)| (stop, seconds.into_inner()))
        .collect();
    let response = build_nearby(snapshot, request, &reachable, interrupt);
    access.enter_stage(PlanStage::Assemble);
    response.ok_or(PlanError::Interrupted)
}

/// Answers a route-detail request over one pinned snapshot, pinning it first.
///
/// # Errors
///
/// If the request is invalid or names what isn't there, or `interrupt` asks to stop.
pub fn route_detail<T, A>(
    state: &TransitState,
    request: &RouteDetailRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<RouteDetailResponse, PlanError<T, A>>
where
    T: Error + 'static,
    A: Error + 'static,
{
    let latest = validated_route(request)?;
    let snapshot = state.pin_for_departure(request.now, request.time, latest)?;
    route_detail_pinned(&snapshot, request, interrupt)
}

/// Answers a route-detail request over a snapshot the caller has already pinned.
///
/// # Errors
///
/// If the request is invalid or names what isn't there, or `interrupt` asks to stop.
pub fn route_detail_pinned<T, A>(
    snapshot: &TransitSnapshot,
    request: &RouteDetailRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<RouteDetailResponse, PlanError<T, A>>
where
    T: Error + 'static,
    A: Error + 'static,
{
    validated_route::<T, A>(request)?;
    let response = build_route_detail(snapshot, request).map_err(|error| match error {
        RouteLookupError::UnknownFeed(feed) => PlanError::State(StateError::UnknownFeed(feed)),
        RouteLookupError::NotFound(missing) => PlanError::NotFound(missing),
    })?;
    if interrupt() {
        return Err(PlanError::Interrupted);
    }
    Ok(response)
}
