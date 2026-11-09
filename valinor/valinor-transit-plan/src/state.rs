//! The owner of timetable state, and the immutable snapshots a query pins.
//!
//! [`TransitState`] holds everything that changes over time: the current GTFS-RT messages,
//! the materialized service windows, and the latest [`TransitSnapshot`].
//! It uses no runtime, opens no connections, and reads no clock; `now` is always an argument.
//! The caller decides how bytes arrive and when to rebuild,
//! which is what lets a server's poller and a phone's foreground refresh share it.
//!
//! # Lifecycle
//!
//! A snapshot is immutable once published; a query pins one with [`TransitState::pin`] and holds
//! the `Arc` for all of its searches, however many later refreshes happen meanwhile.
//! The states and transitions are:
//!
//! | From | Event | To |
//! |---|---|---|
//! | (none) | first `pin` or `refresh` | Scheduled |
//! | Scheduled | fresh bytes ingested | Updated (real-time applied) |
//! | Updated | newer bytes ingested | Updated |
//! | Updated | feed passes its age limit | Stale (that feed falls back to the schedule) |
//! | Stale | fresh bytes ingested | Updated |
//! | Updated | undecodable bytes ingested | Undecodable (that feed falls back to the schedule) |
//! | Undecodable | decodable bytes ingested | Updated |
//! | Updated | feed cleared | Scheduled |
//! | any | service date changes at a query's `now` | window rolled over; feeds kept |
//!
//! Each of these transitions has a test in `scenario_tests.rs`.
//!
//! # Concurrency
//!
//! The published pointer sits behind a read-write lock that only guards cloning an `Arc`,
//! and rebuilds are serialized by a separate mutex.
//! A pin that finds its snapshot behind only on feed bytes serves it while a rebuild is in
//! flight, so queries don't wait for newer real-time data.
//! It waits only when the service date has rolled over or a feed's freshness has flipped,
//! because then the current snapshot would be wrong, not merely not the newest.

use core::sync::atomic::{AtomicU64, Ordering};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock, TryLockError};

use valinor_gtfs::{LoadedTimetable, TimetableSet};
use valinor_gtfs_rt::{
    DecodeError, FeedId, FeedInput, FeedKind, FeedStatus, NewStop, OverlayOptions, OverlayStats,
    RiderData, build_overlay,
};
use valinor_transit::{Snapshot, StopId, Timetable};
use valinor_walk::{SnappedStop, StopSnaps, TransferTable, WalkError};

use crate::error::StateError;
use crate::lookup::CompiledLookup;
use crate::plan::TravelMode;
use crate::response::{VehicleUpdate, vehicle_update};
use crate::window::{WindowData, dates_around, required_dates};

/// Materialized windows kept for reuse: the current one, its predecessor, and an on-demand one.
///
/// A window is the expensive thing to build, so rollovers and repeat on-demand dates reuse it.
const WINDOW_CACHE_SIZE: usize = 3;

/// On-demand snapshots kept, each tied to one state of the feeds.
const ON_DEMAND_CACHE_SIZE: usize = 2;

/// Fixed choices made when a [`TransitState`] is created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StateConfig {
    /// Passed through to the overlay builder.
    pub overlay: OverlayOptions,
}

/// Bytes for one feed, for [`TransitState::ingest`].
#[derive(Debug, Clone, Copy)]
pub struct FeedUpdate<'a> {
    /// The feed these belong to, by its ID in the compiled timetable.
    pub feed: &'a str,
    /// What the bytes carry.
    pub kind: FeedKind,
    /// The feed message; `None` clears this feed.
    pub bytes: Option<&'a [u8]>,
    /// When the bytes were received, in POSIX seconds.
    ///
    /// A message's own header timestamp takes precedence for judging its age.
    pub received_at: i64,
    /// Overrides the default age limit for this kind of feed, in seconds.
    pub stale_after: Option<i64>,
}

