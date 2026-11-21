//! Ferrostar's routing adapter, for Dart: the request to send for a trip, and the routes in
//! the answer.
//!
//! The adapter doesn't do the sending. Dart posts the request to the server, and offline it
//! can hand the same body to [`super::router::OfflineRouter::route`]; either way the answer's
//! bytes come back here.

use crate::route_details::{
    ProfileIntersection, ProfileLeg, RoadClasses, StepRoad, road_name, route_names, speed_profile,
};
use ferrostar::models::{
    GeographicCoordinate, Route, RouteStep, UserLocation, Waypoint, WaypointKind,
};
use ferrostar::routing_adapters::error::ParsingError;
use ferrostar::routing_adapters::{RouteAdapter, RouteRequest, WellKnownRouteProvider};
use flutter_rust_bridge::frb;
use serde::Deserialize;
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
    /// What a driver calls the road this step is on: its route number if it has one
    /// ("I-5 North", "SR 99"), else the street's name. None if the road has neither, or
    /// the answer didn't say.
    pub road_name: Option<String>,
}

impl DirectionsStep {
    /// `step`, on the road called `road_name`.
    fn new(step: RouteStep, road_name: Option<String>) -> Self {
        Self {
            instruction: step.instruction,
            distance: step.distance,
            duration: step.duration,
            road_name,
        }
    }
}

/// A stretch of a route that's all one kind of road at about one speed. A route's runs
/// follow one another from its start to its end, and make its speed profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectionsRun {
    /// How long it takes, in seconds, with the turns along it.
    pub duration: f64,
    /// How far it goes, in meters.
    pub distance: f64,
    /// How fast the router expects it to go, in meters per second: its distance over its
    /// duration. Zero for a run that takes no time.
    pub speed: f64,
    /// Whether it's on a motorway.
    pub motorway: bool,
    /// Whether it's tolled.
    pub toll: bool,
    /// Whether it's a ferry crossing.
    pub ferry: bool,
}

/// The kind of sign a country posts its speed limits on, as Valhalla says for each step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedLimitSign {
    /// The United States' (and a few others'): a white rectangle reading "Speed limit"
    /// over the number.
    Mutcd,
    /// The Vienna Convention's, as in most of the world: a red ring around the number.
    Vienna,
}

/// The unit a country's speed-limit signs are in, as Valhalla says for each step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedLimitUnit {
    KilometersPerHour,
    MilesPerHour,
}

/// How a step's speed limits are signed where it is: what Valhalla's OSRM answer gives
/// each step as `speedLimitSign` and `speedLimitUnit`, which Ferrostar's parser drops.
/// Dart has them on each `NavStep`.
#[frb(ignore)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StepSigns {
    pub(crate) sign: Option<SpeedLimitSign>,
    pub(crate) unit: Option<SpeedLimitUnit>,
}

/// The little of an OSRM answer that Ferrostar's parser leaves out and the app wants:
/// each step's speed-limit sign and unit, its road and the classes of its intersections,
/// and each leg's annotation. Everything else in the answer is skipped.
#[derive(Deserialize)]
struct OsrmAnswer {
    #[serde(default)]
    routes: Vec<OsrmRoute>,
}

#[derive(Deserialize)]
struct OsrmRoute {
    #[serde(default)]
    legs: Vec<OsrmLeg>,
}

#[derive(Deserialize)]
struct OsrmLeg {
    #[serde(default)]
    steps: Vec<OsrmStep>,
    /// The countries the leg passes through, which its intersections refer to.
    #[serde(default)]
    admins: Vec<OsrmAdmin>,
    annotation: Option<OsrmAnnotation>,
}

#[derive(Deserialize)]
struct OsrmAdmin {
    iso_3166_1: Option<String>,
}

/// A leg's `annotation`: a value for each segment of its line.
#[derive(Deserialize)]
struct OsrmAnnotation {
    /// Seconds, without the time turns take.
    #[serde(default)]
    duration: Vec<f64>,
    /// Meters.
    #[serde(default)]
    distance: Vec<f64>,
}

