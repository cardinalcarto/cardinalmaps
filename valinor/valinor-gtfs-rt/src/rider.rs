//! Everything a rider might want to know, kept apart from the data the route scan reads.
//!
//! This is built alongside the overlay and indexed by trip, stop, route, and vehicle.
//! Nothing in the search touches it; the response layer reads it when building legs.
//! Almost every field is optional, because feeds differ in what they send
//! (King County, for one, omits bearing and speed).

use std::collections::HashMap;

use valinor_transit::{RouteId, TripKey};

use crate::alerts::Period;
use crate::date::CalendarDate;
use crate::feed::{FeedId, FeedStatus, ScopedId};
use crate::proto;
use crate::states::TripState;

/// One stop of a trip update, as a rider would be told about it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StopTimeDetails {
    /// The stop's position in the trip, when the trip matched the timetable.
    pub position: Option<u32>,
    /// The GTFS `stop_sequence`, when known.
    pub stop_sequence: Option<u32>,
    /// The stop, when the update names one.
    pub stop_id: Option<String>,
    /// How the stop relates to the schedule.
    pub relationship: Option<proto::trip_update::stop_time_update::ScheduleRelationship>,
    /// Whether a `NO_SERVICE` alert took this stop out of the trip.
    ///
    /// `relationship` is then `SKIPPED`, so that this agrees with routing.
    pub removed_by_alert: bool,
    /// The scheduled arrival, in POSIX seconds.
    pub scheduled_arrival: Option<i64>,
    /// The scheduled departure, in POSIX seconds.
    pub scheduled_departure: Option<i64>,
    /// The predicted arrival, in POSIX seconds.
    pub predicted_arrival: Option<i64>,
    /// The predicted departure, in POSIX seconds.
    pub predicted_departure: Option<i64>,
    /// The arrival delay in seconds, as the feed gave it or derived from the prediction.
    pub arrival_delay: Option<i32>,
    /// The departure delay in seconds, as the feed gave it or derived from the prediction.
    pub departure_delay: Option<i32>,
    /// The arrival uncertainty, in seconds.
    pub arrival_uncertainty: Option<i32>,
    /// The departure uncertainty, in seconds.
    pub departure_uncertainty: Option<i32>,
    /// The predicted occupancy on departure.
    pub occupancy: Option<proto::vehicle_position::OccupancyStatus>,
    /// A stop reassigned by the feed (`assigned_stop_id`), for platform changes.
    pub assigned_stop_id: Option<String>,
    /// A headsign override for this stop.
    pub stop_headsign: Option<String>,
}

/// One trip with real-time information.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TripDetails {
    /// The feed that reported the trip.
    pub feed: Option<FeedId>,
    /// The GTFS trip identifier.
    pub trip_id: String,
    /// The GTFS route identifier, when given.
    pub route_id: Option<String>,
    /// The service date, when given.
    pub start_date: Option<CalendarDate>,
    /// The start in seconds from the start of the day, when given.
    pub start_time: Option<u32>,
    /// The direction, when given.
    pub direction_id: Option<u32>,
    /// What the overlay did to the trip, as far as this update is concerned.
    pub state: Option<TripState>,
    /// The trip's identity in the timetable, when it matched.
    pub trip_key: Option<TripKey>,
    /// The stop pattern the trip runs in the snapshot, when it matched or was added.
    pub route: Option<RouteId>,
    /// The trip descriptor's schedule relationship.
    pub relationship: Option<proto::trip_descriptor::ScheduleRelationship>,
    /// When the update was generated, in POSIX seconds.
    pub timestamp: Option<i64>,
    /// The trip-level delay, in seconds.
    pub delay: Option<i32>,
    /// The vehicle on this trip, when the update named one.
    pub vehicle: Option<VehicleRef>,
    /// The trip's headsign, for added trips.
    pub headsign: Option<String>,
    /// Per-stop information, in stop order.
    pub stops: Vec<StopTimeDetails>,
}