/// What [`TransitState::ingest`] did with some bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    /// The bytes decoded and will take effect at the next rebuild.
    Accepted,
    /// There were no bytes, so the feed was cleared.
    Cleared,
    /// The bytes couldn't be decoded. The feed falls back to the schedule, and the reason
    /// is reported in every response until better bytes arrive.
    Undecodable(DecodeError),
}

/// What [`TransitState::ingest_and_refresh`] did.
#[derive(Debug)]
pub struct IngestAndRefresh {
    /// What became of the bytes.
    pub outcome: IngestOutcome,
    /// The snapshot built from them, or why none could be built.
    pub snapshot: Result<Arc<TransitSnapshot>, StateError>,
}

/// Snaps stops to the street network for a travel mode.
///
/// The state owner has no tiles; a [`crate::StreetPlanner`] provides this, and tests can fake it.
pub trait StopSnapper: core::fmt::Debug + Send + Sync {
    /// Snaps every stop, by position, for building the static table.
    ///
    /// # Errors
    ///
    /// If a tile can't be read, or a stop lands in a tile of a different build.
    fn snap_all(&self, mode: TravelMode, stops: &[(f64, f64)]) -> Result<StopSnaps, WalkError>;

    /// Snaps one stop that arrived with a real-time overlay.
    ///
    /// # Errors
    ///
    /// If a tile can't be read.
    fn snap_one(&self, mode: TravelMode, ll: (f64, f64)) -> Result<SnappedStop, WalkError>;

    /// The walks between nearby stops for a travel mode, in the snapshot's stop numbering.
    ///
    /// In production these were loaded from transfer tiles; tests may build them in memory.
    ///
    /// # Errors
    ///
    /// If a tile can't be read, or the snaps are of a different build.
    fn base_transfers(
        &self,
        mode: TravelMode,
        snaps: &Arc<StopSnaps>,
    ) -> Result<Arc<TransferTable>, WalkError>;

    /// A table that adds an overlay's new stops to `base`, for `snaps` that cover them.
    ///
    /// # Errors
    ///
    /// If a tile can't be read, or the snaps are of a different build.
    fn add_transfers(
        &self,
        mode: TravelMode,
        base: &TransferTable,
        snaps: &Arc<StopSnaps>,
        new_stops: &[StopId],
    ) -> Result<TransferTable, WalkError>;
}

/// Each travel mode's snaps of a snapshot's stops.
#[derive(Debug, Clone)]
pub struct ModeSnaps {
    walking: Arc<StopSnaps>,
    cycling: Arc<StopSnaps>,
    walking_transfers: Arc<TransferTable>,
    cycling_transfers: Arc<TransferTable>,
}

impl ModeSnaps {
    /// The snaps for a travel mode.
    pub fn for_mode(&self, mode: TravelMode) -> &Arc<StopSnaps> {
        match mode {
            TravelMode::Walking => &self.walking,
            TravelMode::Cycling => &self.cycling,
        }
    }

    /// The precomputed walks between stops for a travel mode.
    pub fn transfers_for_mode(&self, mode: TravelMode) -> &Arc<TransferTable> {
        match mode {
            TravelMode::Walking => &self.walking_transfers,
            TravelMode::Cycling => &self.cycling_transfers,
        }
    }

    fn with_new_stops(
        &self,
        snapper: &dyn StopSnapper,
        new_stops: &[NewStop],
    ) -> Result<Self, WalkError> {
        if new_stops.is_empty() {
            return Ok(self.clone());
        }
        // Shares the static table's cost only: a new-stop overlay is rare.
        let (mut walking, mut cycling) = ((*self.walking).clone(), (*self.cycling).clone());
        for stop in new_stops {
            let ll = (f64::from(stop.longitude), f64::from(stop.latitude));
            walking.insert(stop.stop, snapper.snap_one(TravelMode::Walking, ll)?)?;
            cycling.insert(stop.stop, snapper.snap_one(TravelMode::Cycling, ll)?)?;
        }
        let (walking, cycling) = (Arc::new(walking), Arc::new(cycling));
        let stops: Vec<StopId> = new_stops.iter().map(|stop| stop.stop).collect();
        let walking_transfers = snapper.add_transfers(
            TravelMode::Walking,
            &self.walking_transfers,
            &walking,
            &stops,
        )?;
        let cycling_transfers = snapper.add_transfers(
            TravelMode::Cycling,
            &self.cycling_transfers,
            &cycling,
            &stops,
        )?;
        Ok(Self {
            walking,
            cycling,
            walking_transfers: Arc::new(walking_transfers),
            cycling_transfers: Arc::new(cycling_transfers),
        })
    }
}

