//! Bidirectional A* (mainline's `thor::BidirectionalAStar`), the default route search.
//!
//! A forward search from the origin and a reverse search from the destination
//! take turns, each expanding the cheaper of their next labels.
//! When an edge settled by one search has its opposing edge reached by the other,
//! the two trees connect there; the search goes on to a cost threshold
//! (the first connection's cost plus `threshold_delta`, or more when looking for alternates)
//! and then forms paths from the best connections, which it recosts from the origin
//! (the two halves were costed in opposite directions).
//!
//! This follows mainline step by step, including what decides which of two nearly equal
//! paths wins: the queue's bucket order, ties in the connection costs, the hierarchy limits
//! (which both directions use up in step), the reach-based pruning once a connection is
//! found, and shortcuts (taken in sync by both directions).
//! What mainline does only for diagnostics (the expansion callback) is left out.

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};
use valhalla_proto::options::{DateTimeType, HasAlternates, ReverseTimeTracking};
use valhalla_proto::{HierarchyLimits, Location, Options};
use valinor_sif::datetime::time_zone;
use valinor_sif::{
    Cost, Costing, EdgeAccess, INVALID_RESTRICTION, InternalTurn, Label, TimeInfo, TravelMode,
    UNLIMITED_TRANSITIONS, recost_forward, stop_expanding,
};

use crate::alternates::{
    filter_alternates_by_stretch, get_max_sharing, validate_alternate_by_local_optimality,
    validate_alternate_by_sharing, validate_alternate_by_stretch,
};
use crate::config::LEVELS;
use crate::edge_status::{EdgeSet, EdgeStatus, EdgeStatusInfo};
use crate::error::ThorError;
use crate::geo::{AStarHeuristic, PointLL};
use crate::graph;
use crate::label::{INVALID_LABEL, NewLabel, ThorLabel};
use crate::path::PathEdge;
use crate::queue::BucketQueue;
use crate::search::{
    BUCKET_COUNT, INITIAL_LABELS, SearchContext, UNIT_SIZE, add_double, candidate_id, candidates,
    clear_labels, find_percent_along, first_candidate_point, time_context,
};
use crate::time::location_time_info;

/// Mainline's factor on the bee-line time for the reverse tree's start time
/// (`kReverseTTHeuristicFactor`).
const REVERSE_TT_HEURISTIC_FACTOR: f64 = 2.1;

/// The search parameters from Thor's configuration.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BidirectionalParams {
    pub(crate) threshold_delta: f32,
    pub(crate) alternative_cost_extend: f32,
    pub(crate) alternative_iterations_delta: u32,
    pub(crate) extended_search: bool,
}

/// A connection between the trees: an edge reached by both (`CandidateConnection`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CandidateConnection {
    pub(crate) edge_id: GraphId,
    pub(crate) opp_edge_id: GraphId,
    pub(crate) cost: f32,
}

/// One direction's search tree.
#[derive(Debug, Default)]
struct Tree {
    labels: Vec<ThorLabel>,
    queue: BucketQueue,
    status: EdgeStatus,
    heuristic: AStarHeuristic,
    limits: Vec<HierarchyLimits>,
}

impl Tree {
    fn clear(&mut self, max_labels: usize) {
        clear_labels(&mut self.labels, max_labels);
        self.status.clear_retaining(max_labels);
    }

    #[inline]
    fn limit(&self, level: u8) -> Option<&HierarchyLimits> {
        self.limits.get(usize::from(level))
    }

    /// `StopExpanding(limits[level], dist)`, false for levels without limits.
    #[inline]
    fn stop(&self, level: usize, dist: f32) -> bool {
        self.limits
            .get(level)
            .is_some_and(|l| stop_expanding(l, dist))
    }

    #[inline]
    fn unlimited(&self, level: u8) -> bool {
        self.limit(level)
            .is_none_or(|l| l.max_up_transitions == UNLIMITED_TRANSITIONS)
    }
}

/// The edge `ExpandInner` looks at (`EdgeMetadata`).
struct Meta<'e> {
    edge: &'e DirectedEdge,
    edge_id: GraphId,
}

/// Bidirectional A* and its reusable storage.
#[derive(Debug, Default)]
#[expect(clippy::struct_excessive_bools, reason = "mainline's search state")]
pub(crate) struct BidirectionalAStar {
    fwd: Tree,
    rev: Tree,
    ignore_hierarchy_limits: bool,
    cost_diff: f32,
    cost_threshold: f32,
    iterations_threshold: usize,
    desired_paths_count: usize,
    best_connections: Vec<CandidateConnection>,
    pruning_disabled_at_origin: bool,
    pruning_disabled_at_destination: bool,
    /// Whether a formed path has a ferry (`has_ferry_`).
    pub(crate) has_ferry: bool,
    /// Whether not-thru edges are pruned (`not_thru_pruning_`); off on the second pass.
    pub(crate) not_thru_pruning: bool,
    mode: TravelMode,
    params: Option<BidirectionalParams>,
    // Scratch space for forming paths.
    path_edges: Vec<GraphId>,
    recovered: Vec<GraphId>,
    recovered_inner: Vec<GraphId>,
}

/// The reverse tree's start time when only the forward one has a time (or the other way
/// around for arrive by): mainline's `EstimateReverseStartTime`, which assumes a bee line
/// at the costing's top speed, stretched by a factor.
fn estimate_reverse_start_time<P: GraphTileProvider, C: Costing>(
    ctx: &SearchContext<'_, P, C>,
    origin: &Location,
    destination: &Location,
    time_info: &TimeInfo,
    arrive_by: bool,
) -> Result<TimeInfo, ThorError> {
    let origin_pe = candidates(origin)
        .first()
        .ok_or(ThorError::NoCandidates(0))?;
    let dest_pe = candidates(destination)
        .first()
        .ok_or(ThorError::NoCandidates(1))?;
    let origin_pt = crate::search::point_of(origin_pe.ll.as_ref());
    let dest_pt = crate::search::point_of(dest_pe.ll.as_ref());
    let dist = origin_pt.distance(dest_pt);
    let seconds = ctx
        .costing
        .base()
        .bee_line_time_estimate(dist, REVERSE_TT_HEURISTIC_FACTOR);
    let edge = if arrive_by { origin_pe } else { dest_pe };
    let edge_id = candidate_id(edge)?;
    let Some(tile) = graph::tile(ctx.provider, edge_id)? else {
        return Ok(TimeInfo::INVALID);
    };
    let directed = graph::edge(&tile, edge_id)?;
    let Some(end_tile) = graph::tile(ctx.provider, directed.end_node_id())? else {
        return Ok(TimeInfo::INVALID);
    };
    let end_node = graph::node(&end_tile, directed.end_node_id())?;
    let tz_node = if edge.percent_along > 0.5 {
        end_node
    } else {
        // The start node: the opposing edge's end node, which mainline reads from
        // the edge's own tile (where every edge's start node is).
        let opp = graph::opposing_in_end_tile(&end_tile, directed)?;
        let opp_edge = graph::edge(&end_tile, opp)?;
        graph::node(&tile, opp_edge.end_node_id())?
    };
    let tz = tz_index(tz_node);
    #[expect(
        clippy::cast_precision_loss,
        reason = "mainline passes the seconds as a float"
    )]
    let seconds = seconds as f32;
    Ok(if arrive_by {
        time_info.reverse(seconds, tz)
    } else {
        time_info.forward(seconds, tz)
    })
}

