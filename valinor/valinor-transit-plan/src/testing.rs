//! Test helpers: a synthetic feed, GTFS-RT messages, and a straight-line street stand-in.

use core::convert::Infallible;
use std::collections::HashMap;

use prost::Message;
use serde::Serialize;
use valinor_gtfs::{
    FeedId as CompileFeedId, FeedInput, InMemorySource, compile, parse_service_date,
};
use valinor_gtfs::{LatLon, LoadedTimetable};
use valinor_gtfs_rt::proto;
use valinor_transit::{
    Clock, Direction, Seconds, StopId, StopTable, TransferArrival, TransferOutcome,
    TransferRequest, TransferSearch, TransferSink,
};

use crate::plan::{
    AccessEgressRequest, AccessEgressSearch, AccessEgressTables, DirectSearch, TravelMode,
};
use crate::state::{StateConfig, StopLocation, TransitSnapshot, TransitState};

pub(crate) const FEED: &str = "metro";

/// A short synthetic network in Seattle's time zone.
///
/// `S1`..`S4` run north along one line; `S5` is a short walk from `S3`, and `S6` is east of `S5`.
/// `P1` and `P2` are platforms of the station `STN`.
const STOPS: &str = "stop_id,stop_name,stop_code,stop_lat,stop_lon,wheelchair_boarding,\
platform_code,location_type,parent_station\n\
STN,Central Station,,47.6500,-122.3300,1,,1,\n\
P1,Central Platform 1,91,47.6500,-122.3301,1,1,,STN\n\
P2,Central Platform 2,92,47.6500,-122.3299,2,2,,STN\n\
S1,Alpha,101,47.6000,-122.3300,1,,,\n\
S2,Beta,102,47.6100,-122.3300,2,,,\n\
S3,Gamma,103,47.6200,-122.3300,,,,\n\
S4,Delta,104,47.6300,-122.3300,,,,\n\
S5,Epsilon,105,47.6204,-122.3300,,,,\n\
S6,Zeta,106,47.6204,-122.3100,,,,\n";

const TRIPS: &str = "route_id,service_id,trip_id,trip_headsign,block_id,\
wheelchair_accessible,bikes_allowed,direction_id,shape_id\n\
R1,WK,T1,Northbound,B1,1,2,0,SH1\n\
R1,WK,T2,Northbound,B1,1,2,0,SH1\n\
R2,WK,T3,Eastbound,,2,,1,\n\
R2,WK,T4,Eastbound,,2,,1,\n\
R3,WK,T5,Via Bikeway,,1,1,0,\n\
R4,WK,F1,Headway Line,,,,0,\n\
R4,WK,F2,Exact Line,,,,0,\n\
R5,WK,T6,Station Link,,,,0,\n";

const STOP_TIMES: &str = "trip_id,arrival_time,departure_time,stop_id,stop_sequence,\
stop_headsign,pickup_type,drop_off_type,timepoint\n\
T1,08:00:00,08:00:30,S1,10,,,,1\n\
T1,08:10:00,08:10:00,S2,20,Via Beta,,,0\n\
T1,08:20:00,08:20:00,S3,30,,,,1\n\
T1,08:30:00,08:30:00,S4,40,,,,1\n\
T2,08:30:00,08:30:30,S1,10,,,,1\n\
T2,08:40:00,08:40:00,S2,20,Via Beta,,,0\n\
T2,08:50:00,08:50:00,S3,30,,,,1\n\
T2,09:00:00,09:00:00,S4,40,,,,1\n\
T3,08:40:00,08:40:00,S5,1,,,,\n\
T3,09:00:00,09:00:00,S6,2,,,,\n\
T4,09:10:00,09:10:00,S5,1,,,,\n\
T4,09:30:00,09:30:00,S6,2,,,,\n\
T5,08:05:00,08:05:00,S1,1,,,,\n\
T5,08:45:00,08:45:00,S3,2,,,,\n\
F1,10:00:00,10:00:00,S1,1,,,,\n\
F1,10:10:00,10:10:00,S2,2,,,,\n\
F2,11:00:00,11:00:00,S2,1,,,,\n\
F2,11:10:00,11:10:00,S3,2,,,,\n\
T6,09:00:00,09:00:00,S4,1,,,,\n\
T6,09:05:00,09:05:00,P1,2,,,,\n";

