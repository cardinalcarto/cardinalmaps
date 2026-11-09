//! The transfer search trait: how RAPTOR sees the street network.
//!
//! A round's transfers are one multi-source search seeded by the stops the round's route scan
//! improved. This crate is generic over the search, so its tests run on synthetic graphs,
//! and `valinor-walk` implements it over tiles.
//!
//! An implementation keeps its working memory in `self` and reuses it between searches,
//! so no search has to allocate.
//! Everything else comes through borrowed slices and a sink.

use crate::ids::{Direction, Seconds, StopId};

/// A search's clock, in seconds: it grows as the search moves away from its seeds.
///
/// For a forward search it is real time. For a backward search it is real time negated.
/// A transfer search doesn't need to know which: `clock + edge cost` is correct in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Clock(pub(crate) i32);

impl Clock {
    /// A clock at `seconds`.
    ///
    /// For a [`TransferSearch`] implementation to report arrivals it computed itself,
    /// and for callers building a request by hand; `valinor-transit` otherwise makes clocks
    /// from query times.
    #[inline]
    pub const fn new(seconds: i32) -> Self {
        Self(seconds)
    }

    /// This clock advanced by `cost`.
    #[inline]
    #[must_use]
    pub fn advanced_by(self, cost: Seconds) -> Self {
        Self(self.0 + cost.as_i32())
    }

    /// The raw number of seconds.
    #[inline]
    pub const fn seconds(self) -> i32 {
        self.0
    }
}

/// A stop the search starts from, with the clock when the rider is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferSeed {
    /// The stop.
    pub stop: StopId,
    /// The clock at this stop.
    pub clock: Clock,
}

/// A stop the search reached on foot.
#[derive(Debug, Clone, Copy)]
pub struct TransferArrival<P> {
    /// The stop reached.
    pub stop: StopId,
    /// The seed this walk began from; it is never [`Self::stop`].
    pub source: StopId,
    /// The clock on arrival.
    pub clock: Clock,
    /// The implementation's handle for reconstructing this walk.
    pub parent: P,
}

/// Receives the stops a search reaches.
///
/// The router implements this; a transfer search only calls it.
pub trait TransferSink<P> {
    /// Reports a walk to a stop.
    ///
    /// This may be called several times for one stop, in any order; the smallest clock wins.
    fn reach(&mut self, arrival: TransferArrival<P>);
}

/// One multi-source search.
pub struct TransferRequest<'a> {
    /// Which way to follow edges.
    pub direction: Direction,
    /// The stops to start from, each at most once.
    pub seeds: &'a [TransferSeed],
    /// Arrivals with `clock >= upper_bound` are useless to the router.
    ///
    /// The search should stop expanding beyond this; it is the target pruning bound.
    /// A router must not raise it between the searches of one run (one [`TransferSearch::begin_run`]):
    /// an implementation may carry labels across a run, and labels cut off under a lower bound
    /// were never expanded past it.
    pub upper_bound: Clock,
    /// The largest minimum change time of any stop in the snapshot, in seconds.
    ///
    /// A walk reaching a seed's own stop `v` at or after `v`'s clock plus this margin
    /// can't improve `v`, because boarding there after the trip already costs no more,
    /// so the search may leave such arrivals unreported.
    /// [`Self::REPORT_ALL`] asks for every arrival of the contract on [`TransferSearch`].
    pub change_margin: i32,
    /// Whether the caller wants this search to stop.
    /// Check it periodically and return [`TransferOutcome::Interrupted`].
    pub interrupt: &'a dyn Fn() -> bool,
}

impl TransferRequest<'_> {
    /// A [`Self::change_margin`] that makes the search report every arrival.
    pub const REPORT_ALL: i32 = i32::MAX;
}

/// How a transfer search ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferOutcome {
    /// The search finished.
    Completed,
    /// The search stopped because `interrupt` asked it to.
    Interrupted,
}

/// A multi-source shortest path search on the street network.
///
/// # Contract
///
/// Costs are integer seconds on directed edges. A forward search follows each edge as directed.
/// A backward search moves against each edge and charges that same directed edge's cost.
/// All costs are non-negative.
///
/// For every stop `v` and every seed `u ≠ v` whose `u.clock + distance(u → v)` is below
/// `upper_bound`, the search reports the smallest such value over all seeds `u ≠ v`,
/// with `source` being a seed that achieves it.
/// If `v` is itself a seed, the report may be left out when that value is at least
/// `v.clock + change_margin`, since such a walk can't improve boarding at `v`.
///
/// Within one run (one [`Self::begin_run`]), a search may report only what improves on
/// the earlier rounds of that run: a walk that can't beat what an earlier round already reported,
/// by that round's arrivals, may be left out. A router keeps the running minimum per stop,
/// as `valinor-transit`'s does.
///
/// `u ≠ v` is the part that isn't an ordinary multi-source search,
/// and it is what stops a zero-length walk from bypassing a minimum change time.
/// A seed's own stop is not reached from itself,
/// but a walk from a *different* seed to that stop is reported even when it arrives later.
/// A single best label per node can't provide that, as the best label at a node may have come
/// from the very stop being asked about. Keeping the two best labels with distinct sources at each
/// node is enough: excluding any one source still leaves the best of the rest.
///
/// # Lifetimes of parent handles
///
/// [`Self::Parent`] is the implementation's own handle for reconstructing a walk.
/// Each handle returned while an MCR search is running stays valid until the next
/// [`Self::begin_run`] call, so a search may keep them in an arena it clears at that point.
/// Journeys hold no handles: extraction resolves each one with [`Self::walk`] right away,
/// so a journey outlives the searches of its profile.
pub trait TransferSearch {
    /// A handle for reconstructing a walk.
    type Parent: Copy;
    /// A walk resolved from its handle, which owns everything needed to expand it later.
    type Walk: Clone;
    /// A failure of the search itself (for example, a tile that can't be read).
    type Error: core::error::Error + 'static;

    /// Called once before each MCR search; handles from earlier searches may now be reused.
    fn begin_run(&mut self, direction: Direction);

    /// Resolves a handle that is currently valid into an owned walk.
    fn walk(&self, parent: Self::Parent) -> Self::Walk;

    /// Runs one multi-source search, reporting arrivals to `sink`.
    ///
    /// # Errors
    ///
    /// Only for failures of the search itself. An interrupt is not an error;
    /// return [`TransferOutcome::Interrupted`] for it.
    fn search<K: TransferSink<Self::Parent>>(
        &mut self,
        request: &TransferRequest<'_>,
        sink: &mut K,
    ) -> Result<TransferOutcome, Self::Error>;
}
