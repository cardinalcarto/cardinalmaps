//! Turn-by-turn navigation along a [`NavigableRoute`], on Ferrostar's navigation controller.
//!
//! Ferrostar has no Flutter layer, so what its Kotlin and Swift `FerrostarCore` do is split:
//! the state the controller threads through each fix stays here, in a [`NavigationSession`],
//! since it holds Rust step-advance conditions Dart can't carry; when to reroute, which
//! announcements to speak, where fixes come from, and what to draw are Dart's. Nothing here
//! assumes a screen, so the same session can feed the phone, Android Auto or CarPlay.
//!
//! Each fix comes back as a [`NavUpdate`]: only what changes as you go, with indexes into
//! what Dart already has (the route's steps and line), rather than a copy of the whole state.

use super::directions::{
    DirectionsAdapter, DirectionsError, DirectionsPoint, DirectionsRequest, NavigableRoute,
};
use ferrostar::deviation_detection::{DeviationKind, RouteDeviation, RouteDeviationTracking};
use ferrostar::models::{
    CourseOverGround, DrivingSide, LaneInfo, ManeuverModifier as FerrostarModifier,
    ManeuverType as FerrostarManeuverType, Route, RouteStep, Speed, SpokenInstruction,
    UserLocation, VisualInstruction, VisualInstructionContent,
};
use ferrostar::navigation_controller::models::{
    CourseFiltering, NavState, NavigationControllerConfig, TripState, WaypointAdvanceMode,
};
use ferrostar::navigation_controller::step_advance::SerializableStepAdvanceCondition;
use ferrostar::navigation_controller::step_advance::conditions::DistanceToEndOfStepCondition;
use ferrostar::navigation_controller::{Navigator, create_navigator};
use ferrostar::simulation::{
    LocationBias, LocationSimulationState, advance_location_simulation,
    location_simulation_from_coordinates,
};
use flutter_rust_bridge::frb;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// A fix from the device's location provider, or a simulated one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavLocation {
    pub lat: f64,
    pub lng: f64,
    /// How far off the fix may be, in meters.
    pub horizontal_accuracy: f64,
    /// The direction of travel, in degrees clockwise from true north.
    pub course: Option<f64>,
    /// How far off the course may be, in degrees. Ignored without a course.
    pub course_accuracy: Option<f64>,
    /// In meters per second.
    pub speed: Option<f64>,
    /// How far off the speed may be, in meters per second. Ignored without a speed.
    pub speed_accuracy: Option<f64>,
    /// When the fix was taken, in milliseconds since the Unix epoch.
    pub timestamp_ms: i64,
}

impl From<NavLocation> for UserLocation {
    fn from(l: NavLocation) -> Self {
        let millis = Duration::from_millis(l.timestamp_ms.unsigned_abs());
        let timestamp = if l.timestamp_ms >= 0 {
            UNIX_EPOCH + millis
        } else {
            UNIX_EPOCH - millis
        };
        Self {
            coordinates: DirectionsPoint {
                lat: l.lat,
                lng: l.lng,
            }
            .into(),
            horizontal_accuracy: l.horizontal_accuracy,
            course_over_ground: l.course.filter(|c| c.is_finite()).map(|course| {
                // Ferrostar keeps whole degrees in [0, 360), so 359.6 is 0.
                let degrees = (course.rem_euclid(360.0).round() as u16) % 360;
                CourseOverGround {
                    degrees,
                    accuracy: l.course_accuracy.map(whole_degrees),
                }
            }),
            timestamp,
            speed: l.speed.map(|value| Speed {
                value,
                accuracy: l.speed_accuracy,
            }),
        }
    }
}

/// `degrees` as Ferrostar keeps a course's accuracy, in whole degrees.
fn whole_degrees(degrees: f64) -> u16 {
    degrees.round().clamp(0.0, f64::from(u16::MAX)) as u16
}

impl From<UserLocation> for NavLocation {
    fn from(l: UserLocation) -> Self {
        let timestamp_ms = match l.timestamp.duration_since(UNIX_EPOCH) {
            Ok(after) => after.as_millis() as i64,
            Err(before) => -(before.duration().as_millis() as i64),
        };
        Self {
            lat: l.coordinates.lat,
            lng: l.coordinates.lng,
            horizontal_accuracy: l.horizontal_accuracy,
            course: l.course_over_ground.map(|c| f64::from(c.degrees)),
            course_accuracy: l.course_over_ground.and_then(|c| c.accuracy).map(f64::from),
            speed: l.speed.map(|s| s.value),
            speed_accuracy: l.speed.and_then(|s| s.accuracy),
            timestamp_ms,
        }
    }
}

/// How you're traveling, which decides when a step is done and when you're off the route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationMode {
    Driving,
    Cycling,
    Walking,
}

/// How near the end of a step a car must come before the step can be done, in meters.
///
/// A step's end is its maneuver's point, in the middle of the intersection. At 50 km/h a
/// car covers 14 m between fixes a second apart, so 30 m catches two or more fixes there
/// even with the fix a few meters off, while a parallel street a block away is farther.
const DRIVING_ENTRY: u16 = 30;

/// How far a car must then be from the step's line before the next step starts, in meters.
///
/// Small, so the banner turns over as soon as the car is around the corner, not a block
/// later; it only has to be past the noise of a fix taken while still in the intersection.
const DRIVING_EXIT: u16 = 5;

/// The worst accuracy a fix may have to advance a car's step, in meters: a fix any worse
/// than the entry radius can't say which side of the intersection the car is on.
const DRIVING_ADVANCE_ACCURACY: u16 = 32;

/// How near the destination a car must come to arrive, in meters.
///
/// The destination is the point Valhalla snapped to the road; a car pulls up at the curb or
/// into a driveway beside it, a few car lengths away, and the trip should end there rather
/// than wait for the car to drive over the point.
const DRIVING_ARRIVAL: u16 = 20;

/// How far from the route a car may be before it's off it, in meters.
///
/// Wide enough for a divided highway's far side and for fixes thrown 20 to 30 m in a city's
/// canyons, which would otherwise ask for a new route on every block; narrow enough that a
/// turn onto the wrong street shows within a few seconds.
const DRIVING_DEVIATION: f64 = 50.0;