/// A node's time zone index (0 for none).
#[inline]
fn tz_index(node: &NodeInfo) -> u32 {
    node.time_zone_index().map_or(0, |i| u32::from(i.get()))
}

impl BidirectionalAStar {
    pub(crate) fn new() -> Self {
        Self {
            not_thru_pruning: true,
            ..Self::default()
        }
    }

    /// Clears a search, keeping its storage (`Clear`).
    /// The labels and edge statuses both trees have room for (for tests).
    #[cfg(test)]
    pub(crate) fn retained(&self) -> usize {
        [&self.fwd, &self.rev]
            .iter()
            .map(|tree| tree.labels.capacity() + tree.status.retained_edges())
            .sum()
    }

    /// Clears the search, keeping room for `max_labels` labels and edges in each tree (`Clear`).
    pub(crate) fn clear(&mut self, max_labels: usize) {
        self.fwd.clear(max_labels);
        self.rev.clear(max_labels);
        self.has_ferry = false;
        self.not_thru_pruning = true;
        self.pruning_disabled_at_origin = false;
        self.pruning_disabled_at_destination = false;
        self.ignore_hierarchy_limits = false;
    }

    /// Aims the heuristics and resets the queues (`Init`).
    fn init<C: Costing>(&mut self, costing: &C, origll: PointLL, destll: PointLL) {
        let factor = costing.astar_cost_factor();
        self.fwd.heuristic.init(destll, factor);
        self.rev.heuristic.init(origll, factor);
        if self.fwd.labels.capacity() == 0 {
            self.fwd.labels.reserve(INITIAL_LABELS);
            self.rev.labels.reserve(INITIAL_LABELS);
        }
        #[expect(clippy::cast_precision_loss, reason = "mainline's float range")]
        let range = (BUCKET_COUNT * UNIT_SIZE) as f32;
        let (mincostf, _) = self.fwd.heuristic.cost_to(origll);
        self.fwd.queue.reuse(mincostf, range, UNIT_SIZE);
        let (mincostr, _) = self.rev.heuristic.cost_to(destll);
        self.rev.queue.reuse(mincostr, range, UNIT_SIZE);
        self.fwd.status.clear();
        self.rev.status.clear();
        self.cost_diff = mincostf - mincostr;
        self.best_connections.clear();
        self.cost_threshold = f32::MAX;
        self.iterations_threshold = usize::MAX;
        let limits = costing.base().hierarchy_limits();
        self.ignore_hierarchy_limits = limits
            .iter()
            .take(LEVELS)
            .skip(1)
            .all(|l| l.max_up_transitions == UNLIMITED_TRANSITIONS);
        self.fwd.limits.clear();
        self.fwd.limits.extend_from_slice(limits);
        self.rev.limits.clear();
        self.rev.limits.extend_from_slice(limits);
    }

