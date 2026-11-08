//! Costing a known path from the start (mainline's `sif::recost_forward`).
//!
//! Thor uses this for the final paths of bidirectional searches
//! (whose two halves were costed in opposite directions),
//! and for `recostings` in requests.

use thiserror::Error;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};

use crate::cost::{Cost, InternalTurn};
use crate::costing::{Costing, CostingError};
use crate::dynamic::{DisallowMask, EdgeAccess, TimeContext};
use crate::label::{EdgeLabel, Label, LabelState};
use crate::time_info::TimeInfo;

/// An error recosting a path.
#[derive(Debug, Error)]
pub enum RecostError {
    /// The source or target percent along is outside 0 to 1.
    #[error("recost a path: source and target percentages must be between 0 and 1 inclusive")]
    InvalidPercentAlong,
    /// An edge of the path isn't in the tile set.
    #[error("recost a path: edge {0} cannot be found")]
    EdgeNotFound(GraphId),
    /// A node of the path isn't in the tile set.
    #[error("recost a path: node {0} cannot be found")]
    NodeNotFound(GraphId),
    /// The costing doesn't allow an edge of the path.
    #[error("recost a path: edge {0} requires different access than this costing allows")]
    EdgeAccess(GraphId),
    /// The costing doesn't allow a node of the path.
    #[error("recost a path: node {0} requires different access than this costing allows")]
    NodeAccess(GraphId),
    /// A tile couldn't be read.
    #[error("recost a path: {0}")]
    Tile(#[from] GraphTileProviderError),
    /// Costing failed.
    #[error("recost a path: {0}")]
    Costing(#[from] CostingError),
}

/// One edge of a recosted path (the fields of mainline's `PathEdgeLabel` it emits).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecostedEdge {
    /// The edge.
    pub edge_id: GraphId,
    /// The cost from the start of the path to the end of this edge.
    pub cost: Cost,
    /// The cost of the transition onto this edge (zero for the first).
    pub transition_cost: Cost,
    /// Meters from the start of the path to the end of this edge, rounded.
    pub path_distance: u32,
    /// The timed restriction seen on this edge, if any
    /// (see [`EdgeAccess::restriction_idx`]).
    pub restriction_idx: u8,
    /// The label for this edge, as the next edge's predecessor.
    pub label: EdgeLabel,
}

/// The edge and its tile.
fn edge_in<P: GraphTileProvider>(
    provider: &P,
    edge_id: GraphId,
) -> Result<(P::TileHandle, DirectedEdge), RecostError> {
    let tile = provider.get_handle_for_tile_containing(edge_id)?;
    let edge = tile
        .get_directed_edge(edge_id)
        .map_err(|_| RecostError::EdgeNotFound(edge_id))?
        .clone();
    Ok((tile, edge))
}

/// Costs a path from its first edge (mainline's `recost_forward`), calling `on_edge`
/// with each edge's cost.
///
/// `source_pct` and `target_pct` are how far along the first and last edges
/// the path starts and ends.
/// `time_info` is the time at the start, and `invariant` keeps it from moving
/// as the path goes on (mainline's `date_time` type `invariant`).
/// With `ignore_access`, access checks don't fail the recost
/// (but still find destination-only edges and timed restrictions).
///
/// As in mainline, the user's cost factors along edges aren't applied,
/// and complex restrictions aren't checked.
///
/// # Errors
///
/// See [`RecostError`]. Access failures only happen without `ignore_access`.
///
/// # Performance
///
/// This costs each edge once; it gets a tile handle per edge,
/// which is cheap for the memory-mapped tile providers.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "follows mainline's signature and function step by step"
)]
pub fn recost_forward<C: Costing, P: GraphTileProvider>(
    provider: &P,
    costing: &C,
    edges: impl IntoIterator<Item = GraphId>,
    source_pct: f32,
    target_pct: f32,
    time_info: TimeInfo,
    invariant: bool,
    ignore_access: bool,
    mut on_edge: impl FnMut(&RecostedEdge),
) -> Result<(), RecostError> {
    if !(0.0..=1.0).contains(&source_pct) || !(0.0..=1.0).contains(&target_pct) {
        return Err(RecostError::InvalidPercentAlong);
    }
    let mut edges = edges.into_iter().peekable();
    let Some(&first_id) = edges.peek() else {
        return Ok(());
    };
    {
        let (tile, edge) = edge_in(provider, first_id)?;
        if !ignore_access && !costing.edge_filter(&edge, &tile, DisallowMask::NONE) {
            return Err(RecostError::EdgeAccess(first_id));
        }
    }

    let base = costing.base();
    let mode = base.travel_mode();
    let mut source_pct = Some(source_pct);
    let mut previous: Option<(DirectedEdge, EdgeLabel)> = None;
    let mut cost = Cost::ZERO;
    let mut length: f64 = 0.0;
    let mut predecessor: Option<u32> = None;

    while let Some(edge_id) = edges.next() {
        // The node at the end of the previous edge.
        let node: Option<(P::TileHandle, NodeInfo)> = match &previous {
            Some((prev_edge, _)) => {
                let node_id = prev_edge.end_node_id();
                let tile = provider.get_handle_for_tile_containing(node_id)?;
                let node = tile
                    .get_node(node_id)
                    .map_err(|_| RecostError::NodeNotFound(node_id))?
                    .clone();
                Some((tile, node))
            }
            None => None,
        };
        let (tile, edge) = edge_in(provider, edge_id)?;

        // Re-derive U-turns, which mainline marks as dead ends.
        // (Mainline starts from a default label, which nothing reads at the first edge.)
        let mut label = previous.as_ref().map_or_else(
            || EdgeLabel::new(edge_id, &edge, LabelState::default()),
            |(_, label)| *label,
        );
        label.set_deadend(label.opp_local_idx() == edge.local_edge_index());

        if let Some((_, node_info)) = &node
            && !ignore_access
            && !label.deadend()
            && !costing.node_allowed(node_info)
        {
            let node_id = previous.as_ref().map_or(edge_id, |(e, _)| e.end_node_id());
            return Err(RecostError::NodeAccess(node_id));
        }

        let seconds_offset = if invariant { 0.0 } else { cost.secs };
        let offset_time = match &node {
            Some((_, node_info)) => time_info.forward(
                seconds_offset,
                node_info
                    .time_zone_index()
                    .map_or(0, |tz| u32::from(tz.get())),
            ),
            None => time_info,
        };

        let mut access = EdgeAccess::new(0);
        let is_last = edges.peek().is_none();
        // Allowed runs even when ignoring access, for destination-only edges and timed restrictions.
        if predecessor.is_some() {
            let allowed = costing.allowed(
                &edge,
                edge_id,
                &tile,
                &label,
                TimeContext::new(offset_time.current_time(), offset_time.timezone_index),
                &mut access,
            );
            if !allowed && !ignore_access {
                return Err(RecostError::EdgeAccess(edge_id));
            }
        }

        // How much of the edge is used: trimmed at the start and end of the path.
        let mut edge_pct: f32 = 1.0;
        let mut start: f32 = 0.0;
        let mut end: f32 = 1.0;
        if let Some(pct) = source_pct.take() {
            edge_pct -= pct;
            start = pct;
        }
        if is_last {
            edge_pct -= 1.0 - target_pct;
            edge_pct = edge_pct.max(0.0);
            end = target_pct;
        }

        let transition_cost = match &node {
            Some((_, node_info)) => costing.transition_cost(
                &edge,
                &tile,
                node_info,
                &label,
                provider,
                access.edge_destonly,
            )?,
            None => Cost::ZERO,
        };
        let partial = costing.partial_edge_cost(
            &edge,
            edge_id,
            None,
            &tile,
            offset_time.seconds_of_week(),
            start,
            end,
        )?;
        cost += transition_cost + partial.cost;
        #[expect(clippy::cast_precision_loss, reason = "edge lengths fit in 24 bits")]
        let used = edge.length() as f32 * edge_pct;
        length += f64::from(used);

        let turn = match &node {
            Some((_, node_info)) => base.turn_type(label.opp_local_idx(), node_info, &edge, None),
            None => InternalTurn::NoTurn,
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a rounded, non-negative length"
        )]
        let path_distance = length.round() as u32;
        let new_label = EdgeLabel::new(
            edge_id,
            &edge,
            LabelState {
                predecessor,
                mode,
                path_distance,
                closure_pruning: !ignore_access,
                has_measured_speed: !partial.flow_sources.is_empty(),
                internal_turn: turn,
                destonly: access.edge_destonly,
            },
        );
        on_edge(&RecostedEdge {
            edge_id,
            cost,
            transition_cost,
            path_distance,
            restriction_idx: access.restriction_idx,
            label: new_label,
        });
        // Mainline's predecessor counter starts at its invalid value and wraps to 0.
        predecessor = Some(predecessor.map_or(0, |p| p + 1));
        previous = Some((edge, new_label));
    }
    Ok(())
}