/// A vehicle reference on a trip.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VehicleRef {
    /// The vehicle's identifier.
    pub id: Option<String>,
    /// The vehicle's rider-visible label.
    pub label: Option<String>,
    /// The vehicle's license plate.
    pub license_plate: Option<String>,
    /// The vehicle's wheelchair accessibility.
    pub wheelchair: Option<proto::vehicle_descriptor::WheelchairAccessible>,
}

/// A vehicle's position and state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VehicleDetails {
    /// The feed.
    pub feed: Option<FeedId>,
    /// The vehicle.
    pub vehicle: VehicleRef,
    /// The trip the vehicle is on, when given.
    pub trip_id: Option<String>,
    /// The route the vehicle is on, when given.
    pub route_id: Option<String>,
    /// The service date of its trip, when given.
    pub start_date: Option<CalendarDate>,
    /// The start of its trip in seconds from the start of the day, when given.
    ///
    /// For `exact_times=0` frequency trips this, rather than [`Self::trip_key`], tells two
    /// vehicles on the same trip apart: both can resolve to one headway-grid run.
    pub start_time: Option<u32>,
    /// The vehicle's trip, when it matched the timetable.
    pub trip_key: Option<TripKey>,
    /// Latitude, in degrees.
    pub latitude: Option<f32>,
    /// Longitude, in degrees.
    pub longitude: Option<f32>,
    /// Bearing, in degrees clockwise from north.
    pub bearing: Option<f32>,
    /// Speed, in meters per second.
    pub speed: Option<f32>,
    /// Odometer, in meters.
    pub odometer: Option<f64>,
    /// The `stop_sequence` of the current or next stop.
    pub current_stop_sequence: Option<u32>,
    /// The current or next stop.
    pub stop_id: Option<String>,
    /// Whether the vehicle is at, approaching, or heading to that stop.
    pub current_status: Option<proto::vehicle_position::VehicleStopStatus>,
    /// When the position was measured, in POSIX seconds.
    pub timestamp: Option<i64>,
    /// Congestion near the vehicle.
    pub congestion: Option<proto::vehicle_position::CongestionLevel>,
    /// How full the vehicle is.
    pub occupancy: Option<proto::vehicle_position::OccupancyStatus>,
    /// How full the vehicle is, as a percentage.
    pub occupancy_percentage: Option<u32>,
    /// Per-carriage details.
    pub carriages: Vec<proto::vehicle_position::CarriageDetails>,
}

/// One entity an alert informs. Fields within it combine with AND.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InformedEntity {
    /// Agency.
    pub agency_id: Option<String>,
    /// Route.
    pub route_id: Option<String>,
    /// GTFS `route_type`.
    pub route_type: Option<i32>,
    /// Direction.
    pub direction_id: Option<u32>,
    /// Trip.
    pub trip_id: Option<String>,
    /// Service date of the trip, when the selector named one.
    pub trip_date: Option<CalendarDate>,
    /// Start of the trip in seconds from the start of its day, when the selector named one run.
    ///
    /// For a trip with its own schedule this is the descriptor's value; for a frequency trip
    /// a selector with no `start_time` covers every run.
    pub trip_start_time: Option<u32>,
    /// Stop.
    pub stop_id: Option<String>,
}