/// A stop that a street search can snap: base stops and an overlay's new ones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StopLocation {
    /// The stop's number in the snapshot.
    pub stop: StopId,
    /// Degrees north.
    pub latitude: f64,
    /// Degrees east.
    pub longitude: f64,
}

/// The compiled timetables and everything derived once from them.
#[derive(Debug)]
pub(crate) struct TimetableData {
    /// Every loaded file, numbered as one.
    pub(crate) set: TimetableSet,
    /// Each file's service, by the file's position in the set.
    pub(crate) timetables: Vec<Timetable>,
    /// Every feed in the set.
    pub(crate) feeds: Vec<FeedId>,
}

/// Immutable timetable state: everything one query reads.
///
/// Clones of the `Arc` are cheap; a query holds one for its whole duration.
#[derive(Debug)]
pub struct TransitSnapshot {
    pub(crate) data: Arc<TimetableData>,
    pub(crate) window: Arc<WindowData>,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) rider: RiderData,
    pub(crate) new_stops: Vec<NewStop>,
    pub(crate) stats: OverlayStats,
    pub(crate) built_at: i64,
    pub(crate) snaps: Option<ModeSnaps>,
    /// The dates this window was chosen for, for a snapshot of the current moment.
    ///
    /// `None` for an on-demand window, which never rolls over.
    center: Option<Vec<valinor_transit::ServiceDate>>,
    feed_generation: u64,
}

impl TransitSnapshot {
    /// Each mode's snaps of this snapshot's stops, when a street network is attached.
    ///
    /// These cover the overlay's new stops, and are bound to the tile build they were made on.
    pub fn snaps(&self) -> Option<&ModeSnaps> {
        self.snaps.as_ref()
    }

    /// The timetable state a search pins.
    pub fn snapshot(&self) -> &Arc<Snapshot> {
        &self.snapshot
    }

    /// Everything real-time for the response layer.
    pub fn rider(&self) -> &RiderData {
        &self.rider
    }

    /// What the overlay build did.
    pub fn stats(&self) -> &OverlayStats {
        &self.stats
    }

    /// When this snapshot was built, in POSIX seconds.
    pub fn built_at(&self) -> i64 {
        self.built_at
    }

    /// The POSIX time of the window clock's zero.
    pub fn origin_unix(&self) -> i64 {
        self.window.origin_unix
    }

    /// Stops that exist only in real-time data.
    pub fn new_stops(&self) -> &[NewStop] {
        &self.new_stops
    }

    /// Each feed's status as of `now`.
    ///
    /// Staleness and decodability decided at the build are what took effect on routing,
    /// and a pin rebuilds whenever those flip, so these agree with what a query read.
    pub fn feed_status_at(&self, now: i64) -> Vec<FeedStatus> {
        self.rider
            .feed_status()
            .iter()
            .map(|status| status.aged_to(self.built_at, now))
            .collect()
    }

