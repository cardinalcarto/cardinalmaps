//! The rider-facing response: JSON types for transit's own endpoint.
//!
//! Almost everything here is optional, because feeds differ in what they send.
//! Times are POSIX seconds; a stop's `time_zone` and each visit's `utc_offset_seconds`
//! let a client show local time without a time zone database.
//! Everything is built from [`crate::TransitSnapshot`]'s pinned state and borrowed strings are
//! copied once into the response, so it outlives the query that made it.

mod alerts;
mod build;
mod departures;
mod nearby;
mod ride;
mod vehicle;

use serde::Serialize;
use serde_with::skip_serializing_none;

pub(crate) use build::{build_response, empty_response, language_supported};
pub use nearby::{
    ContinuesAs, NearbyPage, NearbyResponse, NearbyRoute, NoRoutesReason, RouteDetailResponse,
    StopDeparture,
};
pub(crate) use nearby::{RouteLookupError, build_nearby, build_route_detail, empty_nearby};
pub(crate) use ride::{RideLookupError, build_ride};
pub(crate) use vehicle::lower;
pub(crate) use vehicle::vehicle_update;

/// The answer to one planning query.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanResponse {
    /// When the response was generated, in POSIX seconds.
    pub generated_at: i64,
    /// The departures searched.
    pub window: Window,
    /// What real-time data this answer used, and how old it was.
    pub realtime: RealtimeStatus,
    /// Every Pareto-optimal journey of the window, by departure then trips.
    pub journeys: Vec<Journey>,
    /// Why `journeys` is empty; absent when it isn't.
    pub no_journeys_reason: Option<NoJourneysReason>,
    /// How a direct trip without transit fared.
    pub direct: DirectResponse,
}

/// One of a ride's departures, described as a leg of its own.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RideResponse {
    /// When the response was generated, in POSIX seconds.
    pub generated_at: i64,
    /// What real-time data this answer used, and how old it was.
    pub realtime: RealtimeStatus,
    /// A plan's transit leg on the departure, from the boarding stop to the alighting stop.
    ///
    /// What only a plan knows is left out: `wait_seconds` is 0, and it has no
    /// `transfer_seconds`, `departures`, or `departure_alerts`.
    pub ride: TransitLeg,
}

/// What is known about whether a stop or leg is accessible to a rider in a wheelchair.
///
/// This is the app's one field for accessibility, on every stop and every leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Accessibility {
    /// The feed says it is accessible, or a wheelchair search routed it on streets.
    Accessible,
    /// The feed says it is not accessible.
    Inaccessible,
    /// Nothing says either way.
    Unknown,
}

/// Why a plan has no journeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoJourneysReason {
    /// No stop is within walking or cycling range of the origin.
    NoAccess,
    /// No stop is within walking or cycling range of the destination.
    NoEgress,
    /// Stops are in range at both ends, but no service connects them in the window.
    NoService,
    /// Transit was found, but every journey is slower than going there directly.
    SlowerThanDirect,
}

/// How a direct street search for the whole trip fared.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirectResponse {
    /// `found`, `no_route`, `not_searched`, or `failed`.
    pub status: &'static str,
    /// How long the direct trip takes, in seconds, when one was found.
    pub seconds: Option<u32>,
    /// Why the search failed, when it did.
    pub error: Option<String>,
}

/// The departure window of a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Window {
    /// The first departure considered, in POSIX seconds.
    pub earliest: i64,
    /// The last departure considered, in POSIX seconds.
    pub latest: i64,
}

/// Overall real-time freshness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeLevel {
    /// No feed was used; the answer is the schedule.
    Schedule,
    /// Some feeds were used and some were stale or undecodable.
    Degraded,
    /// Every provided feed was used.
    Realtime,
}

/// How current the real-time data behind a response was.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RealtimeStatus {
    /// The summary of the feeds below.
    pub level: RealtimeLevel,
    /// When the overlay was built, in POSIX seconds.
    pub built_at: i64,
    /// Each feed message, as of this response.
    pub feeds: Vec<FeedReport>,
}

