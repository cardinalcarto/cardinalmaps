//! How far the graph reaches from an edge (mainline's `loki::Reach`, `src/loki/reach.cc`).
//!
//! A location's minimum reachability asks that its edge lead somewhere:
//! that at least so many nodes can be reached from it (outbound),
//! and that it can be reached from at least so many (inbound).
//! This keeps locations off small islands such as a parking aisle cut off by a gate.
//!
//! Mainline first does a cheap expansion that counts nodes without costing turns
//! (stopping a little early around restrictions), and when that falls short of the limit,
//! an exact one with full costing (`thor::Dijkstras`), keeping the larger count.
//! Both are ported here step by step, because the counts are part of Loki's output
//! (`inbound_reach` and `outbound_reach`) and decide which candidates are reachable:
//!
//! - The cheap expansion counts queued and settled nodes, less the nodes it queued again on
//!   other levels, as mainline does (`size_t` arithmetic, wrapping included).
//!   Mainline's queue is an `ankerl::unordered_dense::set`, which pops in a different order
//!   than our first in, first out [`QueueSet`]; the order can in principle change how often
//!   a node is counted on another level, but not on any of the oracle's 4,500 requests.
//! - The exact expansion uses mainline's double bucket queue ([`BucketQueue`]),
//!   which pops in bucket order and, within a bucket, last in first out.
//!
//! # Quirks reproduced
//!
//! - The exact check is meant to rerun only the directions that fell short,
//!   but tests the direction mask with `|`, so it always runs both.
//!   Running both or one gives the same result (it takes the larger count either way),
//!   so we only run the directions that fell short.
//! - The exact check's reverse expansion tests complex restrictions as if it went forward
//!   (`Restricted(..., true)`), as `thor::Dijkstras` does.

use crate::graph::GraphError;
use std::collections::{HashMap, HashSet, VecDeque};
use valhalla_graphtile::graph_reader::{
    GraphReader, Tile, edge as get_edge, node as get_node, opposing_in_end_tile,
};
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_graphtile::{GraphId, RoadClass, RoadUse};
use valinor_sif::{
    Cost, Costing, DisallowMask, EdgeAccess, EdgeLabel, InternalTurn, Label, LabelState,
    TimeContext, TravelMode,
};

/// Reach in both directions (mainline's `directed_reach`, two 16-bit fields).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DirectedReach {
    pub(crate) outbound: u32,
    pub(crate) inbound: u32,
}

impl DirectedReach {
    /// Stores values the way mainline's 16-bit bit fields do.
    #[inline]
    pub(crate) const fn new(outbound: u32, inbound: u32) -> Self {
        Self {
            outbound: outbound & 0xffff,
            inbound: inbound & 0xffff,
        }
    }
}

/// A first in, first out queue without duplicates (mainline's queue is a set).
#[derive(Debug, Default)]
pub(crate) struct QueueSet {
    queue: VecDeque<GraphId>,
    members: HashSet<GraphId>,
}

impl QueueSet {
    fn clear(&mut self) {
        self.queue.clear();
        self.members.clear();
    }

    fn len(&self) -> usize {
        self.queue.len()
    }

    /// Queues a value; returns whether it was new.
    fn insert(&mut self, id: GraphId) -> bool {
        if !self.members.insert(id) {
            return false;
        }
        self.queue.push_back(id);
        true
    }

    /// Removes and returns the oldest value.
    fn pop_front(&mut self) -> Option<GraphId> {
        let first = self.queue.pop_front()?;
        self.members.remove(&first);
        Some(first)
    }
}

/// Mainline's `baldr::DoubleBucketQueue` over label indexes, sorted by each label's sort cost.
#[derive(Debug, Default)]
struct BucketQueue {
    bucketrange: f32,
    bucketsize: f32,
    inv: f32,
    mincost: f64,
    maxcost: f32,
    currentcost: f32,
    buckets: Vec<Vec<u32>>,
    current: usize,
    overflow: Vec<u32>,
}