/// The worst accuracy a fix may have to count a car as off the route, in meters: half the
/// deviation, so that noise alone can't put a fix past it.
const DRIVING_DEVIATION_ACCURACY: u16 = 25;

/// How near the end of a step a bicycle must come before the step can be done, in meters.
///
/// Between a car's and a walker's: a bicycle covers 5 m or so between fixes, but rides on
/// paths and cycle tracks set back from the road the step follows.
const CYCLING_ENTRY: u16 = 25;

/// How far past the end of the step the bicycle's position on the route must move before
/// the next step starts, in meters.
///
/// Measured along the route rather than from the fix, since riders cross at crosswalks and
/// ride on the far side of the street; as small as a car's for the same reason as a car's.
const CYCLING_EXIT: u16 = 5;

/// The worst accuracy a fix may have to advance a bicycle's step, in meters, as for a car.
const CYCLING_ADVANCE_ACCURACY: u16 = 32;

/// How near the destination a bicycle must come to arrive, in meters: it's ridden up to the
/// door or the rack, closer than a car can park.
const CYCLING_ARRIVAL: u16 = 15;

/// How far from the route a bicycle may be before it's off it, in meters: a path beside a
/// road or across a park is often 20 m or more from the street Valhalla routed along.
const CYCLING_DEVIATION: f64 = 40.0;

/// The worst accuracy a fix may have to count a bicycle as off the route, in meters: half
/// the deviation, as for a car.
const CYCLING_DEVIATION_ACCURACY: u16 = 20;

/// How near the end of a step a walker must come before the step can be done, in meters:
/// Ferrostar's recommendation for walking, which covers either side of a wide street.
const WALKING_ENTRY: u16 = 20;

/// How far past the end of the step the walker's position on the route must move before the
/// next step starts, in meters.
///
/// Measured along the route, as for a bicycle, since walkers keep to whichever sidewalk
/// they're on; Ferrostar recommends 2 to 5 m, and at walking pace 3 m is a step or two
/// around the corner.
const WALKING_EXIT: u16 = 3;

/// The worst accuracy a fix may have to advance a walker's step, in meters: Ferrostar's
/// default, a little worse than a phone in a pocket among buildings usually reports.
const WALKING_ADVANCE_ACCURACY: u16 = 25;

/// How near the destination a walker must come to arrive, in meters: at the door, or across
/// the street from it.
const WALKING_ARRIVAL: u16 = 10;

/// How far from the route a walker may be before they're off it, in meters: a wide street's
/// far sidewalk, or a shortcut across a plaza, is still on the way.
const WALKING_DEVIATION: f64 = 30.0;

/// The worst accuracy a fix may have to count a walker as off the route, in meters: half the
/// deviation, as for a car.
const WALKING_DEVIATION_ACCURACY: u16 = 15;

/// How near a step's line a stop along the route must be to count as passed when the step
/// is done, in meters.
///
/// The router puts each stop on the road, so the line goes through it; this only allows for
/// rounding, and stays small so that a route that passes a stop's street before reaching
/// the stop doesn't count it early.
const WAYPOINT_RANGE: f64 = 15.0;

impl NavigationMode {
    /// Ferrostar's controller configuration for traveling this way.
    ///
    /// Deviation is checked against the route's line. With offline tiles it can instead be
    /// checked against the roads, through `RouteDeviationTracking::Custom`.
    fn config(self) -> NavigationControllerConfig {
        let (step_advance, arrival, deviation, course_filtering) = match self {
            NavigationMode::Driving => (
                SerializableStepAdvanceCondition::DistanceEntryExit {
                    distance_to_end_of_step: DRIVING_ENTRY,
                    distance_after_end_step: DRIVING_EXIT,
                    minimum_horizontal_accuracy: DRIVING_ADVANCE_ACCURACY,
                    has_reached_end_of_current_step: false,
                },
                (DRIVING_ARRIVAL, DRIVING_ADVANCE_ACCURACY),
                (DRIVING_DEVIATION, DRIVING_DEVIATION_ACCURACY),
                // A car goes where the road goes; the snapped course keeps the map steady
                // through the noise of a fix's own.
                CourseFiltering::SnapToRoute,
            ),
            NavigationMode::Cycling => (
                SerializableStepAdvanceCondition::DistanceEntryAndSnappedExit {
                    distance_to_end_of_step: CYCLING_ENTRY,
                    distance_after_end_step: CYCLING_EXIT,
                    minimum_horizontal_accuracy: CYCLING_ADVANCE_ACCURACY,
                    has_reached_end_of_current_step: false,
                },
                (CYCLING_ARRIVAL, CYCLING_ADVANCE_ACCURACY),
                (CYCLING_DEVIATION, CYCLING_DEVIATION_ACCURACY),
                CourseFiltering::SnapToRoute,
            ),
            NavigationMode::Walking => (
                SerializableStepAdvanceCondition::DistanceEntryAndSnappedExit {
                    distance_to_end_of_step: WALKING_ENTRY,
                    distance_after_end_step: WALKING_EXIT,
                    minimum_horizontal_accuracy: WALKING_ADVANCE_ACCURACY,
                    has_reached_end_of_current_step: false,
                },
                (WALKING_ARRIVAL, WALKING_ADVANCE_ACCURACY),
                (WALKING_DEVIATION, WALKING_DEVIATION_ACCURACY),
                // Walkers cut across plazas and stop to look around; the route's direction
                // would point them the wrong way.
                CourseFiltering::Raw,
            ),
        };
        let (arrival_distance, arrival_accuracy) = arrival;
        let (max_acceptable_deviation, deviation_accuracy) = deviation;
        NavigationControllerConfig {
            waypoint_advance: WaypointAdvanceMode::WaypointAlongAdvancingStep(WAYPOINT_RANGE),
            // Ferrostar's constructors for the entry-and-exit conditions are only in its
            // UniFFI build; its serializable form makes the same conditions.
            step_advance_condition: step_advance.into(),
            arrival_step_advance_condition: Arc::new(DistanceToEndOfStepCondition {
                distance: arrival_distance,
                minimum_horizontal_accuracy: arrival_accuracy,
            }),
            route_deviation_tracking: RouteDeviationTracking::StaticThreshold {
                minimum_horizontal_accuracy: deviation_accuracy,
                max_acceptable_deviation,
            },
            snapped_location_course_filtering: course_filtering,
        }
    }
}

