//! Valhalla-compatible path search, map matching, and trip building: mainline's Thor,
//! for `route`, `trace_route`, and `trace_attributes` requests.
//!
//! See `docs/routing.md` for where this sits in the stage chain:
//! Loki correlates each location to candidate edges, Thor finds the best paths between them,
//! and trip building (mainline's `TripLegBuilder`, also part of Thor) turns the paths into
//! the `trip` that Odin narrates.
//!
//! - [`Thor::route`] is mainline's `thor_worker_t::route` up to trip building:
//!   it scores the candidates, picks a path algorithm for each pair of locations
//!   (bidirectional A* by default; time-dependent A*, forward or reverse, for routes with
//!   a time; forward A* for trivial routes), retries with relaxed limits when a search fails,
//!   joins the segments between through locations into legs, and moves the locations'
//!   times along the route.
//!   It returns the [`Paths`]: per route (the best, then alternates) and per leg,
//!   the edges with their elapsed and transition costs.
//! - [`Thor::build_trip`] is mainline's trip building (`TripLegBuilder`):
//!   it turns the [`Paths`] into the request's `trip`, a `TripLeg` per leg,
//!   with the shape, the attributes of each edge and node the request asks for
//!   (mainline's `AttributesController`, from `valinor-tyr`), and the locations' shape indexes.
//! - [`Thor::trace_route`] and [`Thor::trace_attributes`] are mainline's map matching actions:
//!   an edge walk along a route's own shape (`edge_walk`), Meili's map matching (`map_snap`),
//!   or the walk with map matching when it fails (`walk_or_snap`), then trip building.
//!   `trace_route` leaves the trip for Odin (a route per discontinuity),
//!   and `trace_attributes` answers with Tyr's serializer, as mainline's Thor does.
//! - A [`Workspace`] holds the searches' storage, kept between requests so that searches
//!   don't allocate as they go; map matching takes a [`valinor_meili::Workspace`].
//!
//! Results are equivalent to mainline 3.9.1's within the tolerances in `docs/routing.md`;
//! what decides which of nearly equal paths wins (the queue's order within buckets,
//! single precision arithmetic, hierarchy limits, and the searches' thresholds)
//! follows mainline exactly.
//!
//! Not here (yet): `optimized_route`, isochrones, matrices,
//! expansion, multimodal and transit routing, live traffic, and landmarks.

#![expect(
    clippy::similar_names,
    reason = "the searches name tiles, times, and costs as mainline does, side by side"
)]

extern crate alloc;

mod alternates;
mod bidirectional;
mod config;
mod edge_status;
mod error;
mod geo;
mod graph;
mod label;
mod map_matcher;
mod path;
mod route;
mod route_matcher;
mod search;
mod time;
mod trace;
mod trip;
mod unidirectional;

#[cfg(test)]
mod bench_tests;
#[cfg(test)]
mod edge_walk_tests;
#[cfg(test)]
mod oracle_tests;
#[cfg(test)]
mod trace_oracle_tests;
#[cfg(test)]
mod trip_oracle_tests;

pub use config::{ConfigError, HierarchyLimitsConfig, Retention, ThorConfig};
pub use error::ThorError;
pub use path::{EdgeTrim, EdgeTrimming, Leg, PathEdge, Paths, Route};
pub use route::{Thor, Workspace};