    /// Every stop a street search can snap, in the snapshot's stop numbering.
    ///
    /// Stops the file holds without coordinates are omitted.
    pub fn stop_locations(&self) -> impl Iterator<Item = StopLocation> + '_ {
        self.data
            .set
            .parts()
            .flat_map(|part| {
                let count = u32::try_from(part.compiled().stop_count()).unwrap_or(u32::MAX);
                (0..count).filter_map(move |raw| {
                    let local = StopId::new(raw);
                    let coordinate = part.compiled().stop(local)?.coordinate?;
                    Some(StopLocation {
                        stop: part.global_stop(local),
                        latitude: coordinate.lat,
                        longitude: coordinate.lon,
                    })
                })
            })
            .chain(self.new_stops.iter().map(|stop| StopLocation {
                stop: stop.stop,
                latitude: f64::from(stop.latitude),
                longitude: f64::from(stop.longitude),
            }))
    }
}

/// Owns timetable state and publishes snapshots; see the module documentation.
#[derive(Debug)]
pub struct TransitState {
    data: Arc<TimetableData>,
    config: StateConfig,
    current: RwLock<Option<Arc<TransitSnapshot>>>,
    pub(crate) writer: Mutex<Writer>,
    snapper: Option<Box<dyn StopSnapper>>,
    /// The static stops' snaps, made once.
    base_snaps: Option<ModeSnaps>,
    /// Bumped by every ingest, so a pin can see a snapshot is behind without taking the mutex.
    feed_generation: AtomicU64,
}

#[derive(Debug, Default)]
pub(crate) struct Writer {
    feeds: HashMap<(FeedId, FeedKind), FeedInput>,
    /// Most recently used first.
    windows: Vec<Arc<WindowData>>,
    /// Most recently used first.
    on_demand: Vec<Arc<TransitSnapshot>>,
}

impl TransitState {
    /// Takes ownership of one loaded timetable.
    ///
    /// # Performance
    ///
    /// Copies every stop event into a [`Timetable`] once. A window is materialized on first use.
    ///
    /// # Errors
    ///
    /// If `valinor-transit` rejects the file's tables.
    pub fn new(loaded: LoadedTimetable, config: StateConfig) -> Result<Self, StateError> {
        Self::from_set(TimetableSet::single(loaded)?, config)
    }

    /// Takes ownership of several loaded timetables, numbered as one.
    ///
    /// # Performance
    ///
    /// Copies every stop event of every file once. A window is materialized on first use.
    ///
    /// # Errors
    ///
    /// If `valinor-transit` rejects a file's tables.
    pub fn from_set(set: TimetableSet, config: StateConfig) -> Result<Self, StateError> {
        Self::build(set, config, None)
    }

    /// Takes ownership of timetables and snaps their stops to a street network.
    ///
    /// New stops in overlays are snapped with `snapper` when each overlay is built.
    ///
    /// # Performance
    ///
    /// Snaps every stop for each travel mode once, which is the slow part of startup.
    ///
    /// # Errors
    ///
    /// If `valinor-transit` rejects a file's tables, or a stop can't be snapped.
    pub fn with_snapper(
        set: TimetableSet,
        config: StateConfig,
        snapper: Box<dyn StopSnapper>,
    ) -> Result<Self, StateError> {
        Self::build(set, config, Some(snapper))
    }

    fn build(
        set: TimetableSet,
        config: StateConfig,
        snapper: Option<Box<dyn StopSnapper>>,
    ) -> Result<Self, StateError> {
        let timetables = set
            .parts()
            .map(|part| part.compiled().to_timetable())
            .collect::<Result<Vec<_>, _>>()?;
        let mut feeds = Vec::new();
        for (id, _) in set.feeds() {
            let feed = FeedId::new(id).map_err(|_| {
                StateError::Timetable(valinor_gtfs::DecodeError::InvalidValue {
                    table: "feeds",
                    reason: "a feed has an empty identifier",
                })
            })?;
            feeds.push(feed);
        }
        let base_snaps = match &snapper {
            None => None,
            Some(snapper) => {
                let points = stop_points(&set);
                let walking = Arc::new(snapper.snap_all(TravelMode::Walking, &points)?);
                let cycling = Arc::new(snapper.snap_all(TravelMode::Cycling, &points)?);
                let walking_transfers = snapper.base_transfers(TravelMode::Walking, &walking)?;
                let cycling_transfers = snapper.base_transfers(TravelMode::Cycling, &cycling)?;
                Some(ModeSnaps {
                    walking,
                    cycling,
                    walking_transfers,
                    cycling_transfers,
                })
            }
        };
        Ok(Self {
            data: Arc::new(TimetableData {
                set,
                timetables,
                feeds,
            }),
            config,
            current: RwLock::new(None),
            writer: Mutex::new(Writer::default()),
            snapper,
            base_snaps,
            feed_generation: AtomicU64::new(0),
        })
    }

