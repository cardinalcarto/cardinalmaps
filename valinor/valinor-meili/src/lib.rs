//! Valhalla-compatible map matching: mainline's Meili.
//!
//! See `docs/routing.md` for where this sits:
//! Meili is a library that Thor calls for `trace_route` and `trace_attributes`
//! (with `shape_match` `map_snap`, or `walk_or_snap` when the edge walk fails),
//! as Sif is a library that Loki and Thor call.
//!
//! The types in [`match_result`] are what Meili hands Thor (and, through Thor, Tyr's
//! `trace_attributes` serializer), shaped like mainline's `meili/match_result.h`.

pub mod match_result;

pub use match_result::{EdgeSegment, MatchResult, MatchResultType, MatchResults, StateId};