/// Compiles the synthetic feed into a loaded timetable.
pub(crate) fn synthetic_timetable() -> LoadedTimetable {
    let source = InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url,agency_phone,agency_fare_url\n\
             MT,Metro,America/Los_Angeles,https://m.test,555-1000,https://m.test/fare\n",
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_long_name,route_desc,route_type,\
             route_url,route_color,route_text_color\n\
             R1,MT,1,First Avenue,Crosstown,3,https://m.test/1,FDB71A,000000\n\
             R2,MT,2,Second Avenue,,3,,112233,FFFFFF\n\
             R3,MT,3,Bike Express,,3,,,\n\
             R4,MT,4,Frequent Line,,3,,,\n\
             R5,MT,5,Station Line,,3,,,\n",
        )
        .with("stops.txt", STOPS)
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with("trips.txt", TRIPS)
        .with("stop_times.txt", STOP_TIMES)
        .with(
            "frequencies.txt",
            "trip_id,start_time,end_time,headway_secs,exact_times\n\
             F1,10:00:00,11:00:00,1800,0\n\
             F2,11:00:00,12:00:00,1800,1\n",
        )
        .with(
            "shapes.txt",
            "shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence,shape_dist_traveled\n\
             SH1,47.6000,-122.3300,1,0\n\
             SH1,47.6050,-122.3303,2,550\n\
             SH1,47.6100,-122.3300,3,1110\n\
             SH1,47.6200,-122.3300,4,2220\n\
             SH1,47.6300,-122.3300,5,3330\n",
        );
    let compiled = compile(vec![FeedInput {
        id: CompileFeedId::try_new(FEED.to_owned()).expect("valid id"),
        source: Box::new(source),
    }])
    .expect("the synthetic feed compiles");
    LoadedTimetable::from_bytes(compiled.bytes).expect("the compiled file loads")
}

/// A state owner over the synthetic feed.
pub(crate) fn synthetic_state() -> TransitState {
    TransitState::new(synthetic_timetable(), StateConfig::default()).expect("state builds")
}

/// `service_date` as days since the epoch, for a `YYYYMMDD` literal.
pub(crate) fn date(text: &str) -> valinor_transit::ServiceDate {
    parse_service_date(text).expect("valid date")
}

/// The POSIX time of a local time on 2026-06-15, a Monday in PDT (UTC-7).
pub(crate) fn june_15(hours: i64, minutes: i64) -> i64 {
    // 2026-06-15T00:00:00-07:00 is 2026-06-15T07:00:00Z.
    1_781_506_800 + hours * 3600 + minutes * 60
}

pub(crate) const JUNE_15: &str = "20260615";
pub(crate) const JUNE_16: &str = "20260616";

pub(crate) fn encoded_feed(timestamp: i64, entity: Vec<proto::FeedEntity>) -> Vec<u8> {
    proto::FeedMessage {
        header: proto::FeedHeader {
            gtfs_realtime_version: "2.0".to_owned(),
            incrementality: None,
            timestamp: u64::try_from(timestamp).ok(),
            feed_version: None,
        },
        entity,
    }
    .encode_to_vec()
}

pub(crate) fn entity(id: &str) -> proto::FeedEntity {
    proto::FeedEntity {
        id: id.to_owned(),
        ..proto::FeedEntity::default()
    }
}

pub(crate) fn descriptor(trip: &str, date: &str, start: Option<&str>) -> proto::TripDescriptor {
    proto::TripDescriptor {
        trip_id: Some(trip.to_owned()),
        start_date: Some(date.to_owned()),
        start_time: start.map(str::to_owned),
        ..proto::TripDescriptor::default()
    }
}

pub(crate) fn delay_update(trip: &str, date: &str, sequence: u32, delay: i32) -> proto::FeedEntity {
    let mut item = entity(&format!("tu-{trip}"));
    item.trip_update = Some(proto::TripUpdate {
        trip: descriptor(trip, date, None),
        stop_time_update: vec![proto::trip_update::StopTimeUpdate {
            stop_sequence: Some(sequence),
            arrival: Some(proto::trip_update::StopTimeEvent {
                delay: Some(delay),
                uncertainty: Some(30),
                ..proto::trip_update::StopTimeEvent::default()
            }),
            departure: Some(proto::trip_update::StopTimeEvent {
                delay: Some(delay),
                ..proto::trip_update::StopTimeEvent::default()
            }),
            ..proto::trip_update::StopTimeUpdate::default()
        }],
        ..proto::TripUpdate::default()
    });
    item
}