/// Where a trip stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TripStatus {
    /// There's no trip.
    Idle,
    /// On the way.
    Navigating,
    /// Arrived.
    Complete,
}

/// How far from the route you are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavDeviation {
    /// On the current step.
    OnRoute,
    /// Off the current step, but on a later one: a shortcut, or a fix that put you ahead.
    /// The route still holds.
    OffStep,
    /// Off the route altogether, which is when to ask for a new one.
    OffRoute,
}

/// What a maneuver is, broadly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManeuverType {
    Turn,
    NewName,
    Depart,
    Arrive,
    Merge,
    OnRamp,
    OffRamp,
    Fork,
    EndOfRoad,
    Continue,
    Roundabout,
    Rotary,
    RoundaboutTurn,
    Notification,
    ExitRoundabout,
    ExitRotary,
}

impl From<FerrostarManeuverType> for ManeuverType {
    fn from(t: FerrostarManeuverType) -> Self {
        match t {
            FerrostarManeuverType::Turn => ManeuverType::Turn,
            FerrostarManeuverType::NewName => ManeuverType::NewName,
            FerrostarManeuverType::Depart => ManeuverType::Depart,
            FerrostarManeuverType::Arrive => ManeuverType::Arrive,
            FerrostarManeuverType::Merge => ManeuverType::Merge,
            FerrostarManeuverType::OnRamp => ManeuverType::OnRamp,
            FerrostarManeuverType::OffRamp => ManeuverType::OffRamp,
            FerrostarManeuverType::Fork => ManeuverType::Fork,
            FerrostarManeuverType::EndOfRoad => ManeuverType::EndOfRoad,
            FerrostarManeuverType::Continue => ManeuverType::Continue,
            FerrostarManeuverType::Roundabout => ManeuverType::Roundabout,
            FerrostarManeuverType::Rotary => ManeuverType::Rotary,
            FerrostarManeuverType::RoundaboutTurn => ManeuverType::RoundaboutTurn,
            FerrostarManeuverType::Notification => ManeuverType::Notification,
            FerrostarManeuverType::ExitRoundabout => ManeuverType::ExitRoundabout,
            FerrostarManeuverType::ExitRotary => ManeuverType::ExitRotary,
        }
    }
}

/// Which way a maneuver goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManeuverModifier {
    UTurn,
    SharpRight,
    Right,
    SlightRight,
    Straight,
    SlightLeft,
    Left,
    SharpLeft,
}

impl From<FerrostarModifier> for ManeuverModifier {
    fn from(m: FerrostarModifier) -> Self {
        match m {
            FerrostarModifier::UTurn => ManeuverModifier::UTurn,
            FerrostarModifier::SharpRight => ManeuverModifier::SharpRight,
            FerrostarModifier::Right => ManeuverModifier::Right,
            FerrostarModifier::SlightRight => ManeuverModifier::SlightRight,
            FerrostarModifier::Straight => ManeuverModifier::Straight,
            FerrostarModifier::SlightLeft => ManeuverModifier::SlightLeft,
            FerrostarModifier::Left => ManeuverModifier::Left,
            FerrostarModifier::SharpLeft => ManeuverModifier::SharpLeft,
        }
    }
}

/// Which side of the road traffic keeps to, which decides which way roundabouts turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavDrivingSide {
    Left,
    Right,
}

impl From<DrivingSide> for NavDrivingSide {
    fn from(side: DrivingSide) -> Self {
        match side {
            DrivingSide::Left => NavDrivingSide::Left,
            DrivingSide::Right => NavDrivingSide::Right,
        }
    }
}

/// One lane at a maneuver, as a banner's lane guidance shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct NavLane {
    /// Whether this lane can be used for the maneuver.
    pub active: bool,
    /// The ways the lane's arrows point, such as `left` or `straight`.
    pub directions: Vec<String>,
    /// The arrow to take, of those in an active lane.
    pub active_direction: Option<String>,
}

impl From<LaneInfo> for NavLane {
    fn from(lane: LaneInfo) -> Self {
        Self {
            active: lane.active,
            directions: lane.directions,
            active_direction: lane.active_direction,
        }
    }
}

/// One line of a banner.
#[derive(Clone, Debug, PartialEq)]
pub struct NavInstructionContent {
    pub text: String,
    pub maneuver_type: Option<ManeuverType>,
    pub maneuver_modifier: Option<ManeuverModifier>,
    /// For a roundabout, how far around it to go before leaving, in degrees: 180 leaves
    /// in the direction you came in.
    pub roundabout_exit_degrees: Option<u16>,
    /// The lanes, when the banner has lane guidance.
    pub lanes: Vec<NavLane>,
    /// The numbers (or names, such as `8B`) of the exit to take.
    pub exit_numbers: Vec<String>,
}

impl From<VisualInstructionContent> for NavInstructionContent {
    fn from(c: VisualInstructionContent) -> Self {
        Self {
            text: c.text,
            maneuver_type: c.maneuver_type.map(Into::into),
            maneuver_modifier: c.maneuver_modifier.map(Into::into),
            roundabout_exit_degrees: c.roundabout_exit_degrees,
            lanes: c
                .lane_info
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect(),
            exit_numbers: c.exit_numbers,
        }
    }
}

/// A banner for the coming maneuver.
#[derive(Clone, Debug, PartialEq)]
pub struct NavVisualInstruction {
    pub primary: NavInstructionContent,
    pub secondary: Option<NavInstructionContent>,
    /// The line under the banner, such as lane guidance or the maneuver after this one.
    pub sub: Option<NavInstructionContent>,
    /// How far before the maneuver the banner shows, in meters.
    pub trigger_distance_before_maneuver: f64,
}

impl From<VisualInstruction> for NavVisualInstruction {
    fn from(v: VisualInstruction) -> Self {
        Self {
            primary: v.primary_content.into(),
            secondary: v.secondary_content.map(Into::into),
            sub: v.sub_content.map(Into::into),
            trigger_distance_before_maneuver: v.trigger_distance_before_maneuver,
        }
    }
}

