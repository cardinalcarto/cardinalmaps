//! A synthetic timetable behind a fake [`StaticLookup`], and builders for GTFS-RT feeds.

#![allow(clippy::needless_pass_by_value)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use valinor_transit::{
    BaseWindow, DaySlot, RouteAttributes, RouteId, Seconds, ServiceDate, ServiceDay, StopId,
    StopRules, StopTime, Time, Timetable, TimetableBuilder, TripId,
};

use crate::date::CalendarDate;
use crate::decode::DecodedFeed;
use crate::feed::{FeedId, FeedInput, FeedKind};
use crate::lookup::{RouteInfo, ScheduledTrip, StaticLookup, TripQuery, TripRef, WindowDay};
use crate::proto;

/// The POSIX start of the first service day of every test window.
pub(crate) const DAY_0: i64 = 1_767_225_600;
pub(crate) const DAY_SECONDS: i64 = 86_400;
/// Mid-afternoon on the middle day of the window.
pub(crate) const NOW: i64 = DAY_0 + DAY_SECONDS + 15 * 3600;

pub(crate) const DATE_0: &str = "20260101";
pub(crate) const DATE_1: &str = "20260102";
pub(crate) const DATE_2: &str = "20260103";

/// A trip's name and its arrival time at each stop.
pub(crate) type TripSpec<'a> = (&'a str, &'a [i32]);

pub(crate) struct NetBuilder {
    builder: TimetableBuilder,
    /// `(feed, stop_id)` to the builder's key.
    stops: HashMap<(FeedId, String), valinor_transit::StopKey>,
    routes: Vec<RouteSpec>,
    service: valinor_transit::ServiceId,
    feed: FeedId,
    next_trip: u32,
}

struct RouteSpec {
    feed: FeedId,
    route_id: String,
    agency_id: Option<String>,
    route_type: i32,
    /// `(trip, direction, sequences)`, in pattern order.
    trips: Vec<(String, TripId, Option<u32>, Vec<u32>)>,
}

pub(crate) struct Net {
    pub(crate) lookup: FakeLookup,
    pub(crate) base: Arc<BaseWindow>,
}

pub(crate) struct FakeLookup {
    feed_days: Vec<WindowDay>,
    timetable: Timetable,
    service_days: Vec<ServiceDay>,
    stops: HashMap<(FeedId, String), StopId>,
    stop_names: HashMap<(FeedId, StopId), String>,
    routes: Vec<RouteSpec>,
    route_patterns: HashMap<(FeedId, String), Vec<RouteId>>,
    trips: HashMap<(FeedId, String), (TripId, RouteId)>,
    children: HashMap<(FeedId, String), Vec<StopId>>,
    next_trip: u32,
    /// Trips answered as `exact_times=0` runs: `start_time` identifies their vehicles.
    pub(crate) headway_trips: HashSet<String>,
}

pub(crate) fn feed_id(name: &str) -> FeedId {
    FeedId::new(name).expect("non-empty")
}

pub(crate) fn date(text: &str) -> CalendarDate {
    CalendarDate::parse(text).expect("valid date")
}

impl NetBuilder {
    pub(crate) fn new() -> Self {
        let mut builder = TimetableBuilder::new();
        let service = builder
            .add_service((0..3).map(ServiceDate::new))
            .expect("service");
        Self {
            builder,
            stops: HashMap::new(),
            routes: Vec::new(),
            service,
            feed: feed_id("kcm"),
            next_trip: 0,
        }
    }

    /// Later routes and stops belong to `feed`.
    pub(crate) fn in_feed(mut self, feed: &str) -> Self {
        self.feed = feed_id(feed);
        self
    }

    fn stop(&mut self, id: &str) -> valinor_transit::StopKey {
        let key = (self.feed.clone(), id.to_owned());
        if let Some(&stop) = self.stops.get(&key) {
            return stop;
        }
        let stop = self.builder.add_stop(Seconds::ZERO).expect("stop");
        self.stops.insert(key, stop);
        stop
    }

