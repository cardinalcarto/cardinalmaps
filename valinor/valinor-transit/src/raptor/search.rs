//! One MCR search: rounds of route scans and transfer searches.

use crate::error::SearchError;
use crate::ids::{Direction, LaneRef, StopId};
use crate::raptor::labels::Labels;
use crate::raptor::scan::scan_lane;
use crate::snapshot::Snapshot;
use crate::stop_table::{RouteFilter, StopTable};
use crate::timetable::{INFINITY, RouteOccurrence};
use crate::transfer::{
    Clock, TransferArrival, TransferOutcome, TransferRequest, TransferSearch, TransferSeed,
    TransferSink,
};

/// The most trips a search may use.
pub const MAX_TRIPS: u8 = 32;

/// How many route scans run between interrupt checks.
const INTERRUPT_SCAN_INTERVAL: u32 = 1024;

/// What one search reads.
pub(super) struct SearchInput<'a> {
    pub(super) snapshot: &'a Snapshot,
    pub(super) filter: RouteFilter,
    /// The table that seeds the search: access when forward, egress when backward.
    pub(super) seeding: &'a StopTable,
    /// The table leading to the search's target: egress when forward, access when backward.
    pub(super) target: &'a StopTable,
    pub(super) max_trips: u8,
    /// Per round, an exclusive bound on the target's clock, or an empty slice for none.
    ///
    /// Each entry must be above the clock the search would report for that many trips:
    /// see [`crate::profile`] for where such bounds come from.
    pub(super) bounds: &'a [i32],
}

/// Runs one search at clock `start`, leaving its labels for journey extraction.
pub(super) fn run<const BACK: bool, S: TransferSearch>(
    input: &SearchInput<'_>,
    labels: &mut Labels<S::Parent>,
    start: i32,
    transfers: &mut S,
    interrupt: &dyn Fn() -> bool,
) -> Result<(), SearchError<S::Error>> {
    let snapshot = input.snapshot;
    let rounds = usize::from(input.max_trips);
    labels.begin(snapshot.stop_count(), snapshot.route_count(), rounds);
    let direction = if BACK {
        Direction::Backward
    } else {
        Direction::Forward
    };
    transfers.begin_run(direction);

    // Round zero: the access table is already complete, so it needs no transfer search.
    for (stop, cost) in input.seeding.entries() {
        labels.seed_access(stop, start + cost);
    }

    let arenas = snapshot.arenas();
    let day_count = snapshot.days().len();
    let target_costs = input.target.costs();
    let mut scans = 0_u32;

    for round in 1..=input.max_trips {
        if labels.marked.is_empty() {
            break;
        }
        if interrupt() {
            return Err(SearchError::Interrupted);
        }
        if let Some(&bound) = input.bounds.get(usize::from(round)) {
            labels.best_target = labels.best_target.min(bound);
        }
        collect_routes::<BACK, _>(snapshot, input.filter, labels);

        // Sorted route order is arena order, which the prefetcher can follow.
        let mut routes = core::mem::take(&mut labels.touched_routes);
        routes.sort_unstable();
        for &route in &routes {
            let route = route as usize;
            let start_position = labels.route_marks[route].1 as usize;
            for day in 0..day_count {
                let block_ref = snapshot.block_ref(route, day);
                let arena = arenas[usize::from(block_ref.arena)];
                let block = arena.blocks[block_ref.block as usize];
                // A whole day that can't be boarded or can't improve the target is skipped
                // on two integers, without touching its trips.
                let (board_bound, alight_bound) = if BACK {
                    (-block.earliest_arrival, -block.latest_departure)
                } else {
                    (block.latest_departure, block.earliest_arrival)
                };
                if start > board_bound || alight_bound >= labels.best_target {
                    continue;
                }
                for lane_index in block.lanes_start..block.lanes_start + block.lane_count {
                    let lane = &arena.lanes[lane_index as usize];
                    let lane_ref = LaneRef::new(block_ref.arena, lane_index);
                    scan_lane::<BACK, _>(
                        labels,
                        arena,
                        lane,
                        lane_ref,
                        start_position,
                        round,
                        target_costs,
                    );
                }
            }
            scans += 1;
            if scans.is_multiple_of(INTERRUPT_SCAN_INTERVAL) && interrupt() {
                labels.touched_routes = routes;
                return Err(SearchError::Interrupted);
            }
        }
        routes.clear();
        labels.touched_routes = routes;

        labels.commit_trips(round, &snapshot.min_change);
        // A walk after round `r` only serves boardings of round `r + 1`, so the final round
        // walks nowhere useful, and an earlier one is bounded as the next round is.
        if round == input.max_trips {
            labels.improved.clear();
            break;
        }
        let walk_bound = input
            .bounds
            .get(usize::from(round) + 1)
            .copied()
            .unwrap_or(INFINITY);
        let outcome = run_transfers(labels, round, direction, walk_bound, transfers, interrupt)
            .map_err(SearchError::Transfer)?;
        if outcome == TransferOutcome::Interrupted {
            return Err(SearchError::Interrupted);
        }
    }
    Ok(())
}

