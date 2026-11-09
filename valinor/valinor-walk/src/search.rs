//! The transfer search: node-based, multi-source, in both directions.
//!
//! See [`WalkSearch`]. The flat arrays are sized by the dense numbering of [`LazyGraph`];
//! everything is reused between searches, so a warm search allocates nothing.

use std::sync::Arc;

use valhalla_graphtile::GraphId;
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valinor_sif::Costing;
use valinor_transit::{
    Clock, Direction, StopId, TransferArrival, TransferOutcome, TransferRequest, TransferSearch,
    TransferSink,
};

use crate::error::WalkError;
use crate::graph::{GraphStats, INF, LazyGraph, MAX_SECS, NO_ARCS, NO_EDGE, prefix};
use crate::queue::DialQueue;
use crate::snap::StopSnaps;

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::needless_range_loop,
    clippy::many_single_char_names,
    clippy::similar_names,
    reason = "tests, with small sizes and readable indexing"
)]
#[path = "tile_tests.rs"]
mod tile_tests;

/// The "no such thing" value of stops, labels, and arena entries.
const NONE: u32 = u32::MAX;

/// How many labels to settle between interrupt checks.
const INTERRUPT_INTERVAL: u32 = 2048;

/// How many arena entries a run keeps room for between runs (8 bytes each).
const MAX_RETAINED_ARENA: usize = 1 << 22;

/// Two tiers of a node's label, as bits in the low two bits of its stamp.
const SETTLED_MASK: u32 = 3;

/// The two best labels of a node, with distinct sources: tier 0 is the best.
#[derive(Clone, Copy, Debug)]
#[repr(C, align(32))]
struct NodeLabel {
    /// The epoch the labels belong to, shifted left by 2, with a settled bit per tier.
    stamp: u32,
    clock: [i32; 2],
    source: [u32; 2],
    /// The arena entry of how each tier was reached.
    parent: [u32; 2],
}

const UNLABELED: NodeLabel = NodeLabel {
    stamp: 0,
    clock: [i32::MAX; 2],
    source: [NONE; 2],
    parent: [NONE; 2],
};

/// One way a label was reached, in the run's arena.
#[derive(Clone, Copy, Debug)]
struct ArenaEntry {
    /// The entry this one extends, or [`NONE`] if it starts at a seed.
    pred: u32,
    /// The arc followed; or [`SEED_ARC`] with the stop and side of the seed's connector.
    arc: u32,
}

/// The high bit of [`ArenaEntry::arc`] when an entry starts at a seed.
const SEED_ARC: u32 = 1 << 31;

/// A connector between a stop and one end of its edge, seen from a dense node.
///
/// `into` is the cost of walking from the node to the stop, and
/// `from` the cost of walking from the stop to the node.
/// Attachments of a node are a list through the flat [`WalkSearch::attachments`] array.
#[derive(Clone, Copy, Debug)]
struct Attachment {
    stop: u32,
    into: u16,
    from: u16,
    /// Which end of the stop's edge the node is: 0 for the tail, 1 for the head.
    side: u8,
    next: u32,
}

/// What a stop is attached to, in dense numbers.
#[derive(Clone, Copy, Debug)]
struct StopAttach {
    /// The tail and head node of the snapped edge.
    node: [u32; 2],
    /// Walking into the stop from each node.
    into: [u16; 2],
    /// Walking from the stop to each node.
    from: [u16; 2],
    /// The snapped edge's weight along it, and along its opposing edge.
    weights: [u16; 2],
    /// The snapped edge and its opposing edge.
    edges: [u32; 2],
    /// The fraction along the snapped edge.
    percent: f32,
    /// The slice of `same_edge_stops` holding stops on the same (undirected) edge.
    group: (u32, u32),
}

/// The best arrival at a stop so far.
#[derive(Clone, Copy, Debug)]
struct StopLabel {
    stamp: u32,
    clock: i32,
    source: u32,
    /// The arena entry at the node the stop was entered from, or [`NONE`] for a direct walk.
    chain: u32,
    side: u8,
}

/// A handle to a walk [`WalkSearch::search`] found, valid until the next [`begin_run`].
///
/// [`begin_run`]: valinor_transit::TransferSearch::begin_run
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkHandle {
    stop: u32,
    source: u32,
    chain: u32,
    side: u8,
}