/// An announcement for the coming maneuver.
#[derive(Clone, Debug, PartialEq)]
pub struct NavSpokenInstruction {
    pub text: String,
    /// The announcement in SSML, for a speech engine that takes it.
    pub ssml: Option<String>,
    /// What tells this announcement apart from every other on the route, including others
    /// with the same words, so it's spoken once.
    ///
    /// Made when the route is read, so a new route's announcements are new.
    pub utterance_id: String,
    /// How far before the maneuver it's spoken, in meters.
    pub trigger_distance_before_maneuver: f64,
}

impl From<SpokenInstruction> for NavSpokenInstruction {
    fn from(s: SpokenInstruction) -> Self {
        Self {
            text: s.text,
            ssml: s.ssml,
            utterance_id: s.utterance_id.to_string(),
            trigger_distance_before_maneuver: s.trigger_distance_before_maneuver,
        }
    }
}

/// One maneuver and the road after it, with everything navigation shows and says for it.
#[derive(Clone, Debug, PartialEq)]
pub struct NavStep {
    /// The road the step follows.
    pub road_name: Option<String>,
    /// What to do, in words.
    pub instruction: String,
    /// How far this step goes, in meters.
    pub distance: f64,
    /// How long this step takes, in seconds.
    pub duration: f64,
    /// The names or numbers of the exit the step takes.
    pub exits: Vec<String>,
    /// The banners along the step, each shown from its trigger distance.
    pub visual_instructions: Vec<NavVisualInstruction>,
    /// The announcements along the step, each spoken at its trigger distance.
    pub spoken_instructions: Vec<NavSpokenInstruction>,
    /// The step's line, from the maneuver to the next one.
    pub geometry: Vec<DirectionsPoint>,
    /// The index in the route's line where this step's line starts.
    pub geometry_start: u32,
    pub driving_side: Option<NavDrivingSide>,
    /// For a roundabout, the exit to take (1 is the first).
    pub roundabout_exit_number: Option<u32>,
}

/// Where in the route's line each step's line starts.
///
/// Each step starts where the one before it ends, so the step lines are the route's line
/// cut at the maneuvers, sharing the cut points. The exception is the arrival step at the
/// end of each leg, whose line is the destination twice over: the route's line has it
/// once, so the arrival step covers nothing.
fn step_starts(route: &Route) -> Vec<usize> {
    let mut starts = Vec::with_capacity(route.steps.len());
    let mut start = 0;
    for step in &route.steps {
        starts.push(start);
        let first = step.geometry.first();
        if step.geometry.iter().any(|point| Some(point) != first) {
            start += step.geometry.len() - 1;
        }
    }
    starts
}

impl NavigableRoute {
    /// The route's steps, with their banners, announcements and lines.
    ///
    /// `#[frb(sync)]`: the route is never changed, so nothing waits on it, and Dart asks for
    /// its steps once per route.
    #[frb(sync)]
    pub fn steps(&self) -> Vec<NavStep> {
        let starts = step_starts(&self.route);
        self.route
            .steps
            .iter()
            .zip(starts)
            .map(|(step, start)| nav_step(step, start))
            .collect()
    }
}

fn nav_step(step: &RouteStep, start: usize) -> NavStep {
    let step = step.clone();
    NavStep {
        road_name: step.road_name,
        instruction: step.instruction,
        distance: step.distance,
        duration: step.duration,
        exits: step.exits,
        visual_instructions: step
            .visual_instructions
            .into_iter()
            .map(Into::into)
            .collect(),
        spoken_instructions: step
            .spoken_instructions
            .into_iter()
            .map(Into::into)
            .collect(),
        geometry: step.geometry.into_iter().map(Into::into).collect(),
        geometry_start: start as u32,
        driving_side: step.driving_side.map(Into::into),
        roundabout_exit_number: step.roundabout_exit_number.map(u32::from),
    }
}

/// How far there is to go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavProgress {
    /// To the next maneuver, in meters.
    pub distance_to_next_maneuver: f64,
    /// To the next maneuver, in seconds.
    pub duration_to_next_maneuver: f64,
    /// To the destination, in meters.
    pub distance_remaining: f64,
    /// To the destination, in seconds.
    pub duration_remaining: f64,
}

/// Where navigation stands after a fix: what changes as you go, and nothing that doesn't.
#[derive(Clone, Debug, PartialEq)]
pub struct NavUpdate {
    pub status: TripStatus,
    /// The fix as it came; none when idle without one.
    pub location: Option<NavLocation>,
    /// The fix moved onto the route's line, with its course along the line for driving and
    /// cycling. Only while navigating.
    pub snapped_location: Option<NavLocation>,
    /// The index of the current step in the route's steps; once complete, the number of
    /// steps.
    pub step_index: u32,
    /// The index in the route's whole line of the point at or before the snapped location,
    /// so that the line up to it has been traveled and the line from it is still ahead.
    /// Once complete, the line's last point.
    pub geometry_index: u32,
    pub deviation: NavDeviation,
    /// How far from the current step's line (or, off the route, the route's) the fix is,
    /// in meters; 0 on the route.
    pub deviation_meters: f64,
    /// The banner to show; none off the route, where its distances would be wrong.
    pub visual_instruction: Option<NavVisualInstruction>,
    /// The announcement due, the same one on every fix until the next is; Dart speaks each
    /// [`NavSpokenInstruction::utterance_id`] once. None off the route.
    pub spoken_instruction: Option<NavSpokenInstruction>,
    /// Only while navigating.
    pub progress: Option<NavProgress>,
}

/// A trip along one route: Ferrostar's controller, and the state it threads through fixes.
struct Trip {
    navigator: Arc<dyn Navigator>,
    route: Route,
    /// [`step_starts`] of the route.
    step_starts: Vec<usize>,
    state: NavState,
}

impl Trip {
    fn new(route: &NavigableRoute, mode: NavigationMode, location: NavLocation) -> Self {
        let route = route.route.clone();
        let navigator = create_navigator(route.clone(), mode.config(), false);
        let state = navigator.get_initial_state(location.into());
        Self {
            navigator,
            step_starts: step_starts(&route),
            route,
            state,
        }
    }

