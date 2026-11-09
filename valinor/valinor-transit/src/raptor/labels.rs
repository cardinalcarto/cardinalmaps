//! Flat label arrays, cleared by version stamps.
//!
//! Every stop has two kinds of label, because arriving on foot and arriving by trip
//! give different boarding rights: after a trip, boarding at the same stop
//! must wait for the stop's minimum change time.
//!
//! The arrays the route scan reads per stop position (`hot`) are small and separate
//! from the per-round cells, which are written only when a label improves.

use crate::ids::{LaneRef, StopId};
use crate::timetable::INFINITY;

/// Which kind of label a boarding used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelKind {
    Trip,
    Foot,
}

/// What the route scan reads and writes at each stop position, one cache-friendly record per stop.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub(super) struct StopHot {
    stamp: u32,
    /// The earliest arrival by trip, over all rounds so far.
    best_trip: i32,
    /// The earliest clock at which a trip may be boarded here (committed between rounds).
    board: i32,
    origin_round: u8,
    origin_kind: u8,
    /// `round + 1` when this stop was last added to the improved list.
    listed: u8,
    /// `round + 1` when this stop was last marked for the next round.
    marked: u8,
}

const KIND_TRIP: u8 = 0;
const KIND_FOOT: u8 = 1;

impl StopHot {
    const BLANK: Self = Self {
        stamp: 0,
        best_trip: INFINITY,
        board: INFINITY,
        origin_round: 0,
        origin_kind: KIND_FOOT,
        listed: 0,
        marked: 0,
    };
}

/// How a stop was reached by trip in some round.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TripCell {
    pub(crate) time: i32,
    stamp: u32,
    pub(crate) lane: LaneRef,
    /// The trip's row in the lane's grid, counting the leading sentinel.
    pub(crate) row: u32,
    /// Where the rider boarded.
    pub(crate) board_pos: u32,
    /// Where the rider alighted.
    pub(crate) alight_pos: u32,
    pub(crate) origin_round: u8,
    pub(crate) origin_kind: LabelKind,
}

/// How a stop was reached on foot in some round.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FootCell<P> {
    pub(crate) time: i32,
    stamp: u32,
    pub(crate) source: StopId,
    /// `None` for the access entries of round zero.
    pub(crate) parent: Option<P>,
}

/// The best arrival at the target with at most some number of trips.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TargetCell {
    pub(crate) time: i32,
    pub(crate) stop: StopId,
    pub(crate) improved: bool,
}

/// What the last round's scan needs from a stop.
#[derive(Debug, Clone, Copy)]
pub(super) struct StopRead {
    pub(super) best_trip: i32,
    pub(super) board: i32,
}

/// One search's labels. They are reused between searches.
#[derive(Debug)]
pub(crate) struct Labels<P> {
    version: u32,
    pub(super) stop_count: usize,
    pub(super) rounds: usize,
    hot: Vec<StopHot>,
    trips: Vec<TripCell>,
    foot: Vec<FootCell<P>>,
    targets: Vec<TargetCell>,
    pub(super) best_target: i32,
    /// Stops marked for the next round's route collection.
    pub(super) marked: Vec<u32>,
    /// Stops whose trip label improved this round.
    pub(super) improved: Vec<u32>,
    /// Per route: the epoch it was last marked in and the first position to scan.
    pub(super) route_marks: Vec<(u32, u32)>,
    /// Bumped once per round, so a route is queued once per round and no more.
    route_epoch: u32,
    pub(super) touched_routes: Vec<u32>,
    pub(super) seeds: Vec<crate::transfer::TransferSeed>,
}

impl<P: Copy> Labels<P> {
    pub(crate) fn new() -> Self {
        Self {
            version: 0,
            stop_count: 0,
            rounds: 0,
            hot: Vec::new(),
            trips: Vec::new(),
            foot: Vec::new(),
            targets: Vec::new(),
            best_target: INFINITY,
            marked: Vec::new(),
            improved: Vec::new(),
            route_marks: Vec::new(),
            route_epoch: 0,
            touched_routes: Vec::new(),
            seeds: Vec::new(),
        }
    }

