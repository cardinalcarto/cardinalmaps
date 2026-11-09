//! The block layout read by the route scan.
//!
//! Base and overlay service use this one layout and are scanned by the same code.
//!
//! A lane's times are one contiguous, trip-major grid of `i32`s:
//! `[arrival, departure]` per stop position, one row per trip,
//! with a sentinel row at each end (all `-INFINITY`, then all `+INFINITY`).
//! "No trip yet" is simply the sentinel row at the far end for the direction being searched,
//! so the scan reads a time unconditionally,
//! and an unreachable sentinel time never improves anything.
//! Scanning along a boarded trip is a sequential read of one row;
//! looking for an earlier trip reads the previous row.
//! Nothing in the scan looks at a trip's calendar or identity.

use crate::error::BuildError;
use crate::ids::{DaySlot, RouteId, StopId, TripId};
use crate::timetable::lanes::{ResolvedBlock, ResolvedLane};

/// A clock value beyond every real time: an unreachable label.
///
/// It is above [`crate::TIME_LIMIT`] plus [`crate::SECONDS_LIMIT`],
/// and adding a [`crate::SECONDS_LIMIT`] duration to it can't overflow `i32`.
pub(crate) const INFINITY: i32 = 1 << 30;

/// One lane of a route on one day, in an arena.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Lane {
    /// Start of the time grid, including the leading sentinel row.
    pub(crate) times_start: u32,
    /// Start of the stop sequence; shared by all lanes of the route.
    pub(crate) stops_start: u32,
    /// Start of the per-position rules.
    pub(crate) rules_start: u32,
    /// Start of the trip identifiers, one per real row.
    pub(crate) trips_start: u32,
    /// How many stop positions the route has.
    pub(crate) stop_count: u32,
    /// How many real trip rows there are (excluding the sentinels).
    pub(crate) trip_count: u32,
    /// The route this lane belongs to.
    pub(crate) route: RouteId,
    /// The service day this lane's trips run on.
    pub(crate) day: DaySlot,
}

/// One route on one day: a run of lanes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Block {
    pub(crate) lanes_start: u32,
    pub(crate) lane_count: u32,
    /// The earliest arrival anywhere in the block.
    pub(crate) earliest_arrival: i32,
    /// The latest departure anywhere in the block.
    pub(crate) latest_departure: i32,
}

impl Block {
    /// A block with no lanes, which every search prunes without touching any time.
    const EMPTY: Self = Self {
        lanes_start: 0,
        lane_count: 0,
        earliest_arrival: INFINITY,
        latest_departure: -INFINITY,
    };
}

/// Where a route's block for a day lives.
///
/// Stored flat, by `(route, day)`.
/// `arena` indexes the snapshot's `[base, overlay]` pair,
/// so choosing the arena costs an array index rather than a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockRef {
    pub(crate) arena: u8,
    pub(crate) block: u32,
}

impl BlockRef {
    /// The empty block that every arena keeps at index zero.
    pub(crate) const EMPTY: Self = Self { arena: 0, block: 0 };
}

/// A flat store of blocks: the base window has one, and an overlay has one.
#[derive(Debug)]
pub(crate) struct Arena {
    pub(crate) blocks: Vec<Block>,
    pub(crate) lanes: Vec<Lane>,
    pub(crate) times: Vec<i32>,
    pub(crate) stops: Vec<StopId>,
    pub(crate) rules: Vec<u8>,
    pub(crate) trips: Vec<TripId>,
}

impl Arena {
    /// An arena holding only the empty block.
    pub(crate) fn new() -> Self {
        Self {
            blocks: vec![Block::EMPTY],
            lanes: Vec::new(),
            times: Vec::new(),
            stops: Vec::new(),
            rules: Vec::new(),
            trips: Vec::new(),
        }
    }

    /// Appends the stop sequence of a route, returning its start.
    pub(crate) fn push_stops(&mut self, stops: &[StopId]) -> Result<u32, BuildError> {
        let start = offset_of(self.stops.len())?;
        self.stops.extend_from_slice(stops);
        Ok(start)
    }