    fn update(&self) -> NavUpdate {
        let last_point = self.route.geometry.len().saturating_sub(1) as u32;
        match self.state.trip_state() {
            TripState::Idle { user_location } => NavUpdate {
                status: TripStatus::Idle,
                location: user_location.map(Into::into),
                snapped_location: None,
                step_index: 0,
                geometry_index: 0,
                deviation: NavDeviation::OnRoute,
                deviation_meters: 0.0,
                visual_instruction: None,
                spoken_instruction: None,
                progress: None,
            },
            TripState::Complete { user_location, .. } => NavUpdate {
                status: TripStatus::Complete,
                location: Some(user_location.into()),
                snapped_location: None,
                step_index: self.route.steps.len() as u32,
                geometry_index: last_point,
                deviation: NavDeviation::OnRoute,
                deviation_meters: 0.0,
                visual_instruction: None,
                spoken_instruction: None,
                progress: None,
            },
            TripState::Navigating {
                current_step_geometry_index,
                user_location,
                snapped_user_location,
                remaining_steps,
                progress,
                deviation,
                visual_instruction,
                spoken_instruction,
                ..
            } => {
                let step_index = self.route.steps.len() - remaining_steps.len();
                let geometry_index = (self.step_starts[step_index]
                    + current_step_geometry_index.unwrap_or(0) as usize)
                    .min(last_point as usize);
                let (deviation, deviation_meters) = match deviation {
                    RouteDeviation::NoDeviation => (NavDeviation::OnRoute, 0.0),
                    RouteDeviation::Deviation {
                        kind:
                            DeviationKind::OffStepOnRoute {
                                deviation_from_step_line,
                            },
                    } => (NavDeviation::OffStep, deviation_from_step_line),
                    RouteDeviation::Deviation {
                        kind:
                            DeviationKind::CompletelyOffRoute {
                                deviation_from_route_line,
                            },
                    } => (NavDeviation::OffRoute, deviation_from_route_line),
                };
                NavUpdate {
                    status: TripStatus::Navigating,
                    location: Some(user_location.into()),
                    snapped_location: Some(snapped_user_location.into()),
                    step_index: step_index as u32,
                    geometry_index: geometry_index as u32,
                    deviation,
                    deviation_meters,
                    visual_instruction: visual_instruction.map(Into::into),
                    spoken_instruction: spoken_instruction.map(Into::into),
                    progress: Some(NavProgress {
                        distance_to_next_maneuver: progress.distance_to_next_maneuver,
                        duration_to_next_maneuver: progress.duration_to_next_maneuver,
                        distance_remaining: progress.distance_remaining,
                        duration_remaining: progress.duration_remaining,
                    }),
                }
            }
        }
    }
}

/// Navigation along a route, fix by fix. Opaque to Dart.
///
/// Calls that change it take `&mut self`, so flutter_rust_bridge's lock on the session runs
/// them one at a time. They aren't `#[frb(sync)]`: a fix is snapped against the remaining
/// route, which on a long trip is thousands of points, so they run on flutter_rust_bridge's
/// worker pool, off the UI thread. Two calls in flight at once may run in either order, so
/// Dart awaits each before making the next.
#[frb(opaque)]
pub struct NavigationSession {
    mode: NavigationMode,
    trip: Trip,
}

impl NavigationSession {
    /// Starts navigating `route` from `location`, traveling by `mode`.
    pub fn start(
        route: &NavigableRoute,
        mode: NavigationMode,
        location: NavLocation,
    ) -> NavigationSession {
        Self {
            mode,
            trip: Trip::new(route, mode, location),
        }
    }

    /// Where navigation stands now: after [`Self::start`], where it starts.
    ///
    /// `#[frb(sync)]`: it only reads the state, which is quick.
    #[frb(sync)]
    pub fn current(&self) -> NavUpdate {
        self.trip.update()
    }

    /// How the session is traveling.
    #[frb(sync)]
    pub fn mode(&self) -> NavigationMode {
        self.mode
    }

    /// Moves navigation on to `location`: advances the step when its maneuver is done,
    /// finishes the trip on arrival, and says how far off the route the fix is.
    pub fn update_location(&mut self, location: NavLocation) -> NavUpdate {
        let trip = &mut self.trip;
        trip.state = trip
            .navigator
            .update_user_location(location.into(), trip.state.clone());
        trip.update()
    }

    /// Skips to the next step, or finishes the trip from the last, whatever the fixes say:
    /// for when they can't be trusted, as in a tunnel.
    pub fn advance(&mut self) -> NavUpdate {
        let trip = &mut self.trip;
        trip.state = trip.navigator.advance_to_next_step(trip.state.clone());
        trip.update()
    }

    /// The request for a new route from `location` through the stops not yet passed, for
    /// when the session is off the route. The location's course, if it has one, goes in as
    /// the start's heading, so the new route doesn't begin with a U-turn.
    ///
    /// Errs once the trip is over, when there's nowhere left to go.
    #[frb(sync)]
    pub fn reroute_request(
        &self,
        adapter: &DirectionsAdapter,
        location: NavLocation,
    ) -> Result<DirectionsRequest, DirectionsError> {
        let TripState::Navigating {
            remaining_waypoints,
            ..
        } = self.trip.state.trip_state()
        else {
            return Err(DirectionsError::failed("the trip is over"));
        };
        adapter.request_through(location.into(), remaining_waypoints)
    }

    /// Navigates `route` instead, from `location`, traveling the same way: for a new route
    /// after leaving the old one.
    pub fn replace_route(&mut self, route: &NavigableRoute, location: NavLocation) -> NavUpdate {
        self.trip = Trip::new(route, self.mode, location);
        self.trip.update()
    }
}

/// Which side of the route a simulation's fixes are set off to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimulationBias {
    /// On the line.
    None,
    Left,
    Right,
    /// To one side, chosen at random once for the whole simulation.
    Random,
}

/// Why a simulation couldn't be made.
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationError {
    /// A description for logs.
    pub message: String,
}

