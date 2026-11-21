//! Ferrostar's routing adapter, for Dart: the request to send for a trip, and the routes in
//! the answer.
//!
//! The adapter doesn't do the sending. Dart posts the request to the server, and offline it
//! can hand the same body to [`super::router::OfflineRouter::route`]; either way the answer's
//! bytes come back here.

use super::navigation::{ManeuverModifier, ManeuverType};
use crate::route_details::{
    LegElevation, ProfileIntersection, ProfileLeg, RoadClasses, RouterRun, StepRoad, WayShares,
    road_name, route_elevation, route_names, router_profile, speed_profile,
};
use ferrostar::models::{
    GeographicCoordinate, ManeuverModifier as FerrostarModifier,
    ManeuverType as FerrostarManeuverType, Route, RouteStep, UserLocation, Waypoint, WaypointKind,
};
use ferrostar::routing_adapters::error::ParsingError;
use ferrostar::routing_adapters::{RouteAdapter, RouteRequest, WellKnownRouteProvider};
use flutter_rust_bridge::frb;
use serde::Deserialize;
use serde::de::IntoDeserializer;
use serde::de::value::Error as ValueError;
use serde_json::json;
use std::collections::HashMap;
use std::time::SystemTime;

/// How far off a trip's start is taken to be, in meters. Valhalla uses it to decide how sure
/// it can be of which side of the street the start is on.
const START_ACCURACY: f64 = 10.0;

/// Valhalla's costing for going on foot, and by wheelchair.
pub(crate) const PEDESTRIAN: &str = "pedestrian";

/// Valhalla's costing for going by bicycle.
const BICYCLE: &str = "bicycle";

/// How far apart the heights along a route are asked for, in meters.
///
/// The graph keeps a height at most every 32 m along a road, from a terrain model of 30 m
/// cells, so this is all the detail there is: a 500 m walk gets 18 heights, enough for a
/// profile, and no route gets fewer than it could. A long route gets more than a profile
/// some 350 px wide can show (3,300 for 100 km, about 20 kB of the answer), and the app
/// draws as many as it has room for. The climb is added up from all of them, so it
/// doesn't depend on how wide the profile is drawn.
const ELEVATION_INTERVAL: u32 = 30;

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
    /// What its maneuver is, broadly: a turn, a merge, the arrival. None if the answer
    /// didn't say, or said something this doesn't know.
    pub maneuver_type: Option<ManeuverType>,
    /// Which way its maneuver goes. None for one that goes no way in particular (a
    /// departure), or if the answer didn't say, or said something this doesn't know.
    pub maneuver_modifier: Option<ManeuverModifier>,
    /// Which exit to leave by, counting from the first after entering, if its maneuver
    /// is entering a roundabout and the answer says.
    pub roundabout_exit: Option<u8>,
}

impl DirectionsStep {
    /// `step`, with what else the answer said of it, if anything.
    fn new(step: RouteStep, details: Option<StepDetails>) -> Self {
        let details = details.unwrap_or_default();
        Self {
            instruction: step.instruction,
            distance: step.distance,
            duration: step.duration,
            road_name: details.road_name,
            maneuver_type: details.maneuver_type,
            maneuver_modifier: details.maneuver_modifier,
            roundabout_exit: step.roundabout_exit_number,
        }
    }
}

/// What kind of way a run is on, for the tone it's drawn in. Which three a route's runs
/// are among goes by how it's travelled: a drive's are freeway, main road and street, a
/// ride's trail, bike lane and shared road, and a walk's path, sidewalk and stairs. How
/// Valinor's words for a road become these is `run_kind` in the crate's
/// `route_details.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectionsRunKind {
    /// Driven, on a motorway or one of its ramps.
    Freeway,
    /// Driven, on a main road: a highway that isn't a motorway, or an arterial.
    MainRoad,
    /// Driven, on any smaller road.
    Street,
    /// Cycled, on a way without cars: a cycleway, a path, a track.
    Trail,
    /// Cycled, on a road, in a lane of its own, painted or protected.
    BikeLane,
    /// Cycled, on a road, among its traffic.
    SharedRoad,
    /// Walked, on a way without cars: a footpath, a trail, a pedestrian street.
    Path,
    /// Walked, along a street: on its sidewalk, a crossing, or the road itself.
    Sidewalk,
    /// Walked, up or down steps or an escalator.
    Stairs,
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
    /// What kind of way it's on. None for a ferry crossing, and for every run of an
    /// answer from a router that doesn't say.
    pub kind: Option<DirectionsRunKind>,
}

