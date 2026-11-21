//! One of a ride's departures as a leg of its own, for a client to draw and list the departure a
//! rider picked instead of the journey's own.
//!
//! A ride's departures (see [`super::departures`]) say when each leaves and arrives; this
//! describes one as a plan's leg on it would be: every stop with its times, the shape, the
//! segments of a stay-seated ride, the vehicle, and the alerts, from the boarding stop to the
//! alighting stop. The departure is found as the ride's list finds it, by the same patterns
//! between the two stops, lanes, and rules of the travel mode, from the same kind of pinned
//! snapshot, so the leg agrees with what the list said of it.
//!
//! # Performance
//!
//! The patterns between the two stops are found from the boarding stop's occurrences, as for a
//! ride's departures. Every row of those patterns on the requested service date is resolved to
//! its `trip_id`, as a ride's departure is described, without a time bound; only the one
//! chosen is described in full.

use valinor_transit::{Ride, ServiceDate};

use crate::error::{Missing, NotRunning};
use crate::plan::DepartureRange;
use crate::response::build::{RideDescriber, realtime_status};
use crate::response::departures::{Candidate, DepartureSearch, Found, Serving, pairs_for};
use crate::response::{RideResponse, TransitLeg};
use crate::ride::RideRequest;
use crate::state::TransitSnapshot;

/// Why a ride lookup couldn't be answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RideLookupError {
    /// The request's feed isn't loaded.
    UnknownFeed(String),
    /// A stop it names isn't in the feed, or no departure between the two is the trip.
    NotFound(Missing),
    /// The departure is listed between the stops, but doesn't run between them.
    NotRunning(NotRunning),
}

/// Describes the departure a request names on `date`, its `start_date`.
pub(crate) fn build_ride(
    snapshot: &TransitSnapshot,
    request: &RideRequest,
    date: ServiceDate,
) -> Result<RideResponse, RideLookupError> {
    // A ride's own departures are the plan's to list; the leg is the departure alone.
    let rides = RideDescriber::new(snapshot, request.mode, DepartureRange::NONE);
    let ride = rides.departure_ride(request, date)?;
    Ok(RideResponse {
        generated_at: request.now,
        realtime: realtime_status(snapshot, request.now),
        ride,
    })
}

impl RideDescriber<'_> {
    /// The leg of the departure a request names, found among those between its two stops.
    fn departure_ride(
        &self,
        request: &RideRequest,
        date: ServiceDate,
    ) -> Result<TransitLeg, RideLookupError> {
        let part = self
            .set()
            .part_of_feed(&request.feed)
            .ok_or_else(|| RideLookupError::UnknownFeed(request.feed.clone()))?;
        let stop = |stop_id: &str| {
            self.feed_stop(part, &request.feed, stop_id)
                .ok_or(RideLookupError::NotFound(Missing::Stop))
        };
        let (board_stop, alight_stop) = (
            stop(&request.board_stop_id)?,
            stop(&request.alight_stop_id)?,
        );
        let filter = self.mode.route_filter();
        let pairs = pairs_for(&self.snapshot.snapshot, filter, (board_stop, alight_stop));
        // A departure's `trip_id` is the trip it boards, as a ride's list names it.
        let mut found = self.candidates(
            &DepartureSearch {
                pairs: &pairs,
                filter,
                serving: Serving::BoardAndAlight,
                bounds: (i32::MIN, i32::MAX),
            },
            None,
            &mut |key, pair| {
                key.date == date
                    && self
                        .trip_view(key, pair.route, (pair.board, pair.alight))
                        .trip_id
                        == request.trip_id
            },
        );
        found.sort_unstable_by_key(Candidate::order);
        // The runs of a frequency trip share their `trip_id` and `start_date`: the one leaving
        // nearest the requested time is meant, the earliest of two as near.
        let origin = self.origin();
        let chosen = match request.departure {
            Some(wanted) => found.iter().min_by_key(|candidate| {
                (origin + i64::from(candidate.board.departure.into_inner())).abs_diff(wanted)
            }),
            None => found.first(),
        }
        .ok_or(RideLookupError::NotFound(Missing::Trip))?;
        match chosen.found {
            Found::Runs => {}
            Found::Canceled => return Err(RideLookupError::NotRunning(NotRunning::Canceled)),
            Found::Skipped => return Err(RideLookupError::NotRunning(NotRunning::Skipped)),
        }
        let ride = Ride {
            route: chosen.pair.route,
            lane: chosen.lane,
            row: chosen.row,
            trip: chosen.key,
            board_stop,
            board_position: chosen.pair.board,
            board_time: chosen.board.departure,
            alight_stop,
            alight_position: chosen.pair.alight,
            alight_time: chosen.alight.arrival,
        };
        // Nothing says when the rider gets to the stop, so nothing is waited.
        Ok(self.transit_leg(&ride, 0, &|_| {}))
    }
}
