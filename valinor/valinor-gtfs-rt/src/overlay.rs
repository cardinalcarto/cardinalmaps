//! Builds a [`RealtimeOverlay`] from decoded feeds and the static timetable.
//!
//! The build is a pure function of the feeds, the static timetable, and `now`:
//! nothing in it reads a clock or remembers a previous overlay.
//!
//! Changes are collected per `(route, day)` first. Then blocks are laid out in ascending
//! `(route, day)` order, followed by added routes, so the overlay's arena holds them
//! contiguously in route order, exactly like the base window, and the route scan's
//! prefetching behaves the same on both.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use thiserror::Error;
use valinor_transit::{
    ArenaUsage, BaseWindow, BuildError, DaySlot, OverlayBuilder, ResolvedBlock, ResolvedTrip,
    RouteAttributes, RouteId, Seconds, Snapshot, StopEvent, StopId, StopRules, Time, TripId,
    TripKey,
};

use crate::alerts::{
    AlertEffect, NoServiceAlert, Period, TripIdentity, compile_no_service, scheduled_identity,
};
use crate::date::{CalendarDate, parse_start_time};
use crate::feed::{FeedId, FeedInput, ScopedId};
use crate::lookup::{DayClock, ScheduledTrip, StaticLookup, TripQuery, TripRef};
use crate::modifications::{ModifiedLayout, apply_modifications};
use crate::propagate::{Propagated, Trip, propagate, repair, rules_of, window_time};
use crate::proto;
use crate::proto::trip_descriptor::ScheduleRelationship as TripRelationship;
use crate::proto::trip_update::stop_time_update::ScheduleRelationship as StopRelationship;
use crate::proto::trip_update::{StopTimeEvent, StopTimeUpdate};
use crate::proto::{TripDescriptor, TripModifications, TripUpdate};
use crate::rider::{
    AlertDetails, InformedEntity, RiderData, StopTimeDetails, TripDetails, VehicleDetails,
    VehicleRef,
};
use crate::states::TripState;

/// The furthest, in seconds, a prediction may be from its schedule before it is ignored.
///
/// Absolute times in a feed are otherwise unchecked, so one bad timestamp could move a trip
/// anywhere in the window. Real delays can reach hours, but not half a day:
/// a trip that late is canceled, or replaced, in practice.
pub const DEFAULT_MAX_PREDICTION_OFFSET: i64 = 12 * 3600;

/// Choices the overlay leaves to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayOptions {
    /// The minimum change time at stops that exist only in real-time data.
    pub new_stop_min_change: Seconds,
    /// See [`DEFAULT_MAX_PREDICTION_OFFSET`].
    pub max_prediction_offset: i64,
}

impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            new_stop_min_change: Seconds::ZERO,
            max_prediction_offset: DEFAULT_MAX_PREDICTION_OFFSET,
        }
    }
}

/// Why an overlay couldn't be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OverlayError {
    /// The lookup's days and the base window's days differ in number.
    #[error("match a lookup of {lookup} days with a window of {window} days")]
    WindowMismatch {
        /// Days in the lookup.
        lookup: usize,
        /// Days in the window.
        window: usize,
    },
    /// The base window has more days than a [`DaySlot`] can number.
    #[error("number a window of {0} days")]
    TooManyDays(usize),
    /// There were more added trips than can be numbered.
    #[error("number more added trips than a u32 can hold")]
    TripIdsExhausted,
    /// The overlay builder rejected what we gave it.
    ///
    /// The static lookup and base window disagree, or an arena outgrew its offsets.
    #[error("build the overlay: {0}")]
    Build(#[from] BuildError),
}

/// A stop that exists only in real-time data.
///
/// Street search snaps each one when the overlay is built, so stop tables can reach it.
#[derive(Debug, Clone, PartialEq)]
pub struct NewStop {
    /// The stop's number in this overlay's snapshot.
    pub stop: StopId,
    /// The stop's feed-scoped identifier.
    pub id: ScopedId,
    /// Latitude, in degrees.
    pub latitude: f32,
    /// Longitude, in degrees.
    pub longitude: f32,
    /// Everything else the `Stop` entity said.
    pub details: proto::Stop,
}

/// What one active `NO_SERVICE` alert removed from the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AlertImpact {
    /// How many trips the alert removed entirely.
    pub trips_removed: usize,
    /// How many single stop visits the alert removed (stop-scoped selectors).
    pub stops_removed: usize,
}

/// What building an overlay did, for reporting and monitoring.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlayStats {
    /// Trip updates considered, after duplicates were dropped.
    pub trip_updates: usize,
    /// Trip updates that matched a static trip on its service day.
    pub trip_updates_matched: usize,
    /// Trip updates naming a trip that doesn't exist or doesn't run that day.
    pub trip_updates_unmatched: usize,
    /// Trip updates for a service date outside the window.
    pub trip_updates_outside_window: usize,
    /// Trip updates dropped because a newer one named the same trip run.
    pub duplicate_trip_updates: usize,
    /// Trip updates naming no trip at all.
    pub trip_updates_without_trip: usize,
    /// `NEW`, `ADDED`, `REPLACEMENT`, or `DUPLICATED` updates that couldn't be used.
    pub invalid_added_trips: usize,
    /// `TripModifications` that couldn't be applied to a trip.
    pub invalid_modifications: usize,
    /// `Stop` entities with unusable coordinates.
    pub invalid_stops: usize,
    /// Stop time updates that matched no stop of their trip.
    pub stop_updates_unmatched: u64,
    /// Times moved to keep a trip's times in order.
    pub times_repaired: u64,
    /// Event times outside the window's range, which were ignored.
    pub times_out_of_range: u64,
    /// Vehicles carried for display.
    pub vehicles: usize,
    /// Vehicles matched to a static trip.
    pub vehicles_matched: usize,
    /// Alerts carried for display.
    pub alerts: usize,
    /// `NO_SERVICE` alerts active at some time in the window.
    pub no_service_alerts: usize,
    /// What each `NO_SERVICE` alert removed, by alert index in the rider store.
    pub alert_impacts: BTreeMap<usize, AlertImpact>,
    /// Stops appended to the numbering.
    pub new_stops: usize,
    /// Distinct routes given a replacement block.
    pub routes_changed: usize,
    /// Replacement blocks laid out in the arena.
    pub blocks_replaced: usize,
    /// Blocks emptied by removals.
    pub blocks_removed: usize,
    /// Trips whose times or stop rules changed in place.
    pub trips_updated: usize,
    /// Trips canceled or removed by an alert.
    pub trips_removed: usize,
    /// Trips moved to an added pattern or added outright.
    pub trips_added: usize,
    /// Stop visits skipped.
    pub stops_skipped: usize,
    /// Added stop patterns.
    pub added_routes: usize,
    /// Lanes laid out, across all replacement blocks.
    pub lanes: usize,
    /// The most lanes in any replacement block.
    pub max_lanes: usize,
    /// Replacement blocks with more lanes than their distinct stop rules need,
    /// meaning trips overtake one another.
    pub overtaking_blocks: usize,
    /// Of those, blocks with more overtaking lanes than their schedule.
    pub overtaking_blocks_new: usize,
    /// Lanes beyond one per distinct rule set, across replacement blocks: the cost of overtaking.
    pub overtaking_lanes: usize,
    /// The overlay arena's size.
    pub arena: ArenaUsage,
}

/// The result of building an overlay.
#[derive(Debug, Clone)]
pub struct RealtimeOverlay {
    /// The timetable state a query pins.
    pub snapshot: Arc<Snapshot>,
    /// Everything for the response layer, apart from the hot path.
    pub rider: RiderData,
    /// Stops that exist only in real-time data, to be snapped to the street network.
    pub new_stops: Vec<NewStop>,
    /// What the build did.
    pub stats: OverlayStats,
}

/// The latest a `DUPLICATED` trip's `start_time` may be, in seconds of its service day.
const MAX_DUPLICATE_START_TIME: u32 = 3 * 86_400;

