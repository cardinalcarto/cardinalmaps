//! The departures between two stops around a moment, for a client that has no plan's leg to
//! read them from any more.
//!
//! A plan's ride lists its departures around its own (see [`super::departures`]); this lists
//! them around a time instead, between the same two stops, found by the same patterns, lanes,
//! and rules of the travel mode, and described by the same code, from the same kind of pinned
//! snapshot. So a client that kept a journey after a plan dropped it can ask again for what the
//! plan would have listed.
//!
//! There is no leg here for a departure to be relative to: each states its route, headsign, and
//! start date, none is `suggested`, and their alerts are in one list.
//!
//! # Performance
//!
//! As a ride's departures: the occurrences at the boarding stop, the stop sequences of those
//! patterns up to the alighting stop, and per lane a binary search and the rows in the time bound.

use valinor_transit::Time;

use crate::departures::DeparturesRequest;
use crate::response::build::{RideDescriber, realtime_status};
use crate::response::departures::{Candidate, DepartureSearch, Serving, pairs_for};
use crate::response::ride::StopPairError;
use crate::response::{AlertResponse, DeparturesResponse, RideDeparture};
use crate::state::TransitSnapshot;

/// Lists the departures a request asks for; `around` is its time on the snapshot's window clock.
pub(crate) fn build_departures(
    snapshot: &TransitSnapshot,
    request: &DeparturesRequest,
    around: Time,
) -> Result<DeparturesResponse, StopPairError> {
    let rides = RideDescriber::new(snapshot, request.mode, request.range);
    let (departures, alerts) = rides.departures_between(request, around)?;
    Ok(DeparturesResponse {
        generated_at: request.now,
        realtime: realtime_status(snapshot, request.now),
        departures,
        alerts,
    })
}

impl RideDescriber<'_> {
    /// The departures between a request's two stops around `around`, in order of departure,
    /// and the alerts their `alerts` index.
    fn departures_between(
        &self,
        request: &DeparturesRequest,
        around: Time,
    ) -> Result<(Vec<RideDeparture>, Vec<AlertResponse>), StopPairError> {
        let stops = self.stop_pair(
            &request.feed,
            &request.board_stop_id,
            &request.alight_stop_id,
        )?;
        let filter = self.mode.route_filter();
        let pairs = pairs_for(&self.snapshot.snapshot, filter, stops);
        let range = self.departures;
        let seconds = |seconds: u32| i32::try_from(seconds).unwrap_or(i32::MAX);
        let around_seconds = around.into_inner();
        // A side that lists nothing reads nothing:
        // "before" ends a second short of `around`, and "after" starts at it.
        let bounds = (
            if range.before == 0 {
                around_seconds
            } else {
                around_seconds.saturating_sub(seconds(range.before_seconds))
            },
            if range.after == 0 {
                around_seconds.saturating_sub(1)
            } else {
                around_seconds.saturating_add(seconds(range.after_seconds))
            },
        );
        let mut candidates = self.candidates(
            &DepartureSearch {
                pairs: &pairs,
                filter,
                serving: Serving::BoardAndAlight,
                bounds,
            },
            None,
            &mut |_, _| true,
        );
        candidates.sort_unstable_by_key(Candidate::order);
        // A departure leaving exactly at `around` is the first of those after it.
        let at = candidates.partition_point(|candidate| candidate.board.departure < around);
        let first = at.saturating_sub(usize::from(range.before));
        let end = at
            .saturating_add(usize::from(range.after))
            .min(candidates.len());
        let chosen = candidates.get(first..end).unwrap_or_default();

        // The pairs are the two stops' alone, as any ride between them would read.
        let latest = self.latest_by_date(chosen, stops, &pairs, filter, true);
        let access = (
            Some(self.stop_ref(stops.0).accessibility),
            Some(self.stop_ref(stops.1).accessibility),
        );
        let mut pool: Vec<AlertResponse> = Vec::new();
        let departures = chosen
            .iter()
            .map(|candidate| {
                let described = self.described(stops, candidate, access);
                let mut departure = described.departure;
                for alert in described.alerts {
                    let index = pool
                        .iter()
                        .position(|known| *known == alert)
                        .unwrap_or_else(|| {
                            pool.push(alert);
                            pool.len() - 1
                        });
                    if !departure.alerts.contains(&index) {
                        departure.alerts.push(index);
                    }
                }
                // In the list's order, whatever order each departure's own alerts came in.
                departure.alerts.sort_unstable();
                departure.missed = request
                    .ready
                    .is_some_and(|ready| departure.departure < ready);
                departure.last_of_day = candidate.is_last_of_day(&latest);
                departure
            })
            .collect();
        Ok((departures, pool))
    }
}