/// A walk between two stops, resolved into real edges in the direction of travel.
///
/// `edges` may be empty when the two stops share a node and the walk has no length.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedWalk {
    /// The stop the walk begins at, in real travel order.
    pub from: StopId,
    /// The stop the walk ends at.
    pub to: StopId,
    /// The directed edges walked, in order.
    pub edges: Vec<GraphId>,
    /// The whole-part cost of each edge walked, in integer seconds, parallel to `edges`.
    ///
    /// Partial first and last edges are the split parts charged by the search.
    pub edge_secs: Vec<u16>,
    /// The fraction along the first edge where the walk starts.
    pub begin_percent: f32,
    /// The fraction along the last edge where the walk ends.
    pub end_percent: f32,
}

impl ResolvedWalk {
    /// The walk's total cost, in seconds.
    pub fn secs(&self) -> u32 {
        self.edge_secs.iter().map(|&s| u32::from(s)).sum()
    }
}

/// What one [`WalkSearch::search`] did, for profiling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// Labels settled (at most two per node).
    pub labels_settled: u64,
    /// Distinct nodes settled, tier 0.
    pub nodes_settled: u64,
    /// Arcs relaxed (including those that didn't improve anything).
    pub arcs_relaxed: u64,
    /// Queue entries popped, including stale ones.
    pub pops: u64,
    /// Entries in the walk arena at the end of the search.
    pub arena_entries: u64,
    /// Arrivals reported to the sink.
    pub arrivals: u64,
}

/// A multi-source street search over raw tiles.
///
/// Costs are integer seconds per directed edge; see [`crate::graph`] for how they are chosen.
/// Each node keeps its two best labels with distinct sources, so that a stop reachable only
/// through its own seed still gets the best arrival from another seed
/// (the "`u ≠ v`" of [`TransferSearch`]).
///
/// # Performance
///
/// One [`WalkSearch`] is one thread's working memory: flat node labels with version stamps,
/// a bucket queue over seconds, and the memoized tile data of [`LazyGraph`].
/// No per-edge hashing or tile lookups happen on a warm search.
///
/// Create one per worker and reuse it across queries.
pub struct WalkSearch<P: GraphTileProvider, C: Costing> {
    graph: LazyGraph<P, C>,
    snaps: Option<Arc<StopSnaps>>,

    labels: Vec<NodeLabel>,
    /// `stamp` value of the labels currently in force, a multiple of 4.
    ///
    /// A new value per run with [`Self::set_run_pruning`] on (the default), per search otherwise.
    stamp_base: u32,
    /// Whether labels carry over between the searches of one run; see [`Self::set_run_pruning`].
    run_pruning: bool,
    /// Whether to use the request's change margin; see [`Self::set_own_seed_pruning`].
    own_seed_pruning: bool,
    /// The stamp of the current search's stop labels.
    stop_stamp: u32,
    /// By dense node: the first attachment, or [`NONE`].
    node_attach: Vec<u32>,
    attachments: Vec<Attachment>,
    /// Nodes that have an attachment, to clear when snaps are replaced.
    attached_nodes: Vec<u32>,
    stops: Vec<Option<StopAttach>>,
    same_edge_stops: Vec<u32>,

    queue: DialQueue,
    /// The walk tree of the current MCR run.
    arena: Vec<ArenaEntry>,
    stop_labels: Vec<StopLabel>,
    touched_stops: Vec<u32>,

    direction: Direction,
    stats: SearchStats,
    epoch: u32,
}

impl<P: GraphTileProvider, C: Costing> WalkSearch<P, C> {
    /// A search over `provider`, costed by `costing` (one per travel mode, one per thread).
    pub fn new(provider: P, costing: C) -> Self {
        Self {
            graph: LazyGraph::new(provider, costing),
            snaps: None,
            labels: Vec::new(),
            stamp_base: 4,
            run_pruning: true,
            own_seed_pruning: true,
            stop_stamp: 1,
            node_attach: Vec::new(),
            attachments: Vec::new(),
            attached_nodes: Vec::new(),
            stops: Vec::new(),
            same_edge_stops: Vec::new(),
            queue: DialQueue::default(),
            arena: Vec::new(),
            stop_labels: Vec::new(),
            touched_stops: Vec::new(),
            direction: Direction::Forward,
            stats: SearchStats::default(),
            epoch: 1,
        }
    }

