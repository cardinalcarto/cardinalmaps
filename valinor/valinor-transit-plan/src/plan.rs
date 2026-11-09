//! The query API: one request over one pinned snapshot, answered as rider-facing journeys.

use core::error::Error;

use valinor_gtfs::LatLon;
use valinor_transit::{
    RouteAttributes, RouteFilter, Router, SearchError, Seconds, StopTable, Time, TransferSearch,
    TransitQuery,
};

use crate::error::PlanError;
use crate::response::{PlanResponse, build_response};
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
}

impl Default for TransitOptions {
    fn default() -> Self {
        Self { max_trips: 8 }
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
    /// Seconds for the whole trip without riding transit, if a street search can do it.
    ///
    /// Journeys slower than this are removed from the response.
    pub direct: Option<Seconds>,
}

/// Computes access and egress tables, once per query.
///
/// A table must cover every stop riders could use: the search doesn't walk between it and the
/// first boarding, or between the last alighting and the destination.
/// `valinor-walk` implements this over tiles; tests use a straight-line search.
pub trait AccessEgressSearch {
    /// A failure of the search itself.
    type Error: Error + 'static;

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
}

/// Answers a request over one pinned snapshot.
///
/// # Errors
///
/// If the request is invalid, `interrupt` asks to stop, or a search fails.
#[expect(
    clippy::type_complexity,
    reason = "the one place both search errors meet"
)]
pub fn plan<T, A>(
    state: &TransitState,
    router: &mut Router<T>,
    access_egress: &mut A,
    request: &TransitRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<PlanResponse<T::Walk>, PlanError<T::Error, A::Error>>
where
    T: TransferSearch,
    A: AccessEgressSearch,
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
    let latest_unix = request
        .departure
        .checked_add(i64::from(request.window_seconds))
        .ok_or(PlanError::InvalidRequest("the departure is out of range"))?;

    let snapshot = state.pin_for_departure(request.now, request.departure, latest_unix)?;

    let tables = access_egress
        .tables(
            &AccessEgressRequest {
                origin: request.origin,
                destination: request.destination,
                mode: request.mode,
                snapshot: &snapshot,
            },
            interrupt,
        )
        .map_err(PlanError::AccessEgress)?;

    let earliest = window_time(&snapshot, request.departure)?;
    let latest = window_time(&snapshot, latest_unix)?;

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

    Ok(build_response(&snapshot, request, profile, tables.direct))
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