/// Input a change pipeline carries for each feed's data.
#[derive(Debug, Default)]
struct FreshFeeds {
    trip_updates: Vec<(FeedId, TripUpdate)>,
    vehicles: Vec<(FeedId, proto::VehiclePosition)>,
    alerts: Vec<(FeedId, String, proto::Alert)>,
    stops: Vec<(FeedId, proto::Stop)>,
    modifications: Vec<(FeedId, String, TripModifications)>,
}

/// What happens to a static trip in its base route's block.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TripChange {
    /// New times or stop rules on the same stops.
    Times {
        events: Vec<StopEvent>,
        state: TripState,
    },
    /// Removed from the block, for good.
    Removed { state: TripState },
    /// Removed from the block because it moved to an added pattern.
    Moved,
}

#[derive(Debug, Default)]
struct BlockChange {
    trips: HashMap<TripId, TripChange>,
    /// Trips added to this block outright (duplicates on the same pattern).
    added: Vec<ResolvedTrip>,
}

#[derive(Debug, Clone)]
struct AddedTrip {
    trip: TripId,
    key: TripKey,
    events: Vec<StopEvent>,
    feed: FeedId,
    route_id: Option<String>,
    direction_id: Option<u32>,
    state: TripState,
}

/// One added pattern's service, by day.
type AddedDays = BTreeMap<DaySlot, Vec<AddedTrip>>;

struct Context<'a, L: StaticLookup + ?Sized> {
    lookup: &'a L,
    base: Arc<BaseWindow>,
    clocks: Vec<DayClock>,
    now: i64,
    options: OverlayOptions,
    next_trip_id: u32,
    blocks: HashMap<(RouteId, DaySlot), Vec<ScheduledTrip>>,
    new_stops: HashMap<ScopedId, StopId>,
    modified: HashMap<(TripId, DaySlot), ModifiedLayout>,
    modified_refs: HashMap<(TripId, DaySlot), (TripRef, FeedId)>,
    added_index: HashMap<(ScopedId, CalendarDate), (TripId, DaySlot)>,
    changes: BTreeMap<(RouteId, DaySlot), BlockChange>,
    added: BTreeMap<(Vec<StopId>, RouteAttributes), AddedDays>,
    rider: RiderData,
    stats: OverlayStats,
}

/// Builds the overlay for `feeds` over `base`.
///
/// `lookup` must describe the same timetable as `base`.
/// `now` is the time, in POSIX seconds, used to judge staleness and to place trips with no
/// `start_date`; it is the only clock this function uses.
///
/// # Errors
///
/// If `lookup` and `base` disagree, or the overlay can't be laid out.
/// Feed content never causes an error: what can't be used is counted in the stats and dropped.
pub fn build_overlay<L: StaticLookup + ?Sized>(
    lookup: &L,
    base: Arc<BaseWindow>,
    feeds: Vec<FeedInput>,
    now: i64,
    options: OverlayOptions,
) -> Result<RealtimeOverlay, OverlayError> {
    let days = lookup.window_days();
    if days.len() != base.days().len() {
        return Err(OverlayError::WindowMismatch {
            lookup: days.len(),
            window: base.days().len(),
        });
    }
    let mut clocks = Vec::with_capacity(days.len());
    for (slot, (day, service)) in days.iter().zip(base.days()).enumerate() {
        let slot = u8::try_from(slot).map_err(|_| OverlayError::TooManyDays(days.len()))?;
        clocks.push(DayClock::new(*day, DaySlot::new(slot), service.offset));
    }

    let mut statuses = Vec::with_capacity(feeds.len());
    let mut fresh = FreshFeeds::default();
    for input in feeds {
        let status = input.status(now);
        let stale = status.stale;
        statuses.push(status);
        if stale {
            continue;
        }
        sort_entities(input, &mut fresh);
    }

    let mut overlay = OverlayBuilder::new(Arc::clone(&base));
    let mut context = Context {
        lookup,
        base,
        clocks,
        now,
        options,
        next_trip_id: lookup.first_unused_trip_id(),
        blocks: HashMap::new(),
        new_stops: HashMap::new(),
        modified: HashMap::new(),
        modified_refs: HashMap::new(),
        added_index: HashMap::new(),
        changes: BTreeMap::new(),
        added: BTreeMap::new(),
        rider: RiderData::default(),
        stats: OverlayStats::default(),
    };

    let new_stops = context.add_new_stops(fresh.stops, &mut overlay, options)?;
    context.apply_modifications(fresh.modifications);
    context.apply_trip_updates(fresh.trip_updates)?;
    context.apply_remaining_modifications();
    context.add_vehicles(fresh.vehicles);
    let alerts = context.add_alerts(fresh.alerts);
    let block_alerts = context.candidate_alert_blocks(&alerts);
    context.lay_out_blocks(&mut overlay, &alerts, &block_alerts)?;
    context.lay_out_added_routes(&mut overlay, &alerts)?;

    context.rider.set_feeds(statuses);
    let snapshot = overlay.build()?;
    context.stats.arena = snapshot.overlay_usage();
    context.stats.new_stops = new_stops.len();
    Ok(RealtimeOverlay {
        snapshot,
        rider: context.rider,
        new_stops,
        stats: context.stats,
    })
}

fn sort_entities(input: FeedInput, fresh: &mut FreshFeeds) {
    let feed = input.feed;
    let Ok(decoded) = input.decoded else {
        return;
    };
    for entity in decoded.into_message().entity {
        if entity.is_deleted == Some(true) {
            continue;
        }
        if let Some(update) = entity.trip_update {
            fresh.trip_updates.push((feed.clone(), update));
        }
        if let Some(vehicle) = entity.vehicle {
            fresh.vehicles.push((feed.clone(), vehicle));
        }
        if let Some(alert) = entity.alert {
            fresh.alerts.push((feed.clone(), entity.id.clone(), alert));
        }
        if let Some(stop) = entity.stop {
            fresh.stops.push((feed.clone(), stop));
        }
        if let Some(modifications) = entity.trip_modifications {
            fresh
                .modifications
                .push((feed.clone(), entity.id.clone(), modifications));
        }
    }
}

/// The stop a feed's identifier refers to: static first, then this overlay's own.
fn find_stop<L: StaticLookup + ?Sized>(
    lookup: &L,
    new_stops: &HashMap<ScopedId, StopId>,
    feed: &FeedId,
    stop_id: &str,
) -> Option<StopId> {
    lookup
        .stop(feed, stop_id)
        .or_else(|| new_stops.get(&ScopedId::new(feed, stop_id)).copied())
}

