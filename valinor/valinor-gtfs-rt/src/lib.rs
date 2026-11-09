//! Decodes GTFS-RT and builds real-time overlays for transit routing.
//!
//! - [`DecodedFeed::decode`] turns bytes into a feed message; malformed input is an error.
//! - [`StaticLookup`] is how the compiled GTFS answers questions about the timetable.
//! - [`build_overlay`] combines feeds, static data, and a `now` into a [`RealtimeOverlay`]:
//!   an immutable [`valinor_transit::Snapshot`] for the search,
//!   and separate [`RiderData`] for the response layer.
//!
//! See `docs/transit.md` for the design, and [`TripState`] for the trip states and transitions.
//! This crate has no network access and reads no clock, so it also runs on phones.

mod alerts;
mod date;
mod decode;
mod feed;
mod lookup;
mod modifications;
mod overlay;
mod propagate;
pub mod proto;
mod rider;
mod states;

#[cfg(test)]
mod overlay_tests;
#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod testing;

pub use alerts::Period;
pub use date::{CalendarDate, parse_start_time};
pub use decode::{DecodeError, DecodeLimits, DecodedFeed};
pub use feed::{
    DEFAULT_ALERTS_STALE_AFTER, DEFAULT_REALTIME_STALE_AFTER, FeedId, FeedIdError, FeedInput,
    FeedKind, FeedStatus, ScopedId,
};
pub use lookup::{RouteInfo, ScheduledTrip, StaticLookup, TripQuery, TripRef, WindowDay};
pub use overlay::{
    AlertImpact, DEFAULT_MAX_PREDICTION_OFFSET, NewStop, OverlayError, OverlayOptions,
    OverlayStats, RealtimeOverlay, build_overlay,
};
pub use rider::{
    AlertDetails, InformedEntity, RiderData, StopTimeDetails, TripDetails, VehicleDetails,
    VehicleRef,
};
pub use states::TripState;