/// Fixes along a route, as if driving it, for development and tests. Opaque to Dart.
#[frb(opaque)]
pub struct LocationSimulation {
    state: LocationSimulationState,
    finished: bool,
}

impl LocationSimulation {
    /// Fixes along `route`'s line, at most `resample_distance` meters apart if it's given
    /// (otherwise at the line's points), set off `bias_meters` to the side `bias` says.
    ///
    /// `#[frb(sync)]`, as is each step: a simulation is for development, and resampling even
    /// a long route takes a few milliseconds.
    #[frb(sync)]
    pub fn new(
        route: &NavigableRoute,
        resample_distance: Option<f64>,
        bias: SimulationBias,
        bias_meters: f64,
    ) -> Result<LocationSimulation, SimulationError> {
        let bias = match bias {
            SimulationBias::None => LocationBias::None,
            SimulationBias::Left => LocationBias::Left(bias_meters),
            SimulationBias::Right => LocationBias::Right(bias_meters),
            SimulationBias::Random => LocationBias::Random(bias_meters),
        };
        // Ferrostar resamples the line after its first point, leaving the first segment,
        // however long, as one jump; starting from the first point twice resamples it too.
        let line = &route.route.geometry;
        let from_start: Vec<_> = line.first().into_iter().chain(line).copied().collect();
        let start = location_simulation_from_coordinates(&from_start, resample_distance, bias)
            .map_err(|error| SimulationError {
                message: error.to_string(),
            })?;
        // The first point again, now with its course toward the next.
        Ok(Self {
            state: advance_location_simulation(&start),
            finished: false,
        })
    }

    /// The fix the simulation is at: at first, the start of the route.
    #[frb(sync)]
    pub fn current(&self) -> NavLocation {
        self.state.current_location.into()
    }

    /// The next fix along the route, or the last one again once [`Self::is_finished`].
    #[frb(sync)]
    pub fn advance(&mut self) -> NavLocation {
        let next = advance_location_simulation(&self.state);
        // At the end, Ferrostar gives back the same state.
        self.finished = next == self.state;
        self.state = next;
        self.current()
    }

    /// Whether the simulation has reached the end of the route.
    #[frb(sync)]
    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::directions::DistanceUnits;
    use std::path::PathBuf;
    use std::time::SystemTime;

    /// The route in a capture of mainline Valhalla's OSRM-format answer to Ferrostar's
    /// request, which Valinor's router gives byte for byte (`valinor-router`'s
    /// `capture_tests`).
    fn capture(name: &str, costing: &str) -> NavigableRoute {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../valinor/valinor-tyr/fixtures/captures/andorra")
            .join(name)
            .join("osrm/response.json");
        let response: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let body = serde_json::to_vec(&response["body"]).unwrap();
        adapter(costing)
            .parse(body)
            .unwrap()
            .swap_remove(0)
            .navigable
    }