impl<L: StaticLookup + ?Sized> Context<'_, L> {
    fn clock_on(&self, date: CalendarDate) -> Option<DayClock> {
        self.clocks.iter().copied().find(|c| c.date == date)
    }

    fn trip_key(&self, trip: TripId, day: DaySlot) -> TripKey {
        let date = self
            .base
            .days()
            .get(usize::from(day.into_inner()))
            .map_or_else(
                // A slot only ever comes from `clocks`, which is built from the base window.
                || unreachable!("day slot out of the window"),
                |d| d.date,
            );
        TripKey { trip, date }
    }

    fn allocate_trip_id(&mut self) -> Result<TripId, OverlayError> {
        let id = self.next_trip_id;
        self.next_trip_id = id.checked_add(1).ok_or(OverlayError::TripIdsExhausted)?;
        Ok(TripId::new(id))
    }

    /// Appends every usable `Stop` that the static feed doesn't have.
    fn add_new_stops(
        &mut self,
        stops: Vec<(FeedId, proto::Stop)>,
        overlay: &mut OverlayBuilder,
        options: OverlayOptions,
    ) -> Result<Vec<NewStop>, OverlayError> {
        let mut stops = stops;
        // Sorted, so numbering doesn't depend on the feed's entity order.
        stops.sort_by(|(a_feed, a), (b_feed, b)| {
            (a_feed, a.stop_id.as_deref()).cmp(&(b_feed, b.stop_id.as_deref()))
        });
        let mut out = Vec::new();
        for (feed, stop) in stops {
            let Some(stop_id) = stop.stop_id.as_deref() else {
                self.stats.invalid_stops += 1;
                continue;
            };
            if self.lookup.stop(&feed, stop_id).is_some() {
                continue;
            }
            let scoped = ScopedId::new(&feed, stop_id);
            if self.new_stops.contains_key(&scoped) {
                continue;
            }
            let (Some(latitude), Some(longitude)) = (stop.stop_lat, stop.stop_lon) else {
                self.stats.invalid_stops += 1;
                continue;
            };
            if !valid_coordinates(latitude, longitude) {
                self.stats.invalid_stops += 1;
                continue;
            }
            let id = overlay.add_stop(options.new_stop_min_change)?;
            self.new_stops.insert(scoped.clone(), id);
            out.push(NewStop {
                stop: id,
                id: scoped,
                latitude,
                longitude,
                details: stop,
            });
        }
        Ok(out)
    }

    fn block(&mut self, route: RouteId, day: DaySlot) -> &[ScheduledTrip] {
        let lookup = self.lookup;
        self.blocks
            .entry((route, day))
            .or_insert_with(|| lookup.scheduled_block(route, day))
    }

    fn find_trip(&mut self, reference: TripRef) -> Option<ScheduledTrip> {
        self.block(reference.route, reference.day)
            .iter()
            .find(|t| t.trip == reference.trip)
            .cloned()
    }

    /// Finds a static trip, and if no `start_date` was given, picks the most likely day.
    fn resolve_trip(
        &self,
        feed: &FeedId,
        trip_id: &str,
        date: Option<CalendarDate>,
        start_time: Option<u32>,
    ) -> Result<Option<TripRef>, Resolution> {
        if let Some(date) = date {
            if self.clock_on(date).is_none() {
                return Err(Resolution::OutsideWindow);
            }
            return Ok(self.lookup.trip(
                feed,
                &TripQuery {
                    trip_id,
                    date,
                    start_time,
                },
            ));
        }
        // No date: of the days this trip runs, take the one whose run is closest to now.
        let mut best: Option<(i64, TripRef)> = None;
        for clock in &self.clocks {
            let Some(found) = self.lookup.trip(
                feed,
                &TripQuery {
                    trip_id,
                    date: clock.date,
                    start_time,
                },
            ) else {
                continue;
            };
            let from = clock.unix(found.first_departure.into_inner());
            let to = clock.unix(found.last_arrival.into_inner());
            let distance = if self.now < from {
                from - self.now
            } else {
                (self.now - to).max(0)
            };
            if best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, found));
            }
        }
        Ok(best.map(|(_, found)| found))
    }

    fn apply_modifications(&mut self, modifications: Vec<(FeedId, String, TripModifications)>) {
        for (feed, _, set) in modifications {
            let dates: Vec<CalendarDate> = set
                .service_dates
                .iter()
                .filter_map(|d| CalendarDate::parse(d))
                .collect();
            let start_times: Vec<Option<u32>> = if set.start_times.is_empty() {
                vec![None]
            } else {
                set.start_times
                    .iter()
                    .map(|t| parse_start_time(t))
                    .collect()
            };
            for selected in &set.selected_trips {
                for trip_id in &selected.trip_ids {
                    for &date in &dates {
                        for &start_time in &start_times {
                            self.apply_modification(&feed, trip_id, date, start_time, &set);
                        }
                    }
                }
            }
        }
    }

    fn apply_modification(
        &mut self,
        feed: &FeedId,
        trip_id: &str,
        date: CalendarDate,
        start_time: Option<u32>,
        set: &TripModifications,
    ) {
        if self.clock_on(date).is_none() {
            return;
        }
        let Some(found) = self.lookup.trip(
            feed,
            &TripQuery {
                trip_id,
                date,
                start_time,
            },
        ) else {
            self.stats.invalid_modifications += 1;
            return;
        };
        let route_stops = self.base.route_stops(found.route).map(<[StopId]>::to_vec);
        let (Some(route_stops), Some(scheduled)) = (route_stops, self.find_trip(found)) else {
            self.stats.invalid_modifications += 1;
            return;
        };
        let lookup = self.lookup;
        let new_stops = &self.new_stops;
        let resolve = |id: &str| find_stop(lookup, new_stops, feed, id);
        match apply_modifications(
            &route_stops,
            &scheduled.events,
            &scheduled.stop_sequences,
            &set.modifications,
            self.options.max_prediction_offset,
            &resolve,
        ) {
            Ok(layout) => {
                self.modified.insert((found.trip, found.day), layout);
                self.modified_refs
                    .insert((found.trip, found.day), (found, feed.clone()));
            }
            Err(_) => self.stats.invalid_modifications += 1,
        }
    }

    fn apply_trip_updates(
        &mut self,
        updates: Vec<(FeedId, TripUpdate)>,
    ) -> Result<(), OverlayError> {
        let mut updates = updates;
        // Trip identity, then time, so that the newest update of each run comes last.
        updates.sort_by(|(a_feed, a), (b_feed, b)| {
            (a_feed, identity_key(&a.trip), a.timestamp).cmp(&(
                b_feed,
                identity_key(&b.trip),
                b.timestamp,
            ))
        });
        let mut iter = updates.into_iter().peekable();
        while let Some((feed, update)) = iter.next() {
            if iter.peek().is_some_and(|(f, next)| {
                f == &feed && identity_key(&next.trip) == identity_key(&update.trip)
            }) {
                self.stats.duplicate_trip_updates += 1;
                continue;
            }
            self.stats.trip_updates += 1;
            self.apply_trip_update(&feed, &update)?;
        }
        Ok(())
    }

    // `ADDED` is deprecated in the specification, but feeds in the wild still send it.
    #[allow(deprecated)]
    fn apply_trip_update(
        &mut self,
        feed: &FeedId,
        update: &TripUpdate,
    ) -> Result<(), OverlayError> {
        let (trip_id, date_text, start_time_text) = effective_identity(&update.trip);
        let Some(trip_id) = trip_id.map(str::to_owned) else {
            self.stats.trip_updates_without_trip += 1;
            return Ok(());
        };
        let date = date_text.and_then(CalendarDate::parse);
        let start_time = start_time_text.and_then(parse_start_time);
        let relationship = update.trip.schedule_relationship();
        match relationship {
            TripRelationship::Added | TripRelationship::New | TripRelationship::Replacement => {
                self.apply_added(feed, &trip_id, date, start_time, relationship, update)
            }
            TripRelationship::Duplicated => self.apply_duplicated(feed, &trip_id, date, update),
            TripRelationship::Scheduled
            | TripRelationship::Unscheduled
            | TripRelationship::Canceled
            | TripRelationship::Deleted => {
                self.apply_to_existing(feed, &trip_id, date, start_time, relationship, update);
                Ok(())
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn apply_to_existing(
        &mut self,
        feed: &FeedId,
        trip_id: &str,
        date: Option<CalendarDate>,
        start_time: Option<u32>,
        relationship: TripRelationship,
        update: &TripUpdate,
    ) {
        let reference = match self.resolve_trip(feed, trip_id, date, start_time) {
            Ok(Some(reference)) => reference,
            Ok(None) => {
                self.stats.trip_updates_unmatched += 1;
                self.push_unmatched(feed, trip_id, date, start_time, update);
                return;
            }
            Err(Resolution::OutsideWindow) => {
                self.stats.trip_updates_outside_window += 1;
                self.push_unmatched(feed, trip_id, date, start_time, update);
                return;
            }
        };
        let Some(scheduled) = self.find_trip(reference) else {
            // The lookup named a trip its own block doesn't have.
            self.stats.trip_updates_unmatched += 1;
            self.push_unmatched(feed, trip_id, date, start_time, update);
            return;
        };
        self.stats.trip_updates_matched += 1;
        let key = self.trip_key(reference.trip, reference.day);

        if matches!(
            relationship,
            TripRelationship::Canceled | TripRelationship::Deleted
        ) {
            // A cancellation outranks any detour planned for the trip.
            self.modified.remove(&(reference.trip, reference.day));
            self.changes
                .entry((reference.route, reference.day))
                .or_default()
                .trips
                .insert(
                    reference.trip,
                    TripChange::Removed {
                        state: TripState::Canceled,
                    },
                );
            let details = trip_details(feed, trip_id, date, start_time, update, Some(key), None);
            self.rider.push_trip(TripDetails {
                state: Some(TripState::Canceled),
                route: Some(reference.route),
                ..details
            });
            self.rider.set_state(key, TripState::Canceled);
            return;
        }

        let clock = self.clocks[usize::from(reference.day.into_inner())];
        let lookup = self.lookup;
        let new_stops = &self.new_stops;
        let resolve = |id: &str| find_stop(lookup, new_stops, feed, id);
        let layout = self.modified.get(&(reference.trip, reference.day)).cloned();
        let was_modified = layout.is_some();
        let (mut stops, base_events, sequences): (Vec<StopId>, Vec<StopEvent>, Vec<Option<u32>>) =
            match layout {
                Some(layout) => (layout.stops, layout.events, layout.sequences),
                None => (
                    self.base
                        .route_stops(reference.route)
                        .map(<[StopId]>::to_vec)
                        .unwrap_or_default(),
                    scheduled.events.clone(),
                    scheduled.stop_sequences.iter().map(|&s| Some(s)).collect(),
                ),
            };
        // `assigned_stop_id` changes which stop a position calls at, so the stop pattern changes.
        let mut pattern_changed = was_modified;
        for stop_update in &update.stop_time_update {
            let Some(assigned) = stop_update
                .stop_time_properties
                .as_ref()
                .and_then(|p| p.assigned_stop_id.as_deref())
            else {
                continue;
            };
            let Some(new_stop) = resolve(assigned) else {
                self.stats.stop_updates_unmatched += 1;
                continue;
            };
            // Locate the position the same way an update matches it.
            let position = stop_update
                .stop_sequence
                .and_then(|sequence| sequences.iter().position(|&s| s == Some(sequence)))
                .or_else(|| {
                    stop_update
                        .stop_id
                        .as_deref()
                        .and_then(&resolve)
                        .and_then(|old| stops.iter().position(|&s| s == old))
                });
            if let Some(position) = position
                && stops.get(position) != Some(&new_stop)
            {
                stops[position] = new_stop;
                pattern_changed = true;
            }
        }

        let Ok(propagated) = propagate(
            &Trip {
                events: &base_events,
                sequences: &sequences,
                stops: &stops,
                max_offset: self.options.max_prediction_offset,
            },
            &update.stop_time_update,
            update.delay,
            clock,
            &resolve,
        ) else {
            // Delays pushed a time out of range: the update isn't usable, so fall back.
            self.stats.times_out_of_range += 1;
            self.push_unmatched(feed, trip_id, date, start_time, update);
            return;
        };
        self.stats.stop_updates_unmatched += u64::from(propagated.unmatched);
        self.stats.times_repaired += u64::from(propagated.repaired);
        self.stats.times_out_of_range += u64::from(propagated.out_of_range);

        let skipped = propagated
            .events
            .iter()
            .any(|e| e.rules == StopRules::SKIPPED);
        let changed = propagated.differs_from(&scheduled.events) || was_modified;
        let state = if pattern_changed {
            TripState::Replaced
        } else if skipped {
            TripState::StopSkipped
        } else {
            TripState::Delayed
        };
        let details = trip_details(
            feed,
            trip_id,
            date,
            start_time,
            update,
            Some(key),
            Some((&propagated, &base_events, &sequences, clock)),
        );
        let details = self.with_stop_names(feed, details, &stops);
        self.rider.push_trip(TripDetails {
            state: changed.then_some(state),
            route: Some(reference.route),
            route_id: details
                .route_id
                .clone()
                .or_else(|| route_id_of(lookup, reference.route)),
            ..details
        });

        if !changed && !pattern_changed {
            return;
        }
        self.rider.set_state(key, state);
        if pattern_changed {
            self.modified.remove(&(reference.trip, reference.day));
            self.push_moved(
                feed,
                reference,
                stops,
                propagated.events,
                state,
                key,
                &update.trip,
            );
        } else {
            self.changes
                .entry((reference.route, reference.day))
                .or_default()
                .trips
                .insert(
                    reference.trip,
                    TripChange::Times {
                        events: propagated.events,
                        state,
                    },
                );
        }
    }

    /// Moves a static trip off its route block and onto an added pattern.
    #[allow(clippy::too_many_arguments)]
    fn push_moved(
        &mut self,
        feed: &FeedId,
        reference: TripRef,
        stops: Vec<StopId>,
        events: Vec<StopEvent>,
        state: TripState,
        key: TripKey,
        trip: &TripDescriptor,
    ) {
        self.changes
            .entry((reference.route, reference.day))
            .or_default()
            .trips
            .insert(reference.trip, TripChange::Moved);
        let attributes = self
            .base
            .route_attributes(reference.route)
            .unwrap_or(RouteAttributes::NONE);
        self.added
            .entry((stops, attributes))
            .or_default()
            .entry(reference.day)
            .or_default()
            .push(AddedTrip {
                trip: reference.trip,
                key,
                events,
                feed: feed.clone(),
                route_id: trip.route_id.clone().or_else(|| {
                    self.lookup
                        .route_info(reference.route)
                        .map(|info| info.route_id.to_owned())
                }),
                direction_id: trip.direction_id,
                state,
            });
    }

    /// A `NEW`, `ADDED`, or `REPLACEMENT` trip: its stop time updates define the whole trip.
    fn apply_added(
        &mut self,
        feed: &FeedId,
        trip_id: &str,
        date: Option<CalendarDate>,
        start_time: Option<u32>,
        relationship: TripRelationship,
        update: &TripUpdate,
    ) -> Result<(), OverlayError> {
        let Ok(original) = self.resolve_trip(feed, trip_id, date, start_time) else {
            self.stats.trip_updates_outside_window += 1;
            self.push_unmatched(feed, trip_id, date, start_time, update);
            return Ok(());
        };
        // Some producers still mark a trip that exists in the static feed as `ADDED`;
        // treating any match as a replacement avoids running it twice.
        let date_or_original = date.or_else(|| {
            original.map(|reference| self.clocks[usize::from(reference.day.into_inner())].date)
        });

        let lookup = self.lookup;
        let new_stops = &self.new_stops;
        let clocks = &self.clocks;
        let resolve = |id: &str| find_stop(lookup, new_stops, feed, id);
        let Some(built) = build_added_events(
            &update.stop_time_update,
            date_or_original,
            clocks,
            self.options.max_prediction_offset,
            &resolve,
        ) else {
            self.stats.invalid_added_trips += 1;
            self.push_unmatched(feed, trip_id, date, start_time, update);
            return Ok(());
        };
        let (stops, events, clock) = built;

        let (trip, route, state) = match (original, relationship) {
            (Some(reference), _) => (reference.trip, Some(reference), TripState::Replaced),
            (None, TripRelationship::Replacement) => {
                // A replacement of a trip we can't find still needs a home.
                (self.allocate_trip_id()?, None, TripState::Replaced)
            }
            (None, _) => (self.allocate_trip_id()?, None, TripState::Added),
        };
        let day = route.map_or(clock.slot, |reference| reference.day);

        let key = self.trip_key(trip, day);
        let details = trip_details(feed, trip_id, date, start_time, update, Some(key), None);
        self.rider.push_trip(TripDetails {
            state: Some(state),
            route: route.map(|r| r.route),
            ..details
        });
        self.rider.set_state(key, state);
        if let Some(date) = date {
            self.added_index
                .insert((ScopedId::new(feed, trip_id), date), (trip, day));
        }
        if let Some(reference) = route {
            self.stats.trip_updates_matched += 1;
            // This update defines the whole trip, so any planned detour is superseded.
            self.modified.remove(&(reference.trip, reference.day));
            self.push_moved(feed, reference, stops, events, state, key, &update.trip);
            return Ok(());
        }

        // A brand-new trip: copy its route's attributes if its route is known.
        let route_id = update.trip.route_id.clone();
        let attributes = route_id
            .as_deref()
            .and_then(|id| self.lookup.route_patterns(feed, id).first().copied())
            .and_then(|route| self.base.route_attributes(route))
            .unwrap_or(RouteAttributes::NONE);
        self.added
            .entry((stops, attributes))
            .or_default()
            .entry(day)
            .or_default()
            .push(AddedTrip {
                trip,
                key,
                events,
                feed: feed.clone(),
                route_id,
                direction_id: update.trip.direction_id,
                state,
            });
        Ok(())
    }

    /// A `DUPLICATED` trip: a copy of a scheduled one, usually on another day or time.
    #[allow(clippy::too_many_lines)]
    fn apply_duplicated(
        &mut self,
        feed: &FeedId,
        trip_id: &str,
        date: Option<CalendarDate>,
        update: &TripUpdate,
    ) -> Result<(), OverlayError> {
        let properties = update.trip_properties.clone().unwrap_or_default();
        let new_date = properties
            .start_date
            .as_deref()
            .and_then(CalendarDate::parse);
        // GTFS times stop short of this, so anything later is a nonsense start time.
        let new_time = properties
            .start_time
            .as_deref()
            .and_then(parse_start_time)
            .filter(|&time| time <= MAX_DUPLICATE_START_TIME);
        // The template may run on the original date only, so try that first.
        let source_days = date
            .into_iter()
            .chain(new_date)
            .chain(self.clocks.iter().map(|c| c.date));
        let mut template = None;
        for day in source_days {
            if let Ok(Some(found)) = self.resolve_trip(feed, trip_id, Some(day), None) {
                template = Some(found);
                break;
            }
        }
        let Some(template) = template else {
            self.stats.invalid_added_trips += 1;
            self.push_unmatched(feed, trip_id, date, None, update);
            return Ok(());
        };
        let target_clock = new_date.and_then(|d| self.clock_on(d));
        let Some(target_clock) = target_clock else {
            self.stats.invalid_added_trips += 1;
            self.push_unmatched(feed, trip_id, date, None, update);
            return Ok(());
        };
        let Some(scheduled) = self.find_trip(template) else {
            self.stats.invalid_added_trips += 1;
            self.push_unmatched(feed, trip_id, date, None, update);
            return Ok(());
        };
        let source_clock = self.clocks[usize::from(template.day.into_inner())];
        let first = scheduled.events.first().map(|e| e.departure.into_inner());
        // With no new start time the copy keeps the template's time of day.
        let shift = match (new_time, first) {
            (Some(new_time), Some(first)) => i64::from(new_time) - relative(source_clock, first),
            _ => 0,
        };

        let Some(events) = shifted_events(&scheduled.events, source_clock, target_clock, shift)
        else {
            self.stats.invalid_added_trips += 1;
            self.push_unmatched(feed, trip_id, date, None, update);
            return Ok(());
        };

        let new_id = self.allocate_trip_id()?;
        let key = self.trip_key(new_id, target_clock.slot);
        let lookup = self.lookup;
        let new_stops = &self.new_stops;
        let resolve = |id: &str| find_stop(lookup, new_stops, feed, id);
        let stops = self
            .base
            .route_stops(template.route)
            .map(<[StopId]>::to_vec)
            .unwrap_or_default();
        let sequences: Vec<Option<u32>> =
            scheduled.stop_sequences.iter().map(|&s| Some(s)).collect();
        let Ok(propagated) = propagate(
            &Trip {
                events: &events,
                sequences: &sequences,
                stops: &stops,
                max_offset: self.options.max_prediction_offset,
            },
            &update.stop_time_update,
            update.delay,
            target_clock,
            &resolve,
        ) else {
            self.stats.invalid_added_trips += 1;
            self.push_unmatched(feed, trip_id, date, None, update);
            return Ok(());
        };

        self.stats.trip_updates_matched += 1;
        self.stats.times_repaired += u64::from(propagated.repaired);
        let new_trip_id = properties
            .trip_id
            .clone()
            .unwrap_or_else(|| trip_id.to_owned());
        let details = trip_details(
            feed,
            &new_trip_id,
            new_date,
            new_time,
            update,
            Some(key),
            Some((&propagated, &events, &sequences, target_clock)),
        );
        let details = self.with_stop_names(feed, details, &stops);
        self.rider.push_trip(TripDetails {
            state: Some(TripState::Added),
            route: Some(template.route),
            ..details
        });
        self.rider.set_state(key, TripState::Added);
        if let Some(date) = new_date {
            self.added_index.insert(
                (ScopedId::new(feed, &new_trip_id), date),
                (new_id, target_clock.slot),
            );
        }
        self.changes
            .entry((template.route, target_clock.slot))
            .or_default()
            .added
            .push(ResolvedTrip {
                trip: new_id,
                events: propagated.events,
            });
        Ok(())
    }

    /// A trip the feed mentions but we could not use: still shown to riders.
    fn push_unmatched(
        &mut self,
        feed: &FeedId,
        trip_id: &str,
        date: Option<CalendarDate>,
        start_time: Option<u32>,
        update: &TripUpdate,
    ) {
        let details = trip_details(feed, trip_id, date, start_time, update, None, None);
        self.rider.push_trip(details);
    }

    /// Trips with modifications but no trip update still run the detoured pattern.
    fn apply_remaining_modifications(&mut self) {
        let mut remaining: Vec<((TripId, DaySlot), ModifiedLayout)> =
            self.modified.drain().collect();
        remaining.sort_by_key(|((trip, day), _)| (*trip, *day));
        for ((trip, day), layout) in remaining {
            let Some((reference, feed)) = self.modified_refs.get(&(trip, day)).cloned() else {
                self.stats.invalid_modifications += 1;
                continue;
            };
            let key = self.trip_key(trip, day);
            self.rider.set_state(key, TripState::Replaced);
            self.push_moved(
                &feed,
                reference,
                layout.stops,
                layout.events,
                TripState::Replaced,
                key,
                &TripDescriptor::default(),
            );
        }
    }

    /// Fills in the stop identifier of positions the feed didn't name.
    fn with_stop_names(
        &self,
        feed: &FeedId,
        mut details: TripDetails,
        stops: &[StopId],
    ) -> TripDetails {
        for (stop, stop_details) in stops.iter().zip(&mut details.stops) {
            if stop_details.stop_id.is_none() {
                stop_details.stop_id = self
                    .lookup
                    .stop_gtfs_id(feed, *stop)
                    .map(str::to_owned)
                    .or_else(|| {
                        self.new_stops
                            .iter()
                            .find(|(scoped, id)| **id == *stop && scoped.feed == *feed)
                            .map(|(scoped, _)| scoped.id.to_string())
                    });
            }
        }
        details
    }

    fn add_vehicles(&mut self, vehicles: Vec<(FeedId, proto::VehiclePosition)>) {
        for (feed, vehicle) in vehicles {
            self.stats.vehicles += 1;
            let descriptor = vehicle.trip.as_ref();
            let trip_id = descriptor.and_then(|d| d.trip_id.clone());
            let start_date = descriptor
                .and_then(|d| d.start_date.as_deref())
                .and_then(CalendarDate::parse);
            let start_time = descriptor
                .and_then(|d| d.start_time.as_deref())
                .and_then(parse_start_time);
            let reference = trip_id.as_deref().and_then(|id| {
                self.resolve_trip(&feed, id, start_date, start_time)
                    .ok()
                    .flatten()
            });
            let trip_key = reference.map(|r| self.trip_key(r.trip, r.day));
            if trip_key.is_some() {
                self.stats.vehicles_matched += 1;
            }
            let position = vehicle.position.as_ref();
            let details = VehicleDetails {
                feed: Some(feed),
                vehicle: vehicle
                    .vehicle
                    .as_ref()
                    .map(vehicle_ref)
                    .unwrap_or_default(),
                trip_id,
                route_id: descriptor
                    .and_then(|d| d.route_id.clone())
                    .or_else(|| reference.and_then(|r| route_id_of(self.lookup, r.route))),
                start_date,
                trip_key,
                latitude: position.map(|p| p.latitude),
                longitude: position.map(|p| p.longitude),
                bearing: position.and_then(|p| p.bearing),
                speed: position.and_then(|p| p.speed),
                odometer: position.and_then(|p| p.odometer),
                current_stop_sequence: vehicle.current_stop_sequence,
                stop_id: vehicle.stop_id.clone(),
                current_status: vehicle.current_status.map(|_| vehicle.current_status()),
                timestamp: unix_time(vehicle.timestamp),
                congestion: vehicle.congestion_level.map(|_| vehicle.congestion_level()),
                occupancy: vehicle.occupancy_status.map(|_| vehicle.occupancy_status()),
                occupancy_percentage: vehicle.occupancy_percentage,
                carriages: vehicle.multi_carriage_details.clone(),
            };
            self.rider.push_vehicle(details);
        }
    }

    /// Stores every alert for display, and returns those that remove service.
    #[allow(deprecated)]
    fn add_alerts(&mut self, alerts: Vec<(FeedId, String, proto::Alert)>) -> Vec<NoServiceAlert> {
        let mut compiled = Vec::new();
        for (feed, entity_id, alert) in alerts {
            self.stats.alerts += 1;
            let lookup = self.lookup;
            let new_stops = &self.new_stops;
            let resolve = |id: &str| find_stop(lookup, new_stops, &feed, id);
            let rider_index = self.rider.alerts().len();
            if let Some(no_service) = compile_no_service(
                &alert,
                &feed,
                rider_index,
                lookup,
                &self.clocks,
                &self.added_index,
                &resolve,
            ) {
                self.stats
                    .alert_impacts
                    .insert(rider_index, AlertImpact::default());
                compiled.push(no_service);
            }
            let details = AlertDetails {
                feed: Some(feed),
                entity_id,
                active_periods: alert.active_period.iter().map(Period::from_range).collect(),
                communication_periods: alert
                    .communication_period
                    .iter()
                    .map(Period::from_range)
                    .collect(),
                impact_periods: alert.impact_period.iter().map(Period::from_range).collect(),
                cause: alert.cause.map(|_| alert.cause()),
                effect: alert.effect.map(|_| alert.effect()),
                severity: alert.severity_level.map(|_| alert.severity_level()),
                informed: alert.informed_entity.iter().map(informed_entity).collect(),
                url: alert.url,
                header: alert.header_text,
                description: alert.description_text,
                tts_header: alert.tts_header_text,
                tts_description: alert.tts_description_text,
                image: alert.image,
                image_alternative_text: alert.image_alternative_text,
                cause_detail: alert.cause_detail,
                effect_detail: alert.effect_detail,
            };
            self.rider.push_alert(details);
        }
        self.stats.no_service_alerts = compiled.len();
        compiled
    }

    /// Which `(route, day)` blocks each alert might change, as indices into `alerts`.
    fn candidate_alert_blocks(
        &self,
        alerts: &[NoServiceAlert],
    ) -> BTreeMap<(RouteId, DaySlot), Vec<usize>> {
        let mut blocks: BTreeMap<(RouteId, DaySlot), Vec<usize>> = BTreeMap::new();
        if alerts.is_empty() {
            return blocks;
        }
        let route_count = u32::try_from(self.base.route_count()).unwrap_or(u32::MAX);
        let mut inverted: Option<HashMap<StopId, Vec<RouteId>>> = None;
        for (index, alert) in alerts.iter().enumerate() {
            let base = &self.base;
            let mut routes_of_stop = |stop: StopId| {
                inverted
                    .get_or_insert_with(|| {
                        let mut map: HashMap<StopId, Vec<RouteId>> = HashMap::new();
                        for raw in 0..route_count {
                            let route = RouteId::new(raw);
                            for &stop in base.route_stops(route).unwrap_or(&[]) {
                                map.entry(stop).or_default().push(route);
                            }
                        }
                        map
                    })
                    .get(&stop)
                    .cloned()
                    .unwrap_or_default()
            };
            for route in alert.candidate_routes(self.lookup, route_count, &mut routes_of_stop) {
                for clock in &self.clocks {
                    blocks.entry((route, clock.slot)).or_default().push(index);
                }
            }
        }
        blocks
    }

    /// Lays out replacement blocks for base routes, in `(route, day)` order.
    #[allow(clippy::too_many_lines)]
    fn lay_out_blocks(
        &mut self,
        overlay: &mut OverlayBuilder,
        alerts: &[NoServiceAlert],
        block_alerts: &BTreeMap<(RouteId, DaySlot), Vec<usize>>,
    ) -> Result<(), OverlayError> {
        let keys: BTreeSet<(RouteId, DaySlot)> = self
            .changes
            .keys()
            .chain(block_alerts.keys())
            .copied()
            .collect();
        let lookup = self.lookup;
        let base = Arc::clone(&self.base);
        let mut routes_changed = BTreeSet::new();
        for key in keys {
            let (route, day) = key;
            let change = self.changes.remove(&key).unwrap_or_default();
            let scheduled = self
                .blocks
                .remove(&key)
                .unwrap_or_else(|| lookup.scheduled_block(route, day));
            let clock = self.clocks[usize::from(day.into_inner())];
            let info = lookup.route_info(route);
            let stops = base.route_stops(route).unwrap_or(&[]);
            let relevant = block_alerts.get(&key).map_or(&[][..], Vec::as_slice);

            let mut changed = !change.added.is_empty();
            let mut trips = Vec::with_capacity(scheduled.len() + change.added.len());
            for trip in &scheduled {
                let plan = change.trips.get(&trip.trip);
                let mut events = match plan {
                    Some(TripChange::Times { events, .. }) => {
                        changed = true;
                        events.clone()
                    }
                    Some(TripChange::Removed { .. }) => {
                        changed = true;
                        self.stats.trips_removed += 1;
                        continue;
                    }
                    Some(TripChange::Moved) => {
                        changed = true;
                        continue;
                    }
                    None => trip.events.clone(),
                };
                let key_of_trip = self.trip_key(trip.trip, day);
                let effect = match info {
                    Some(info) => combined_effect(
                        relevant.iter().filter_map(|&i| alerts.get(i)),
                        scheduled_identity(info.feed, trip, Some(info), day),
                        stops,
                        &events,
                        clock,
                        &mut self.stats.alert_impacts,
                    ),
                    None => AlertEffect::default(),
                };
                if effect.remove {
                    changed = true;
                    self.stats.trips_removed += 1;
                    self.rider.set_state(key_of_trip, TripState::NoService);
                    continue;
                }
                if !effect.stops_removed.is_empty() {
                    changed = true;
                    for position in effect.stops_removed {
                        if let Some(event) = events.get_mut(position) {
                            event.rules = StopRules::SKIPPED;
                        }
                    }
                    self.rider.set_state(key_of_trip, TripState::StopSkipped);
                }
                if plan.is_some() || events != trip.events {
                    self.stats.trips_updated += 1;
                }
                self.stats.stops_skipped += events
                    .iter()
                    .zip(&trip.events)
                    .filter(|(now, was)| {
                        now.rules == StopRules::SKIPPED && was.rules != StopRules::SKIPPED
                    })
                    .count();
                trips.push(ResolvedTrip {
                    trip: trip.trip,
                    events,
                });
            }
            self.stats.trips_added += change.added.len();
            trips.extend(change.added);
            if !changed {
                continue;
            }
            routes_changed.insert(route);
            if trips.is_empty() {
                overlay.remove_service(route, day)?;
                self.stats.blocks_removed += 1;
                continue;
            }
            let rule_groups = distinct_rule_sets(&trips);
            let block = ResolvedBlock::from_trips(trips)?;
            let base_extra = base_overtaking_lanes(&scheduled);
            self.record_block(block.lanes().len(), rule_groups, base_extra);
            overlay.replace_block(route, day, &block)?;
        }
        self.stats.routes_changed += routes_changed.len();
        Ok(())
    }

    /// Lays out added patterns after every base route, so route order holds in the arena.
    fn lay_out_added_routes(
        &mut self,
        overlay: &mut OverlayBuilder,
        alerts: &[NoServiceAlert],
    ) -> Result<(), OverlayError> {
        let lookup = self.lookup;
        let added = core::mem::take(&mut self.added);
        for ((stops, attributes), days) in added {
            let mut laid_out: Vec<(DaySlot, Vec<ResolvedTrip>, Vec<TripKey>)> = Vec::new();
            for (day, trips) in days {
                let clock = self.clocks[usize::from(day.into_inner())];
                let mut resolved = Vec::with_capacity(trips.len());
                let mut keys = Vec::with_capacity(trips.len());
                for trip in trips {
                    let info = trip.route_id.as_deref().and_then(|id| {
                        let pattern = lookup.route_patterns(&trip.feed, id).first().copied()?;
                        lookup.route_info(pattern)
                    });
                    let identity = TripIdentity {
                        feed: &trip.feed,
                        route: info,
                        direction: trip.direction_id,
                        trip: trip.trip,
                        day,
                    };
                    let effect = combined_effect(
                        alerts.iter(),
                        identity,
                        &stops,
                        &trip.events,
                        clock,
                        &mut self.stats.alert_impacts,
                    );
                    if effect.remove {
                        self.stats.trips_removed += 1;
                        self.rider.set_state(trip.key, TripState::NoService);
                        continue;
                    }
                    let mut events = trip.events;
                    for position in effect.stops_removed {
                        if let Some(event) = events.get_mut(position) {
                            event.rules = StopRules::SKIPPED;
                        }
                    }
                    self.rider.set_state(trip.key, trip.state);
                    self.stats.trips_added += 1;
                    keys.push(trip.key);
                    resolved.push(ResolvedTrip {
                        trip: trip.trip,
                        events,
                    });
                }
                if !resolved.is_empty() {
                    laid_out.push((day, resolved, keys));
                }
            }
            if laid_out.is_empty() {
                continue;
            }
            let route = overlay.add_route(&stops, attributes)?;
            self.stats.added_routes += 1;
            self.stats.routes_changed += 1;
            for (day, trips, keys) in laid_out {
                let rule_groups = distinct_rule_sets(&trips);
                let block = ResolvedBlock::from_trips(trips)?;
                self.record_block(block.lanes().len(), rule_groups, 0);
                overlay.replace_block(route, day, &block)?;
                for key in keys {
                    self.rider.set_trip_route(key, route);
                }
            }
        }
        Ok(())
    }

    fn record_block(&mut self, lanes: usize, rule_groups: usize, base_extra: usize) {
        self.stats.blocks_replaced += 1;
        self.stats.lanes += lanes;
        self.stats.max_lanes = self.stats.max_lanes.max(lanes);
        let extra = lanes.saturating_sub(rule_groups);
        if extra > 0 {
            self.stats.overtaking_blocks += 1;
            self.stats.overtaking_lanes += extra;
        }
        if extra > base_extra {
            self.stats.overtaking_blocks_new += 1;
        }
    }
}

/// Why a trip couldn't be looked up at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    OutsideWindow,
}

/// Evaluates alerts against one trip, crediting each alert with what it does.
fn combined_effect<'a>(
    alerts: impl Iterator<Item = &'a NoServiceAlert>,
    identity: TripIdentity<'_>,
    stops: &[StopId],
    events: &[StopEvent],
    clock: DayClock,
    impacts: &mut BTreeMap<usize, AlertImpact>,
) -> AlertEffect {
    let mut combined = AlertEffect::default();
    for alert in alerts {
        let effect = alert.effect_on(identity, stops, events, clock);
        let impact = impacts.entry(alert.rider_index).or_default();
        if effect.remove {
            impact.trips_removed += 1;
            combined.remove = true;
        }
        impact.stops_removed += effect.stops_removed.len();
        combined.stops_removed.extend(effect.stops_removed);
    }
    combined.stops_removed.sort_unstable();
    combined.stops_removed.dedup();
    combined
}

