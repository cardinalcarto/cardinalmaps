//! The costing interface Loki and Thor call (mainline's virtual `DynamicCost` methods),
//! and building a costing from a request's options (mainline's `CostFactory`).

use enumset::EnumSet;
use thiserror::Error;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::predicted_speeds::{FlowSource, select_speed};
use valhalla_graphtile::graph_tile::{
    DirectedEdge, GraphTile, GraphTileDecodingError, NodeInfo, SectionDecodingError,
};
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};
use valhalla_proto::Costing as CostingProto;
use valhalla_proto::costing::Type as CostingType;

use crate::auto::AutoCost;
use crate::bicycle::BicycleCost;
use crate::cost::{Cost, InternalTurn};
use crate::dynamic::{DisallowMask, DynamicCost, EdgeAccess, TimeContext};
use crate::label::Label;
use crate::nocost::NoCost;
use crate::pedestrian::PedestrianCost;

/// An error from a costing.
#[derive(Debug, Error)]
pub enum CostingError {
    /// The costing exists in mainline but isn't implemented here yet.
    #[error("build {0:?} costing: not implemented")]
    NotImplemented(CostingType),
    /// The request's costing type isn't one mainline defines.
    #[error("build costing: unknown costing type {0}")]
    UnknownCosting(i32),
    /// An edge has a predicted speed flag, but its tile has no usable profile for it.
    #[error("read the predicted speed of edge {0}: the tile has no profile for it")]
    MissingSpeedProfile(GraphId),
    /// A tile section costing reads (such as levels) is corrupt.
    #[error("read tile data: {0}")]
    Section(#[from] SectionDecodingError),
    /// An edge's info (which pedestrian costing reads at elevators) is corrupt.
    #[error("read edge info: {0}")]
    EdgeInfo(#[from] GraphTileDecodingError),
    /// A tile costing needs couldn't be read.
    #[error("read a tile: {0}")]
    Tile(#[from] GraphTileProviderError),
    /// An edge costing needs isn't where its graph ID says.
    #[error("find edge {0}")]
    MissingEdge(GraphId),
}

/// An edge's cost and where its speed came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeCostResult {
    /// The cost and elapsed time of traversing the whole edge.
    pub cost: Cost,
    /// The speed sources used (mainline's `flow_sources`); empty for the edge's default speed.
    ///
    /// Thor sets [`Label::has_measured_speed`] when this isn't empty.
    pub flow_sources: EnumSet<FlowSource>,
}

/// The costing interface (mainline's `DynamicCost` virtual methods).
///
/// Implementations are monomorphized: Loki and Thor can be generic over a costing
/// (as with [`AutoCost`]), or use [`AnyCosting`], which dispatches with a `match`.
///
/// Times: `time` (for access) is a moment in a time zone, for timed restrictions;
/// `seconds_of_week` (for edge costs) is the local time of week the edge is entered,
/// in seconds from Sunday 00:00, for predicted speeds.
/// Both are absent for routes without a time.
pub trait Costing {
    /// The shared state (mainline's `DynamicCost` base class).
    fn base(&self) -> &DynamicCost;

    /// The shared state, for Thor to adjust (passes, hierarchy limits, and so on).
    fn base_mut(&mut self) -> &mut DynamicCost;

    /// Whether the edge may be entered from `pred` (mainline's `Allowed`).
    ///
    /// This checks access, U-turns, simple turn restrictions, the costing's own rules,
    /// and the edge's access restrictions at `time`.
    /// It doesn't check complex restrictions; see [`DynamicCost::restricted`].
    ///
    /// # Performance
    ///
    /// This runs for every edge a search considers. It allocates nothing.
    fn allowed<T: GraphTile, L: Label>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        pred: &L,
        time: TimeContext,
        access: &mut EdgeAccess,
    ) -> bool;

    /// Whether an edge may be used in a reverse search (mainline's `AllowedReverse`).
    ///
    /// `edge` is the edge the reverse search expands to, `opp_edge` (with `opp_edge_id`,
    /// in `tile`) is its opposing edge, which is the direction of travel,
    /// and `pred` is the label of the edge the search came from.
    ///
    /// # Performance
    ///
    /// As for [`allowed`](Self::allowed).
    #[expect(clippy::too_many_arguments, reason = "mainline's signature")]
    fn allowed_reverse<T: GraphTile, L: Label>(
        &self,
        edge: &DirectedEdge,
        pred: &L,
        opp_edge: &DirectedEdge,
        opp_edge_id: GraphId,
        tile: &T,
        time: TimeContext,
        access: &mut EdgeAccess,
    ) -> bool;

    /// The cost of the whole edge (mainline's `EdgeCost`).
    ///
    /// # Errors
    ///
    /// Fails if the edge has a predicted speed flag but the tile has no profile for it.
    ///
    /// # Performance
    ///
    /// This runs for every edge a search expands to. It allocates nothing.
    fn edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        seconds_of_week: Option<u32>,
    ) -> Result<EdgeCostResult, CostingError>;

