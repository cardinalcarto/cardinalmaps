//! Valhalla-compatible path search: mainline's Thor, for `route` requests.
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
//! - A [`Workspace`] holds the searches' storage, kept between requests so that searches
//!   don't allocate as they go.
//!
//! Results are equivalent to mainline 3.9.1's within the tolerances in `docs/routing.md`;
//! what decides which of nearly equal paths wins (the queue's order within buckets,
//! single precision arithmetic, hierarchy limits, and the searches' thresholds)
//! follows mainline exactly.
//!
//! Not here (yet): trip building, `optimized_route`, map matching, isochrones, matrices,
//! expansion, multimodal and transit routing, live traffic, and interrupting a search.

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
mod path;
mod queue;
mod route;
mod search;
mod time;
mod unidirectional;

#[cfg(test)]
mod bench_tests;
#[cfg(test)]
mod oracle_tests;

pub use config::{ConfigError, HierarchyLimitsConfig, ThorConfig};
pub use error::ThorError;
pub use path::{EdgeTrim, EdgeTrimming, Leg, PathEdge, Paths, Route};
pub use route::{Thor, Workspace};
