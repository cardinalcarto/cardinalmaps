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
use serde_json::json;
use std::collections::HashMap;
use std::time::SystemTime;

/// How far off a trip's start is taken to be, in meters. Valhalla uses it to decide how sure
/// it can be of which side of the street the start is on.
const START_ACCURACY: f64 = 10.0;

/// Valhalla's costing for going on foot, and by wheelchair.
pub(crate) const PEDESTRIAN: &str = "pedestrian";

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

/// A route as Ferrostar parsed it, whole: what navigation follows. Opaque to Dart.
///
/// [`DirectionsRoute`] summarizes it for the directions sheet; navigation starts from this,
/// so it follows exactly the route the sheet showed.
#[frb(opaque)]
#[derive(Clone, Debug)]
pub struct NavigableRoute {
    pub(crate) route: Route,
    /// The locale its instructions are in, as [`valhalla_language`] resolved the one asked
    /// for: what the announcements are spoken in.
    pub(crate) language: &'static str,
}

impl NavigableRoute {
    /// The locale the route's banners and announcements are in, a BCP-47 tag such as
    /// `pt-PT`: the nearest Valhalla has to the language asked for, which may not be it, so
    /// the voice that speaks them is chosen from this rather than from the device's.
    ///
    /// `#[frb(sync)]`: it's a field.
    #[frb(sync)]
    pub fn language(&self) -> String {
        self.language.to_owned()
    }
}

/// A way to get there.
///
/// Not `PartialEq`: Ferrostar's [`Route`] isn't, and Dart compares routes by identity.
/// `non_opaque`, since flutter_rust_bridge would otherwise make it opaque for its opaque
/// field, and Dart reads the rest of it as plain data.
#[frb(non_opaque)]
#[derive(Clone, Debug)]
pub struct DirectionsRoute {
    /// The line it follows.
    pub geometry: Vec<DirectionsPoint>,
    /// How far it goes, in meters.
    pub distance: f64,
    /// How long it takes, in seconds.
    pub duration: f64,
    pub steps: Vec<DirectionsStep>,
    /// The whole route, to navigate.
    pub navigable: NavigableRoute,
}

impl DirectionsRoute {
    /// `route`, its instructions in `language`.
    fn new(route: Route, language: &'static str) -> Self {
        Self {
            geometry: route.geometry.iter().copied().map(Into::into).collect(),
            distance: route.distance,
            // Ferrostar's routes have no total of their own.
            duration: route.steps.iter().map(|step| step.duration).sum(),
            steps: route.steps.iter().cloned().map(Into::into).collect(),
            navigable: NavigableRoute { route, language },
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
    pub(crate) fn failed(error: impl ToString) -> Self {
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

/// The units a route's instructions give distances in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DistanceUnits {
    Kilometers,
    Miles,
}

impl DistanceUnits {
    /// Valhalla's name for them.
    fn valhalla(self) -> &'static str {
        match self {
            DistanceUnits::Kilometers => "kilometers",
            DistanceUnits::Miles => "miles",
        }
    }
}

/// The language Valhalla has instructions in that's nearest `tag` (a BCP-47 tag such as
/// `es-MX`): the tag's own locale if it has one, else its language's (`es-ES`), else `en-US`.
///
/// Valhalla matches a request's language exactly against its locales and their aliases,
/// and answers any other in `en-US` without saying so, so `es-MX` as it stands would get
/// English. The locales are Ilúvatar's, which are the router's own.
fn valhalla_language(tag: &str) -> &'static str {
    let language = tag.split(['-', '_']).next().unwrap_or(tag);
    illuvatar::locale_tag(tag)
        .or_else(|| illuvatar::locale_tag(language))
        .unwrap_or("en-US")
}

/// Ferrostar's adapter for one way of traveling. Opaque to Dart.
#[frb(opaque)]
pub struct DirectionsAdapter {
    adapter: RouteAdapter,
    /// The locale asked for, which the routes it reads are in.
    language: &'static str,
}

impl DirectionsAdapter {
    /// An adapter for a Valhalla server's route endpoint and a costing such as `auto`,
    /// asking for up to `alternates` routes besides the best one, with instructions in
    /// `language` (a BCP-47 tag such as `pt-PT`, as [`valhalla_language`] resolves it) and
    /// distances in `units`. With `wheelchair`, the routes are ones a wheelchair can take:
    /// Valhalla's `pedestrian` costing of type `wheelchair`, which is the only costing it
    /// goes with.
    #[frb(sync)]
    pub fn valhalla(
        endpoint_url: String,
        costing: String,
        alternates: u32,
        language: String,
        units: DistanceUnits,
        wheelchair: bool,
    ) -> Result<DirectionsAdapter, DirectionsError> {
        let language = valhalla_language(&language);
        // Ferrostar sets each of these keys at the top of the request, over its own.
        let mut options = json!({
            "alternates": alternates,
            "language": language,
            "units": units.valhalla(),
        });
        if wheelchair {
            if costing != PEDESTRIAN {
                return Err(DirectionsError::failed(format!(
                    "a wheelchair route needs {PEDESTRIAN} costing, not {costing}"
                )));
            }
            options["costing_options"] = json!({ (PEDESTRIAN): { "type": "wheelchair" } });
        }
        let adapter =
            RouteAdapter::from_well_known_route_provider(WellKnownRouteProvider::Valhalla {
                endpoint_url,
                profile: costing,
                options_json: Some(options.to_string()),
            })
            .map_err(DirectionsError::failed)?;
        Ok(Self { adapter, language })
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
        self.request_through(start, vec![end])
    }