impl BucketQueue {
    /// Resets the queue (`reuse`); mainline's searches start at cost 0 with unit buckets.
    fn reuse(&mut self, range: f32) {
        self.currentcost = 0.0;
        self.mincost = 0.0;
        self.bucketrange = range;
        self.bucketsize = 1.0;
        self.inv = 1.0;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline stores the double sum in a float"
        )]
        let maxcost = (self.mincost + f64::from(self.bucketrange)) as f32;
        self.maxcost = maxcost;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mainline converts the float count to size_t"
        )]
        let count = (range / self.bucketsize) as usize + 1;
        self.buckets.iter_mut().for_each(Vec::clear);
        self.buckets.resize_with(count, Vec::new);
        self.current = 0;
        self.overflow.clear();
    }

    /// Where a label with this sort cost goes (`get_bucket`): `None` is the overflow bucket.
    fn bucket_index(&self, cost: f32) -> Option<usize> {
        if cost < self.currentcost {
            Some(self.current)
        } else if cost < self.maxcost {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "mainline casts to uint32"
            )]
            let index = ((f64::from(cost) - self.mincost) * f64::from(self.inv)) as u32 as usize;
            Some(index)
        } else {
            None
        }
    }

    fn bucket_mut(&mut self, index: Option<usize>) -> &mut Vec<u32> {
        match index {
            // The index is always in range for a cost below `maxcost`;
            // anything else goes to the overflow bucket rather than panicking.
            Some(i) if i < self.buckets.len() => &mut self.buckets[i],
            _ => &mut self.overflow,
        }
    }

    fn add(&mut self, label: u32, sortcost: f32) {
        let index = self.bucket_index(sortcost);
        self.bucket_mut(index).push(label);
    }

    /// Moves a label to the bucket of its new sort cost (`decrease`).
    fn decrease(&mut self, label: u32, oldcost: f32, newcost: f32) {
        let old = self.bucket_index(oldcost);
        let new = self.bucket_index(newcost);
        if old != new {
            self.bucket_mut(new).push(label);
            let previous = self.bucket_mut(old);
            if let Some(position) = previous.iter().position(|&l| l == label) {
                previous.remove(position);
            }
        }
    }

    /// Whether every bucket from the current one on is empty, advancing past empty ones.
    fn empty(&mut self) -> bool {
        while self.current < self.buckets.len() && self.buckets[self.current].is_empty() {
            self.current += 1;
            self.currentcost += self.bucketsize;
        }
        self.current == self.buckets.len()
    }

    fn pop(&mut self, sortcost: impl Fn(u32) -> f32) -> Option<u32> {
        if self.empty() {
            if self.overflow.is_empty() {
                return None;
            }
            self.empty_overflow(&sortcost);
            if self.empty() {
                return None;
            }
        }
        self.buckets.get_mut(self.current)?.pop()
    }

    /// Refills the buckets from the overflow bucket, starting at its cheapest label.
    fn empty_overflow(&mut self, sortcost: &impl Fn(u32) -> f32) {
        // The first label with the smallest cost (`std::min_element`).
        let mut min: Option<f32> = None;
        for &label in &self.overflow {
            let cost = sortcost(label);
            if min.is_none_or(|m| cost < m) {
                min = Some(cost);
            }
        }
        if let Some(min) = min {
            let min = f64::from(min);
            let range = f64::from(self.bucketrange);
            self.mincost += ((min - self.mincost) / range).floor() * range;
            if self.mincost > min {
                self.mincost -= range;
            } else if self.mincost + range < min {
                self.mincost += range;
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "mainline stores the double sum in a float"
            )]
            let maxcost = (self.mincost + range) as f32;
            self.maxcost = maxcost;
            let mut kept = Vec::with_capacity(self.overflow.len());
            for label in core::mem::take(&mut self.overflow) {
                let cost = sortcost(label);
                if cost < self.maxcost {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "mainline casts to uint32"
                    )]
                    let index = ((f64::from(cost) - self.mincost) * f64::from(self.inv)) as u32;
                    match self.buckets.get_mut(index as usize) {
                        Some(bucket) => bucket.push(label),
                        None => kept.push(label),
                    }
                } else {
                    kept.push(label);
                }
            }
            self.overflow = kept;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline stores the double in a float"
        )]
        let currentcost = self.mincost as f32;
        self.currentcost = currentcost;
        self.current = 0;
    }
}