    /// Finds the best path, and alternates if the options ask for them (`GetBestPath`).
    ///
    /// Returns no paths if the search fails.
    #[expect(clippy::too_many_lines, reason = "mainline's main loop, step by step")]
    pub(crate) fn best_path<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        params: &BidirectionalParams,
        origin: &mut Location,
        destination: &mut Location,
        options: &Options,
    ) -> Result<Vec<Vec<PathEdge>>, ThorError> {
        self.mode = ctx.costing.base().travel_mode();
        self.params = Some(*params);
        self.desired_paths_count = 1;
        if let Some(HasAlternates::Alternates(n)) = options.has_alternates
            && n > 0
        {
            self.desired_paths_count += usize::try_from(n).unwrap_or(usize::MAX - 1);
        }
        let origin_new = first_candidate_point(origin, 0)?;
        let destination_new = first_candidate_point(destination, 1)?;
        self.init(ctx.costing, origin_new, destination_new);

        let date_time_type = DateTimeType::try_from(options.date_time_type).ok();
        let invariant = date_time_type == Some(DateTimeType::Invariant);
        let arrive_by = date_time_type == Some(DateTimeType::ArriveBy);
        let heuristic = ReverseTimeTracking::try_from(options.reverse_time_tracking)
            .is_ok_and(|r| r == ReverseTimeTracking::RttHeuristic);
        let (forward_time_info, reverse_time_info) = if arrive_by {
            let reverse = location_time_info(ctx.provider, destination, ctx.now)?;
            let forward = if heuristic {
                estimate_reverse_start_time(ctx, origin, destination, &reverse, true)?
            } else {
                TimeInfo::INVALID
            };
            (forward, reverse)
        } else {
            let forward = location_time_info(ctx.provider, origin, ctx.now)?;
            let reverse = if invariant {
                location_time_info(ctx.provider, destination, ctx.now)?
            } else if heuristic {
                estimate_reverse_start_time(ctx, origin, destination, &forward, false)?
            } else {
                TimeInfo::INVALID
            };
            (forward, reverse)
        };

        self.set_origin(ctx, origin, &forward_time_info)?;
        self.set_destination(ctx, destination, &reverse_time_info)?;

        let mut forward_pred_idx: Option<u32> = Some(0);
        let mut reverse_pred_idx: Option<u32> = Some(0);
        let placeholder = candidate_id(
            candidates(origin)
                .first()
                .ok_or(ThorError::NoCandidates(0))?,
        )?;
        let mut fwd_pred = ThorLabel::empty(placeholder);
        let mut rev_pred = ThorLabel::empty(placeholder);
        let mut expand_forward = true;
        let mut expand_reverse = true;
        let mut n = 0;
        loop {
            ctx.check_interrupt(&mut n)?;
            if self.fwd.labels.len() + self.rev.labels.len() > self.iterations_threshold {
                return self.form_path(ctx, options, origin, destination, &forward_time_info);
            }

            if expand_forward {
                forward_pred_idx = self.fwd.queue.pop(&self.fwd.labels);
                if let Some(idx) = forward_pred_idx {
                    fwd_pred = *label_at(&self.fwd.labels, idx)?;
                    self.fwd.status.update(fwd_pred.edge_id, EdgeSet::Permanent);
                    if fwd_pred.sortcost + self.cost_diff > self.cost_threshold {
                        return self.form_path(
                            ctx,
                            options,
                            origin,
                            destination,
                            &forward_time_info,
                        );
                    }
                    if self.connects(&fwd_pred, true)
                        && self.set_forward_connection(ctx, &fwd_pred)?
                    {
                        continue;
                    }
                } else {
                    if !self.best_connections.is_empty() {
                        return self.form_path(
                            ctx,
                            options,
                            origin,
                            destination,
                            &forward_time_info,
                        );
                    }
                    if !params.extended_search || !self.pruning_disabled_at_destination {
                        return Ok(Vec::new());
                    }
                }
            }
            if expand_reverse {
                reverse_pred_idx = self.rev.queue.pop(&self.rev.labels);
                if let Some(idx) = reverse_pred_idx {
                    rev_pred = *label_at(&self.rev.labels, idx)?;
                    self.rev.status.update(rev_pred.edge_id, EdgeSet::Permanent);
                    if rev_pred.sortcost > self.cost_threshold {
                        return self.form_path(
                            ctx,
                            options,
                            origin,
                            destination,
                            &forward_time_info,
                        );
                    }
                    if self.connects(&rev_pred, false)
                        && self.set_reverse_connection(ctx, &rev_pred)?
                    {
                        continue;
                    }
                } else {
                    if !self.best_connections.is_empty() {
                        return self.form_path(
                            ctx,
                            options,
                            origin,
                            destination,
                            &forward_time_info,
                        );
                    }
                    if !params.extended_search || !self.pruning_disabled_at_origin {
                        return Ok(Vec::new());
                    }
                }
            }

            let forward_exhausted = forward_pred_idx.is_none();
            let reverse_exhausted = reverse_pred_idx.is_none();
            if forward_exhausted && reverse_exhausted {
                return Ok(Vec::new());
            }

            // Use up the hierarchy limits in both directions together.
            let mut force_forward = false;
            let mut force_reverse = false;
            if !self.ignore_hierarchy_limits {
                for level in (1..LEVELS).rev() {
                    let rev_stop = self.rev.stop(level, rev_pred.distance);
                    let fwd_stop = self.fwd.stop(level, fwd_pred.distance);
                    if rev_stop && !fwd_stop {
                        force_forward = true;
                        break;
                    } else if fwd_stop && !rev_stop {
                        force_reverse = true;
                        break;
                    }
                }
            }

            if !forward_exhausted
                && ((!force_reverse && (fwd_pred.sortcost + self.cost_diff) < rev_pred.sortcost)
                    || force_forward
                    || reverse_exhausted)
            {
                expand_forward = true;
                expand_reverse = false;
                let Some(pred_idx) = forward_pred_idx else {
                    continue;
                };

                if (fwd_pred.not_thru() && fwd_pred.not_thru_pruning())
                    || (!self.ignore_hierarchy_limits
                        && self
                            .fwd
                            .stop(usize::from(fwd_pred.end_node.level()), fwd_pred.distance))
                {
                    continue;
                }

                // Reach-based pruning (https://repub.eur.nl/pub/16100/ei2009-10.pdf).
                if !unbounded(self.cost_threshold)
                    && let Some(prev) = fwd_pred.predecessor_index()
                {
                    let Some(tile) = graph::tile(ctx.provider, fwd_pred.end_node)? else {
                        continue;
                    };
                    let end_ll = graph::node_ll(&tile, graph::node(&tile, fwd_pred.end_node)?);
                    let prev_cost = self.fwd.labels.get(prev).map_or(0.0, |l| l.cost.cost);
                    let route_lower_bound =
                        prev_cost + fwd_pred.transition_cost.cost + rev_pred.sortcost
                            - self.rev.heuristic.cost_to(end_ll).0;
                    if route_lower_bound > self.cost_threshold {
                        continue;
                    }
                }

                self.expand::<true, P, C>(
                    ctx,
                    fwd_pred.end_node,
                    &mut fwd_pred,
                    pred_idx,
                    None,
                    &forward_time_info,
                    invariant,
                )?;
            } else {
                expand_forward = false;
                expand_reverse = true;
                let Some(pred_idx) = reverse_pred_idx else {
                    continue;
                };

                if (rev_pred.not_thru() && rev_pred.not_thru_pruning())
                    || (!self.ignore_hierarchy_limits
                        && self
                            .rev
                            .stop(usize::from(rev_pred.end_node.level()), rev_pred.distance))
                {
                    continue;
                }

                let Some(opp_pred_id) = rev_pred.opp_edge_id else {
                    continue;
                };
                let Some(opp_pred_tile) = graph::tile(ctx.provider, opp_pred_id)? else {
                    continue;
                };
                let opp_pred_edge = graph::edge(&opp_pred_tile, opp_pred_id)?;

                if !unbounded(self.cost_threshold)
                    && let Some(prev) = rev_pred.predecessor_index()
                {
                    let Some(tile) = graph::tile(ctx.provider, rev_pred.end_node)? else {
                        continue;
                    };
                    let end_ll = graph::node_ll(&tile, graph::node(&tile, rev_pred.end_node)?);
                    let prev_cost = self.rev.labels.get(prev).map_or(0.0, |l| l.cost.cost);
                    let route_lower_bound =
                        prev_cost + rev_pred.transition_cost.cost + fwd_pred.sortcost
                            - self.fwd.heuristic.cost_to(end_ll).0;
                    if route_lower_bound > self.cost_threshold {
                        continue;
                    }
                }

                self.expand::<false, P, C>(
                    ctx,
                    rev_pred.end_node,
                    &mut rev_pred,
                    pred_idx,
                    Some(opp_pred_edge),
                    &reverse_time_info,
                    invariant,
                )?;
            }
        }
    }

    /// Whether a settled edge meets the other tree: its opposing edge is settled there,
    /// or is one of the other tree's starting edges still in its queue.
    fn connects(&self, pred: &ThorLabel, forward: bool) -> bool {
        let other = if forward { &self.rev } else { &self.fwd };
        let Some(opp) = pred.opp_edge_id else {
            return false;
        };
        let status = other.status.get(opp);
        status.set() == EdgeSet::Permanent
            || (status.set() == EdgeSet::Temporary
                && label_at(&other.labels, status.index())
                    .is_ok_and(|l| l.predecessor == INVALID_LABEL))
    }

    /// Records a connection's cost and moves the threshold (the shared part of
    /// `SetForwardConnection` and `SetReverseConnection`).
    fn add_connection(&mut self, connection: CandidateConnection) {
        let Some(params) = self.params else {
            return;
        };
        let c = connection.cost;
        let improves = self.best_connections.first().is_none_or(|b| c < b.cost);
        if unbounded(self.cost_threshold) || improves {
            if self.desired_paths_count == 1 {
                self.cost_threshold = c + params.threshold_delta;
            } else {
                self.cost_threshold = params.alternative_cost_extend * c + params.threshold_delta;
                self.iterations_threshold = (self.fwd.labels.len() + self.rev.labels.len())
                    .saturating_add(
                        usize::try_from(params.alternative_iterations_delta).unwrap_or(usize::MAX),
                    );
            }
        }
        self.best_connections.push(connection);
        let last = self.best_connections.len() - 1;
        if self.best_connections.first().is_some_and(|b| c < b.cost) {
            self.best_connections.swap(0, last);
        }
    }

    fn set_forward_connection<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        pred: &ThorLabel,
    ) -> Result<bool, ThorError> {
        let Some(oppedge) = pred.opp_edge_id else {
            return Ok(false);
        };
        let opp_status = self.rev.status.get(oppedge);
        let opp_pred = *label_at(&self.rev.labels, opp_status.index())?;
        if pred.internal_turn() != InternalTurn::NoTurn {
            return Ok(false);
        }
        if pred.on_complex_restriction()
            && is_bridging_edge_restricted(
                ctx,
                &self.fwd.labels,
                &self.rev.labels,
                pred,
                &opp_pred,
            )?
        {
            return Ok(false);
        }
        let c = if let Some(prev) = pred.predecessor_index() {
            label_at(&self.fwd.labels, index_u32(prev))?.cost.cost
                + opp_pred.cost.cost
                + pred.transition_cost.cost
        } else {
            let oppcost = match opp_pred.predecessor_index() {
                Some(i) => label_at(&self.rev.labels, index_u32(i))?.cost.cost,
                None => 0.0,
            };
            pred.cost.cost + oppcost + opp_pred.transition_cost.cost
        };
        self.add_connection(CandidateConnection {
            edge_id: pred.edge_id,
            opp_edge_id: oppedge,
            cost: c,
        });
        Ok(true)
    }

    fn set_reverse_connection<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        rev_pred: &ThorLabel,
    ) -> Result<bool, ThorError> {
        let Some(fwd_edge_id) = rev_pred.opp_edge_id else {
            return Ok(false);
        };
        let fwd_status = self.fwd.status.get(fwd_edge_id);
        let fwd_pred = *label_at(&self.fwd.labels, fwd_status.index())?;
        if rev_pred.internal_turn() != InternalTurn::NoTurn {
            return Ok(false);
        }
        if rev_pred.on_complex_restriction()
            && is_bridging_edge_restricted(
                ctx,
                &self.fwd.labels,
                &self.rev.labels,
                &fwd_pred,
                rev_pred,
            )?
        {
            return Ok(false);
        }
        let c = if let Some(prev) = rev_pred.predecessor_index() {
            label_at(&self.rev.labels, index_u32(prev))?.cost.cost
                + fwd_pred.cost.cost
                + rev_pred.transition_cost.cost
        } else {
            let oppcost = match fwd_pred.predecessor_index() {
                Some(i) => label_at(&self.fwd.labels, index_u32(i))?.cost.cost,
                None => 0.0,
            };
            rev_pred.cost.cost + oppcost + fwd_pred.transition_cost.cost
        };
        self.add_connection(CandidateConnection {
            edge_id: fwd_edge_id,
            opp_edge_id: rev_pred.edge_id,
            cost: c,
        });
        Ok(true)
    }
}