    /// The request for a route from `start` through `waypoints`, the last its end.
    pub(crate) fn request_through(
        &self,
        start: UserLocation,
        waypoints: Vec<Waypoint>,
    ) -> Result<DirectionsRequest, DirectionsError> {
        let request = self
            .adapter
            .generate_request(start, waypoints)
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
        Ok(routes
            .into_iter()
            .map(|route| DirectionsRoute::new(route, self.language))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrostar::models::ManeuverType;

    fn adapter() -> DirectionsAdapter {
        DirectionsAdapter::valhalla(
            "http://router/route".into(),
            "bicycle".into(),
            2,
            "es-MX".into(),
            DistanceUnits::Miles,
            false,
        )
        .unwrap()
    }

    fn walking(wheelchair: bool) -> Result<DirectionsAdapter, DirectionsError> {
        DirectionsAdapter::valhalla(
            "http://router/route".into(),
            "pedestrian".into(),
            3,
            "en-US".into(),
            DistanceUnits::Kilometers,
            wheelchair,
        )
    }

    fn body(adapter: DirectionsAdapter) -> serde_json::Value {
        serde_json::from_slice(&adapter.request(FROM, TO).unwrap().body).unwrap()
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
        assert_eq!(body["language"], "es-ES");
        assert_eq!(body["units"], "miles");
        assert_eq!(body["locations"][0]["lat"], 10.5);
        assert_eq!(body["locations"][1]["lon"], 21.5);
        assert_eq!(body["locations"][1]["type"], "break");
    }

    #[test]
    fn a_language_falls_back_to_its_nearest_locale() {
        for (tag, expected) in [
            ("es-MX", "es-ES"),
            ("de-AT", "de-DE"),
            ("pt-BR", "pt-BR"),
            ("pt", "pt-PT"),
            ("en-GB", "en-GB"),
            ("en-CA", "en-US"),
            ("zh-CN", "en-US"),
            ("", "en-US"),
        ] {
            assert_eq!(valhalla_language(tag), expected, "{tag}");
        }
    }

    #[test]
    fn a_wheelchair_route_is_asked_for_with_pedestrian_costing_of_that_type() {
        let body = body(walking(true).unwrap());
        assert_eq!(body["costing"], "pedestrian");
        assert_eq!(body["costing_options"]["pedestrian"]["type"], "wheelchair");
        assert_eq!(body["alternates"], 3);
        assert_eq!(body["language"], "en-US");
        assert_eq!(body["units"], "kilometers");
        // Ferrostar's own keys are still there.
        assert_eq!(body["format"], "osrm");
        assert_eq!(body["voice_instructions"], true);
    }

    #[test]
    fn a_walking_route_has_no_costing_options() {
        let body = body(walking(false).unwrap());
        assert_eq!(body["costing"], "pedestrian");
        assert!(body["costing_options"].is_null());
    }

    #[test]
    fn a_wheelchair_is_not_a_kind_of_other_costing() {
        let made = DirectionsAdapter::valhalla(
            "http://router/route".into(),
            "auto".into(),
            3,
            "en-US".into(),
            DistanceUnits::Kilometers,
            true,
        );
        assert_eq!(made.err().unwrap().kind, DirectionsErrorKind::Failed);
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

    /// What Ferrostar parses from the offline router's answer to the request its own
    /// generator makes, through the adapter the app makes for `costing`, `language` and
    /// `units`.
    fn ferrostar_routes(
        router: &crate::api::router::OfflineRouter,
        costing: &str,
        language: &str,
        units: DistanceUnits,
        from: DirectionsPoint,
        to: DirectionsPoint,
    ) -> Vec<Route> {
        let adapter = DirectionsAdapter::valhalla(
            "http://router/route".into(),
            costing.into(),
            0,
            language.into(),
            units,
            false,
        )
        .unwrap();
        let request = adapter.request(from, to).unwrap();
        let response = router.route(request.body);
        let body = String::from_utf8(response.body).unwrap();
        assert_eq!(
            response.status, 200,
            "{costing} {language} {units:?}: {body}"
        );
        adapter.adapter.parse_response(body.into_bytes()).unwrap()
    }

    /// Checks what turn-by-turn guidance needs of each step, and returns every spoken text.
    ///
    /// Every step but the arrival has banners and announcements (the arrival, which has no
    /// length, has neither, as in mainline), and each triggers within its step,
    /// farthest from the maneuver first.
    fn check_guidance(routes: &[Route], context: &str) -> Vec<String> {
        assert!(!routes.is_empty(), "{context}: no routes");
        let mut spoken = Vec::new();
        for route in routes {
            let (arrival, steps) = route.steps.split_last().unwrap();
            assert!(!steps.is_empty(), "{context}: only an arrival");
            assert!(arrival.visual_instructions.is_empty(), "{context}");
            assert!(arrival.spoken_instructions.is_empty(), "{context}");
            for (i, step) in steps.iter().enumerate() {
                let context = format!("{context}, step {i} ({})", step.instruction);
                assert!(
                    !step.visual_instructions.is_empty(),
                    "{context}: no banners"
                );
                assert!(
                    !step.spoken_instructions.is_empty(),
                    "{context}: no announcements"
                );
                // Announcement distances are written with one decimal, so they may round up
                // past the step's length by half a decimeter.
                let within = |distance: f64| (0.0..=step.distance + 0.05).contains(&distance);
                let mut previous = f64::INFINITY;
                for banner in &step.visual_instructions {
                    let primary = &banner.primary_content;
                    assert!(!primary.text.is_empty(), "{context}: empty banner");
                    assert!(
                        primary.maneuver_type.is_some(),
                        "{context}: no maneuver type"
                    );
                    let distance = banner.trigger_distance_before_maneuver;
                    assert!(within(distance), "{context}: banner at {distance}");
                    assert!(distance <= previous, "{context}: banners out of order");
                    previous = distance;
                }
                previous = f64::INFINITY;
                for announcement in &step.spoken_instructions {
                    assert!(
                        !announcement.text.is_empty(),
                        "{context}: empty announcement"
                    );
                    assert_eq!(
                        announcement.ssml.as_deref(),
                        Some(format!("<speak>{}</speak>", announcement.text).as_str()),
                        "{context}"
                    );
                    let distance = announcement.trigger_distance_before_maneuver;
                    assert!(
                        within(distance),
                        "{context}: announcement at {distance} of {}",
                        step.distance
                    );
                    assert!(
                        distance <= previous,
                        "{context}: announcements out of order"
                    );
                    previous = distance;
                    spoken.push(announcement.text.clone());
                }
            }
            // The departure is announced as the trip starts.
            let first = &steps[0];
            let departure = first.spoken_instructions[0].trigger_distance_before_maneuver;
            assert!(
                (departure - first.distance).abs() <= 0.05,
                "{context}: departure announced at {departure} of {}",
                first.distance
            );
        }
        spoken
    }

    /// Whether any announcement says one of `words`.
    fn says(spoken: &[String], words: &[&str]) -> bool {
        spoken
            .iter()
            .any(|text| words.iter().any(|word| text.contains(word)))
    }

    /// Ferrostar's own request and parser around the offline router, for each costing,
    /// in English and kilometers, in German and miles, and in a language Valhalla doesn't
    /// have.
    fn check_ferrostar_guidance(
        router: &crate::api::router::OfflineRouter,
        trips: &[(&str, DirectionsPoint, DirectionsPoint)],
    ) {
        use DistanceUnits::{Kilometers, Miles};
        for &(costing, from, to) in trips {
            let english = check_guidance(
                &ferrostar_routes(router, costing, "en-US", Kilometers, from, to),
                &format!("{costing} en-US kilometers"),
            );
            assert!(says(&english, &["destination"]), "{costing}: {english:?}");
            assert!(says(&english, &[" meters", "kilometer"]), "{costing}");
            assert!(!says(&english, &[" feet", " mile"]), "{costing}");

            let german = check_guidance(
                &ferrostar_routes(router, costing, "de-DE", Miles, from, to),
                &format!("{costing} de-DE miles"),
            );
            assert!(says(&german, &["Ziel"]), "{costing}: {german:?}");
            assert!(says(&german, &["Fuß", "Meile"]), "{costing}");
            assert!(!says(&german, &[" Meter", "Kilometer"]), "{costing}");

            // A language Valhalla doesn't have is asked for in en-US (`valhalla_language`),
            // as mainline would answer it anyway.
            let fallback = check_guidance(
                &ferrostar_routes(router, costing, "xx-XX", Kilometers, from, to),
                &format!("{costing} xx-XX kilometers"),
            );
            assert_eq!(fallback, english, "{costing}");
        }
    }

    fn point(lat: f64, lng: f64) -> DirectionsPoint {
        DirectionsPoint { lat, lng }
    }

    #[test]
    fn ferrostar_parses_guidance_for_every_step() {
        let tiles = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../valinor/valhalla-graphtile/fixtures/andorra-tiles.tar");
        let router =
            crate::api::router::OfflineRouter::open(vec![tiles.to_string_lossy().into()], None)
                .ok()
                .unwrap();
        let start = point(42.5078, 1.5211);
        check_ferrostar_guidance(
            &router,
            &[
                ("auto", start, point(42.5355, 1.583)),
                ("pedestrian", start, point(42.5099, 1.5357)),
                ("bicycle", start, point(42.5562, 1.5331)),
            ],
        );

        // A roundabout's banner says which way its exit lies.
        let routes = ferrostar_routes(
            &router,
            "auto",
            "en-US",
            DistanceUnits::Kilometers,
            point(42.505, 1.52),
            point(42.505, 1.515),
        );
        check_guidance(&routes, "roundabout");
        let roundabouts: Vec<_> = routes[0]
            .steps
            .iter()
            .flat_map(|step| &step.visual_instructions)
            .filter(|banner| banner.primary_content.maneuver_type == Some(ManeuverType::Roundabout))
            .collect();
        assert!(!roundabouts.is_empty());
        for banner in roundabouts {
            assert!(banner.primary_content.roundabout_exit_degrees.is_some());
        }
    }

    /// The same over Seattle, with lanes and exit numbers on the highway,
    /// from `CARDINAL_SEATTLE_TILES` (street tiles covering Seattle; real data,
    /// so not in the repository).
    #[test]
    #[ignore = "needs CARDINAL_SEATTLE_TILES"]
    fn ferrostar_parses_guidance_for_every_step_in_seattle() {
        let tiles = std::env::var("CARDINAL_SEATTLE_TILES").expect("CARDINAL_SEATTLE_TILES");
        let router = crate::api::router::OfflineRouter::open(vec![tiles], None)
            .ok()
            .unwrap();
        check_ferrostar_guidance(
            &router,
            &[
                ("auto", point(47.7062, -122.3255), point(47.4502, -122.3088)),
                (
                    "pedestrian",
                    point(47.6094, -122.3422),
                    point(47.6205, -122.3493),
                ),
                (
                    "bicycle",
                    point(47.6510, -122.3500),
                    point(47.6553, -122.3035),
                ),
            ],
        );

        let routes = ferrostar_routes(
            &router,
            "auto",
            "en-US",
            DistanceUnits::Kilometers,
            point(47.7062, -122.3255),
            point(47.4502, -122.3088),
        );
        let banners: Vec<_> = routes[0]
            .steps
            .iter()
            .flat_map(|step| &step.visual_instructions)
            .collect();
        assert!(
            banners
                .iter()
                .any(|banner| !banner.primary_content.exit_numbers.is_empty())
        );
        assert!(banners.iter().any(|banner| {
            banner
                .sub_content
                .as_ref()
                .is_some_and(|sub| sub.lane_info.is_some())
        }));
    }
}
