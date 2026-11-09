//! Turns a profile's journeys into rider-facing legs.

use valinor_gtfs::{
    Accessibility, AgencyInfo, CompiledTimetable, GtfsRoute, LatLon, RouteInfo, StopInfo, TripInfo,
};
use valinor_gtfs_rt::proto::alert::Effect;
use valinor_gtfs_rt::proto::trip_update::stop_time_update::ScheduleRelationship;
use valinor_gtfs_rt::{
    FeedId, FeedKind, FeedStatus, ScopedId, StopTimeDetails, TripDetails, TripState,
};
use valinor_transit::{Leg, Profile, ProfileEntry, Ride, Seconds, StopEvent, StopId, StopRules};

use crate::plan::TransitRequest;
use crate::response::alerts::{LegSelectorContext, LegStop, leg_alerts};
use crate::response::vehicle::{lower, vehicle_from};
use crate::response::{
    AccessEgressLeg, AgencyResponse, FeedReport, FeedState, Journey, JourneyLeg, PlanResponse,
    RealtimeLevel, RealtimeStatus, RouteResponse, StopRef, StopVisit, TransferLeg, TransitLeg,
    TripResponse, Vehicle, WalkDurations, Window,
};
use crate::state::TransitSnapshot;
use crate::window::calendar_date;

pub(crate) fn build_response<W: Clone>(
    snapshot: &TransitSnapshot,
    request: &TransitRequest,
    profile: Profile<W>,
    direct: Option<Seconds>,
) -> PlanResponse<W> {
    let builder = Builder { snapshot, request };
    let journeys = profile
        .entries
        .into_iter()
        // A journey no faster than going there directly only adds a wait.
        .filter(|entry| {
            let duration = entry.arrival.into_inner() - entry.departure.into_inner();
            direct.is_none_or(|direct| i64::from(duration) <= i64::from(direct.into_inner()))
        })
        .map(|entry| builder.journey(entry))
        .collect();
    let feeds: Vec<FeedReport> = snapshot
        .feed_status_at(request.now)
        .iter()
        .map(feed_report)
        .collect();
    PlanResponse {
        generated_at: request.now,
        window: Window {
            earliest: request.departure,
            latest: request.departure + i64::from(request.window_seconds),
        },
        realtime: RealtimeStatus {
            level: realtime_level(&feeds),
            built_at: snapshot.built_at,
            feeds,
        },
        journeys,
    }
}

fn realtime_level(feeds: &[FeedReport]) -> RealtimeLevel {
    let applied = feeds
        .iter()
        .filter(|feed| feed.state == FeedState::Applied)
        .count();
    match (applied, feeds.len()) {
        (0, _) => RealtimeLevel::Schedule,
        (applied, total) if applied == total => RealtimeLevel::Realtime,
        _ => RealtimeLevel::Degraded,
    }
}

/// The JSON form of one feed's status.
pub(crate) fn feed_report(status: &FeedStatus) -> FeedReport {
    let state = if status.error.is_some() {
        FeedState::Undecodable
    } else if status.stale {
        FeedState::Stale
    } else {
        FeedState::Applied
    };
    FeedReport {
        feed: status.feed.to_string(),
        kind: match status.kind {
            FeedKind::TripUpdates => "trip_updates",
            FeedKind::VehiclePositions => "vehicle_positions",
            FeedKind::Alerts => "alerts",
            FeedKind::Combined => "combined",
        },
        state,
        age_seconds: status.age,
        header_timestamp: status.header_timestamp,
        stale_after_seconds: status.stale_after,
        entities_dropped: status.entities_dropped,
        error: status.error.as_ref().map(ToString::to_string),
    }
}