fn distinct_rule_sets(trips: &[ResolvedTrip]) -> usize {
    trips
        .iter()
        .map(|trip| trip.events.iter().map(|e| e.rules).collect::<Vec<_>>())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// The lanes beyond one per rule set that the schedule alone needs.
fn base_overtaking_lanes(scheduled: &[ScheduledTrip]) -> usize {
    let trips: Vec<ResolvedTrip> = scheduled
        .iter()
        .map(|trip| ResolvedTrip {
            trip: trip.trip,
            events: trip.events.clone(),
        })
        .collect();
    let groups = distinct_rule_sets(&trips);
    ResolvedBlock::from_trips(trips).map_or(0, |block| block.lanes().len().saturating_sub(groups))
}

/// The trip identity GTFS-RT gives: the descriptor's, or its `modified_trip` selector's.
fn effective_identity(trip: &TripDescriptor) -> (Option<&str>, Option<&str>, Option<&str>) {
    match trip.modified_trip.as_ref() {
        Some(modified) => (
            modified.affected_trip_id.as_deref(),
            modified.start_date.as_deref(),
            modified.start_time.as_deref(),
        ),
        None => (
            trip.trip_id.as_deref(),
            trip.start_date.as_deref(),
            trip.start_time.as_deref(),
        ),
    }
}

fn identity_key(trip: &TripDescriptor) -> (Option<&str>, Option<&str>, Option<&str>) {
    effective_identity(trip)
}

fn valid_coordinates(latitude: f32, longitude: f32) -> bool {
    latitude.is_finite()
        && longitude.is_finite()
        && (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
}

/// An added trip's event, in POSIX seconds: its `time`, else `scheduled_time` plus `delay`.
fn added_event_unix(event: Option<&StopTimeEvent>) -> Option<i64> {
    let event = event?;
    if let Some(time) = event.time.filter(|&time| time > 0) {
        return Some(time);
    }
    let scheduled = event.scheduled_time.filter(|&time| time > 0)?;
    Some(scheduled.saturating_add(i64::from(event.delay.unwrap_or_default())))
}

/// Builds an added trip's stops and events from its stop time updates.
///
/// Every stop must be resolvable and timed, since nothing else defines this trip,
/// and within `max_offset` seconds of the two days its service day can reach.
fn build_added_events(
    updates: &[StopTimeUpdate],
    date: Option<CalendarDate>,
    clocks: &[DayClock],
    max_offset: i64,
    resolve: &dyn Fn(&str) -> Option<StopId>,
) -> Option<(Vec<StopId>, Vec<StopEvent>, DayClock)> {
    let mut stops = Vec::with_capacity(updates.len());
    let mut unix = Vec::with_capacity(updates.len());
    let mut rules = Vec::with_capacity(updates.len());
    for update in updates {
        if update.schedule_relationship() == StopRelationship::Skipped {
            continue;
        }
        let stop_id = update
            .stop_time_properties
            .as_ref()
            .and_then(|p| p.assigned_stop_id.as_deref())
            .or(update.stop_id.as_deref())?;
        stops.push(resolve(stop_id)?);
        let arrival = added_event_unix(update.arrival.as_ref());
        let departure = added_event_unix(update.departure.as_ref());
        let (arrival, departure) = match (arrival, departure) {
            (Some(a), Some(d)) => (a, d),
            (Some(a), None) => (a, a),
            (None, Some(d)) => (d, d),
            (None, None) => return None,
        };
        unix.push((arrival, departure));
        rules.push(rules_of(Some(update), StopRules::REGULAR));
    }
    if stops.len() < 2 {
        return None;
    }
    let first = unix.first()?.0;
    let clock = match date {
        Some(date) => clocks.iter().copied().find(|c| c.date == date)?,
        None => clocks
            .iter()
            .rev()
            .copied()
            .find(|c| c.start_unix() <= first)?,
    };
    if unix.iter().any(|&(arrival, departure)| {
        !within_service_day(arrival, clock, max_offset)
            || !within_service_day(departure, clock, max_offset)
    }) {
        return None;
    }
    let mut arrivals: Vec<i64> = unix.iter().map(|&(a, _)| clock.window_seconds(a)).collect();
    let mut departures: Vec<i64> = unix.iter().map(|&(_, d)| clock.window_seconds(d)).collect();
    repair(&mut arrivals, &mut departures, Some(0));
    let mut events = Vec::with_capacity(stops.len());
    for ((&arrival, &departure), &rule) in arrivals.iter().zip(&departures).zip(&rules) {
        events.push(StopEvent {
            arrival: window_time(arrival).ok()?,
            departure: window_time(departure).ok()?,
            rules: rule,
        });
    }
    Some((stops, events, clock))
}

/// Whether a POSIX time could belong to `clock`'s service day, within `max_offset` seconds.
///
/// GTFS times run past midnight, so a service day spans up to two calendar days.
fn within_service_day(unix: i64, clock: DayClock, max_offset: i64) -> bool {
    let from = clock.start_unix().saturating_sub(max_offset);
    let to = clock
        .start_unix()
        .saturating_add(2 * 86_400)
        .saturating_add(max_offset);
    (from..=to).contains(&unix)
}

/// Seconds from the start of `clock`'s service day to a window time on it.
fn relative(clock: DayClock, seconds: i32) -> i64 {
    clock.unix(seconds) - clock.start_unix()
}

/// A trip's events moved from one service day to another, plus `shift` seconds.
fn shifted_events(
    events: &[StopEvent],
    source: DayClock,
    target: DayClock,
    shift: i64,
) -> Option<Vec<StopEvent>> {
    let convert = |time: Time| {
        let seconds = relative(source, time.into_inner()) + shift + target.start_unix();
        window_time(target.window_seconds(seconds)).ok()
    };
    events
        .iter()
        .map(|event| {
            Some(StopEvent {
                arrival: convert(event.arrival)?,
                departure: convert(event.departure)?,
                rules: event.rules,
            })
        })
        .collect()
}

fn unix_time(time: Option<u64>) -> Option<i64> {
    time.filter(|&t| t != 0).and_then(|t| i64::try_from(t).ok())
}

fn vehicle_ref(vehicle: &proto::VehicleDescriptor) -> VehicleRef {
    VehicleRef {
        id: vehicle.id.clone(),
        label: vehicle.label.clone(),
        license_plate: vehicle.license_plate.clone(),
        wheelchair: vehicle
            .wheelchair_accessible
            .map(|_| vehicle.wheelchair_accessible()),
    }
}

fn informed_entity(selector: &proto::EntitySelector) -> InformedEntity {
    let trip = selector.trip.as_ref();
    InformedEntity {
        agency_id: selector.agency_id.clone(),
        route_id: selector.route_id.clone(),
        route_type: selector.route_type,
        direction_id: selector.direction_id,
        trip_id: trip.and_then(|t| t.trip_id.clone()),
        trip_date: trip
            .and_then(|t| t.start_date.as_deref())
            .and_then(CalendarDate::parse),
        stop_id: selector.stop_id.clone(),
    }
}

/// Delay of a prediction against its schedule, in seconds.
fn delay_of(predicted: Option<Time>, scheduled: Time) -> Option<i32> {
    predicted.map(|time| time.into_inner() - scheduled.into_inner())
}

/// The propagation result and layout that a matched trip's rider details are built from.
type Layout<'a> = (&'a Propagated, &'a [StopEvent], &'a [Option<u32>], DayClock);

/// Builds a trip's rider details, per stop position when it matched the timetable.
fn trip_details(
    feed: &FeedId,
    trip_id: &str,
    date: Option<CalendarDate>,
    start_time: Option<u32>,
    update: &TripUpdate,
    key: Option<TripKey>,
    layout: Option<Layout<'_>>,
) -> TripDetails {
    let descriptor = &update.trip;
    let stops = match layout {
        Some((propagated, events, sequences, clock)) => (0..propagated.events.len())
            .map(|position| {
                let prediction = propagated.predictions[position];
                let stop_update = prediction
                    .update
                    .and_then(|index| update.stop_time_update.get(index));
                let scheduled = events[position];
                let mut details = stop_update.map(raw_stop_details).unwrap_or_default();
                details.position = u32::try_from(position).ok();
                details.stop_sequence = sequences[position];
                details.scheduled_arrival = Some(clock.unix(scheduled.arrival.into_inner()));
                details.scheduled_departure = Some(clock.unix(scheduled.departure.into_inner()));
                details.predicted_arrival =
                    prediction.arrival.map(|time| clock.unix(time.into_inner()));
                details.predicted_departure = prediction
                    .departure
                    .map(|time| clock.unix(time.into_inner()));
                details.arrival_delay = details
                    .arrival_delay
                    .or_else(|| delay_of(prediction.arrival, scheduled.arrival));
                details.departure_delay = details
                    .departure_delay
                    .or_else(|| delay_of(prediction.departure, scheduled.departure));
                details
            })
            .collect(),
        None => update
            .stop_time_update
            .iter()
            .map(raw_stop_details)
            .collect(),
    };
    TripDetails {
        feed: Some(feed.clone()),
        trip_id: trip_id.to_owned(),
        route_id: descriptor.route_id.clone(),
        start_date: date,
        start_time,
        direction_id: descriptor.direction_id,
        state: None,
        trip_key: key,
        route: None,
        relationship: descriptor
            .schedule_relationship
            .map(|_| descriptor.schedule_relationship()),
        timestamp: unix_time(update.timestamp),
        delay: update.delay,
        vehicle: update.vehicle.as_ref().map(vehicle_ref),
        headsign: update
            .trip_properties
            .as_ref()
            .and_then(|p| p.trip_headsign.clone()),
        stops,
    }
}

/// The GTFS route a stop pattern belongs to.
fn route_id_of<L: StaticLookup + ?Sized>(lookup: &L, route: RouteId) -> Option<String> {
    lookup
        .route_info(route)
        .map(|info| info.route_id.to_owned())
}

/// A stop update as the feed gave it, before any matching to the timetable.
fn raw_stop_details(update: &StopTimeUpdate) -> StopTimeDetails {
    let properties = update.stop_time_properties.as_ref();
    StopTimeDetails {
        stop_sequence: update.stop_sequence,
        stop_id: update.stop_id.clone(),
        relationship: Some(update.schedule_relationship()),
        predicted_arrival: update
            .arrival
            .as_ref()
            .and_then(|e| e.time)
            .filter(|&t| t > 0),
        predicted_departure: update
            .departure
            .as_ref()
            .and_then(|e| e.time)
            .filter(|&t| t > 0),
        arrival_delay: update.arrival.as_ref().and_then(|e| e.delay),
        departure_delay: update.departure.as_ref().and_then(|e| e.delay),
        arrival_uncertainty: update.arrival.as_ref().and_then(|e| e.uncertainty),
        departure_uncertainty: update.departure.as_ref().and_then(|e| e.uncertainty),
        occupancy: update
            .departure_occupancy_status
            .map(|_| update.departure_occupancy_status()),
        assigned_stop_id: properties.and_then(|p| p.assigned_stop_id.clone()),
        stop_headsign: properties.and_then(|p| p.stop_headsign.clone()),
        ..StopTimeDetails::default()
    }
}