/// What a rider sees of a service alert.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlertDetails {
    /// The feed that published the alert.
    pub feed: Option<FeedId>,
    /// The entity ID this alert had in its feed.
    ///
    /// For display only: these are not stable, and nothing keys on them.
    pub entity_id: String,
    /// When the alert applies, from the older `active_period` field.
    pub active_periods: Vec<Period>,
    /// When to show the alert.
    pub communication_periods: Vec<Period>,
    /// When the service is affected.
    pub impact_periods: Vec<Period>,
    /// The cause.
    pub cause: Option<proto::alert::Cause>,
    /// The effect.
    pub effect: Option<proto::alert::Effect>,
    /// The severity.
    pub severity: Option<proto::alert::SeverityLevel>,
    /// A URL, in every translation the feed gave.
    pub url: Option<proto::TranslatedString>,
    /// The header, in every translation.
    pub header: Option<proto::TranslatedString>,
    /// The description, in every translation.
    pub description: Option<proto::TranslatedString>,
    /// A text-to-speech header.
    pub tts_header: Option<proto::TranslatedString>,
    /// A text-to-speech description.
    pub tts_description: Option<proto::TranslatedString>,
    /// An image.
    pub image: Option<proto::TranslatedImage>,
    /// An image's alternative text.
    pub image_alternative_text: Option<proto::TranslatedString>,
    /// Detail on the cause.
    pub cause_detail: Option<proto::TranslatedString>,
    /// Detail on the effect.
    pub effect_detail: Option<proto::TranslatedString>,
    /// Everything the alert informs.
    pub informed: Vec<InformedEntity>,
}

/// Rider-facing real-time data, indexed by trip, stop, route, and vehicle.
///
/// All identifiers are scoped to their feed, so agencies never collide.
#[derive(Debug, Clone, Default)]
pub struct RiderData {
    trips: Vec<TripDetails>,
    trip_index: HashMap<(ScopedId, Option<CalendarDate>, Option<u32>), usize>,
    trips_by_key: HashMap<TripKey, usize>,
    /// Every update of a run, oldest first; more than one only for `exact_times=0` trips.
    trips_of_key: HashMap<TripKey, Vec<usize>>,
    trips_by_route: HashMap<ScopedId, Vec<usize>>,
    /// `(trip index, stops index)` per stop.
    stop_times: HashMap<ScopedId, Vec<(usize, usize)>>,
    vehicles: Vec<VehicleDetails>,
    vehicle_index: HashMap<ScopedId, usize>,
    vehicles_by_trip: HashMap<ScopedId, Vec<usize>>,
    vehicles_by_route: HashMap<ScopedId, Vec<usize>>,
    alerts: Vec<AlertDetails>,
    alerts_by_trip: HashMap<ScopedId, Vec<usize>>,
    alerts_by_stop: HashMap<ScopedId, Vec<usize>>,
    alerts_by_route: HashMap<ScopedId, Vec<usize>>,
    alerts_by_agency: HashMap<ScopedId, Vec<usize>>,
    alerts_by_route_type: HashMap<(FeedId, i32), Vec<usize>>,
    states: HashMap<TripKey, TripState>,
    feeds: Vec<FeedStatus>,
}

impl RiderData {
    pub(crate) fn push_trip(&mut self, trip: TripDetails) {
        let Some(feed) = trip.feed.clone() else {
            return;
        };
        let index = self.trips.len();
        self.trip_index.insert(
            (
                ScopedId::new(&feed, &trip.trip_id),
                trip.start_date,
                trip.start_time,
            ),
            index,
        );
        if let Some(key) = trip.trip_key {
            self.trips_by_key.insert(key, index);
            self.trips_of_key.entry(key).or_default().push(index);
        }
        if let Some(route) = trip.route_id.as_deref() {
            self.trips_by_route
                .entry(ScopedId::new(&feed, route))
                .or_default()
                .push(index);
        }
        for (position, stop) in trip.stops.iter().enumerate() {
            if let Some(stop_id) = stop.stop_id.as_deref() {
                self.stop_times
                    .entry(ScopedId::new(&feed, stop_id))
                    .or_default()
                    .push((index, position));
            }
        }
        self.trips.push(trip);
    }

    /// Records the route a trip ended up on, once added routes are numbered.
    pub(crate) fn set_trip_route(&mut self, key: TripKey, route: RouteId) {
        if let Some(&index) = self.trips_by_key.get(&key) {
            self.trips[index].route = Some(route);
        }
    }

    pub(crate) fn set_state(&mut self, key: TripKey, state: TripState) {
        self.states.insert(key, state);
        if let Some(&index) = self.trips_by_key.get(&key) {
            self.trips[index].state = Some(state);
        }
    }