    /// Which stops this search can reach.
    ///
    /// Call this whenever the query's snapshot has different stops: a pointer comparison
    /// skips the work when it hasn't changed.
    ///
    /// # Errors
    ///
    /// Refuses `snaps` made on a different tile build, and fails on unreadable tiles.
    #[expect(clippy::too_many_lines, reason = "one pass over the stops")]
    pub fn bind(&mut self, snaps: Arc<StopSnaps>) -> Result<(), WalkError> {
        if self.snaps.as_ref().is_some_and(|s| Arc::ptr_eq(s, &snaps)) {
            return Ok(());
        }
        self.clear_attachments();
        let stop_count = snaps.stop_count();
        self.stops.clear();
        self.stops.resize(stop_count, None);
        self.stop_labels.clear();
        self.stop_labels.resize(
            stop_count,
            StopLabel {
                stamp: 0,
                clock: i32::MAX,
                source: NONE,
                chain: NONE,
                side: 0,
            },
        );
        self.same_edge_stops.clear();
        let mut groups: Vec<(u32, u32)> = Vec::new();
        for stop in 0..stop_count {
            let stop_id = StopId::new(u32::try_from(stop).map_err(|_| WalkError::TooLarge)?);
            let Some(point) = snaps.point(stop_id) else {
                continue;
            };
            // Every snap's edge, checked directly, so a tile set from another build is refused.
            let build = self
                .graph
                .build_id_of(point.edge)?
                .ok_or(WalkError::SnappedEdgeMissing(point.edge))?;
            if build != snaps.build_id() {
                return Err(WalkError::BuildMismatch {
                    snaps: snaps.build_id(),
                    tiles: build,
                });
            }
            let Some(edge) = self.graph.describe_edge(point.edge)? else {
                continue;
            };
            let w = edge.weights;
            let into_tail = prefix(w.forward, point.percent_along);
            let from_head = sub_weight(w.forward, into_tail);
            let into_head = prefix(w.backward, 1.0 - point.percent_along);
            let from_tail = sub_weight(w.backward, into_head);
            let attach = StopAttach {
                node: [edge.tail, edge.head],
                into: [into_tail, into_head],
                from: [from_tail, from_head],
                weights: [w.forward, w.backward],
                edges: [edge.edge_dense, edge.opposing_dense],
                percent: point.percent_along,
                group: (0, 0),
            };
            self.stops[stop] = Some(attach);
            let canonical = edge.edge_dense.min(edge.opposing_dense);
            groups.push((
                canonical,
                u32::try_from(stop).map_err(|_| WalkError::TooLarge)?,
            ));
            self.ensure_node_arrays();
            for (side, (&node, (&into, &from))) in attach
                .node
                .iter()
                .zip(attach.into.iter().zip(attach.from.iter()))
                .enumerate()
            {
                let attachment =
                    u32::try_from(self.attachments.len()).map_err(|_| WalkError::TooLarge)?;
                self.attachments.push(Attachment {
                    stop: u32::try_from(stop).map_err(|_| WalkError::TooLarge)?,
                    into,
                    from,
                    side: u8::from(side == 1),
                    next: self.node_attach[node as usize],
                });
                if self.node_attach[node as usize] == NONE {
                    self.attached_nodes.push(node);
                }
                self.node_attach[node as usize] = attachment;
            }
        }
        groups.sort_unstable();
        // Group stops on the same undirected edge, for direct walks along it.
        let mut start = 0;
        while start < groups.len() {
            let edge = groups[start].0;
            let mut end = start + 1;
            while end < groups.len() && groups[end].0 == edge {
                end += 1;
            }
            if end - start > 1 {
                let begin =
                    u32::try_from(self.same_edge_stops.len()).map_err(|_| WalkError::TooLarge)?;
                let count = u32::try_from(end - start).map_err(|_| WalkError::TooLarge)?;
                for &(_, stop) in &groups[start..end] {
                    self.same_edge_stops.push(stop);
                }
                for &(_, stop) in &groups[start..end] {
                    if let Some(attach) = self.stops[stop as usize].as_mut() {
                        attach.group = (begin, count);
                    }
                }
            }
            start = end;
        }
        self.snaps = Some(snaps);
        Ok(())
    }

    /// Clears the attachments of the previously bound snaps.
    fn clear_attachments(&mut self) {
        for &node in &self.attached_nodes {
            if let Some(slot) = self.node_attach.get_mut(node as usize) {
                *slot = NONE;
            }
        }
        self.attached_nodes.clear();
        self.attachments.clear();
    }

