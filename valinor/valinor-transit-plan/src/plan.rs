//! The query API: one request over one pinned snapshot, answered as rider-facing journeys.

use core::error::Error;

use valinor_gtfs::LatLon;
use valinor_transit::{
    RouteAttributes, RouteFilter, Router, SearchError, Seconds, StopId, StopTable, Time,
    TransferSearch, TransitQuery,
};

use crate::error::PlanError;
use crate::response::{NoJourneysReason, PlanResponse, WalkPath, build_response, empty_response};
use crate::state::{TransitSnapshot, TransitState};

/// The longest departure window a query may ask for.
///
/// A profile's cost grows with its window, and a window spanning days has no rider-facing use.
pub const MAX_DEPARTURE_WINDOW_SECONDS: u32 = 24 * 3600;

/// How a rider travels between transit legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TravelMode {
    /// On foot; all routes may be ridden.
    #[default]
    Walking,
    /// With a bicycle on board; only routes that allow bikes may be ridden.
    Cycling,
}

impl TravelMode {
    pub(crate) fn route_filter(self) -> RouteFilter {
        match self {
            Self::Walking => RouteFilter::ALL,
            Self::Cycling => RouteFilter::requiring(RouteAttributes::BIKES_ALLOWED),
        }
    }

    /// The name used in response JSON.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Walking => "walking",
            Self::Cycling => "cycling",
        }
    }
}

/// Knobs a query sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitOptions {
    /// The most trips a journey may use; at most [`valinor_transit::MAX_TRIPS`].
    pub max_trips: u8,
    /// The longest access or egress leg to consider, in seconds.
    pub max_walk_seconds: u32,
}

impl Default for TransitOptions {
    fn default() -> Self {
        Self {
            max_trips: 8,
            max_walk_seconds: 1800,
        }
    }
}

/// One planning question.
#[derive(Debug, Clone, Copy)]
pub struct TransitRequest {
    /// Where the rider starts.
    pub origin: LatLon,
    /// Where the rider is going.
    pub destination: LatLon,
    /// The start of the departure window, in POSIX seconds.
    pub departure: i64,
    /// The length of the departure window, in seconds; see [`Self::new`] for the default.
    pub window_seconds: u32,
    /// Knobs for the search.
    pub options: TransitOptions,
    /// How the rider travels between transit legs.
    pub mode: TravelMode,
    /// The moment the query runs, in POSIX seconds.
    pub now: i64,
}

impl TransitRequest {
    /// A request with a 60-minute window and default options.
    pub fn new(origin: LatLon, destination: LatLon, departure: i64, now: i64) -> Self {
        Self {
            origin,
            destination,
            departure,
            window_seconds: 3600,
            options: TransitOptions::default(),
            mode: TravelMode::Walking,
            now,
        }
    }
}

/// What an [`AccessEgressSearch`] is asked.
#[derive(Debug, Clone, Copy)]
pub struct AccessEgressRequest<'a> {
    /// Where the rider starts.
    pub origin: LatLon,
    /// Where the rider is going.
    pub destination: LatLon,
    /// How the rider travels.
    pub mode: TravelMode,
    /// The pinned state, for its stop numbering and locations.
    pub snapshot: &'a TransitSnapshot,
}

/// Stop tables for one query.
#[derive(Debug, Clone)]
pub struct AccessEgressTables {
    /// Seconds from the origin to each stop.
    pub access: StopTable,
    /// Seconds from each stop to the destination.
    pub egress: StopTable,
    /// How a direct trip without transit fared; journeys slower than it are removed.
    pub direct: DirectSearch,
}

/// The outcome of a direct street search for the whole trip.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DirectSearch {
    /// Found: this many seconds.
    Found(Seconds),
    /// The search ran and the streets don't connect the two ends.
    NoRoute,
    /// The search didn't run, because it would take longer than transit could ever matter.
    #[default]
    NotSearched,
    /// The search ran into a problem; the plan is still valid without the comparison.
    Failed(String),
}

/// Computes access and egress tables, once per query, and expands walking legs for the response.
///
/// A table must cover every stop riders could use: the search doesn't walk between it and the
/// first boarding, or between the last alighting and the destination.
/// `valinor-walk` implements this over tiles ([`crate::StreetPlanner`]); tests use a
/// straight-line search. The expansion methods run after the journeys are found.
pub trait AccessEgressSearch {
    /// A failure of the search itself.
    type Error: Error + 'static;

    /// The transfer search's resolved walk, which [`Self::transfer_path`] expands.
    type Walk;