    /// The feeds this timetable can take real-time data for.
    pub fn feeds(&self) -> &[FeedId] {
        &self.data.feeds
    }

    /// Replaces what is known about one feed.
    ///
    /// Undecodable bytes replace the feed's previous message: the feed then falls back to the
    /// schedule and reports why, so a rider is never shown an old update as if it were current.
    /// Neither happens until the next rebuild, which [`Self::pin`] does itself if needed.
    ///
    /// # Errors
    ///
    /// If the feed isn't in the compiled timetable.
    pub fn ingest(&self, update: FeedUpdate<'_>) -> Result<IngestOutcome, StateError> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        self.store_update(&mut writer, update)
    }

    /// [`Self::ingest`] and [`Self::refresh`] as one step.
    ///
    /// Queries in the meantime keep serving the previous snapshot, since the rebuild holds
    /// the writer from the moment the new bytes arrive.
    ///
    /// # Errors
    ///
    /// If the feed isn't in the compiled timetable. A rebuild failure is in the result:
    /// by then the bytes have been stored.
    pub fn ingest_and_refresh(
        &self,
        update: FeedUpdate<'_>,
        now: i64,
    ) -> Result<IngestAndRefresh, StateError> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let outcome = self.store_update(&mut writer, update)?;
        let snapshot = self.rebuild(&mut writer, now);
        Ok(IngestAndRefresh { outcome, snapshot })
    }

    fn store_update(
        &self,
        writer: &mut Writer,
        update: FeedUpdate<'_>,
    ) -> Result<IngestOutcome, StateError> {
        let feed = self
            .data
            .feeds
            .iter()
            .find(|feed| feed.as_str() == update.feed)
            .cloned()
            .ok_or_else(|| StateError::UnknownFeed(update.feed.to_owned()))?;
        let key = (feed.clone(), update.kind);
        let outcome = match update.bytes {
            None => {
                writer.feeds.remove(&key);
                IngestOutcome::Cleared
            }
            Some(bytes) => {
                let mut input = FeedInput::from_bytes(feed, update.kind, bytes);
                input.received_at = Some(update.received_at);
                if let Some(stale_after) = update.stale_after {
                    input.stale_after = stale_after;
                }
                let outcome = match &input.decoded {
                    Ok(_) => IngestOutcome::Accepted,
                    Err(error) => IngestOutcome::Undecodable(error.clone()),
                };
                writer.feeds.insert(key, input);
                outcome
            }
        };
        // Inside the lock, so a rebuild reads feeds no older than the generation it records.
        self.feed_generation.fetch_add(1, Ordering::Release);
        Ok(outcome)
    }

    /// Rebuilds and publishes a snapshot for `now` from the current feeds.
    ///
    /// Call this after [`Self::ingest`] from a background thread so queries don't pay for it,
    /// or use [`Self::ingest_and_refresh`].
    ///
    /// # Errors
    ///
    /// If `now` is outside the timetable's coverage, or building fails.
    pub fn refresh(&self, now: i64) -> Result<Arc<TransitSnapshot>, StateError> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        self.rebuild(&mut writer, now)
    }

    /// The snapshot for `now`, rebuilt first if it is out of date.
    ///
    /// A snapshot is out of date if a feed changed, a feed's freshness flipped,
    /// or the service date rolled over. A query calls this once and keeps the `Arc`.
    ///
    /// # Errors
    ///
    /// If `now` is outside the timetable's coverage and no snapshot covers it.
    pub fn pin(&self, now: i64) -> Result<Arc<TransitSnapshot>, StateError> {
        let current = self.read_current();
        if let Some(snapshot) = &current
            && !self.is_out_of_date(snapshot, now)
        {
            return Ok(Arc::clone(snapshot));
        }
        let mut writer = match self.writer.try_lock() {
            Ok(writer) => writer,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                // A rebuild is running. If this snapshot is only behind on feed bytes,
                // that rebuild will catch it up, and a query needn't wait for it.
                if let Some(snapshot) = current
                    && !Self::is_wrong_at(&self.data.set, &snapshot, now)
                {
                    return Ok(snapshot);
                }
                self.writer.lock().unwrap_or_else(PoisonError::into_inner)
            }
        };
        // Somebody else may have rebuilt while we waited for the mutex.
        if let Some(current) = self.read_current()
            && !self.is_out_of_date(&current, now)
        {
            return Ok(current);
        }
        self.rebuild(&mut writer, now)
    }

    /// The snapshot for a query departing between two POSIX times.
    ///
    /// Usually this is the current snapshot. A departure on another date gets a window of its
    /// own, with the same feeds applied, so alerts for that date still take effect.
    ///
    /// # Errors
    ///
    /// If the departure is outside the timetable's coverage, or building fails.
    pub fn pin_for_departure(
        &self,
        now: i64,
        earliest: i64,
        latest: i64,
    ) -> Result<Arc<TransitSnapshot>, StateError> {
        let needed = required_dates(&self.data.set, earliest, latest)?;
        match self.pin(now) {
            Ok(current) if current.window.covers(&needed) => return Ok(current),
            // `now` may legitimately be outside the feed; the departure isn't.
            Ok(_) | Err(StateError::DateNotCovered(_)) => {}
            Err(error) => return Err(error),
        }
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        // Under the lock, so this can't be newer than the feeds the build reads.
        let generation = self.feed_generation.load(Ordering::Acquire);
        if let Some(position) = writer.on_demand.iter().position(|snapshot| {
            snapshot.window.dates == needed
                && snapshot.feed_generation == generation
                && !Self::stale_flipped(snapshot, now)
        }) {
            let found = writer.on_demand.remove(position);
            writer.on_demand.insert(0, Arc::clone(&found));
            return Ok(found);
        }
        let window = self.window(&mut writer, needed)?;
        let built = Arc::new(self.build_snapshot(&writer, window, None, generation, now)?);
        writer.on_demand.insert(0, Arc::clone(&built));
        writer.on_demand.truncate(ON_DEMAND_CACHE_SIZE);
        Ok(built)
    }

    /// The latest real-time state of one vehicle, by its feed and vehicle ID.
    ///
    /// # Errors
    ///
    /// If `now` is outside the timetable's coverage, or building a snapshot fails.
    pub fn vehicle(&self, now: i64, feed: &str, id: &str) -> Result<VehicleUpdate, StateError> {
        let snapshot = self.pin(now)?;
        Ok(vehicle_update(&snapshot, now, feed, id))
    }

    fn read_current(&self) -> Option<Arc<TransitSnapshot>> {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn is_out_of_date(&self, snapshot: &TransitSnapshot, now: i64) -> bool {
        snapshot.feed_generation != self.feed_generation.load(Ordering::Acquire)
            || Self::is_wrong_at(&self.data.set, snapshot, now)
    }

    /// Whether a snapshot is wrong at `now`, rather than only behind the newest feed bytes.
    fn is_wrong_at(set: &TimetableSet, snapshot: &TransitSnapshot, now: i64) -> bool {
        snapshot.center.as_deref() != Some(dates_around(set, now).as_slice())
            || Self::stale_flipped(snapshot, now)
    }

    /// Whether any feed's staleness differs at `now` from what the snapshot applied.
    fn stale_flipped(snapshot: &TransitSnapshot, now: i64) -> bool {
        snapshot
            .rider
            .feed_status()
            .iter()
            .any(|status| status.aged_to(snapshot.built_at, now).stale != status.stale)
    }

    fn rebuild(&self, writer: &mut Writer, now: i64) -> Result<Arc<TransitSnapshot>, StateError> {
        let center = dates_around(&self.data.set, now);
        if center.is_empty() {
            return Err(StateError::DateNotCovered(now));
        }
        let window = self.window(writer, center.clone())?;
        let generation = self.feed_generation.load(Ordering::Acquire);
        let built = Arc::new(self.build_snapshot(writer, window, Some(center), generation, now)?);
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&built));
        Ok(built)
    }

    fn window(
        &self,
        writer: &mut Writer,
        dates: Vec<valinor_transit::ServiceDate>,
    ) -> Result<Arc<WindowData>, StateError> {
        if let Some(position) = writer.windows.iter().position(|w| w.dates == dates) {
            let window = writer.windows.remove(position);
            writer.windows.insert(0, Arc::clone(&window));
            return Ok(window);
        }
        let window = Arc::new(WindowData::materialize(
            &self.data.set,
            &self.data.timetables,
            dates,
        )?);
        writer.windows.insert(0, Arc::clone(&window));
        writer.windows.truncate(WINDOW_CACHE_SIZE);
        Ok(window)
    }

    fn build_snapshot(
        &self,
        writer: &Writer,
        window: Arc<WindowData>,
        center: Option<Vec<valinor_transit::ServiceDate>>,
        feed_generation: u64,
        now: i64,
    ) -> Result<TransitSnapshot, StateError> {
        let lookup = CompiledLookup::new(&self.data, &window);
        let inputs = feeds_in_order(&writer.feeds);
        let overlay = build_overlay(
            &lookup,
            Arc::clone(&window.base),
            inputs,
            now,
            self.config.overlay,
        )?;
        let snaps = match (&self.base_snaps, &self.snapper) {
            (Some(base), Some(snapper)) => {
                Some(base.with_new_stops(snapper.as_ref(), &overlay.new_stops)?)
            }
            _ => None,
        };
        Ok(TransitSnapshot {
            data: Arc::clone(&self.data),
            window,
            snapshot: overlay.snapshot,
            rider: overlay.rider,
            new_stops: overlay.new_stops,
            stats: overlay.stats,
            built_at: now,
            snaps,
            center,
            feed_generation,
        })
    }
}

