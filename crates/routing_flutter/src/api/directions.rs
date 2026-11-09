//! Ferrostar's routing adapter, for Dart: the request to send for a trip, and the routes in
//! the answer.
//!
//! The adapter doesn't do the sending. Dart posts the request to the server, and offline it
//! can hand the same body to [`super::router::OfflineRouter::route`]; either way the answer's
//! bytes come back here.

use ferrostar::models::{
    GeographicCoordinate, Route, RouteStep, UserLocation, Waypoint, WaypointKind,
};
use ferrostar::routing_adapters::error::ParsingError;
use ferrostar::routing_adapters::{RouteAdapter, RouteRequest, WellKnownRouteProvider};
use flutter_rust_bridge::frb;
use std::collections::HashMap;
use std::time::SystemTime;

/// How far off a trip's start is taken to be, in meters. Valhalla uses it to decide how sure
/// it can be of which side of the street the start is on.
const START_ACCURACY: f64 = 10.0;

/// A point on the ground, in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectionsPoint {
    pub lat: f64,
    pub lng: f64,
}

impl From<GeographicCoordinate> for DirectionsPoint {
    fn from(c: GeographicCoordinate) -> Self {
        Self {
            lat: c.lat,
            lng: c.lng,
        }
    }
}

impl From<DirectionsPoint> for GeographicCoordinate {
    fn from(p: DirectionsPoint) -> Self {
        Self {
            lat: p.lat,
            lng: p.lng,
        }
    }
}

/// One maneuver and the stretch of road after it.
#[derive(Clone, Debug, PartialEq)]
pub struct DirectionsStep {
    /// What to do, in words.
    pub instruction: String,
    /// How far this step goes, in meters.
    pub distance: f64,
    /// How long this step takes, in seconds.
    pub duration: f64,
}

impl From<RouteStep> for DirectionsStep {
    fn from(step: RouteStep) -> Self {
        Self {
            instruction: step.instruction,
            distance: step.distance,
            duration: step.duration,
        }
    }
}

/// A way to get there.
#[derive(Clone, Debug, PartialEq)]
pub struct DirectionsRoute {
    /// The line it follows.
    pub geometry: Vec<DirectionsPoint>,
    /// How far it goes, in meters.
    pub distance: f64,
    /// How long it takes, in seconds.
    pub duration: f64,
    pub steps: Vec<DirectionsStep>,
}

impl From<Route> for DirectionsRoute {
    fn from(route: Route) -> Self {
        Self {
            geometry: route.geometry.into_iter().map(Into::into).collect(),
            distance: route.distance,
            // Ferrostar's routes have no total of their own.
            duration: route.steps.iter().map(|step| step.duration).sum(),
            steps: route.steps.into_iter().map(Into::into).collect(),
        }
    }
}

/// An HTTP POST to make.
#[derive(Clone, Debug, PartialEq)]
pub struct DirectionsRequest {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// What kind of problem left no routes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DirectionsErrorKind {
    /// The router found no way between the two points, or no road near one of them.
    NoRoute,
    /// The request couldn't be made, or the answer wasn't understood.
    Failed,
}

/// Why there are no routes.
#[derive(Clone, Debug, PartialEq)]
pub struct DirectionsError {
    /// What kind of problem it is.
    pub kind: DirectionsErrorKind,
    /// A description for logs.
    pub message: String,
}

impl DirectionsError {
    fn failed(error: impl ToString) -> Self {
        Self {
            kind: DirectionsErrorKind::Failed,
            message: error.to_string(),
        }
    }
}

/// Answers from the router that mean there's no way to get there, as opposed to something
/// having gone wrong.
const NO_ROUTE_CODES: [&str; 3] = ["NoRoute", "NoSegment", "DistanceExceeded"];