/// How high a route is along the way.
#[derive(Clone, Debug, PartialEq)]
pub struct DirectionsElevation {
    /// How far apart the heights are along the route, in meters.
    pub interval: f64,
    /// Meters above sea level: at the route's start, then every `interval` along it, and
    /// last at its end, which is less than an interval past the one before. At least two.
    pub heights: Vec<f32>,
    /// How far it climbs in all, in meters, not counting rises and dips of under 3 m,
    /// which are as likely the terrain model's ripple as the road's.
    pub climb: f64,
    /// How far it descends in all, in meters, counted the same way.
    pub descent: f64,
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
/// each step's speed-limit sign and unit, its road, its maneuver and the classes of its
/// intersections, and each leg's annotation and, from Valinor, its details. Everything
/// else in the answer is skipped.
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
    /// Meters.
    #[serde(default)]
    distance: f64,
    /// What Valinor adds when asked (`valinor_leg_details`); an older Valinor, or
    /// another router, has none.
    valinor_details: Option<OsrmLegDetails>,
}

/// A leg's `valinor_details` (see `valinor/docs/osrm-leg-details.md`).
#[derive(Deserialize)]
struct OsrmLegDetails {
    /// The leg's roads from its start to its end.
    #[serde(default)]
    runs: Vec<OsrmRun>,
    /// How far apart `elevation`'s heights are, in meters; none without elevation.
    elevation_interval: Option<f64>,
    /// Meters above sea level along the leg.
    #[serde(default)]
    elevation: Vec<f32>,
}

/// One of a leg's runs. A word this doesn't know reads as it comes, and one that's
/// missing as empty, which is no kind of road.
#[derive(Deserialize)]
struct OsrmRun {
    /// Seconds.
    #[serde(default)]
    duration: f64,
    /// Meters.
    #[serde(default)]
    distance: f64,
    #[serde(default)]
    travel_mode: String,
    #[serde(default)]
    road_class: String,
    #[serde(default, rename = "use")]
    road_use: String,
    cycle_lane: Option<String>,
    #[serde(default)]
    toll: bool,
}

impl<'a> From<&'a OsrmRun> for RouterRun<'a> {
    fn from(run: &'a OsrmRun) -> Self {
        Self {
            duration: run.duration,
            distance: run.distance,
            travel_mode: &run.travel_mode,
            road_class: &run.road_class,
            road_use: &run.road_use,
            cycle_lane: run.cycle_lane.as_deref(),
            toll: run.toll,
        }
    }
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
    maneuver: Option<OsrmManeuver>,
}

/// A step's `maneuver`, as far as its words for what it is. Ferrostar reads the same
/// words only of the banners, which are about the step after.
#[derive(Deserialize)]
struct OsrmManeuver {
    /// `turn`, `depart`, `off ramp` and so on.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// `left`, `slight right`, `uturn` and so on.
    modifier: Option<String>,
}

impl OsrmManeuver {
    /// What it is, broadly; none if it doesn't say or the word is one Ferrostar doesn't
    /// have. The words are OSRM's, which Ferrostar's own type reads.
    fn maneuver_type(&self) -> Option<ManeuverType> {
        let kind = self.kind.as_deref()?.into_deserializer();
        FerrostarManeuverType::deserialize::<serde::de::value::StrDeserializer<ValueError>>(kind)
            .ok()
            .map(Into::into)
    }

    /// Which way it goes; none if it doesn't say or the word is one Ferrostar doesn't
    /// have.
    fn modifier(&self) -> Option<ManeuverModifier> {
        let modifier = self.modifier.as_deref()?.into_deserializer();
        FerrostarModifier::deserialize::<serde::de::value::StrDeserializer<ValueError>>(modifier)
            .ok()
            .map(Into::into)
    }
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

    /// The leg's elevation, if Valinor gave it.
    fn elevation(&self) -> Option<LegElevation<'_>> {
        let details = self.valinor_details.as_ref()?;
        Some(LegElevation {
            interval: details.elevation_interval?,
            heights: &details.elevation,
            length: self.distance,
        })
    }
}

impl OsrmRoute {
    /// The route's speed profile: from Valinor's own runs if every leg has them, else
    /// from the legs' annotations and their intersections' classes, as for a router that
    /// has no such runs.
    fn runs(&self) -> Vec<DirectionsRun> {
        let details: Option<Vec<&OsrmLegDetails>> = self
            .legs
            .iter()
            .map(|leg| leg.valinor_details.as_ref())
            .collect();
        match details {
            Some(details) if !details.is_empty() => router_profile(
                details
                    .iter()
                    .flat_map(|details| &details.runs)
                    .map(RouterRun::from),
            ),
            _ => {
                let legs: Vec<ProfileLeg> = self.legs.iter().map(OsrmLeg::profile).collect();
                speed_profile(&legs)
            }
        }
    }

    /// The route's elevation, if Valinor gave every leg's.
    fn elevation(&self) -> Option<DirectionsElevation> {
        let legs: Vec<Option<LegElevation>> = self.legs.iter().map(OsrmLeg::elevation).collect();
        route_elevation(&legs)
    }
}

