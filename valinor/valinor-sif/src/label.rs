//! What costing reads about the predecessor of an edge: the path search's edge label.
//!
//! Mainline's costings take a `sif::EdgeLabel`, Thor's record of an edge it has reached.
//! Costing reads only a few of its fields; [`Label`] names those,
//! so Thor can keep its own label type (with costs, sort keys, and so on)
//! and implement the trait over it.
//! [`EdgeLabel`] is a compact implementation that Thor can embed or use as is.
//!
//! # What Thor must provide
//!
//! Each method documents where mainline's value comes from.
//! Most are copied from the label's directed edge when the label is made
//! ([`EdgeLabel::new`] does this);
//! the rest are path search state:
//!
//! - [`Label::predecessor`]: the index of the previous label in the same search,
//!   in the store passed to [`DynamicCost::restricted`](crate::DynamicCost::restricted),
//!   or `None` at the origin (mainline's `kInvalidLabel`).
//! - [`Label::mode`]: the travel mode the edge was reached with.
//! - [`Label::path_distance`]: meters from the origin, including this edge.
//! - [`Label::closure_pruning`]: whether the search still avoids closed edges
//!   (always false here, since there is no live traffic).
//! - [`Label::has_measured_speed`]: whether the edge's speed came from measurements
//!   (any flow source but the edge's default speed; see
//!   [`EdgeCostResult::flow_sources`](crate::EdgeCostResult)).
//! - [`Label::internal_turn`]: from [`DynamicCost::turn_type`](crate::DynamicCost::turn_type).
//! - [`Label::destonly`]: the edge's destination-only flag,
//!   or true if [`allowed`](crate::Costing::allowed) found it destination-only at this time.
//! - [`Label::deadend`]: the edge's dead end flag, which Thor may also set
//!   when it starts a search on the edge.

use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile};
use valhalla_graphtile::{GraphId, RoadClass, RoadUse};

use crate::cost::{InternalTurn, TravelMode};

/// The predecessor edge fields that costing reads (mainline's `sif::EdgeLabel`).
///
/// See the [module docs](self) for what Thor must provide.
pub trait Label {
    /// The graph ID of the label's edge (`edgeid`).
    fn edge_id(&self) -> GraphId;
    /// The index of the previous label, or `None` at the origin (`predecessor`).
    fn predecessor(&self) -> Option<u32>;
    /// The edge's [`opposing_local_index`](DirectedEdge::opposing_local_index) (`opp_local_idx`).
    fn opp_local_idx(&self) -> u8;
    /// The edge's simple turn [`restrictions`](DirectedEdge::restrictions), as bits by
    /// local edge index (`restrictions`).
    fn restrictions(&self) -> u8;
    /// The edge's use (`use`).
    fn road_use(&self) -> RoadUse;
    /// The edge's road class (`classification`).
    fn classification(&self) -> RoadClass;
    /// The edge's toll flag (`toll`).
    fn toll(&self) -> bool;
    /// The edge's bridge flag (`bridge`).
    fn bridge(&self) -> bool;
    /// The edge's tunnel flag (`tunnel`).
    fn tunnel(&self) -> bool;
    /// Whether the edge is unpaved (`unpaved`).
    fn unpaved(&self) -> bool;
    /// Whether the edge is a dead end (`deadend`); a U-turn is allowed there.
    fn deadend(&self) -> bool;
    /// Whether the edge is destination-only (`destonly`).
    fn destonly(&self) -> bool;
    /// The travel mode the edge was reached with (`mode`).
    fn mode(&self) -> TravelMode;
    /// Meters from the origin, including this edge (`path_distance`).
    fn path_distance(&self) -> u32;
    /// Whether the search avoids closed edges (`closure_pruning`).
    fn closure_pruning(&self) -> bool;
    /// Whether the edge's speed came from measurements (`has_measured_speed`).
    fn has_measured_speed(&self) -> bool;
    /// The turn made onto the edge, if it is short and internal (`internal_turn`).
    fn internal_turn(&self) -> InternalTurn;
}

/// A store of labels that [`Label::predecessor`] indexes into.
///
/// Complex restrictions walk back along the path through it.
pub trait LabelStore {
    /// The label type.
    type Label: Label;

    /// The label at an index, if there is one.
    fn label(&self, index: u32) -> Option<&Self::Label>;
}

impl<L: Label> LabelStore for [L] {
    type Label = L;

    #[inline]
    fn label(&self, index: u32) -> Option<&L> {
        self.get(usize::try_from(index).ok()?)
    }
}