#[derive(Deserialize)]
struct OsrmStep {
    /// The street's names, `; `-separated; empty if it has none.
    #[serde(default)]
    name: String,
    /// The road's route numbers, `; `-separated.
    #[serde(rename = "ref")]
    refs: Option<String>,
    /// Meters.
    #[serde(default)]
    distance: f64,
    #[serde(default)]
    intersections: Vec<OsrmIntersection>,
    #[serde(rename = "speedLimitSign")]
    sign: Option<String>,
    #[serde(rename = "speedLimitUnit")]
    unit: Option<String>,
}

#[derive(Deserialize)]
struct OsrmIntersection {
    /// The index of its point in the leg's line.
    geometry_index: Option<usize>,
    /// The index of its country in the leg's `admins`.
    admin_index: Option<usize>,
    /// What the road leaving it is: `toll`, `motorway`, `ferry` and others.
    #[serde(default)]
    classes: Vec<String>,
    /// Seconds the turn onto the road leaving it takes.
    #[serde(default)]
    turn_duration: f64,
}

impl From<&OsrmStep> for StepSigns {
    fn from(step: &OsrmStep) -> Self {
        Self {
            sign: match step.sign.as_deref() {
                Some("mutcd") => Some(SpeedLimitSign::Mutcd),
                Some("vienna") => Some(SpeedLimitSign::Vienna),
                _ => None,
            },
            unit: match step.unit.as_deref() {
                Some("km/h") => Some(SpeedLimitUnit::KilometersPerHour),
                Some("mph") => Some(SpeedLimitUnit::MilesPerHour),
                _ => None,
            },
        }
    }
}

impl OsrmLeg {
    /// What a driver calls the road `step` of this leg is on ([`road_name`]), written
    /// as it is in the country the step starts in.
    fn road_name(&self, step: &OsrmStep) -> Option<String> {
        let country = step
            .intersections
            .first()
            .and_then(|intersection| self.admins.get(intersection.admin_index?))
            .and_then(|admin| admin.iso_3166_1.as_deref());
        road_name(&step.name, step.refs.as_deref(), country == Some("US"))
    }

    /// The leg as its speed profile needs it. A leg with an intersection that doesn't say
    /// where on the line it is can't have its annotation matched to its roads, so it's
    /// given as one with no annotation.
    fn profile(&self) -> ProfileLeg {
        let intersections: Option<Vec<ProfileIntersection>> = self
            .steps
            .iter()
            .flat_map(|step| &step.intersections)
            .map(|intersection| {
                Some(ProfileIntersection {
                    geometry_index: intersection.geometry_index?,
                    classes: RoadClasses::of(&intersection.classes),
                    turn_duration: intersection.turn_duration,
                })
            })
            .collect();
        match (&self.annotation, intersections) {
            (Some(annotation), Some(intersections)) => ProfileLeg {
                durations: annotation.duration.clone(),
                distances: annotation.distance.clone(),
                intersections,
            },
            _ => ProfileLeg::default(),
        }
    }
}

/// What the answer says of one step that Ferrostar's parser drops.
#[frb(ignore)]
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StepDetails {
    pub(crate) signs: StepSigns,
    /// What a driver calls the road it's on, if anything.
    pub(crate) road_name: Option<String>,
}

/// What the answer says of one route that Ferrostar's parser drops, and what's worked
/// out from that.
#[frb(ignore)]
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RouteDetails {
    /// Step by step as Ferrostar lays a route's steps out: every leg's, in order.
    pub(crate) steps: Vec<StepDetails>,
    /// The roads the route is named after, in the order they're travelled
    /// ([`route_names`]).
    pub(crate) road_names: Vec<String>,
    /// Every class any of its intersections has.
    pub(crate) classes: RoadClasses,
    /// Its speed profile ([`speed_profile`]).
    pub(crate) runs: Vec<DirectionsRun>,
}

