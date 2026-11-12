//! The route scan: the loop RAPTOR's speed depends on.
//!
//! Nothing here depends on a trip's calendar, on an overlay, or on identity.
//! Per stop position it reads one flag byte, two labels, and at most a few integers of one row.
//!
//! A lane scanned forward reads rows by increasing index.
//! A backward scan reads the same grid mirrored:
//! virtual row `v` is real row `trips + 1 - v`, boarding reads arrivals and alighting departures,
//! both negated. Both directions then look for the first trip whose board clock is
//! `>=` the label, and a board clock grows with `v` in both.

use crate::raptor::labels::Labels;
use crate::timetable::{Arena, INFINITY, Lane};

/// The grid of one lane, read through the search's direction.
struct Grid<'a, const BACK: bool> {
    times: &'a [i32],
    stride: usize,
    trips: usize,
}

impl<const BACK: bool> Grid<'_, BACK> {
    /// The real row of a virtual row.
    #[inline]
    fn real_row(&self, v: usize) -> usize {
        if BACK { self.trips + 1 - v } else { v }
    }

    #[inline]
    fn row(&self, v: usize) -> &[i32] {
        let row = self.real_row(v);
        &self.times[row * self.stride..][..self.stride]
    }

    /// The clock at which a trip can be boarded at a position, in a row.
    #[inline]
    fn board_clock(row: &[i32], position: usize) -> i32 {
        if BACK {
            -row[2 * position]
        } else {
            row[2 * position + 1]
        }
    }

    /// The clock at which a trip reaches a position, in a row.
    #[inline]
    fn alight_clock(row: &[i32], position: usize) -> i32 {
        if BACK {
            -row[2 * position + 1]
        } else {
            row[2 * position]
        }
    }

    /// The lowest virtual row at or before `upper` that can be boarded with `label` at `position`.
    ///
    /// `upper` is known to be boardable (the sentinel row always is).
    /// The previous row is tried first: usually no earlier trip works,
    /// and that row is the adjacent cache line.
    #[inline]
    fn first_boardable(&self, position: usize, label: i32, upper: usize) -> usize {
        if upper == 1 {
            return upper;
        }
        let previous = Self::board_clock(self.row(upper - 1), position);
        if previous < label {
            return upper;
        }
        let mut low = 1;
        let mut high = upper - 1;
        while low < high {
            let mid = low + (high - low) / 2;
            if Self::board_clock(self.row(mid), position) >= label {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        high
    }
}

/// Scans one lane from a stop position, improving labels for round `round`.
///
/// `start` is the first position of the lane this round needs to look at.
#[inline]
#[expect(
    clippy::cast_possible_truncation,
    reason = "rows and positions fit the u32 lane fields, checked when the lane was built"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "the hot path, inlined into its one caller"
)]
pub(super) fn scan_lane<const BACK: bool, P: Copy>(
    labels: &mut Labels<P>,
    arena: &Arena,
    lane: &Lane,
    lane_ref: crate::ids::LaneRef,
    start: usize,
    round: u8,
    target_costs: &[i32],
    (can_alight, can_board): (u8, u8),
) {
    let stop_count = lane.stop_count as usize;
    let stops = arena.lane_stops(lane);
    let rules = arena.lane_rules(lane);
    let grid = Grid::<'_, BACK> {
        times: arena.lane_times(lane),
        stride: stop_count * 2,
        trips: lane.trip_count as usize,
    };

    let no_trip = grid.trips + 1;
    let mut current = no_trip;
    let mut row = grid.row(current);
    let mut board_position = 0_u32;
    let mut origin = (0_u8, crate::raptor::labels::LabelKind::Foot);

    let first = if BACK { stop_count - 1 - start } else { start };
    for step in first..stop_count {
        let position = if BACK { stop_count - 1 - step } else { step };
        let stop = stops[position].index();
        // A byte holds the stop's rules twice; `can_alight` and `can_board` pick which copy
        // to read, so a wheelchair exclusion costs nothing here beyond this same bit test.
        let flags = rules[position];
        let read = labels.read(stop);

        // Alight from the current trip first: boarding here is for later stops.
        // The sentinel row reads as unreachable, so "no trip yet" needs no branch.
        let alight = Grid::<BACK>::alight_clock(row, position);
        if (flags & can_alight) != 0 && alight < read.best_trip && alight < labels.best_target {
            let target_cost = target_costs[stop];
            labels.improve_trip(
                round,
                stop,
                alight,
                lane_ref,
                // In range: positions and rows fit the `u32` lane fields.
                grid.real_row(current) as u32,
                board_position,
                position as u32,
                origin,
                target_cost,
            );
        }

        // Then see whether this round's label reaches an earlier trip.
        let label = read.board;
        if (flags & can_board) != 0 && label < INFINITY {
            let board = Grid::<BACK>::board_clock(row, position);
            if label <= board {
                let earliest = grid.first_boardable(position, label, current);
                if earliest < current {
                    current = earliest;
                    row = grid.row(current);
                    board_position = position as u32;
                    origin = labels.board_origin(stop);
                }
            }
        }
    }
}