/// How one feed stood at the time of the response.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeedReport {
    /// The feed's ID.
    pub feed: String,
    /// `trip_updates`, `vehicle_positions`, `alerts`, or `combined`.
    pub kind: &'static str,
    /// Whether this feed contributed to routing.
    pub state: FeedState,
    /// Seconds since the message's own timestamp, when it carried one.
    pub age_seconds: Option<i64>,
    /// The message's own timestamp.
    pub header_timestamp: Option<i64>,
    /// The age limit it was held to.
    pub stale_after_seconds: i64,
    /// Entities dropped while decoding.
    pub entities_dropped: usize,
    /// Why the bytes couldn't be decoded, if they couldn't.
    pub error: Option<String>,
}

/// One feed's effect on routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedState {
    /// Fresh and applied.
    Applied,
    /// Older than its limit; routing fell back to the schedule for this feed.
    Stale,
    /// The bytes couldn't be decoded; routing fell back to the schedule for this feed.
    Undecodable,
}

/// One Pareto-optimal way to make the trip.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Journey {
    /// When the rider leaves the origin, in POSIX seconds.
    pub departure: i64,
    /// When the rider reaches the destination, in POSIX seconds.
    pub arrival: i64,
    /// Total travel time.
    pub duration_seconds: i64,
    /// How many transit trips this uses.
    pub trips: u8,
    /// Time spent walking or cycling, by part of the journey.
    pub walking: WalkDurations,
    /// Time spent waiting for a boarding after the access or a transfer.
    pub wait_seconds: i64,
    /// The legs in travel order.
    pub legs: Vec<JourneyLeg>,
}

/// How long a journey spends off the vehicle, on foot or on a bicycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WalkDurations {
    /// The first leg.
    pub access_seconds: i64,
    /// The walks between trips.
    pub transfer_seconds: i64,
    /// The last leg.
    pub egress_seconds: i64,
    /// All three.
    pub total_seconds: i64,
}

/// One leg of a journey.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JourneyLeg {
    /// From the origin to the first stop.
    Access(AccessEgressLeg),
    /// A ride on one trip.
    Transit(Box<TransitLeg>),
    /// A walk or ride between two different stops.
    Transfer(Box<TransferLeg>),
    /// From the last stop to the destination.
    Egress(AccessEgressLeg),
}

/// A leg on foot or by bicycle between the journey's ends and a stop.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccessEgressLeg {
    /// `walking`, `cycling`, or `wheelchair`.
    pub mode: &'static str,
    /// What is known about accessibility on this leg.
    pub accessibility: Accessibility,
    /// How long the leg takes.
    pub duration_seconds: i64,
    /// When the leg starts, in POSIX seconds.
    pub departure: i64,
    /// When the leg ends, in POSIX seconds.
    pub arrival: i64,
    /// The stop at the transit end of this leg.
    pub stop: StopRef,
    /// The street path, when a street search expanded it.
    pub walk: Option<WalkPath>,
    /// `STOP_MOVED` alerts about the stop this leg walks to or from, to show next to the walk.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_moved: Vec<StopMovedNotice>,
}

/// A leg on foot or by bicycle between two stops.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransferLeg {
    /// `walking`, `cycling`, or `wheelchair`.
    pub mode: &'static str,
    /// What is known about accessibility on this leg.
    pub accessibility: Accessibility,
    /// How long the leg takes.
    pub duration_seconds: i64,
    /// When the leg starts, in POSIX seconds.
    pub departure: i64,
    /// When the leg ends, in POSIX seconds.
    pub arrival: i64,
    /// The stop left.
    pub from: StopRef,
    /// The stop reached.
    pub to: StopRef,
    /// The street path, when a street search expanded it.
    pub walk: Option<WalkPath>,
    /// `STOP_MOVED` alerts about the stop this leg walks to or from, to show next to the walk.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_moved: Vec<StopMovedNotice>,
}