/// A stop's name by its feed-scoped ID, whatever kind of stop it is.
pub(crate) fn stop_name_of(
    snapshot: &TransitSnapshot,
    feed: &FeedId,
    stop_id: &str,
) -> Option<String> {
    let compiled = snapshot.data.loaded.timetable();
    compiled
        .location_by_gtfs_id(feed.as_str(), stop_id)
        .and_then(|location| compiled.location(location))
        .and_then(|info| info.name.map(str::to_owned))
        .or_else(|| {
            snapshot
                .new_stops
                .iter()
                .find(|stop| stop.id.feed == *feed && &*stop.id.id == stop_id)
                .and_then(new_stop_name)
        })
}

fn new_stop_name(stop: &valinor_gtfs_rt::NewStop) -> Option<String> {
    stop.details
        .stop_name
        .as_ref()
        .and_then(|name| name.translation.first())
        .map(|translation| translation.text.clone())
}

/// One request's context for building a response.
struct Builder<'a> {
    snapshot: &'a TransitSnapshot,
    request: &'a TransitRequest,
}

impl Builder<'_> {
    fn compiled(&self) -> &CompiledTimetable<'_> {
        self.snapshot.data.loaded.timetable()
    }

    fn origin(&self) -> i64 {
        self.snapshot.window.origin_unix
    }

    fn journey<W: Clone>(&self, entry: ProfileEntry<W>) -> Journey<W> {
        let origin = self.origin();
        let departure = origin + i64::from(entry.departure.into_inner());
        let arrival = origin + i64::from(entry.arrival.into_inner());
        let mode = self.request.mode.name();
        let mut legs = Vec::with_capacity(entry.journey.legs.len());
        let mut clock = departure;
        let (mut access, mut transfers, mut egress, mut waiting) = (0_i64, 0_i64, 0_i64, 0_i64);
        for leg in entry.journey.legs {
            match leg {
                Leg::Access { stop, duration } => {
                    let duration = i64::from(duration.into_inner());
                    access += duration;
                    legs.push(JourneyLeg::Access(AccessEgressLeg {
                        mode,
                        duration_seconds: duration,
                        departure: clock,
                        arrival: clock + duration,
                        stop: self.stop_ref(stop),
                    }));
                    clock += duration;
                }
                Leg::Ride(ride) => {
                    let board = origin + i64::from(ride.board_time.into_inner());
                    let alight = origin + i64::from(ride.alight_time.into_inner());
                    let wait = board - clock;
                    waiting += wait;
                    legs.push(JourneyLeg::Transit(Box::new(self.transit_leg(&ride, wait))));
                    clock = alight;
                }
                Leg::Transfer {
                    from,
                    to,
                    duration,
                    walk,
                } => {
                    let duration = i64::from(duration.into_inner());
                    transfers += duration;
                    legs.push(JourneyLeg::Transfer(Box::new(TransferLeg {
                        mode,
                        duration_seconds: duration,
                        departure: clock,
                        arrival: clock + duration,
                        from: self.stop_ref(from),
                        to: self.stop_ref(to),
                        walk: Some(walk),
                    })));
                    clock += duration;
                }
                Leg::Egress { stop, duration } => {
                    let duration = i64::from(duration.into_inner());
                    egress += duration;
                    legs.push(JourneyLeg::Egress(AccessEgressLeg {
                        mode,
                        duration_seconds: duration,
                        departure: clock,
                        arrival: clock + duration,
                        stop: self.stop_ref(stop),
                    }));
                    clock += duration;
                }
            }
        }
        Journey {
            departure,
            arrival,
            duration_seconds: arrival - departure,
            trips: entry.trips,
            walking: WalkDurations {
                access_seconds: access,
                transfer_seconds: transfers,
                egress_seconds: egress,
                total_seconds: access + transfers + egress,
            },
            wait_seconds: waiting,
            legs,
        }
    }

    fn stop_ref(&self, stop: StopId) -> StopRef {
        let compiled = self.compiled();
        if let Some(info) = compiled.stop(stop) {
            return self.static_stop_ref(info);
        }
        let new = self.snapshot.new_stops.iter().find(|s| s.stop == stop);
        StopRef {
            stop_id: new.map_or_else(|| stop.to_string(), |s| s.id.id.to_string()),
            name: new.and_then(new_stop_name),
            code: new.and_then(|s| first_text(s.details.stop_code.as_ref())),
            platform: new.and_then(|s| first_text(s.details.platform_code.as_ref())),
            latitude: new.map(|s| f64::from(s.latitude)),
            longitude: new.map(|s| f64::from(s.longitude)),
            wheelchair_boarding: new.and_then(|s| match s.details.wheelchair_boarding() {
                valinor_gtfs_rt::proto::stop::WheelchairBoarding::Available => Some("yes"),
                valinor_gtfs_rt::proto::stop::WheelchairBoarding::NotAvailable => Some("no"),
                valinor_gtfs_rt::proto::stop::WheelchairBoarding::Unknown => None,
            }),
            parent_station: None,
            parent_station_name: None,
            time_zone: new.and_then(|s| s.details.stop_timezone.clone()),
        }
    }

    fn static_stop_ref(&self, info: StopInfo<'_>) -> StopRef {
        let compiled = self.compiled();
        let parent = info.parent_station.and_then(|id| compiled.location(id));
        StopRef {
            stop_id: info.gtfs_id.to_owned(),
            name: info.name.map(str::to_owned),
            code: info.code.map(str::to_owned),
            platform: info.platform.map(str::to_owned),
            latitude: info.coordinate.map(|c| c.lat),
            longitude: info.coordinate.map(|c| c.lon),
            wheelchair_boarding: accessibility(info.wheelchair_boarding),
            parent_station: parent.map(|p| p.gtfs_id.to_owned()),
            parent_station_name: parent.and_then(|p| p.name.map(str::to_owned)),
            time_zone: Some(info.time_zone.to_owned()),
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one linear assembly of a leg from its sources"
    )]
    fn transit_leg(&self, ride: &Ride, wait: i64) -> TransitLeg {
        let compiled = self.compiled();
        let rider = &self.snapshot.rider;
        let static_trip = compiled.trip(ride.trip.trip);
        let details = rider.trip_by_key(ride.trip);
        // Static positions are valid only when the ride is on the trip's own stop pattern.
        let on_base_pattern = static_trip.is_some_and(|info| info.pattern == ride.route);
        let gtfs_route = gtfs_route_of(compiled, static_trip, details);
        let route = gtfs_route.and_then(|route| compiled.route(route));
        let agency = route
            .and_then(|r| r.agency)
            .and_then(|agency| compiled.agency(agency));
        let feed = self.feed_of(route, details);
        let trip_id = static_trip
            .map(|info| info.gtfs_id.to_owned())
            .or_else(|| details.map(|d| d.trip_id.clone()))
            .unwrap_or_default();

        let events = self
            .snapshot
            .snapshot
            .lane_trip_events(ride.lane, ride.row)
            .unwrap_or_default();
        let route_stops = self
            .snapshot
            .snapshot
            .route_stops(ride.route)
            .unwrap_or(&[]);
        let day_start = self.snapshot.window.day_start_unix(ride.trip.date);

        let mut stops = Vec::new();
        for position in ride.board_position..=ride.alight_position {
            let index = position as usize;
            let (Some(stop), Some(event)) = (route_stops.get(index), events.get(index)) else {
                continue;
            };
            let context = VisitContext {
                feed: feed.as_ref(),
                static_trip: static_trip.filter(|_| on_base_pattern),
                details,
                day_start,
            };
            stops.push(self.stop_visit(&context, index, *stop, *event));
        }

        let board_unix = self.origin() + i64::from(ride.board_time.into_inner());
        let alight_unix = self.origin() + i64::from(ride.alight_time.into_inner());
        let headway_based = static_trip.map(|info| info.headway_based);

        let vehicle = feed.as_ref().and_then(|feed| {
            self.vehicle_for(
                feed,
                &trip_id,
                ride,
                details,
                headway_based.unwrap_or(false),
            )
        });

        let leg_stops: Vec<LegStop<'_>> = (ride.board_position..=ride.alight_position)
            .filter_map(|position| {
                let stop = *route_stops.get(position as usize)?;
                Some(LegStop {
                    stop,
                    gtfs_id: self.stop_gtfs_id_of(stop),
                })
            })
            .collect();
        let alerts = match feed.as_ref() {
            Some(feed) => leg_alerts(
                compiled,
                rider,
                &LegSelectorContext {
                    feed,
                    agency_id: agency.and_then(|a| a.gtfs_id),
                    route_id: route.map(|r| r.gtfs_id),
                    route_type: route.map(|r| i32::from(r.route_type)),
                    direction_id: static_trip
                        .and_then(|i| i.direction)
                        .map(u32::from)
                        .or_else(|| details.and_then(|d| d.direction_id)),
                    trip_id: &trip_id,
                    date: calendar_date(ride.trip.date),
                    stops: leg_stops,
                    from: board_unix,
                    to: alight_unix,
                },
            ),
            None => Vec::new(),
        };

        let shape = on_base_pattern
            .then(|| {
                compiled.shape_between(
                    ride.trip.trip,
                    ride.board_position as usize,
                    ride.alight_position as usize,
                )
            })
            .flatten()
            .map(|slice| encode_polyline6(&slice.points()));

        let realtime = stops
            .iter()
            .any(|s| s.predicted_arrival.is_some() || s.predicted_departure.is_some());
        let delay_seconds = stops.first().and_then(|s| s.departure_delay_seconds);

        TransitLeg {
            departure: board_unix,
            arrival: alight_unix,
            duration_seconds: alight_unix - board_unix,
            wait_seconds: wait,
            delay_seconds,
            realtime,
            route: route.map(route_response),
            agency: agency.map(agency_response),
            trip: trip_response(
                ride,
                &trip_id,
                static_trip,
                details,
                feed.as_ref(),
                self.snapshot,
            ),
            stops,
            shape,
            vehicle,
            alerts,
        }
    }

    /// The feed a trip belongs to, from its route or its real-time record.
    fn feed_of(
        &self,
        route: Option<RouteInfo<'_>>,
        details: Option<&TripDetails>,
    ) -> Option<FeedId> {
        details.and_then(|d| d.feed.clone()).or_else(|| {
            route.and_then(|r| {
                self.snapshot
                    .data
                    .feeds
                    .iter()
                    .find(|f| f.as_str() == r.feed)
                    .cloned()
            })
        })
    }

    fn stop_gtfs_id_of(&self, stop: StopId) -> Option<&str> {
        self.compiled()
            .stop(stop)
            .map(|info| info.gtfs_id)
            .or_else(|| {
                self.snapshot
                    .new_stops
                    .iter()
                    .find(|n| n.stop == stop)
                    .map(|n| &*n.id.id)
            })
    }

    fn stop_visit(
        &self,
        visit: &VisitContext<'_>,
        index: usize,
        stop: StopId,
        event: StopEvent,
    ) -> StopVisit {
        let origin = self.origin();
        let live = visit.details.and_then(|d| {
            d.stops
                .iter()
                .find(|s| s.position == u32::try_from(index).ok())
        });
        let scheduled = visit.scheduled(index).or_else(|| {
            let live = live?;
            Some((live.scheduled_arrival?, live.scheduled_departure?))
        });
        let unix_of = |time: i32| origin + i64::from(time);
        // A trip that exists only in real-time data has no schedule; its times are predictions.
        let added_only = visit.static_trip.is_none() && live.is_none() && visit.details.is_some();
        let predicted_arrival = live
            .and_then(|s| s.predicted_arrival)
            .or_else(|| added_only.then(|| unix_of(event.arrival.into_inner())));
        let predicted_departure = live
            .and_then(|s| s.predicted_departure)
            .or_else(|| added_only.then(|| unix_of(event.departure.into_inner())));
        let arrival = unix_of(event.arrival.into_inner());
        let departure = unix_of(event.departure.into_inner());

        let stop_ref = self.stop_ref(stop);
        let (skipped, removed_by_alert) =
            self.skip_marks(visit, index, live, event, &stop_ref, (arrival, departure));

        let approximate = visit
            .static_trip
            .and_then(|info| info.time_is_approximate_at(index))
            .filter(|&approximate| approximate);

        let headsign = live.and_then(|s| s.stop_headsign.clone()).or_else(|| {
            let info = visit.static_trip?;
            let text = self.compiled().stop_headsign(&info, index)?;
            (Some(text) != info.headsign).then(|| text.to_owned())
        });

        let (pickup_type, drop_off_type) = visit.static_trip.map_or((None, None), |info| {
            (
                pickup_or_drop_off(info.pickup_type_at(index)),
                pickup_or_drop_off(info.drop_off_type_at(index)),
            )
        });

        StopVisit {
            stop: stop_ref,
            stop_sequence: live.and_then(|s| s.stop_sequence).or_else(|| {
                visit
                    .static_trip
                    .and_then(|info| info.stop_sequence_at(index))
            }),
            arrival,
            departure,
            scheduled_arrival: scheduled.map(|(a, _)| a),
            scheduled_departure: scheduled.map(|(_, d)| d),
            predicted_arrival,
            predicted_departure,
            arrival_delay_seconds: live
                .and_then(|s| s.arrival_delay)
                .or_else(|| delay_between(predicted_arrival, scheduled.map(|(a, _)| a))),
            departure_delay_seconds: live
                .and_then(|s| s.departure_delay)
                .or_else(|| delay_between(predicted_departure, scheduled.map(|(_, d)| d))),
            arrival_uncertainty_seconds: live.and_then(|s| s.arrival_uncertainty),
            departure_uncertainty_seconds: live.and_then(|s| s.departure_uncertainty),
            approximate,
            schedule_relationship: live
                .and_then(|s| s.relationship)
                .map(|relationship| lower(relationship.as_str_name())),
            skipped,
            removed_by_alert,
            pickup_type,
            drop_off_type,
            headsign,
            assigned_stop_id: live.and_then(|s| s.assigned_stop_id.clone()),
            occupancy: live
                .and_then(|s| s.occupancy)
                .map(|occupancy| lower(occupancy.as_str_name())),
            utc_offset_seconds: self.compiled().stop_utc_offset_at(stop, departure),
        }
    }

    /// Whether a stop is skipped, and whether an alert is why.
    fn skip_marks(
        &self,
        visit: &VisitContext<'_>,
        index: usize,
        live: Option<&StopTimeDetails>,
        event: StopEvent,
        stop_ref: &StopRef,
        times: (i64, i64),
    ) -> (bool, bool) {
        if let Some(live) = live {
            let skipped = live.relationship == Some(ScheduleRelationship::Skipped);
            return (skipped, live.removed_by_alert);
        }
        // Without a stop time update, a `SKIPPED` event can only come from a `NO_SERVICE` alert
        // on this stop, unless the schedule itself forbids both boarding and alighting.
        let Some(info) = visit.static_trip else {
            return (false, false);
        };
        if event.rules != StopRules::SKIPPED {
            return (false, false);
        }
        let scheduled_as_skip =
            info.pickup_type_at(index) == 1 && info.drop_off_type_at(index) == 1;
        if scheduled_as_skip {
            return (false, false);
        }
        let removed_by_alert = visit.feed.is_some_and(|feed| {
            self.snapshot
                .rider
                .alerts_for_stop(&ScopedId::new(feed, &stop_ref.stop_id))
                .iter()
                .any(|alert| {
                    alert.effect == Some(Effect::NoService)
                        && alert
                            .active_periods
                            .iter()
                            .chain(&alert.impact_periods)
                            .any(|period| {
                                period.start.is_none_or(|start| start <= times.1)
                                    && period.end.is_none_or(|end| times.0 < end)
                            })
                })
        });
        (true, removed_by_alert)
    }

    /// The vehicle running a ride.
    ///
    /// Vehicles are matched by run, and headway-based runs by the real-time descriptor's
    /// `start_time`, because two vehicles can resolve to one grid run.
    fn vehicle_for(
        &self,
        feed: &FeedId,
        trip_id: &str,
        ride: &Ride,
        details: Option<&TripDetails>,
        headway_based: bool,
    ) -> Option<Vehicle> {
        let rider = &self.snapshot.rider;
        let candidates: Vec<_> = rider
            .vehicles_on_trip(&ScopedId::new(feed, trip_id))
            .into_iter()
            .filter(|vehicle| vehicle.trip_key == Some(ride.trip))
            .collect();
        let chosen = if headway_based {
            match details.and_then(|d| d.start_time) {
                Some(start_time) => candidates
                    .iter()
                    .copied()
                    .find(|vehicle| vehicle.start_time == Some(start_time)),
                // Nothing says which vehicle this grid run is, so don't guess between several.
                None if candidates.len() == 1 => candidates.first().copied(),
                None => None,
            }
        } else {
            candidates.first().copied()
        };
        // Feeds can send an empty descriptor; only one that names the vehicle counts.
        let reference = details
            .and_then(|d| d.vehicle.as_ref())
            .filter(|v| v.id.is_some() || v.label.is_some() || v.license_plate.is_some());
        let chosen = chosen.or_else(|| {
            let id = reference?.id.as_deref()?;
            rider.vehicle(&ScopedId::new(feed, id))
        });
        match (chosen, reference) {
            (None, None) => None,
            (chosen, reference) => Some(vehicle_from(self.snapshot, chosen, reference)),
        }
    }
}

