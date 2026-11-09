//! The street network as the search sees it: raw tiles behind a dense, lazily built index.
//!
//! Nothing here is preprocessing that depends on stops or the timetable.
//! A tile gets a *slot*, with a range of dense node and edge numbers,
//! the first time something refers to it; a node's arcs are read out of its tile and costed
//! once, the first time the search expands it.
//! After that, an expansion touches only flat arrays:
//! no tile lookups, no costing, no hashing, and no pointer chasing into tile data.
//! Each node's arcs are appended in the order nodes are first expanded,
//! which is roughly the order the search visits them,
//! so a warm search moves through memory the way the prefetcher likes.
//!
//! # Costs
//!
//! Each directed edge has one cost: Sif's [`secs`](valinor_sif::Cost::secs) for traversing it,
//! rounded once to integer seconds, or [`INF`] if Sif doesn't allow it.
//! An arc stores the cost of the edge as directed (`secs[0]`, charged by a forward search)
//! and of the opposing edge (`secs[1]`, charged by a backward search moving against it).
//! Both come from [`Weights::of`], so a forward search and a backward search can't disagree
//! about a directed edge: that one function is the only place a directed edge gets its cost.
//!
//! Costs depend on the direction of travel, because the world isn't flat:
//! a backward search moving from B to A charges the time of travelling A to B.
//! Access is the intersection of what [`Costing::allowed`] and [`Costing::allowed_reverse`]
//! say about the edge (with no predecessor, since a node-based search has none).

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{BuildId, DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_hierarchy::STANDARD_LEVELS;
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};
use valinor_sif::{Costing, EdgeAccess, InternalTurn, Label, TimeContext, TravelMode};

use crate::error::WalkError;

/// An edge that can't be traversed (or doesn't exist) for this mode.
pub(crate) const INF: u16 = u16::MAX;

/// The largest cost of one directed edge, in seconds; anything above counts as [`INF`].
///
/// 18 hours: a longer directed edge isn't something a person crosses in a transfer.
pub(crate) const MAX_SECS: u32 = INF as u32 - 1;

/// The high bit of [`HotArc::head`]: this is the last arc of its node.
const LAST_ARC: u32 = 1 << 31;

/// [`Self::first_arc`](LazyGraph::first_arc) for a node nobody has expanded.
const UNREAD: u32 = u32::MAX;

/// [`Self::first_arc`](LazyGraph::first_arc) for a node with no arcs (a barrier, or a dead end).
pub(crate) const NO_ARCS: u32 = u32::MAX - 1;

/// `(level, tile) -> slot` for a tile that hasn't been looked for yet.
const NO_SLOT: u32 = u32::MAX;

/// `(level, tile) -> slot` for a tile that isn't in the tile set.
const ABSENT_TILE: u32 = u32::MAX - 1;

/// An arc's dense edge number when the arc is a transition between hierarchy levels.
pub(crate) const NO_EDGE: u32 = u32::MAX;

/// The levels of the standard hierarchy the search can walk on.
const LEVELS: usize = 3;

/// An outgoing arc of a node, as a search reads it: the hot data.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct HotArc {
    /// The dense number of the arc's head, with [`LAST_ARC`] set on a node's last arc.
    head_and_flag: u32,
    /// The cost when a forward search follows this edge as directed, and
    /// when a backward search moves against it (`[forward, backward]`), or [`INF`].
    pub(crate) secs: [u16; 2],
}

impl HotArc {
    /// The dense number of the node this arc leads to.
    #[inline]
    pub(crate) const fn head(self) -> u32 {
        self.head_and_flag & !LAST_ARC
    }

    /// Whether this is the last arc of its node.
    #[inline]
    pub(crate) const fn is_last(self) -> bool {
        self.head_and_flag & LAST_ARC != 0
    }
}

