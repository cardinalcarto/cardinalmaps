//! The immutable view a query pins: a base window, and possibly an overlay.

use std::sync::Arc;

use crate::ids::{DaySlot, LaneRef, RouteId, ServiceDay, StopId, Time, TripKey};
use crate::timetable::{
    Arena, ArenaUsage, BaseWindow, BlockRef, Lane, RouteAttributes, StopEvent, StopRouteIndex,
    StopRules,
};

/// A timetable window together with any real-time changes to it.
///
/// A query pins one `Arc<Snapshot>`, and every search in its profile reads that same snapshot.
/// The directory is flat over `(route, day)` for base and added routes alike,
/// so a scan never asks which of the two it is reading.
#[derive(Debug)]
pub struct Snapshot {
    pub(crate) base: Arc<BaseWindow>,
    pub(crate) overlay_arena: Arena,
    /// Occurrences of the overlay's added routes, indexed over every stop.
    pub(crate) additions: StopRouteIndex,
    pub(crate) directory: Vec<BlockRef>,
    pub(crate) attributes: Vec<RouteAttributes>,
    /// Per stop, base stops first and then the overlay's.
    pub(crate) min_change: Vec<i32>,
    /// `(start, length)` of each added route's stops in the overlay arena.
    pub(crate) added_route_stops: Vec<(u32, u32)>,
}

impl Snapshot {
    /// A snapshot with the schedule alone.
    pub fn schedule(base: Arc<BaseWindow>) -> Arc<Self> {
        Arc::new(Self {
            directory: base.directory.clone(),
            attributes: base.attributes.clone(),
            min_change: base.min_change.clone(),
            overlay_arena: Arena::new(),
            additions: StopRouteIndex::default(),
            added_route_stops: Vec::new(),
            base,
        })
    }

    /// Every stop this snapshot numbers: the base stops, then the overlay's new ones.
    #[inline]
    pub fn stop_count(&self) -> usize {
        self.min_change.len()
    }

    /// Every route this snapshot numbers: the base routes, then the overlay's new ones.
    #[inline]
    pub fn route_count(&self) -> usize {
        self.attributes.len()
    }

    /// The window's service days, in slot order.
    pub fn days(&self) -> &[ServiceDay] {
        self.base.days()
    }

    /// The base window this snapshot is built on.
    pub fn base(&self) -> &Arc<BaseWindow> {
        &self.base
    }

    /// The stop sequence of a route, whether from the base window or the overlay.
    pub fn route_stops(&self, route: RouteId) -> Option<&[StopId]> {
        let base_routes = self.base.route_count();
        match route.index().checked_sub(base_routes) {
            None => self.base.route_stops(route),
            Some(added) => {
                let &(start, length) = self.added_route_stops.get(added)?;
                self.overlay_arena
                    .stops
                    .get(start as usize..(start + length) as usize)
            }
        }
    }

    /// The route attributes of `route`.
    pub fn route_attributes(&self, route: RouteId) -> Option<RouteAttributes> {
        self.attributes.get(route.index()).copied()
    }

    /// The minimum change time at `stop`, in seconds.
    pub fn min_change(&self, stop: StopId) -> Option<i32> {
        self.min_change.get(stop.index()).copied()
    }

    /// The trip at `row` of a lane, as it ran on its service date.
    pub fn trip_key(&self, lane: LaneRef, row: u32) -> Option<TripKey> {
        let arena = self.arena(lane.arena());
        let record = arena.lanes.get(lane.index() as usize)?;
        if row >= record.trip_count {
            return None;
        }
        let trip = *arena
            .trips
            .get(record.trips_start.checked_add(row)? as usize)?;
        let date = self.base.days.get(record.day.index())?.date;
        Some(TripKey { trip, date })
    }

    /// The route a lane belongs to.
    pub fn lane_route(&self, lane: LaneRef) -> Option<RouteId> {
        self.arena(lane.arena())
            .lanes
            .get(lane.index() as usize)
            .map(|l| l.route)
    }

    /// The service day of a lane.
    pub fn lane_day(&self, lane: LaneRef) -> Option<DaySlot> {
        self.arena(lane.arena())
            .lanes
            .get(lane.index() as usize)
            .map(|l| l.day)
    }

    /// The lanes of a route's block on a day, in scan order.
    ///
    /// An empty block has no lanes. `None` means the route or day is unknown.
    ///
    /// # Performance
    ///
    /// Allocates. This is for diagnostics and the response layer, not the route scan.
    pub fn block_lanes(&self, route: RouteId, day: DaySlot) -> Option<Vec<LaneRef>> {
        if route.index() >= self.route_count() || day.index() >= self.base.days.len() {
            return None;
        }
        let reference = self.block_ref(route.index(), day.index());
        let arena = self.arena(usize::from(reference.arena));
        let block = arena.blocks.get(reference.block as usize)?;
        Some(
            (0..block.lane_count)
                .map(|lane| LaneRef::new(reference.arena, block.lanes_start + lane))
                .collect(),
        )
    }

    /// How many trips a lane has.
    pub fn lane_trip_count(&self, lane: LaneRef) -> Option<u32> {
        self.lane(lane).map(|l| l.trip_count)
    }

    /// The stop events of the trip at `row` of a lane.
    ///
    /// # Performance
    ///
    /// Allocates. This is for diagnostics and the response layer, not the route scan.
    pub fn lane_trip_events(&self, lane: LaneRef, row: u32) -> Option<Vec<StopEvent>> {
        let record = self.lane(lane)?;
        if row >= record.trip_count {
            return None;
        }
        let arena = self.arena(lane.arena());
        let stride = record.stop_count as usize * 2;
        let grid = arena.lane_times(record);
        // Row zero of the grid is the leading sentinel.
        let times = grid.get((row as usize + 1) * stride..(row as usize + 2) * stride)?;
        let rules = arena.lane_rules(record);
        let mut events = Vec::with_capacity(record.stop_count as usize);
        for (pair, &rule) in times.chunks_exact(2).zip(rules) {
            let [arrival, departure] = *pair else {
                return None;
            };
            events.push(StopEvent {
                arrival: Time::try_new(arrival).ok()?,
                departure: Time::try_new(departure).ok()?,
                rules: StopRules::decode(rule),
            });
        }
        Some(events)
    }

    /// How much the overlay's arena holds; all zero for a schedule-only snapshot.
    pub fn overlay_usage(&self) -> ArenaUsage {
        self.overlay_arena.usage()
    }

    /// `[base, overlay]`: selecting an arena is an index, not a branch.
    #[inline]
    pub(crate) fn arenas(&self) -> [&Arena; 2] {
        [&self.base.arena, &self.overlay_arena]
    }

    #[inline]
    pub(crate) fn arena(&self, index: usize) -> &Arena {
        self.arenas()[index]
    }

    /// The directory entry of a route on a day.
    #[inline]
    pub(crate) fn block_ref(&self, route: usize, day: usize) -> BlockRef {
        self.directory[route * self.base.days.len() + day]
    }

    /// A lane by its reference.
    #[inline]
    pub(crate) fn lane(&self, lane: LaneRef) -> Option<&Lane> {
        self.arena(lane.arena()).lanes.get(lane.index() as usize)
    }
}