/// What a stop visit needs to know about its ride.
struct VisitContext<'a> {
    feed: Option<&'a FeedId>,
    /// Set only when the ride is on the trip's own pattern, so positions agree.
    static_trip: Option<TripInfo<'a>>,
    details: Option<&'a TripDetails>,
    day_start: Option<i64>,
}

impl VisitContext<'_> {
    /// Scheduled arrival and departure from the compiled file, in POSIX seconds.
    fn scheduled(&self, index: usize) -> Option<(i64, i64)> {
        let info = self.static_trip?;
        let day_start = self.day_start?;
        let (arrival, departure) = info.time_at(index)?;
        Some((
            day_start + i64::from(arrival),
            day_start + i64::from(departure),
        ))
    }
}

fn gtfs_route_of(
    compiled: &CompiledTimetable<'_>,
    static_trip: Option<TripInfo<'_>>,
    details: Option<&TripDetails>,
) -> Option<GtfsRoute> {
    if let Some(info) = static_trip {
        return Some(info.route);
    }
    let details = details?;
    let route = details
        .route
        .and_then(|route| compiled.pattern(route))
        .map(|pattern| pattern.route);
    route.or_else(|| {
        let feed = details.feed.as_ref()?;
        compiled.route_by_gtfs_id(feed.as_str(), details.route_id.as_deref()?)
    })
}