/// What the answer says of one step that Ferrostar's parser drops.
#[frb(ignore)]
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StepDetails {
    pub(crate) signs: StepSigns,
    /// What a driver calls the road it's on, if anything.
    pub(crate) road_name: Option<String>,
    /// What its maneuver is, if the answer said in words this knows.
    pub(crate) maneuver_type: Option<ManeuverType>,
    /// Which way its maneuver goes, likewise.
    pub(crate) maneuver_modifier: Option<ManeuverModifier>,
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
    /// Its speed profile ([`OsrmRoute::runs`]).
    pub(crate) runs: Vec<DirectionsRun>,
    /// Its elevation, if the answer gave it.
    pub(crate) elevation: Option<DirectionsElevation>,
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
                let maneuver = step.maneuver.as_ref();
                details.steps.push(StepDetails {
                    signs: step.into(),
                    road_name,
                    maneuver_type: maneuver.and_then(OsrmManeuver::maneuver_type),
                    maneuver_modifier: maneuver.and_then(OsrmManeuver::modifier),
                });
                for intersection in &step.intersections {
                    details.classes = details.classes.or(RoadClasses::of(&intersection.classes));
                }
            }
        }
        details.runs = route.runs();
        details.elevation = route.elevation();
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
    /// Its speed profile: the runs it's made of, from its start to its end.
    ///
    /// From Valinor, which says what each of a route's roads is, every run has a `kind`
    /// (but a ferry crossing), is tolled only where the road is, and the runs' durations
    /// and distances add up to the route's, to the millisecond and within a meter.
    ///
    /// From a router that doesn't say (an older Valinor, or Valhalla), the runs are made
    /// from the route's line instead: none has a `kind`, a run is tolled wherever the
    /// maneuver it's part of is tolled at all, and they add up to the route's to within a
    /// fraction of a percent, since they're measured along the line and the route's own
    /// totals from its roads' lengths in whole meters. Empty if that answer has no
    /// annotation to make them from.
    pub runs: Vec<DirectionsRun>,
    /// How high it is along the way, and how far it climbs. None if the answer didn't
    /// say, which it does only from Valinor, for a ride or a walk, where the map it
    /// routes on has elevation.
    pub elevation: Option<DirectionsElevation>,
    /// The share of its distance, 0 to 1, on trails if it's a ride, or on paths if it's a
    /// walk. Nothing if its runs have no kinds.
    #[frb(default = 0.0)]
    pub trail_share: f64,
    /// The share of its distance, 0 to 1, in bike lanes. Nothing if its runs have no
    /// kinds.
    #[frb(default = 0.0)]
    pub bike_lane_share: f64,
    /// Whether any of it is stairs. False if its runs have no kinds.
    #[frb(default = false)]
    pub has_stairs: bool,
    /// The whole route, to navigate.
    pub navigable: NavigableRoute,
}

impl DirectionsRoute {
    /// `route`, its instructions in `language`, with what else the answer said of it.
    /// `details.steps` are used only if there's one for each of the route's steps: if
    /// not, no step has signs, a road name or a maneuver, rather than some other step's.
    fn new(route: Route, language: &'static str, details: RouteDetails) -> Self {
        let mut step_details = details.steps;
        if step_details.len() != route.steps.len() {
            step_details.clear();
        }
        let signs = step_details.iter().map(|step| step.signs).collect();
        let mut step_details = step_details.into_iter();
        let shares = WayShares::of(&details.runs);
        Self {
            geometry: route.geometry.iter().copied().map(Into::into).collect(),
            distance: route.distance,
            // Ferrostar's routes have no total of their own.
            duration: route.steps.iter().map(|step| step.duration).sum(),
            steps: route
                .steps
                .iter()
                .cloned()
                .map(|step| DirectionsStep::new(step, step_details.next()))
                .collect(),
            road_names: details.road_names,
            has_toll: details.classes.toll,
            has_ferry: details.classes.ferry,
            has_motorway: details.classes.motorway,
            runs: details.runs,
            elevation: details.elevation,
            trail_share: shares.trail,
            bike_lane_share: shares.bike_lane,
            has_stairs: shares.stairs,
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
    ///
    /// Every request asks Valinor for its legs' details (`valinor_leg_details`), which
    /// the routes' runs are made from, and a ride or a walk for its elevation too, every
    /// 30 m ([`ELEVATION_INTERVAL`]): a drive's row draws its speeds, not its hills. A
    /// router that has no such details ignores the one and answers the other with
    /// nothing, and the routes are read as they were before there were any.
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
            "valinor_leg_details": true,
        });
        if costing == BICYCLE || costing == PEDESTRIAN {
            options["elevation_interval"] = json!(ELEVATION_INTERVAL);
        }
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
    /// leaves out: each step's speed-limit sign and unit, its road and its maneuver, and
    /// what the intersections' classes and the legs' annotations and details say of each
    /// route, which is whether it has tolls, a ferry or a motorway, its speed profile,
    /// and its elevation. The routes
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
        // A street with a number still goes by its name.
        assert_eq!(
            routes[2].steps[7].road_name.as_deref(),
            Some("Edgar Martinez Drive South")
        );

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