    /// The cost of turning from `pred` onto `edge` at `node` (mainline's `TransitionCost`).
    ///
    /// `tile` holds `edge`; `provider` gives the predecessor's tile when a costing needs it
    /// (pedestrian elevators count the levels between the two edges).
    /// `edge_destonly` is [`EdgeAccess::edge_destonly`] from the access check.
    ///
    /// # Errors
    ///
    /// Fails if a tile the costing needs can't be read.
    ///
    /// # Performance
    ///
    /// This runs for every edge a search expands to.
    /// Only transitions at elevators read another tile.
    fn transition_cost<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        edge: &DirectedEdge,
        tile: &T,
        node: &NodeInfo,
        pred: &L,
        provider: &P,
        edge_destonly: bool,
    ) -> Result<Cost, CostingError>;

    /// The cost of a transition in a reverse search (mainline's `TransitionCostReverse`).
    ///
    /// `idx` is the local index (at `node`) of the edge the reverse search expands to,
    /// `opp_edge` is that edge's opposing edge, `opp_pred_edge` the opposing edge
    /// of the predecessor (whose turn data is read), and `pred` the predecessor's label.
    /// `tile` holds `node` and `opp_edge`.
    ///
    /// # Errors
    ///
    /// As for [`transition_cost`](Self::transition_cost).
    #[expect(clippy::too_many_arguments, reason = "mainline's signature")]
    fn transition_cost_reverse<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        idx: u8,
        node: &NodeInfo,
        opp_edge: &DirectedEdge,
        opp_pred_edge: &DirectedEdge,
        pred: &L,
        tile: &T,
        provider: &P,
        has_measured_speed: bool,
        internal_turn: InternalTurn,
        opp_edge_destonly: bool,
    ) -> Result<Cost, CostingError>;

    /// Loki's edge filter: whether a location may correlate to the edge
    /// (mainline's `Allowed(edge, tile, disallow_mask)`).
    fn edge_filter<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        tile: &T,
        disallow: DisallowMask,
    ) -> bool;

    /// Whether the node may be passed through (mainline's `Allowed(node)`), also Loki's node filter.
    #[inline]
    fn node_allowed(&self, node: &NodeInfo) -> bool {
        self.base().node_allowed(node)
    }

    /// Seconds per meter for the A* heuristic (mainline's `AStarCostFactor`).
    fn astar_cost_factor(&self) -> f32;

    /// Whether a failed search may be retried with relaxed limits (mainline's `AllowMultiPass`).
    fn allow_multi_pass(&self) -> bool {
        false
    }

    /// The costing's vehicle or pedestrian type (mainline's `travel_type`).
    fn travel_type(&self) -> u8 {
        0
    }

    /// The cost of a whole edge without a time (mainline's time-free `EdgeCost` overload).
    ///
    /// # Errors
    ///
    /// As for [`edge_cost`](Self::edge_cost).
    #[inline]
    fn edge_cost_untimed<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
    ) -> Result<EdgeCostResult, CostingError> {
        self.edge_cost(edge, edge_id, tile, None)
    }

    /// The cost of the fraction of an edge from `start` to `end`,
    /// with the user's cost factors over that fraction (mainline's `PartialEdgeCost`).
    ///
    /// `factor_edge` is the edge whose user cost factors apply:
    /// mainline passes the edge's ID when path search starts or ends on it,
    /// and an invalid ID (`None`) when it recosts a path, which applies none.
    ///
    /// # Errors
    ///
    /// As for [`edge_cost`](Self::edge_cost).
    #[expect(clippy::too_many_arguments, reason = "mainline's signature")]
    fn partial_edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        factor_edge: Option<GraphId>,
        tile: &T,
        seconds_of_week: Option<u32>,
        start: f32,
        end: f32,
    ) -> Result<EdgeCostResult, CostingError>;
}