/// Each route's [`RouteDetails`] in an OSRM answer's `body`, in the answer's order. Empty
/// if the answer doesn't parse, which leaves every route without them.
fn route_details(body: &[u8]) -> Vec<RouteDetails> {
    let Ok(answer) = serde_json::from_slice::<OsrmAnswer>(body) else {
        return Vec::new();
    };
    let mut routes: Vec<RouteDetails> = Vec::with_capacity(answer.routes.len());
    // Each route's steps as naming it needs them. The routes are named together, so
    // that those the answer gives side by side are told apart.
    let mut roads: Vec<Vec<StepRoad>> = Vec::with_capacity(answer.routes.len());
    for route in &answer.routes {
        let mut details = RouteDetails::default();
        let mut route_roads = Vec::new();
        for leg in &route.legs {
            for step in &leg.steps {
                let road_name = leg.road_name(step);
                route_roads.push(StepRoad {
                    name: road_name.clone(),
                    length: step.distance,
                });
                details.steps.push(StepDetails {
                    signs: step.into(),
                    road_name,
                });
                for intersection in &step.intersections {
                    details.classes = details.classes.or(RoadClasses::of(&intersection.classes));
                }
            }
        }
        let legs: Vec<ProfileLeg> = route.legs.iter().map(OsrmLeg::profile).collect();
        details.runs = speed_profile(&legs);
        routes.push(details);
        roads.push(route_roads);
    }
    for (details, road_names) in routes.iter_mut().zip(route_names(&roads)) {
        details.road_names = road_names;
    }
    routes
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
    /// How each of the route's steps has its speed limits signed, one for each of
    /// `route.steps`; or none at all if the answer didn't say for every step.
    pub(crate) signs: Vec<StepSigns>,
}

impl NavigableRoute {
    /// How the step at `index` has its speed limits signed; unsaid if the answer didn't.
    pub(crate) fn step_signs(&self, index: usize) -> StepSigns {
        self.signs.get(index).copied().unwrap_or_default()
    }
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
    /// The roads to call it by, in the order they're travelled, such as "I-5 North" then
    /// "Mercer Street": the roads that make up most of it, with more where it would
    /// otherwise be called the same as another of the routes it came with. Empty if none
    /// of its roads has a name, or the answer didn't say.
    pub road_names: Vec<String>,
    /// Whether any of it is tolled.
    pub has_toll: bool,
    /// Whether any of it is a ferry crossing.
    pub has_ferry: bool,
    /// Whether any of it is on a motorway.
    pub has_motorway: bool,
    /// Its speed profile: the runs it's made of, from its start to its end. Their
    /// durations and distances add up to the route's to within a fraction of a percent,
    /// not exactly: the runs are measured along the route's line, and the route's own
    /// totals from its roads' lengths in whole meters. Empty if the answer has no
    /// annotation to make them from.
    pub runs: Vec<DirectionsRun>,
    /// The whole route, to navigate.
    pub navigable: NavigableRoute,
}

