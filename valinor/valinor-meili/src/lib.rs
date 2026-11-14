//! Valhalla-compatible map matching: mainline's Meili.
//!
//! See `docs/routing.md` for where this sits:
//! Meili is a library that Thor calls for `trace_route` and `trace_attributes`
//! (with `shape_match` `map_snap`, or `walk_or_snap` when the edge walk fails),
//! as Sif is a library that Loki and Thor call.
//!
//! - [`MapMatcherFactory`] holds the `meili` config ([`MeiliConfig`]) and makes a [`MapMatcher`]
//!   for each request, with the request's customizable `trace_options` and its costing.
//! - [`Measurement::from_shape`] makes the measurements of a request's shape, as Thor does.
//! - [`MapMatcher::offline_match`] matches them (mainline's `OfflineMatch`), returning up to k
//!   [`MatchResults`]; a [`Workspace`] holds what it needs between requests,
//!   and the candidate of each matched state ([`Workspace::candidate`]).
//! - [`MeiliError`] says how mainline's Thor reports each failure.
//!
//! The types in [`match_result`] are what Meili hands Thor (and, through Thor, Tyr's
//! `trace_attributes` serializer), shaped like mainline's `meili/match_result.h`.
//!
//! Results are equivalent to mainline 3.9.1's within the tolerances in `docs/routing.md`;
//! the arithmetic and queue order that decide which edges win follow mainline,
//! but not its hash order (see `candidate_search`).
//! Nothing here is async or panics on bad tile data: corrupt tiles are errors.

#![allow(
    clippy::similar_names,
    reason = "mainline's names (tiles, times, mins and maxes), side by side"
)]

extern crate alloc;

mod candidate_search;
mod config;
mod error;
mod geometry;
mod grid;
mod map_matcher;
pub mod match_result;
mod match_route;
mod measurement;
mod routing;
mod state;
mod topk;
mod viterbi;

#[cfg(test)]
mod oracle_tests;

pub use candidate_search::{Candidate, CandidateEdge, CandidateGrids};
pub use config::{
    CandidateSearchConfig, ConfigError, EmissionCostConfig, MeiliConfig, RoutingConfig,
    TransitionCostConfig,
};
pub use error::MeiliError;
pub use map_matcher::{MapMatcher, MapMatcherFactory, Workspace};
pub use match_result::{EdgeSegment, MatchResult, MatchResultType, MatchResults, StateId};
pub use measurement::{Measurement, MeasurementError};