/// A `STOP_MOVED` alert, attached to the walk that goes to or from the moved stop.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StopMovedNotice {
    /// `end` for a walk to the stop, `start` for a walk from it.
    pub walk_end: &'static str,
    /// The moved stop's `stop_id`.
    pub stop_id: String,
    /// The moved stop's name.
    pub name: Option<String>,
    /// The alert's short text, in every language; the new location is only ever in prose.
    pub header: Vec<Translation>,
    /// The alert's long text, in every language.
    pub description: Vec<Translation>,
    /// The alert's URL, in every language.
    pub url: Vec<Translation>,
}

/// A real street path for a walking or cycling leg.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WalkPath {
    /// The path's length, in meters.
    pub distance_meters: f64,
    /// How long the street search says it takes, in seconds.
    pub duration_seconds: f64,
    /// The path as a polyline6 string, as Valhalla's route shapes are.
    pub shape: String,
    /// Turn-by-turn directions.
    pub maneuvers: Vec<WalkManeuver>,
}

/// One instruction along a walking or cycling path.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WalkManeuver {
    /// The maneuver's type, as Valhalla's `Maneuver` names it, in `snake_case`.
    #[serde(rename = "type")]
    pub kind: String,
    /// What to do, in words.
    pub instruction: String,
    /// What to say before the maneuver.
    pub verbal_pre_transition_instruction: Option<String>,
    /// What to say after the maneuver.
    pub verbal_post_transition_instruction: Option<String>,
    /// Streets this maneuver follows.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub street_names: Vec<String>,
    /// The maneuver's length, in meters.
    pub distance_meters: f64,
    /// The maneuver's time, in seconds.
    pub duration_seconds: f64,
    /// Its start in the path's decoded shape.
    pub begin_shape_index: u32,
    /// Its end in the path's decoded shape.
    pub end_shape_index: u32,
}

/// A ride on one trip.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransitLeg {
    /// Whether the trip and both of its boarding and alighting stops are accessible:
    /// `inaccessible` if any says no, `accessible` if all say yes, and otherwise `unknown`.
    pub accessibility: Accessibility,
    /// When the trip leaves the boarding stop, as routing used it, in POSIX seconds.
    pub departure: i64,
    /// When the trip reaches the alighting stop, as routing used it, in POSIX seconds.
    pub arrival: i64,
    /// How long the ride takes.
    pub duration_seconds: i64,
    /// Seconds waiting at the boarding stop after the rider gets there.
    pub wait_seconds: i64,
    /// Seconds from the journey's previous ride reaching its alighting stop until the rider can
    /// board here, as routing counts it: the walk's cost, which includes any minimum transfer
    /// time, or after a ride to this same stop, the stop's change time.
    ///
    /// A departure leaving before the previous ride's arrival plus this is one the rider misses.
    /// Absent on the journey's first ride, which the rider reaches from the origin instead.
    pub transfer_seconds: Option<i64>,
    /// The departure's delay from the schedule at the boarding stop, when known.
    pub delay_seconds: Option<i32>,
    /// Whether any time in this leg came from real-time data.
    pub realtime: bool,
    /// The route.
    pub route: Option<RouteResponse>,
    /// The agency.
    pub agency: Option<AgencyResponse>,
    /// The trip.
    pub trip: TripResponse,
    /// Every stop from boarding to alighting.
    ///
    /// The first element is the boarding, the last the alighting.
    pub stops: Vec<StopVisit>,
    /// The original trips this ride spans, for a stay-seated ride across more than one.
    ///
    /// Empty for a ride on one trip.
    /// The leg's `route` and `trip` are the segment the rider boards.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<RideSegment>,
    /// The vehicle's path between boarding and alighting, as a polyline6 string,
    /// when the trip has a shape in the schedule.
    pub shape: Option<String>,
    /// The vehicle operating this run, when real-time data names one.
    pub vehicle: Option<Vehicle>,
    /// Alerts that apply to this leg.
    pub alerts: Vec<LegAlert>,
    /// Departures of any route from this ride's boarding stop to its alighting stop, in order of
    /// departure, around and including this leg's own: what a rider has if they miss it,
    /// or instead of it.
    ///
    /// Empty when the request's departure range lists none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub departures: Vec<RideDeparture>,
    /// Alerts that apply to some of `departures` but not to this leg itself.
    ///
    /// A departure's `departure_alerts` index this list, and its `alerts` index `alerts`,
    /// so an alert shared with the leg isn't repeated.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub departure_alerts: Vec<AlertResponse>,
}

