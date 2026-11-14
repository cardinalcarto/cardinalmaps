//! JSON bytes in and out, so the server and the app answer requests identically.

use serde::{Deserialize, Serialize};
use serde_json::json;
use valinor_gtfs::LatLon;
use valinor_walk::WalkError;

use crate::error::{PlanError, StateError};
use crate::nearby::{
    DEFAULT_HORIZON_SECONDS, DEFAULT_NEARBY_DEPARTURES, DirectionKey, NearbyRequest,
    RouteDetailRequest, TripChoice,
};
use crate::plan::{DepartureRange, TransitOptions, TransitRequest, TravelMode};
use crate::response::language_supported;
use crate::street::{StreetError, StreetPlanner};

/// What the server or bridge sends back: a status and a JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonResponse {
    /// The HTTP status a server would send.
    pub status: u16,
    /// The JSON body: a response, or `{"error": ..., "kind": ...}`.
    pub body: Vec<u8>,
}

fn error_body(status: u16, kind: &str, message: impl Into<String>) -> JsonResponse {
    let body = json!({ "error": message.into(), "kind": kind });
    JsonResponse {
        status,
        body: serde_json::to_vec(&body).unwrap_or_default(),
    }
}

/// A coordinate in a request body.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct JsonCoordinate {
    /// Degrees north.
    pub lat: f64,
    /// Degrees east.
    pub lon: f64,
}

/// A transit planning request, as the server and the bridge take it.
#[derive(Debug, Clone, Deserialize)]
pub struct PlanRequestBody {
    /// Where the rider starts.
    pub origin: JsonCoordinate,
    /// Where the rider is going.
    pub destination: JsonCoordinate,
    /// The start of the departure window, in POSIX seconds; `now` when absent.
    #[serde(default)]
    pub departure: Option<i64>,
    /// The departure window's length in seconds; 60 minutes when absent.
    #[serde(default)]
    pub window_seconds: Option<u32>,
    /// `walking` (the default) or `cycling`.
    #[serde(default)]
    pub mode: Option<String>,
    /// The most trips a journey may use.
    #[serde(default)]
    pub max_trips: Option<u8>,
    /// The longest access or egress walk, in seconds.
    #[serde(default)]
    pub max_walk_seconds: Option<u32>,
    /// How many departures before its own each ride lists; two when absent.
    #[serde(default)]
    pub departures_before: Option<u8>,
    /// How many departures after its own each ride lists; three when absent.
    #[serde(default)]
    pub departures_after: Option<u8>,
    /// The narration language for walking and cycling legs (a BCP 47 tag); `en-US` when absent.
    #[serde(default)]
    pub language: Option<String>,
}

/// [`parse_body`] for tests, which expect the body to parse.
#[cfg(test)]
pub(crate) fn parse_body_for_test(body: &[u8], now: i64) -> (TransitRequest, String) {
    match parse_body(body, now) {
        Ok(parsed) => parsed,
        Err(response) => panic!(
            "the body doesn't parse: {}",
            String::from_utf8_lossy(&response.body)
        ),
    }
}

/// A request's travel mode, `walking` when absent.
fn parse_mode(mode: Option<&str>) -> Result<TravelMode, JsonResponse> {
    match mode {
        None | Some("walking") => Ok(TravelMode::Walking),
        Some("cycling") => Ok(TravelMode::Cycling),
        Some("wheelchair") => Ok(TravelMode::Wheelchair),
        Some(other) => Err(error_body(
            400,
            "invalid_request",
            format!("unknown mode {other:?}"),
        )),
    }
}

fn parse_json<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, JsonResponse> {
    serde_json::from_slice(body)
        .map_err(|error| error_body(400, "invalid_request", error.to_string()))
}

