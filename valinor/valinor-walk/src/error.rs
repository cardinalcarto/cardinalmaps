//! Errors from street search and stop snapping.

use thiserror::Error;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{BuildId, LookupError};
use valhalla_graphtile::tile_provider::GraphTileProviderError;
use valinor_sif::CostingError;

/// Why a street search or a stop snap failed.
#[derive(Debug, Error)]
pub enum WalkError {
    /// A tile couldn't be read (a tile that is simply absent is a dead end, not an error).
    #[error("read a tile: {0}")]
    Tile(#[from] GraphTileProviderError),
    /// An edge or node of a tile couldn't be found where its graph ID says.
    #[error("find {0} in its tile: {1}")]
    Lookup(GraphId, LookupError),
    /// Costing an edge failed.
    #[error("cost an edge: {0}")]
    Costing(#[from] CostingError),
    /// Stops were snapped on different tiles from the ones this search reads.
    #[error("use stop snaps of tile build {snaps:?} with tiles of build {tiles:?}")]
    BuildMismatch {
        /// The build the stops were snapped on.
        snaps: BuildId,
        /// The build of the tiles in use.
        tiles: BuildId,
    },
    /// Loki couldn't correlate a location.
    #[error("correlate a location: {0}")]
    Loki(#[from] valinor_loki::SearchError),
    /// A stop snap names an edge the tiles don't have.
    #[error("find the snapped edge {0} in the tiles")]
    SnappedEdgeMissing(GraphId),
    /// A location wasn't correlated to any edge.
    #[error("correlate a location to any edge")]
    NoCandidates,
    /// A thread building a transfer table panicked.
    #[error("finish a transfer table: a worker thread panicked")]
    WorkerPanicked,
    /// A walk handed to a [`TransferTable`](crate::TransferTable) breaks its invariants.
    #[error("load walk from {from} to {to} ({seconds} s): out of order or out of range")]
    InvalidWalk {
        /// The stop the walk starts at.
        from: u32,
        /// The stop the walk ends at.
        to: u32,
        /// Its seconds.
        seconds: u32,
    },
    /// The graph grew past what its 32-bit dense indexes can address.
    #[error("index more than 2^31 nodes or edges")]
    TooLarge,
}

#[cfg(test)]
impl WalkError {
    /// Whether the error is an unavailable tile (for tests of partial tile sets).
    #[allow(dead_code)]
    pub(crate) fn is_not_available(&self) -> bool {
        matches!(self, Self::Tile(e) if e.is_not_available())
    }
}