/// One departure between a ride's boarding and alighting stops, of any route serving the two.
///
/// Only those exact two stops count, not others nearby. A departure that stays seated as its
/// vehicle continues as another route is one when it lets the rider on at the boarding stop and
/// off at the alighting stop, and is described by the route it boards.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags of the JSON a client reads, each omitted when false"
)]
pub struct RideDeparture {
    /// `trip_id` of the trip boarded.
    pub trip_id: String,
    /// `start_date`, as `YYYYMMDD`; absent when it is the leg's own trip's.
    pub start_date: Option<String>,
    /// The headsign at the boarding stop: its `stop_headsign`, or else `trip_headsign`.
    ///
    /// Absent when it is the leg's own: its first stop's `headsign`, or else its trip's.
    pub headsign: Option<String>,
    /// The GTFS route it boards, as a leg's `route`; absent when it is the leg's own route.
    pub route: Option<RouteResponse>,
    /// Whether it runs, and whether real-time data has seen it.
    pub status: DepartureStatus,
    /// When it leaves the boarding stop, as routing would use it (the prediction when there is
    /// one), in POSIX seconds; the scheduled time for a canceled or skipped departure.
    pub departure: i64,
    /// When it reaches the alighting stop, in the same way.
    pub arrival: i64,
    /// Scheduled departure from the boarding stop, in POSIX seconds.
    pub scheduled_departure: Option<i64>,
    /// Scheduled arrival at the alighting stop, in POSIX seconds.
    pub scheduled_arrival: Option<i64>,
    /// The departure's delay from the schedule at the boarding stop, when known.
    pub delay_seconds: Option<i32>,
    /// Whether `departure` or `arrival` came from real-time data; absent when neither did.
    #[serde(skip_serializing_if = "is_false")]
    pub realtime: bool,
    /// The vehicle's ID, when real-time data names one.
    pub vehicle_id: Option<String>,
    /// Whether the trip and both stops are accessible, as a leg's `accessibility` is.
    pub accessibility: Accessibility,
    /// `bikes_allowed`: `yes`, `no`, or absent when unknown.
    pub bikes_allowed: Option<&'static str>,
    /// Whether this is the journey's own departure; absent on the others.
    #[serde(skip_serializing_if = "is_false")]
    pub suggested: bool,
    /// Whether it leaves the boarding stop before the rider can be there:
    /// before the previous ride's arrival plus the leg's `transfer_seconds`, or for the first
    /// ride, before the request's departure (the window's `earliest`) plus the walk from the
    /// origin.
    #[serde(skip_serializing_if = "is_false")]
    pub missed: bool,
    /// Whether no later departure of any route between these two stops runs on its service date:
    /// the connection's last of the day. Canceled and skipped departures don't count.
    #[serde(skip_serializing_if = "is_false")]
    pub last_of_day: bool,
    /// Indices into the leg's `alerts` that apply to this departure.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alerts: Vec<usize>,
    /// Indices into the leg's `departure_alerts` that apply to this departure.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub departure_alerts: Vec<usize>,
}

/// Whether a departure runs, and whether real-time data has seen it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DepartureStatus {
    /// It runs on its schedule, as far as anything says.
    Scheduled,
    /// It runs, with times from real-time data.
    Live,
    /// Real-time data or a `NO_SERVICE` alert says it doesn't run.
    Canceled,
    /// It runs, but no longer lets riders on at the boarding stop or off at the alighting stop.
    Skipped,
}

// `serde` passes a reference.
#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde's signature")]
fn is_false(value: &bool) -> bool {
    !*value
}