fn route_response(route: RouteInfo<'_>) -> RouteResponse {
    RouteResponse {
        route_id: route.gtfs_id.to_owned(),
        short_name: route.short_name.map(str::to_owned),
        long_name: route.long_name.map(str::to_owned),
        description: route.description.map(str::to_owned),
        route_type: route.route_type,
        color: route.color.map(hex),
        text_color: route.text_color.map(hex),
        url: route.url.map(str::to_owned),
    }
}

fn agency_response(agency: AgencyInfo<'_>) -> AgencyResponse {
    AgencyResponse {
        agency_id: agency.gtfs_id.map(str::to_owned),
        name: agency.name.to_owned(),
        url: agency.url.map(str::to_owned),
        phone: agency.phone.map(str::to_owned),
        fare_url: agency.fare_url.map(str::to_owned),
    }
}

fn trip_response(
    ride: &Ride,
    trip_id: &str,
    static_trip: Option<TripInfo<'_>>,
    details: Option<&TripDetails>,
    feed: Option<&FeedId>,
    snapshot: &TransitSnapshot,
) -> TripResponse {
    let state = snapshot.rider.trip_state(ride.trip);
    TripResponse {
        feed: feed.map(ToString::to_string),
        trip_id: trip_id.to_owned(),
        start_date: calendar_date(ride.trip.date).map(|date| date.to_string()),
        start_time: details.and_then(|d| d.start_time),
        headsign: details
            .and_then(|d| d.headsign.clone())
            .or_else(|| static_trip.and_then(|i| i.headsign.map(str::to_owned))),
        direction_id: static_trip
            .and_then(|i| i.direction)
            .map(u32::from)
            .or_else(|| details.and_then(|d| d.direction_id)),
        block_id: static_trip.and_then(|i| i.block_id.map(str::to_owned)),
        headway_based: static_trip.map(|i| i.headway_based).filter(|&based| based),
        wheelchair_accessible: static_trip.and_then(|i| accessibility(i.wheelchair_accessible)),
        bikes_allowed: static_trip.and_then(|i| accessibility(i.bikes_allowed)),
        schedule_relationship: details
            .and_then(|d| d.relationship)
            .map(|relationship| lower(relationship.as_str_name())),
        realtime_state: match state {
            TripState::Scheduled | TripState::Canceled | TripState::NoService => None,
            TripState::Delayed => Some("delayed"),
            TripState::StopSkipped => Some("stop_skipped"),
            TripState::Added => Some("added"),
            TripState::Replaced => Some("replaced"),
        },
        delay_seconds: details.and_then(|d| d.delay),
        updated_at: details.and_then(|d| d.timestamp),
    }
}