fn parse_body(body: &[u8], now: i64) -> Result<(TransitRequest, String), JsonResponse> {
    let parsed: PlanRequestBody = parse_json(body)?;
    let mode = parse_mode(parsed.mode.as_deref())?;
    let language = parsed
        .language
        .clone()
        .unwrap_or_else(|| "en-US".to_owned());
    if !language_supported(&language) {
        return Err(error_body(
            400,
            "invalid_request",
            format!("unsupported language {language:?}"),
        ));
    }
    let mut request = TransitRequest::new(
        LatLon {
            lat: parsed.origin.lat,
            lon: parsed.origin.lon,
        },
        LatLon {
            lat: parsed.destination.lat,
            lon: parsed.destination.lon,
        },
        parsed.departure.unwrap_or(now),
        now,
    );
    request.mode = mode;
    if let Some(seconds) = parsed.window_seconds {
        request.window_seconds = seconds;
    }
    let defaults = TransitOptions::default();
    request.options = TransitOptions {
        max_trips: parsed.max_trips.unwrap_or(defaults.max_trips),
        max_walk_seconds: parsed.max_walk_seconds.unwrap_or(defaults.max_walk_seconds),
        departures: DepartureRange {
            before: parsed
                .departures_before
                .unwrap_or(defaults.departures.before),
            after: parsed.departures_after.unwrap_or(defaults.departures.after),
            ..defaults.departures
        },
    };
    Ok((request, language))
}

/// A nearby-departures request, as the server and the bridge take it.
#[derive(Debug, Clone, Deserialize)]
pub struct NearbyRequestBody {
    /// Where the rider is.
    pub location: JsonCoordinate,
    /// When the rider sets off, in POSIX seconds; `now` when absent.
    #[serde(default)]
    pub time: Option<i64>,
    /// `walking` (the default) or `wheelchair`.
    #[serde(default)]
    pub mode: Option<String>,
    /// The longest walk to a stop, in seconds; 30 minutes when absent.
    #[serde(default)]
    pub max_walk_seconds: Option<u32>,
    /// How far ahead departures are listed, in seconds; two hours when absent.
    #[serde(default)]
    pub horizon_seconds: Option<u32>,
    /// How many departures that run each direction lists (canceled and skipped ones among
    /// them don't count), at most ten; three when absent.
    #[serde(default)]
    pub max_departures: Option<u8>,
    /// How many routes to list, soonest first; all when absent.
    #[serde(default)]
    pub max_routes: Option<u32>,
}

/// A route-detail request, as the server and the bridge take it,
/// by the identifiers a nearby response gave.
#[derive(Debug, Clone, Deserialize)]
pub struct RouteRequestBody {
    /// The route's feed.
    pub feed: String,
    /// `route_id`.
    pub route_id: String,
    /// The direction's `direction_id`; give this or `terminus_stop_id`, as the direction did.
    #[serde(default)]
    pub direction_id: Option<u32>,
    /// The direction's `terminus_stop_id`, for trips without a `direction_id`.
    #[serde(default)]
    pub terminus_stop_id: Option<String>,
    /// The stop's `stop_id`, in the route's feed.
    pub stop_id: String,
    /// When the rider sets off, in POSIX seconds; `now` when absent.
    #[serde(default)]
    pub time: Option<i64>,
    /// `walking` (the default) or `wheelchair`.
    #[serde(default)]
    pub mode: Option<String>,
    /// The walk to the stop, in seconds; zero when absent.
    #[serde(default)]
    pub walk_seconds: Option<u32>,
    /// How far ahead departures are listed, in seconds; two hours when absent.
    #[serde(default)]
    pub horizon_seconds: Option<u32>,
    /// How many departures that run to list (canceled and skipped ones among them don't
    /// count), at most ten; three when absent.
    #[serde(default)]
    pub max_departures: Option<u8>,
    /// The trip whose stops to show, by a departure's `trip_id`; the next that runs when absent.
    #[serde(default)]
    pub trip_id: Option<String>,
    /// That departure's `start_date`, which tells a trip's runs on two service days apart.
    #[serde(default)]
    pub start_date: Option<String>,
}

fn parse_nearby(body: &[u8], now: i64) -> Result<NearbyRequest, JsonResponse> {
    let parsed: NearbyRequestBody = parse_json(body)?;
    let mut request = NearbyRequest::new(
        LatLon {
            lat: parsed.location.lat,
            lon: parsed.location.lon,
        },
        parsed.time.unwrap_or(now),
        now,
    );
    request.mode = parse_mode(parsed.mode.as_deref())?;
    request.max_walk_seconds = parsed.max_walk_seconds.unwrap_or(request.max_walk_seconds);
    request.horizon_seconds = parsed.horizon_seconds.unwrap_or(request.horizon_seconds);
    request.max_departures = parsed.max_departures.unwrap_or(request.max_departures);
    request.max_routes = parsed.max_routes;
    Ok(request)
}