/// Every static stop's coordinates as `(lon, lat)`, by the set's stop number.
///
/// A stop with no coordinates is at `(0, 0)`, where no tile set has a street, so it stays
/// unsnapped; the table keeps its numbering.
fn stop_points(set: &TimetableSet) -> Vec<(f64, f64)> {
    let mut points = Vec::with_capacity(set.stop_count());
    for part in set.parts() {
        let count = u32::try_from(part.compiled().stop_count()).unwrap_or(u32::MAX);
        points.extend((0..count).map(|raw| {
            part.compiled()
                .stop(StopId::new(raw))
                .and_then(|info| info.coordinate)
                .map_or((0.0, 0.0), |coordinate| (coordinate.lon, coordinate.lat))
        }));
    }
    points
}

/// The feeds in a stable order, so overlays and statuses are deterministic.
fn feeds_in_order(feeds: &HashMap<(FeedId, FeedKind), FeedInput>) -> Vec<FeedInput> {
    let mut inputs: Vec<(&FeedId, u8, &FeedInput)> = feeds
        .iter()
        .map(|((feed, kind), input)| (feed, kind_rank(*kind), input))
        .collect();
    inputs.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    inputs
        .into_iter()
        .map(|(_, _, input)| input.clone())
        .collect()
}

/// A stable order for feed kinds, which have no natural one.
fn kind_rank(kind: FeedKind) -> u8 {
    match kind {
        FeedKind::TripUpdates => 0,
        FeedKind::VehiclePositions => 1,
        FeedKind::Alerts => 2,
        FeedKind::Combined => 3,
    }
}