/// An edge label of the exact expansion (the parts of mainline's `BDEdgeLabel` it uses).
#[derive(Clone, Copy, Debug)]
struct ReachLabel {
    base: EdgeLabel,
    predecessor: Option<u32>,
    path_distance: u32,
    cost: Cost,
    sortcost: f32,
    end_node: GraphId,
    destonly_mask: u8,
}

impl Label for ReachLabel {
    fn edge_id(&self) -> GraphId {
        self.base.edge_id()
    }
    fn predecessor(&self) -> Option<u32> {
        self.predecessor
    }
    fn opp_local_idx(&self) -> u8 {
        self.base.opp_local_idx()
    }
    fn restrictions(&self) -> u8 {
        self.base.restrictions()
    }
    fn road_use(&self) -> RoadUse {
        self.base.road_use()
    }
    fn classification(&self) -> RoadClass {
        self.base.classification()
    }
    fn toll(&self) -> bool {
        self.base.toll()
    }
    fn bridge(&self) -> bool {
        self.base.bridge()
    }
    fn tunnel(&self) -> bool {
        self.base.tunnel()
    }
    fn unpaved(&self) -> bool {
        self.base.unpaved()
    }
    fn deadend(&self) -> bool {
        self.base.deadend()
    }
    fn destonly(&self) -> bool {
        self.base.destonly()
    }
    fn mode(&self) -> TravelMode {
        self.base.mode()
    }
    fn path_distance(&self) -> u32 {
        self.path_distance
    }
    fn closure_pruning(&self) -> bool {
        self.base.closure_pruning()
    }
    fn has_measured_speed(&self) -> bool {
        self.base.has_measured_speed()
    }
    fn internal_turn(&self) -> InternalTurn {
        self.base.internal_turn()
    }
}

/// Edge states of the exact expansion (mainline's `EdgeStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeSet {
    Temporary(u32),
    Permanent,
}

/// What the exact expansion does after settling an edge (`ExpansionRecommendation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Recommendation {
    Continue,
    Stop,
}

/// The direction of an exact expansion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Forward,
    Reverse,
}

/// Computes reach, reusing its buffers between edges (mainline's `Reach`).
#[derive(Debug, Default)]
pub(crate) struct Reach {
    queue: QueueSet,
    done: HashSet<GraphId>,
    /// Nodes counted twice because they were reached on more than one level.
    transitions: u64,
    limit: u32,
    labels: Vec<ReachLabel>,
    status: HashMap<GraphId, EdgeSet>,
    adjacency: BucketQueue,
}

/// The cheap forward expansion skips edges it can't be sure of without a predecessor.
const FORWARD_DISALLOW: DisallowMask = DisallowMask::from_bits(
    DisallowMask::END_RESTRICTION.bits()
        | DisallowMask::SIMPLE_RESTRICTION.bits()
        | DisallowMask::CLOSURE.bits()
        | DisallowMask::SHORTCUT.bits(),
);
/// The cheap reverse expansion's equivalent.
const REVERSE_DISALLOW: DisallowMask = DisallowMask::from_bits(
    DisallowMask::START_RESTRICTION.bits()
        | DisallowMask::SIMPLE_RESTRICTION.bits()
        | DisallowMask::CLOSURE.bits()
        | DisallowMask::SHORTCUT.bits(),
);

impl Reach {
    /// The cheap expansion's count: settled plus queued nodes, less duplicates on other levels
    /// (`size_t` arithmetic in mainline, so it wraps).
    fn count(&self) -> u64 {
        (self.queue.len() as u64 + self.done.len() as u64).wrapping_sub(self.transitions)
    }

    /// The exact expansion's count of settled nodes, less duplicates on other levels.
    fn done_count(&self) -> u64 {
        (self.done.len() as u64).wrapping_sub(self.transitions)
    }