/// An arc's identity, for reconstructing walks: dense numbers of the edge and its opposing edge.
///
/// Kept beside the hot arcs, since only walk reconstruction reads it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ColdArc {
    /// The dense number of the directed edge the arc follows, or [`NO_EDGE`] for a transition.
    pub(crate) edge: u32,
    /// The dense number of the opposing edge, or [`NO_EDGE`].
    pub(crate) opposing: u32,
}

/// What a node-based search knows of its predecessor: nothing.
///
/// A dead end, so no U-turn rule applies, with no restrictions, and no toll or bridge to
/// compare with; the path into a node has no bearing on the node-based cost of the edges out of it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NoPredecessor {
    pub(crate) mode: TravelMode,
}

impl Label for NoPredecessor {
    #[inline]
    fn edge_id(&self) -> GraphId {
        // Read only by transitions, which transfers don't use. The ID is never looked up.
        GraphId::try_from_id(0).unwrap_or_else(|_| unreachable!("zero is a valid graph ID"))
    }
    #[inline]
    fn predecessor(&self) -> Option<u32> {
        None
    }
    #[inline]
    fn opp_local_idx(&self) -> u8 {
        u8::MAX
    }
    #[inline]
    fn restrictions(&self) -> u8 {
        0
    }
    #[inline]
    fn road_use(&self) -> valhalla_graphtile::RoadUse {
        valhalla_graphtile::RoadUse::Other
    }
    #[inline]
    fn classification(&self) -> valhalla_graphtile::RoadClass {
        valhalla_graphtile::RoadClass::ServiceOther
    }
    #[inline]
    fn toll(&self) -> bool {
        false
    }
    #[inline]
    fn bridge(&self) -> bool {
        false
    }
    #[inline]
    fn tunnel(&self) -> bool {
        false
    }
    #[inline]
    fn unpaved(&self) -> bool {
        false
    }
    #[inline]
    fn deadend(&self) -> bool {
        true
    }
    #[inline]
    fn destonly(&self) -> bool {
        false
    }
    #[inline]
    fn mode(&self) -> TravelMode {
        self.mode
    }
    #[inline]
    fn path_distance(&self) -> u32 {
        0
    }
    #[inline]
    fn closure_pruning(&self) -> bool {
        false
    }
    #[inline]
    fn has_measured_speed(&self) -> bool {
        false
    }
    #[inline]
    fn internal_turn(&self) -> InternalTurn {
        InternalTurn::default()
    }
}

/// Where a tile's nodes and edges are in the dense numbering, and its handle.
#[derive(Clone, Debug)]
pub(crate) struct TileSlot<H> {
    pub(crate) handle: H,
    pub(crate) base: GraphId,
    pub(crate) node_base: u32,
    pub(crate) node_count: u32,
    pub(crate) edge_base: u32,
    pub(crate) edge_count: u32,
}

/// What a directed edge and its opposing edge weigh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Weights {
    /// The edge as directed, or [`INF`].
    pub(crate) forward: u16,
    /// The opposing edge, or [`INF`].
    pub(crate) backward: u16,
}

impl Weights {
    /// The cost of traversing a directed edge, or [`INF`] if it can't be traversed.
    ///
    /// `edge` is the direction of travel and `opposing` its opposing edge, each in its tile.
    /// This is the only place a directed edge gets its integer cost.
    #[inline]
    fn of<C: Costing, T: GraphTile>(
        costing: &C,
        pred: NoPredecessor,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        opposing: &DirectedEdge,
    ) -> Result<u16, WalkError> {
        // Time-independent: transfers are charged the same whenever they are walked.
        let mut access = EdgeAccess::new(0);
        if !costing.allowed(edge, edge_id, tile, &pred, TimeContext::NONE, &mut access) {
            return Ok(INF);
        }
        let mut access = EdgeAccess::new(0);
        if !costing.allowed_reverse(
            opposing,
            &pred,
            edge,
            edge_id,
            tile,
            TimeContext::NONE,
            &mut access,
        ) {
            return Ok(INF);
        }
        let cost = costing.edge_cost(edge, edge_id, tile, None)?.cost;
        Ok(rounded_secs(cost.secs))
    }
}

