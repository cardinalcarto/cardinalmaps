//! Reading the graph as Loki and Meili do (the parts of mainline's `GraphReader` they use).
//!
//! Mainline's reader returns a null tile for a tile that isn't in the tile set,
//! and its callers treat that as a dead end: they skip the bin, edge, or node.
//! [`GraphReader`] does the same with `Ok(None)`,
//! and keeps every other failure (I/O, corrupt tiles) as an error,
//! which callers report as mainline reports its exceptions.

use crate::GraphId;
use crate::graph_tile::{
    DirectedEdge, EdgeInfo, GraphTile, GraphTileDecodingError, LookupError, NodeInfo,
    SectionDecodingError,
};
use crate::midgard::PointLL;
use crate::tile_provider::{GraphTileProvider, GraphTileProviderError};
use thiserror::Error;

/// A failure while reading the graph.
///
/// None of these happen with a valid tile set;
/// they mean that a tile is corrupt, missing pieces, or couldn't be read.
#[derive(Debug, Error)]
pub enum GraphReadError {
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
}

/// The handle type of a provider.
pub type Tile<P> = <P as GraphTileProvider>::TileHandle;

/// Graph access over a tile provider (mainline's `GraphReader`).
pub struct GraphReader<'p, P> {
    provider: &'p P,
}

impl<P> Clone for GraphReader<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for GraphReader<'_, P> {}

impl<'p, P: GraphTileProvider> GraphReader<'p, P> {
    /// Graph access over `provider`.
    #[inline]
    pub const fn new(provider: &'p P) -> Self {
        Self { provider }
    }

    /// The tile provider.
    #[inline]
    pub const fn provider(self) -> &'p P {
        self.provider
    }

    /// The tile containing `id`, or `None` if it isn't in the tile set (`GetGraphTile`).
    ///
    /// # Errors
    ///
    /// Fails if a tile can't be read or the graph data is corrupt.
    pub fn tile(self, id: GraphId) -> Result<Option<Tile<P>>, GraphReadError> {
        match self.provider.get_handle_for_tile_containing(id) {
            Ok(tile) => Ok(Some(tile)),
            Err(GraphTileProviderError::TileNotAvailable(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The tile containing `id`, reusing `hint` if it is that tile
    /// (`GetGraphTile(id, tile)`).
    ///
    /// # Errors
    ///
    /// Fails if a tile can't be read or the graph data is corrupt.
    pub fn tile_from(self, id: GraphId, hint: &Tile<P>) -> Result<Option<Tile<P>>, GraphReadError> {
        if hint.graph_id().tile_base_id() == id.tile_base_id() {
            Ok(Some(hint.clone()))
        } else {
            self.tile(id)
        }
    }

    /// The ID of the opposing edge and its tile, or `None` if that tile isn't in the tile set
    /// (`GetOpposingEdgeId`).
    ///
    /// # Errors
    ///
    /// Fails if a tile can't be read or the graph data is corrupt.
    pub fn opposing(
        self,
        edge_id: GraphId,
        hint: &Tile<P>,
    ) -> Result<Option<(GraphId, Tile<P>)>, GraphReadError> {
        let opp_id = match self.provider.get_opposing_edge_id(edge_id, hint) {
            Ok(id) => id,
            Err(GraphTileProviderError::TileNotAvailable(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(self.tile_from(opp_id, hint)?.map(|tile| (opp_id, tile)))
    }
}

/// The directed edge `id` in `tile`.
///
/// # Errors
///
/// Fails if a tile can't be read or the graph data is corrupt.
#[inline]
pub fn edge<T: GraphTile>(tile: &T, id: GraphId) -> Result<&DirectedEdge, GraphReadError> {
    tile.get_directed_edge(id)
        .map_err(|e| GraphReadError::Lookup(id, e))
}

/// The node `id` in `tile`.
///
/// # Errors
///
/// Fails if a tile can't be read or the graph data is corrupt.
#[inline]
pub fn node<T: GraphTile>(tile: &T, id: GraphId) -> Result<&NodeInfo, GraphReadError> {
    tile.get_node(id).map_err(|e| GraphReadError::Lookup(id, e))
}

/// A node's position, as mainline computes it (`NodeInfo::latlng`).
#[inline]
pub fn node_ll<T: GraphTile>(tile: &T, node: &NodeInfo) -> PointLL {
    PointLL::from(node.coordinate_f64(tile.header().sw_corner()))
}

/// An edge's shape, in the direction its edge info is stored (`EdgeInfo::shape`).
///
/// # Errors
///
/// Fails if a tile can't be read or the graph data is corrupt.
#[inline]
pub fn shape(info: &EdgeInfo<'_>) -> Result<Vec<PointLL>, GraphReadError> {
    Ok(info
        .decode_raw_shape::<f64>()?
        .into_iter()
        .map(PointLL::from)
        .collect())
}

/// The edge's opposing edge, given that the caller already has the end node's tile
/// (`GetOpposingEdge(edge, tile)`): the edge at the end node at the opposing index.
///
/// # Errors
///
/// Fails if a tile can't be read or the graph data is corrupt.
pub fn opposing_in_end_tile<T: GraphTile>(
    end_tile: &T,
    edge: &DirectedEdge,
) -> Result<GraphId, GraphReadError> {
    let end = edge.end_node_id();
    let node = node(end_tile, end)?;
    Ok(end
        .with_feature_index(u64::from(node.edge_index()) + u64::from(edge.opposing_edge_index()))
        .map_err(GraphTileProviderError::from)?)
}
