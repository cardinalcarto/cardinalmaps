//! JSON bytes in and out, so the server and the app answer requests identically.

use serde::Deserialize;
use serde_json::json;
use valinor_gtfs::LatLon;
use valinor_walk::WalkError;

use crate::error::{PlanError, StateError};
use crate::plan::{TransitOptions, TransitRequest, TravelMode};
use crate::response::{PlanResponse, language_supported};
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
    /// The narration language for walking and cycling legs (a BCP 47 tag); `en-US` when absent.
    #[serde(default)]
    pub language: Option<String>,
}

fn parse_body(body: &[u8], now: i64) -> Result<(TransitRequest, String), JsonResponse> {
    let parsed: PlanRequestBody = serde_json::from_slice(body)
        .map_err(|error| error_body(400, "invalid_request", error.to_string()))?;
    let mode = match parsed.mode.as_deref() {
        None | Some("walking") => TravelMode::Walking,
        Some("cycling") => TravelMode::Cycling,
        Some("wheelchair") => TravelMode::Wheelchair,
        Some(other) => {
            return Err(error_body(
                400,
                "invalid_request",
                format!("unknown mode {other:?}"),
            ));
        }
    };
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
    };
    Ok((request, language))
}

impl StreetPlanner {
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

fn ok_body(response: &PlanResponse) -> JsonResponse {
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
        PlanError::AccessEgress(StreetError::NoStreetNearby(which)) => error_body(
            400,
            "no_street_nearby",
            format!("no street near the {which}"),
        ),
        other => error_body(500, "internal", other.to_string()),
    }
}