/// Whether the cost threshold is still unset (mainline's `std::numeric_limits<float>::max()`).
#[inline]
#[expect(clippy::float_cmp, reason = "an exact sentinel")]
fn unbounded(threshold: f32) -> bool {
    threshold == f32::MAX
}

/// The label at an index.
#[inline]
pub(crate) fn label_at(labels: &[ThorLabel], index: u32) -> Result<&ThorLabel, ThorError> {
    usize::try_from(index)
        .ok()
        .and_then(|i| labels.get(i))
        .ok_or(ThorError::DanglingLabel(index))
}

/// A label index as mainline stores it.
#[inline]
pub(crate) fn index_u32(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(INVALID_LABEL)
}

/// The graph ID of the `i`th edge from `first`.
#[inline]
pub(crate) fn nth_edge(first: GraphId, i: usize) -> Result<GraphId, ThorError> {
    Ok(first
        .with_feature_index(first.feature_index() + i as u64)
        .map_err(GraphTileProviderError::from)?)
}

impl BidirectionalAStar {
    /// Expands from a node: the edges leaving it, and those at the other levels' copies of it
    /// (`Expand`).
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "mainline's Expand, step by step"
    )]
    fn expand<const FORWARD: bool, P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        node: GraphId,
        pred: &mut ThorLabel,
        pred_idx: u32,
        opp_pred_edge: Option<&DirectedEdge>,
        time_info: &TimeInfo,
        invariant: bool,
    ) -> Result<(), ThorError> {
        let Some(tile) = graph::tile(ctx.provider, node)? else {
            return Ok(());
        };
        let nodeinfo = graph::node(&tile, node)?;
        // The time moves even if it's invariant, for the time zone.
        let seconds_offset = if invariant { 0.0 } else { pred.cost.secs };
        let tz = tz_index(nodeinfo);
        let offset_time = if FORWARD {
            time_info.forward(seconds_offset, tz)
        } else {
            time_info.reverse(seconds_offset, tz)
        };

        // At a node with an access restriction (a barrier), only a U-turn is possible.
        if !ctx.costing.node_allowed(nodeinfo) {
            let Some(pred_tile) = graph::tile_for(ctx.provider, &tile, pred.edge_id)? else {
                return Ok(());
            };
            pred.set_deadend(true);
            if let Some((opp_id, opp_tile)) =
                graph::opposing(ctx.provider, &pred_tile, pred.edge_id)?
            {
                let opp_edge = graph::edge(&opp_tile, opp_id)?;
                let mut shortcuts = 0;
                self.expand_inner::<FORWARD, P, C>(
                    ctx,
                    pred,
                    opp_pred_edge,
                    nodeinfo,
                    pred_idx,
                    &Meta {
                        edge: opp_edge,
                        edge_id: opp_id,
                    },
                    &mut shortcuts,
                    &opp_tile,
                    &offset_time,
                )?;
            }
            return Ok(());
        }

        let mut disable_uturn = false;
        let mut shortcuts = 0;
        let mut uturn: Option<usize> = None;
        let first = graph::first_edge_id(node, nodeinfo)?;
        let edges = tile.get_outbound_edges_from_node(nodeinfo);
        for (i, edge) in edges.iter().enumerate() {
            // Wait with the U-turn: it's only for dead ends.
            let is_uturn = pred.opp_local_idx() == edge.local_edge_index();
            if is_uturn {
                uturn = Some(i);
            }
            let expanded = !is_uturn
                && self.expand_inner::<FORWARD, P, C>(
                    ctx,
                    pred,
                    opp_pred_edge,
                    nodeinfo,
                    pred_idx,
                    &Meta {
                        edge,
                        edge_id: nth_edge(first, i)?,
                    },
                    &mut shortcuts,
                    &tile,
                    &offset_time,
                )?;
            disable_uturn = expanded || disable_uturn;
        }

        // Transitions: expand from the node's copies on other levels.
        for trans in tile.get_transitions(nodeinfo) {
            let end = trans.corresponding_end_node_id();
            let tree = if FORWARD { &self.fwd } else { &self.rev };
            if !trans.is_up()
                && !self.ignore_hierarchy_limits
                && tree.stop(usize::from(end.level()), pred.distance)
            {
                continue;
            }
            let Some(trans_tile) = graph::tile(ctx.provider, end)? else {
                continue;
            };
            let tree = if FORWARD {
                &mut self.fwd
            } else {
                &mut self.rev
            };
            if let Some(limits) = tree.limits.get_mut(usize::from(node.level())) {
                limits.up_transition_count = limits
                    .up_transition_count
                    .saturating_add(u32::from(trans.is_up()));
            }
            let trans_node = graph::node(&trans_tile, end)?;
            let trans_first = graph::first_edge_id(end, trans_node)?;
            let mut trans_shortcuts = 0;
            for (i, edge) in trans_tile
                .get_outbound_edges_from_node(trans_node)
                .iter()
                .enumerate()
            {
                let expanded = self.expand_inner::<FORWARD, P, C>(
                    ctx,
                    pred,
                    opp_pred_edge,
                    trans_node,
                    pred_idx,
                    &Meta {
                        edge,
                        edge_id: nth_edge(trans_first, i)?,
                    },
                    &mut trans_shortcuts,
                    &trans_tile,
                    &offset_time,
                )?;
                disable_uturn = expanded || disable_uturn;
            }
        }

        // Nothing else was possible: a dead end, so try the U-turn.
        if !disable_uturn
            && let Some(i) = uturn
            && let Some(edge) = edges.get(i)
        {
            pred.set_deadend(true);
            self.expand_inner::<FORWARD, P, C>(
                ctx,
                pred,
                opp_pred_edge,
                nodeinfo,
                pred_idx,
                &Meta {
                    edge,
                    edge_id: nth_edge(first, i)?,
                },
                &mut shortcuts,
                &tile,
                &offset_time,
            )?;
        }
        Ok(())
    }

    /// Considers one edge from `pred` (`ExpandInner`): labels it, or improves its label.
    ///
    /// Returns whether the edge could be expanded (or was settled already), which rules out
    /// a U-turn at the node; an edge that is labeled but will be pruned as not-thru doesn't.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "mainline's ExpandInner, step by step"
    )]
    fn expand_inner<const FORWARD: bool, P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        pred: &ThorLabel,
        opp_pred_edge: Option<&DirectedEdge>,
        nodeinfo: &NodeInfo,
        pred_idx: u32,
        meta: &Meta<'_>,
        shortcuts: &mut u32,
        tile: &P::TileHandle,
        time_info: &TimeInfo,
    ) -> Result<bool, ThorError> {
        // A regular edge superseded by a shortcut the search took.
        if *shortcuts & graph::superseded_mask(meta.edge) != 0 {
            return Ok(false);
        }
        let ignore_hierarchy_limits = self.ignore_hierarchy_limits;
        let not_thru_pruning_on = self.not_thru_pruning;
        let mode = self.mode;
        let base = ctx.costing.base();
        let (this, other) = if FORWARD {
            (&mut self.fwd, &self.rev)
        } else {
            (&mut self.rev, &self.fwd)
        };
        let edge_count = tile.directed_edges().len();

        // The end node's tile and the opposing edge, found when first needed.
        let mut opp: Option<(P::TileHandle, GraphId)> = None;
        let get_opp = |opp: &mut Option<(P::TileHandle, GraphId)>| -> Result<bool, ThorError> {
            if opp.is_some() {
                return Ok(true);
            }
            let Some(t2) = graph::end_tile(ctx.provider, tile, meta.edge)? else {
                return Ok(false);
            };
            let id = graph::opposing_in_end_tile(&t2, meta.edge)?;
            *opp = Some((t2, id));
            Ok(true)
        };

        // Take shortcuts in step with the other direction, and only once the search has
        // stopped expanding on the level they skip.
        if meta.edge.is_shortcut() {
            if ignore_hierarchy_limits || !get_opp(&mut opp)? {
                return Ok(false);
            }
            let Some((_, opp_id)) = &opp else {
                return Ok(false);
            };
            let opp_set = other.status.get(*opp_id).set();
            let next_level = usize::from(meta.edge_id.level()) + 1;
            if (opp_set != EdgeSet::Skipped && this.stop(next_level, pred.distance))
                || opp_set == EdgeSet::Permanent
                || opp_set == EdgeSet::Temporary
            {
                *shortcuts |= graph::shortcut_mask(meta.edge);
            } else {
                this.status.set(
                    meta.edge_id,
                    EdgeStatusInfo::new(EdgeSet::Skipped, 0),
                    edge_count,
                );
                return Ok(false);
            }
        }

        if this.status.get(meta.edge_id).set() == EdgeSet::Permanent {
            return Ok(true);
        }

        if !FORWARD {
            // Cheaply skip edges the reverse search can't use before finding the opposing edge.
            if meta.edge.reverse_access().is_disjoint(base.access_mode()) {
                return Ok(false);
            }
            if !get_opp(&mut opp)? {
                return Ok(false);
            }
        }
        // The reverse search's opposing edge, which it costs.
        let rev: Option<(P::TileHandle, GraphId)> = if FORWARD { None } else { opp.take() };
        let opp_edge: Option<(&P::TileHandle, GraphId, &DirectedEdge)> = match &rev {
            Some((t2, id)) => Some((t2, *id, graph::edge(t2, *id)?)),
            None => None,
        };

        let time = time_context(time_info);
        let mut access = EdgeAccess::new(pred.destonly_access_restr_mask);
        let Tree { labels, status, .. } = &mut *this;
        if FORWARD {
            if !ctx
                .costing
                .allowed(meta.edge, meta.edge_id, tile, pred, time, &mut access)
                || base.restricted(
                    meta.edge,
                    pred,
                    labels.as_slice(),
                    tile,
                    meta.edge_id,
                    true,
                    time,
                    |id| status.update(id, EdgeSet::Unreached),
                )?
            {
                return Ok(false);
            }
        } else {
            let Some((t2, opp_id, opp_edge)) = opp_edge else {
                return Ok(false);
            };
            if !ctx.costing.allowed_reverse(
                meta.edge,
                pred,
                opp_edge,
                opp_id,
                t2,
                time,
                &mut access,
            ) || base.restricted(
                meta.edge,
                pred,
                labels.as_slice(),
                tile,
                meta.edge_id,
                false,
                time,
                |id| status.update(id, EdgeSet::Unreached),
            )? {
                return Ok(false);
            }
        }

        let edge_cost = match opp_edge {
            None => ctx.edge_cost(meta.edge, meta.edge_id, tile, time_info)?,
            Some((t2, opp_id, opp_edge)) => ctx.edge_cost(opp_edge, opp_id, t2, time_info)?,
        };
        let has_measured_speed = !edge_cost.flow_sources.is_empty();
        let mut newcost = pred.cost + edge_cost.cost;
        let transition_cost = match opp_edge {
            None => ctx.costing.transition_cost(
                meta.edge,
                tile,
                nodeinfo,
                pred,
                ctx.provider,
                access.edge_destonly,
            )?,
            Some((t2, _, opp_edge)) => {
                let Some(opp_pred_edge) = opp_pred_edge else {
                    return Ok(false);
                };
                ctx.costing.transition_cost_reverse(
                    meta.edge.local_edge_index(),
                    nodeinfo,
                    opp_edge,
                    opp_pred_edge,
                    pred,
                    t2,
                    ctx.provider,
                    has_measured_speed,
                    pred.internal_turn(),
                    access.edge_destonly,
                )?
            }
        };
        newcost += transition_cost;

        // A cheaper path to an edge in the queue: update its label.
        // (The A* heuristic doesn't change, so the sort cost moves with the cost.)
        let status = this.status.get(meta.edge_id);
        if status.set() == EdgeSet::Temporary {
            let idx = status.index();
            if let Some(lab) = usize::try_from(idx)
                .ok()
                .and_then(|i| this.labels.get_mut(i))
                && newcost.cost < lab.cost.cost
            {
                let newsortcost = lab.sortcost - (lab.cost.cost - newcost.cost);
                this.queue.decrease(idx, lab.sortcost, newsortcost);
                lab.update(
                    pred_idx,
                    newcost,
                    newsortcost,
                    transition_cost,
                    access.restriction_idx,
                );
            }
            return Ok(true);
        }

        if rev.is_none() && !get_opp(&mut opp)? {
            return Ok(false);
        }
        let ((Some((t2, opp_id)), _) | (None, Some((t2, opp_id)))) = (&rev, &opp) else {
            return Ok(false);
        };
        let end_node = meta.edge.end_node_id();
        let end_node_ll = graph::node_ll(t2, graph::node(t2, end_node)?);
        let (heuristic, mut dist) = this.heuristic.cost_to(end_node_ll);
        let sortcost = newcost.cost + heuristic;

        // Not-thru pruning is off on the second pass; edges are still labeled so that the
        // trees can meet on them.
        let not_thru_pruning =
            not_thru_pruning_on && (pred.not_thru_pruning() || !meta.edge.no_thru());

        let idx = index_u32(this.labels.len());
        if !this.unlimited(meta.edge_id.level()) {
            // Hierarchy limits use the distance from this search's start instead.
            dist = other.heuristic.distance(end_node_ll);
        }
        let (closed_edge, internal_turn) = match opp_edge {
            None => (
                base.is_closed(meta.edge),
                base.turn_type(pred.opp_local_idx(), nodeinfo, meta.edge, None),
            ),
            Some((_, _, opp_edge)) => (
                base.is_closed(opp_edge),
                base.turn_type(
                    meta.edge.local_edge_index(),
                    nodeinfo,
                    opp_edge,
                    opp_pred_edge,
                ),
            ),
        };
        this.labels.push(ThorLabel::new(
            meta.edge,
            NewLabel {
                predecessor: pred_idx,
                edge_id: meta.edge_id,
                opp_edge_id: Some(*opp_id),
                cost: newcost,
                sortcost,
                distance: dist,
                mode,
                transition_cost,
                not_thru_pruning,
                closure_pruning: pred.closure_pruning() || !closed_edge,
                has_measured_speed,
                internal_turn,
                restriction_idx: access.restriction_idx,
                destonly: access.edge_destonly,
                destonly_access_restr_mask: access.destonly_access_restr_mask,
            },
        ));
        this.queue.add(idx, sortcost);
        this.status.set(
            meta.edge_id,
            EdgeStatusInfo::new(EdgeSet::Temporary, idx),
            edge_count,
        );

        // Labeled, but not expanded further if it's a not-thru edge that will be pruned:
        // then a U-turn is still allowed.
        Ok(!(pred.not_thru_pruning() && meta.edge.no_thru()))
    }

    /// Adds the origin's candidate edges to the forward search (`SetOrigin`).
    fn set_origin<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        origin: &mut Location,
        time_info: &TimeInfo,
    ) -> Result<(), ThorError> {
        let base = ctx.costing.base();
        // Only skip inbound edges (at a node) if there are others.
        let has_other_edges = candidates(origin).iter().any(|e| !e.end_node);
        let mut closest_tz: Option<u32> = None;
        for edge in candidates(origin) {
            if has_other_edges && edge.end_node {
                continue;
            }
            let edge_id = candidate_id(edge)?;
            #[expect(clippy::cast_possible_truncation, reason = "mainline takes a float")]
            let percent_along = edge.percent_along as f32;
            if base.avoid_as_origin_edge(edge_id, percent_along) {
                continue;
            }
            let Some(tile) = graph::tile(ctx.provider, edge_id)? else {
                continue;
            };
            let directed = graph::edge(&tile, edge_id)?;
            let Some(end_tile) = graph::tile(ctx.provider, directed.end_node_id())? else {
                continue;
            };
            let nodeinfo = graph::node(&end_tile, directed.end_node_id())?;
            let partial =
                ctx.partial_edge_cost(directed, edge_id, &tile, time_info, percent_along, 1.0)?;
            let mut cost = partial.cost;
            if closest_tz.is_none() {
                closest_tz = Some(tz_index(nodeinfo));
            }
            // The snap distance, as if covered at 1 m/s (mainline's "maximum penalty").
            cost.cost = add_double(cost.cost, edge.distance);
            let node_ll = graph::node_ll(&end_tile, nodeinfo);
            let mut dist = self.fwd.heuristic.distance(node_ll);
            let sortcost = cost.cost + self.fwd.heuristic.cost(dist);

            let idx = index_u32(self.fwd.labels.len());
            self.fwd.status.set(
                edge_id,
                EdgeStatusInfo::new(EdgeSet::Temporary, idx),
                tile.directed_edges().len(),
            );
            if !self.fwd.unlimited(edge_id.level()) {
                dist = self.rev.heuristic.distance(node_ll);
            }
            let (mask, edge_destonly) = base.exempted_access_restrictions(
                directed,
                &tile,
                edge_id,
                time_context(time_info),
            );
            let mut label = ThorLabel::new(
                directed,
                NewLabel {
                    predecessor: INVALID_LABEL,
                    edge_id,
                    opp_edge_id: None,
                    cost,
                    sortcost,
                    distance: dist,
                    mode: self.mode,
                    transition_cost: Cost::ZERO,
                    not_thru_pruning: !directed.no_thru(),
                    closure_pruning: !base.is_closed(directed),
                    has_measured_speed: !partial.flow_sources.is_empty(),
                    internal_turn: InternalTurn::NoTurn,
                    restriction_idx: INVALID_RESTRICTION,
                    destonly: edge_destonly,
                    destonly_access_restr_mask: mask,
                },
            );
            // Mainline: "There is an issue with not_thru flags on small loops."
            label.set_not_thru(false);
            self.pruning_disabled_at_origin = self.pruning_disabled_at_origin
                || !label.closure_pruning()
                || !label.not_thru_pruning()
                || label.destonly();
            self.fwd.labels.push(label);
            self.fwd.queue.add(idx, sortcost);
        }

        // A `current` time becomes the current time at the origin.
        if origin.date_time == "current"
            && let Some(tz) = closest_tz
            && let Some(zone) = time_zone(tz)
        {
            origin.date_time = zone
                .to_datetime(ctx.now)
                .strftime("%Y-%m-%dT%H:%M")
                .to_string();
        }
        Ok(())
    }

    /// Adds the destination's candidate edges to the reverse search (`SetDestination`).
    fn set_destination<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        destination: &Location,
        time_info: &TimeInfo,
    ) -> Result<(), ThorError> {
        let base = ctx.costing.base();
        // Only skip outbound edges (at a node) if there are others.
        let has_other_edges = candidates(destination).iter().any(|e| !e.begin_node);
        for edge in candidates(destination) {
            if has_other_edges && edge.begin_node {
                continue;
            }
            let edge_id = candidate_id(edge)?;
            #[expect(clippy::cast_possible_truncation, reason = "mainline takes a float")]
            let percent_along = edge.percent_along as f32;
            if base.avoid_as_destination_edge(edge_id, percent_along) {
                continue;
            }
            let Some(tile) = graph::tile(ctx.provider, edge_id)? else {
                continue;
            };
            let directed = graph::edge(&tile, edge_id)?;
            let Some((opp_edge_id, opp_tile)) = graph::opposing(ctx.provider, &tile, edge_id)?
            else {
                continue;
            };
            let opp_dir_edge = graph::edge(&opp_tile, opp_edge_id)?;
            // Cost the edge itself: it's the direction of travel at the destination.
            let partial =
                ctx.partial_edge_cost(directed, edge_id, &tile, time_info, 0.0, percent_along)?;
            let mut cost = partial.cost;
            cost.cost = add_double(cost.cost, edge.distance);
            // The opposing edge ends at this edge's start node, which is in this edge's tile.
            let start = opp_dir_edge.end_node_id();
            let node_ll = graph::node_ll(&tile, graph::node(&tile, start)?);
            let mut dist = self.rev.heuristic.distance(node_ll);
            let sortcost = cost.cost + self.rev.heuristic.cost(dist);

            let idx = index_u32(self.rev.labels.len());
            self.rev.status.set(
                opp_edge_id,
                EdgeStatusInfo::new(EdgeSet::Temporary, idx),
                opp_tile.directed_edges().len(),
            );
            if !self.rev.unlimited(opp_edge_id.level()) {
                dist = self.fwd.heuristic.distance(node_ll);
            }
            let (mask, edge_destonly) = base.exempted_access_restrictions(
                directed,
                &tile,
                edge_id,
                time_context(time_info),
            );
            let mut label = ThorLabel::new(
                opp_dir_edge,
                NewLabel {
                    predecessor: INVALID_LABEL,
                    edge_id: opp_edge_id,
                    opp_edge_id: Some(edge_id),
                    cost,
                    sortcost,
                    distance: dist,
                    mode: self.mode,
                    transition_cost: Cost::ZERO,
                    not_thru_pruning: !opp_dir_edge.no_thru(),
                    closure_pruning: !base.is_closed(directed),
                    has_measured_speed: !partial.flow_sources.is_empty(),
                    internal_turn: InternalTurn::NoTurn,
                    restriction_idx: INVALID_RESTRICTION,
                    destonly: edge_destonly,
                    destonly_access_restr_mask: mask,
                },
            );
            label.set_not_thru(false);
            self.pruning_disabled_at_destination = self.pruning_disabled_at_destination
                || !label.closure_pruning()
                || !label.not_thru_pruning()
                || label.destonly();
            self.rev.labels.push(label);
            self.rev.queue.add(idx, sortcost);
        }
        Ok(())
    }

    /// Forms the paths of the best connections, recosted from the origin (`FormPath`),
    /// keeping alternates that pass the viability tests.
    #[expect(clippy::too_many_lines, reason = "mainline's FormPath, step by step")]
    fn form_path<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        options: &Options,
        origin: &Location,
        destination: &Location,
        time_info: &TimeInfo,
    ) -> Result<Vec<Vec<PathEdge>>, ThorError> {
        if self.desired_paths_count > 1 {
            filter_alternates_by_stretch(&mut self.best_connections);
        }
        let max_sharing = if self.desired_paths_count > 1 {
            get_max_sharing(origin, destination)?
        } else {
            0.0
        };
        let invariant = DateTimeType::try_from(options.date_time_type)
            .is_ok_and(|t| t == DateTimeType::Invariant);
        let mut shared_edge_ids = Vec::new();
        let mut paths: Vec<Vec<PathEdge>> = Vec::new();
        let connections = core::mem::take(&mut self.best_connections);
        for connection in &connections {
            if paths.len() >= self.desired_paths_count {
                break;
            }
            let idx1 = self.fwd.status.get(connection.edge_id).index();
            let idx2 = self.rev.status.get(connection.opp_edge_id).index();
            self.recovered_inner.clear();
            self.path_edges.clear();

            // Back along the forward tree.
            let mut next = Some(idx1);
            while let Some(index) = next {
                let label = *label_at(&self.fwd.labels, index)?;
                let tile = graph::tile(ctx.provider, label.edge_id)?
                    .ok_or(ThorError::TileGone(label.edge_id))?;
                if graph::edge(&tile, label.edge_id)?.is_shortcut() {
                    graph::recover_shortcut(ctx.provider, label.edge_id, &mut self.recovered)?;
                    self.recovered_inner
                        .extend(self.recovered.iter().skip(1).copied());
                    self.path_edges.extend(self.recovered.iter().rev().copied());
                } else {
                    self.path_edges.push(label.edge_id);
                }
                if label.is_ferry() {
                    self.has_ferry = true;
                }
                next = label.predecessor();
            }
            self.path_edges.reverse();

            // Then along the reverse tree, by opposing edges, after the connecting edge.
            let mut next = label_at(&self.rev.labels, idx2)?.predecessor();
            while let Some(index) = next {
                let label = *label_at(&self.rev.labels, index)?;
                let tile = graph::tile(ctx.provider, label.edge_id)?
                    .ok_or(ThorError::TileGone(label.edge_id))?;
                let (opp_id, opp_tile) = graph::opposing(ctx.provider, &tile, label.edge_id)?
                    .ok_or(ThorError::TileGone(label.edge_id))?;
                if graph::edge(&opp_tile, opp_id)?.is_shortcut() {
                    graph::recover_shortcut(ctx.provider, opp_id, &mut self.recovered)?;
                    self.recovered_inner
                        .extend(self.recovered.iter().skip(1).copied());
                    self.path_edges.extend(self.recovered.iter().copied());
                } else {
                    self.path_edges.push(opp_id);
                }
                if label.is_ferry() {
                    self.has_ferry = true;
                }
                next = label.predecessor();
            }

            let (Some(&first), Some(&last)) = (self.path_edges.first(), self.path_edges.last())
            else {
                continue;
            };
            let source_pct = find_percent_along(origin, first)?;
            let target_pct = find_percent_along(destination, last)?;

            let mut path = Vec::with_capacity(self.path_edges.len());
            let recovered_inner = &self.recovered_inner;
            let recosted = recost_forward(
                ctx.provider,
                ctx.costing,
                self.path_edges.iter().copied(),
                source_pct,
                target_pct,
                *time_info,
                invariant,
                true,
                |e| {
                    #[expect(clippy::cast_precision_loss, reason = "mainline stores it in a float")]
                    let path_distance = e.path_distance as f32;
                    path.push(PathEdge {
                        edge_id: e.edge_id,
                        mode: e.label.mode(),
                        elapsed_cost: e.cost,
                        transition_cost: e.transition_cost,
                        path_distance,
                        restriction_index: e.restriction_idx,
                        start_node_is_recovered: recovered_inner.contains(&e.edge_id),
                    });
                },
            );
            // Mainline logs the failure and moves on to the next connection.
            if recosted.is_err() {
                continue;
            }

            let viable = match paths.first() {
                None => true,
                Some(best) => {
                    validate_alternate_by_sharing(&mut shared_edge_ids, &paths, &path, max_sharing)
                        && validate_alternate_by_stretch(best, &path)
                        && validate_alternate_by_local_optimality(&path)
                }
            };
            if viable {
                paths.push(path);
            }
        }
        self.best_connections = connections;
        Ok(paths)
    }
}