/// Turns the stops marked last round into the routes to scan and where to start on each.
fn collect_routes<const BACK: bool, P: Copy>(
    snapshot: &Snapshot,
    filter: RouteFilter,
    labels: &mut Labels<P>,
) {
    let epoch = labels.next_route_epoch();
    let marked = core::mem::take(&mut labels.marked);
    let mark_route = |labels: &mut Labels<P>, occurrence: RouteOccurrence| {
        let route = occurrence.route as usize;
        if !filter.allows(snapshot.attributes[route]) {
            return;
        }
        let mark = &mut labels.route_marks[route];
        if mark.0 == epoch {
            mark.1 = if BACK {
                mark.1.max(occurrence.position)
            } else {
                mark.1.min(occurrence.position)
            };
        } else {
            *mark = (epoch, occurrence.position);
            labels.touched_routes.push(occurrence.route);
        }
    };
    for &stop in &marked {
        let stop = stop as usize;
        for &occurrence in snapshot.base.index.occurrences(stop) {
            mark_route(labels, occurrence);
        }
        for &occurrence in snapshot.additions.occurrences(stop) {
            mark_route(labels, occurrence);
        }
    }
    let mut marked = marked;
    marked.clear();
    labels.marked = marked;
}

/// Writes reports from a transfer search straight into the labels.
struct LabelSink<'l, P> {
    labels: &'l mut Labels<P>,
    round: u8,
}

impl<P: Copy> TransferSink<P> for LabelSink<'_, P> {
    #[inline]
    fn reach(&mut self, arrival: TransferArrival<P>) {
        let stop = arrival.stop.index();
        // A report outside the snapshot is a bug in the search; it can't reach this stop.
        debug_assert!(
            stop < self.labels.stop_count,
            "transfer search reported a stop out of range"
        );
        if stop < self.labels.stop_count {
            self.labels.improve_foot(
                self.round,
                stop,
                arrival.clock.seconds(),
                arrival.source,
                arrival.parent,
            );
        }
    }
}

/// Runs the same-round multi-source walk from every stop this round's scan improved.
fn run_transfers<S: TransferSearch>(
    labels: &mut Labels<S::Parent>,
    round: u8,
    direction: Direction,
    walk_bound: i32,
    transfers: &mut S,
    interrupt: &dyn Fn() -> bool,
) -> Result<TransferOutcome, S::Error> {
    let mut seeds = core::mem::take(&mut labels.seeds);
    seeds.clear();
    let improved = core::mem::take(&mut labels.improved);
    for &stop in &improved {
        seeds.push(TransferSeed {
            stop: StopId::new(stop),
            clock: Clock(labels.read(stop as usize).best_trip),
        });
    }
    let mut improved = improved;
    improved.clear();
    labels.improved = improved;

    if seeds.is_empty() {
        labels.seeds = seeds;
        return Ok(TransferOutcome::Completed);
    }
    let request = TransferRequest {
        direction,
        seeds: &seeds,
        upper_bound: Clock(labels.best_target.min(walk_bound)),
        interrupt,
    };
    let mut sink = LabelSink { labels, round };
    let outcome = transfers.search(&request, &mut sink);
    labels.seeds = seeds;
    outcome
}