    fn clear(&mut self) {
        self.queue.clear();
        self.done.clear();
        self.transitions = 0;
        self.labels.clear();
        self.status.clear();
    }

    /// The inbound and outbound reach of an edge, up to `max_reach` (`Reach::operator()`).
    ///
    /// # Errors
    ///
    /// Fails if a tile can't be read or costing fails.
    pub(crate) fn reach<P: GraphTileProvider, C: Costing>(
        &mut self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        max_reach: u32,
        graph: GraphReader<'_, P>,
        costing: &C,
    ) -> Result<DirectedReach, GraphError> {
        if max_reach == 0 {
            return Ok(DirectedReach::default());
        }
        self.limit = max_reach;
        let limit = u64::from(max_reach);
        let start_tile = graph.tile(edge_id)?;

        // Outbound: a simple forward expansion until the limit or a dead end.
        self.clear();
        if let Some(tile) = &start_tile
            && costing.edge_filter(
                edge,
                tile,
                DisallowMask::SIMPLE_RESTRICTION | DisallowMask::SHORTCUT,
            )
        {
            self.enqueue(edge.end_node_id(), graph, costing, tile)?;
        }
        let mut tile = start_tile.clone();
        while self.count() < limit {
            let Some(node_id) = self.queue.pop_front() else {
                break;
            };
            self.done.insert(node_id);
            tile = match &tile {
                Some(hint) => graph.tile_from(node_id, hint)?,
                None => graph.tile(node_id)?,
            };
            let Some(node_tile) = &tile else {
                continue;
            };
            let node = get_node(node_tile, node_id)?;
            for out in node_tile.get_outbound_edges_from_node(node) {
                if costing.edge_filter(out, node_tile, FORWARD_DISALLOW) {
                    self.enqueue(out.end_node_id(), graph, costing, node_tile)?;
                }
            }
        }
        let outbound = clamp_count(self.count(), max_reach);

        // Inbound: a simple reverse expansion from the edge's begin node.
        self.clear();
        if let Some(tile) = &start_tile
            && costing.edge_filter(edge, tile, DisallowMask::SHORTCUT)
            && let Some(begin) = begin_node(graph, edge, tile)?
        {
            self.enqueue(begin, graph, costing, tile)?;
        }
        let mut tile = start_tile.clone();
        while self.count() < limit {
            let Some(node_id) = self.queue.pop_front() else {
                break;
            };
            self.done.insert(node_id);
            tile = match &tile {
                Some(hint) => graph.tile_from(node_id, hint)?,
                None => graph.tile(node_id)?,
            };
            let Some(node_tile) = tile.clone() else {
                continue;
            };
            let node = get_node(&node_tile, node_id)?;
            let first = node_id
                .with_feature_index(u64::from(node.edge_index()))
                .map_err(valhalla_graphtile::tile_provider::GraphTileProviderError::from)?;
            for (i, out) in node_tile
                .get_outbound_edges_from_node(node)
                .iter()
                .enumerate()
            {
                let out_id = first
                    .with_feature_index(first.feature_index() + i as u64)
                    .map_err(valhalla_graphtile::tile_provider::GraphTileProviderError::from)?;
                let Some((opp_id, opp_tile)) = graph.opposing(out_id, &node_tile)? else {
                    tile = None;
                    continue;
                };
                let opp = get_edge(&opp_tile, opp_id)?;
                if costing.edge_filter(opp, &opp_tile, REVERSE_DISALLOW) {
                    self.enqueue(out.end_node_id(), graph, costing, &opp_tile)?;
                }
                tile = Some(opp_tile);
            }
        }
        let inbound = clamp_count(self.count(), max_reach);
        let mut reach = DirectedReach::new(outbound, inbound);

        // When either falls short, check exactly, with full costing.
        let retry_outbound = reach.outbound < max_reach;
        let retry_inbound = reach.inbound < max_reach;
        if retry_outbound || retry_inbound {
            let exact = self.exact(edge, edge_id, graph, costing, retry_outbound, retry_inbound)?;
            reach = DirectedReach::new(
                reach.outbound.max(exact.outbound),
                reach.inbound.max(exact.inbound),
            );
        }
        Ok(reach)
    }