        // Andorra's main road goes by its number, where it has no street name.
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
            .filter(|banner| {
                banner.primary_content.maneuver_type == Some(FerrostarManeuverType::Roundabout)
            })
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

    #[test]
    fn the_request_asks_valinor_for_its_legs_details() {
        // A ride and a walk, by wheelchair too, ask for their elevation as well.
        for adapter in [adapter(), walking(false).unwrap(), walking(true).unwrap()] {
            let body = body(adapter);
            assert_eq!(body["valinor_leg_details"], true);
            assert_eq!(body["elevation_interval"], 30);
            // Ferrostar's own keys are still there.
            assert_eq!(body["format"], "osrm");
            assert_eq!(body["filters"]["action"], "include");
        }
        // A drive doesn't.
        for costing in ["auto", "motorcycle", "truck"] {
            let adapter = DirectionsAdapter::valhalla(
                "http://router/route".into(),
                costing.into(),
                2,
                "en-US".into(),
                DistanceUnits::Miles,
                false,
            )
            .unwrap();
            let body = body(adapter);
            assert_eq!(body["valinor_leg_details"], true, "{costing}");
            assert!(body.get("elevation_interval").is_none(), "{costing}");
        }
    }

    /// Valinor's answers, with its legs' details, to mainline Valhalla's own trips across
    /// Seattle: the routes of [`SEATTLE_ALTERNATES`], two rides and two walks. Written
    /// by `write_leg_details_fixtures` in `valinor-tyr`. The trips were built without
    /// elevation, so these have none.
    fn seattle_details(case: &str) -> serde_json::Value {
        fixture(&format!("fixtures/seattle_{case}_details.json"))
    }

    /// How many runs Valinor gives each route of `body`.
    fn valinor_run_counts(body: &serde_json::Value) -> Vec<usize> {
        body["routes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|route| {
                route["legs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|leg| leg["valinor_details"]["runs"].as_array().unwrap().len())
                    .sum()
            })
            .collect()
    }

    /// Checks that `route`'s runs, made from Valinor's, cover it: that they add up to
    /// what Ferrostar says it takes, to the 2 ms and the decimeter that rounding each
    /// step and each run leaves, and each run's speed is its own distance over its
    /// duration.
    fn check_valinors_runs_cover(route: &DirectionsRoute) {
        let duration: f64 = route.runs.iter().map(|run| run.duration).sum();
        let distance: f64 = route.runs.iter().map(|run| run.distance).sum();
        assert!(
            (duration - route.duration).abs() < 0.005,
            "{duration} s of {}",
            route.duration
        );
        assert!(
            (distance - route.distance).abs() < 0.1,
            "{distance} m of {}",
            route.distance
        );
        for run in &route.runs {
            assert!(run.duration > 0.0 && run.distance > 0.0, "{run:?}");
            assert_eq!(run.speed, run.distance / run.duration);
        }
    }

    /// Where each of `runs` starts along the route, to the meter, with the run.
    fn placed(runs: &[DirectionsRun]) -> Vec<(f64, &DirectionsRun)> {
        let mut start = 0.0;
        runs.iter()
            .map(|run| {
                let placed = (f64::round(start), run);
                start += run.distance;
                placed
            })
            .collect()
    }

    #[test]
    fn a_drives_runs_from_valinor_have_kinds_and_the_toll_where_it_is() {
        use DirectionsRunKind::{Freeway, MainRoad, Street};
        let body = seattle_details("auto_alternates");
        let routes = parse(&body);
        assert_eq!(routes.len(), 3);
        // Valinor's runs, which split wherever the router's speed changes, come down to
        // about a dozen.
        assert_eq!(valinor_run_counts(&body), [52, 63, 49]);
        let counts: Vec<usize> = routes.iter().map(|route| route.runs.len()).collect();
        assert_eq!(counts, [7, 11, 13]);
        for route in &routes {
            check_valinors_runs_cover(route);
            assert!(route.elevation.is_none());
            assert_eq!((route.trail_share, route.bike_lane_share), (0.0, 0.0));
            assert!(!route.has_stairs);
            for run in &route.runs {
                // A drive's three kinds, and a motorway is a freeway.
                assert!(matches!(run.kind, Some(Freeway | MainRoad | Street)));
                assert_eq!(run.motorway, run.kind == Some(Freeway), "{run:?}");
                assert!(!run.ferry);
            }
        }

        // The first is a street to start, main roads, 10.3 km of Interstate 5, and main
        // roads again.
        let kinds: Vec<(f64, Option<DirectionsRunKind>)> = placed(&routes[0].runs)
            .iter()
            .map(|(start, run)| (*start, run.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                (0.0, Some(Street)),
                (54.0, Some(MainRoad)),
                (595.0, Some(MainRoad)),
                (3390.0, Some(Freeway)),
                (4079.0, Some(Freeway)),
                (13735.0, Some(MainRoad)),
                (13946.0, Some(MainRoad)),
            ]
        );
        assert!(routes[0].runs.iter().all(|run| !run.toll));
        assert!(routes[1].runs.iter().all(|run| !run.toll));

        // The third takes State Route 99 through its tolled tunnel. The answer marks
        // every intersection of its 7.8 km step on SR 99 as tolled, since the maneuver
        // is, in part. The runs are tolled for the 3.8 km that the road is: the ramp
        // down to the tunnel, then the tunnel and the road out of it, a motorway.
        let tolled: Vec<(f64, f64, Option<DirectionsRunKind>)> = placed(&routes[2].runs)
            .iter()
            .filter(|(_, run)| run.toll)
            .map(|(start, run)| (*start, run.distance.round(), run.kind))
            .collect();
        assert_eq!(
            tolled,
            [
                (7379.0, 272.0, Some(MainRoad)),
                (7651.0, 3485.0, Some(Freeway)),
            ]
        );
        let step = &body["routes"][2]["legs"][0]["steps"][10];
        assert_eq!(step["ref"], "SR 99");
        assert_eq!(step["distance"], 7803.0);
        assert!(
            step["intersections"]
                .as_array()
                .unwrap()
                .iter()
                .all(|intersection| intersection["classes"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("toll")))
        );
        // What the route as a whole has is as the intersections say, as before.
        assert!(routes[2].has_toll && routes[2].has_motorway && !routes[2].has_ferry);
    }