/// One original trip of a stay-seated ride.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RideSegment {
    /// `trip_id`.
    pub trip_id: String,
    /// The original trip's route.
    pub route: Option<RouteResponse>,
    /// `trip_headsign`.
    pub headsign: Option<String>,
    /// The first of this leg's `stops` this segment covers.
    pub first_stop_index: usize,
    /// The last of this leg's `stops` this segment covers, inclusive.
    pub last_stop_index: usize,
}

/// A GTFS route.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteResponse {
    /// `route_id`.
    pub route_id: String,
    /// `route_short_name`.
    pub short_name: Option<String>,
    /// `route_long_name`.
    pub long_name: Option<String>,
    /// `route_desc`.
    pub description: Option<String>,
    /// `route_type`, including extended values.
    pub route_type: u16,
    /// `route_color`, as `RRGGBB`.
    pub color: Option<String>,
    /// `route_text_color`, as `RRGGBB`.
    pub text_color: Option<String>,
    /// `route_url`.
    pub url: Option<String>,
}

/// An agency.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgencyResponse {
    /// `agency_id`, which single-agency feeds may omit.
    pub agency_id: Option<String>,
    /// The name riders see.
    pub name: String,
    /// `agency_url`.
    pub url: Option<String>,
    /// `agency_phone`.
    pub phone: Option<String>,
    /// `agency_fare_url`.
    pub fare_url: Option<String>,
}

/// A trip as it ran on one service date.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TripResponse {
    /// The feed the trip belongs to.
    pub feed: Option<String>,
    /// `trip_id`, shared by every run of a frequency trip.
    pub trip_id: String,
    /// `start_date`, as `YYYYMMDD`.
    pub start_date: Option<String>,
    /// `start_time` from real-time data, in seconds from the start of the service day.
    ///
    /// For a headway-based run this identifies the vehicle's run, not the schedule's.
    pub start_time: Option<u32>,
    /// `trip_headsign`.
    pub headsign: Option<String>,
    /// `direction_id`.
    pub direction_id: Option<u32>,
    /// `block_id`.
    pub block_id: Option<String>,
    /// Whether the feed only promises a headway for this trip (`exact_times=0`),
    /// so its times are approximate and vehicles are not tied to specific runs.
    pub headway_based: Option<bool>,
    /// `wheelchair_accessible`: `yes`, `no`, or absent when unknown.
    pub wheelchair_accessible: Option<&'static str>,
    /// `bikes_allowed`: `yes`, `no`, or absent when unknown.
    pub bikes_allowed: Option<&'static str>,
    /// The trip's schedule relationship, from real-time data.
    pub schedule_relationship: Option<String>,
    /// What the overlay did to the trip: `delayed`, `stop_skipped`, `replaced`, or `added`.
    /// Absent when it runs to schedule.
    pub realtime_state: Option<&'static str>,
    /// The trip-level delay from real-time data, in seconds.
    pub delay_seconds: Option<i32>,
    /// When the trip's real-time information was generated, in POSIX seconds.
    pub updated_at: Option<i64>,
}

/// A stop at the time a rider would see it.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StopRef {
    /// Whether a rider in a wheelchair can use this stop.
    pub accessibility: Accessibility,
    /// The feed this stop belongs to; a `stop_id` is only unique within its feed.
    pub feed: Option<String>,
    /// `stop_id`.
    pub stop_id: String,
    /// `stop_name`.
    pub name: Option<String>,
    /// `stop_code`.
    pub code: Option<String>,
    /// `platform_code`.
    pub platform: Option<String>,
    /// Degrees north.
    pub latitude: Option<f64>,
    /// Degrees east.
    pub longitude: Option<f64>,
    /// `wheelchair_boarding`: `yes`, `no`, or absent when unknown.
    pub wheelchair_boarding: Option<&'static str>,
    /// `parent_station`'s `stop_id`.
    pub parent_station: Option<String>,
    /// `parent_station`'s name.
    pub parent_station_name: Option<String>,
    /// `stop_timezone`, or the feed's when the stop doesn't set one.
    pub time_zone: Option<String>,
}