pub(crate) fn vehicle_entity(
    id: &str,
    trip: &str,
    date: &str,
    start: Option<&str>,
    latitude: f32,
    longitude: f32,
) -> proto::FeedEntity {
    let mut item = entity(&format!("veh-{id}"));
    item.vehicle = Some(proto::VehiclePosition {
        trip: Some(descriptor(trip, date, start)),
        vehicle: Some(proto::VehicleDescriptor {
            id: Some(id.to_owned()),
            label: Some(format!("Bus {id}")),
            ..proto::VehicleDescriptor::default()
        }),
        position: Some(proto::Position {
            latitude,
            longitude,
            bearing: Some(90.0),
            speed: Some(11.5),
            ..proto::Position::default()
        }),
        current_stop_sequence: Some(20),
        stop_id: Some("S2".to_owned()),
        occupancy_status: Some(proto::vehicle_position::OccupancyStatus::ManySeatsAvailable as i32),
        congestion_level: Some(proto::vehicle_position::CongestionLevel::RunningSmoothly as i32),
        timestamp: Some(1_781_535_000),
        ..proto::VehiclePosition::default()
    });
    item
}

#[allow(deprecated)]
pub(crate) fn alert_entity(
    id: &str,
    effect: proto::alert::Effect,
    selectors: Vec<proto::EntitySelector>,
) -> proto::FeedEntity {
    let mut item = entity(id);
    item.alert = Some(proto::Alert {
        active_period: vec![proto::TimeRange {
            start: Some(1_700_000_000),
            end: Some(1_900_000_000),
        }],
        informed_entity: selectors,
        effect: Some(effect as i32),
        cause: Some(proto::alert::Cause::Construction as i32),
        severity_level: Some(proto::alert::SeverityLevel::Warning as i32),
        header_text: Some(translated(&["Stop relocated", "Parada reubicada"])),
        description_text: Some(translated(&[
            "The temporary stop is on Pine St just east of the old one.",
        ])),
        url: Some(translated(&["https://m.test/alert"])),
        ..proto::Alert::default()
    });
    item
}

fn translated(texts: &[&str]) -> proto::TranslatedString {
    proto::TranslatedString {
        translation: texts
            .iter()
            .enumerate()
            .map(|(index, text)| proto::translated_string::Translation {
                text: (*text).to_owned(),
                language: Some(if index == 0 { "en" } else { "es" }.to_owned()),
            })
            .collect(),
    }
}

pub(crate) fn stop_selector(stop: &str) -> proto::EntitySelector {
    proto::EntitySelector {
        stop_id: Some(stop.to_owned()),
        ..proto::EntitySelector::default()
    }
}

pub(crate) fn route_stop_selector(route: &str, stop: &str) -> proto::EntitySelector {
    proto::EntitySelector {
        route_id: Some(route.to_owned()),
        stop_id: Some(stop.to_owned()),
        ..proto::EntitySelector::default()
    }
}

pub(crate) fn route_selector(route: &str) -> proto::EntitySelector {
    proto::EntitySelector {
        route_id: Some(route.to_owned()),
        ..proto::EntitySelector::default()
    }
}

/// Meters between two points, close enough for these tests.
fn meters_between(a: LatLon, b: LatLon) -> f64 {
    const EARTH_RADIUS_METERS: f64 = 6_371_008.8;
    let lat = f64::midpoint(a.lat, b.lat).to_radians();
    let dx = (b.lon - a.lon).to_radians() * lat.cos();
    let dy = (b.lat - a.lat).to_radians();
    (dx * dx + dy * dy).sqrt() * EARTH_RADIUS_METERS
}

fn seconds_for(meters: f64, speed: f64) -> Seconds {
    #[expect(clippy::cast_possible_truncation, reason = "test distances are small")]
    #[expect(clippy::cast_sign_loss, reason = "distances are positive")]
    let seconds = (meters / speed).ceil() as u32;
    Seconds::try_new(seconds).expect("in range")
}

/// A walk found by [`StraightLineTransfers`]; what the JSON snapshot shows for it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct StraightWalk {
    pub(crate) meters: u32,
}

/// A grid-indexed straight-line street stand-in for transfer searches.
#[derive(Debug)]
pub(crate) struct StraightLineTransfers {
    points: Vec<Option<LatLon>>,
    grid: HashMap<(i32, i32), Vec<StopId>>,
    radius_meters: f64,
    speed: f64,
    cell_lat: f64,
    cell_lon: f64,
}