    #[test]
    fn a_rides_runs_from_valinor_say_how_much_is_trail_and_bike_lane() {
        use DirectionsRunKind::{BikeLane, SharedRoad, Trail};
        // From Capitol Hill to Columbia City: nearly half in bike lanes, a sixth on
        // trails.
        let body = seattle_details("bicycle_capitol_hill_to_columbia_city");
        let routes = parse(&body);
        let route = &routes[0];
        check_valinors_runs_cover(route);
        // A ride's speed follows the grade, so Valinor's runs are many; they come down
        // to the stretches of each kind.
        assert_eq!(valinor_run_counts(&body), [171]);
        assert_eq!(route.runs.len(), 16);
        for run in &route.runs {
            assert!(matches!(run.kind, Some(Trail | BikeLane | SharedRoad)));
            assert!(!run.motorway && !run.toll && !run.ferry);
        }
        assert!((route.trail_share - 0.170).abs() < 0.001, "{route:?}");
        assert!((route.bike_lane_share - 0.481).abs() < 0.001);
        assert!(!route.has_stairs);
        // The longest run is 3.2 km of bike lane down the middle of it.
        let longest = route
            .runs
            .iter()
            .max_by(|a, b| a.distance.total_cmp(&b.distance))
            .unwrap();
        assert_eq!(longest.kind, Some(BikeLane));
        assert!((longest.distance - 3165.0).abs() < 1.0);

        // From Fremont to the university, along the Burke-Gilman Trail: mostly trail.
        let body = seattle_details("bicycle_fremont_to_uw");
        let routes = parse(&body);
        let route = &routes[0];
        check_valinors_runs_cover(route);
        assert_eq!(valinor_run_counts(&body), [81]);
        let kinds: Vec<Option<DirectionsRunKind>> = route.runs.iter().map(|run| run.kind).collect();
        assert_eq!(
            kinds,
            [
                Some(SharedRoad),
                Some(BikeLane),
                Some(BikeLane),
                Some(Trail),
                Some(Trail),
                Some(SharedRoad),
            ]
        );
        assert!((route.trail_share - 0.661).abs() < 0.001);
        assert!((route.bike_lane_share - 0.143).abs() < 0.001);
    }

    #[test]
    fn a_walks_runs_from_valinor_say_whether_it_has_stairs() {
        use DirectionsRunKind::{Path, Sidewalk, Stairs};
        // From Westlake to Capitol Hill: out of the station by its stairs, along the
        // street, and into the other by steps and an escalator.
        let body = seattle_details("pedestrian_westlake_to_capitol_hill");
        let routes = parse(&body);
        let route = &routes[0];
        check_valinors_runs_cover(route);
        assert_eq!(valinor_run_counts(&body), [67]);
        let kinds: Vec<(f64, Option<DirectionsRunKind>)> = placed(&route.runs)
            .iter()
            .map(|(start, run)| (*start, run.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                (0.0, Some(Stairs)),
                (10.0, Some(Path)),
                (64.0, Some(Sidewalk)),
                (1712.0, Some(Path)),
                (1752.0, Some(Stairs)),
                (1757.0, Some(Path)),
                (1763.0, Some(Stairs)),
                (1768.0, Some(Path)),
            ]
        );
        assert!(route.has_stairs);
        assert!((route.trail_share - 0.058).abs() < 0.001);
        assert_eq!(route.bike_lane_share, 0.0);

        // From Pike Place Market to the Space Needle there are none. Each block's
        // footway and each crossing between is a run of its own, a path and a sidewalk
        // in turn, since kinds never join.
        let body = seattle_details("pedestrian_pike_place_to_space_needle");
        let routes = parse(&body);
        let route = &routes[0];
        check_valinors_runs_cover(route);
        assert_eq!(valinor_run_counts(&body), [53]);
        assert_eq!(route.runs.len(), 22);
        assert!(!route.has_stairs);
        for run in &route.runs {
            assert!(matches!(run.kind, Some(Path | Sidewalk)));
        }
        assert!((route.trail_share - 0.616).abs() < 0.001);
    }