/// One stop of a ride.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StopVisit {
    /// The stop.
    #[serde(flatten)]
    pub stop: StopRef,
    /// The GTFS `stop_sequence`.
    pub stop_sequence: Option<u32>,
    /// Arrival as routing used it, in POSIX seconds.
    pub arrival: i64,
    /// Departure as routing used it, in POSIX seconds.
    pub departure: i64,
    /// Scheduled arrival, in POSIX seconds.
    pub scheduled_arrival: Option<i64>,
    /// Scheduled departure, in POSIX seconds.
    pub scheduled_departure: Option<i64>,
    /// Predicted arrival from real-time data, in POSIX seconds.
    pub predicted_arrival: Option<i64>,
    /// Predicted departure from real-time data, in POSIX seconds.
    pub predicted_departure: Option<i64>,
    /// Arrival delay in seconds, when known.
    pub arrival_delay_seconds: Option<i32>,
    /// Departure delay in seconds, when known.
    pub departure_delay_seconds: Option<i32>,
    /// Arrival uncertainty from real-time data, in seconds.
    pub arrival_uncertainty_seconds: Option<i32>,
    /// Departure uncertainty from real-time data, in seconds.
    pub departure_uncertainty_seconds: Option<i32>,
    /// Whether the schedule marks this time approximate
    /// (`timepoint=0`, interpolated, or a headway-based trip).
    pub approximate: Option<bool>,
    /// The stop's schedule relationship from real-time data.
    pub schedule_relationship: Option<String>,
    /// Whether the trip no longer stops here.
    pub skipped: bool,
    /// Whether an active `NO_SERVICE` alert is why this stop was removed.
    pub removed_by_alert: bool,
    /// `pickup_type`, when not regular.
    pub pickup_type: Option<&'static str>,
    /// `drop_off_type`, when not regular.
    pub drop_off_type: Option<&'static str>,
    /// A headsign for this stop, when it differs from the trip's.
    pub headsign: Option<String>,
    /// A stop reassigned by real-time data for this visit, such as a platform change.
    pub assigned_stop_id: Option<String>,
    /// Predicted occupancy on departure.
    pub occupancy: Option<String>,
    /// This stop's UTC offset at the visit, in seconds east.
    pub utc_offset_seconds: Option<i32>,
}

/// The vehicle operating a ride, or a later update on its position.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Vehicle {
    /// The feed that reported it.
    pub feed: Option<String>,
    /// The vehicle's ID.
    pub id: Option<String>,
    /// The vehicle's rider-visible label.
    pub label: Option<String>,
    /// The vehicle's license plate.
    pub license_plate: Option<String>,
    /// The vehicle's wheelchair accessibility.
    pub wheelchair_accessible: Option<String>,
    /// The trip it is running, when given.
    pub trip_id: Option<String>,
    /// The route it is running, when given.
    pub route_id: Option<String>,
    /// Degrees north.
    pub latitude: Option<f32>,
    /// Degrees east.
    pub longitude: Option<f32>,
    /// Degrees clockwise from north.
    pub bearing: Option<f32>,
    /// Meters per second.
    pub speed_meters_per_second: Option<f32>,
    /// Odometer, in meters.
    pub odometer_meters: Option<f64>,
    /// Whether it is at, approaching, or heading to its current stop.
    pub status: Option<String>,
    /// The current or next stop.
    pub current_stop: Option<VehicleStop>,
    /// The `stop_sequence` of the current or next stop.
    pub current_stop_sequence: Option<u32>,
    /// How full the vehicle is.
    pub occupancy: Option<String>,
    /// How full the vehicle is, as a percentage.
    pub occupancy_percentage: Option<u32>,
    /// Congestion near the vehicle.
    pub congestion: Option<String>,
    /// When its position was measured, in POSIX seconds.
    pub timestamp: Option<i64>,
}