    /// Computes the tables for one request.
    ///
    /// # Errors
    ///
    /// Only for failures of the search; check `interrupt` and return an empty table to stop.
    fn tables(
        &mut self,
        request: &AccessEgressRequest<'_>,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<AccessEgressTables, Self::Error>;

    /// The walking path from the origin to `stop`, or `None` if it has none to show.
    ///
    /// # Errors
    ///
    /// If the path can't be expanded.
    fn access_path(
        &mut self,
        _request: &AccessEgressRequest<'_>,
        _stop: StopId,
    ) -> Result<Option<WalkPath>, Self::Error> {
        Ok(None)
    }

    /// The walking path from `stop` to the destination, or `None` if it has none to show.
    ///
    /// # Errors
    ///
    /// If the path can't be expanded.
    fn egress_path(
        &mut self,
        _request: &AccessEgressRequest<'_>,
        _stop: StopId,
    ) -> Result<Option<WalkPath>, Self::Error> {
        Ok(None)
    }

    /// The walking path of a transfer between two stops, or `None` if it has none to show.
    ///
    /// # Errors
    ///
    /// If the path can't be expanded.
    fn transfer_path(
        &mut self,
        _request: &AccessEgressRequest<'_>,
        _walk: &Self::Walk,
        _from: StopId,
        _to: StopId,
    ) -> Result<Option<WalkPath>, Self::Error> {
        Ok(None)
    }
}

/// Checks a request and returns the last departure of its window.
pub(crate) fn validated_latest<T, A>(request: &TransitRequest) -> Result<i64, PlanError<T, A>>
where
    T: Error + 'static,
    A: Error + 'static,
{
    if request.window_seconds > MAX_DEPARTURE_WINDOW_SECONDS {
        return Err(PlanError::InvalidRequest("the window is longer than a day"));
    }
    if !coordinates_are_valid(request.origin) || !coordinates_are_valid(request.destination) {
        return Err(PlanError::InvalidRequest(
            "a coordinate is not a valid latitude and longitude",
        ));
    }
    if request.options.max_trips == 0 || request.options.max_trips > valinor_transit::MAX_TRIPS {
        return Err(PlanError::InvalidRequest("max_trips is out of range"));
    }
    request
        .departure
        .checked_add(i64::from(request.window_seconds))
        .ok_or(PlanError::InvalidRequest("the departure is out of range"))
}

/// Answers a request over one pinned snapshot, pinning it first.
///
/// # Errors
///
/// If the request is invalid, `interrupt` asks to stop, or a search fails.
pub fn plan<T, A>(
    state: &TransitState,
    router: &mut Router<T>,
    access_egress: &mut A,
    request: &TransitRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<PlanResponse, PlanError<T::Error, A::Error>>
where
    T: TransferSearch,
    A: AccessEgressSearch<Walk = T::Walk>,
{
    let latest_unix = validated_latest(request)?;
    let snapshot = state.pin_for_departure(request.now, request.departure, latest_unix)?;
    plan_pinned(&snapshot, router, access_egress, request, interrupt)
}

/// Answers a request over a snapshot the caller has already pinned.
///
/// A caller that pins first can prepare per-snapshot state (see [`crate::StreetPlanner`]).
///
/// # Errors
///
/// If the request is invalid, `interrupt` asks to stop, or a search fails.
pub fn plan_pinned<T, A>(
    snapshot: &TransitSnapshot,
    router: &mut Router<T>,
    access_egress: &mut A,
    request: &TransitRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<PlanResponse, PlanError<T::Error, A::Error>>
where
    T: TransferSearch,
    A: AccessEgressSearch<Walk = T::Walk>,
{
    let latest_unix = validated_latest(request)?;
    let tables = access_egress
        .tables(
            &AccessEgressRequest {
                origin: request.origin,
                destination: request.destination,
                mode: request.mode,
                snapshot,
            },
            interrupt,
        )
        .map_err(PlanError::AccessEgress)?;
    if interrupt() {
        // A search over empty tables would finish before ever checking the interrupt.
        return Err(PlanError::Interrupted);
    }
    if tables.access.is_empty() || tables.egress.is_empty() {
        let reason = if tables.access.is_empty() {
            NoJourneysReason::NoAccess
        } else {
            NoJourneysReason::NoEgress
        };
        return Ok(empty_response(snapshot, request, reason, &tables.direct));
    }

    let earliest = window_time(snapshot, request.departure)?;
    let latest = window_time(snapshot, latest_unix)?;

    let query = TransitQuery {
        snapshot: snapshot.snapshot.clone(),
        access: &tables.access,
        egress: &tables.egress,
        filter: request.mode.route_filter(),
        max_trips: request.options.max_trips,
    };
    let profile = router
        .profile(&query, earliest, latest, interrupt)
        .map_err(|error| match error {
            SearchError::Interrupted => PlanError::Interrupted,
            other => PlanError::Search(other),
        })?;

    build_response(snapshot, request, profile, &tables.direct, access_egress)
        .map_err(PlanError::AccessEgress)
}

fn window_time<T: Error + 'static, A: Error + 'static>(
    snapshot: &TransitSnapshot,
    unix: i64,
) -> Result<Time, PlanError<T, A>> {
    let seconds = unix.saturating_sub(snapshot.window.origin_unix);
    i32::try_from(seconds)
        .ok()
        .and_then(|time| Time::try_new(time).ok())
        .ok_or(PlanError::TimeOutOfRange(seconds))
}

fn coordinates_are_valid(coordinate: LatLon) -> bool {
    coordinate.lat.is_finite()
        && coordinate.lon.is_finite()
        && (-90.0..=90.0).contains(&coordinate.lat)
        && (-180.0..=180.0).contains(&coordinate.lon)
}