    #[test]
    fn each_step_has_its_maneuver() {
        use ManeuverModifier::{Left, SlightLeft, SlightRight};
        let mut body = seattle_details("auto_alternates");
        let routes = parse(&body);
        let maneuvers: Vec<(Option<ManeuverType>, Option<ManeuverModifier>)> = routes[0]
            .steps
            .iter()
            .map(|step| (step.maneuver_type, step.maneuver_modifier))
            .collect();
        assert_eq!(
            maneuvers,
            [
                (Some(ManeuverType::Depart), None),
                (Some(ManeuverType::Turn), Some(Left)),
                (Some(ManeuverType::Turn), Some(Left)),
                (Some(ManeuverType::Turn), Some(SlightRight)),
                (Some(ManeuverType::Fork), Some(SlightLeft)),
                (Some(ManeuverType::OffRamp), Some(SlightLeft)),
                (Some(ManeuverType::Fork), Some(SlightRight)),
                (Some(ManeuverType::OffRamp), Some(SlightLeft)),
                (Some(ManeuverType::Turn), Some(Left)),
                (Some(ManeuverType::Turn), Some(SlightRight)),
                (Some(ManeuverType::Arrive), None),
            ]
        );
        assert!(
            routes[0]
                .steps
                .iter()
                .all(|step| step.roundabout_exit.is_none())
        );
        // The other routes have an on ramp and the end of a road, and a ride new names.
        let types = |route: &DirectionsRoute| -> Vec<ManeuverType> {
            route
                .steps
                .iter()
                .filter_map(|step| step.maneuver_type)
                .collect()
        };
        assert!(types(&routes[1]).contains(&ManeuverType::OnRamp));
        assert!(types(&routes[1]).contains(&ManeuverType::EndOfRoad));
        let ride = parse(&seattle_details("bicycle_fremont_to_uw"));
        assert!(types(&ride[0]).contains(&ManeuverType::NewName));
        // An arrival says which side the place is on, where the router knows.
        let arrival = ride[0].steps.last().unwrap();
        assert_eq!(
            (arrival.maneuver_type, arrival.maneuver_modifier),
            (Some(ManeuverType::Arrive), Some(Left))
        );

        // A word this doesn't know, or none at all, is no maneuver, and nothing else of
        // the answer is lost for it.
        let steps = body["routes"][0]["legs"][0]["steps"]
            .as_array_mut()
            .unwrap();
        steps[1]["maneuver"]["type"] = json!("levitate");
        steps[2]["maneuver"]["modifier"] = json!("upward");
        steps[3]["maneuver"]
            .as_object_mut()
            .unwrap()
            .remove("modifier");
        let routes = parse(&body);
        let steps = &routes[0].steps;
        assert_eq!(
            (steps[1].maneuver_type, steps[1].maneuver_modifier),
            (None, Some(Left))
        );
        assert_eq!(
            (steps[2].maneuver_type, steps[2].maneuver_modifier),
            (Some(ManeuverType::Turn), None)
        );
        assert_eq!(
            (steps[3].maneuver_type, steps[3].maneuver_modifier),
            (Some(ManeuverType::Turn), None)
        );
        assert_eq!(steps[1].road_name.as_deref(), Some("Rainier Avenue South"));
        assert_eq!(routes[0].runs.len(), 7);
    }