fn parse_route(body: &[u8], now: i64) -> Result<RouteDetailRequest, JsonResponse> {
    let parsed: RouteRequestBody = parse_json(body)?;
    let direction = match (parsed.direction_id, parsed.terminus_stop_id) {
        (Some(id), None) => DirectionKey::Id(id),
        (None, Some(stop)) => DirectionKey::Terminus(stop),
        _ => {
            return Err(error_body(
                400,
                "invalid_request",
                "give exactly one of direction_id and terminus_stop_id",
            ));
        }
    };
    let trip = match (parsed.trip_id, parsed.start_date) {
        (Some(trip_id), start_date) => Some(TripChoice {
            trip_id,
            start_date,
        }),
        (None, None) => None,
        (None, Some(_)) => {
            return Err(error_body(
                400,
                "invalid_request",
                "a start_date needs the trip_id it is of",
            ));
        }
    };
    Ok(RouteDetailRequest {
        feed: parsed.feed,
        route_id: parsed.route_id,
        direction,
        stop_id: parsed.stop_id,
        time: parsed.time.unwrap_or(now),
        now,
        mode: parse_mode(parsed.mode.as_deref())?,
        walk_seconds: parsed.walk_seconds.unwrap_or(0),
        horizon_seconds: parsed.horizon_seconds.unwrap_or(DEFAULT_HORIZON_SECONDS),
        max_departures: parsed.max_departures.unwrap_or(DEFAULT_NEARBY_DEPARTURES),
        trip,
    })
}

impl StreetPlanner {
    /// Answers a JSON nearby-departures request, as the HTTP endpoint and the app's bridge do.
    pub fn nearby_json(&self, body: &[u8], now: i64, interrupt: &dyn Fn() -> bool) -> JsonResponse {
        let request = match parse_nearby(body, now) {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
        match self.nearby(&request, interrupt) {
            Ok(response) => ok_body(&response),
            Err(error) => plan_error(&error),
        }
    }

    /// Answers a JSON route-detail request, as the HTTP endpoint and the app's bridge do.
    pub fn route_json(&self, body: &[u8], now: i64, interrupt: &dyn Fn() -> bool) -> JsonResponse {
        let request = match parse_route(body, now) {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
        match self.route_detail(&request, interrupt) {
            Ok(response) => ok_body(&response),
            Err(error) => plan_error(&error),
        }
    }

    /// Answers a JSON planning request, as the HTTP endpoint and the app's bridge do.
    pub fn plan_json(&self, body: &[u8], now: i64, interrupt: &dyn Fn() -> bool) -> JsonResponse {
        let (request, language) = match parse_body(body, now) {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
        match self.plan(&request, &language, interrupt) {
            Ok(response) => ok_body(&response),
            Err(error) => plan_error(&error),
        }
    }

    /// Answers a vehicle lookup as JSON.
    pub fn vehicle_json(&self, feed: &str, id: &str, now: i64) -> JsonResponse {
        match self.vehicle(now, feed, id) {
            Ok(update) => match serde_json::to_vec(&update) {
                Ok(body) => JsonResponse { status: 200, body },
                Err(error) => error_body(500, "internal", error.to_string()),
            },
            Err(error) => state_error(&error),
        }
    }
}

fn ok_body<T: Serialize>(response: &T) -> JsonResponse {
    match serde_json::to_vec(response) {
        Ok(body) => JsonResponse { status: 200, body },
        Err(error) => error_body(500, "internal", error.to_string()),
    }
}

fn state_error(error: &StateError) -> JsonResponse {
    match error {
        StateError::DateNotCovered(_) => error_body(404, "date_not_covered", error.to_string()),
        StateError::UnknownFeed(_) => error_body(404, "unknown_feed", error.to_string()),
        other => error_body(500, "internal", other.to_string()),
    }
}

fn plan_error(error: &PlanError<WalkError, StreetError>) -> JsonResponse {
    match error {
        PlanError::InvalidRequest(reason) => error_body(400, "invalid_request", *reason),
        PlanError::Interrupted => error_body(499, "interrupted", error.to_string()),
        PlanError::State(state) => state_error(state),
        PlanError::NotFound(missing) => error_body(
            404,
            &format!("unknown_{}", missing.name()),
            error.to_string(),
        ),
        PlanError::AccessEgress(StreetError::NoStreetNearby(which)) => error_body(
            400,
            "no_street_nearby",
            format!("no street near the {which}"),
        ),
        other => error_body(500, "internal", other.to_string()),
    }
}