    /// Marks stops that an alert removed from a trip with real-time details.
    pub(crate) fn mark_stops_removed(&mut self, key: TripKey, positions: &[usize]) {
        let Some(&index) = self.trips_by_key.get(&key) else {
            return;
        };
        let Some(trip) = self.trips.get_mut(index) else {
            return;
        };
        for &position in positions {
            let position = u32::try_from(position).ok();
            if let Some(stop) = trip.stops.iter_mut().find(|s| s.position == position) {
                stop.relationship =
                    Some(proto::trip_update::stop_time_update::ScheduleRelationship::Skipped);
                stop.removed_by_alert = true;
            }
        }
    }

    pub(crate) fn push_vehicle(&mut self, vehicle: VehicleDetails) {
        let Some(feed) = vehicle.feed.clone() else {
            return;
        };
        let index = self.vehicles.len();
        if let Some(id) = vehicle.vehicle.id.as_deref() {
            self.vehicle_index.insert(ScopedId::new(&feed, id), index);
        }
        if let Some(trip) = vehicle.trip_id.as_deref() {
            self.vehicles_by_trip
                .entry(ScopedId::new(&feed, trip))
                .or_default()
                .push(index);
        }
        if let Some(route) = vehicle.route_id.as_deref() {
            self.vehicles_by_route
                .entry(ScopedId::new(&feed, route))
                .or_default()
                .push(index);
        }
        self.vehicles.push(vehicle);
    }

    pub(crate) fn push_alert(&mut self, alert: AlertDetails) -> usize {
        let index = self.alerts.len();
        let feed = alert.feed.clone();
        for informed in &alert.informed {
            let Some(feed) = feed.as_ref() else { continue };
            if let Some(trip) = informed.trip_id.as_deref() {
                push(&mut self.alerts_by_trip, ScopedId::new(feed, trip), index);
            }
            if let Some(stop) = informed.stop_id.as_deref() {
                push(&mut self.alerts_by_stop, ScopedId::new(feed, stop), index);
            }
            if let Some(route) = informed.route_id.as_deref() {
                push(&mut self.alerts_by_route, ScopedId::new(feed, route), index);
            }
            if let Some(agency) = informed.agency_id.as_deref() {
                push(
                    &mut self.alerts_by_agency,
                    ScopedId::new(feed, agency),
                    index,
                );
            }
            if let Some(route_type) = informed.route_type {
                push(
                    &mut self.alerts_by_route_type,
                    (feed.clone(), route_type),
                    index,
                );
            }
        }
        self.alerts.push(alert);
        index
    }

    /// Also finds an alert from a stop it informs indirectly, as a station's platform.
    pub(crate) fn index_alert_at_stop(&mut self, alert: usize, stop: ScopedId) {
        push(&mut self.alerts_by_stop, stop, alert);
    }

    pub(crate) fn set_feeds(&mut self, feeds: Vec<FeedStatus>) {
        self.feeds = feeds;
    }

    /// A trip's details by its GTFS identity and service date.
    pub fn trip(
        &self,
        trip: &ScopedId,
        date: Option<CalendarDate>,
        start_time: Option<u32>,
    ) -> Option<&TripDetails> {
        let &index = self.trip_index.get(&(trip.clone(), date, start_time))?;
        self.trips.get(index)
    }

    /// A trip's details by its identity in the timetable.
    pub fn trip_by_key(&self, key: TripKey) -> Option<&TripDetails> {
        self.trips_by_key.get(&key).and_then(|&i| self.trips.get(i))
    }

    /// Every update that resolved to a timetable run, oldest first.
    ///
    /// Each `exact_times=0` vehicle on the run has its own, told apart by
    /// [`TripDetails::start_time`]; other runs have at most one.
    pub fn trips_of_run(&self, key: TripKey) -> Vec<&TripDetails> {
        self.trips_of_key
            .get(&key)
            .into_iter()
            .flatten()
            .filter_map(|&i| self.trips.get(i))
            .collect()
    }