    fn adapter(costing: &str) -> DirectionsAdapter {
        DirectionsAdapter::valhalla(
            "http://router/route".into(),
            costing.into(),
            0,
            "pt-PT".into(),
            DistanceUnits::Miles,
        )
        .unwrap()
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    /// A good fix at `lat`, `lng`, heading `course`.
    fn fix(lat: f64, lng: f64, course: Option<f64>) -> NavLocation {
        NavLocation {
            lat,
            lng,
            horizontal_accuracy: 5.0,
            course,
            course_accuracy: course.map(|_| 10.0),
            speed: Some(10.0),
            speed_accuracy: None,
            timestamp_ms: now_ms(),
        }
    }

    /// Every update of a simulated trip along `route`, fixes 5 m apart on the line.
    fn simulate(route: &NavigableRoute, mode: NavigationMode) -> Vec<NavUpdate> {
        let mut simulation =
            LocationSimulation::new(route, Some(5.0), SimulationBias::None, 0.0).unwrap();
        let mut session = NavigationSession::start(route, mode, simulation.current());
        let mut updates = vec![session.current()];
        while !simulation.is_finished() {
            let next = simulation.advance();
            updates.push(session.update_location(next));
        }
        updates
    }

    const TRIPS: [(&str, &str, NavigationMode); 5] = [
        ("ferrostar_auto", "auto", NavigationMode::Driving),
        ("ferrostar_auto_via", "auto", NavigationMode::Driving),
        ("ferrostar_roundabout", "auto", NavigationMode::Driving),
        ("ferrostar_bicycle", "bicycle", NavigationMode::Cycling),
        (
            "ferrostar_pedestrian_german_miles",
            "pedestrian",
            NavigationMode::Walking,
        ),
    ];

    /// Meters from `p` to the segment from `a` to `b`, on a plane tangent at `p`, which is
    /// close enough over a segment's length.
    fn meters_to_segment(p: DirectionsPoint, a: DirectionsPoint, b: DirectionsPoint) -> f64 {
        let scale = 111_320.0;
        let cos = p.lat.to_radians().cos();
        let xy = |q: DirectionsPoint| ((q.lng - p.lng) * cos * scale, (q.lat - p.lat) * scale);
        let ((ax, ay), (bx, by)) = (xy(a), xy(b));
        let (dx, dy) = (bx - ax, by - ay);
        let length2 = dx * dx + dy * dy;
        let t = if length2 == 0.0 {
            0.0
        } else {
            (-(ax * dx + ay * dy) / length2).clamp(0.0, 1.0)
        };
        (ax + t * dx).hypot(ay + t * dy)
    }

    #[test]
    fn the_step_lines_are_the_route_line_cut_at_each_maneuver() {
        let mut cases: Vec<_> = TRIPS
            .iter()
            .map(|(name, costing, _)| (*name, *costing))
            .collect();
        // Two legs, so an arrival in the middle.
        cases.push(("auto_three_legs", "auto"));
        for (name, costing) in cases {
            let route = capture(name, costing);
            let line = &route.route.geometry;
            let steps = route.steps();
            assert_eq!(steps[0].geometry_start, 0, "{name}");
            for (i, step) in steps.iter().enumerate() {
                let start = step.geometry_start as usize;
                let points: Vec<_> = step.geometry.iter().map(|p| (p.lat, p.lng)).collect();
                if points.iter().all(|p| *p == points[0]) {
                    // An arrival: the end of its leg, once in the route's line.
                    assert_eq!((line[start].lat, line[start].lng), points[0], "{name} {i}");
                } else {
                    let along: Vec<_> = line[start..start + points.len()]
                        .iter()
                        .map(|p| (p.lat, p.lng))
                        .collect();
                    assert_eq!(along, points, "{name} step {i}");
                }
            }
            let last = steps.last().unwrap();
            assert_eq!(
                last.geometry_start as usize,
                line.len() - 1,
                "{name}: the arrival is at the line's end"
            );
        }
    }

    #[test]
    fn a_simulated_trip_arrives_with_steps_only_going_forward() {
        for (name, costing, mode) in TRIPS {
            let route = capture(name, costing);
            let line = &route.route.geometry;
            let steps = route.route.steps.len() as u32;
            let updates = simulate(&route, mode);

            let complete = updates
                .iter()
                .position(|u| u.status == TripStatus::Complete)
                .unwrap_or_else(|| panic!("{name} never arrives"));
            assert!(
                updates[complete..]
                    .iter()
                    .all(|u| u.status == TripStatus::Complete && u.step_index == steps),
                "{name} stays arrived"
            );
            assert!(
                updates[..complete]
                    .iter()
                    .all(|u| u.status == TripStatus::Navigating)
            );

            let mut visited = vec![false; steps as usize];
            for pair in updates.windows(2) {
                assert!(pair[0].step_index <= pair[1].step_index, "{name}");
                // The line's index can step back: where a step turns sharply back,
                // fixes past the turn are still within the exit distance of the step
                // before, and snap back along it until they leave it.
            }
            for update in &updates[..complete] {
                visited[update.step_index as usize] = true;
                assert_eq!(update.deviation, NavDeviation::OnRoute, "{name}");
                // Valhalla gives the arrival no banner.
                let banners = &route.route.steps[update.step_index as usize].visual_instructions;
                assert_eq!(
                    update.visual_instruction.is_some(),
                    !banners.is_empty(),
                    "{name}"
                );
                // The snapped fix is on the segment the index starts.
                let i = update.geometry_index as usize;
                let snapped = update.snapped_location.unwrap();
                let at = DirectionsPoint {
                    lat: snapped.lat,
                    lng: snapped.lng,
                };
                let next = line[(i + 1).min(line.len() - 1)];
                let off = meters_to_segment(at, line[i].into(), next.into());
                assert!(off < 0.5, "{name}: {off} m from segment {i}");
            }
            assert_eq!(
                updates[complete - 1].geometry_index as usize,
                line.len() - 1,
                "{name} reaches the end of the line"
            );
            // Fixes every 5 m see every step, however short.
            let missed: Vec<_> = (0..steps as usize).filter(|&i| !visited[i]).collect();
            assert!(missed.is_empty(), "{name} skips steps {missed:?}");
        }
    }

    #[test]
    fn every_announcement_is_due_once_in_order() {
        for (name, costing, mode) in TRIPS {
            let route = capture(name, costing);
            let expected: Vec<String> = route
                .steps()
                .into_iter()
                .flat_map(|step| step.spoken_instructions)
                .map(|spoken| spoken.utterance_id)
                .collect();
            assert!(!expected.is_empty(), "{name} has announcements");
            let mut said: Vec<String> = Vec::new();
            for update in simulate(&route, mode) {
                if let Some(spoken) = update.spoken_instruction
                    && said.last() != Some(&spoken.utterance_id)
                {
                    said.push(spoken.utterance_id);
                }
            }
            assert_eq!(said, expected, "{name}");
        }
    }

    #[test]
    fn announcements_have_text_and_ids_unique_to_the_route() {
        let route = capture("ferrostar_pedestrian_german_miles", "pedestrian");
        let spoken: Vec<_> = route
            .steps()
            .into_iter()
            .flat_map(|step| step.spoken_instructions)
            .collect();
        let mut ids: Vec<_> = spoken.iter().map(|s| s.utterance_id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), spoken.len());
        assert!(spoken.iter().all(|s| {
            !s.text.is_empty()
                && s.ssml
                    .as_deref()
                    .is_some_and(|ssml| ssml.starts_with("<speak>"))
        }));
    }

    #[test]
    fn a_fix_far_from_the_route_is_off_it() {
        let route = capture("ferrostar_auto", "auto");
        let start = route.route.geometry[0];
        let mut session = NavigationSession::start(
            &route,
            NavigationMode::Driving,
            fix(start.lat, start.lng, None),
        );
        assert_eq!(session.current().deviation, NavDeviation::OnRoute);

        // A kilometer south of the start, where the route doesn't go. Ferrostar checks the
        // fix before the one it's given, so it takes a second to tell.
        let away = fix(start.lat - 0.01, start.lng, None);
        session.update_location(away);
        let update = session.update_location(away);
        assert_eq!(update.status, TripStatus::Navigating);
        assert_eq!(update.deviation, NavDeviation::OffRoute);
        assert!(
            update.deviation_meters > 1000.0,
            "{}",
            update.deviation_meters
        );
        assert_eq!(update.visual_instruction, None);
        assert_eq!(update.spoken_instruction, None);

        // A fix that may be that far off by its own account says nothing either way.
        let mut vague = away;
        vague.horizontal_accuracy = 100.0;
        session.update_location(vague);
        assert_eq!(
            session.update_location(vague).deviation,
            NavDeviation::OnRoute
        );
    }

    #[test]
    fn a_reroute_goes_from_the_fix_with_its_heading_through_the_stops_left() {
        let route = capture("ferrostar_auto_via", "auto");
        let waypoints = route.route.waypoints.clone();
        let adapter = adapter("auto");
        let locations = |request: DirectionsRequest| -> Vec<serde_json::Value> {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["language"], "pt-PT");
            assert_eq!(body["units"], "miles");
            body["locations"].as_array().unwrap().clone()
        };