    /// Grows the node arrays to cover every node numbered so far.
    fn ensure_node_arrays(&mut self) {
        let nodes = self.graph.node_count();
        if self.node_attach.len() < nodes {
            self.node_attach.resize(nodes, NONE);
            self.labels.resize(nodes, UNLABELED);
        }
    }

    /// The graph's work so far, across every search on this worker.
    pub fn graph_stats(&self) -> GraphStats {
        self.graph.stats()
    }

    /// The work of the most recent [`search`](TransferSearch::search).
    pub fn last_search_stats(&self) -> SearchStats {
        self.stats
    }

    /// Bytes retained by this search's label, queue, and arena arrays.
    pub fn working_memory_bytes(&self) -> usize {
        self.labels.capacity() * size_of::<NodeLabel>()
            + self.node_attach.capacity() * size_of::<u32>()
            + self.attachments.capacity() * size_of::<Attachment>()
            + self.arena.capacity() * size_of::<ArenaEntry>()
            + self.stop_labels.capacity() * size_of::<StopLabel>()
            + self.same_edge_stops.capacity() * size_of::<u32>()
    }

    /// Forgets every node label.
    fn start_stamp(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 || self.epoch >= (1 << 30) {
            // Rare wrap: forget every stamp.
            for label in &mut self.labels {
                label.stamp = 0;
            }
            self.epoch = 1;
        }
        self.stamp_base = self.epoch << 2;
    }

    /// Whether node labels stay between the searches of one [`begin_run`] (the default).
    ///
    /// With this on, a later round's walk is cut wherever a node already has two labels
    /// from earlier rounds, no later than the new one and with sources distinct from it:
    /// such a label can't reach a stop earlier than what was reported before,
    /// and a walk label that isn't an improvement in an earlier round is not one now.
    /// Turning it off makes every search start from nothing (for measurement).
    ///
    /// [`begin_run`]: valinor_transit::TransferSearch::begin_run
    pub fn set_run_pruning(&mut self, enabled: bool) {
        self.run_pruning = enabled;
    }

    /// Whether a node's second label is kept only when a seed could need it (the default).
    ///
    /// The second label exists so a stop can be reached from another seed than itself.
    /// A walk to a seed's own stop only helps if it arrives before that seed's clock plus the
    /// request's change margin, so a second label that far behind the first can go.
    /// In particular, a timetable with no minimum change times needs no second labels at all.
    /// Turning this off keeps every label, for measurement.
    pub fn set_own_seed_pruning(&mut self, enabled: bool) {
        self.own_seed_pruning = enabled;
    }
}

/// `whole - part` of an edge's weight, respecting [`INF`].
#[inline]
const fn sub_weight(whole: u16, part: u16) -> u16 {
    if whole == INF { INF } else { whole - part }
}

/// Adds a seed's stop or node labels straight into the arrays of [`WalkSearch`].
///
/// Reported separately from the settle loop because a seed's connector is a real walk
/// on the seed's own edge, not an arc of the graph.
#[inline]
#[expect(clippy::too_many_arguments, reason = "the flat arrays of one search")]
fn relax(
    labels: &mut [NodeLabel],
    queue: &mut DialQueue,
    arena: &mut Vec<ArenaEntry>,
    stamp_base: u32,
    margin: i32,
    node: u32,
    clock: i32,
    source: u32,
    pred: u32,
    arc: u32,
) {
    let label = &mut labels[node as usize];
    let push = |arena: &mut Vec<ArenaEntry>| {
        arena.push(ArenaEntry { pred, arc });
        u32::try_from(arena.len() - 1).unwrap_or(NONE)
    };
    if label.stamp & !SETTLED_MASK != stamp_base {
        *label = NodeLabel {
            stamp: stamp_base,
            clock: [clock, i32::MAX],
            source: [source, NONE],
            parent: [push(arena), NONE],
        };
        queue.push(clock, node);
        return;
    }
    if source == label.source[0] {
        if clock < label.clock[0] {
            label.clock[0] = clock;
            label.parent[0] = push(arena);
            label.stamp &= !1;
            queue.push(clock, node);
        }
        return;
    }
    if clock < label.clock[0] {
        // The old best moves down a tier, along with whether it has been settled
        // (it may have been, in an earlier round of the run),
        // unless it is too far behind to matter; see `WalkSearch::set_own_seed_pruning`.
        if label.clock[0] < clock.saturating_add(margin) {
            label.clock[1] = label.clock[0];
            label.source[1] = label.source[0];
            label.parent[1] = label.parent[0];
            let was_settled = label.stamp & 1;
            label.stamp = (label.stamp & !SETTLED_MASK) | (was_settled << 1);
        } else {
            label.clock[1] = i32::MAX;
            label.source[1] = NONE;
            label.parent[1] = NONE;
            label.stamp &= !SETTLED_MASK;
        }
        label.clock[0] = clock;
        label.source[0] = source;
        label.parent[0] = push(arena);
        queue.push(clock, node);
        return;
    }
    if clock >= label.clock[0].saturating_add(margin) {
        return;
    }
    if source == label.source[1] {
        if clock < label.clock[1] {
            label.clock[1] = clock;
            label.parent[1] = push(arena);
            label.stamp &= !2;
            queue.push(clock, node);
        }
        return;
    }
    if clock < label.clock[1] {
        label.clock[1] = clock;
        label.source[1] = source;
        label.parent[1] = push(arena);
        label.stamp &= !2;
        queue.push(clock, node);
    }
}

