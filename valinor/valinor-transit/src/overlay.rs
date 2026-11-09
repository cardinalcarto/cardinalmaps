//! Builds real-time overlays from service that has already been resolved.
//!
//! Decoding GTFS-RT is the job of `valinor-gtfs`. This builder takes the outcome: per-route,
//! per-day [`ResolvedBlock`]s (lanes already split for overtaking and skips),
//! removals of service, added routes, and added stops.
//!
//! Each replacement is laid out in the overlay's arena, in the same layout as the base window,
//! and the directory entry for that route and day is pointed at it.
//!
//! An overlay stop is numbered after every base stop,
//! so its [`StopId`] indexes the same flat arrays as a base stop's does.
//! Stop tables and transfer searches address it by that [`StopId`].
//! Snapping a new stop to the street network happens when the overlay is built,
//! in `valinor-walk`; this crate only needs the stop to exist.

use std::sync::Arc;

use crate::error::BuildError;
use crate::ids::{DaySlot, RouteId, Seconds, StopId};
use crate::snapshot::Snapshot;
use crate::timetable::{
    Arena, BaseWindow, BlockRef, ResolvedBlock, RouteAttributes, StopRouteIndex, offset_of,
};

/// Builds a [`Snapshot`] with real-time changes over a [`BaseWindow`].
#[derive(Debug)]
pub struct OverlayBuilder {
    base: Arc<BaseWindow>,
    arena: Arena,
    directory: Vec<BlockRef>,
    attributes: Vec<RouteAttributes>,
    min_change: Vec<i32>,
    /// `(route, start, length)` of each added route's stops in the overlay arena.
    added_routes: Vec<(usize, u32, u32)>,
    /// The start of each base route's stop sequence in the overlay arena, once copied there.
    base_route_stops: Vec<Option<u32>>,
}

impl OverlayBuilder {
    /// Starts an overlay on top of `base`, initially with no changes.
    pub fn new(base: Arc<BaseWindow>) -> Self {
        let base_route_stops = vec![None; base.route_count()];
        Self {
            arena: Arena::new(),
            directory: base.directory.clone(),
            attributes: base.attributes.clone(),
            min_change: base.min_change.clone(),
            added_routes: Vec::new(),
            base_route_stops,
            base,
        }
    }

    /// Adds a stop that the static timetable doesn't have, returning its number.
    ///
    /// The number follows every base stop, and is valid in this overlay's snapshot only.
    /// Stop tables must be built from [`Snapshot::stop_count`] to address it.
    ///
    /// # Errors
    ///
    /// If stops can no longer be numbered by a `u32`.
    pub fn add_stop(&mut self, min_change: Seconds) -> Result<StopId, BuildError> {
        let id = StopId::new(offset_of(self.min_change.len())?);
        self.min_change.push(min_change.as_i32());
        Ok(id)
    }

    /// Adds a route, which may visit base stops, overlay stops, or both.
    ///
    /// The route has no service until blocks are given for it with [`Self::replace_block`].
    ///
    /// # Errors
    ///
    /// If `stops` has fewer than two stops or contains an unknown stop.
    pub fn add_route(
        &mut self,
        stops: &[StopId],
        attributes: RouteAttributes,
    ) -> Result<RouteId, BuildError> {
        if stops.len() < 2 {
            return Err(BuildError::PatternTooShort(stops.len()));
        }
        for &stop in stops {
            if stop.index() >= self.min_change.len() {
                return Err(BuildError::UnknownStopId(stop));
            }
        }
        let route = self.attributes.len();
        let start = self.arena.push_stops(stops)?;
        self.added_routes
            .push((route, start, offset_of(stops.len())?));
        self.attributes.push(attributes);
        self.directory
            .extend(core::iter::repeat_n(BlockRef::EMPTY, self.base.days.len()));
        Ok(RouteId::new(offset_of(route)?))
    }

    /// Replaces a route's service on one day with a whole new block.
    ///
    /// Each changed route and service day gets a replacement; an unchanged one keeps its
    /// base block.
    ///
    /// # Errors
    ///
    /// If the route or day is unknown, or the block's trips don't match the route's stops.
    pub fn replace_block(
        &mut self,
        route: RouteId,
        day: DaySlot,
        block: &ResolvedBlock,
    ) -> Result<(), BuildError> {
        let slot = self.slot(route, day)?;
        let (stops_start, stop_count) = self.route_stops_in_arena(route)?;
        let index = self
            .arena
            .push_block(route, day, stops_start, stop_count as usize, block)?;
        self.directory[slot] = if index == 0 {
            BlockRef::EMPTY
        } else {
            BlockRef {
                arena: 1,
                block: index,
            }
        };
        Ok(())
    }

    /// Removes all of a route's service on one day.
    ///
    /// # Errors
    ///
    /// If the route or day is unknown.
    pub fn remove_service(&mut self, route: RouteId, day: DaySlot) -> Result<(), BuildError> {
        let slot = self.slot(route, day)?;
        self.directory[slot] = BlockRef::EMPTY;
        Ok(())
    }

    /// Finishes the overlay, producing a snapshot to publish.
    ///
    /// # Errors
    ///
    /// If an index would exceed its `u32` offsets.
    pub fn build(self) -> Result<Arc<Snapshot>, BuildError> {
        let arena = self.arena;
        let additions = StopRouteIndex::build(
            self.min_change.len(),
            self.added_routes.iter().map(|&(route, start, length)| {
                (
                    route,
                    &arena.stops[start as usize..(start + length) as usize],
                )
            }),
        )?;
        let added_route_stops = self
            .added_routes
            .iter()
            .map(|&(_, start, length)| (start, length))
            .collect();
        Ok(Arc::new(Snapshot {
            base: self.base,
            overlay_arena: arena,
            additions,
            directory: self.directory,
            attributes: self.attributes,
            min_change: self.min_change,
            added_route_stops,
        }))
    }

    /// The flat directory index of a route on a day, checked.
    fn slot(&self, route: RouteId, day: DaySlot) -> Result<usize, BuildError> {
        if route.index() >= self.attributes.len() {
            return Err(BuildError::UnknownRoute(route));
        }
        if day.index() >= self.base.days.len() {
            return Err(BuildError::UnknownDay(day));
        }
        Ok(route.index() * self.base.days.len() + day.index())
    }

    /// The stop sequence of a route in the overlay arena,
    /// copying a base route's sequence on first use.
    fn route_stops_in_arena(&mut self, route: RouteId) -> Result<(u32, u32), BuildError> {
        let base_routes = self.base.route_count();
        if let Some(added) = route.index().checked_sub(base_routes) {
            let &(_, start, length) = self
                .added_routes
                .get(added)
                .ok_or(BuildError::UnknownRoute(route))?;
            return Ok((start, length));
        }
        let stops = self
            .base
            .route_stops(route)
            .ok_or(BuildError::UnknownRoute(route))?;
        let count = offset_of(stops.len())?;
        let start = if let Some(start) = self.base_route_stops[route.index()] {
            start
        } else {
            let start = self.arena.push_stops(stops)?;
            self.base_route_stops[route.index()] = Some(start);
            start
        };
        Ok((start, count))
    }
}