/// Seconds rounded once to the integer cost of a directed edge.
#[inline]
fn rounded_secs(secs: f32) -> u16 {
    if !secs.is_finite() || secs < 0.0 {
        return INF;
    }
    let rounded = secs.round();
    #[expect(
        clippy::cast_precision_loss,
        reason = "the limit is a small integer, exact in f32"
    )]
    if rounded > MAX_SECS as f32 {
        return INF;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a finite, rounded value in range"
    )]
    let value = rounded as u16;
    value
}

/// The part of an edge's whole cost up to `fraction` (0 to 1 along it), rounded once.
///
/// A stop that splits an edge makes two parts, `prefix(weight, fraction)` and
/// `weight - prefix(weight, fraction)`, which sum to the whole edge by construction.
#[inline]
pub(crate) fn prefix(weight: u16, fraction: f32) -> u16 {
    if weight == INF {
        return INF;
    }
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let part = (f32::from(weight) * fraction).round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "between 0 and `weight`"
    )]
    let value = part as u16;
    value.min(weight)
}

/// How much the search has read from tiles and costed, for profiling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphStats {
    /// Tiles the search has looked for in the provider.
    pub tile_lookups: u64,
    /// Tiles that were in the tile set and have a slot.
    pub tile_slots: usize,
    /// Nodes expanded for the first time (their arcs read and costed).
    pub nodes_read: u64,
    /// Arcs stored.
    pub arcs_stored: u64,
    /// Bytes held by the flat node and arc arrays (excluding labels and the queue).
    pub bytes: usize,
}

/// The tile directory and memoized arcs.
#[derive(Debug)]
pub(crate) struct LazyGraph<P: GraphTileProvider, C> {
    pub(crate) provider: P,
    pub(crate) costing: C,
    mode: TravelMode,
    /// `(level, tile) -> slot`, flat by level; the planet's tile counts are small.
    directories: [Box<[u32]>; LEVELS],
    /// Tiles in the order they were first touched; dense bases increase along this.
    pub(crate) slots: Vec<TileSlot<P::TileHandle>>,
    node_total: u32,
    edge_total: u32,
    /// By dense node: the index of the node's first arc, [`UNREAD`], or [`NO_ARCS`].
    pub(crate) first_arc: Vec<u32>,
    /// All arcs, each node's contiguous, in first-expansion order.
    pub(crate) arcs: Vec<HotArc>,
    pub(crate) cold_arcs: Vec<ColdArc>,
    stats: GraphStats,
}

impl<P: GraphTileProvider, C: Costing> LazyGraph<P, C> {
    /// A graph over `provider`, costed by `costing`.
    pub(crate) fn new(provider: P, costing: C) -> Self {
        let mode = costing.base().travel_mode();
        let levels = &*STANDARD_LEVELS;
        let directories = [0, 1, 2].map(|level| {
            vec![NO_SLOT; levels[level].tiling_system.tile_count() as usize].into_boxed_slice()
        });
        Self {
            provider,
            costing,
            mode,
            directories,
            slots: Vec::new(),
            node_total: 0,
            edge_total: 0,
            first_arc: Vec::new(),
            arcs: Vec::new(),
            cold_arcs: Vec::new(),
            stats: GraphStats::default(),
        }
    }

    /// How many dense nodes are numbered so far.
    #[inline]
    pub(crate) fn node_count(&self) -> usize {
        self.node_total as usize
    }

    /// How many dense edges are numbered so far.
    #[inline]
    pub(crate) fn edge_count(&self) -> usize {
        self.edge_total as usize
    }