/// One [`TransferSearch::search`], with the borrows of [`WalkSearch`] split apart.
struct Run<'a, P: GraphTileProvider, C: Costing> {
    search: &'a mut WalkSearch<P, C>,
    dir: usize,
    upper_bound: i32,
    margin: i32,
}

impl<P: GraphTileProvider, C: Costing> TransferSearch for WalkSearch<P, C> {
    type Parent = WalkHandle;
    type Walk = ResolvedWalk;
    type Error = WalkError;

    fn begin_run(&mut self, direction: Direction) {
        self.direction = direction;
        self.arena.clear();
        // A rare long run shouldn't hold on to its peak (32 MB here, as Thor limits its labels).
        if self.arena.capacity() > MAX_RETAINED_ARENA {
            self.arena.shrink_to(MAX_RETAINED_ARENA);
        }
        self.start_stamp();
    }

    fn walk(&self, parent: WalkHandle) -> ResolvedWalk {
        self.resolve(parent)
    }

    fn search<K: TransferSink<WalkHandle>>(
        &mut self,
        request: &TransferRequest<'_>,
        sink: &mut K,
    ) -> Result<TransferOutcome, WalkError> {
        let dir = usize::from(request.direction == Direction::Backward);
        if !self.run_pruning {
            self.start_stamp();
        }
        self.stop_stamp = self.stop_stamp.wrapping_add(1);
        if self.stop_stamp == 0 {
            for label in &mut self.stop_labels {
                label.stamp = 0;
            }
            self.stop_stamp = 1;
        }
        self.stats = SearchStats::default();
        self.touched_stops.clear();
        self.ensure_node_arrays();
        let upper_bound = request.upper_bound.seconds();
        let own_seed_pruning = self.own_seed_pruning;

        // The buckets must span every clock from the earliest seed until the bound,
        // or until the latest seed plus a longest edge.
        let min_clock = request.seeds.iter().map(|s| s.clock.seconds()).min();
        let Some(min_clock) = min_clock else {
            return Ok(TransferOutcome::Completed);
        };
        let max_clock = request
            .seeds
            .iter()
            .map(|s| s.clock.seconds())
            .max()
            .unwrap_or(min_clock);
        let span = u64::try_from(i64::from(max_clock) - i64::from(min_clock)).unwrap_or(0)
            + u64::from(MAX_SECS)
            + 2;
        self.queue.reset(min_clock, span);

        let mut run = Run {
            search: self,
            dir,
            upper_bound,
            margin: if own_seed_pruning {
                request.change_margin
            } else {
                TransferRequest::REPORT_ALL
            },
        };
        for seed in request.seeds {
            run.seed(seed.stop, seed.clock.seconds());
        }
        let outcome = run.settle_all(request)?;
        if outcome == TransferOutcome::Interrupted {
            self.queue.clear();
            return Ok(outcome);
        }
        self.report(sink);
        self.stats.arena_entries = self.arena.len() as u64;
        Ok(TransferOutcome::Completed)
    }
}

