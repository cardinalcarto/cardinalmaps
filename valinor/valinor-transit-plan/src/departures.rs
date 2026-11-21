//! A departures lookup: every route's departures between two stops, around a moment.
//!
//! See `docs/transit.md` ("Departure ranges"). A plan lists each ride's departures between its
//! two stops, around the journey's own. A client that keeps a journey open after a later plan
//! no longer has it has no leg to read them from, and asks for them here instead:
//! by the feed and the two stops of the ride, and the time to centre the list on.
//!
//! Like [`crate::plan`], this has no clock: `now` is always an argument.

use core::error::Error;

use crate::error::{Missing, PlanError, StateError};
use crate::plan::{DepartureRange, TravelMode, validated_range, window_time};
use crate::response::{DeparturesResponse, StopPairError, build_departures};
use crate::state::{TransitSnapshot, TransitState};

/// The departures between two stops around a moment, by the identifiers a plan gave.
#[derive(Debug, Clone)]
pub struct DeparturesRequest {
    /// The feed of the two stops.
    pub feed: String,
    /// The boarding stop, by its `stop_id` in the feed.
    pub board_stop_id: String,
    /// The alighting stop, by its `stop_id` in the feed.
    pub alight_stop_id: String,
    /// The departure time the list is centred on, in POSIX seconds:
    /// up to `range.before` departures leave the boarding stop before it,
    /// and up to `range.after` at it or later.
    pub around: i64,
    /// How many departures to list on each side of `around`, and how far from it to look.
    ///
    /// Unlike a plan's, there is no departure of the journey's own between the two sides,
    /// and a range that lists nothing on both is refused.
    pub range: DepartureRange,
    /// When the rider can be at the boarding stop, in POSIX seconds, when the caller knows:
    /// a departure leaving before it is `missed`. None is when `None`.
    pub ready: Option<i64>,
    /// The plan's travel mode, whose rules the departures between the stops are read with.
    pub mode: TravelMode,
    /// The moment the query runs, in POSIX seconds.
    pub now: i64,
}

/// Checks a request's range.
fn validated_departures<T: Error + 'static, A: Error + 'static>(
    request: &DeparturesRequest,
) -> Result<(), PlanError<T, A>> {
    validated_range(request.range)?;
    if request.range.is_empty() {
        return Err(PlanError::InvalidRequest(
            "a departures lookup lists no departures on either side",
        ));
    }
    Ok(())
}

/// Answers a departures lookup over one pinned snapshot, pinning one as a plan departing at
/// `around` would: the current one when its window has that moment's service days,
/// else a window around it with the same feeds.
///
/// # Errors
///
/// If the request is invalid or names what isn't there, no timetable covers `around`,
/// or `interrupt` asks to stop.
pub fn departures<T, A>(
    state: &TransitState,
    request: &DeparturesRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<DeparturesResponse, PlanError<T, A>>
where
    T: Error + 'static,
    A: Error + 'static,
{
    validated_departures(request)?;
    let snapshot = state.pin_for_departure(request.now, request.around, request.around)?;
    departures_pinned(&snapshot, request, interrupt)
}

/// Answers a departures lookup over a snapshot the caller has already pinned.
///
/// # Errors
///
/// As [`departures`], but for the coverage of `around`: a snapshot whose window doesn't have
/// that moment's service days lists nothing, and one too far from it to tell the time on its
/// clock can't answer.
pub fn departures_pinned<T, A>(
    snapshot: &TransitSnapshot,
    request: &DeparturesRequest,
    interrupt: &dyn Fn() -> bool,
) -> Result<DeparturesResponse, PlanError<T, A>>
where
    T: Error + 'static,
    A: Error + 'static,
{
    validated_departures::<T, A>(request)?;
    let around = window_time(snapshot, request.around)?;
    let response = build_departures(snapshot, request, around).map_err(|error| match error {
        StopPairError::UnknownFeed(feed) => PlanError::State(StateError::UnknownFeed(feed)),
        StopPairError::UnknownStop => PlanError::NotFound(Missing::Stop),
    })?;
    if interrupt() {
        return Err(PlanError::Interrupted);
    }
    Ok(response)
}