    /// Queues a node and its doppelgängers on other levels (`Reach::enqueue`).
    fn enqueue<P: GraphTileProvider, C: Costing>(
        &mut self,
        node_id: GraphId,
        graph: GraphReader<'_, P>,
        costing: &C,
        hint: &Tile<P>,
    ) -> Result<(), GraphError> {
        if self.done.contains(&node_id) {
            return Ok(());
        }
        let Some(tile) = graph.tile_from(node_id, hint)? else {
            return Ok(());
        };
        let node = get_node(&tile, node_id)?;
        if !costing.node_allowed(node) {
            return Ok(());
        }
        self.queue.insert(node_id);
        for transition in tile.get_transitions(node) {
            let end = transition.corresponding_end_node_id();
            if self.done.contains(&end) {
                continue;
            }
            self.queue.insert(end);
            self.transitions += 1;
        }
        Ok(())
    }

    /// The exact reach, with full costing (`Reach::exact`).
    fn exact<P: GraphTileProvider, C: Costing>(
        &mut self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        graph: GraphReader<'_, P>,
        costing: &C,
        outbound: bool,
        inbound: bool,
    ) -> Result<DirectedReach, GraphError> {
        let Some(tile) = graph.tile(edge_id)? else {
            return Ok(DirectedReach::default());
        };
        if !costing.edge_filter(edge, &tile, DisallowMask::SHORTCUT) {
            return Ok(DirectedReach::default());
        }
        // Mainline bails if the end node can't be found.
        let Some(end_tile) = graph.tile_from(edge.end_node_id(), &tile)? else {
            return Ok(DirectedReach::default());
        };
        get_node(&end_tile, edge.end_node_id())?;

        let mut reach = DirectedReach::default();
        if outbound {
            self.clear();
            self.compute(Direction::Forward, edge_id, graph, costing)?;
            reach.outbound = clamp_count(self.done_count(), self.limit);
        }
        if inbound {
            self.clear();
            self.compute(Direction::Reverse, edge_id, graph, costing)?;
            reach.inbound = clamp_count(self.done_count(), self.limit);
        }
        Ok(DirectedReach::new(reach.outbound, reach.inbound))
    }

