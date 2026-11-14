//! Loki's graph errors: reading the graph (with `valhalla_graphtile`'s
//! [`GraphReader`](valhalla_graphtile::graph_reader::GraphReader)) or costing it.

use thiserror::Error;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_reader::GraphReadError;
use valhalla_graphtile::graph_tile::{GraphTileDecodingError, LookupError, SectionDecodingError};
use valhalla_graphtile::tile_provider::GraphTileProviderError;
use valinor_sif::CostingError;

/// A failure while reading the graph or costing it.
///
/// None of these happen with a valid tile set;
/// they mean that a tile is corrupt, missing pieces, or couldn't be read.
#[derive(Debug, Error)]
pub enum GraphError {
    /// A tile couldn't be loaded.
    #[error("load a tile: {0}")]
    Tile(#[from] GraphTileProviderError),
    /// A graph ID points outside of its tile.
    #[error("find {0} in its tile: {1}")]
    Lookup(GraphId, LookupError),
    /// An edge's info is corrupt.
    #[error("read edge info: {0}")]
    EdgeInfo(#[from] GraphTileDecodingError),
    /// An edge's shape is corrupt.
    #[error("decode an edge shape: {0}")]
    Shape(#[from] std::io::Error),
    /// A variable-length section (names, levels, restrictions) is corrupt.
    #[error("read tile data: {0}")]
    Section(#[from] SectionDecodingError),
    /// Costing failed.
    #[error("cost an edge: {0}")]
    Costing(#[from] CostingError),
}

impl From<GraphReadError> for GraphError {
    #[inline]
    fn from(e: GraphReadError) -> Self {
        match e {
            GraphReadError::Tile(e) => Self::Tile(e),
            GraphReadError::Lookup(id, e) => Self::Lookup(id, e),
            GraphReadError::EdgeInfo(e) => Self::EdgeInfo(e),
            GraphReadError::Shape(e) => Self::Shape(e),
            GraphReadError::Section(e) => Self::Section(e),
        }
    }
}
