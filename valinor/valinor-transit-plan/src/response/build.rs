//! Turns a profile's journeys into rider-facing legs.

use std::collections::HashMap;

use valinor_gtfs::{
    Accessibility as GtfsAccessibility, AgencyInfo, GtfsRoute, LatLon, RouteInfo, StopInfo,
    TimetablePart, TimetableSet, TripInfo, TripSegmentInfo,
};
use valinor_gtfs_rt::proto::alert::Effect;
use valinor_gtfs_rt::proto::trip_update::stop_time_update::ScheduleRelationship;
use valinor_gtfs_rt::{
    FeedId, FeedKind, FeedStatus, ScopedId, StopTimeDetails, TripDetails, TripState,
};
use valinor_transit::{Leg, Profile, ProfileEntry, Ride, StopEvent, StopId, StopRules, TripKey};

use crate::plan::{
    AccessEgressRequest, AccessEgressSearch, DirectSearch, PlanStage, TransitRequest, TravelMode,
};
use crate::response::alerts::{LegSelectorContext, LegStop, ServiceRun, leg_alerts};
use crate::response::vehicle::{lower, vehicle_from};
use crate::response::{
    AccessEgressLeg, Accessibility, AgencyResponse, DirectResponse, FeedReport, FeedState, Journey,
    JourneyLeg, LegAlert, NoJourneysReason, PlanResponse, RealtimeLevel, RealtimeStatus,
    RideSegment, RouteResponse, StopMovedNotice, StopRef, StopVisit, TransferLeg, TransitLeg,
    TripResponse, Vehicle, WalkDurations, WalkPath, Window,
};
use crate::state::TransitSnapshot;
use crate::window::calendar_date;

pub(crate) fn build_response<A: AccessEgressSearch>(
    snapshot: &TransitSnapshot,
    request: &TransitRequest,
    profile: Profile<A::Walk>,
    direct: &DirectSearch,
    access_egress: &mut A,
) -> Result<PlanResponse, A::Error> {
    let found = profile.entries.len();
    let mut builder = Builder {
        snapshot,
        request,
        access_egress,
        access_paths: HashMap::new(),
        egress_paths: HashMap::new(),
    };
    let mut journeys = Vec::new();
    for entry in profile.entries {
        // A journey no faster than going there directly only adds a wait.
        let duration = entry.arrival.into_inner() - entry.departure.into_inner();
        if let DirectSearch::Found(direct) = direct
            && i64::from(duration) > i64::from(direct.into_inner())
        {
            continue;
        }
        journeys.push(builder.journey(entry)?);
    }
    let reason = match (journeys.is_empty(), found) {
        (false, _) => None,
        (true, 0) => Some(NoJourneysReason::NoService),
        (true, _) => Some(NoJourneysReason::SlowerThanDirect),
    };
    Ok(assemble(snapshot, request, journeys, reason, direct))
}

/// A response with no journeys, for a query that found nothing to search.
pub(crate) fn empty_response(
    snapshot: &TransitSnapshot,
    request: &TransitRequest,
    reason: NoJourneysReason,
    direct: &DirectSearch,
) -> PlanResponse {
    assemble(snapshot, request, Vec::new(), Some(reason), direct)
}

fn assemble(
    snapshot: &TransitSnapshot,
    request: &TransitRequest,
    journeys: Vec<Journey>,
    no_journeys_reason: Option<NoJourneysReason>,
    direct: &DirectSearch,
) -> PlanResponse {
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
        no_journeys_reason,
        direct: direct_response(direct),
    }
}

fn direct_response(direct: &DirectSearch) -> DirectResponse {
    let status = match direct {
        DirectSearch::Found(_) => "found",
        DirectSearch::NoRoute => "no_route",
        DirectSearch::NotSearched => "not_searched",
        DirectSearch::Failed(_) => "failed",
    };
    DirectResponse {
        status,
        seconds: match direct {
            DirectSearch::Found(seconds) => Some(seconds.into_inner()),
            _ => None,
        },
        error: match direct {
            DirectSearch::Failed(error) => Some(error.clone()),
            _ => None,
        },
    }
}

