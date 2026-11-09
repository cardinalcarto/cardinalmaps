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

pub(crate) fn run_profile<S: TransferSearch>(
    router: &mut Router<S>,
    query: &TransitQuery<'_>,
    earliest: Time,
    latest: Time,
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

    router.run_forward(query, earliest_seconds, interrupt)?;
    push_arrivals(router, &mut queue, &mut seen_arrivals);

    while let Some(core::cmp::Reverse(arrival)) = queue.pop() {
        router.run_backward(query, arrival, interrupt)?;

        let mut departures = Vec::new();
        for result in round_results(&router.backward, Direction::Backward) {
            let departure = result.time.into_inner();
            if departure < earliest_seconds || departure > latest_seconds {
                continue;
            }
            // This comes before the filter below: a forward search after a departure can lead to
            // entries that matter even when this one is beaten.
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
            router.run_forward(query, departure + 1, interrupt)?;
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
