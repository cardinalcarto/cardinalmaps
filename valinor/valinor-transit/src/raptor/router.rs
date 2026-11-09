//! The public face of the search: queries, runs, and the reusable router.

use std::sync::Arc;

use crate::error::{QueryError, SearchError};
use crate::ids::{Direction, Time};
use crate::journey::{Journey, trace_journey};
use crate::profile::{self, Profile, ProfileOptions};
use crate::raptor::clock_of;
use crate::raptor::labels::Labels;
use crate::raptor::search::{self, MAX_TRIPS, SearchInput};
use crate::snapshot::Snapshot;
use crate::stop_table::{RouteFilter, StopTable};
use crate::timetable::INFINITY;
use crate::transfer::TransferSearch;

/// What a profile or search is asked.
///
/// A query pins one snapshot: all of its searches read the same real-time state,
/// and the journeys it returns refer to that snapshot's lanes.
#[derive(Debug, Clone)]
pub struct TransitQuery<'a> {
    /// The pinned timetable state.
    pub snapshot: Arc<Snapshot>,
    /// Seconds from the origin to each stop.
    pub access: &'a StopTable,
    /// Seconds from each stop to the target.
    pub egress: &'a StopTable,
    /// Which routes may be ridden.
    pub filter: RouteFilter,
    /// The most trips a journey may use; at most [`MAX_TRIPS`].
    pub max_trips: u8,
}

impl TransitQuery<'_> {
    pub(crate) fn validate(&self) -> Result<(), QueryError> {
        if self.max_trips == 0 || self.max_trips > MAX_TRIPS {
            return Err(QueryError::MaxTrips(self.max_trips));
        }
        for table in [self.access, self.egress] {
            if table.stop_count() != self.snapshot.stop_count() {
                return Err(QueryError::StopTableSize {
                    expected: self.snapshot.stop_count(),
                    got: table.stop_count(),
                });
            }
        }
        Ok(())
    }

    fn input<'b>(&'b self, direction: Direction, bounds: &'b [i32]) -> SearchInput<'b> {
        let (seeding, target) = match direction {
            Direction::Forward => (self.access, self.egress),
            Direction::Backward => (self.egress, self.access),
        };
        SearchInput {
            snapshot: &self.snapshot,
            filter: self.filter,
            seeding,
            target,
            max_trips: self.max_trips,
            bounds,
        }
    }
}

/// The best way to reach a search's target using up to some number of trips.
///
/// A forward search reports arrival times, a backward search reports departure times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundResult {
    /// The number of trips.
    pub trips: u8,
    /// The arrival (forward) or departure (backward) at the end of the journey.
    pub time: Time,
}

/// A finished search, from which journeys can be read.
#[derive(Debug)]
pub struct SearchRun<'r, S: TransferSearch> {
    query: &'r TransitQuery<'r>,
    labels: &'r Labels<S::Parent>,
    transfers: &'r S,
    direction: Direction,
}

impl<S: TransferSearch> SearchRun<'_, S> {
    /// The trip counts at which the search improved on the target.
    ///
    /// # Performance
    ///
    /// Reads the search's labels without allocating beyond the returned vector.
    pub fn results(&self) -> Vec<RoundResult> {
        round_results(self.labels, self.direction)
    }

    /// The journey with this number of trips, if the search improved the target with that many.
    pub fn journey(&self, trips: u8) -> Option<Journey<S::Walk>> {
        trace_journey(self.query, self.labels, self.direction, trips)
            .map(|journey| journey.resolve_walks(|parent| self.transfers.walk(parent)))
    }
}

pub(crate) fn round_results<P: Copy>(labels: &Labels<P>, direction: Direction) -> Vec<RoundResult> {
    let mut out = Vec::new();
    for (trips, target) in labels.targets().iter().enumerate() {
        if !target.improved || target.time >= INFINITY {
            continue;
        }
        let real = match direction {
            Direction::Forward => clock_of::<false>(target.time),
            Direction::Backward => clock_of::<true>(target.time),
        };
        let (Ok(time), Ok(trips)) = (Time::try_new(real), u8::try_from(trips)) else {
            continue;
        };
        out.push(RoundResult { trips, time });
    }
    out
}

/// Runs transit searches, reusing its memory between them.
///
/// Holds the two sets of labels and the transfer search.
/// One router serves one thread; keep one per worker.
#[derive(Debug)]
pub struct Router<S: TransferSearch> {
    pub(crate) transfers: S,
    pub(crate) forward: Labels<S::Parent>,
    pub(crate) backward: Labels<S::Parent>,
}