fn accessibility(value: Accessibility) -> Option<&'static str> {
    match value {
        Accessibility::Unknown => None,
        Accessibility::Yes => Some("yes"),
        Accessibility::No => Some("no"),
    }
}

/// The name of a `pickup_type` or `drop_off_type` that isn't regular.
fn pickup_or_drop_off(value: u8) -> Option<&'static str> {
    match value {
        0 => None,
        1 => Some("none"),
        2 => Some("phone_agency"),
        3 => Some("coordinate_with_driver"),
        _ => Some("unknown"),
    }
}

fn delay_between(predicted: Option<i64>, scheduled: Option<i64>) -> Option<i32> {
    i32::try_from(predicted? - scheduled?).ok()
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("{r:02X}{g:02X}{b:02X}")
}

fn first_text(text: Option<&valinor_gtfs_rt::proto::TranslatedString>) -> Option<String> {
    text?.translation.first().map(|t| t.text.clone())
}

/// Encodes coordinates as a polyline with six decimal places, as Valhalla does.
fn encode_polyline6(points: &[LatLon]) -> String {
    let mut encoded = String::with_capacity(points.len() * 8);
    let (mut last_lat, mut last_lon) = (0_i64, 0_i64);
    for point in points {
        let lat = to_polyline_units(point.lat);
        let lon = to_polyline_units(point.lon);
        encode_value(lat - last_lat, &mut encoded);
        encode_value(lon - last_lon, &mut encoded);
        (last_lat, last_lon) = (lat, lon);
    }
    encoded
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a coordinate times a million is far inside i64"
)]
fn to_polyline_units(degrees: f64) -> i64 {
    (degrees * 1e6).round() as i64
}

fn encode_value(value: i64, out: &mut String) {
    let mut value = (value << 1) ^ (value >> 63);
    loop {
        let mut chunk = u32::try_from(value & 0x1f).unwrap_or(0);
        value >>= 5;
        if value != 0 {
            chunk |= 0x20;
        }
        out.push(char::from_u32(chunk + 63).unwrap_or('?'));
        if value == 0 {
            break;
        }
    }
}