    /// A Dijkstra expansion from (or to) the edge (`Dijkstras::Compute`),
    /// stopping once enough nodes are settled.
    fn compute<P: GraphTileProvider, C: Costing>(
        &mut self,
        direction: Direction,
        edge_id: GraphId,
        graph: GraphReader<'_, P>,
        costing: &C,
    ) -> Result<(), GraphError> {
        // `GetExpansionHints`: 50 buckets per node of reach, each one cost unit wide.
        #[expect(
            clippy::cast_precision_loss,
            reason = "mainline converts the bucket count to a float"
        )]
        let range = (u64::from(self.limit) * 50) as f32;
        self.adjacency.reuse(range);
        self.seed(direction, edge_id, graph, costing)?;

        let mut decision = Recommendation::Continue;
        while decision != Recommendation::Stop {
            let labels = &self.labels;
            let Some(index) = self.adjacency.pop(|l| {
                labels
                    .get(l as usize)
                    .map_or(f32::MAX, |label| label.sortcost)
            }) else {
                break;
            };
            let Some(pred) = self.labels.get(index as usize).copied() else {
                break;
            };
            self.status.insert(pred.edge_id(), EdgeSet::Permanent);

            let opp_pred = if direction == Direction::Reverse {
                let Some(tile) = graph.tile(pred.edge_id())? else {
                    continue;
                };
                match graph.opposing(pred.edge_id(), &tile)? {
                    Some((opp_id, opp_tile)) => Some((opp_id, opp_tile)),
                    None => continue,
                }
            } else {
                None
            };

            decision = if self.done_count() < u64::from(self.limit) {
                Recommendation::Continue
            } else {
                Recommendation::Stop
            };
            // Mainline expands even when told to stop (only a prune skips it).
            let opp_pred_edge = match &opp_pred {
                Some((id, tile)) => Some(get_edge(tile, *id)?.clone()),
                None => None,
            };
            self.expand(
                direction,
                pred.end_node,
                &pred,
                index,
                opp_pred_edge.as_ref(),
                false,
                graph,
                costing,
            )?;
        }
        Ok(())
    }

    /// Adds the edge's label (`SetOriginLocations` and `SetDestinationLocations`
    /// for a location on the edge's start, at distance 0).
    fn seed<P: GraphTileProvider, C: Costing>(
        &mut self,
        direction: Direction,
        edge_id: GraphId,
        graph: GraphReader<'_, P>,
        costing: &C,
    ) -> Result<(), GraphError> {
        let base = costing.base();
        let avoided = match direction {
            Direction::Forward => base.avoid_as_origin_edge(edge_id, 0.0),
            Direction::Reverse => base.avoid_as_destination_edge(edge_id, 0.0),
        };
        if avoided {
            return Ok(());
        }
        let Some(tile) = graph.tile(edge_id)? else {
            return Ok(());
        };
        let edge = get_edge(&tile, edge_id)?;
        let Some((opp_id, opp_tile)) = graph.opposing(edge_id, &tile)? else {
            return Ok(());
        };
        // The fraction of the edge from the location (at its start) to the end, or to the start.
        let (end, path_distance) = match direction {
            Direction::Forward => (1.0, edge.length()),
            Direction::Reverse => (0.0, 0),
        };
        let result =
            costing.partial_edge_cost(edge, edge_id, Some(edge_id), &tile, None, 0.0, end)?;
        let (destonly_mask, destonly) =
            base.exempted_access_restrictions(edge, &tile, edge_id, TimeContext::NONE);
        let state = LabelState {
            predecessor: None,
            mode: base.travel_mode(),
            path_distance,
            closure_pruning: true,
            has_measured_speed: !result.flow_sources.is_empty(),
            internal_turn: InternalTurn::NoTurn,
            destonly,
        };
        let (label_id, label_edge) = match direction {
            Direction::Forward => (edge_id, edge.clone()),
            Direction::Reverse => (opp_id, get_edge(&opp_tile, opp_id)?.clone()),
        };
        let index = self.push_label(ReachLabel {
            base: EdgeLabel::new(label_id, &label_edge, state),
            predecessor: None,
            path_distance,
            cost: result.cost,
            sortcost: result.cost.cost,
            end_node: label_edge.end_node_id(),
            destonly_mask,
        });
        self.status.insert(label_id, EdgeSet::Temporary(index));
        Ok(())
    }

    fn push_label(&mut self, label: ReachLabel) -> u32 {
        let index = u32::try_from(self.labels.len()).unwrap_or(u32::MAX);
        let sortcost = label.sortcost;
        self.labels.push(label);
        self.adjacency.add(index, sortcost);
        index
    }

    /// Marks a node settled, with its doppelgängers on other levels (`Reach::ExpandingNode`).
    fn expanding_node<T: GraphTile>(
        &mut self,
        tile: &T,
        node_id: GraphId,
    ) -> Result<(), GraphError> {
        if self.done.insert(node_id) {
            let node = get_node(tile, node_id)?;
            for transition in tile.get_transitions(node) {
                self.done.insert(transition.corresponding_end_node_id());
                self.transitions += 1;
            }
        }
        Ok(())
    }

    /// Expands from a node (`Dijkstras::ExpandInner`).
    #[expect(clippy::too_many_arguments, reason = "mainline's signature")]
    #[expect(
        clippy::too_many_lines,
        reason = "a step by step port of mainline's loop"
    )]
    fn expand<P: GraphTileProvider, C: Costing>(
        &mut self,
        direction: Direction,
        node_id: GraphId,
        pred: &ReachLabel,
        pred_index: u32,
        opp_pred_edge: Option<&DirectedEdge>,
        from_transition: bool,
        graph: GraphReader<'_, P>,
        costing: &C,
    ) -> Result<(), GraphError> {
        let forward = direction == Direction::Forward;
        let Some(tile) = graph.tile(node_id)? else {
            return Ok(());
        };
        if !from_transition {
            self.expanding_node(&tile, node_id)?;
        }
        let node = get_node(&tile, node_id)?.clone();
        if !costing.node_allowed(&node) {
            return Ok(());
        }
        let base = costing.base();
        let access_mode = base.access_mode();
        let first = u64::from(node.edge_index());
        for (i, edge) in tile.get_outbound_edges_from_node(&node).iter().enumerate() {
            let edge_id = node_id
                .with_feature_index(first + i as u64)
                .map_err(valhalla_graphtile::tile_provider::GraphTileProviderError::from)?;
            let status = self.status.get(&edge_id).copied();
            let access = if forward {
                edge.forward_access()
            } else {
                edge.reverse_access()
            };
            if edge.is_shortcut()
                || status == Some(EdgeSet::Permanent)
                || access.is_disjoint(access_mode)
            {
                continue;
            }

            let opp = if forward {
                None
            } else {
                match graph.opposing(edge_id, &tile)? {
                    Some((opp_id, opp_tile)) => {
                        let opp_edge = get_edge(&opp_tile, opp_id)?.clone();
                        Some((opp_id, opp_tile, opp_edge))
                    }
                    None => continue,
                }
            };

            let mut edge_access = EdgeAccess::new(pred.destonly_mask);
            let allowed = match &opp {
                None => costing.allowed(
                    edge,
                    edge_id,
                    &tile,
                    pred,
                    TimeContext::NONE,
                    &mut edge_access,
                ),
                Some((opp_id, opp_tile, opp_edge)) => costing.allowed_reverse(
                    edge,
                    pred,
                    opp_edge,
                    *opp_id,
                    opp_tile,
                    TimeContext::NONE,
                    &mut edge_access,
                ),
            };
            if !allowed
                || base.restricted(
                    edge,
                    pred,
                    &self.labels,
                    &tile,
                    edge_id,
                    true,
                    TimeContext::NONE,
                    |_| {},
                )?
            {
                continue;
            }

            let (transition_cost, edge_cost, internal_turn) = match &opp {
                None => {
                    let transition = costing.transition_cost(
                        edge,
                        &tile,
                        &node,
                        pred,
                        graph.provider(),
                        edge_access.edge_destonly,
                    )?;
                    let cost = costing.edge_cost(edge, edge_id, &tile, None)?;
                    let turn = base.turn_type(pred.opp_local_idx(), &node, edge, None);
                    (transition, cost, turn)
                }
                Some((opp_id, opp_tile, opp_edge)) => {
                    let Some(opp_pred_edge) = opp_pred_edge else {
                        continue;
                    };
                    let transition = costing.transition_cost_reverse(
                        edge.local_edge_index(),
                        &node,
                        opp_edge,
                        opp_pred_edge,
                        pred,
                        opp_tile,
                        graph.provider(),
                        pred.has_measured_speed(),
                        pred.internal_turn(),
                        edge_access.edge_destonly,
                    )?;
                    let cost = costing.edge_cost(opp_edge, *opp_id, opp_tile, None)?;
                    let turn = base.turn_type(
                        edge.local_edge_index(),
                        &node,
                        opp_edge,
                        Some(opp_pred_edge),
                    );
                    (transition, cost, turn)
                }
            };
            let newcost = pred.cost + edge_cost.cost + transition_cost;
            let path_distance = pred.path_distance.wrapping_add(edge.length());

            if let Some(EdgeSet::Temporary(index)) = status {
                if let Some(label) = self.labels.get(index as usize).copied()
                    && newcost.cost < label.cost.cost
                {
                    let newsortcost = label.sortcost - (label.cost.cost - newcost.cost);
                    self.adjacency.decrease(index, label.sortcost, newsortcost);
                    if let Some(label) = self.labels.get_mut(index as usize) {
                        label.predecessor = Some(pred_index);
                        label.cost = newcost;
                        label.sortcost = newsortcost;
                        label.path_distance = path_distance;
                    }
                }
                continue;
            }

            let state = LabelState {
                predecessor: Some(pred_index),
                mode: base.travel_mode(),
                path_distance,
                closure_pruning: true,
                has_measured_speed: !edge_cost.flow_sources.is_empty(),
                internal_turn,
                destonly: edge_access.edge_destonly,
            };
            let index = self.push_label(ReachLabel {
                base: EdgeLabel::new(edge_id, edge, state),
                predecessor: Some(pred_index),
                path_distance,
                cost: newcost,
                sortcost: newcost.cost,
                end_node: edge.end_node_id(),
                destonly_mask: edge_access.destonly_access_restr_mask,
            });
            self.status.insert(edge_id, EdgeSet::Temporary(index));
        }

        if !from_transition {
            for transition in tile.get_transitions(&node) {
                self.expand(
                    direction,
                    transition.corresponding_end_node_id(),
                    pred,
                    pred_index,
                    opp_pred_edge,
                    true,
                    graph,
                    costing,
                )?;
            }
        }
        Ok(())
    }
}