impl DirectionsRoute {
    /// `route`, its instructions in `language`, with what else the answer said of it.
    /// `details.steps` are used only if there's one for each of the route's steps: if
    /// not, no step has signs or a road name, rather than some other step's.
    fn new(route: Route, language: &'static str, details: RouteDetails) -> Self {
        let mut step_details = details.steps;
        if step_details.len() != route.steps.len() {
            step_details.clear();
        }
        let signs = step_details.iter().map(|step| step.signs).collect();
        let mut road_names = step_details.into_iter().map(|step| step.road_name);
        Self {
            geometry: route.geometry.iter().copied().map(Into::into).collect(),
            distance: route.distance,
            // Ferrostar's routes have no total of their own.
            duration: route.steps.iter().map(|step| step.duration).sum(),
            steps: route
                .steps
                .iter()
                .cloned()
                .map(|step| DirectionsStep::new(step, road_names.next().flatten()))
                .collect(),
            road_names: details.road_names,
            has_toll: details.classes.toll,
            has_ferry: details.classes.ferry,
            has_motorway: details.classes.motorway,
            runs: details.runs,
            navigable: NavigableRoute {
                route,
                language,
                signs,
            },
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
    ///
    /// Ferrostar's parser reads the routes; the answer is read again only for what it
    /// leaves out: each step's speed-limit sign and unit and its road, and what the
    /// intersections' classes and the legs' annotations say of each route, which is
    /// whether it has tolls, a ferry or a motorway, and its speed profile. The routes
    /// are named here too, together, so that no two are called the same if their roads
    /// can tell them apart.
    pub fn parse(&self, body: Vec<u8>) -> Result<Vec<DirectionsRoute>, DirectionsError> {
        let details = route_details(&body);
        let routes = self.adapter.parse_response(body)?;
        let mut details = details.into_iter();
        Ok(routes
            .into_iter()
            .map(|route| {
                DirectionsRoute::new(route, self.language, details.next().unwrap_or_default())
            })
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
    fn the_request_asks_for_the_annotation_the_speed_profile_is_made_from() {
        // Ferrostar's own filter: Valhalla annotates a leg only with what's asked for.
        let filters = &body(adapter())["filters"];
        assert_eq!(filters["action"], "include");
        let attributes = filters["attributes"].as_array().unwrap();
        for attribute in ["shape_attributes.time", "shape_attributes.length"] {
            assert!(attributes.contains(&attribute.into()), "{attribute}");
        }
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

    /// A router's answer from a fixture.
    fn fixture(path: &str) -> serde_json::Value {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
        let mut body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Valinor's captures hold the answer under `body`.
        if let Some(inner) = body.get_mut("body") {
            body = inner.take();
        }
        body
    }

    /// The routes in an answer, as the app's adapter reads them.
    fn parse(body: &serde_json::Value) -> Vec<DirectionsRoute> {
        adapter().parse(serde_json::to_vec(body).unwrap()).unwrap()
    }

    /// The routes in a fixture of a router's answer, as the app's adapter reads them.
    fn parse_fixture(path: &str) -> Vec<DirectionsRoute> {
        parse(&fixture(path))
    }

    /// Valinor's three routes across Seattle, from Columbia City to Ballard: two by
    /// Interstate 5 and one through the tolled State Route 99 tunnel. Asked for without
    /// Ferrostar's filter, so it has no annotation.
    const SEATTLE_ALTERNATES: &str =
        "../../valinor/valinor-tyr/fixtures/captures/seattle/auto_alternates/osrm/response.json";

    /// Valinor's answer to Ferrostar's request for a drive the length of Andorra, which
    /// ends through the tolled Envalira tunnel.
    const ANDORRA_CROSS_COUNTRY: &str = "../../valinor/valinor-tyr/fixtures/captures/andorra/ferrostar_auto_cross_country/osrm/response.json";

    #[test]
    fn routes_are_named_after_their_main_roads() {
        let names = |routes: &[DirectionsRoute]| -> Vec<Vec<String>> {
            routes
                .iter()
                .map(|route| route.road_names.clone())
                .collect()
        };
        let mut body = fixture(SEATTLE_ALTERNATES);
        let routes = parse(&body);
        // The second leaves the Interstate halfway for a street that's over a quarter of
        // it. The tiles this was captured from have no countries, so the Interstate is
        // as OpenStreetMap tags it.
        assert_eq!(
            names(&routes),
            [
                vec!["I 5 North"],
                vec!["I 5 North", "9th Avenue North"],
                vec!["SR 99"],
            ]
        );
        // Each step has its road: a street's name, a motorway's number, or nothing on a
        // ramp.
        let step_roads: Vec<Option<&str>> = routes[0]
            .steps
            .iter()
            .map(|step| step.road_name.as_deref())
            .collect();
        assert_eq!(
            step_roads[..8],
            [
                None,
                Some("Rainier Avenue South"),
                Some("South Alaska Street"),
                Some("15th Avenue South"),
                Some("South Columbian Way"),
                Some("I 5 North"),
                None,
                None,
            ]
        );
        // A street with a number goes by the number.
        assert_eq!(routes[2].steps[7].road_name.as_deref(), Some("SR 519"));

        // In the United States, the Interstate is written as it's signed.
        for route in body["routes"].as_array_mut().unwrap() {
            route["legs"][0]["admins"] = json!([{ "iso_3166_1": "US" }]);
        }
        let routes = parse(&body);
        assert_eq!(
            names(&routes),
            [
                vec!["I-5 North"],
                vec!["I-5 North", "9th Avenue North"],
                vec!["SR 99"],
            ]
        );
        assert_eq!(routes[0].steps[5].road_name.as_deref(), Some("I-5 North"));

        // Andorra's main roads are numbered, under a different street name in each town.
        let routes = parse_fixture(ANDORRA_CROSS_COUNTRY);
        assert_eq!(names(&routes), [["CG-2"]]);
    }

    #[test]
    fn routes_named_alike_are_told_apart() {
        // Cut the second route off where it leaves the Interstate, so that it's named as
        // the first is, and they differ in how they get off: the first by Northeast 45th
        // Street, the second by Mercer Street.
        let mut body = fixture(SEATTLE_ALTERNATES);
        let steps = body["routes"][1]["legs"][0]["steps"]
            .as_array_mut()
            .unwrap();
        let arrival = steps.pop().unwrap();
        steps.truncate(7);
        steps.push(arrival);
        let routes = parse(&body);
        assert_eq!(routes[0].road_names, ["I 5 North", "North Midvale Place"]);
        assert_eq!(routes[1].road_names, ["I 5 North", "Mercer Street"]);
        assert_eq!(routes[2].road_names, ["SR 99"]);
    }

    #[test]
    fn a_route_says_whether_it_has_tolls_a_ferry_or_a_motorway() {
        let facts = |route: &DirectionsRoute| (route.has_toll, route.has_ferry, route.has_motorway);
        let mut body = fixture(SEATTLE_ALTERNATES);
        let routes = parse(&body);
        assert_eq!(facts(&routes[0]), (false, false, true));
        assert_eq!(facts(&routes[1]), (false, false, true));
        // By the Interstate, then the tunnel.
        assert_eq!(facts(&routes[2]), (true, false, true));

        // Andorra has a tolled tunnel and no motorways.
        let routes = parse_fixture(ANDORRA_CROSS_COUNTRY);
        assert_eq!(facts(&routes[0]), (true, false, false));
        // Mainline's route across Wallingford is all streets.
        let routes = parse_fixture("fixtures/wallingford_osrm.json");
        assert_eq!(facts(&routes[0]), (false, false, false));

        // One intersection anywhere is enough: here, the last before arriving leaves by
        // a ferry.
        let steps = body["routes"][0]["legs"][0]["steps"]
            .as_array_mut()
            .unwrap();
        let before_arriving = steps.len() - 2;
        steps[before_arriving]["intersections"][0]["classes"] = json!(["ferry", "restricted"]);
        let routes = parse(&body);
        assert_eq!(facts(&routes[0]), (false, true, true));
        assert_eq!(facts(&routes[1]), (false, false, true));
    }

    /// Checks that `route`'s runs cover it: that their durations and distances add up to
    /// what its `legs` in the answer say they take, and each run's speed is its own
    /// distance over its duration.
    fn check_runs_cover(route: &DirectionsRoute, legs: &serde_json::Value) {
        let legs = legs.as_array().unwrap();
        let total = |key: &str| -> f64 { legs.iter().map(|leg| leg[key].as_f64().unwrap()).sum() };
        let duration: f64 = route.runs.iter().map(|run| run.duration).sum();
        let distance: f64 = route.runs.iter().map(|run| run.distance).sum();
        // The runs are exactly the annotation's segments and the intersections' turns.
        let annotated = |key: &str| -> f64 {
            legs.iter()
                .flat_map(|leg| leg["annotation"][key].as_array().unwrap())
                .map(|value| value.as_f64().unwrap())
                .sum()
        };
        let turns: f64 = legs
            .iter()
            .flat_map(|leg| leg["steps"].as_array().unwrap())
            .flat_map(|step| step["intersections"].as_array().unwrap())
            .filter_map(|intersection| intersection["turn_duration"].as_f64())
            .sum();
        assert!((duration - (annotated("duration") + turns)).abs() < 1e-6);
        assert!((distance - annotated("distance")).abs() < 1e-6);
        // Which is the leg's own totals but for rounding. The annotation times and
        // measures each segment along the line, to the millisecond and the decimeter;
        // the leg's totals are of its roads, whose lengths the graph keeps to the whole
        // meter. That's up to half a meter a road, which on these routes comes to a
        // tenth of a percent over tens of kilometers and two tenths over two.
        assert!(
            (duration - total("duration")).abs() <= 0.005 * total("duration"),
            "{duration} s of {}",
            total("duration")
        );
        assert!(
            (distance - total("distance")).abs() <= 0.005 * total("distance"),
            "{distance} m of {}",
            total("distance")
        );
        // Ferrostar's totals, which Dart has, are the same.
        assert!((route.duration - total("duration")).abs() < 0.01);
        assert!((route.distance - total("distance")).abs() < 0.01);
        for run in &route.runs {
            assert!(run.duration > 0.0 && run.distance > 0.0, "{run:?}");
            assert_eq!(run.speed, run.distance / run.duration);
        }
    }

    #[test]
    fn a_routes_runs_cover_it() {
        let body = fixture(ANDORRA_CROSS_COUNTRY);
        let legs = &body["routes"][0]["legs"];
        // 477 stretches of road between intersections, over 1731 segments of line.
        assert_eq!(
            legs[0]["annotation"]["duration"].as_array().unwrap().len(),
            1731
        );
        let routes = parse(&body);
        let route = &routes[0];
        check_runs_cover(route, legs);
        assert_eq!(route.runs.len(), 34);
        // No run is under a hundredth of the route unless it's a different kind of road
        // from those beside it.
        let shortest = route.duration / 100.0;
        assert!(route.runs.iter().all(|run| run.duration >= shortest));
        // The tolled tunnel and its approach, 3.6 km, are the only tolled runs, and are
        // side by side near the end.
        let tolled: Vec<usize> = (0..route.runs.len())
            .filter(|&run| route.runs[run].toll)
            .collect();
        assert_eq!(tolled, [30, 31]);
        let tolled_distance: f64 = tolled.iter().map(|&run| route.runs[run].distance).sum();
        assert!((tolled_distance - 3604.0).abs() < 10.0, "{tolled_distance}");
        assert!(route.runs.iter().all(|run| !run.motorway && !run.ferry));
        // Through the tunnel is the fastest of it, at about 80 km/h.
        let tunnel = &route.runs[31];
        assert!((tunnel.speed - 21.9).abs() < 0.1, "{tunnel:?}");
        assert!(route.runs.iter().all(|run| run.speed <= tunnel.speed + 0.5));

        // A route through a waypoint that isn't a stop is one leg, and a short one has
        // fewer, shorter runs.
        for (capture, runs) in [("ferrostar_auto_via", 23), ("ferrostar_auto", 26)] {
            let body = fixture(&format!(
                "../../valinor/valinor-tyr/fixtures/captures/andorra/{capture}/osrm/response.json"
            ));
            let routes = parse(&body);
            check_runs_cover(&routes[0], &body["routes"][0]["legs"]);
            assert_eq!(routes[0].runs.len(), runs, "{capture}");
        }

        // Mainline Valhalla's answer has the same annotation.
        let body = fixture("fixtures/wallingford_osrm.json");
        let routes = parse(&body);
        check_runs_cover(&routes[0], &body["routes"][0]["legs"]);
    }

    #[test]
    fn a_route_without_an_annotation_has_no_runs() {
        // Asked for without Ferrostar's filter.
        let routes = parse_fixture(SEATTLE_ALTERNATES);
        assert!(routes.iter().all(|route| route.runs.is_empty()));
        assert!(routes[0].has_motorway);

        // Nor does one whose annotation can't be matched to its roads, for an
        // intersection that doesn't say where it is.
        let mut body = fixture(ANDORRA_CROSS_COUNTRY);
        assert!(!parse(&body)[0].runs.is_empty());
        body["routes"][0]["legs"][0]["steps"][3]["intersections"][0]
            .as_object_mut()
            .unwrap()
            .remove("geometry_index");
        let routes = parse(&body);
        assert!(routes[0].runs.is_empty());
        assert_eq!(routes[0].road_names, ["CG-2"]);
    }

    #[test]
    fn each_step_keeps_its_speed_limit_sign_and_unit() {
        // Mainline Valhalla's answer in Seattle, which signs in mph on MUTCD signs.
        let routes = parse_fixture("fixtures/wallingford_osrm.json");
        let navigable = &routes[0].navigable;
        assert_eq!(navigable.route.steps.len(), 8);
        assert_eq!(navigable.signs.len(), 8);
        for i in 0..8 {
            assert_eq!(
                navigable.step_signs(i),
                StepSigns {
                    sign: Some(SpeedLimitSign::Mutcd),
                    unit: Some(SpeedLimitUnit::MilesPerHour),
                }
            );
        }

        // Valinor's in Andorra, through a waypoint, on Vienna signs in km/h: the legs'
        // steps are one list, as Ferrostar's are.
        let routes = parse_fixture(
            "../../valinor/valinor-tyr/fixtures/captures/andorra/ferrostar_auto_via/osrm/response.json",
        );
        let navigable = &routes[0].navigable;
        assert!(navigable.route.steps.len() > 2);
        assert_eq!(navigable.signs.len(), navigable.route.steps.len());
        assert!(navigable.signs.iter().all(|signs| *signs
            == StepSigns {
                sign: Some(SpeedLimitSign::Vienna),
                unit: Some(SpeedLimitUnit::KilometersPerHour),
            }));
    }

    #[test]
    fn signs_are_unsaid_where_the_answer_does_not_say() {
        // An unknown sign or unit is none, and the other still counts.
        let step = |sign: Option<&str>, unit: Option<&str>| {
            let step = json!({ "speedLimitSign": sign, "speedLimitUnit": unit });
            StepSigns::from(&serde_json::from_value::<OsrmStep>(step).unwrap())
        };
        assert_eq!(
            step(Some("china"), Some("mph")),
            StepSigns {
                sign: None,
                unit: Some(SpeedLimitUnit::MilesPerHour),
            }
        );
        assert_eq!(step(None, None), StepSigns::default());
        assert!(route_details(b"<html>").is_empty());

        // An answer whose steps don't line up with Ferrostar's has none for any step,
        // rather than some step's for another.
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/wallingford_osrm.json");
        let mut body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut details = route_details(&serde_json::to_vec(&body).unwrap());
        assert_eq!(details.len(), 1);
        details[0].steps.pop();
        let route = adapter()
            .adapter
            .parse_response(serde_json::to_vec(&body).unwrap())
            .unwrap()
            .swap_remove(0);
        let route = DirectionsRoute::new(route, "en-US", details.swap_remove(0));
        assert!(route.steps.iter().all(|step| step.road_name.is_none()));
        // What's said of the route as a whole doesn't depend on the steps lining up.
        assert_eq!(
            route.road_names,
            ["Woodlawn Avenue North", "North 45th Street"]
        );
        let navigable = route.navigable;
        assert!(navigable.signs.is_empty());
        assert_eq!(navigable.step_signs(0), StepSigns::default());

        // Nor does one without them.
        for step in body["routes"][0]["legs"][0]["steps"]
            .as_array_mut()
            .unwrap()
        {
            let step = step.as_object_mut().unwrap();
            step.remove("speedLimitSign");
            step.remove("speedLimitUnit");
        }
        let routes = adapter().parse(serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(routes[0].navigable.step_signs(3), StepSigns::default());
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