/// Whether connecting the trees at `fwd_pred` and `rev_pred` completes a complex restriction
/// (mainline's `IsBridgingEdgeRestricted`).
///
/// It builds a "patch" of the path around the connection: up to ten edges back along the
/// forward tree and up to 21 forward along the reverse tree (while they're on complex
/// restrictions), and checks whether any restriction ending on the reverse side's edges
/// matches part of it, or whether the trees form a double U-turn.
fn is_bridging_edge_restricted<P: GraphTileProvider, C: Costing>(
    ctx: &SearchContext<'_, P, C>,
    fwd_labels: &[ThorLabel],
    rev_labels: &[ThorLabel],
    fwd_pred: &ThorLabel,
    rev_pred: &ThorLabel,
) -> Result<bool, ThorError> {
    const M: usize = 10;
    const PATCH_PATH_SIZE: usize = M * 2 + 1;
    let access_mode = ctx.costing.base().access_mode();
    let mut patch: Vec<u64> = Vec::with_capacity(PATCH_PATH_SIZE);
    patch.push(fwd_pred.edge_id.value());
    let mut next = *fwd_pred;
    for _ in 0..M {
        let Some(index) = next.predecessor() else {
            break;
        };
        next = *label_at(fwd_labels, index)?;
        if !next.on_complex_restriction() {
            break;
        }
        patch.push(next.edge_id.value());
    }
    patch.reverse();

    let mut tile: Option<P::TileHandle> = None;
    let mut next = *rev_pred;
    for _ in 0..PATCH_PATH_SIZE {
        let Some(index) = next.predecessor() else {
            break;
        };
        next = *label_at(rev_labels, index)?;
        if !next.on_complex_restriction() {
            break;
        }
        let Some(edge_id) = next.opp_edge_id else {
            break;
        };
        // A double U-turn: both trees went into a restriction and turned back.
        if patch.contains(&edge_id.value()) {
            return Ok(true);
        }
        patch.push(edge_id.value());

        let t = match &tile {
            Some(t) if t.may_contain_id(edge_id) => t.clone(),
            _ => graph::tile(ctx.provider, edge_id)?.ok_or(ThorError::TileGone(edge_id))?,
        };
        let edge = graph::edge(&t, edge_id)?;
        if !edge.end_restriction().is_disjoint(access_mode) {
            let mut found = false;
            for restriction in t.get_complex_restrictions(true, edge_id, access_mode) {
                let restriction = restriction?;
                found = true;
                // From, the vias in path order, then to.
                let mut ids = Vec::with_capacity(restriction.vias().len() + 2);
                ids.push(restriction.raw_from_graph_id());
                ids.extend(restriction.vias().iter().rev().map(GraphId::value));
                ids.push(restriction.raw_to_graph_id());
                if patch.windows(ids.len()).any(|w| w == ids.as_slice()) {
                    return Ok(true);
                }
            }
            if !found {
                return Err(ThorError::MissingRestriction(edge_id));
            }
        }
        tile = Some(t);
    }
    Ok(false)
}