/// `std::min(static_cast<uint32_t>(count), max_reach)`.
#[inline]
fn clamp_count(count: u64, max_reach: u32) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mainline casts the size_t count to uint32"
    )]
    let count = count as u32;
    count.min(max_reach)
}

/// The begin node of an edge: the end node of its opposing edge (`GetBeginNodeId`),
/// or `None` if the opposing edge's tile isn't in the tile set.
fn begin_node<P: GraphTileProvider>(
    graph: GraphReader<'_, P>,
    edge: &DirectedEdge,
    tile: &Tile<P>,
) -> Result<Option<GraphId>, GraphError> {
    let Some(end_tile) = graph.tile_from(edge.end_node_id(), tile)? else {
        return Ok(None);
    };
    let opp_id = opposing_in_end_tile(&end_tile, edge)?;
    Ok(Some(get_edge(&end_tile, opp_id)?.end_node_id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(i: u64) -> GraphId {
        GraphId::try_from_components(2, 763_926, i).unwrap()
    }

    #[test]
    fn queue_sets_pop_in_order_without_duplicates() {
        let mut set = QueueSet::default();
        for i in 0..4 {
            assert!(set.insert(id(i)));
        }
        assert!(!set.insert(id(2)));
        assert_eq!(set.pop_front(), Some(id(0)));
        assert!(set.insert(id(0)));
        let rest: Vec<GraphId> = core::iter::from_fn(|| set.pop_front()).collect();
        assert_eq!(rest, vec![id(1), id(2), id(3), id(0)]);
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn bucket_queue_pops_in_cost_order_and_lifo_within_a_bucket() {
        let costs = [5.5_f32, 1.2, 1.7, 0.0, 3000.0, 2.0];
        let mut queue = BucketQueue::default();
        queue.reuse(100.0);
        for (i, &c) in costs.iter().enumerate() {
            queue.add(u32::try_from(i).unwrap(), c);
        }
        let cost = |l: u32| costs[l as usize];
        let order: Vec<u32> = core::iter::from_fn(|| queue.pop(cost)).collect();
        // 1.2 and 1.7 share a bucket, so the later one comes first; 3000 overflows.
        assert_eq!(order, vec![3, 2, 1, 5, 0, 4]);
    }

    #[test]
    fn bucket_queue_moves_decreased_labels() {
        let mut costs = [10.0_f32, 20.0];
        let mut queue = BucketQueue::default();
        queue.reuse(100.0);
        queue.add(0, costs[0]);
        queue.add(1, costs[1]);
        queue.decrease(1, 20.0, 5.0);
        costs[1] = 5.0;
        let cost = |l: u32| costs[l as usize];
        assert_eq!(queue.pop(cost), Some(1));
        assert_eq!(queue.pop(cost), Some(0));
        assert_eq!(queue.pop(cost), None);
    }

    #[test]
    fn reach_fields_are_16_bits() {
        assert_eq!(
            DirectedReach::new(0x1_0005, 7),
            DirectedReach {
                outbound: 5,
                inbound: 7
            }
        );
    }
}