    /// All trips with real-time information.
    pub fn trips(&self) -> &[TripDetails] {
        &self.trips
    }

    /// Trips with real-time information on a route.
    pub fn trips_on_route(&self, route: &ScopedId) -> Vec<&TripDetails> {
        Self::lookup_many(&self.trips_by_route, route, &self.trips)
    }

    /// Real-time stop times at a stop, as `(trip, its stop details)`.
    pub fn stop_times_at(&self, stop: &ScopedId) -> Vec<(&TripDetails, &StopTimeDetails)> {
        self.stop_times
            .get(stop)
            .into_iter()
            .flatten()
            .filter_map(|&(trip, position)| {
                let details = self.trips.get(trip)?;
                Some((details, details.stops.get(position)?))
            })
            .collect()
    }

    /// A vehicle by its identifier.
    pub fn vehicle(&self, id: &ScopedId) -> Option<&VehicleDetails> {
        self.vehicle_index
            .get(id)
            .and_then(|&i| self.vehicles.get(i))
    }

    /// All vehicles.
    pub fn vehicles(&self) -> &[VehicleDetails] {
        &self.vehicles
    }

    /// Vehicles running a trip.
    pub fn vehicles_on_trip(&self, trip: &ScopedId) -> Vec<&VehicleDetails> {
        Self::lookup_many(&self.vehicles_by_trip, trip, &self.vehicles)
    }

    /// Vehicles running a route.
    pub fn vehicles_on_route(&self, route: &ScopedId) -> Vec<&VehicleDetails> {
        Self::lookup_many(&self.vehicles_by_route, route, &self.vehicles)
    }

    /// All alerts.
    pub fn alerts(&self) -> &[AlertDetails] {
        &self.alerts
    }

    /// Alerts that inform a trip.
    pub fn alerts_for_trip(&self, trip: &ScopedId) -> Vec<&AlertDetails> {
        Self::lookup_many(&self.alerts_by_trip, trip, &self.alerts)
    }

    /// Alerts that inform a stop.
    pub fn alerts_for_stop(&self, stop: &ScopedId) -> Vec<&AlertDetails> {
        Self::lookup_many(&self.alerts_by_stop, stop, &self.alerts)
    }

    /// Alerts that inform a route.
    pub fn alerts_for_route(&self, route: &ScopedId) -> Vec<&AlertDetails> {
        Self::lookup_many(&self.alerts_by_route, route, &self.alerts)
    }

    /// Alerts that inform an agency.
    pub fn alerts_for_agency(&self, agency: &ScopedId) -> Vec<&AlertDetails> {
        Self::lookup_many(&self.alerts_by_agency, agency, &self.alerts)
    }

    /// Alerts that inform a route type.
    pub fn alerts_for_route_type(&self, feed: &FeedId, route_type: i32) -> Vec<&AlertDetails> {
        self.alerts_by_route_type
            .get(&(feed.clone(), route_type))
            .into_iter()
            .flatten()
            .filter_map(|&i| self.alerts.get(i))
            .collect()
    }

    /// What the overlay did to a trip; a trip with no real-time information is scheduled.
    pub fn trip_state(&self, key: TripKey) -> TripState {
        self.states
            .get(&key)
            .copied()
            .unwrap_or(TripState::Scheduled)
    }

    /// Every feed message's age and staleness.
    pub fn feed_status(&self) -> &[FeedStatus] {
        &self.feeds
    }

    fn lookup_many<'a, T>(
        index: &HashMap<ScopedId, Vec<usize>>,
        key: &ScopedId,
        values: &'a [T],
    ) -> Vec<&'a T> {
        index
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|&i| values.get(i))
            .collect()
    }
}

fn push<K: core::hash::Hash + Eq>(index: &mut HashMap<K, Vec<usize>>, key: K, value: usize) {
    let entry = index.entry(key).or_default();
    if entry.last() != Some(&value) {
        entry.push(value);
    }
}