/// Picks an edge's speed as mainline's `GraphTile::GetSpeed` does, with no live traffic.
///
/// # Errors
///
/// Fails if the edge has a predicted speed flag but the tile has no profile for it.
#[inline]
pub(crate) fn edge_speed<T: GraphTile>(
    tile: &T,
    edge: &DirectedEdge,
    edge_id: GraphId,
    flow_mask: u8,
    seconds_of_week: Option<u32>,
    is_truck: bool,
) -> Result<(u32, EnumSet<FlowSource>), CostingError> {
    let mask = EnumSet::<FlowSource>::from_repr_truncated(flow_mask);
    let index = usize::try_from(edge_id.feature_index()).unwrap_or(usize::MAX);
    let selected = select_speed(
        edge,
        mask,
        seconds_of_week.map(u64::from),
        is_truck,
        |seconds| {
            tile.get_predicted_speed(index, seconds)
                .ok_or(CostingError::MissingSpeedProfile(edge_id))
        },
    )?;
    Ok((selected.kph, selected.flow_sources))
}

/// A costing chosen at run time, dispatched with a `match` (no virtual calls).
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "costings are built once per request"
)]
pub enum AnyCosting {
    Auto(AutoCost),
    Pedestrian(PedestrianCost),
    Bicycle(BicycleCost),
    /// `"costing": "none"`, which Loki uses for `locate` without a costing.
    None(NoCost),
}