impl<L: Label> LabelStore for Vec<L> {
    type Label = L;

    #[inline]
    fn label(&self, index: u32) -> Option<&L> {
        self.as_slice().label(index)
    }
}

/// Bits of [`EdgeLabel::flags`].
mod flag {
    pub(super) const TOLL: u16 = 1;
    pub(super) const BRIDGE: u16 = 1 << 1;
    pub(super) const TUNNEL: u16 = 1 << 2;
    pub(super) const UNPAVED: u16 = 1 << 3;
    pub(super) const DEADEND: u16 = 1 << 4;
    pub(super) const DESTONLY: u16 = 1 << 5;
    pub(super) const CLOSURE_PRUNING: u16 = 1 << 6;
    pub(super) const HAS_MEASURED_SPEED: u16 = 1 << 7;
}

/// A compact [`Label`]: the fields of mainline's `EdgeLabel` that costing reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeLabel {
    edge_id: GraphId,
    predecessor: u32,
    path_distance: u32,
    road_use: RoadUse,
    classification: RoadClass,
    mode: TravelMode,
    internal_turn: InternalTurn,
    opp_local_idx: u8,
    restrictions: u8,
    flags: u16,
}

/// What [`EdgeLabel::new`] needs besides the edge itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LabelState {
    /// The index of the previous label, or `None` at the origin.
    pub predecessor: Option<u32>,
    /// The travel mode the edge was reached with.
    pub mode: TravelMode,
    /// Meters from the origin, including this edge.
    pub path_distance: u32,
    /// Whether the search avoids closed edges.
    pub closure_pruning: bool,
    /// Whether the edge's speed came from measurements.
    pub has_measured_speed: bool,
    /// The turn made onto the edge, if it is short and internal.
    pub internal_turn: InternalTurn,
    /// Whether [`allowed`](crate::Costing::allowed) found the edge destination-only
    /// (mainline's `destonly` constructor argument); the edge's own flag counts either way.
    pub destonly: bool,
}

impl EdgeLabel {
    /// A label for `edge`, as mainline's `EdgeLabel` constructor fills it in.
    ///
    /// An edge whose use isn't one Valhalla defines (which a parsed tile never has)
    /// gets [`RoadUse::Other`].
    #[inline]
    pub fn new(edge_id: GraphId, edge: &DirectedEdge, state: LabelState) -> Self {
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
        set(flag::DESTONLY, state.destonly || edge.dest_only());
        set(flag::CLOSURE_PRUNING, state.closure_pruning);
        set(flag::HAS_MEASURED_SPEED, state.has_measured_speed);
        Self {
            edge_id,
            predecessor: state.predecessor.unwrap_or(u32::MAX),
            path_distance: state.path_distance,
            road_use: edge.road_use().unwrap_or(RoadUse::Other),
            classification: edge.classification(),
            mode: state.mode,
            internal_turn: state.internal_turn,
            opp_local_idx: edge.opposing_local_index(),
            restrictions: edge.restrictions().bits(),
            flags,
        }
    }

    /// A label for the edge `edge_id` in `tile` (see [`EdgeLabel::new`]).
    ///
    /// Returns `None` if the edge isn't in the tile.
    #[inline]
    pub fn for_edge_in<T: GraphTile>(
        tile: &T,
        edge_id: GraphId,
        state: LabelState,
    ) -> Option<Self> {
        tile.get_directed_edge(edge_id)
            .ok()
            .map(|edge| Self::new(edge_id, edge, state))
    }

    /// Marks the edge as a dead end (or not), as Thor does at the origin
    /// (mainline's `set_deadend`).
    #[inline]
    pub fn set_deadend(&mut self, deadend: bool) {
        if deadend {
            self.flags |= flag::DEADEND;
        } else {
            self.flags &= !flag::DEADEND;
        }
    }

    /// Clears the simple turn restrictions read off the edge this label was built from.
    ///
    /// A label built for checking a seed's own edge has no predecessor to turn from,
    /// so nothing should be restricted about the seed edge relative to itself.
    #[inline]
    pub fn clear_restrictions(&mut self) {
        self.restrictions = 0;
    }

    #[inline]
    const fn has(&self, bit: u16) -> bool {
        self.flags & bit != 0
    }
}

impl Label for EdgeLabel {
    #[inline]
    fn edge_id(&self) -> GraphId {
        self.edge_id
    }
    #[inline]
    fn predecessor(&self) -> Option<u32> {
        (self.predecessor != u32::MAX).then_some(self.predecessor)
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