impl From<ParsingError> for DirectionsError {
    fn from(error: ParsingError) -> Self {
        let kind = match &error {
            ParsingError::InvalidStatusCode { code, .. }
                if NO_ROUTE_CODES.contains(&code.as_str()) =>
            {
                DirectionsErrorKind::NoRoute
            }
            _ => DirectionsErrorKind::Failed,
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

/// Ferrostar's adapter for one way of traveling. Opaque to Dart.
#[frb(opaque)]
pub struct DirectionsAdapter {
    adapter: RouteAdapter,
}

impl DirectionsAdapter {
    /// An adapter for a Valhalla server's route endpoint and a costing such as `auto`,
    /// asking for up to `alternates` routes besides the best one.
    #[frb(sync)]
    pub fn valhalla(
        endpoint_url: String,
        costing: String,
        alternates: u32,
    ) -> Result<DirectionsAdapter, DirectionsError> {
        let adapter =
            RouteAdapter::from_well_known_route_provider(WellKnownRouteProvider::Valhalla {
                endpoint_url,
                profile: costing,
                options_json: Some(format!(r#"{{"alternates":{alternates}}}"#)),
            })
            .map_err(DirectionsError::failed)?;
        Ok(Self { adapter })
    }

    /// The request for routes from `from` to `to`.
    #[frb(sync)]
    pub fn request(
        &self,
        from: DirectionsPoint,
        to: DirectionsPoint,
    ) -> Result<DirectionsRequest, DirectionsError> {
        let start = UserLocation {
            coordinates: from.into(),
            horizontal_accuracy: START_ACCURACY,
            course_over_ground: None,
            timestamp: SystemTime::now(),
            speed: None,
        };
        let end = Waypoint {
            coordinate: to.into(),
            kind: WaypointKind::Break,
            properties: None,
        };
        let request = self
            .adapter
            .generate_request(start, vec![end])
            .map_err(DirectionsError::failed)?;
        match request {
            RouteRequest::HttpPost { url, headers, body } => {
                Ok(DirectionsRequest { url, headers, body })
            }
            RouteRequest::HttpGet { url, .. } => Err(DirectionsError::failed(format!(
                "expected a POST to {url}, not a GET"
            ))),
        }
    }

    /// The routes in an answer's body, the best first.
    ///
    /// Not `#[frb(sync)]`: an answer can be megabytes of JSON, so this runs off the UI
    /// thread. The status of the answer doesn't matter, since an error's body says what
    /// went wrong.
    pub fn parse(&self, body: Vec<u8>) -> Result<Vec<DirectionsRoute>, DirectionsError> {
        let routes = self.adapter.parse_response(body)?;
        Ok(routes.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter() -> DirectionsAdapter {
        DirectionsAdapter::valhalla("http://router/route".into(), "bicycle".into(), 2).unwrap()
    }

    const FROM: DirectionsPoint = DirectionsPoint {
        lat: 10.5,
        lng: 20.25,
    };
    const TO: DirectionsPoint = DirectionsPoint {
        lat: 11.75,
        lng: 21.5,
    };

    #[test]
    fn the_request_is_a_post_of_ferrostars_valhalla_body() {
        let request = adapter().request(FROM, TO).unwrap();
        assert_eq!(request.url, "http://router/route");
        assert_eq!(request.headers["Content-Type"], "application/json");
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["format"], "osrm");
        assert_eq!(body["costing"], "bicycle");
        assert_eq!(body["alternates"], 2);
        assert_eq!(body["locations"][0]["lat"], 10.5);
        assert_eq!(body["locations"][1]["lon"], 21.5);
        assert_eq!(body["locations"][1]["type"], "break");
    }

    #[test]
    fn no_way_there_is_told_apart_from_failure() {
        for code in NO_ROUTE_CODES {
            let body =
                format!(r#"{{"code":"{code}","message":"none","routes":[],"waypoints":[]}}"#);
            assert_eq!(
                adapter().parse(body.into_bytes()).unwrap_err().kind,
                DirectionsErrorKind::NoRoute
            );
        }
        let body = br#"{"code":"InvalidUrl","message":"bad","routes":[],"waypoints":[]}"#;
        assert_eq!(
            adapter().parse(body.to_vec()).unwrap_err().kind,
            DirectionsErrorKind::Failed
        );
    }

    #[test]
    fn an_answer_that_is_not_json_is_a_failure() {
        assert_eq!(
            adapter().parse(b"<html>".to_vec()).unwrap_err().kind,
            DirectionsErrorKind::Failed
        );
    }
}