    /// What the graph has read so far.
    pub(crate) fn stats(&self) -> GraphStats {
        GraphStats {
            tile_slots: self.slots.len(),
            arcs_stored: self.arcs.len() as u64,
            bytes: self.first_arc.capacity() * size_of::<u32>()
                + self.arcs.capacity() * size_of::<HotArc>()
                + self.cold_arcs.capacity() * size_of::<ColdArc>()
                + self.directories.iter().map(|d| d.len() * 4).sum::<usize>(),
            ..self.stats
        }
    }

    /// The slot of the tile that holds `id`, or `None` if the tile isn't in the tile set.
    ///
    /// # Performance
    ///
    /// Two array reads, except the first time a tile is touched.
    #[inline]
    fn slot_of(&mut self, id: GraphId) -> Result<Option<u32>, WalkError> {
        let level = usize::from(id.level());
        let tile = usize::try_from(id.tile_id()).unwrap_or(usize::MAX);
        let Some(&slot) = self.directories.get(level).and_then(|d| d.get(tile)) else {
            return Ok(None);
        };
        match slot {
            NO_SLOT => self.open_slot(id),
            ABSENT_TILE => Ok(None),
            slot => Ok(Some(slot)),
        }
    }

    /// Opens a tile for the first time, giving it a slot.
    #[cold]
    fn open_slot(&mut self, id: GraphId) -> Result<Option<u32>, WalkError> {
        self.stats.tile_lookups += 1;
        let level = usize::from(id.level());
        let tile = usize::try_from(id.tile_id()).unwrap_or(usize::MAX);
        let handle = match self.provider.get_handle_for_tile_containing(id) {
            Ok(handle) => handle,
            Err(GraphTileProviderError::TileNotAvailable(_)) => {
                self.directories[level][tile] = ABSENT_TILE;
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let node_count = u32::try_from(handle.nodes().len()).map_err(|_| WalkError::TooLarge)?;
        let edge_count =
            u32::try_from(handle.directed_edges().len()).map_err(|_| WalkError::TooLarge)?;
        let node_base = self.node_total;
        let edge_base = self.edge_total;
        self.node_total = self
            .node_total
            .checked_add(node_count)
            .filter(|&total| total < LAST_ARC)
            .ok_or(WalkError::TooLarge)?;
        self.edge_total = self
            .edge_total
            .checked_add(edge_count)
            .filter(|&total| total < NO_EDGE)
            .ok_or(WalkError::TooLarge)?;
        self.first_arc.resize(self.node_total as usize, UNREAD);
        let slot = u32::try_from(self.slots.len()).map_err(|_| WalkError::TooLarge)?;
        self.directories[level][tile] = slot;
        self.slots.push(TileSlot {
            base: id.tile_base_id(),
            handle,
            node_base,
            node_count,
            edge_base,
            edge_count,
        });
        Ok(Some(slot))
    }

    /// The dense number of a node, or `None` if its tile isn't in the tile set.
    ///
    /// # Errors
    ///
    /// Fails if the tile can't be read.
    #[inline]
    pub(crate) fn dense_node(&mut self, id: GraphId) -> Result<Option<u32>, WalkError> {
        let Some(slot) = self.slot_of(id)? else {
            return Ok(None);
        };
        let slot = &self.slots[slot as usize];
        let index = u32::try_from(id.feature_index()).unwrap_or(u32::MAX);
        if index >= slot.node_count {
            return Err(WalkError::Lookup(
                id,
                valhalla_graphtile::graph_tile::LookupError::InvalidIndex,
            ));
        }
        Ok(Some(slot.node_base + index))
    }

    /// The dense number of a directed edge, or `None` if its tile isn't in the tile set.
    ///
    /// # Errors
    ///
    /// Fails if the tile can't be read or the edge isn't in it.
    pub(crate) fn dense_edge(&mut self, id: GraphId) -> Result<Option<u32>, WalkError> {
        let Some(slot) = self.slot_of(id)? else {
            return Ok(None);
        };
        let slot = &self.slots[slot as usize];
        let index = u32::try_from(id.feature_index()).unwrap_or(u32::MAX);
        if index >= slot.edge_count {
            return Err(WalkError::Lookup(
                id,
                valhalla_graphtile::graph_tile::LookupError::InvalidIndex,
            ));
        }
        Ok(Some(slot.edge_base + index))
    }

    /// The tile holding `id`, from its slot, or `None` if it isn't in the tile set.
    ///
    /// # Errors
    ///
    /// Fails if the tile can't be read.
    #[inline]
    pub(crate) fn tile(&mut self, id: GraphId) -> Result<Option<P::TileHandle>, WalkError> {
        Ok(self
            .slot_of(id)?
            .map(|slot| self.slots[slot as usize].handle.clone()))
    }

    /// The dense number of a directed edge whose tile already has a slot.
    pub(crate) fn known_dense_edge(&self, id: GraphId) -> Option<u32> {
        let level = usize::from(id.level());
        let tile = usize::try_from(id.tile_id()).ok()?;
        let &slot = self.directories.get(level)?.get(tile)?;
        if slot >= ABSENT_TILE {
            return None;
        }
        let slot = self.slots.get(slot as usize)?;
        let index = u32::try_from(id.feature_index()).ok()?;
        (index < slot.edge_count).then_some(slot.edge_base + index)
    }

    /// The tile slot containing the dense number `dense`, by searching the slots' bases.
    fn slot_by_dense(
        &self,
        dense: u32,
        base_of: impl Fn(&TileSlot<P::TileHandle>) -> u32,
    ) -> &TileSlot<P::TileHandle> {
        let index = self.slots.partition_point(|s| base_of(s) <= dense);
        // Slot bases start at zero, so `index` is at least 1 for any dense number issued.
        &self.slots[index.saturating_sub(1)]
    }

    /// The graph ID of a dense node.
    pub(crate) fn node_id(&self, dense: u32) -> Option<GraphId> {
        let slot = self.slot_by_dense(dense, |s| s.node_base);
        slot.base
            .with_feature_index(u64::from(dense - slot.node_base))
            .ok()
    }

    /// The graph ID of a dense edge.
    pub(crate) fn edge_id(&self, dense: u32) -> Option<GraphId> {
        let slot = self.slot_by_dense(dense, |s| s.edge_base);
        slot.base
            .with_feature_index(u64::from(dense - slot.edge_base))
            .ok()
    }

    /// Everything about a directed edge a stop snap needs: its dense number, the same for its
    /// opposing edge, the dense numbers of its two end nodes, and its [`Weights`].
    ///
    /// Returns `None` if the edge's tile or its end node's tile isn't in the tile set.
    ///
    /// # Errors
    ///
    /// Fails if a tile can't be read, the edge isn't in its tile, or costing fails.
    pub(crate) fn describe_edge(
        &mut self,
        edge_id: GraphId,
    ) -> Result<Option<EdgeDescription>, WalkError> {
        let Some(edge_dense) = self.dense_edge(edge_id)? else {
            return Ok(None);
        };
        let Some(slot) = self.slot_of(edge_id)? else {
            return Ok(None);
        };
        let tile = self.slots[slot as usize].handle.clone();
        let edge = tile
            .get_directed_edge(edge_id)
            .map_err(|e| WalkError::Lookup(edge_id, e))?;
        let end = edge.end_node_id();
        let Some(end_slot) = self.slot_of(end)? else {
            return Ok(None);
        };
        let head = self
            .dense_node(end)?
            .ok_or(WalkError::SnappedEdgeMissing(edge_id))?;
        let end_tile = self.slots[end_slot as usize].handle.clone();
        let end_node = end_tile
            .get_node(end)
            .map_err(|e| WalkError::Lookup(end, e))?;
        let opposing_id = end
            .with_feature_index(
                u64::from(end_node.edge_index()) + u64::from(edge.opposing_edge_index()),
            )
            .map_err(GraphTileProviderError::from)?;
        let opposing = end_tile
            .get_directed_edge(opposing_id)
            .map_err(|e| WalkError::Lookup(opposing_id, e))?;
        let opp_dense = self
            .dense_edge(opposing_id)?
            .ok_or(WalkError::SnappedEdgeMissing(edge_id))?;
        let tail = opposing.end_node_id();
        let tail_dense = self
            .dense_node(tail)?
            .ok_or(WalkError::SnappedEdgeMissing(edge_id))?;
        let pred = NoPredecessor { mode: self.mode };
        let forward = Weights::of(&self.costing, pred, edge, edge_id, &tile, opposing)?;
        let backward = Weights::of(&self.costing, pred, opposing, opposing_id, &end_tile, edge)?;
        Ok(Some(EdgeDescription {
            edge_dense,
            opposing_dense: opp_dense,
            tail: tail_dense,
            head,
            weights: Weights { forward, backward },
        }))
    }

    /// Reads a node's arcs out of its tile and costs them, once.
    ///
    /// # Errors
    ///
    /// Fails if a tile can't be read, or the node or an edge isn't in it, or costing fails.
    #[cold]
    pub(crate) fn materialize(&mut self, dense: u32) -> Result<(), WalkError> {
        self.stats.nodes_read += 1;
        // Only dense numbers from `dense_node` get here, and those always have an ID.
        let Some(node_id) = self.node_id(dense) else {
            self.first_arc[dense as usize] = NO_ARCS;
            return Ok(());
        };
        let Some(slot) = self.slot_of(node_id)? else {
            self.first_arc[dense as usize] = NO_ARCS;
            return Ok(());
        };
        let tile = self.slots[slot as usize].handle.clone();
        let node = tile
            .get_node(node_id)
            .map_err(|e| WalkError::Lookup(node_id, e))?;
        if !self.costing.node_allowed(node) {
            // A barrier: nothing may pass through, in either direction.
            self.first_arc[dense as usize] = NO_ARCS;
            return Ok(());
        }
        let first = self.arcs.len();
        let pred = NoPredecessor { mode: self.mode };
        self.read_edges(&tile, node_id, node, pred)?;
        self.read_transitions(&tile, node)?;
        if self.arcs.len() == first {
            self.first_arc[dense as usize] = NO_ARCS;
        } else {
            self.first_arc[dense as usize] =
                u32::try_from(first).map_err(|_| WalkError::TooLarge)?;
            if let Some(last) = self.arcs.last_mut() {
                last.head_and_flag |= LAST_ARC;
            }
        }
        Ok(())
    }

    /// Appends the arcs of `node`'s outbound edges.
    fn read_edges(
        &mut self,
        tile: &P::TileHandle,
        node_id: GraphId,
        node: &NodeInfo,
        pred: NoPredecessor,
    ) -> Result<(), WalkError> {
        let first_edge = u64::from(node.edge_index());
        for (i, edge) in tile.get_outbound_edges_from_node(node).iter().enumerate() {
            if edge.is_shortcut() {
                continue;
            }
            let edge_id = node_id
                .tile_base_id()
                .with_feature_index(first_edge + i as u64)
                .map_err(GraphTileProviderError::from)?;
            let end = edge.end_node_id();
            let Some(head_dense) = self.dense_node(end)? else {
                // The tile set ends here: a dead end, as in Thor.
                continue;
            };
            let Some(end_slot) = self.slot_of(end)? else {
                continue;
            };
            let end_tile = self.slots[end_slot as usize].handle.clone();
            let end_node = end_tile
                .get_node(end)
                .map_err(|e| WalkError::Lookup(end, e))?;
            let opposing_id = end
                .with_feature_index(
                    u64::from(end_node.edge_index()) + u64::from(edge.opposing_edge_index()),
                )
                .map_err(GraphTileProviderError::from)?;
            let opposing = end_tile
                .get_directed_edge(opposing_id)
                .map_err(|e| WalkError::Lookup(opposing_id, e))?;
            let forward = Weights::of(&self.costing, pred, edge, edge_id, tile, opposing)?;
            let backward =
                Weights::of(&self.costing, pred, opposing, opposing_id, &end_tile, edge)?;
            if forward == INF && backward == INF {
                continue;
            }
            let dense_edge = self
                .dense_edge(edge_id)?
                .ok_or(WalkError::SnappedEdgeMissing(edge_id))?;
            let dense_opp = self
                .dense_edge(opposing_id)?
                .ok_or(WalkError::SnappedEdgeMissing(opposing_id))?;
            self.arcs.push(HotArc {
                head_and_flag: head_dense,
                secs: [forward, backward],
            });
            self.cold_arcs.push(ColdArc {
                edge: dense_edge,
                opposing: dense_opp,
            });
        }
        Ok(())
    }

    /// Appends the arcs of a node's transitions to its copies on other hierarchy levels.
    ///
    /// Moving between levels costs nothing, in either direction.
    fn read_transitions(&mut self, tile: &P::TileHandle, node: &NodeInfo) -> Result<(), WalkError> {
        for transition in tile.get_transitions(node) {
            let end = transition.corresponding_end_node_id();
            let Some(head_dense) = self.dense_node(end)? else {
                continue;
            };
            self.arcs.push(HotArc {
                head_and_flag: head_dense,
                secs: [0, 0],
            });
            self.cold_arcs.push(ColdArc {
                edge: NO_EDGE,
                opposing: NO_EDGE,
            });
        }
        Ok(())
    }

    /// The tile build of a tile with a slot, for tests.
    #[cfg(test)]
    pub(crate) fn build_id(&self) -> Option<BuildId> {
        self.slots.first().map(|s| s.handle.header().build_id())
    }

    /// The build ID of the tile holding `id`, opening it if needed.
    ///
    /// # Errors
    ///
    /// Fails if the tile can't be read.
    pub(crate) fn build_id_of(&mut self, id: GraphId) -> Result<Option<BuildId>, WalkError> {
        Ok(self
            .slot_of(id)?
            .map(|slot| self.slots[slot as usize].handle.header().build_id()))
    }
}

/// What a stop snap needs to know about its edge; see [`LazyGraph::describe_edge`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EdgeDescription {
    pub(crate) edge_dense: u32,
    pub(crate) opposing_dense: u32,
    pub(crate) tail: u32,
    pub(crate) head: u32,
    pub(crate) weights: Weights,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_split_the_whole_edge() {
        for weight in [0u16, 1, 7, 100, 65_000] {
            for tenths in 0..=10u8 {
                let fraction = f32::from(tenths) / 10.0;
                let head = prefix(weight, fraction);
                let tail = weight - head;
                assert_eq!(head + tail, weight);
                assert!(head <= weight);
            }
        }
    }

    #[test]
    fn prefix_is_monotone_so_differences_are_never_negative() {
        for weight in [3u16, 17, 99, 1000] {
            let mut previous = 0;
            for hundredths in 0..=100u16 {
                let part = prefix(weight, f32::from(hundredths) / 100.0);
                assert!(part >= previous, "{part} < {previous}");
                previous = part;
            }
        }
    }

    #[test]
    fn an_impassable_edge_has_no_prefix() {
        assert_eq!(prefix(INF, 0.5), INF);
    }

    #[test]
    fn rounding_rejects_what_can_never_be_walked() {
        assert_eq!(rounded_secs(f32::NAN), INF);
        assert_eq!(rounded_secs(-1.0), INF);
        assert_eq!(rounded_secs(70_000.0), INF);
        assert_eq!(rounded_secs(0.4), 0);
        assert_eq!(rounded_secs(0.5), 1);
        assert_eq!(rounded_secs(123.0), 123);
    }
}
