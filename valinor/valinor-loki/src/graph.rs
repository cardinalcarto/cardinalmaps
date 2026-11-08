//! Reading the graph as Loki does (the parts of mainline's `GraphReader` it uses).
//!
//! Mainline's reader returns a null tile for a tile that isn't in the tile set,
//! and Loki treats that as a dead end: it skips the bin, edge, or node.
//! [`Graph`] does the same with `Ok(None)`,
//! and keeps every other failure (I/O, corrupt tiles) as an error,
//! which the actions report as mainline reports its exceptions.

use crate::geo::PointLL;
use thiserror::Error;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{
    DirectedEdge, EdgeInfo, GraphTile, GraphTileDecodingError, LookupError, NodeInfo,
    SectionDecodingError,
};
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};
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

/// The handle type of a provider.
pub(crate) type Tile<P> = <P as GraphTileProvider>::TileHandle;

/// Graph access over a tile provider.
pub(crate) struct Graph<'p, P> {
    provider: &'p P,
}

impl<P> Clone for Graph<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for Graph<'_, P> {}

impl<'p, P: GraphTileProvider> Graph<'p, P> {
    #[inline]
    pub(crate) const fn new(provider: &'p P) -> Self {
        Self { provider }
    }

    #[inline]
    pub(crate) const fn provider(self) -> &'p P {
        self.provider
    }

    /// The tile containing `id`, or `None` if it isn't in the tile set (`GetGraphTile`).
    pub(crate) fn tile(self, id: GraphId) -> Result<Option<Tile<P>>, GraphError> {
        match self.provider.get_handle_for_tile_containing(id) {
            Ok(tile) => Ok(Some(tile)),
            Err(GraphTileProviderError::TileNotAvailable(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The tile containing `id`, reusing `hint` if it is that tile
    /// (`GetGraphTile(id, tile)`).
    pub(crate) fn tile_from(
        self,
        id: GraphId,
        hint: &Tile<P>,
    ) -> Result<Option<Tile<P>>, GraphError> {
        if hint.graph_id().tile_base_id() == id.tile_base_id() {
            Ok(Some(hint.clone()))
        } else {
            self.tile(id)
        }
    }

    /// The ID of the opposing edge and its tile, or `None` if that tile isn't in the tile set
    /// (`GetOpposingEdgeId`).
    pub(crate) fn opposing(
        self,
        edge_id: GraphId,
        hint: &Tile<P>,
    ) -> Result<Option<(GraphId, Tile<P>)>, GraphError> {
        let opp_id = match self.provider.get_opposing_edge_id(edge_id, hint) {
            Ok(id) => id,
            Err(GraphTileProviderError::TileNotAvailable(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(self.tile_from(opp_id, hint)?.map(|tile| (opp_id, tile)))
    }
}

/// The directed edge `id` in `tile`.
#[inline]
pub(crate) fn edge<T: GraphTile>(tile: &T, id: GraphId) -> Result<&DirectedEdge, GraphError> {
    tile.get_directed_edge(id)
        .map_err(|e| GraphError::Lookup(id, e))
}

/// The node `id` in `tile`.
#[inline]
pub(crate) fn node<T: GraphTile>(tile: &T, id: GraphId) -> Result<&NodeInfo, GraphError> {
    tile.get_node(id).map_err(|e| GraphError::Lookup(id, e))
}

/// A node's position, as mainline computes it (`NodeInfo::latlng`).
#[inline]
pub(crate) fn node_ll<T: GraphTile>(tile: &T, node: &NodeInfo) -> PointLL {
    PointLL::from(node.coordinate_f64(tile.header().sw_corner()))
}

/// An edge's shape, in the direction its edge info is stored (`EdgeInfo::shape`).
#[inline]
pub(crate) fn shape(info: &EdgeInfo<'_>) -> Result<Vec<PointLL>, GraphError> {
    Ok(info
        .decode_raw_shape::<f64>()?
        .into_iter()
        .map(PointLL::from)
        .collect())
}

/// The edge's opposing edge, given that the caller already has the end node's tile
/// (`GetOpposingEdge(edge, tile)`): the edge at the end node at the opposing index.
pub(crate) fn opposing_in_end_tile<T: GraphTile>(
    end_tile: &T,
    edge: &DirectedEdge,
) -> Result<GraphId, GraphError> {
    let end = edge.end_node_id();
    let node = node(end_tile, end)?;
    Ok(end
        .with_feature_index(u64::from(node.edge_index()) + u64::from(edge.opposing_edge_index()))
        .map_err(GraphTileProviderError::from)?)
}
