//! Meili's errors, and how mainline's Thor reports each.

use thiserror::Error;
use valhalla_graphtile::graph_reader::GraphReadError;
use valinor_sif::CostingError;

use crate::measurement::MeasurementError;

/// A failure to build a matcher or to match a trace.
///
/// Mainline's Thor reports these as follows (`thor_worker_t::trace_route`, `trace_attributes`,
/// and `parse_measurements`):
///
/// - [`NoCandidates`](Self::NoCandidates) is error 443 (`valhalla_exception_t{443}`).
/// - [`Measurement`](Self::Measurement) is error 424 ("Failed to parse shape").
/// - [`TurnPenaltyFactor`](Self::TurnPenaltyFactor) and [`Costing`](Self::Costing) happen
///   when the matcher is made, before matching: error 499 with mainline's exception message.
/// - [`Interrupted`](Self::Interrupted) means the client went away; nothing is sent.
/// - Everything else happens inside `OfflineMatch`, which Thor reports as error 442
///   ("No path could be found for input").
#[derive(Debug, Error)]
pub enum MeiliError {
    /// Fewer than two measurements, or no measurement with candidate edges
    /// (mainline's `HasMinimumCandidates` check).
    #[error("match the trace: no measurement has candidate edges")]
    NoCandidates,
    /// A shape point makes a measurement mainline rejects.
    #[error(transparent)]
    Measurement(#[from] MeasurementError),
    /// The request's `turn_penalty_factor` is negative
    /// (mainline: "Expect turn penalty factor to be nonnegative").
    #[error("make the matcher: expected a nonnegative turn penalty factor, got {0}")]
    TurnPenaltyFactor(f32),
    /// The request's costing can't be built.
    #[error("make the matcher: {0}")]
    Costing(#[from] CostingError),
    /// `k` was 0 (mainline: "expect k to be positive but got 0").
    #[error("match the trace: expected k to be positive")]
    ZeroK,
    /// The interrupt callback asked to stop.
    #[error("match the trace: interrupted")]
    Interrupted,
    /// Costing couldn't cost an edge while matching (its tile has no speed profile for it,
    /// or a section costing reads is corrupt).
    #[error("match the trace: cost an edge: {0}")]
    EdgeCost(CostingError),
    /// A tile couldn't be read, or is corrupt.
    #[error("match the trace: {0}")]
    Graph(#[from] GraphReadError),
    /// An internal consistency check failed, where mainline throws a `std::logic_error`
    /// or `std::runtime_error` (some are reachable: `cut_segments` fails on some traces
    /// that loop back over an edge, "unexpectedly unable to locate target edge").
    #[error("match the trace: {0}")]
    Logic(&'static str),
}

impl From<valhalla_graphtile::tile_provider::GraphTileProviderError> for MeiliError {
    #[inline]
    fn from(e: valhalla_graphtile::tile_provider::GraphTileProviderError) -> Self {
        Self::Graph(GraphReadError::Tile(e))
    }
}

impl From<valhalla_graphtile::graph_tile::GraphTileDecodingError> for MeiliError {
    #[inline]
    fn from(e: valhalla_graphtile::graph_tile::GraphTileDecodingError) -> Self {
        Self::Graph(GraphReadError::EdgeInfo(e))
    }
}