    /// Prepares for a search over `stop_count` stops and `rounds` rounds of trips.
    ///
    /// Sized from the pinned snapshot: overlay stops share these arrays with base stops.
    pub(super) fn begin(&mut self, stop_count: usize, route_count: usize, rounds: usize) {
        self.stop_count = stop_count;
        self.rounds = rounds;
        let cells = stop_count * (rounds + 1);
        self.hot.resize(stop_count, StopHot::BLANK);
        self.trips.resize(
            cells,
            TripCell {
                time: INFINITY,
                stamp: 0,
                lane: LaneRef::new(0, 0),
                row: 0,
                board_pos: 0,
                alight_pos: 0,
                origin_round: 0,
                origin_kind: LabelKind::Foot,
            },
        );
        self.foot.resize(
            cells,
            FootCell {
                time: INFINITY,
                stamp: 0,
                source: StopId::new(0),
                parent: None,
            },
        );
        self.route_marks.resize(route_count, (0, 0));
        self.version = self.version.wrapping_add(1);
        if self.version == 0 {
            // Stamps from 2^32 searches ago would look current, so clear them for real.
            self.hot.fill(StopHot::BLANK);
            for cell in &mut self.trips {
                cell.stamp = 0;
            }
            for cell in &mut self.foot {
                cell.stamp = 0;
            }
            self.version = 1;
        }
        self.targets.clear();
        self.targets.resize(
            rounds + 1,
            TargetCell {
                time: INFINITY,
                stop: StopId::new(0),
                improved: false,
            },
        );
        self.best_target = INFINITY;
        self.marked.clear();
        self.improved.clear();
        self.touched_routes.clear();
    }

    /// Starts a new round's route collection, returning its epoch.
    pub(super) fn next_route_epoch(&mut self) -> u32 {
        self.route_epoch = self.route_epoch.wrapping_add(1);
        if self.route_epoch == 0 {
            // A mark 2^32 rounds old would look current, so clear them for real.
            self.route_marks.fill((0, 0));
            self.route_epoch = 1;
        }
        self.route_epoch
    }

    /// Reads a stop's labels. A stale stamp reads as unreached: a conditional move, not a branch.
    #[inline]
    pub(super) fn read(&self, stop: usize) -> StopRead {
        let hot = &self.hot[stop];
        let fresh = hot.stamp == self.version;
        StopRead {
            best_trip: if fresh { hot.best_trip } else { INFINITY },
            board: if fresh { hot.board } else { INFINITY },
        }
    }

    /// The label a boarding at this stop came from, as `(round, kind)`.
    #[inline]
    pub(super) fn board_origin(&self, stop: usize) -> (u8, LabelKind) {
        let hot = &self.hot[stop];
        let kind = if hot.origin_kind == KIND_TRIP {
            LabelKind::Trip
        } else {
            LabelKind::Foot
        };
        (hot.origin_round, kind)
    }

    #[inline]
    fn touch(&mut self, stop: usize) -> &mut StopHot {
        let version = self.version;
        let hot = &mut self.hot[stop];
        if hot.stamp != version {
            *hot = StopHot {
                stamp: version,
                ..StopHot::BLANK
            };
        }
        hot
    }

    #[inline]
    fn cell_index(&self, round: usize, stop: usize) -> usize {
        round * self.stop_count + stop
    }

