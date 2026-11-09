//! The profile loop: alternating forward and backward searches.
//!
//! Every Pareto-optimal `(departure, arrival, trips)` in the window is found by a chain:
//! the forward search at `earliest` gives an arrival no later than the wanted one,
//! the backward search from an arrival gives the latest departures reaching it,
//! and the forward search just after such a departure gives the next arrival for that trip count.
//! Arrivals only grow along such a chain, so it reaches every wanted arrival.

use std::collections::{BTreeSet, BinaryHeap};

use crate::error::SearchError;
use crate::ids::Direction;
use crate::ids::Time;
use crate::journey::{Journey, trace_journey};
use crate::raptor::{Router, TransitQuery, round_results};
use crate::timetable::INFINITY;
use crate::transfer::TransferSearch;

/// One Pareto-optimal way to make the journey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileEntry<W> {
    /// When the rider leaves the origin.
    pub departure: Time,
    /// When the rider reaches the target.
    pub arrival: Time,
    /// How many trips it uses.
    pub trips: u8,
    /// The journey.
    pub journey: Journey<W>,
}

/// All Pareto-optimal journeys of a window, by departure then trips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile<W> {
    /// The entries, by increasing departure, then trips.
    pub entries: Vec<ProfileEntry<W>>,
}

/// Which optional prunings a profile uses. None of them changes what any search reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileOptions {
    /// Bound each search with the journeys the profile already knows, and each backward search
    /// with the start of the window.
    pub known_journey_bounds: bool,
    /// The walk limit of a bootstrap pass, in seconds, or `None` for no pass.
    ///
    /// A forward search at the end of the window, with walks this short, finds real journeys
    /// that depart no earlier than any search of the profile, so they bound all of them.
    /// Needs `known_journey_bounds`. The limit only decides how good those bounds are.
    pub bootstrap_walk_limit: Option<u32>,
}

/// The default limit of a profile's bootstrap walks: 30 minutes.
pub const DEFAULT_BOOTSTRAP_WALK_LIMIT: u32 = 1800;

impl Default for ProfileOptions {
    fn default() -> Self {
        Self {
            known_journey_bounds: true,
            bootstrap_walk_limit: Some(DEFAULT_BOOTSTRAP_WALK_LIMIT),
        }
    }
}

/// The journeys a profile has found, as bounds for its later searches.
///
/// # Why these bounds change nothing
///
/// A forward search at `τ` reports, per number of trips `k`,
/// the earliest arrival over all journeys departing at or after `τ` with at most `k` trips.
/// A known journey departing at or after `τ` with `j <= k` trips and arrival `a`
/// is one of those journeys, so the search's report for `k` is at most `a`,
/// and `a + 1` is a valid exclusive bound for round `k`.
/// Mirrored, a backward search from arrival `A` reports the latest departure with at most `k`
/// trips arriving by `A`, so a known journey arriving by `A` with departure `d` makes `-d + 1`
/// a valid exclusive bound on that round's clock.
/// With an exclusive bound above what the round reports, the round reports the same thing.
#[derive(Debug, Default)]
struct KnownJourneys {
    /// `(departure, arrival, trips)`.
    journeys: Vec<(i32, i32, u8)>,
}

impl KnownJourneys {
    fn add(&mut self, departure: i32, arrival: i32, trips: u8) {
        self.journeys.push((departure, arrival, trips));
    }

    /// Fills `bounds` by round for a forward search at `departure`.
    fn forward_bounds(&self, departure: i32, bounds: &mut [i32]) {
        bounds.fill(INFINITY);
        for &(d, a, trips) in &self.journeys {
            if d >= departure
                && let Some(slot) = bounds.get_mut(usize::from(trips))
            {
                *slot = (*slot).min(a + 1);
            }
        }
        prefix_min(bounds);
    }

    /// Fills `bounds` by round for a backward search arriving by `arrival`.
    ///
    /// Departures before `earliest` are never used, so that is a bound for every round.
    fn backward_bounds(&self, arrival: i32, earliest: i32, bounds: &mut [i32]) {
        bounds.fill(1 - earliest);
        for &(d, a, trips) in &self.journeys {
            if a <= arrival
                && let Some(slot) = bounds.get_mut(usize::from(trips))
            {
                *slot = (*slot).min(1 - d);
            }
        }
        prefix_min(bounds);
    }
}

/// A journey with fewer trips also bounds every larger number of trips.
///
/// This also keeps the bounds non-increasing by round,
/// which the transfer search's carry-over of node labels across rounds relies on.
fn prefix_min(bounds: &mut [i32]) {
    let mut best = INFINITY;
    for bound in bounds {
        best = best.min(*bound);
        *bound = best;
    }
}