/// A vehicle's current or next stop.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VehicleStop {
    /// `stop_id`.
    pub stop_id: String,
    /// `stop_name`, when it matched a stop in the timetable.
    pub name: Option<String>,
}

/// A lookup of one vehicle by ID, for updating its position on a journey already shown.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VehicleUpdate {
    /// The vehicle, or absent if its feed has no recent data for it.
    pub vehicle: Option<Vehicle>,
    /// How the vehicle's feed stood at this moment.
    pub feeds: Vec<FeedReport>,
    /// When the overlay was built, in POSIX seconds.
    pub built_at: i64,
}

/// A service alert that applies to a leg.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LegAlert {
    /// The alert.
    #[serde(flatten)]
    pub alert: AlertResponse,
    /// When this is a `STOP_MOVED` alert about a stop the rider walks to or from:
    /// the stops on this leg it moves, so a client can show it next to that walk.
    pub moved_stops: Option<Vec<MovedStop>>,
}

/// A stop on a leg that has been moved.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MovedStop {
    /// `boarding` or `alighting`.
    pub role: &'static str,
    /// The stop's `stop_id`.
    pub stop_id: String,
    /// The stop's name.
    pub name: Option<String>,
}

/// One text or URL in one language.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Translation {
    /// The text.
    pub text: String,
    /// The language, when the feed gave one.
    pub language: Option<String>,
}

/// One localized image.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageResponse {
    /// The image's location.
    pub url: String,
    /// The IANA media type.
    pub media_type: String,
    /// The language, when the feed gave one.
    pub language: Option<String>,
}

/// One period, active from `start` until before `end`.
#[skip_serializing_none]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PeriodResponse {
    /// POSIX seconds; absent when unbounded.
    pub start: Option<i64>,
    /// POSIX seconds; absent when unbounded.
    pub end: Option<i64>,
}

/// One entity an alert informs; fields within one combine with AND.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InformedEntityResponse {
    /// `agency_id`.
    pub agency_id: Option<String>,
    /// `route_id`.
    pub route_id: Option<String>,
    /// `route_type`.
    pub route_type: Option<i32>,
    /// `direction_id`.
    pub direction_id: Option<u32>,
    /// `trip_id`.
    pub trip_id: Option<String>,
    /// `start_date` of the trip selector.
    pub trip_date: Option<String>,
    /// `start_time` of the trip selector, in seconds from the start of its service day.
    pub trip_start_time: Option<u32>,
    /// `stop_id`.
    pub stop_id: Option<String>,
}

/// A service alert.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlertResponse {
    /// The feed that published it.
    pub feed: Option<String>,
    /// The feed's entity ID, for display only.
    pub id: String,
    /// Cause, lowercase, as GTFS-RT names it (for example `construction`).
    pub cause: Option<String>,
    /// Effect, lowercase, as GTFS-RT names it (for example `stop_moved`).
    pub effect: Option<String>,
    /// Severity, lowercase, as GTFS-RT names it.
    pub severity: Option<String>,
    /// When the alert applies, from `active_period`.
    pub active_periods: Vec<PeriodResponse>,
    /// When to show the alert.
    pub communication_periods: Vec<PeriodResponse>,
    /// When the service is affected.
    pub impact_periods: Vec<PeriodResponse>,
    /// `header_text`, in every language.
    pub header: Vec<Translation>,
    /// `description_text`, in every language.
    pub description: Vec<Translation>,
    /// `url`, in every language.
    pub url: Vec<Translation>,
    /// `tts_header_text`, in every language.
    pub tts_header: Vec<Translation>,
    /// `tts_description_text`, in every language.
    pub tts_description: Vec<Translation>,
    /// `cause_detail`, in every language.
    pub cause_detail: Vec<Translation>,
    /// `effect_detail`, in every language.
    pub effect_detail: Vec<Translation>,
    /// `image`, in every localized version.
    pub image: Vec<ImageResponse>,
    /// `image_alternative_text`, in every language.
    pub image_alternative_text: Vec<Translation>,
    /// Everything the alert informs.
    pub informed: Vec<InformedEntityResponse>,
}