impl<P: GraphTileProvider, C: Costing> Run<'_, P, C> {
    /// Starts labels at a seed: at the nodes it attaches to, and at other stops on its edge.
    fn seed(&mut self, stop: StopId, clock: i32) {
        if clock >= self.upper_bound {
            return;
        }
        let stop_index = stop.into_inner();
        let Some(attach) = self
            .search
            .stops
            .get(stop_index as usize)
            .copied()
            .flatten()
        else {
            return;
        };
        self.search.ensure_node_arrays();
        // Leaving the stop: on a forward search the stop is left along each direction of its
        // edge; on a backward search the search moves against the edge,
        // which a rider travels into the stop.
        let leave = if self.dir == 0 {
            attach.from
        } else {
            attach.into
        };
        for side in 0..2 {
            let cost = leave[side];
            if cost == INF {
                continue;
            }
            let at = clock.saturating_add(i32::from(cost));
            if at < self.upper_bound {
                relax(
                    &mut self.search.labels,
                    &mut self.search.queue,
                    &mut self.search.arena,
                    self.search.stamp_base,
                    self.margin,
                    attach.node[side],
                    at,
                    stop_index,
                    NONE,
                    SEED_ARC | (stop_index << 1) | u32::from(side == 1),
                );
            }
        }
        self.direct_walks(stop_index, attach, clock);
    }

    /// Considers walks along the seed's own edge to the other stops on it.
    fn direct_walks(&mut self, source: u32, attach: StopAttach, clock: i32) {
        let (begin, count) = attach.group;
        let dir = self.dir;
        let upper_bound = self.upper_bound;
        let search = &mut *self.search;
        for at in begin..begin + count {
            let other = search.same_edge_stops[at as usize];
            if other == source {
                continue;
            }
            let Some(v) = search.stops[other as usize] else {
                continue;
            };
            // In the frame of the source's edge: the fraction along it of the other stop.
            let q = if v.edges[0] == attach.edges[0] {
                v.percent
            } else {
                1.0 - v.percent
            };
            // A forward seed walks from the source; a backward search's seed is the
            // destination of the real walk. `real_from` and `real_to` are in real travel order.
            let (real_from, real_to) = if dir == 0 {
                (attach.percent, q)
            } else {
                (q, attach.percent)
            };
            let cost = direct_cost(attach.weights, real_from, real_to);
            if cost == INF {
                continue;
            }
            let arrival = clock.saturating_add(i32::from(cost));
            if arrival < upper_bound {
                update_stop(search, other, arrival, source, NONE, 0);
            }
        }
    }

    /// Settles queue entries until none are left or the bound is reached.
    fn settle_all(&mut self, request: &TransferRequest<'_>) -> Result<TransferOutcome, WalkError> {
        let mut since_check = 0u32;
        let search = &mut *self.search;
        while let Some((clock, node)) = search.queue.pop() {
            search.stats.pops += 1;
            since_check += 1;
            if since_check >= INTERRUPT_INTERVAL {
                since_check = 0;
                if (request.interrupt)() {
                    return Ok(TransferOutcome::Interrupted);
                }
            }
            let stamp = search.labels[node as usize].stamp;
            if stamp & !SETTLED_MASK != search.stamp_base {
                continue;
            }
            let label = search.labels[node as usize];
            let mut settled = [(0i32, 0u32, 0u32); 2];
            let mut count = 0;
            for tier in 0..2 {
                if label.clock[tier] == clock && stamp & (1 << tier) == 0 {
                    settled[count] = (clock, label.source[tier], label.parent[tier]);
                    count += 1;
                    search.labels[node as usize].stamp |= 1 << tier;
                    if tier == 0 {
                        search.stats.nodes_settled += 1;
                    }
                }
            }
            if count == 0 {
                continue;
            }
            search.stats.labels_settled += count as u64;
            if search.node_attach[node as usize] != NONE {
                reach_attached(search, node, &settled[..count], self.dir, self.upper_bound);
            }
            let mut first = search.graph.first_arc[node as usize];
            if first == NO_ARCS {
                continue;
            }
            if first == u32::MAX {
                search.graph.materialize(node)?;
                search.ensure_node_arrays();
                first = search.graph.first_arc[node as usize];
                if first == NO_ARCS {
                    continue;
                }
            }
            let mut index = first as usize;
            loop {
                let arc = search.graph.arcs[index];
                search.stats.arcs_relaxed += 1;
                let weight = arc.secs[self.dir];
                if weight != INF {
                    for &(at, source, parent) in &settled[..count] {
                        let next = at.saturating_add(i32::from(weight));
                        if next < self.upper_bound {
                            relax(
                                &mut search.labels,
                                &mut search.queue,
                                &mut search.arena,
                                search.stamp_base,
                                self.margin,
                                arc.head(),
                                next,
                                source,
                                parent,
                                u32::try_from(index).unwrap_or(NONE),
                            );
                        }
                    }
                }
                if arc.is_last() {
                    break;
                }
                index += 1;
            }
        }
        Ok(TransferOutcome::Completed)
    }
}

