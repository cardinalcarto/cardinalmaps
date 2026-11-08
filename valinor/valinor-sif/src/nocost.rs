//! The costing that costs nothing (mainline's `NoCost` in `src/sif/nocost.cc`).
//!
//! Loki uses it for `locate` requests without a costing (and for linear cost features):
//! every edge but shortcuts and transit lines is allowed, every node may be passed,
//! an edge costs its length, and transitions are free.

use enumset::EnumSet;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::Costing as CostingProto;

use crate::Costing;
use crate::cost::{Cost, InternalTurn, TravelMode};
use crate::costing::{CostingError, EdgeCostResult, partial_from_full};
use crate::dynamic::{BaseSetup, DisallowMask, DynamicCost, EdgeAccess, TimeContext};
use crate::label::Label;

/// The costing for `"costing": "none"` (mainline's `NoCost`).
#[derive(Clone, Debug)]
pub struct NoCost {
    base: DynamicCost,
}

impl NoCost {
    /// The costing from a request's options (mainline's `CreateNoCost`):
    /// driving, with every access mode allowed.
    pub fn new(costing: &CostingProto) -> Self {
        Self {
            base: DynamicCost::new(
                costing,
                BaseSetup {
                    mode: TravelMode::Drive,
                    access_mask: EnumSet::all(),
                    penalize_uturns: false,
                },
            ),
        }
    }
}

impl Costing for NoCost {
    #[inline]
    fn base(&self) -> &DynamicCost {
        &self.base
    }

    #[inline]
    fn base_mut(&mut self) -> &mut DynamicCost {
        &mut self.base
    }

    #[inline]
    fn allowed<T: GraphTile, L: Label>(
        &self,
        edge: &DirectedEdge,
        _edge_id: GraphId,
        _tile: &T,
        _pred: &L,
        _time: TimeContext,
        _access: &mut EdgeAccess,
    ) -> bool {
        !edge.is_shortcut()
    }

    #[inline]
    fn allowed_reverse<T: GraphTile, L: Label>(
        &self,
        _edge: &DirectedEdge,
        _pred: &L,
        opp_edge: &DirectedEdge,
        _opp_edge_id: GraphId,
        _tile: &T,
        _time: TimeContext,
        _access: &mut EdgeAccess,
    ) -> bool {
        !opp_edge.is_shortcut()
    }

    #[inline]
    fn edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        _edge_id: GraphId,
        _tile: &T,
        _seconds_of_week: Option<u32>,
    ) -> Result<EdgeCostResult, CostingError> {
        #[expect(clippy::cast_precision_loss, reason = "edge lengths fit in 24 bits")]
        let length = edge.length() as f32;
        Ok(EdgeCostResult {
            cost: Cost::new(length, length),
            flow_sources: EnumSet::empty(),
        })
    }

    #[inline]
    fn transition_cost<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        _edge: &DirectedEdge,
        _tile: &T,
        _node: &NodeInfo,
        _pred: &L,
        _provider: &P,
        _edge_destonly: bool,
    ) -> Result<Cost, CostingError> {
        Ok(Cost::ZERO)
    }

    #[inline]
    fn transition_cost_reverse<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        _idx: u8,
        _node: &NodeInfo,
        _opp_edge: &DirectedEdge,
        _opp_pred_edge: &DirectedEdge,
        _pred: &L,
        _tile: &T,
        _provider: &P,
        _has_measured_speed: bool,
        _internal_turn: InternalTurn,
        _opp_edge_destonly: bool,
    ) -> Result<Cost, CostingError> {
        Ok(Cost::ZERO)
    }

    /// Mainline's `NoCost` ignores the mask: only shortcuts and transit lines are ruled out.
    #[inline]
    fn edge_filter<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        _tile: &T,
        _disallow: DisallowMask,
    ) -> bool {
        !(edge.is_shortcut() || edge.is_transit_line())
    }

    #[inline]
    fn node_allowed(&self, _node: &NodeInfo) -> bool {
        true
    }

    #[inline]
    fn astar_cost_factor(&self) -> f32 {
        1.0
    }

    #[inline]
    fn partial_edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        factor_edge: Option<GraphId>,
        tile: &T,
        seconds_of_week: Option<u32>,
        start: f32,
        end: f32,
    ) -> Result<EdgeCostResult, CostingError> {
        let full = self.edge_cost(edge, edge_id, tile, seconds_of_week)?;
        Ok(partial_from_full(&self.base, full, factor_edge, start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::{EdgeLabel, LabelState};
    use std::path::PathBuf;
    use valhalla_graphtile::tile_provider::DirectoryGraphTileProvider;

    #[test]
    fn allows_everything_but_shortcuts() {
        let provider = DirectoryGraphTileProvider::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../valhalla-graphtile/fixtures/andorra-tiles"),
            core::num::NonZeroUsize::new(4).unwrap(),
        );
        let tile = provider
            .get_handle_for_tile_containing(GraphId::try_from_components(0, 3015, 0).unwrap())
            .unwrap();
        let costing = NoCost::new(&CostingProto::default());
        let mut shortcuts = 0;
        for (i, edge) in tile.directed_edges().iter().enumerate() {
            let id = tile.graph_id().with_feature_index(i as u64).unwrap();
            let pred = EdgeLabel::new(id, edge, LabelState::default());
            let mut access = EdgeAccess::default();
            let allowed = costing.allowed(edge, id, &tile, &pred, TimeContext::NONE, &mut access);
            assert_eq!(allowed, !edge.is_shortcut());
            assert_eq!(
                costing.edge_filter(edge, &tile, DisallowMask::NONE),
                !edge.is_shortcut()
            );
            #[expect(clippy::cast_precision_loss, reason = "test values are small")]
            let length = edge.length() as f32;
            let cost = costing.edge_cost(edge, id, &tile, None).unwrap().cost;
            assert_eq!(cost, Cost::new(length, length));
            shortcuts += usize::from(edge.is_shortcut());
        }
        assert!(shortcuts > 0, "the level 0 tile has shortcuts");
        assert_eq!(costing.base().travel_mode(), TravelMode::Drive);
    }
}