pub(crate) fn run_profile<S: TransferSearch>(
    router: &mut Router<S>,
    query: &TransitQuery<'_>,
    earliest: Time,
    latest: Time,
    options: ProfileOptions,
    interrupt: &dyn Fn() -> bool,
) -> Result<Profile<S::Walk>, SearchError<S::Error>> {
    let earliest_seconds = earliest.into_inner();
    let latest_seconds = latest.into_inner();

    let mut queue: BinaryHeap<core::cmp::Reverse<i32>> = BinaryHeap::new();
    let mut seen_arrivals = BTreeSet::new();
    let mut seen_departures = BTreeSet::new();
    let mut entries = Vec::new();
    // The entries so far, to skip resolving the walks of one they already beat.
    let mut found: Vec<(Time, Time, u8)> = Vec::new();

    let mut known = KnownJourneys::default();
    let mut bounds = vec![INFINITY; usize::from(query.max_trips) + 1];

    if options.known_journey_bounds
        && let Some(limit) = options.bootstrap_walk_limit
    {
        router.transfers.set_walk_limit(Some(limit));
        let outcome = router.run_forward(query, latest_seconds, &[], interrupt);
        router.transfers.set_walk_limit(None);
        outcome?;
        for result in round_results(&router.forward, Direction::Forward) {
            known.add(latest_seconds, result.time.into_inner(), result.trips);
        }
        known.forward_bounds(earliest_seconds, &mut bounds);
    }

    router.run_forward(query, earliest_seconds, &bounds, interrupt)?;
    push_arrivals(router, &mut queue, &mut seen_arrivals);

    while let Some(core::cmp::Reverse(arrival)) = queue.pop() {
        if options.known_journey_bounds {
            known.backward_bounds(arrival, earliest_seconds, &mut bounds);
        } else {
            bounds.fill(INFINITY);
        }
        router.run_backward(query, arrival, &bounds, interrupt)?;

        let mut departures = Vec::new();
        for result in round_results(&router.backward, Direction::Backward) {
            let departure = result.time.into_inner();
            known.add(departure, arrival, result.trips);
            if departure < earliest_seconds || departure > latest_seconds {
                continue;
            }
            // Before the filter below, though either order is correct: `departure` is the latest
            // for this arrival and trip count, so an entry that beats this one has this same
            // departure, which was queued when that entry was found.
            if departure < latest_seconds && seen_departures.insert(departure) {
                departures.push(departure);
            }
            let Some(trace) =
                trace_journey(query, &router.backward, Direction::Backward, result.trips)
            else {
                continue;
            };
            let beaten = found
                .iter()
                .any(|&(d, a, k)| d >= trace.departure && a <= trace.arrival && k <= trace.trips);
            if beaten {
                continue;
            }
            found.push((trace.departure, trace.arrival, trace.trips));
            let journey = trace.resolve_walks(|parent| router.transfers.walk(parent));
            entries.push(ProfileEntry {
                departure: journey.departure,
                arrival: journey.arrival,
                trips: journey.trips,
                journey,
            });
        }

        for departure in departures {
            if options.known_journey_bounds {
                known.forward_bounds(departure + 1, &mut bounds);
            } else {
                bounds.fill(INFINITY);
            }
            router.run_forward(query, departure + 1, &bounds, interrupt)?;
            push_arrivals(router, &mut queue, &mut seen_arrivals);
        }
    }

    Ok(Profile {
        entries: pareto_reduce(entries),
    })
}

fn push_arrivals<S: TransferSearch>(
    router: &Router<S>,
    queue: &mut BinaryHeap<core::cmp::Reverse<i32>>,
    seen: &mut BTreeSet<i32>,
) {
    for result in round_results(&router.forward, Direction::Forward) {
        let arrival = result.time.into_inner();
        if seen.insert(arrival) {
            queue.push(core::cmp::Reverse(arrival));
        }
    }
}

/// Keeps the entries no other entry beats on departure, arrival, and trips.
///
/// A later departure, an earlier arrival, and fewer trips are each better.
pub(crate) fn pareto_reduce<W>(mut entries: Vec<ProfileEntry<W>>) -> Vec<ProfileEntry<W>> {
    // Every possible dominator of an entry sorts before it.
    entries.sort_by_key(|e| (core::cmp::Reverse(e.departure), e.arrival, e.trips));

    let mut kept: Vec<ProfileEntry<W>> = Vec::new();
    let mut best_arrival_by_trips: Vec<Option<Time>> = Vec::new();
    for entry in entries {
        let trips = usize::from(entry.trips);
        if best_arrival_by_trips.len() <= trips {
            best_arrival_by_trips.resize(trips + 1, None);
        }
        let dominated = best_arrival_by_trips[..=trips]
            .iter()
            .any(|best| best.is_some_and(|best| best <= entry.arrival));
        if dominated {
            continue;
        }
        best_arrival_by_trips[trips] = Some(entry.arrival);
        kept.push(entry);
    }
    kept.sort_by_key(|e| (e.departure, e.trips));
    kept
}
