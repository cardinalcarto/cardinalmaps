//! Thor's internal errors, and how they reach the client.
//!
//! Path search fails in two ways a client sees:
//! no path (mainline's error 442, a [`ValhallaError`] returned directly),
//! and everything mainline would report as an exception (error 499, "Unknown",
//! with the exception's text): unreadable or corrupt tiles, costing failures,
//! and inconsistent input from Loki.
//! [`ThorError`] is the second kind.

use thiserror::Error;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{LookupError, SectionDecodingError};
use valhalla_graphtile::tile_provider::GraphTileProviderError;
use valhalla_response::{ErrorCode, ValhallaError};
use valinor_sif::{CostingError, RecostError};

/// A failure inside path search (mainline's exceptions, error 499).
#[derive(Debug, Error)]
pub enum ThorError {
    /// A tile couldn't be read.
    #[error("read a tile: {0}")]
    Tile(#[from] GraphTileProviderError),
    /// A graph ID from the tiles or the request doesn't exist in its tile.
    #[error("look up {0}: {1}")]
    Lookup(GraphId, LookupError),
    /// A tile section (complex restrictions) is corrupt.
    #[error("read tile data: {0}")]
    Section(#[from] SectionDecodingError),
    /// Costing failed.
    #[error("cost an edge: {0}")]
    Costing(#[from] CostingError),
    /// Recosting a path failed.
    #[error("{0}")]
    Recost(#[from] RecostError),
    /// A path uses an edge that isn't among its location's candidates
    /// (mainline's "Could not find candidate edge used for origin label").
    #[error("find the candidate edge {0} used for a location")]
    MissingCandidate(GraphId),
    /// An edge's flags say a complex restriction ends on it, but its tile has none
    /// (mainline's "Found no restrictions in tile even though edge-label.on_complex_rest() == true").
    #[error("find the complex restrictions ending on edge {0}: there are none")]
    MissingRestriction(GraphId),
    /// A label index points nowhere (mainline would read out of bounds).
    #[error("read label {0}: it does not exist")]
    DanglingLabel(u32),
    /// A tile the search used is gone (mainline's `tile_gone_error_t`).
    #[error("form a path: the tile of edge {0} is gone")]
    TileGone(GraphId),
    /// A location has no candidate edges where the search needs one.
    #[error("read the candidate edges of location {0}: there are none")]
    NoCandidates(usize),
    /// A graph ID in the request isn't valid.
    #[error("read graph ID {0}: it is not valid")]
    InvalidGraphId(u64),
    /// The request has no options, or its costing can't be built.
    #[error("build the costing: {0}")]
    Options(String),
}

impl From<ThorError> for ValhallaError {
    fn from(error: ThorError) -> Self {
        ValhallaError::with_detail(ErrorCode::ThorUnknown, error.to_string())
    }
}