    /// Adds a route with one pattern; `step` spaces the trips' `stop_sequence` values.
    pub(crate) fn route(self, route_id: &str, stops: &[&str], trips: &[TripSpec<'_>]) -> Self {
        self.route_with(route_id, Some("agency"), 3, Some(0), 1, stops, trips)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn route_with(
        mut self,
        route_id: &str,
        agency: Option<&str>,
        route_type: i32,
        direction: Option<u32>,
        step: u32,
        stops: &[&str],
        trips: &[TripSpec<'_>],
    ) -> Self {
        let keys: Vec<_> = stops.iter().map(|s| self.stop(s)).collect();
        let pattern = self
            .builder
            .add_pattern(&keys, RouteAttributes::NONE)
            .expect("pattern");
        let mut trip_specs = Vec::with_capacity(trips.len());
        for &(name, times) in trips {
            let id = TripId::new(self.next_trip);
            self.next_trip += 1;
            let events = times
                .iter()
                .map(|&time| StopTime {
                    arrival: Time::try_new(time).expect("in range"),
                    departure: Time::try_new(time).expect("in range"),
                    rules: StopRules::REGULAR,
                })
                .collect();
            self.builder
                .add_trip(pattern, id, self.service, events)
                .expect("trip");
            let sequences = (1..=u32::try_from(stops.len()).expect("few stops"))
                .map(|i| i * step)
                .collect();
            trip_specs.push((name.to_owned(), id, direction, sequences));
        }
        self.routes.push(RouteSpec {
            feed: self.feed.clone(),
            route_id: route_id.to_owned(),
            agency_id: agency.map(str::to_owned),
            route_type,
            trips: trip_specs,
        });
        self
    }

    pub(crate) fn build(self) -> Net {
        let timetable = self.builder.build().expect("timetable");
        let service_days: Vec<ServiceDay> = (0_i32..3)
            .map(|d| ServiceDay {
                date: ServiceDate::new(u32::try_from(d).expect("small")),
                offset: Time::try_new(d * 86_400).expect("in range"),
            })
            .collect();
        let feed_days = vec![
            WindowDay {
                date: date(DATE_0),
                start_unix: DAY_0,
            },
            WindowDay {
                date: date(DATE_1),
                start_unix: DAY_0 + DAY_SECONDS,
            },
            WindowDay {
                date: date(DATE_2),
                start_unix: DAY_0 + 2 * DAY_SECONDS,
            },
        ];
        let base = Arc::new(timetable.materialize(&service_days).expect("window"));

        let stops: HashMap<(FeedId, String), StopId> = self
            .stops
            .iter()
            .map(|(key, &stop)| (key.clone(), timetable.stop_id(stop)))
            .collect();
        let stop_names = stops
            .iter()
            .map(|((feed, name), &id)| ((feed.clone(), id), name.clone()))
            .collect();
        let mut route_patterns: HashMap<(FeedId, String), Vec<RouteId>> = HashMap::new();
        let mut trips = HashMap::new();
        let mut next_trip = 0;
        for (index, route) in self.routes.iter().enumerate() {
            let id = RouteId::new(u32::try_from(index).expect("few routes"));
            route_patterns
                .entry((route.feed.clone(), route.route_id.clone()))
                .or_default()
                .push(id);
            for (name, trip, _, _) in &route.trips {
                trips.insert((route.feed.clone(), name.clone()), (*trip, id));
                next_trip = next_trip.max(trip.into_inner() + 1);
            }
        }
        Net {
            lookup: FakeLookup {
                feed_days,
                timetable,
                service_days,
                stops,
                stop_names,
                routes: self.routes,
                route_patterns,
                trips,
                children: HashMap::new(),
                next_trip,
                headway_trips: HashSet::new(),
            },
            base,
        }
    }
}

impl FakeLookup {
    /// Makes `station` stand for `platforms` of `feed`.
    pub(crate) fn add_station(&mut self, feed: &str, station: &str, platforms: &[&str]) {
        let feed = feed_id(feed);
        let ids = platforms
            .iter()
            .map(|p| {
                *self
                    .stops
                    .get(&(feed.clone(), (*p).to_owned()))
                    .expect("known platform")
            })
            .collect();
        self.children.insert((feed, station.to_owned()), ids);
    }

    pub(crate) fn stop_number(&self, feed: &str, stop: &str) -> StopId {
        *self
            .stops
            .get(&(feed_id(feed), stop.to_owned()))
            .expect("known stop")
    }
}

impl FakeLookup {
    pub(crate) fn first_unused_trip_id_for_test(&self) -> u32 {
        self.next_trip
    }
}

impl StaticLookup for FakeLookup {
    fn window_days(&self) -> &[WindowDay] {
        &self.feed_days
    }

    fn trip(&self, feed: &FeedId, query: &TripQuery<'_>) -> Option<TripRef> {
        let &(trip, route) = self.trips.get(&(feed.clone(), query.trip_id.to_owned()))?;
        let (slot, day) = self
            .feed_days
            .iter()
            .enumerate()
            .find(|(_, d)| d.date == query.date)?;
        let service = self.service_days[slot];
        let resolved = self
            .timetable
            .resolved_trips(route, service.date, service.offset)
            .ok()?;
        let found = resolved.into_iter().find(|t| t.trip == trip)?;
        let _ = day;
        Some(TripRef {
            segment: None,
            trip,
            route,
            day: DaySlot::new(u8::try_from(slot).ok()?),
            first_departure: found.events.first()?.departure,
            last_arrival: found.events.last()?.arrival,
            headway_start: self
                .headway_trips
                .contains(query.trip_id)
                .then_some(query.start_time)
                .flatten(),
        })
    }

    fn route_patterns(&self, feed: &FeedId, route_id: &str) -> Vec<RouteId> {
        self.route_patterns
            .get(&(feed.clone(), route_id.to_owned()))
            .cloned()
            .unwrap_or_default()
    }

    fn route_info(&self, route: RouteId) -> Option<RouteInfo<'_>> {
        let spec = self.routes.get(route.into_inner() as usize)?;
        Some(RouteInfo {
            feed: &spec.feed,
            route_id: &spec.route_id,
            agency_id: spec.agency_id.as_deref(),
            route_type: spec.route_type,
        })
    }

    fn stop(&self, feed: &FeedId, stop_id: &str) -> Option<StopId> {
        self.stops.get(&(feed.clone(), stop_id.to_owned())).copied()
    }

    fn stop_gtfs_id(&self, feed: &FeedId, stop: StopId) -> Option<&str> {
        self.stop_names
            .get(&(feed.clone(), stop))
            .map(String::as_str)
    }

    fn child_stops(&self, feed: &FeedId, stop_id: &str) -> Vec<StopId> {
        self.children
            .get(&(feed.clone(), stop_id.to_owned()))
            .cloned()
            .unwrap_or_default()
    }

    fn scheduled_block(&self, route: RouteId, day: DaySlot) -> Vec<ScheduledTrip> {
        let service = self.service_days[usize::from(day.into_inner())];
        let spec = &self.routes[route.into_inner() as usize];
        self.timetable
            .resolved_trips(route, service.date, service.offset)
            .expect("known route")
            .into_iter()
            .map(|trip| {
                let (_, _, direction, sequences) = spec
                    .trips
                    .iter()
                    .find(|(_, id, _, _)| *id == trip.trip)
                    .expect("known trip");
                ScheduledTrip {
                    segments: Vec::new(),
                    trip: trip.trip,
                    direction_id: *direction,
                    events: trip.events,
                    stop_sequences: sequences.clone(),
                }
            })
            .collect()
    }

    fn first_unused_trip_id(&self) -> u32 {
        self.next_trip
    }
}

// Feed builders.

pub(crate) fn feed_message(timestamp: i64, entity: Vec<proto::FeedEntity>) -> DecodedFeed {
    DecodedFeed::from_message(proto::FeedMessage {
        header: proto::FeedHeader {
            gtfs_realtime_version: "2.0".into(),
            incrementality: None,
            timestamp: Some(u64::try_from(timestamp).expect("positive")),
            feed_version: None,
        },
        entity,
    })
}

pub(crate) fn input(
    feed: &str,
    kind: FeedKind,
    timestamp: i64,
    entity: Vec<proto::FeedEntity>,
) -> FeedInput {
    FeedInput::new(feed_id(feed), kind, feed_message(timestamp, entity))
}

/// A trip-updates feed that is current at [`NOW`].
pub(crate) fn fresh(updates: Vec<proto::TripUpdate>) -> FeedInput {
    updates_at(NOW, updates)
}

pub(crate) fn updates_at(timestamp: i64, updates: Vec<proto::TripUpdate>) -> FeedInput {
    input(
        "kcm",
        FeedKind::TripUpdates,
        timestamp,
        updates.into_iter().map(entity_of_update).collect(),
    )
}

pub(crate) fn entity_of_update(update: proto::TripUpdate) -> proto::FeedEntity {
    proto::FeedEntity {
        // Real entity IDs carry a timestamp prefix; nothing may rely on them being stable.
        id: format!("{}_{}", 17, update.trip.trip_id.clone().unwrap_or_default()),
        trip_update: Some(update),
        ..Default::default()
    }
}

pub(crate) fn alerts_feed(alerts: Vec<proto::Alert>) -> FeedInput {
    input(
        "kcm",
        FeedKind::Alerts,
        NOW,
        alerts
            .into_iter()
            .enumerate()
            .map(|(i, alert)| proto::FeedEntity {
                id: format!("{i}"),
                alert: Some(alert),
                ..Default::default()
            })
            .collect(),
    )
}

pub(crate) fn descriptor(trip: &str, start_date: &str) -> proto::TripDescriptor {
    proto::TripDescriptor {
        trip_id: Some(trip.into()),
        start_date: Some(start_date.into()),
        ..Default::default()
    }
}

pub(crate) fn update(
    trip: &str,
    start_date: &str,
    stops: Vec<proto::trip_update::StopTimeUpdate>,
) -> proto::TripUpdate {
    proto::TripUpdate {
        trip: descriptor(trip, start_date),
        stop_time_update: stops,
        ..Default::default()
    }
}

pub(crate) fn delay_at(sequence: u32, seconds: i32) -> proto::trip_update::StopTimeUpdate {
    let event = proto::trip_update::StopTimeEvent {
        delay: Some(seconds),
        ..Default::default()
    };
    proto::trip_update::StopTimeUpdate {
        stop_sequence: Some(sequence),
        arrival: Some(event),
        departure: Some(event),
        ..Default::default()
    }
}

pub(crate) fn skip_at(sequence: u32) -> proto::trip_update::StopTimeUpdate {
    let mut update = proto::trip_update::StopTimeUpdate {
        stop_sequence: Some(sequence),
        ..Default::default()
    };
    update.set_schedule_relationship(
        proto::trip_update::stop_time_update::ScheduleRelationship::Skipped,
    );
    update
}

pub(crate) fn time_at(stop: &str, unix: i64) -> proto::trip_update::StopTimeUpdate {
    let event = proto::trip_update::StopTimeEvent {
        time: Some(unix),
        ..Default::default()
    };
    proto::trip_update::StopTimeUpdate {
        stop_id: Some(stop.into()),
        arrival: Some(event),
        departure: Some(event),
        ..Default::default()
    }
}

pub(crate) fn with_relationship(
    mut update: proto::TripUpdate,
    relationship: proto::trip_descriptor::ScheduleRelationship,
) -> proto::TripUpdate {
    update.trip.set_schedule_relationship(relationship);
    update
}

pub(crate) fn alert(
    effect: proto::alert::Effect,
    selectors: Vec<proto::EntitySelector>,
) -> proto::Alert {
    let mut alert = proto::Alert {
        informed_entity: selectors,
        ..Default::default()
    };
    alert.set_effect(effect);
    alert
}

pub(crate) fn no_service(selectors: Vec<proto::EntitySelector>) -> proto::Alert {
    alert(proto::alert::Effect::NoService, selectors)
}

#[allow(deprecated)]
pub(crate) fn during(
    mut alert: proto::Alert,
    start: Option<i64>,
    end: Option<i64>,
) -> proto::Alert {
    alert.active_period.push(proto::TimeRange {
        start: start.map(|s| u64::try_from(s).expect("positive")),
        end: end.map(|e| u64::try_from(e).expect("positive")),
    });
    alert
}

pub(crate) fn route_selector(route: &str) -> proto::EntitySelector {
    proto::EntitySelector {
        route_id: Some(route.into()),
        ..Default::default()
    }
}

pub(crate) fn stop_selector(stop: &str) -> proto::EntitySelector {
    proto::EntitySelector {
        stop_id: Some(stop.into()),
        ..Default::default()
    }
}

pub(crate) fn trip_selector(trip: &str, start_date: Option<&str>) -> proto::EntitySelector {
    proto::EntitySelector {
        trip: Some(proto::TripDescriptor {
            trip_id: Some(trip.into()),
            start_date: start_date.map(str::to_owned),
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub(crate) fn window_seconds(day: u8, unix_or_relative: i32) -> i32 {
    i32::from(day) * 86_400 + unix_or_relative
}