impl StraightLineTransfers {
    pub(crate) fn new(
        stops: impl IntoIterator<Item = StopLocation>,
        radius_meters: f64,
        speed: f64,
    ) -> Self {
        let stops: Vec<StopLocation> = stops.into_iter().collect();
        let highest = stops.iter().map(|s| s.stop.into_inner()).max().unwrap_or(0) as usize;
        let mut points = vec![None; highest + 1];
        let cell_lat = radius_meters / 111_195.0;
        let cell_lon = cell_lat / 47.0_f64.to_radians().cos();
        let mut grid: HashMap<(i32, i32), Vec<StopId>> = HashMap::new();
        for stop in stops {
            let point = LatLon {
                lat: stop.latitude,
                lon: stop.longitude,
            };
            points[stop.stop.into_inner() as usize] = Some(point);
            grid.entry(Self::cell(point, cell_lat, cell_lon))
                .or_default()
                .push(stop.stop);
        }
        Self {
            points,
            grid,
            radius_meters,
            speed,
            cell_lat,
            cell_lon,
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "cells of a metro area fit in an i32"
    )]
    fn cell(point: LatLon, cell_lat: f64, cell_lon: f64) -> (i32, i32) {
        (
            (point.lat / cell_lat).floor() as i32,
            (point.lon / cell_lon).floor() as i32,
        )
    }
}

impl TransferSearch for StraightLineTransfers {
    type Parent = StraightWalk;
    type Walk = StraightWalk;
    type Error = Infallible;

    fn begin_run(&mut self, _direction: Direction) {}

    fn walk(&self, parent: Self::Parent) -> Self::Walk {
        parent
    }

    fn search<K: TransferSink<Self::Parent>>(
        &mut self,
        request: &TransferRequest<'_>,
        sink: &mut K,
    ) -> Result<TransferOutcome, Self::Error> {
        for (index, seed) in request.seeds.iter().enumerate() {
            if index % 64 == 0 && (request.interrupt)() {
                return Ok(TransferOutcome::Interrupted);
            }
            let Some(point) = self
                .points
                .get(seed.stop.into_inner() as usize)
                .copied()
                .flatten()
            else {
                continue;
            };
            let (row, column) = Self::cell(point, self.cell_lat, self.cell_lon);
            for dr in -1..=1 {
                for dc in -1..=1 {
                    let Some(nearby) = self.grid.get(&(row + dr, column + dc)) else {
                        continue;
                    };
                    for &stop in nearby {
                        if stop == seed.stop {
                            continue;
                        }
                        let Some(target) = self.points[stop.into_inner() as usize] else {
                            continue;
                        };
                        let meters = meters_between(point, target);
                        if meters > self.radius_meters {
                            continue;
                        }
                        let clock: Clock = seed.clock.advanced_by(seconds_for(meters, self.speed));
                        if clock >= request.upper_bound {
                            continue;
                        }
                        #[expect(clippy::cast_possible_truncation, reason = "below the radius")]
                        #[expect(clippy::cast_sign_loss, reason = "distances are positive")]
                        let meters = meters as u32;
                        sink.reach(TransferArrival {
                            stop,
                            source: seed.stop,
                            clock,
                            parent: StraightWalk { meters },
                        });
                    }
                }
            }
        }
        Ok(TransferOutcome::Completed)
    }
}

/// A straight-line access and egress search reaching every stop within a radius.
#[derive(Debug)]
pub(crate) struct StraightLineAccess {
    pub(crate) radius_meters: f64,
    pub(crate) walk_speed: f64,
    pub(crate) bike_speed: f64,
    pub(crate) include_direct: bool,
}

impl Default for StraightLineAccess {
    fn default() -> Self {
        Self {
            radius_meters: 1_200.0,
            walk_speed: 1.33,
            bike_speed: 4.5,
            include_direct: false,
        }
    }
}

impl AccessEgressSearch for StraightLineAccess {
    type Error = Infallible;
    type Walk = StraightWalk;

    fn tables(
        &mut self,
        request: &AccessEgressRequest<'_>,
        _interrupt: &dyn Fn() -> bool,
    ) -> Result<AccessEgressTables, Self::Error> {
        let speed = match request.mode {
            TravelMode::Walking | TravelMode::Wheelchair => self.walk_speed,
            TravelMode::Cycling => self.bike_speed,
        };
        Ok(AccessEgressTables {
            access: self.table(request.snapshot, request.origin, speed),
            egress: self.table(request.snapshot, request.destination, speed),
            direct: if self.include_direct {
                DirectSearch::Found(seconds_for(
                    meters_between(request.origin, request.destination),
                    speed,
                ))
            } else {
                DirectSearch::NotSearched
            },
        })
    }
}

impl StraightLineAccess {
    fn table(&self, snapshot: &TransitSnapshot, point: LatLon, speed: f64) -> StopTable {
        let mut table = StopTable::new(snapshot.snapshot().stop_count());
        for location in snapshot.stop_locations() {
            let there = LatLon {
                lat: location.latitude,
                lon: location.longitude,
            };
            let meters = meters_between(point, there);
            if meters <= self.radius_meters {
                table
                    .set(location.stop, seconds_for(meters, speed))
                    .expect("stop is in range");
            }
        }
        table
    }
}
