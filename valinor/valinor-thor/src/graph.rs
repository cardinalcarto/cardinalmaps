//! Graph lookups that path search makes (the parts of mainline's `GraphReader` it uses).
//!
//! Mainline's reader returns a null tile when a tile isn't in the tile set,
//! and the searches treat that as the end of the graph.
//! Here that is `Ok(None)` ([`GraphTileProviderError::TileNotAvailable`]);
//! every other failure (I/O, corrupt data) is an error.

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};

use crate::error::ThorError;
use crate::geo::PointLL;

/// A tile, or `None` if the tile set doesn't have it (mainline's null tile).
pub(crate) fn tile<P: GraphTileProvider>(
    provider: &P,
    id: GraphId,
) -> Result<Option<P::TileHandle>, ThorError> {
    match provider.get_handle_for_tile_containing(id) {
        Ok(tile) => Ok(Some(tile)),
        Err(GraphTileProviderError::TileNotAvailable(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The tile holding `id`: `hint` if it does, else a lookup (mainline's `GetGraphTile(id, tile)`).
pub(crate) fn tile_for<P: GraphTileProvider>(
    provider: &P,
    hint: &P::TileHandle,
    id: GraphId,
) -> Result<Option<P::TileHandle>, ThorError> {
    if hint.may_contain_id(id) {
        Ok(Some(hint.clone()))
    } else {
        tile(provider, id)
    }
}

/// A directed edge in its tile.
pub(crate) fn edge<T: GraphTile>(tile: &T, id: GraphId) -> Result<&DirectedEdge, ThorError> {
    tile.get_directed_edge(id)
        .map_err(|e| ThorError::Lookup(id, e))
}

/// A node in its tile.
pub(crate) fn node<T: GraphTile>(tile: &T, id: GraphId) -> Result<&NodeInfo, ThorError> {
    tile.get_node(id).map_err(|e| ThorError::Lookup(id, e))
}

/// A node's position, as mainline computes it (`NodeInfo::latlng`).
#[inline]
pub(crate) fn node_ll<T: GraphTile>(tile: &T, node: &NodeInfo) -> PointLL {
    let c = node.coordinate_f64(tile.header().sw_corner());
    PointLL::new(c.x, c.y)
}

/// The graph ID of the first edge leaving a node.
#[inline]
pub(crate) fn first_edge_id(node_id: GraphId, node: &NodeInfo) -> Result<GraphId, ThorError> {
    Ok(node_id
        .with_feature_index(u64::from(node.edge_index()))
        .map_err(GraphTileProviderError::from)?)
}

/// The edge's opposing edge, given the tile of its end node
/// (mainline's `GraphTile::GetOpposingEdgeId(edge)` on that tile).
pub(crate) fn opposing_in_end_tile<T: GraphTile>(
    end_tile: &T,
    edge: &DirectedEdge,
) -> Result<GraphId, ThorError> {
    let end = edge.end_node_id();
    let node = node(end_tile, end)?;
    Ok(end
        .with_feature_index(u64::from(node.edge_index()) + u64::from(edge.opposing_edge_index()))
        .map_err(GraphTileProviderError::from)?)
}

/// The tile of an edge's end node: `tile` (the edge's) unless the edge leaves it.
pub(crate) fn end_tile<P: GraphTileProvider>(
    provider: &P,
    tile: &P::TileHandle,
    edge: &DirectedEdge,
) -> Result<Option<P::TileHandle>, ThorError> {
    if edge.leaves_tile() {
        self::tile(provider, edge.end_node_id())
    } else {
        Ok(Some(tile.clone()))
    }
}

/// An edge's opposing edge and that edge's tile (mainline's `GetOpposingEdgeId(id, edge, tile)`),
/// or `None` if the end node's tile isn't in the tile set.
pub(crate) fn opposing<P: GraphTileProvider>(
    provider: &P,
    tile: &P::TileHandle,
    edge_id: GraphId,
) -> Result<Option<(GraphId, P::TileHandle)>, ThorError> {
    let edge = edge(tile, edge_id)?;
    let Some(end_tile) = end_tile(provider, tile, edge)? else {
        return Ok(None);
    };
    let opp = opposing_in_end_tile(&end_tile, edge)?;
    Ok(Some((opp, end_tile)))
}

/// The start node of an edge (mainline's `edge_startnode`: the opposing edge's end node).
pub(crate) fn start_node<P: GraphTileProvider>(
    provider: &P,
    tile: &P::TileHandle,
    edge_id: GraphId,
) -> Result<Option<GraphId>, ThorError> {
    let Some((opp, opp_tile)) = opposing(provider, tile, edge_id)? else {
        return Ok(None);
    };
    Ok(Some(self::edge(&opp_tile, opp)?.end_node_id()))
}

/// A node's time zone index, 0 for none.
#[inline]
fn tz_index(node: &NodeInfo) -> u32 {
    node.time_zone_index().map_or(0, |i| u32::from(i.get()))
}

/// The time zone of an edge (mainline's `GetTimezoneFromEdge`):
/// its start node's (even if that is 0), or its end node's if the start node can't be read.
pub(crate) fn timezone_from_edge<P: GraphTileProvider>(
    provider: &P,
    edge_id: GraphId,
) -> Result<u32, ThorError> {
    let Some(tile) = tile(provider, edge_id)? else {
        return Ok(0);
    };
    let edge = edge(&tile, edge_id)?;
    let end_id = edge.end_node_id();
    let end_tile = end_tile(provider, &tile, edge)?;
    // Mainline's `GetDirectedEdgeNodes` leaves the start node invalid without the end tile,
    // and `nodeinfo` returns null for invalid nodes and missing tiles.
    let start_id = match &end_tile {
        Some(t) => Some(self::edge(t, opposing_in_end_tile(t, edge)?)?.end_node_id()),
        None => None,
    };
    if let Some(start_id) = start_id
        && let Some(start_tile) = tile_for(provider, &tile, start_id)?
    {
        return Ok(tz_index(node(&start_tile, start_id)?));
    }
    match &end_tile {
        Some(t) => Ok(tz_index(node(t, end_id)?)),
        None => Ok(0),
    }
}

/// The time zone of a node (mainline's `GetTimezone`), 0 if it has none or its tile is missing.
pub(crate) fn timezone_of_node<P: GraphTileProvider>(
    provider: &P,
    node_id: GraphId,
) -> Result<u32, ThorError> {
    let Some(tile) = tile(provider, node_id)? else {
        return Ok(0);
    };
    Ok(tz_index(node(&tile, node_id)?))
}

/// Whether there is a transition from node `n1` to `n2` on another level.
fn is_transition<P: GraphTileProvider>(
    provider: &P,
    n1: GraphId,
    n2: GraphId,
) -> Result<bool, ThorError> {
    if n1.level() == n2.level() {
        return Ok(false);
    }
    let Some(tile) = tile(provider, n1)? else {
        return Ok(false);
    };
    let node = node(&tile, n1)?;
    Ok(tile
        .get_transitions(node)
        .iter()
        .any(|t| t.corresponding_end_node_id() == n2))
}

/// Whether two edges share a node, directly or through a transition
/// (mainline's `AreEdgesConnected`).
pub(crate) fn are_edges_connected<P: GraphTileProvider>(
    provider: &P,
    edge1: GraphId,
    edge2: GraphId,
) -> Result<bool, ThorError> {
    let Some(t1) = tile(provider, edge1)? else {
        return Ok(false);
    };
    let Some(t2) = tile_for(provider, &t1, edge2)? else {
        return Ok(false);
    };
    let de1 = edge(&t1, edge1)?;
    let de2 = edge(&t2, edge2)?;
    let (end1, end2) = (de1.end_node_id(), de2.end_node_id());
    if end1 == end2 || is_transition(provider, end1, end2)? {
        return Ok(true);
    }
    // Mainline's `GetOpposingEdge` returns null when the end node's tile is missing.
    let opp_end = |tile: &P::TileHandle, id: GraphId| -> Result<Option<GraphId>, ThorError> {
        Ok(match opposing(provider, tile, id)? {
            Some((opp, opp_tile)) => Some(edge(&opp_tile, opp)?.end_node_id()),
            None => None,
        })
    };
    let de1_opp_end = opp_end(&t1, edge1)?;
    if let Some(start1) = de1_opp_end
        && (start1 == end2 || is_transition(provider, start1, end2)?)
    {
        return Ok(true);
    }
    let (Some(start1), Some(start2)) = (de1_opp_end, opp_end(&t2, edge2)?) else {
        return Ok(false);
    };
    Ok(start2 == end1
        || start2 == start1
        || is_transition(provider, start2, end1)?
        || is_transition(provider, start2, start1)?)
}

/// Whether two edges are the same enough that one continues the other in a shortcut
/// (the comparison in mainline's `recover_shortcut`).
fn continues_shortcut(edge: &DirectedEdge, shortcut: &DirectedEdge) -> bool {
    !edge.is_shortcut()
        && edge.forward_access() == shortcut.forward_access()
        && edge.reverse_access() == shortcut.reverse_access()
        && edge.has_sign() == shortcut.has_sign()
        && edge.road_use().ok() == shortcut.road_use().ok()
        && edge.classification() == shortcut.classification()
        && edge.roundabout() == shortcut.roundabout()
        && edge.is_link() == shortcut.is_link()
        && edge.toll() == shortcut.toll()
        && edge.dest_only() == shortcut.dest_only()
        && edge.dest_only_hgv() == shortcut.dest_only_hgv()
        && edge.is_unpaved() == shortcut.is_unpaved()
        && edge.surface() == shortcut.surface()
        && edge.road_use().ok() != Some(valhalla_graphtile::RoadUse::Construction)
}

/// A shortcut's bit in the masks of its superseded edges (mainline's `shortcut()`).
#[inline]
pub(crate) fn shortcut_mask(edge: &DirectedEdge) -> u32 {
    edge.shortcut_index().map_or(0, |i| 1 << i)
}

/// The bit of the shortcut that supersedes an edge (mainline's `superseded()`).
#[inline]
pub(crate) fn superseded_mask(edge: &DirectedEdge) -> u32 {
    edge.superseded_index().map_or(0, |i| 1 << i)
}

/// The edges a shortcut stands for (mainline's `RecoverShortcut`, without its cache),
/// or the shortcut itself if they can't be recovered.
///
/// The result goes to `out`, which is cleared first.
pub(crate) fn recover_shortcut<P: GraphTileProvider>(
    provider: &P,
    shortcut_id: GraphId,
    out: &mut Vec<GraphId>,
) -> Result<(), ThorError> {
    out.clear();
    let failed = |out: &mut Vec<GraphId>| {
        out.clear();
        out.push(shortcut_id);
        Ok(())
    };
    let Some(shortcut_tile) = tile(provider, shortcut_id)? else {
        return failed(out);
    };
    let shortcut = edge(&shortcut_tile, shortcut_id)?.clone();
    if !shortcut.is_shortcut() {
        return failed(out);
    }
    let Some(mut begin_node) = start_node(provider, &shortcut_tile, shortcut_id)? else {
        return failed(out);
    };
    // The start node is in the shortcut's tile, as every edge is in its start node's.
    let mut tile = shortcut_tile.clone();
    let node0 = node(&tile, begin_node)?;
    let first = first_edge_id(begin_node, node0)?;
    let mask = shortcut_mask(&shortcut);
    let Some(position) = tile
        .get_outbound_edges_from_node(node0)
        .iter()
        .position(|de| mask & superseded_mask(de) != 0)
    else {
        return failed(out);
    };
    let first_id = first
        .with_feature_index(first.feature_index() + position as u64)
        .map_err(GraphTileProviderError::from)?;
    out.push(first_id);

    let mut current = edge(&tile, first_id)?.clone();
    let mut accumulated_length = current.length();
    let mut edge_count: u32 = 1;
    let mut roundoff_error: u32 = 1;
    while current.end_node_id() != shortcut.end_node_id() {
        // `GetEndNode(current_edge, tile)`, which moves `tile` to the end node's.
        let end_id = current.end_node_id();
        let Some(end) = tile_for(provider, &tile, end_id)? else {
            return failed(out);
        };
        tile = end;
        let end_node = node(&tile, end_id)?;
        let first = first_edge_id(end_id, end_node)?;
        let mut next = None;
        for (i, de) in tile
            .get_outbound_edges_from_node(end_node)
            .iter()
            .enumerate()
        {
            if begin_node != de.end_node_id() && continues_shortcut(de, &shortcut) {
                next = Some((
                    first
                        .with_feature_index(first.feature_index() + i as u64)
                        .map_err(GraphTileProviderError::from)?,
                    de.clone(),
                ));
                break;
            }
        }
        edge_count += 1;
        // `std::round(edge_count * 0.5) + 1`, rounding halves away from zero.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a small, positive count"
        )]
        let rounded = (f64::from(edge_count) * 0.5).round() as u32;
        roundoff_error = rounded + 1;
        let Some((next_id, next_edge)) = next else {
            return failed(out);
        };
        out.push(next_id);
        accumulated_length += next_edge.length();
        current = next_edge;
        begin_node = end_id;
        if accumulated_length > shortcut.length() + roundoff_error {
            return failed(out);
        }
    }
    // `accumulated_length < shortcut->length() - roundoff_error`, in unsigned arithmetic.
    if accumulated_length < shortcut.length().wrapping_sub(roundoff_error) {
        return failed(out);
    }
    Ok(())
}
