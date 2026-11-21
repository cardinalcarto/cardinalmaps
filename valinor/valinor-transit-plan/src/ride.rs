//! A ride lookup: one of a plan ride's departures, described as the plan's leg on it would be.
//!
//! See `docs/transit.md` ("Departure ranges"). A plan lists each ride's departures between its
//! two stops; a client that lets the rider pick another asks for that one's leg, to draw its
//! shape and list its stops. The request names it by what the plan gave: the feed and the two
//! stops of the ride, and the departure's `trip_id` and `start_date`.
//!
//! Like [`crate::plan`], this has no clock: `now` is always an argument.

use core::error::Error;

use valinor_gtfs::parse_service_date;
use valinor_gtfs_rt::CalendarDate;
use valinor_transit::ServiceDate;

use crate::error::{PlanError, StateError};
use crate::plan::TravelMode;
use crate::response::{RideLookupError, RideResponse, build_ride};
use crate::state::{TransitSnapshot, TransitState};

/// One departure between two stops of a ride, by the identifiers a plan gave.
#[derive(Debug, Clone)]
pub struct RideRequest {
    /// The feed of the ride's stops.
    pub feed: String,
    /// The ride's boarding stop, by its `stop_id` in the feed.
    pub board_stop_id: String,
    /// The ride's alighting stop, by its `stop_id` in the feed.
    pub alight_stop_id: String,
    /// The departure's `trip_id`: the trip it boards.
    pub trip_id: String,
    /// The departure's `start_date`, as `YYYYMMDD`: the leg's own when the departure leaves
    /// it out.
    pub start_date: String,
    /// When the departure leaves the boarding stop, in POSIX seconds, which tells the runs of a
    /// frequency trip apart: the run leaving nearest it is meant. The first run when `None`.
    pub departure: Option<i64>,
    /// The plan's travel mode, whose rules the departures between the stops are read with.
    pub mode: TravelMode,
    /// The moment the query runs, in POSIX seconds.
    pub now: i64,
}

/// The service date a request names.
pub(crate) fn validated_ride<T: Error + 'static, A: Error + 'static>(
    request: &RideRequest,
) -> Result<ServiceDate, PlanError<T, A>> {
    CalendarDate::parse(&request.start_date)
        .and_then(|_| parse_service_date(&request.start_date))
        .ok_or(PlanError::InvalidRequest(
            "start_date is not a YYYYMMDD date",
        ))
}

/// Answers a ride lookup over one pinned snapshot, pinning one that has the start date's trips.
///
/// # Errors
///
/// If the request is invalid or names what isn't there, the departure doesn't run between the
/// two stops, or `interrupt` asks to stop.
pub fn ride<T, A>(
    state: &TransitState,
    request: &RideRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<RideResponse, PlanError<T, A>>
where
    T: Error + 'static,
    A: Error + 'static,
{
    let date = validated_ride(request)?;
    let snapshot = state.pin_for_service_date(request.now, date)?;
    ride_pinned(&snapshot, request, interrupt)
}

/// Answers a ride lookup over a snapshot the caller has already pinned.
///
/// # Errors
///
/// As [`ride`]; a snapshot whose window doesn't have the start date has no such trip.
pub fn ride_pinned<T, A>(
    snapshot: &TransitSnapshot,
    request: &RideRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<RideResponse, PlanError<T, A>>
where
    T: Error + 'static,
    A: Error + 'static,
{
    let date = validated_ride::<T, A>(request)?;
    let response = build_ride(snapshot, request, date).map_err(|error| match error {
        RideLookupError::UnknownFeed(feed) => PlanError::State(StateError::UnknownFeed(feed)),
        RideLookupError::NotFound(missing) => PlanError::NotFound(missing),
        RideLookupError::NotRunning(why) => PlanError::NotRunning(why),
    })?;
    if interrupt() {
        return Err(PlanError::Interrupted);
    }
    Ok(response)
}