    /// Lays out a block, returning its index.
    ///
    /// `stops_start` must be the route's sequence in this arena,
    /// and every lane's trips must have exactly `stop_count` events.
    pub(crate) fn push_block(
        &mut self,
        route: RouteId,
        day: DaySlot,
        stops_start: u32,
        stop_count: usize,
        block: &ResolvedBlock,
    ) -> Result<u32, BuildError> {
        if block.trip_count() == 0 {
            return Ok(0);
        }
        let lanes_start = offset_of(self.lanes.len())?;
        let mut earliest_arrival = INFINITY;
        let mut latest_departure = -INFINITY;
        for lane in block.lanes() {
            let (lane_earliest, lane_latest) =
                self.push_lane(route, day, stops_start, stop_count, lane)?;
            earliest_arrival = earliest_arrival.min(lane_earliest);
            latest_departure = latest_departure.max(lane_latest);
        }
        let index = offset_of(self.blocks.len())?;
        self.blocks.push(Block {
            lanes_start,
            lane_count: offset_of(block.lanes().len())?,
            earliest_arrival,
            latest_departure,
        });
        Ok(index)
    }

    fn push_lane(
        &mut self,
        route: RouteId,
        day: DaySlot,
        stops_start: u32,
        stop_count: usize,
        lane: &ResolvedLane,
    ) -> Result<(i32, i32), BuildError> {
        for trip in lane.trips() {
            if trip.events.len() != stop_count {
                return Err(BuildError::WrongStopCount {
                    trip: trip.trip,
                    expected: stop_count,
                    got: trip.events.len(),
                });
            }
        }

        // A lane's reference spends one bit on its arena; see `LaneRef`.
        if self.lanes.len() >= 1 << 31 {
            return Err(BuildError::TableTooLarge);
        }
        let times_start = offset_of(self.times.len())?;
        let rules_start = offset_of(self.rules.len())?;
        let trips_start = offset_of(self.trips.len())?;

        self.rules.extend(lane.rules_bytes());
        self.times
            .extend(core::iter::repeat_n(-INFINITY, stop_count * 2));
        let mut earliest_arrival = INFINITY;
        let mut latest_departure = -INFINITY;
        for trip in lane.trips() {
            self.trips.push(trip.trip);
            for event in &trip.events {
                let arrival = event.arrival.into_inner();
                let departure = event.departure.into_inner();
                earliest_arrival = earliest_arrival.min(arrival);
                latest_departure = latest_departure.max(departure);
                self.times.push(arrival);
                self.times.push(departure);
            }
        }
        self.times
            .extend(core::iter::repeat_n(INFINITY, stop_count * 2));

        self.lanes.push(Lane {
            times_start,
            stops_start,
            rules_start,
            trips_start,
            stop_count: offset_of(stop_count)?,
            trip_count: offset_of(lane.trips().len())?,
            route,
            day,
        });
        Ok((earliest_arrival, latest_departure))
    }

    /// The contiguous time grid of a lane, including both sentinel rows.
    #[inline]
    pub(crate) fn lane_times(&self, lane: &Lane) -> &[i32] {
        let rows = lane.trip_count as usize + 2;
        let len = rows * lane.stop_count as usize * 2;
        &self.times[lane.times_start as usize..][..len]
    }

    /// The stop sequence of a lane.
    #[inline]
    pub(crate) fn lane_stops(&self, lane: &Lane) -> &[StopId] {
        &self.stops[lane.stops_start as usize..][..lane.stop_count as usize]
    }

    /// The per-position rule flags of a lane.
    #[inline]
    pub(crate) fn lane_rules(&self, lane: &Lane) -> &[u8] {
        &self.rules[lane.rules_start as usize..][..lane.stop_count as usize]
    }
}

/// Converts a length to a `u32` table offset.
pub(crate) fn offset_of(len: usize) -> Result<u32, BuildError> {
    u32::try_from(len).map_err(|_| BuildError::TableTooLarge)
}