    /// Records that a route boarded at `board_pos` reaches this stop by trip, if that improves it.
    ///
    /// `arrival` must already be below the stop's best and the target bound.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "stops are numbered by u32 when a timetable is built"
    )]
    pub(super) fn improve_trip(
        &mut self,
        round: u8,
        stop: usize,
        arrival: i32,
        lane: LaneRef,
        row: u32,
        board_pos: u32,
        alight_pos: u32,
        origin: (u8, LabelKind),
        target_cost: i32,
    ) {
        let mark = round + 1;
        let hot = self.touch(stop);
        hot.best_trip = arrival;
        let first_this_round = hot.listed != mark;
        hot.listed = mark;
        if first_this_round {
            self.improved.push(stop as u32);
        }
        let index = self.cell_index(usize::from(round), stop);
        let version = self.version;
        self.trips[index] = TripCell {
            time: arrival,
            stamp: version,
            lane,
            row,
            board_pos,
            alight_pos,
            origin_round: origin.0,
            origin_kind: origin.1,
        };
        let total = arrival + target_cost;
        if total < self.best_target {
            self.best_target = total;
            self.targets[usize::from(round)] = TargetCell {
                time: total,
                stop: StopId::new(stop as u32),
                improved: true,
            };
        }
    }

    /// Records a stop reachable on foot at the start of a search.
    pub(super) fn seed_access(&mut self, stop: StopId, clock: i32) {
        let index = stop.index();
        self.set_foot(0, index, clock, stop, None);
    }

    /// Records a walk to a stop that improves when it can be boarded.
    #[inline]
    pub(super) fn improve_foot(
        &mut self,
        round: u8,
        stop: usize,
        clock: i32,
        source: StopId,
        parent: P,
    ) {
        if clock < self.read(stop).board && clock < self.best_target {
            self.set_foot(round, stop, clock, source, Some(parent));
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "stops are numbered by u32 when a timetable is built"
    )]
    fn set_foot(&mut self, round: u8, stop: usize, clock: i32, source: StopId, parent: Option<P>) {
        let mark = round + 1;
        let hot = self.touch(stop);
        if clock >= hot.board {
            return;
        }
        hot.board = clock;
        hot.origin_round = round;
        hot.origin_kind = KIND_FOOT;
        let newly_marked = hot.marked != mark;
        hot.marked = mark;
        if newly_marked {
            self.marked.push(stop as u32);
        }
        let index = self.cell_index(usize::from(round), stop);
        let version = self.version;
        self.foot[index] = FootCell {
            time: clock,
            stamp: version,
            source,
            parent,
        };
    }

    /// Lets this round's trip arrivals be boarded in the next round, and marks those that help.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "stops are numbered by u32 when a timetable is built"
    )]
    pub(super) fn commit_trips(&mut self, round: u8, min_change: &[i32]) {
        let mark = round + 1;
        for i in 0..self.improved.len() {
            let stop = self.improved[i] as usize;
            let change = min_change[stop];
            let hot = &mut self.hot[stop];
            let boardable = hot.best_trip + change;
            if boardable < hot.board {
                hot.board = boardable;
                hot.origin_round = round;
                hot.origin_kind = KIND_TRIP;
                if hot.marked != mark {
                    hot.marked = mark;
                    self.marked.push(stop as u32);
                }
            }
        }
    }

    /// A round's trip label at a stop, if it is current.
    pub(crate) fn trip_cell(&self, round: u8, stop: usize) -> Option<TripCell> {
        let cell = self.trips.get(self.cell_index(usize::from(round), stop))?;
        (cell.stamp == self.version).then_some(*cell)
    }

    /// A round's foot label at a stop, if it is current.
    pub(crate) fn foot_cell(&self, round: u8, stop: usize) -> Option<FootCell<P>> {
        let cell = self.foot.get(self.cell_index(usize::from(round), stop))?;
        (cell.stamp == self.version).then_some(*cell)
    }

    /// The target label of a round.
    pub(crate) fn target(&self, round: u8) -> Option<TargetCell> {
        self.targets.get(usize::from(round)).copied()
    }

    /// The rounds of the last search, each with the target's best label.
    pub(crate) fn targets(&self) -> &[TargetCell] {
        &self.targets
    }

    /// The trip clock at a stop in a round (used to find a walk's length).
    pub(crate) fn trip_time(&self, round: u8, stop: usize) -> Option<i32> {
        self.trip_cell(round, stop).map(|cell| cell.time)
    }
}