        let start = route.route.geometry[0];
        let mut session = NavigationSession::start(
            &route,
            NavigationMode::Driving,
            fix(start.lat, start.lng, None),
        );
        let here = fix(42.51, 1.52, Some(271.6));
        let before = locations(session.reroute_request(&adapter, here).unwrap());
        assert_eq!(before.len(), 3);
        assert_eq!(before[0]["lat"], 42.51);
        assert_eq!(before[0]["heading"], 272);
        assert_eq!(before[1]["lat"], waypoints[1].coordinate.lat);
        assert_eq!(before[1]["type"], "via");
        assert_eq!(before[2]["lon"], waypoints[2].coordinate.lng);
        assert_eq!(before[2]["type"], "break");

        // Past the stop, only the destination is left.
        let mut simulation =
            LocationSimulation::new(&route, Some(5.0), SimulationBias::None, 0.0).unwrap();
        let via = waypoints[1].coordinate;
        loop {
            let next = simulation.advance();
            session.update_location(next);
            if (next.lat - via.lat).abs() < 1e-6 && (next.lng - via.lng).abs() < 1e-6 {
                break;
            }
        }
        for _ in 0..20 {
            session.update_location(simulation.advance());
        }
        let after = locations(
            session
                .reroute_request(&adapter, fix(42.55, 1.52, None))
                .unwrap(),
        );
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].get("heading"), None);
        assert_eq!(after[1]["lat"], waypoints[2].coordinate.lat);

        // Once there, there's nowhere to reroute to.
        while !simulation.is_finished() {
            session.update_location(simulation.advance());
        }
        assert_eq!(session.current().status, TripStatus::Complete);
        assert!(session.reroute_request(&adapter, here).is_err());
    }

    #[test]
    fn a_new_route_starts_over_traveling_the_same_way() {
        let route = capture("ferrostar_pedestrian_german_miles", "pedestrian");
        let start = route.route.geometry[0];
        let mut session = NavigationSession::start(
            &route,
            NavigationMode::Walking,
            fix(start.lat, start.lng, None),
        );
        session.advance();
        assert_eq!(session.advance().step_index, 2);
        let update = session.replace_route(&route, fix(start.lat, start.lng, None));
        assert_eq!(update.step_index, 0);
        assert_eq!(update.status, TripStatus::Navigating);
        assert_eq!(session.mode(), NavigationMode::Walking);
    }

    #[test]
    fn skipping_every_step_arrives() {
        let route = capture("ferrostar_roundabout", "auto");
        let start = route.route.geometry[0];
        let mut session = NavigationSession::start(
            &route,
            NavigationMode::Driving,
            fix(start.lat, start.lng, None),
        );
        let steps = route.route.steps.len() as u32;
        for i in 1..steps {
            assert_eq!(session.advance().step_index, i);
        }
        let done = session.advance();
        assert_eq!(done.status, TripStatus::Complete);
        assert_eq!(done.step_index, steps);
        assert_eq!(done.geometry_index as usize, route.route.geometry.len() - 1);
    }

    #[test]
    fn a_route_is_spoken_in_the_language_its_adapter_resolved() {
        // The captures' adapter asks for pt-PT, which Valhalla has.
        assert_eq!(capture("ferrostar_auto", "auto").language(), "pt-PT");
        // Valhalla has no de-AT, so the request asks for de-DE, and the route says so.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../../valinor/valinor-tyr/fixtures/captures/andorra/ferrostar_auto/osrm/response.json",
        );
        let response: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let austrian = DirectionsAdapter::valhalla(
            "http://router/route".into(),
            "auto".into(),
            0,
            "de-AT".into(),
            DistanceUnits::Kilometers,
        )
        .unwrap();
        let routes = austrian
            .parse(serde_json::to_vec(&response["body"]).unwrap())
            .unwrap();
        assert_eq!(routes[0].navigable.language(), "de-DE");
    }

    #[test]
    fn banners_carry_maneuvers_and_roundabout_exits() {
        let route = capture("ferrostar_roundabout", "auto");
        let banners: Vec<_> = route
            .steps()
            .into_iter()
            .flat_map(|step| step.visual_instructions)
            .collect();
        let roundabout = banners
            .iter()
            .find(|b| b.primary.maneuver_type == Some(ManeuverType::Roundabout))
            .expect("a roundabout banner");
        assert!(roundabout.primary.roundabout_exit_degrees.is_some());
        assert!(
            banners
                .iter()
                .any(|b| b.primary.maneuver_type == Some(ManeuverType::Arrive))
        );
    }

    #[test]
    fn a_location_keeps_its_fields_through_ferrostar() {
        let location = NavLocation {
            lat: 47.6,
            lng: -122.3,
            horizontal_accuracy: 4.5,
            course: Some(359.6),
            course_accuracy: Some(12.4),
            speed: Some(13.2),
            speed_accuracy: Some(0.5),
            timestamp_ms: 1_763_000_000_123,
        };
        let back = NavLocation::from(UserLocation::from(location));
        assert_eq!(
            back,
            NavLocation {
                course: Some(0.0),
                course_accuracy: Some(12.0),
                ..location
            }
        );
        let bare = NavLocation {
            course: None,
            speed: None,
            ..location
        };
        let back = NavLocation::from(UserLocation::from(bare));
        assert_eq!(back.course_accuracy, None);
        assert_eq!(back.speed_accuracy, None);
    }

    #[test]
    fn a_simulation_ends_at_the_destination() {
        let route = capture("ferrostar_roundabout", "auto");
        let mut simulation =
            LocationSimulation::new(&route, Some(10.0), SimulationBias::Left, 3.0).unwrap();
        assert!(!simulation.is_finished());
        let mut count = 0;
        let mut last = simulation.current();
        while !simulation.is_finished() {
            last = simulation.advance();
            count += 1;
        }
        // A fix at each of the line's points, and more so that none are over 10 m apart.
        let points = route.route.geometry.len() as f64;
        let resampled = route.route.distance / 10.0;
        assert!(
            (count as f64) >= resampled && (count as f64) <= points + resampled,
            "{count} fixes for {} m",
            route.route.distance
        );
        let end = *route.route.geometry.last().unwrap();
        assert!((last.lat - end.lat).abs() < 1e-4 && (last.lng - end.lng).abs() < 1e-4);
        assert!(last.course.is_some());
        assert_eq!(simulation.advance(), last);
    }
}
