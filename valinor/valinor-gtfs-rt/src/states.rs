//! The states of a trip in an overlay.
//!
//! An overlay is rebuilt from scratch each time, so a trip's state is a pure function of the
//! feeds and the clock; a transition between two overlays is simply a change in that input.
//! The table is the one in `docs/transit.md`:
//!
//! | State | Input | Effect on routing |
//! |---|---|---|
//! | [`TripState::Scheduled`] | No update for the trip | Base times |
//! | [`TripState::Delayed`] | Updated times | Times replaced, delays propagated along the trip |
//! | [`TripState::StopSkipped`] | `SKIPPED` stop time update | No boarding or alighting there |
//! | [`TripState::Canceled`] | Canceled or deleted trip | Removed from that service day |
//! | [`TripState::Added`], [`TripState::Replaced`] | New trip or stop pattern | Own pattern in the overlay |
//! | (new stop) | `Stop` entity or `replacement_stops`, not in the static feed | Stop appended; patterns may use it |
//! | [`TripState::NoService`] | `NO_SERVICE` alert active at the trip's time | Removed; or the stop is removed when the selector names a stop |
//! | (stale) | Feed older than its limit | The feed is ignored: everything falls back to Scheduled |
//!
//! ## Transitions
//!
//! Each of these has a test named after it in `src/overlay_tests.rs`:
//!
//! - `Scheduled` → `Delayed`: a trip update with times arrives.
//! - `Delayed` → `Scheduled`: the update vanishes or its delay becomes zero.
//! - `Delayed` → `Delayed`: propagation stops at the next stop time update.
//! - `Scheduled` → `StopSkipped`: a `SKIPPED` stop time update arrives.
//! - `StopSkipped` → `Scheduled`: the skip vanishes.
//! - `Delayed` → `StopSkipped`: both an update and a skip; delay carries across the skipped stop.
//! - `Scheduled` → `Canceled`: a `CANCELED` or `DELETED` trip descriptor.
//! - `Canceled` → `Scheduled`: the cancellation vanishes.
//! - `Canceled` wins over `Delayed`: a cancellation ignores any times.
//! - `Scheduled` → `Replaced`: a `REPLACEMENT` descriptor, `TripModifications`, or `assigned_stop_id`.
//! - `Replaced` → `Scheduled`: the replacement vanishes and the original pattern returns.
//! - (no trip) → `Added`: `NEW`, `ADDED`, or `DUPLICATED`; → removed when it vanishes.
//! - `Added` may use a new stop; the new stop disappears when its feed goes stale.
//! - Any → `NoService`: an active `NO_SERVICE` alert; → back when it ends.
//! - `NoService` wins over `Delayed`, `Replaced`, and `Added`.
//! - `Delayed`, `StopSkipped`, `Canceled`, `Replaced`, `Added` → `Scheduled`: the feed goes stale.
//!
//! A real trip can be delayed and skip stops at once:
//! [`TripState`] reports the strongest effect, and both take place in routing.

/// What the overlay did to a trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TripState {
    /// The trip runs on its base times.
    Scheduled,
    /// The trip's times were replaced.
    Delayed,
    /// At least one stop is skipped; other stops may also be delayed.
    StopSkipped,
    /// The trip was canceled or deleted, and is removed from its service day.
    Canceled,
    /// The trip is new, or is a duplicate of a scheduled one.
    Added,
    /// The trip's stop pattern was changed.
    Replaced,
    /// A `NO_SERVICE` alert removed the trip.
    NoService,
}