    #[test]
    fn every_word_valinor_has_for_a_maneuver_is_read() {
        let read = |kind: &str, modifier: &str| {
            let maneuver = OsrmManeuver {
                kind: Some(kind.into()),
                modifier: Some(modifier.into()),
            };
            (maneuver.maneuver_type(), maneuver.modifier())
        };
        // The types Valinor's serializer writes (`valinor-tyr/src/tyr/osrm/maneuvers.rs`),
        // and OSRM's `roundabout turn`, which it doesn't.
        for (word, kind) in [
            ("depart", ManeuverType::Depart),
            ("arrive", ManeuverType::Arrive),
            ("turn", ManeuverType::Turn),
            ("continue", ManeuverType::Continue),
            ("new name", ManeuverType::NewName),
            ("merge", ManeuverType::Merge),
            ("on ramp", ManeuverType::OnRamp),
            ("off ramp", ManeuverType::OffRamp),
            ("fork", ManeuverType::Fork),
            ("end of road", ManeuverType::EndOfRoad),
            ("roundabout", ManeuverType::Roundabout),
            ("rotary", ManeuverType::Rotary),
            ("roundabout turn", ManeuverType::RoundaboutTurn),
            ("exit roundabout", ManeuverType::ExitRoundabout),
            ("exit rotary", ManeuverType::ExitRotary),
            ("notification", ManeuverType::Notification),
        ] {
            assert_eq!(read(word, "").0, Some(kind), "{word}");
        }
        for (word, modifier) in [
            ("uturn", ManeuverModifier::UTurn),
            ("sharp right", ManeuverModifier::SharpRight),
            ("right", ManeuverModifier::Right),
            ("slight right", ManeuverModifier::SlightRight),
            ("straight", ManeuverModifier::Straight),
            ("slight left", ManeuverModifier::SlightLeft),
            ("left", ManeuverModifier::Left),
            ("sharp left", ManeuverModifier::SharpLeft),
        ] {
            assert_eq!(read("", word).1, Some(modifier), "{word}");
        }
        assert_eq!(read("", ""), (None, None));
        assert_eq!(read("Turn", "LEFT"), (None, None));
        let unsaid = OsrmManeuver {
            kind: None,
            modifier: None,
        };
        assert_eq!((unsaid.maneuver_type(), unsaid.modifier()), (None, None));
    }

    #[test]
    fn an_answer_without_valinors_details_is_read_as_before() {
        // Mainline Valhalla's, and Valinor's own from before it had any: the runs are
        // made from the annotation, and say nothing of kinds or elevation.
        for path in ["fixtures/wallingford_osrm.json", ANDORRA_CROSS_COUNTRY] {
            let body = fixture(path);
            let routes = parse(&body);
            let route = &routes[0];
            check_runs_cover(route, &body["routes"][0]["legs"]);
            assert!(route.runs.iter().all(|run| run.kind.is_none()), "{path}");
            assert!(route.elevation.is_none());
            assert_eq!((route.trail_share, route.bike_lane_share), (0.0, 0.0));
            assert!(!route.has_stairs);
        }

        // With its details taken out, Valinor's answer has no runs to make from an
        // annotation it wasn't asked for, and the rest is as it was.
        let detailed = seattle_details("auto_alternates");
        let mut plain = detailed.clone();
        for route in plain["routes"].as_array_mut().unwrap() {
            let details = route["legs"][0]
                .as_object_mut()
                .unwrap()
                .remove("valinor_details");
            assert!(details.is_some());
        }
        assert_eq!(plain, fixture(SEATTLE_ALTERNATES));
        let (detailed, plain) = (parse(&detailed), parse(&plain));
        for (detailed, plain) in detailed.iter().zip(&plain) {
            assert!(plain.runs.is_empty() && !detailed.runs.is_empty());
            assert_eq!(detailed.steps, plain.steps);
            assert_eq!(detailed.road_names, plain.road_names);
            assert_eq!(detailed.has_toll, plain.has_toll);
            assert_eq!(detailed.has_motorway, plain.has_motorway);
            assert_eq!(detailed.duration, plain.duration);
            assert_eq!(detailed.distance, plain.distance);
        }

        // A route with details for only some of its legs falls back for all of it.
        let mut body = fixture(
            "../../valinor/valinor-tyr/fixtures/captures/andorra/auto_three_legs/osrm/response.json",
        );
        let legs = body["routes"][0]["legs"].as_array_mut().unwrap();
        assert_eq!(legs.len(), 2);
        legs[0]["valinor_details"] = json!({ "runs": [{
            "duration": 60.0, "distance": 600.0, "speed": 10.0, "travel_mode": "drive",
            "road_class": "primary", "use": "road",
        }] });
        let routes = parse(&body);
        assert!(routes[0].runs.iter().all(|run| run.kind.is_none()));
    }

    /// The offline router over the Andorra fixture's tiles, which have elevation.
    fn andorra_router() -> crate::api::router::OfflineRouter {
        let tiles = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../valinor/valhalla-graphtile/fixtures/andorra-tiles.tar");
        crate::api::router::OfflineRouter::open(vec![tiles.to_string_lossy().into()], None)
            .ok()
            .unwrap()
    }

    /// The app's adapter for `costing`, and the router's answer to its request for a
    /// way from Andorra la Vella to `to`.
    fn andorra_answer(
        router: &crate::api::router::OfflineRouter,
        costing: &str,
        to: DirectionsPoint,
    ) -> (DirectionsAdapter, Vec<u8>) {
        let adapter = DirectionsAdapter::valhalla(
            "http://router/route".into(),
            costing.into(),
            0,
            "en-US".into(),
            DistanceUnits::Kilometers,
            false,
        )
        .unwrap();
        let request = adapter.request(point(42.5078, 1.5211), to).unwrap();
        let response = router.route(request.body);
        assert_eq!(response.status, 200, "{costing}");
        (adapter, response.body)
    }