/// Whether Ilúvatar can narrate this language.
pub(crate) fn language_supported(language: &str) -> bool {
    illuvatar::is_supported_language(language)
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
    let part = snapshot.data.set.part_of_feed(feed.as_str());
    part.and_then(|part| {
        let compiled = part.compiled();
        compiled
            .location_by_gtfs_id(feed.as_str(), stop_id)
            .and_then(|location| compiled.location(location))
            .and_then(|info| info.name.map(str::to_owned))
    })
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
struct Builder<'a, A: AccessEgressSearch> {
    snapshot: &'a TransitSnapshot,
    request: &'a TransitRequest,
    access_egress: &'a mut A,
    /// Expanded once per stop: many journeys of a window share their access and egress.
    access_paths: HashMap<StopId, Option<WalkPath>>,
    egress_paths: HashMap<StopId, Option<WalkPath>>,
}

impl<A: AccessEgressSearch> Builder<'_, A> {
    fn set(&self) -> &TimetableSet {
        &self.snapshot.data.set
    }

    fn origin(&self) -> i64 {
        self.snapshot.window.origin_unix
    }

    fn access_path(&mut self, stop: StopId) -> Result<Option<WalkPath>, A::Error> {
        if let Some(path) = self.access_paths.get(&stop) {
            return Ok(path.clone());
        }
        let snapshot = self.snapshot;
        let request = AccessEgressRequest {
            origin: self.request.origin,
            destination: self.request.destination,
            mode: self.request.mode,
            snapshot,
        };
        let path = self.access_egress.access_path(&request, stop)?;
        self.access_egress.enter_stage(PlanStage::Assemble);
        self.access_paths.insert(stop, path.clone());
        Ok(path)
    }

    fn egress_path(&mut self, stop: StopId) -> Result<Option<WalkPath>, A::Error> {
        if let Some(path) = self.egress_paths.get(&stop) {
            return Ok(path.clone());
        }
        let snapshot = self.snapshot;
        let request = AccessEgressRequest {
            origin: self.request.origin,
            destination: self.request.destination,
            mode: self.request.mode,
            snapshot,
        };
        let path = self.access_egress.egress_path(&request, stop)?;
        self.access_egress.enter_stage(PlanStage::Assemble);
        self.egress_paths.insert(stop, path.clone());
        Ok(path)
    }

    /// What is known about a walking or cycling leg: a wheelchair search routes on streets
    /// that allow a wheelchair; a search on foot or by bicycle says nothing about access.
    fn walking_accessibility(&self) -> Accessibility {
        if self.request.mode == TravelMode::Wheelchair {
            Accessibility::Accessible
        } else {
            Accessibility::Unknown
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one linear assembly of a journey's legs"
    )]
    fn journey(&mut self, entry: ProfileEntry<A::Walk>) -> Result<Journey, A::Error> {
        let origin = self.origin();
        let departure = origin + i64::from(entry.departure.into_inner());
        let arrival = origin + i64::from(entry.arrival.into_inner());
        let mode = self.request.mode.name();
        let walking_accessibility = self.walking_accessibility();
        let mut legs = Vec::with_capacity(entry.journey.legs.len());
        let mut clock = departure;
        let (mut access, mut transfers, mut egress, mut waiting) = (0_i64, 0_i64, 0_i64, 0_i64);
        for leg in entry.journey.legs {
            match leg {
                Leg::Access { stop, duration } => {
                    let duration = i64::from(duration.into_inner());
                    access += duration;
                    let walk = self.access_path(stop)?;
                    legs.push(JourneyLeg::Access(AccessEgressLeg {
                        mode,
                        accessibility: walking_accessibility,
                        duration_seconds: duration,
                        departure: clock,
                        arrival: clock + duration,
                        stop: self.stop_ref(stop),
                        walk,
                        stop_moved: Vec::new(),
                    }));
                    clock += duration;
                }
                Leg::Ride(ride) => {
                    let board = origin + i64::from(ride.board_time.into_inner());
                    let alight = origin + i64::from(ride.alight_time.into_inner());
                    let wait = board - clock;
                    waiting += wait;
                    self.access_egress.enter_stage(PlanStage::TransitLeg);
                    let leg = self.transit_leg(&ride, wait);
                    self.access_egress.enter_stage(PlanStage::Assemble);
                    legs.push(JourneyLeg::Transit(Box::new(leg)));
                    clock = alight;
                }
                Leg::Transfer {
                    from,
                    to,
                    duration,
                    walk,
                } => {
                    // The walk's own seconds: any more the transfer cost is a wait to board,
                    // which the next ride's `wait_seconds` shows.
                    let duration = self
                        .access_egress
                        .transfer_seconds(&walk)
                        .map_or_else(|| i64::from(duration.into_inner()), i64::from);
                    transfers += duration;
                    let path = {
                        let snapshot = self.snapshot;
                        let request = AccessEgressRequest {
                            origin: self.request.origin,
                            destination: self.request.destination,
                            mode: self.request.mode,
                            snapshot,
                        };
                        self.access_egress
                            .transfer_path(&request, &walk, from, to)?
                    };
                    self.access_egress.enter_stage(PlanStage::Assemble);
                    legs.push(JourneyLeg::Transfer(Box::new(TransferLeg {
                        mode,
                        accessibility: walking_accessibility,
                        duration_seconds: duration,
                        departure: clock,
                        arrival: clock + duration,
                        from: self.stop_ref(from),
                        to: self.stop_ref(to),
                        walk: path,
                        stop_moved: Vec::new(),
                    })));
                    clock += duration;
                }
                Leg::Egress { stop, duration } => {
                    let duration = i64::from(duration.into_inner());
                    egress += duration;
                    let walk = self.egress_path(stop)?;
                    legs.push(JourneyLeg::Egress(AccessEgressLeg {
                        mode,
                        accessibility: walking_accessibility,
                        duration_seconds: duration,
                        departure: clock,
                        arrival: clock + duration,
                        stop: self.stop_ref(stop),
                        walk,
                        stop_moved: Vec::new(),
                    }));
                    clock += duration;
                }
            }
        }
        mark_moved_stop_walks(&mut legs);
        Ok(Journey {
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
        })
    }

    fn stop_ref(&self, stop: StopId) -> StopRef {
        if let Some((part, local)) = self.set().stop_part(stop)
            && let Some(info) = part.compiled().stop(local)
        {
            return Self::static_stop_ref(part, info);
        }
        let new = self.snapshot.new_stops.iter().find(|s| s.stop == stop);
        StopRef {
            accessibility: new.map_or(Accessibility::Unknown, |s| {
                match s.details.wheelchair_boarding() {
                    valinor_gtfs_rt::proto::stop::WheelchairBoarding::Available => {
                        Accessibility::Accessible
                    }
                    valinor_gtfs_rt::proto::stop::WheelchairBoarding::NotAvailable => {
                        Accessibility::Inaccessible
                    }
                    valinor_gtfs_rt::proto::stop::WheelchairBoarding::Unknown => {
                        Accessibility::Unknown
                    }
                }
            }),
            feed: new.map(|s| s.id.feed.to_string()),
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

    fn static_stop_ref(part: TimetablePart<'_>, info: StopInfo<'_>) -> StopRef {
        let parent = info
            .parent_station
            .and_then(|id| part.compiled().location(id));
        StopRef {
            accessibility: accessibility_of(info.wheelchair_boarding),
            feed: Some(info.feed.to_owned()),
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
        let set = self.set();
        let rider = &self.snapshot.rider;
        let trip_part = set.trip_part(ride.trip.trip);
        let static_trip = trip_part.and_then(|(part, local)| part.compiled().trip(local));
        // The original trip this ride boards, on a combined stay-seated trip.
        let trip_segments = trip_part
            .map(|(part, local)| part.compiled().trip_segments(local))
            .unwrap_or_default();
        let boarded_segment = trip_segments
            .iter()
            .find(|s| {
                s.first_position <= ride.board_position && ride.board_position <= s.last_position
            })
            .copied();
        // A combined trip's update is about one original trip, so a leg on another reads nothing
        // from it: `details` belongs to the stretch the rider boards.
        let details = match boarded_segment {
            Some(segment) => rider.trip_details_named(ride.trip, segment.gtfs_id),
            None => rider.trip_by_key(ride.trip),
        };
        // The stretches this ride actually rides, in order; empty for a ride on one plain trip.
        let ridden_segments: Vec<TripSegmentInfo<'_>> = trip_segments
            .iter()
            .filter(|s| {
                s.first_position <= ride.alight_position && ride.board_position <= s.last_position
            })
            .copied()
            .collect();
        // Static positions are valid only when the ride is on the trip's own stop pattern.
        let on_base_pattern = trip_part
            .zip(static_trip)
            .is_some_and(|((part, _), info)| part.global_route(info.pattern) == ride.route);
        // Which file answers for this leg: its trip's, or for a trip only real-time data knows,
        // the file of its feed or of its route.
        let part = trip_part
            .map(|(part, _)| part)
            .or_else(|| {
                details
                    .and_then(|d| d.feed.as_ref())
                    .and_then(|feed| set.part_of_feed(feed.as_str()))
            })
            .or_else(|| set.route_part(ride.route).map(|(part, _)| part));
        let gtfs_route = boarded_segment
            .map(|segment| segment.route)
            .or_else(|| part.and_then(|part| gtfs_route_of(set, part, static_trip, details)));
        let route = part
            .zip(gtfs_route)
            .and_then(|(part, route)| part.compiled().route(route));
        let agency = part
            .zip(route.and_then(|r| r.agency))
            .and_then(|(part, agency)| part.compiled().agency(agency));
        let feed = self.feed_of(route, details);
        let trip_id = boarded_segment
            .map(|segment| segment.gtfs_id.to_owned())
            .or_else(|| static_trip.map(|info| info.gtfs_id.to_owned()))
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
        let day_start = part.and_then(|part| {
            self.snapshot
                .window
                .day_start_unix(part.index(), ride.trip.date)
        });

        let mut stops = Vec::new();
        // For each ridden position, its index in `stops`, where a same-stop join appears once.
        let mut stop_indices = Vec::new();
        for position in ride.board_position..=ride.alight_position {
            let index = position as usize;
            let (Some(stop), Some(event)) = (route_stops.get(index), events.get(index)) else {
                continue;
            };
            let context = VisitContext {
                feed: feed.as_ref(),
                part,
                static_trip: static_trip.filter(|_| on_base_pattern),
                details,
                day_start,
                key: ride.trip,
            };
            let visit = self.stop_visit(&context, index, *stop, *event);
            // Where one original trip ends and the next starts at the same stop,
            // a rider sees one stop: the arrival of the first and the departure of the next.
            let joins_previous = stops.last().is_some_and(|previous: &StopVisit| {
                previous.stop.stop_id == visit.stop.stop_id
                    && previous.stop.feed == visit.stop.feed
                    && is_segment_start(&trip_segments, position)
            });
            stop_indices.push(stops.len().saturating_sub(usize::from(joins_previous)));
            if joins_previous {
                if let Some(previous) = stops.last_mut() {
                    previous.departure = visit.departure;
                    previous.scheduled_departure = visit.scheduled_departure;
                    previous.predicted_departure = visit.predicted_departure;
                    previous.departure_delay_seconds = visit.departure_delay_seconds;
                    previous.departure_uncertainty_seconds = visit.departure_uncertainty_seconds;
                }
            } else {
                stops.push(visit);
            }
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
            .or_else(|| {
                // On a combined trip the vehicle may be reporting another original trip's ID.
                trip_segments
                    .iter()
                    .find_map(|segment| self.vehicle_for(feed, segment.gtfs_id, ride, None, false))
            })
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
        let alerts = match (feed.as_ref(), part) {
            (Some(feed), Some(part)) if !trip_segments.is_empty() => {
                let mut alerts: Vec<LegAlert> = Vec::new();
                for segment in &trip_segments {
                    let from = segment.first_position.max(ride.board_position);
                    let to = segment.last_position.min(ride.alight_position);
                    if from > to {
                        continue;
                    }
                    let stops: Vec<LegStop<'_>> = (from..=to)
                        .filter_map(|position| {
                            let stop = *route_stops.get(position as usize)?;
                            Some(LegStop {
                                stop,
                                gtfs_id: self.stop_gtfs_id_of(stop),
                            })
                        })
                        .collect();
                    let route = part.compiled().route(segment.route);
                    let found = leg_alerts(
                        part,
                        rider,
                        &LegSelectorContext {
                            feed,
                            agency_id: agency.and_then(|a| a.gtfs_id),
                            route_id: route.map(|r| r.gtfs_id),
                            route_type: route.map(|r| i32::from(r.route_type)),
                            direction_id: segment.direction.map(u32::from),
                            trip_id: segment.gtfs_id,
                            date: calendar_date(ride.trip.date),
                            run: Some(ServiceRun {
                                trip: static_trip.map(|_| ride.trip.trip),
                                date: ride.trip.date,
                                start_time: static_trip
                                    .and_then(|info| info.time_at(segment.first_position as usize))
                                    .and_then(|(_, first)| u32::try_from(first).ok()),
                            }),
                            stops,
                            from: board_unix,
                            to: alight_unix,
                        },
                    );
                    for alert in found {
                        if !alerts.contains(&alert) {
                            alerts.push(alert);
                        }
                    }
                }
                alerts
            }
            (Some(feed), Some(part)) => leg_alerts(
                part,
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
                    run: Some(ServiceRun {
                        trip: static_trip.map(|_| ride.trip.trip),
                        date: ride.trip.date,
                        start_time: details.and_then(|d| d.start_time).or_else(|| {
                            let (_, first) = static_trip?.time_at(0)?;
                            u32::try_from(first).ok()
                        }),
                    }),
                    stops: leg_stops,
                    from: board_unix,
                    to: alight_unix,
                },
            ),
            _ => Vec::new(),
        };

        let shape = trip_part
            .filter(|_| on_base_pattern)
            .and_then(|(part, local)| {
                part.compiled().shape_points_between(
                    local,
                    ride.board_position as usize,
                    ride.alight_position as usize,
                )
            })
            .map(|points| {
                self.access_egress.enter_stage(PlanStage::TransitShape);
                let shape = encode_polyline6(&points);
                self.access_egress.enter_stage(PlanStage::TransitLeg);
                shape
            });
        let segments = Self::ride_segments(part, &trip_segments, ride, &stop_indices);

        let realtime = stops
            .iter()
            .any(|s| s.predicted_arrival.is_some() || s.predicted_departure.is_some());
        let delay_seconds = stops.first().and_then(|s| s.departure_delay_seconds);

        // A stay-seated ride is as accessible as its least accessible ridden stretch.
        let trip_accessibility = if ridden_segments.is_empty() {
            static_trip.map(|info| accessibility_of(info.wheelchair_accessible))
        } else {
            Some(combined_accessibility(
                ridden_segments
                    .iter()
                    .map(|s| accessibility_of(s.wheelchair_accessible)),
            ))
        };
        let accessibility = leg_accessibility(trip_accessibility, &stops);
        TransitLeg {
            accessibility,
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
                boarded_segment,
                &ridden_segments,
                details,
                feed.as_ref(),
                self.snapshot,
            ),
            stops,
            segments,
            shape,
            vehicle,
            alerts,
        }
    }

    /// The original trips a stay-seated ride spans; empty for a ride on one trip.
    fn ride_segments(
        part: Option<TimetablePart<'_>>,
        segments: &[TripSegmentInfo<'_>],
        ride: &Ride,
        stop_indices: &[usize],
    ) -> Vec<RideSegment> {
        let Some(part) = part else {
            return Vec::new();
        };
        if segments.len() < 2 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for segment in segments {
            let from = segment.first_position.max(ride.board_position);
            let to = segment.last_position.min(ride.alight_position);
            if from > to {
                continue;
            }
            let route = part.compiled().route(segment.route).map(route_response);
            let index_of = |position: u32| {
                stop_indices
                    .get((position - ride.board_position) as usize)
                    .copied()
                    .unwrap_or(0)
            };
            out.push(RideSegment {
                trip_id: segment.gtfs_id.to_owned(),
                route,
                headsign: segment.headsign.map(str::to_owned),
                first_stop_index: index_of(from),
                last_stop_index: index_of(to),
            });
        }
        // A ride within one original trip is just a ride on that trip.
        if out.len() < 2 {
            out.clear();
        }
        out
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
        self.set()
            .stop_part(stop)
            .and_then(|(part, local)| part.compiled().stop(local))
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
        let live = u32::try_from(index)
            .ok()
            .and_then(|position| self.snapshot.rider.stop_details_at(visit.key, position));
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
            let text = visit.part?.compiled().stop_headsign(&info, index)?;
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
            utc_offset_seconds: self
                .set()
                .stop_part(stop)
                .and_then(|(part, local)| part.compiled().stop_utc_offset_at(local, departure)),
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

/// Copies each `STOP_MOVED` mark of a ride onto the walk that reaches or leaves that stop.
fn mark_moved_stop_walks(legs: &mut [JourneyLeg]) {
    for index in 0..legs.len() {
        let notices: Vec<(usize, StopMovedNotice)> = match &legs[index] {
            JourneyLeg::Transit(leg) => leg
                .alerts
                .iter()
                .flat_map(|alert| walks_affected_by(index, alert))
                .collect(),
            _ => continue,
        };
        for (walk, notice) in notices {
            if let Some(leg) = legs.get_mut(walk) {
                match leg {
                    JourneyLeg::Access(leg) | JourneyLeg::Egress(leg) => {
                        leg.stop_moved.push(notice);
                    }
                    JourneyLeg::Transfer(leg) => leg.stop_moved.push(notice),
                    JourneyLeg::Transit(_) => {}
                }
            }
        }
    }
}

/// The walking legs next to ride `index` that a `STOP_MOVED` alert concerns.
fn walks_affected_by(index: usize, alert: &LegAlert) -> Vec<(usize, StopMovedNotice)> {
    let Some(moved) = &alert.moved_stops else {
        return Vec::new();
    };
    moved
        .iter()
        .filter_map(|stop| {
            // Walks run to a boarding and from an alighting.
            let (walk, walk_end) = match stop.role {
                "boarding" => (index.checked_sub(1)?, "end"),
                "alighting" => (index + 1, "start"),
                _ => return None,
            };
            Some((
                walk,
                StopMovedNotice {
                    walk_end,
                    stop_id: stop.stop_id.clone(),
                    name: stop.name.clone(),
                    header: alert.alert.header.clone(),
                    description: alert.alert.description.clone(),
                    url: alert.alert.url.clone(),
                },
            ))
        })
        .collect()
}

/// What a stop visit needs to know about its ride.
struct VisitContext<'a> {
    feed: Option<&'a FeedId>,
    /// The file this leg's trip is in, when it has one.
    part: Option<TimetablePart<'a>>,
    /// Set only when the ride is on the trip's own pattern, so positions agree.
    static_trip: Option<TripInfo<'a>>,
    details: Option<&'a TripDetails>,
    day_start: Option<i64>,
    /// The ride's run, for finding its real-time details position by position.
    key: TripKey,
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

/// Whether a position is the start of a later original trip on a combined one.
fn is_segment_start(segments: &[TripSegmentInfo<'_>], position: u32) -> bool {
    segments
        .iter()
        .skip(1)
        .any(|segment| segment.first_position == position)
}

fn gtfs_route_of(
    set: &TimetableSet,
    part: TimetablePart<'_>,
    static_trip: Option<TripInfo<'_>>,
    details: Option<&TripDetails>,
) -> Option<GtfsRoute> {
    if let Some(info) = static_trip {
        return Some(info.route);
    }
    let details = details?;
    let route = details
        .route
        .and_then(|route| set.route_part(route))
        .filter(|(found, _)| found.index() == part.index())
        .and_then(|(found, local)| found.compiled().pattern(local))
        .map(|pattern| pattern.route);
    route.or_else(|| {
        let feed = details.feed.as_ref()?;
        part.compiled()
            .route_by_gtfs_id(feed.as_str(), details.route_id.as_deref()?)
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

#[allow(clippy::too_many_arguments, reason = "one leg's sources")]
fn trip_response(
    ride: &Ride,
    trip_id: &str,
    static_trip: Option<TripInfo<'_>>,
    boarded_segment: Option<TripSegmentInfo<'_>>,
    ridden_segments: &[TripSegmentInfo<'_>],
    details: Option<&TripDetails>,
    feed: Option<&FeedId>,
    snapshot: &TransitSnapshot,
) -> TripResponse {
    // A ride shows what real-time data did to the stretches it rides, and no others.
    let state = if ridden_segments.is_empty() {
        snapshot.rider.trip_state(ride.trip)
    } else {
        ridden_segments
            .iter()
            .map(|s| snapshot.rider.stretch_state(ride.trip, s.first_position))
            .find(|state| *state != TripState::Scheduled)
            .unwrap_or(TripState::Scheduled)
    };
    TripResponse {
        feed: feed.map(ToString::to_string),
        trip_id: trip_id.to_owned(),
        start_date: calendar_date(ride.trip.date).map(|date| date.to_string()),
        start_time: details.and_then(|d| d.start_time),
        headsign: details
            .and_then(|d| d.headsign.clone())
            .or_else(|| boarded_segment.and_then(|s| s.headsign.map(str::to_owned)))
            .or_else(|| static_trip.and_then(|i| i.headsign.map(str::to_owned))),
        direction_id: boarded_segment
            .and_then(|s| s.direction)
            .or_else(|| static_trip.and_then(|i| i.direction))
            .map(u32::from)
            .or_else(|| details.and_then(|d| d.direction_id)),
        block_id: static_trip.and_then(|i| i.block_id.map(str::to_owned)),
        headway_based: static_trip.map(|i| i.headway_based).filter(|&based| based),
        wheelchair_accessible: boarded_segment
            .map(|s| s.wheelchair_accessible)
            .or_else(|| static_trip.map(|i| i.wheelchair_accessible))
            .and_then(accessibility),
        bikes_allowed: boarded_segment
            .map(|s| s.bikes_allowed)
            .or_else(|| static_trip.map(|i| i.bikes_allowed))
            .and_then(accessibility),
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

fn accessibility(value: GtfsAccessibility) -> Option<&'static str> {
    match value {
        GtfsAccessibility::Unknown => None,
        GtfsAccessibility::Yes => Some("yes"),
        GtfsAccessibility::No => Some("no"),
    }
}

fn accessibility_of(value: GtfsAccessibility) -> Accessibility {
    match value {
        GtfsAccessibility::Unknown => Accessibility::Unknown,
        GtfsAccessibility::Yes => Accessibility::Accessible,
        GtfsAccessibility::No => Accessibility::Inaccessible,
    }
}

/// The least accessible of several values: known-inaccessible beats unknown beats accessible.
fn combined_accessibility(parts: impl Iterator<Item = Accessibility>) -> Accessibility {
    let mut combined = Accessibility::Accessible;
    for part in parts {
        combined = match (combined, part) {
            (Accessibility::Inaccessible, _) | (_, Accessibility::Inaccessible) => {
                Accessibility::Inaccessible
            }
            (Accessibility::Unknown, _) | (_, Accessibility::Unknown) => Accessibility::Unknown,
            _ => Accessibility::Accessible,
        };
    }
    combined
}

/// A transit leg's accessibility: its trip's and its boarding and alighting stops'.
fn leg_accessibility(trip: Option<Accessibility>, stops: &[StopVisit]) -> Accessibility {
    let parts = trip
        .into_iter()
        .chain(stops.first().map(|visit| visit.stop.accessibility))
        .chain(stops.last().map(|visit| visit.stop.accessibility));
    let mut all_accessible = true;
    let mut seen_any = false;
    for part in parts {
        seen_any = true;
        match part {
            Accessibility::Inaccessible => return Accessibility::Inaccessible,
            Accessibility::Unknown => all_accessible = false,
            Accessibility::Accessible => {}
        }
    }
    if seen_any && all_accessible {
        Accessibility::Accessible
    } else {
        Accessibility::Unknown
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
