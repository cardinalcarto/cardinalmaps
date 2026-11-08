//! Thor's edge labels (mainline's `sif::BDEdgeLabel`, which all the searches here use).
//!
//! A label records how a search reached a directed edge: the previous label,
//! the cost and time at the end of the edge, the sort cost (with the A* heuristic),
//! the transition cost onto the edge, and what costing reads about the edge
//! when the search goes on from it (see [`valinor_sif::label`]).

use valhalla_graphtile::graph_tile::DirectedEdge;
use valhalla_graphtile::{GraphId, RoadClass, RoadUse};
use valinor_sif::{Cost, InternalTurn, Label, TravelMode};

use crate::queue::SortCost;

/// Bits of [`ThorLabel::flags`].
mod flag {
    pub(super) const TOLL: u32 = 1;
    pub(super) const BRIDGE: u32 = 1 << 1;
    pub(super) const TUNNEL: u32 = 1 << 2;
    pub(super) const UNPAVED: u32 = 1 << 3;
    pub(super) const DEADEND: u32 = 1 << 4;
    pub(super) const DESTONLY: u32 = 1 << 5;
    pub(super) const CLOSURE_PRUNING: u32 = 1 << 6;
    pub(super) const HAS_MEASURED_SPEED: u32 = 1 << 7;
    pub(super) const NOT_THRU: u32 = 1 << 8;
    pub(super) const ON_COMPLEX_RESTRICTION: u32 = 1 << 9;
    pub(super) const NOT_THRU_PRUNING: u32 = 1 << 10;
    pub(super) const ORIGIN: u32 = 1 << 11;
    pub(super) const DESTINATION: u32 = 1 << 12;
}

/// The largest path distance a label holds (mainline stores it in 25 bits).
const PATH_DISTANCE_MASK: u32 = (1 << 25) - 1;

/// A label for an edge reached by a search (mainline's `BDEdgeLabel`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ThorLabel {
    pub(crate) edge_id: GraphId,
    /// The opposing edge, which mainline leaves invalid at the origins of forward searches.
    pub(crate) opp_edge_id: Option<GraphId>,
    pub(crate) end_node: GraphId,
    pub(crate) predecessor: u32,
    pub(crate) cost: Cost,
    pub(crate) sortcost: f32,
    pub(crate) transition_cost: Cost,
    /// The distance to the destination (forward) or origin (reverse),
    /// or from the start for hierarchy limits (see the searches).
    pub(crate) distance: f32,
    path_distance: u32,
    pub(crate) restriction_idx: u8,
    pub(crate) destonly_access_restr_mask: u8,
    road_use: RoadUse,
    classification: RoadClass,
    mode: TravelMode,
    internal_turn: InternalTurn,
    opp_local_idx: u8,
    restrictions: u8,
    flags: u32,
}

/// No previous label (mainline's `kInvalidLabel`).
pub(crate) const INVALID_LABEL: u32 = u32::MAX;

/// What a new label holds besides what it copies from its edge.
#[derive(Clone, Copy, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "mainline's label flags")]
pub(crate) struct NewLabel {
    pub(crate) predecessor: u32,
    pub(crate) edge_id: GraphId,
    pub(crate) opp_edge_id: Option<GraphId>,
    pub(crate) cost: Cost,
    pub(crate) sortcost: f32,
    pub(crate) distance: f32,
    pub(crate) mode: TravelMode,
    pub(crate) transition_cost: Cost,
    pub(crate) not_thru_pruning: bool,
    pub(crate) closure_pruning: bool,
    pub(crate) has_measured_speed: bool,
    pub(crate) internal_turn: InternalTurn,
    pub(crate) restriction_idx: u8,
    pub(crate) destonly: bool,
    pub(crate) destonly_access_restr_mask: u8,
}

impl ThorLabel {
    /// A label for `edge`, as `BDEdgeLabel`'s constructors fill it in
    /// (with a path distance of 0, which only unidirectional searches update).
    #[inline]
    pub(crate) fn new(edge: &DirectedEdge, new: NewLabel) -> Self {
        let mut flags = 0;
        let mut set = |bit, on: bool| {
            if on {
                flags |= bit;
            }
        };
        set(flag::TOLL, edge.toll());
        set(flag::BRIDGE, edge.is_bridge());
        set(flag::TUNNEL, edge.is_tunnel());
        set(flag::UNPAVED, edge.is_unpaved());
        set(flag::DEADEND, edge.is_dead_end());
        set(flag::DESTONLY, new.destonly || edge.dest_only());
        set(flag::CLOSURE_PRUNING, new.closure_pruning);
        set(flag::HAS_MEASURED_SPEED, new.has_measured_speed);
        set(flag::NOT_THRU, edge.no_thru());
        set(
            flag::ON_COMPLEX_RESTRICTION,
            edge.part_of_complex_restriction()
                || !edge.start_restriction().is_empty()
                || !edge.end_restriction().is_empty(),
        );
        set(flag::NOT_THRU_PRUNING, new.not_thru_pruning);
        Self {
            edge_id: new.edge_id,
            opp_edge_id: new.opp_edge_id,
            end_node: edge.end_node_id(),
            predecessor: new.predecessor,
            cost: new.cost,
            sortcost: new.sortcost,
            transition_cost: new.transition_cost,
            distance: new.distance,
            path_distance: 0,
            restriction_idx: new.restriction_idx,
            destonly_access_restr_mask: new.destonly_access_restr_mask,
            road_use: edge.road_use().unwrap_or(RoadUse::Other),
            classification: edge.classification(),
            mode: new.mode,
            internal_turn: new.internal_turn,
            opp_local_idx: edge.opposing_local_index(),
            restrictions: edge.restrictions().bits(),
            flags,
        }
    }