/// Offers each settled tier's clock to the stops attached to `node`.
#[inline]
fn reach_attached<P: GraphTileProvider, C: Costing>(
    search: &mut WalkSearch<P, C>,
    node: u32,
    settled: &[(i32, u32, u32)],
    dir: usize,
    upper_bound: i32,
) {
    let mut at = search.node_attach[node as usize];
    while at != NONE {
        let attachment = search.attachments[at as usize];
        at = attachment.next;
        let cost = if dir == 0 {
            attachment.into
        } else {
            attachment.from
        };
        if cost == INF {
            continue;
        }
        for &(clock, source, parent) in settled {
            if source == attachment.stop {
                continue;
            }
            let arrival = clock.saturating_add(i32::from(cost));
            if arrival < upper_bound {
                update_stop(
                    search,
                    attachment.stop,
                    arrival,
                    source,
                    parent,
                    attachment.side,
                );
            }
        }
    }
}

/// Keeps the best arrival at `stop` so far.
#[inline]
fn update_stop<P: GraphTileProvider, C: Costing>(
    search: &mut WalkSearch<P, C>,
    stop: u32,
    clock: i32,
    source: u32,
    chain: u32,
    side: u8,
) {
    let stamp = search.stop_stamp;
    let label = &mut search.stop_labels[stop as usize];
    if label.stamp != stamp {
        *label = StopLabel {
            stamp,
            clock,
            source,
            chain,
            side,
        };
        search.touched_stops.push(stop);
        return;
    }
    if clock < label.clock {
        *label = StopLabel {
            stamp,
            clock,
            source,
            chain,
            side,
        };
    }
}

impl<P: GraphTileProvider, C: Costing> WalkSearch<P, C> {
    /// Sends each stop's best arrival to the sink.
    fn report<K: TransferSink<WalkHandle>>(&mut self, sink: &mut K) {
        for &stop in &self.touched_stops {
            let label = self.stop_labels[stop as usize];
            self.stats.arrivals += 1;
            sink.reach(TransferArrival {
                stop: StopId::new(stop),
                source: StopId::new(label.source),
                clock: Clock::new(label.clock),
                parent: WalkHandle {
                    stop,
                    source: label.source,
                    chain: label.chain,
                    side: label.side,
                },
            });
        }
    }