impl<S: TransferSearch> Router<S> {
    /// A router over a transfer search.
    pub fn new(transfers: S) -> Self {
        Self {
            transfers,
            forward: Labels::new(),
            backward: Labels::new(),
        }
    }

    /// The transfer search, to reconstruct walks from a journey's parent handles.
    pub fn transfers(&self) -> &S {
        &self.transfers
    }

    /// Runs a forward search departing at or after `departure`.
    ///
    /// The journeys it finds use at least one trip, and at most `query.max_trips`.
    ///
    /// # Errors
    ///
    /// If the query is invalid, `interrupt` asks to stop, or a transfer search fails.
    pub fn forward<'r>(
        &'r mut self,
        query: &'r TransitQuery<'r>,
        departure: Time,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<SearchRun<'r, S>, SearchError<S::Error>> {
        query.validate()?;
        search::run::<false, S>(
            &query.input(Direction::Forward, &[]),
            &mut self.forward,
            departure.into_inner(),
            &mut self.transfers,
            interrupt,
        )?;
        Ok(SearchRun {
            query,
            labels: &self.forward,
            transfers: &self.transfers,
            direction: Direction::Forward,
        })
    }

    /// Runs a backward search arriving at or before `arrival`.
    ///
    /// It is the exact mirror of [`Self::forward`].
    /// The times it reports are the latest departures that can arrive by `arrival`.
    ///
    /// # Errors
    ///
    /// If the query is invalid, `interrupt` asks to stop, or a transfer search fails.
    pub fn backward<'r>(
        &'r mut self,
        query: &'r TransitQuery<'r>,
        arrival: Time,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<SearchRun<'r, S>, SearchError<S::Error>> {
        query.validate()?;
        search::run::<true, S>(
            &query.input(Direction::Backward, &[]),
            &mut self.backward,
            clock_of::<true>(arrival.into_inner()),
            &mut self.transfers,
            interrupt,
        )?;
        Ok(SearchRun {
            query,
            labels: &self.backward,
            transfers: &self.transfers,
            direction: Direction::Backward,
        })
    }

    /// Computes the profile for departures between `earliest` and `latest`, inclusive.
    ///
    /// # Errors
    ///
    /// If the query or window is invalid, `interrupt` asks to stop, or a transfer search fails.
    pub fn profile(
        &mut self,
        query: &TransitQuery<'_>,
        earliest: Time,
        latest: Time,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<Profile<S::Walk>, SearchError<S::Error>> {
        query.validate()?;
        if earliest > latest {
            return Err(QueryError::EmptyWindow.into());
        }
        self.profile_with(
            query,
            earliest,
            latest,
            ProfileOptions::default(),
            interrupt,
        )
    }

    /// [`Self::profile`], with its optional prunings chosen explicitly.
    ///
    /// Each option leaves every search's results unchanged; they exist to measure and test that.
    ///
    /// # Errors
    ///
    /// If the query or window is invalid, `interrupt` asks to stop, or a transfer search fails.
    pub fn profile_with(
        &mut self,
        query: &TransitQuery<'_>,
        earliest: Time,
        latest: Time,
        options: ProfileOptions,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<Profile<S::Walk>, SearchError<S::Error>> {
        query.validate()?;
        if earliest > latest {
            return Err(QueryError::EmptyWindow.into());
        }
        profile::run_profile(self, query, earliest, latest, options, interrupt)
    }

    pub(crate) fn run_forward(
        &mut self,
        query: &TransitQuery<'_>,
        departure: i32,
        bounds: &[i32],
        interrupt: &dyn Fn() -> bool,
    ) -> Result<(), SearchError<S::Error>> {
        search::run::<false, S>(
            &query.input(Direction::Forward, bounds),
            &mut self.forward,
            departure,
            &mut self.transfers,
            interrupt,
        )
    }

    pub(crate) fn run_backward(
        &mut self,
        query: &TransitQuery<'_>,
        arrival: i32,
        bounds: &[i32],
        interrupt: &dyn Fn() -> bool,
    ) -> Result<(), SearchError<S::Error>> {
        search::run::<true, S>(
            &query.input(Direction::Backward, bounds),
            &mut self.backward,
            clock_of::<true>(arrival),
            &mut self.transfers,
            interrupt,
        )
    }
}