    /// A label with nothing set, as mainline's default `BDEdgeLabel`
    /// (the searches compare against one before a direction has popped anything).
    ///
    /// Its edge and end node are `placeholder`, which nothing reads.
    pub(crate) fn empty(placeholder: GraphId) -> Self {
        Self {
            edge_id: placeholder,
            opp_edge_id: None,
            end_node: placeholder,
            predecessor: INVALID_LABEL,
            cost: Cost::ZERO,
            sortcost: 0.0,
            transition_cost: Cost::ZERO,
            distance: 0.0,
            path_distance: 0,
            restriction_idx: 0,
            destonly_access_restr_mask: 0,
            road_use: RoadUse::Road,
            classification: RoadClass::Motorway,
            mode: TravelMode::Drive,
            internal_turn: InternalTurn::NoTurn,
            opp_local_idx: 0,
            restrictions: 0,
            flags: 0,
        }
    }

    #[inline]
    const fn has(&self, bit: u32) -> bool {
        self.flags & bit != 0
    }

    #[inline]
    fn put(&mut self, bit: u32, on: bool) {
        if on {
            self.flags |= bit;
        } else {
            self.flags &= !bit;
        }
    }

    /// Updates the label for a cheaper path (`Update` without a path distance).
    #[inline]
    pub(crate) fn update(
        &mut self,
        predecessor: u32,
        cost: Cost,
        sortcost: f32,
        transition_cost: Cost,
        restriction_idx: u8,
    ) {
        self.predecessor = predecessor;
        self.cost = cost;
        self.sortcost = sortcost;
        self.transition_cost = transition_cost;
        self.restriction_idx = restriction_idx;
    }

    /// Updates the label, with its path distance (`Update` with a path distance).
    #[inline]
    pub(crate) fn update_with_distance(
        &mut self,
        predecessor: u32,
        cost: Cost,
        sortcost: f32,
        transition_cost: Cost,
        path_distance: u32,
        restriction_idx: u8,
    ) {
        self.update(
            predecessor,
            cost,
            sortcost,
            transition_cost,
            restriction_idx,
        );
        self.path_distance = path_distance & PATH_DISTANCE_MASK;
    }

    #[inline]
    pub(crate) fn predecessor_index(&self) -> Option<usize> {
        (self.predecessor != INVALID_LABEL)
            .then(|| usize::try_from(self.predecessor).ok())
            .flatten()
    }

    #[inline]
    pub(crate) fn not_thru(&self) -> bool {
        self.has(flag::NOT_THRU)
    }

    #[inline]
    pub(crate) fn set_not_thru(&mut self, not_thru: bool) {
        self.put(flag::NOT_THRU, not_thru);
    }

    #[inline]
    pub(crate) fn not_thru_pruning(&self) -> bool {
        self.has(flag::NOT_THRU_PRUNING)
    }

    #[inline]
    pub(crate) fn on_complex_restriction(&self) -> bool {
        self.has(flag::ON_COMPLEX_RESTRICTION)
    }

    #[inline]
    pub(crate) fn set_deadend(&mut self, deadend: bool) {
        self.put(flag::DEADEND, deadend);
    }

    #[inline]
    pub(crate) fn origin(&self) -> bool {
        self.has(flag::ORIGIN)
    }

    #[inline]
    pub(crate) fn set_origin(&mut self) {
        self.put(flag::ORIGIN, true);
    }

    #[inline]
    pub(crate) fn destination(&self) -> bool {
        self.has(flag::DESTINATION)
    }

    #[inline]
    pub(crate) fn set_destination(&mut self) {
        self.put(flag::DESTINATION, true);
    }

    #[inline]
    pub(crate) fn is_ferry(&self) -> bool {
        self.road_use == RoadUse::Ferry
    }
}

impl SortCost for ThorLabel {
    #[inline]
    fn sortcost(&self) -> f32 {
        self.sortcost
    }
}

impl Label for ThorLabel {
    #[inline]
    fn edge_id(&self) -> GraphId {
        self.edge_id
    }
    #[inline]
    fn predecessor(&self) -> Option<u32> {
        (self.predecessor != INVALID_LABEL).then_some(self.predecessor)
    }
    #[inline]
    fn opp_local_idx(&self) -> u8 {
        self.opp_local_idx
    }
    #[inline]
    fn restrictions(&self) -> u8 {
        self.restrictions
    }
    #[inline]
    fn road_use(&self) -> RoadUse {
        self.road_use
    }
    #[inline]
    fn classification(&self) -> RoadClass {
        self.classification
    }
    #[inline]
    fn toll(&self) -> bool {
        self.has(flag::TOLL)
    }
    #[inline]
    fn bridge(&self) -> bool {
        self.has(flag::BRIDGE)
    }
    #[inline]
    fn tunnel(&self) -> bool {
        self.has(flag::TUNNEL)
    }
    #[inline]
    fn unpaved(&self) -> bool {
        self.has(flag::UNPAVED)
    }
    #[inline]
    fn deadend(&self) -> bool {
        self.has(flag::DEADEND)
    }
    #[inline]
    fn destonly(&self) -> bool {
        self.has(flag::DESTONLY)
    }
    #[inline]
    fn mode(&self) -> TravelMode {
        self.mode
    }
    #[inline]
    fn path_distance(&self) -> u32 {
        self.path_distance
    }
    #[inline]
    fn closure_pruning(&self) -> bool {
        self.has(flag::CLOSURE_PRUNING)
    }
    #[inline]
    fn has_measured_speed(&self) -> bool {
        self.has(flag::HAS_MEASURED_SPEED)
    }
    #[inline]
    fn internal_turn(&self) -> InternalTurn {
        self.internal_turn
    }
}