impl AnyCosting {
    /// Builds the costing a request's options describe (mainline's `CostFactory::Create`).
    ///
    /// # Errors
    ///
    /// Costings other than auto, pedestrian, and bicycle are [not implemented yet](CostingError::NotImplemented).
    pub fn new(costing: &CostingProto) -> Result<Self, CostingError> {
        let costing_type = CostingType::try_from(costing.r#type)
            .map_err(|_| CostingError::UnknownCosting(costing.r#type))?;
        Ok(match costing_type {
            CostingType::Auto => Self::Auto(AutoCost::new(costing)),
            CostingType::Pedestrian => Self::Pedestrian(PedestrianCost::new(costing)),
            CostingType::Bicycle => Self::Bicycle(BicycleCost::new(costing)),
            CostingType::None => Self::None(NoCost::new(costing)),
            other => return Err(CostingError::NotImplemented(other)),
        })
    }
}

/// Forwards a method to the costing in an [`AnyCosting`].
macro_rules! dispatch {
    ($self:ident, $c:ident => $call:expr) => {
        match $self {
            AnyCosting::Auto($c) => $call,
            AnyCosting::Pedestrian($c) => $call,
            AnyCosting::Bicycle($c) => $call,
            AnyCosting::None($c) => $call,
        }
    };
}

impl Costing for AnyCosting {
    #[inline]
    fn base(&self) -> &DynamicCost {
        dispatch!(self, c => c.base())
    }

    #[inline]
    fn base_mut(&mut self) -> &mut DynamicCost {
        dispatch!(self, c => c.base_mut())
    }

    #[inline]
    fn allowed<T: GraphTile, L: Label>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        pred: &L,
        time: TimeContext,
        access: &mut EdgeAccess,
    ) -> bool {
        dispatch!(self, c => c.allowed(edge, edge_id, tile, pred, time, access))
    }

    #[inline]
    fn allowed_reverse<T: GraphTile, L: Label>(
        &self,
        edge: &DirectedEdge,
        pred: &L,
        opp_edge: &DirectedEdge,
        opp_edge_id: GraphId,
        tile: &T,
        time: TimeContext,
        access: &mut EdgeAccess,
    ) -> bool {
        dispatch!(self, c => c.allowed_reverse(edge, pred, opp_edge, opp_edge_id, tile, time, access))
    }

    #[inline]
    fn edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        seconds_of_week: Option<u32>,
    ) -> Result<EdgeCostResult, CostingError> {
        dispatch!(self, c => c.edge_cost(edge, edge_id, tile, seconds_of_week))
    }

    #[inline]
    fn transition_cost<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        edge: &DirectedEdge,
        tile: &T,
        node: &NodeInfo,
        pred: &L,
        provider: &P,
        edge_destonly: bool,
    ) -> Result<Cost, CostingError> {
        dispatch!(self, c => c.transition_cost(edge, tile, node, pred, provider, edge_destonly))
    }

    #[inline]
    fn transition_cost_reverse<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        idx: u8,
        node: &NodeInfo,
        opp_edge: &DirectedEdge,
        opp_pred_edge: &DirectedEdge,
        pred: &L,
        tile: &T,
        provider: &P,
        has_measured_speed: bool,
        internal_turn: InternalTurn,
        opp_edge_destonly: bool,
    ) -> Result<Cost, CostingError> {
        dispatch!(self, c => c.transition_cost_reverse(
            idx, node, opp_edge, opp_pred_edge, pred, tile, provider, has_measured_speed,
            internal_turn, opp_edge_destonly,
        ))
    }

    #[inline]
    fn edge_filter<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        tile: &T,
        disallow: DisallowMask,
    ) -> bool {
        dispatch!(self, c => c.edge_filter(edge, tile, disallow))
    }

    #[inline]
    fn node_allowed(&self, node: &NodeInfo) -> bool {
        dispatch!(self, c => c.node_allowed(node))
    }

    #[inline]
    fn astar_cost_factor(&self) -> f32 {
        dispatch!(self, c => c.astar_cost_factor())
    }

    #[inline]
    fn allow_multi_pass(&self) -> bool {
        dispatch!(self, c => c.allow_multi_pass())
    }

    #[inline]
    fn travel_type(&self) -> u8 {
        dispatch!(self, c => c.travel_type())
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
        dispatch!(self, c => c.partial_edge_cost(
            edge,
            edge_id,
            factor_edge,
            tile,
            seconds_of_week,
            start,
            end
        ))
    }
}

/// Mainline's `PartialEdgeCost`, given the costing's full edge cost without user factors
/// (computed with an invalid edge ID, so that the factors aren't applied twice).
#[inline]
pub(crate) fn partial_from_full(
    base: &DynamicCost,
    full: EdgeCostResult,
    factor_edge: Option<GraphId>,
    start: f32,
    end: f32,
) -> EdgeCostResult {
    let fraction = (end - start).max(f32::EPSILON);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mainline multiplies a Cost (floats) by the double factor converted to float"
    )]
    let factor = factor_edge.map_or(1.0, |id| base.partial_edge_factor(id, start, end)) as f32;
    EdgeCostResult {
        cost: full.cost * fraction * factor,
        flow_sources: full.flow_sources,
    }
}
