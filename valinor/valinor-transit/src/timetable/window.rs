//! A materialized window of service days, immutable and shared by every query.

use crate::error::BuildError;
#[cfg(test)]
use crate::ids::DaySlot;
use crate::ids::{RouteId, ServiceDay, StopId};
use crate::timetable::arena::{Arena, BlockRef, offset_of};
use crate::timetable::pairs::TransferPairs;

/// Boolean attributes of a route.
///
/// The set is held as bits so a [`crate::RouteFilter`] is a mask test on the route,
/// not a per-trip check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RouteAttributes(u8);

impl RouteAttributes {
    /// No attributes.
    pub const NONE: Self = Self(0);
    /// Bicycles may be taken on board.
    pub const BIKES_ALLOWED: Self = Self(1);

    /// Whether every attribute in `other` is present.
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOr for RouteAttributes {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// One occurrence of a route at a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RouteOccurrence {
    pub(crate) route: u32,
    pub(crate) position: u32,
}

/// For each stop, the routes serving it at each of their positions there.
///
/// A loop route has one entry per time it visits a stop.
#[derive(Debug, Clone, Default)]
pub struct StopRouteIndex {
    /// `offsets[s]..offsets[s + 1]` is stop `s`'s run of `entries`. Empty means no entries.
    offsets: Vec<u32>,
    entries: Vec<RouteOccurrence>,
}

impl StopRouteIndex {
    /// Builds the index for routes given as `(route id, stops)`.
    pub(crate) fn build<'a>(
        stop_count: usize,
        routes: impl Iterator<Item = (usize, &'a [StopId])> + Clone,
    ) -> Result<Self, BuildError> {
        let mut counts = vec![0_u32; stop_count + 1];
        for (_route, stops) in routes.clone() {
            for stop in stops {
                let count = &mut counts[stop.index() + 1];
                *count = count.checked_add(1).ok_or(BuildError::TableTooLarge)?;
            }
        }
        for s in 1..counts.len() {
            counts[s] = counts[s]
                .checked_add(counts[s - 1])
                .ok_or(BuildError::TableTooLarge)?;
        }
        let offsets = counts.clone();
        let total = counts.last().copied().unwrap_or(0) as usize;
        let mut entries = vec![
            RouteOccurrence {
                route: 0,
                position: 0
            };
            total
        ];
        for (route, stops) in routes {
            let route = offset_of(route)?;
            for (position, stop) in stops.iter().enumerate() {
                let slot = &mut counts[stop.index()];
                entries[*slot as usize] = RouteOccurrence {
                    route,
                    position: offset_of(position)?,
                };
                *slot += 1;
            }
        }
        Ok(Self { offsets, entries })
    }

    /// The occurrences at a stop; empty when the index has no entries for it.
    #[inline]
    pub(crate) fn occurrences(&self, stop: usize) -> &[RouteOccurrence] {
        match (self.offsets.get(stop), self.offsets.get(stop + 1)) {
            (Some(&start), Some(&end)) => &self.entries[start as usize..end as usize],
            _ => &[],
        }
    }
}

/// The base window: the active trips of some service days, laid out for scanning.
#[derive(Debug)]
pub struct BaseWindow {
    pub(crate) arena: Arena,
    /// Flat by `(route, day)`.
    pub(crate) directory: Vec<BlockRef>,
    pub(crate) attributes: Vec<RouteAttributes>,
    /// Each route's `(start, length)` in the arena's stop sequences.
    pub(crate) route_stops: Vec<(u32, u32)>,
    /// The per-stop minimum change times, in seconds.
    pub(crate) min_change: Vec<i32>,
    /// The rules for walking between particular stops.
    pub(crate) pairs: TransferPairs,
    pub(crate) index: StopRouteIndex,
    pub(crate) days: Vec<ServiceDay>,
}

impl BaseWindow {
    #[expect(
        clippy::too_many_arguments,
        reason = "one call site, each a distinct table"
    )]
    pub(crate) fn new(
        arena: Arena,
        directory: Vec<BlockRef>,
        attributes: Vec<RouteAttributes>,
        route_stops: Vec<(u32, u32)>,
        min_change: Vec<i32>,
        pairs: TransferPairs,
        index: StopRouteIndex,
        days: Vec<ServiceDay>,
    ) -> Self {
        Self {
            arena,
            directory,
            attributes,
            route_stops,
            min_change,
            pairs,
            index,
            days,
        }
    }

    /// The window's service days, in slot order.
    pub fn days(&self) -> &[ServiceDay] {
        &self.days
    }

    /// The stop sequence of a route.
    pub fn route_stops(&self, route: RouteId) -> Option<&[StopId]> {
        let &(start, length) = self.route_stops.get(route.index())?;
        self.arena
            .stops
            .get(start as usize..(start + length) as usize)
    }

    /// The attributes of a route.
    pub fn route_attributes(&self, route: RouteId) -> Option<RouteAttributes> {
        self.attributes.get(route.index()).copied()
    }

    /// How many stops the window numbers.
    pub fn stop_count(&self) -> usize {
        self.min_change.len()
    }

    /// How many routes the window numbers.
    pub fn route_count(&self) -> usize {
        self.attributes.len()
    }

    /// The directory slot of a route on a day.
    #[cfg(test)]
    pub(crate) fn directory_index(&self, route: RouteId, day: DaySlot) -> usize {
        route.index() * self.days.len() + day.index()
    }

    /// The trips of a route on a day as `(trip, [arrival, departure, ...])`, for tests.
    #[cfg(test)]
    pub(crate) fn lane_trips_for_test(&self, route: RouteId, day: DaySlot) -> Vec<(u32, Vec<i32>)> {
        let block_ref = self.directory[self.directory_index(route, day)];
        let block = self.arena.blocks[block_ref.block as usize];
        let mut out = Vec::new();
        for lane in &self.arena.lanes[block.lanes_start as usize..][..block.lane_count as usize] {
            let stride = lane.stop_count as usize * 2;
            let grid = self.arena.lane_times(lane);
            let trips = &self.arena.trips[lane.trips_start as usize..][..lane.trip_count as usize];
            for (i, trip) in trips.iter().enumerate() {
                out.push((
                    trip.into_inner(),
                    grid[(i + 1) * stride..(i + 2) * stride].to_vec(),
                ));
            }
        }
        out
    }
}