    #[test]
    fn a_ride_and_a_walk_from_the_router_have_their_elevation() {
        let router = andorra_router();
        // A ride up the valley to Ordino, 7.9 km, and a walk across town, 1.5 km.
        for (costing, to, heights, first, last, climb, descent) in [
            (
                "bicycle",
                point(42.5562, 1.5331),
                263,
                1038.6,
                1296.2,
                378.0,
                123.0,
            ),
            (
                "pedestrian",
                point(42.5099, 1.5357),
                52,
                1038.8,
                1028.0,
                10.2,
                21.0,
            ),
        ] {
            let (adapter, answer) = andorra_answer(&router, costing, to);
            let routes = adapter.parse(answer).unwrap();
            let route = &routes[0];
            check_valinors_runs_cover(route);
            assert!(route.runs.iter().all(|run| run.kind.is_some()), "{costing}");
            let elevation = route.elevation.as_ref().expect(costing);
            assert_eq!(elevation.interval, 30.0);
            // One at the start, one every 30 m, and one at the end.
            assert_eq!(elevation.heights.len(), heights, "{costing}");
            assert_eq!(
                elevation.heights.len(),
                (route.distance / 30.0).floor() as usize + 2
            );
            let (start, end) = (elevation.heights[0], elevation.heights[heights - 1]);
            assert!((start - first).abs() < 0.05, "{costing}: {start}");
            assert!((end - last).abs() < 0.05, "{costing}: {end}");
            assert!(
                (elevation.climb - climb).abs() < 0.05,
                "{costing}: {elevation:?}"
            );
            assert!((elevation.descent - descent).abs() < 0.05, "{costing}");
            // The climbs less the descents is the rise from one end to the other, but
            // for under 3 m of ripple at each end.
            let rise = f64::from(end - start);
            assert!((elevation.climb - elevation.descent - rise).abs() < 6.0);
        }
    }

    #[test]
    fn a_drive_from_the_router_has_runs_of_its_roads_and_no_elevation() {
        use DirectionsRunKind::MainRoad;
        let router = andorra_router();
        // The length of Andorra, to the far end of the tolled Envalira tunnel.
        let (adapter, answer) = andorra_answer(&router, "auto", point(42.5425, 1.7336));
        let body: serde_json::Value = serde_json::from_slice(&answer).unwrap();
        let routes = adapter.parse(answer.clone()).unwrap();
        let route = &routes[0];
        check_valinors_runs_cover(route);
        assert!(route.elevation.is_none());
        assert_eq!(valinor_run_counts(&body), [162]);
        assert_eq!(route.runs.len(), 35);
        // The toll is on the tunnel and its approach, 3.3 km, and not on the 263 m of
        // free road between the two that the maneuver takes in.
        let tolled: Vec<(f64, Option<DirectionsRunKind>)> = route
            .runs
            .iter()
            .filter(|run| run.toll)
            .map(|run| (run.distance.round(), run.kind))
            .collect();
        assert_eq!(
            tolled,
            [
                (173.0, Some(MainRoad)),
                (193.0, Some(MainRoad)),
                (2975.0, Some(MainRoad)),
            ]
        );
        assert!(route.has_toll);

        // The same answer without its details is read from its annotation, as an older
        // server's is: no kinds, and the toll on the whole maneuver, 3.6 km.
        let mut plain = body.clone();
        plain["routes"][0]["legs"][0]
            .as_object_mut()
            .unwrap()
            .remove("valinor_details");
        let routes = adapter.parse(serde_json::to_vec(&plain).unwrap()).unwrap();
        let fallback = &routes[0];
        check_runs_cover(fallback, &plain["routes"][0]["legs"]);
        assert!(fallback.runs.iter().all(|run| run.kind.is_none()));
        let tolled: f64 = fallback
            .runs
            .iter()
            .filter(|run| run.toll)
            .map(|run| run.distance)
            .sum();
        assert!((tolled - 3604.0).abs() < 10.0, "{tolled}");
        assert_eq!(fallback.steps, route.steps);
        assert_eq!(fallback.road_names, route.road_names);
    }

    #[test]
    fn a_roundabouts_step_says_which_exit() {
        let router = andorra_router();
        let adapter = DirectionsAdapter::valhalla(
            "http://router/route".into(),
            "auto".into(),
            0,
            "en-US".into(),
            DistanceUnits::Kilometers,
            false,
        )
        .unwrap();
        let request = adapter
            .request(point(42.505, 1.52), point(42.505, 1.515))
            .unwrap();
        let routes = adapter.parse(router.route(request.body).body).unwrap();
        let exits: Vec<(Option<ManeuverType>, Option<u8>)> = routes[0]
            .steps
            .iter()
            .filter(|step| step.roundabout_exit.is_some())
            .map(|step| (step.maneuver_type, step.roundabout_exit))
            .collect();
        // Into the one roundabout on the way, and out by its third exit.
        assert_eq!(exits, [(Some(ManeuverType::Roundabout), Some(3))]);
    }
}