    /// Follows a handle to its walk, in real travel order.
    fn resolve(&self, handle: WalkHandle) -> ResolvedWalk {
        let forward = self.direction == Direction::Forward;
        let dir = usize::from(!forward);
        let (from_stop, to_stop) = if forward {
            (handle.source, handle.stop)
        } else {
            (handle.stop, handle.source)
        };
        let empty = ResolvedWalk {
            from: StopId::new(from_stop),
            to: StopId::new(to_stop),
            edges: Vec::new(),
            edge_secs: Vec::new(),
            begin_percent: 0.0,
            end_percent: 0.0,
        };
        let source = self.stops.get(handle.source as usize).copied().flatten();
        let target = self.stops.get(handle.stop as usize).copied().flatten();
        let (Some(source), Some(target)) = (source, target) else {
            return empty;
        };

        // A walk along one edge between two stops never touches a node label.
        if handle.chain == NONE {
            let (from, to) = if forward {
                (source, target)
            } else {
                (target, source)
            };
            let (begin, end, along, cost) = same_edge_walk(from, to);
            return ResolvedWalk {
                edges: self.graph.edge_id(along).into_iter().collect(),
                edge_secs: vec![cost],
                begin_percent: begin,
                end_percent: end,
                ..empty
            };
        }

        // The graph part of the walk, from the seed outward in search order.
        let mut arcs = Vec::new();
        let mut seed_side = 0u8;
        let mut at = handle.chain;
        while at != NONE {
            let Some(entry) = self.arena.get(at as usize) else {
                return empty;
            };
            if entry.arc & SEED_ARC != 0 {
                seed_side = u8::from(entry.arc & 1 == 1);
                break;
            }
            arcs.push(entry.arc);
            at = entry.pred;
        }
        arcs.reverse();
        let mut walked: Vec<(u32, u16)> = Vec::with_capacity(arcs.len() + 2);
        for &arc in &arcs {
            let cold = self.graph.cold_arcs[arc as usize];
            if cold.edge != NO_EDGE {
                let cost = self.graph.arcs[arc as usize].secs[dir];
                walked.push((if forward { cold.edge } else { cold.opposing }, cost));
            }
        }
        if !forward {
            walked.reverse();
        }

        // Which directed edge a connector covers, and the fraction where it meets the stop.
        let frame = |attach: StopAttach, side: u8| -> (u32, f32) {
            if side == 0 {
                (attach.edges[0], attach.percent)
            } else {
                (attach.edges[1], 1.0 - attach.percent)
            }
        };
        let (begin, end) = if forward {
            // A forward seed leaving by its head walks the edge from its fraction,
            // and one leaving by its tail walks the opposing edge.
            let (seed_edge, seed_fraction) = frame(source, 1 - seed_side);
            let (target_edge, target_fraction) = frame(target, handle.side);
            walked.insert(0, (seed_edge, source.from[usize::from(seed_side)]));
            walked.push((target_edge, target.into[usize::from(handle.side)]));
            (seed_fraction, target_fraction)
        } else {
            // A backward seed is the end of the real walk, entered by the tail along
            // the edge, or by the head along the opposing edge.
            let (seed_edge, seed_fraction) = frame(source, seed_side);
            let (target_edge, target_fraction) = frame(target, 1 - handle.side);
            walked.insert(0, (target_edge, target.from[usize::from(handle.side)]));
            walked.push((seed_edge, source.into[usize::from(seed_side)]));
            (target_fraction, seed_fraction)
        };
        let mut walk = ResolvedWalk {
            edges: Vec::with_capacity(walked.len()),
            edge_secs: Vec::with_capacity(walked.len()),
            begin_percent: begin,
            end_percent: end,
            ..empty
        };
        for (edge, cost) in walked {
            if let Some(id) = self.graph.edge_id(edge) {
                walk.edges.push(id);
                walk.edge_secs.push(cost);
            }
        }
        trim_zero_length(&mut walk);
        walk
    }
}

/// The begin and end fractions, the directed edge, and the cost of a walk from stop `from` to
/// stop `to` along their shared edge.
fn same_edge_walk(from: StopAttach, to: StopAttach) -> (f32, f32, u32, u16) {
    let q = if to.edges[0] == from.edges[0] {
        to.percent
    } else {
        1.0 - to.percent
    };
    let p = from.percent;
    let cost = direct_cost(from.weights, p, q);
    if p <= q {
        (p, q, from.edges[0], cost)
    } else {
        (1.0 - p, 1.0 - q, from.edges[1], cost)
    }
}

/// The cost of walking along an edge, in real travel order from fraction `real_from` to
/// `real_to` of the edge (with `weights` of the edge and its opposing edge),
/// or [`INF`] if that direction isn't allowed.
#[inline]
fn direct_cost(weights: [u16; 2], real_from: f32, real_to: f32) -> u16 {
    if real_from <= real_to {
        if weights[0] == INF {
            return INF;
        }
        prefix(weights[0], real_to) - prefix(weights[0], real_from)
    } else {
        if weights[1] == INF {
            return INF;
        }
        prefix(weights[1], 1.0 - real_to) - prefix(weights[1], 1.0 - real_from)
    }
}

/// Removes the parts of a walk with no length: an end at a node leaves a zero-length edge.
fn trim_zero_length(walk: &mut ResolvedWalk) {
    if walk.begin_percent >= 1.0 && walk.edges.len() > 1 {
        walk.edges.remove(0);
        walk.edge_secs.remove(0);
        walk.begin_percent = 0.0;
    }
    if walk.end_percent <= 0.0 && walk.edges.len() > 1 {
        walk.edges.pop();
        walk.edge_secs.pop();
        walk.end_percent = 1.0;
    }
    let degenerate = walk.edges.len() == 1 && (walk.begin_percent >= walk.end_percent - 1e-6);
    if degenerate {
        walk.edges.clear();
        walk.edge_secs.clear();
    }
}
