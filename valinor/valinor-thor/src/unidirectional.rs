//! Time-dependent A* (mainline's `UnidirectionalAStar`, as `TimeDepForward` and
//! `TimeDepReverse`): a one-directional search that costs each edge at the time
//! the route reaches it.
//!
//! The forward search starts at the origin's time (`depart_at`, `current`) and moves the time
//! forward with the cost so far; the reverse search starts at the destination's time
//! (`arrive_by`) and moves it back. Thor also uses the forward search, without a time,
//! for short routes whose origin and destination edges are the same or adjacent
//! (bidirectional A* doesn't handle those well).
//!
//! The destination's candidate edges become labels with only the part of the edge that
//! is used, flagged as destinations; the search ends when it pops one.
//! Origin edges get no edge status, so a route can come back around to them
//! (around the block on a one-way street, for example).
//! This follows mainline step by step, including its queue order, hierarchy limits
//! (with the arterial expansion distance stretched for medium-length routes),
//! and its float arithmetic.

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::{HierarchyLimits, Location};
use valinor_sif::{
    Cost, Costing, EdgeAccess, INVALID_RESTRICTION, InternalTurn, Label, TimeContext, TimeInfo,
    TravelMode, stop_expanding,
};

use crate::bidirectional::{index_u32, label_at, nth_edge};
use crate::edge_status::{EdgeSet, EdgeStatus, EdgeStatusInfo};
use crate::error::ThorError;
use crate::geo::{AStarHeuristic, PointLL};
use crate::graph;
use crate::label::{INVALID_LABEL, NewLabel, ThorLabel};
use crate::path::PathEdge;
use crate::queue::BucketQueue;
use crate::search::{
    BUCKET_COUNT, INITIAL_LABELS, SearchContext, UNIT_SIZE, add_double, candidate_id, candidates,
    clear_labels, first_candidate_point, time_context,
};
use crate::time::location_time_info;

/// Iterations allowed without getting closer to the destination
/// (`kMaxIterationsWithoutConvergence`).
const MAX_ITERATIONS_WITHOUT_CONVERGENCE: u32 = 1_800_000;

/// A destination candidate edge: where along the edge the location is, and its score.
#[derive(Clone, Copy, Debug)]
struct Destination {
    edge_id: GraphId,
    percent_along: f64,
    distance: f64,
}

/// The best destination label so far, and its cost (`best_path`).
type BestPath = Option<(u32, f32)>;

/// Time-dependent A* in one direction (`FORWARD` or not), with its reusable storage.
#[derive(Debug, Default)]
pub(crate) struct UnidirectionalAStar<const FORWARD: bool> {
    labels: Vec<ThorLabel>,
    queue: BucketQueue,
    status: EdgeStatus,
    heuristic: AStarHeuristic,
    limits: Vec<HierarchyLimits>,
    /// The candidate edges of the far location (`destinations_`), by edge.
    destinations: Vec<Destination>,
    mode: TravelMode,
    /// Whether the path has a ferry (`has_ferry_`).
    pub(crate) has_ferry: bool,
}

/// A node's time zone index (0 for none).
#[inline]
fn tz_index(node: &NodeInfo) -> u32 {
    node.time_zone_index().map_or(0, |i| u32::from(i.get()))
}

/// `static_cast<uint32_t>` of a non-negative double.
#[inline]
fn to_u32(value: f64) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "mainline's static_cast<uint32_t>"
    )]
    let v = value as u32;
    v
}

/// A double narrowed to a float, as C++ does when passing it as one.
#[inline]
fn to_f32(value: f64) -> f32 {
    #[expect(clippy::cast_possible_truncation, reason = "C++ double to float")]
    let v = value as f32;
    v
}

impl<const FORWARD: bool> UnidirectionalAStar<FORWARD> {
    /// The algorithm's name, as in the trip's `algorithms`.
    pub(crate) const NAME: &'static str = if FORWARD {
        "time_dependent_forward_a*"
    } else {
        "time_dependent_reverse_a*"
    };

    /// Bytes of storage the search holds, used or not.
    pub(crate) fn bytes(&self) -> usize {
        self.labels.capacity() * size_of::<ThorLabel>()
            + self.queue.bytes()
            + self.status.bytes()
            + self.destinations.capacity() * size_of::<Destination>()
    }

    /// The labels and edge statuses the search has room for (for tests).
    #[cfg(test)]
    pub(crate) fn retained(&self) -> usize {
        self.labels.capacity() + self.status.retained_edges()
    }

    /// Clears a search, keeping room for `max_labels` labels and edges (`Clear`).
    pub(crate) fn clear(&mut self, max_labels: usize) {
        clear_labels(&mut self.labels, max_labels);
        self.destinations.clear();
        self.status.clear_retaining(max_labels);
        self.has_ferry = false;
    }

    /// Aims the heuristic and resets the queue (`Init`).
    fn init<C: Costing>(&mut self, costing: &C, origll: PointLL, destll: PointLL) {
        let factor = costing.astar_cost_factor();
        let mincost = if FORWARD {
            self.heuristic.init(destll, factor);
            self.heuristic.cost_to(origll).0
        } else {
            self.heuristic.init(origll, factor);
            self.heuristic.cost_to(destll).0
        };
        if self.labels.capacity() == 0 {
            self.labels.reserve(INITIAL_LABELS);
        }
        #[expect(clippy::cast_precision_loss, reason = "mainline's float range")]
        let range = (BUCKET_COUNT * UNIT_SIZE) as f32;
        self.queue.reuse(mincost, range, UNIT_SIZE);
        self.status.clear();
        self.limits.clear();
        self.limits
            .extend_from_slice(costing.base().hierarchy_limits());
    }

    /// Stretches the arterial level's expansion distance for routes of 25 to 100 km,
    /// when the request uses the default hierarchy limits (`ModifyHierarchyLimits`).
    fn modify_hierarchy_limits<C: Costing>(&mut self, costing: &C, dist: f32) {
        if !costing.base().default_hierarchy_limits() {
            return;
        }
        let mut factor = 1.0f32;
        if 25_000.0 < dist && dist < 100_000.0 {
            factor = (100_000.0 / dist).min(3.0);
        }
        if let Some(arterial) = self.limits.get_mut(1) {
            arterial.expand_within_dist *= factor;
        }
    }

    /// Finds the best path (`GetBestPath`); no paths if the search fails.
    ///
    /// `origin` and `destination` are the route's; the reverse search starts at the
    /// destination.
    pub(crate) fn best_path<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        origin: &mut Location,
        destination: &mut Location,
    ) -> Result<Vec<Vec<PathEdge>>, ThorError> {
        self.mode = ctx.costing.base().travel_mode();
        let origin_new = first_candidate_point(origin, 0)?;
        let destination_new = first_candidate_point(destination, 1)?;
        self.init(ctx.costing, origin_new, destination_new);
        let mut mindist =
            self.heuristic
                .distance(if FORWARD { origin_new } else { destination_new });

        let (startpoint, endpoint) = if FORWARD {
            (origin, destination)
        } else {
            (destination, origin)
        };
        let time_info = location_time_info(ctx.provider, startpoint, ctx.now)?;

        // The destination first, in case an origin edge is also a destination edge.
        self.set_destination(ctx, endpoint)?;
        self.set_origin(ctx, startpoint, &time_info)?;
        self.modify_hierarchy_limits(ctx.costing, mindist);

        let mut nc: u32 = 0;
        let mut best_path: BestPath = None;
        let mut n = 0;
        loop {
            ctx.check_interrupt(&mut n)?;
            let Some(predindex) = self.queue.pop(&self.labels) else {
                return Ok(Vec::new());
            };
            let mut pred = *label_at(&self.labels, predindex)?;
            if pred.destination() {
                return Ok(vec![self.form_path(predindex)?]);
            }
            // Origin edges stay open, so that loops (around the block) are possible.
            if !pred.origin() {
                self.status.update(pred.edge_id, EdgeSet::Permanent);
            }

            // Give up if the search stops converging on the destination.
            // (A high count, because entering destination-only roads costs a lot.)
            let dist2dest = pred.distance;
            if dist2dest < mindist {
                mindist = dist2dest;
                nc = 0;
            } else {
                let previous = nc;
                nc = nc.saturating_add(1);
                if previous > MAX_ITERATIONS_WITHOUT_CONVERGENCE {
                    return Ok(match best_path {
                        Some((index, _)) => vec![self.form_path(index)?],
                        None => Vec::new(),
                    });
                }
            }

            if self
                .limits
                .get(usize::from(pred.end_node.level()))
                .is_some_and(|l| stop_expanding(l, dist2dest))
            {
                continue;
            }

            let opp_pred: Option<(P::TileHandle, GraphId)> = if FORWARD {
                None
            } else {
                let Some(opp_id) = pred.opp_edge_id else {
                    continue;
                };
                let tile = graph::tile(ctx.provider, opp_id)?.ok_or(ThorError::TileGone(opp_id))?;
                Some((tile, opp_id))
            };
            let opp_pred_edge = match &opp_pred {
                Some((tile, id)) => Some(graph::edge(tile, *id)?),
                None => None,
            };

            self.expand(
                ctx,
                pred.end_node,
                &mut pred,
                predindex,
                opp_pred_edge,
                &time_info,
                &mut best_path,
            )?;
        }
    }

    /// Expands from a node (`Expand`). Returns whether any edge could be expanded.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "mainline's Expand, step by step"
    )]
    fn expand<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        node: GraphId,
        pred: &mut ThorLabel,
        pred_idx: u32,
        opp_pred_edge: Option<&DirectedEdge>,
        time_info: &TimeInfo,
        best_path: &mut BestPath,
    ) -> Result<bool, ThorError> {
        let Some(tile) = graph::tile(ctx.provider, node)? else {
            return Ok(false);
        };
        let nodeinfo = graph::node(&tile, node)?;
        let tz = tz_index(nodeinfo);
        let offset_time = if FORWARD {
            time_info.forward(pred.cost.secs, tz)
        } else {
            time_info.reverse(pred.cost.secs, tz)
        };

        // At a barrier, only a U-turn is possible.
        if !ctx.costing.node_allowed(nodeinfo) {
            let Some(pred_tile) = graph::tile_for(ctx.provider, &tile, pred.edge_id)? else {
                return Ok(false);
            };
            pred.set_deadend(true);
            let Some((opp_id, opp_tile)) = graph::opposing(ctx.provider, &pred_tile, pred.edge_id)?
            else {
                return Ok(false);
            };
            let opp_edge = graph::edge(&opp_tile, opp_id)?;
            return self.expand_inner(
                ctx,
                pred,
                opp_pred_edge,
                nodeinfo,
                pred_idx,
                opp_edge,
                opp_id,
                &opp_tile,
                &offset_time,
                best_path,
            );
        }

        let mut disable_uturn = false;
        let mut uturn: Option<usize> = None;
        let first = graph::first_edge_id(node, nodeinfo)?;
        let edges = tile.get_outbound_edges_from_node(nodeinfo);
        for (i, edge) in edges.iter().enumerate() {
            let is_uturn = pred.opp_local_idx() == edge.local_edge_index();
            if is_uturn {
                uturn = Some(i);
            }
            let expanded = !is_uturn
                && self.expand_inner(
                    ctx,
                    pred,
                    opp_pred_edge,
                    nodeinfo,
                    pred_idx,
                    edge,
                    nth_edge(first, i)?,
                    &tile,
                    &offset_time,
                    best_path,
                )?;
            disable_uturn = expanded || disable_uturn;
        }

        for trans in tile.get_transitions(nodeinfo) {
            let end = trans.corresponding_end_node_id();
            if !trans.is_up()
                && self
                    .limits
                    .get(usize::from(end.level()))
                    .is_some_and(|l| stop_expanding(l, pred.distance))
            {
                continue;
            }
            let Some(trans_tile) = graph::tile(ctx.provider, end)? else {
                continue;
            };
            if let Some(limits) = self.limits.get_mut(usize::from(node.level())) {
                limits.up_transition_count = limits
                    .up_transition_count
                    .saturating_add(u32::from(trans.is_up()));
            }
            let trans_node = graph::node(&trans_tile, end)?;
            let trans_first = graph::first_edge_id(end, trans_node)?;
            for (i, edge) in trans_tile
                .get_outbound_edges_from_node(trans_node)
                .iter()
                .enumerate()
            {
                let expanded = self.expand_inner(
                    ctx,
                    pred,
                    opp_pred_edge,
                    trans_node,
                    pred_idx,
                    edge,
                    nth_edge(trans_first, i)?,
                    &trans_tile,
                    &offset_time,
                    best_path,
                )?;
                disable_uturn = expanded || disable_uturn;
            }
        }

        if !disable_uturn
            && let Some(i) = uturn
            && let Some(edge) = edges.get(i)
        {
            pred.set_deadend(true);
            let expanded = self.expand_inner(
                ctx,
                pred,
                opp_pred_edge,
                nodeinfo,
                pred_idx,
                edge,
                nth_edge(first, i)?,
                &tile,
                &offset_time,
                best_path,
            )?;
            disable_uturn = expanded || disable_uturn;
        }
        Ok(disable_uturn)
    }

    /// Considers one edge from `pred` (`ExpandInner`): labels it (and its destination parts),
    /// or improves its label. Returns whether the edge could have been expanded.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "mainline's ExpandInner, step by step"
    )]
    fn expand_inner<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        pred: &ThorLabel,
        opp_pred_edge: Option<&DirectedEdge>,
        nodeinfo: &NodeInfo,
        pred_idx: u32,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &P::TileHandle,
        time_info: &TimeInfo,
        best_path: &mut BestPath,
    ) -> Result<bool, ThorError> {
        // Time-dependent searches don't take shortcuts.
        if edge.is_shortcut() {
            return Ok(false);
        }
        let base = ctx.costing.base();
        if !FORWARD && edge.reverse_access().is_disjoint(base.access_mode()) {
            return Ok(false);
        }
        if self.status.get(edge_id).set() == EdgeSet::Permanent {
            return Ok(true);
        }
        let Some(endtile) = graph::end_tile(ctx.provider, tile, edge)? else {
            return Ok(false);
        };
        let opp: Option<(GraphId, &DirectedEdge)> = if FORWARD {
            None
        } else {
            let opp_id = graph::opposing_in_end_tile(&endtile, edge)?;
            Some((opp_id, graph::edge(&endtile, opp_id)?))
        };

        let edge_cost = match opp {
            None => ctx.edge_cost(edge, edge_id, tile, time_info)?,
            Some((opp_id, opp_edge)) => ctx.edge_cost(opp_edge, opp_id, &endtile, time_info)?,
        };
        let has_measured_speed = !edge_cost.flow_sources.is_empty();
        let endpoint = graph::node_ll(&endtile, graph::node(&endtile, edge.end_node_id())?);
        let edge_count = tile.directed_edges().len();
        // Unidirectional searches check access at the node's time zone.
        let time = TimeContext::new(time_info.local_time, tz_index(nodeinfo));

        // The access checks and transition cost that adding or updating a label share.
        let access_and_transition =
            |this: &mut Self| -> Result<Option<(EdgeAccess, Cost)>, ThorError> {
                let mut access = EdgeAccess::new(pred.destonly_access_restr_mask);
                let Self { labels, status, .. } = this;
                let allowed = match opp {
                    None => ctx
                        .costing
                        .allowed(edge, edge_id, tile, pred, time, &mut access),
                    Some((opp_id, opp_edge)) => ctx.costing.allowed_reverse(
                        edge,
                        pred,
                        opp_edge,
                        opp_id,
                        &endtile,
                        time,
                        &mut access,
                    ),
                };
                if !allowed
                    || base.restricted(
                        edge,
                        pred,
                        labels.as_slice(),
                        tile,
                        edge_id,
                        FORWARD,
                        time,
                        |id| status.update(id, EdgeSet::Unreached),
                    )?
                {
                    return Ok(None);
                }
                let transition_cost = match opp {
                    None => ctx.costing.transition_cost(
                        edge,
                        tile,
                        nodeinfo,
                        pred,
                        ctx.provider,
                        access.edge_destonly,
                    )?,
                    Some((_, opp_edge)) => {
                        let Some(opp_pred_edge) = opp_pred_edge else {
                            return Ok(None);
                        };
                        ctx.costing.transition_cost_reverse(
                            edge.local_edge_index(),
                            nodeinfo,
                            opp_edge,
                            opp_pred_edge,
                            pred,
                            &endtile,
                            ctx.provider,
                            has_measured_speed,
                            pred.internal_turn(),
                            access.edge_destonly,
                        )?
                    }
                };
                Ok(Some((access, transition_cost)))
            };

        let add_label = |this: &mut Self,
                         dest: Option<Destination>,
                         best_path: &mut BestPath|
         -> Result<bool, ThorError> {
            let Some((access, transition_cost)) = access_and_transition(this)? else {
                return Ok(false);
            };
            let percent_traversed: f64 = match dest {
                None => 1.0,
                Some(d) if FORWARD => d.percent_along,
                Some(d) => 1.0 - d.percent_along,
            };
            let mut cost = pred.cost + transition_cost + edge_cost.cost * to_f32(percent_traversed);
            cost.cost = add_double(cost.cost, dest.map_or(0.0, |d| d.distance));
            let mut dist = 0.0;
            let sortcost = cost.cost
                + if dest.is_some() {
                    this.heuristic.cost(0.0)
                } else {
                    let (h, d) = this.heuristic.cost_to(endpoint);
                    dist = d;
                    h
                };
            let path_distance = to_u32(
                f64::from(pred.path_distance())
                    + f64::from(edge.length()) * percent_traversed
                    + 0.5,
            );
            let idx = index_u32(this.labels.len());
            if dest.is_some() && best_path.is_none_or(|(_, c)| cost.cost < c) {
                // The best destination so far, in case the search stops converging.
                *best_path = Some((idx, cost.cost));
            }
            let (closed_edge, internal_turn) = match opp {
                None => (
                    base.is_closed(edge),
                    base.turn_type(pred.opp_local_idx(), nodeinfo, edge, None),
                ),
                Some((_, opp_edge)) => (
                    base.is_closed(opp_edge),
                    base.turn_type(edge.local_edge_index(), nodeinfo, opp_edge, opp_pred_edge),
                ),
            };
            let mut label = ThorLabel::new(
                edge,
                NewLabel {
                    predecessor: pred_idx,
                    edge_id,
                    opp_edge_id: opp.map(|(id, _)| id),
                    cost,
                    sortcost,
                    distance: dist,
                    mode: this.mode,
                    transition_cost,
                    not_thru_pruning: pred.not_thru_pruning() || !edge.no_thru(),
                    closure_pruning: pred.closure_pruning() || !closed_edge,
                    has_measured_speed,
                    internal_turn,
                    restriction_idx: access.restriction_idx,
                    destonly: access.edge_destonly,
                    destonly_access_restr_mask: access.destonly_access_restr_mask,
                },
            );
            label.update_with_distance(
                pred_idx,
                cost,
                sortcost,
                transition_cost,
                path_distance,
                access.restriction_idx,
            );
            if dest.is_some() {
                label.set_destination();
            }
            this.labels.push(label);
            this.queue.add(idx, sortcost);
            if dest.is_none() {
                // Only labels that aren't destinations get an edge status.
                this.status.set(
                    edge_id,
                    EdgeStatusInfo::new(EdgeSet::Temporary, idx),
                    edge_count,
                );
            }
            Ok(true)
        };

        let status = self.status.get(edge_id);
        let mut added = if status.set() == EdgeSet::Temporary {
            match access_and_transition(self)? {
                None => false,
                Some((access, transition_cost)) => {
                    let newcost = pred.cost + transition_cost + edge_cost.cost;
                    let idx = self.status.get(edge_id).index();
                    if let Some(lab) = usize::try_from(idx)
                        .ok()
                        .and_then(|i| self.labels.get_mut(i))
                        && newcost.cost < lab.cost.cost
                    {
                        let newsortcost = lab.sortcost - (lab.cost.cost - newcost.cost);
                        self.queue.decrease(idx, lab.sortcost, newsortcost);
                        lab.update(
                            pred_idx,
                            newcost,
                            newsortcost,
                            transition_cost,
                            access.restriction_idx,
                        );
                    }
                    true
                }
            }
        } else {
            add_label(self, None, best_path)?
        };

        // The parts of this edge that end at the destination.
        let key = opp.map_or(edge_id, |(id, _)| id);
        let mut i = 0;
        while let Some(dest) = self.destinations.get(i).copied() {
            if dest.edge_id == key {
                added = add_label(self, Some(dest), best_path)? || added;
            }
            i += 1;
        }
        Ok(added)
    }

    /// Records the far location's candidate edges (`SetDestination`).
    fn set_destination<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        dest: &Location,
    ) -> Result<(), ThorError> {
        let base = ctx.costing.base();
        let at_node =
            |e: &valhalla_proto::PathEdge| if FORWARD { e.begin_node } else { e.end_node };
        let has_other_edges = candidates(dest).iter().any(|e| !at_node(e));
        for edge in candidates(dest) {
            if has_other_edges && at_node(edge) {
                continue;
            }
            let edge_id = candidate_id(edge)?;
            if graph::tile(ctx.provider, edge_id)?.is_none() {
                continue;
            }
            let percent_along = to_f32(edge.percent_along);
            let avoided = if FORWARD {
                base.avoid_as_destination_edge(edge_id, percent_along)
            } else {
                base.avoid_as_origin_edge(edge_id, percent_along)
            };
            if avoided {
                continue;
            }
            // Keyed by the edge itself, not its opposing edge.
            self.destinations.push(Destination {
                edge_id,
                percent_along: edge.percent_along,
                distance: edge.distance,
            });
        }
        Ok(())
    }

    /// Adds the start location's candidate edges to the queue (`SetOrigin`),
    /// with whole labels for trivial routes that end on the same edge.
    #[expect(clippy::too_many_lines, reason = "mainline's SetOrigin, step by step")]
    fn set_origin<P: GraphTileProvider, C: Costing>(
        &mut self,
        ctx: &SearchContext<'_, P, C>,
        origin: &Location,
        time_info: &TimeInfo,
    ) -> Result<(), ThorError> {
        let base = ctx.costing.base();
        let at_node =
            |e: &valhalla_proto::PathEdge| if FORWARD { e.end_node } else { e.begin_node };
        let has_other_edges = candidates(origin).iter().any(|e| !at_node(e));
        for edge in candidates(origin) {
            let edge_id = candidate_id(edge)?;
            // A node snap that arrives at the start is useless, unless the route is
            // "super trivial": both locations snapped to the same end of the same edge.
            #[expect(clippy::float_cmp, reason = "mainline compares exactly")]
            let super_trivial = self
                .destinations
                .iter()
                .any(|d| d.edge_id == edge_id && d.percent_along == edge.percent_along);
            if at_node(edge) && has_other_edges && !super_trivial {
                continue;
            }
            let percent_along = to_f32(edge.percent_along);
            let avoided = if FORWARD {
                base.avoid_as_origin_edge(edge_id, percent_along)
            } else {
                base.avoid_as_destination_edge(edge_id, percent_along)
            };
            if avoided {
                continue;
            }
            let Some(tile) = graph::tile(ctx.provider, edge_id)? else {
                continue;
            };
            let directed = graph::edge(&tile, edge_id)?;
            let (opp, endpoint) = if FORWARD {
                let Some(end_tile) = graph::tile(ctx.provider, directed.end_node_id())? else {
                    continue;
                };
                let end = graph::node(&end_tile, directed.end_node_id())?;
                (None, graph::node_ll(&end_tile, end))
            } else {
                let Some((opp_id, opp_tile)) = graph::opposing(ctx.provider, &tile, edge_id)?
                else {
                    continue;
                };
                let opp_edge = graph::edge(&opp_tile, opp_id)?.clone();
                // The opposing edge ends at this edge's start node, in this edge's tile.
                let start = graph::node(&tile, opp_edge.end_node_id())?;
                (Some((opp_id, opp_edge)), graph::node_ll(&tile, start))
            };
            let (mask, edge_destonly) = base.exempted_access_restrictions(
                directed,
                &tile,
                edge_id,
                time_context(time_info),
            );

            let add_label = |this: &mut Self, dest: Option<Destination>| -> Result<(), ThorError> {
                let start = if FORWARD {
                    edge.percent_along
                } else {
                    dest.map_or(0.0, |d| d.percent_along)
                };
                let end = if FORWARD {
                    dest.map_or(1.0, |d| d.percent_along)
                } else {
                    edge.percent_along
                };
                let percent_traversed = end - start;
                if percent_traversed < 0.0 {
                    // Not a trivial route after all.
                    return Ok(());
                }
                let partial = ctx.partial_edge_cost(
                    directed,
                    edge_id,
                    &tile,
                    time_info,
                    to_f32(start),
                    to_f32(end),
                )?;
                let mut cost = partial.cost;
                cost.cost = add_double(cost.cost, edge.distance + dest.map_or(0.0, |d| d.distance));
                let mut dist = 0.0;
                let sortcost = cost.cost
                    + if dest.is_some() {
                        this.heuristic.cost(0.0)
                    } else {
                        let (h, d) = this.heuristic.cost_to(endpoint);
                        dist = d;
                        h
                    };
                let path_distance = to_u32(f64::from(directed.length()) * percent_traversed + 0.5);
                let idx = index_u32(this.labels.len());
                let (label_edge, label_id, label_opp) = match &opp {
                    None => (directed, edge_id, None),
                    Some((opp_id, opp_edge)) => (opp_edge, *opp_id, Some(edge_id)),
                };
                let mut label = ThorLabel::new(
                    label_edge,
                    NewLabel {
                        predecessor: INVALID_LABEL,
                        edge_id: label_id,
                        opp_edge_id: label_opp,
                        cost,
                        sortcost,
                        distance: dist,
                        mode: this.mode,
                        transition_cost: Cost::ZERO,
                        not_thru_pruning: false,
                        closure_pruning: !base.is_closed(directed),
                        has_measured_speed: !partial.flow_sources.is_empty(),
                        internal_turn: InternalTurn::NoTurn,
                        restriction_idx: INVALID_RESTRICTION,
                        destonly: edge_destonly,
                        destonly_access_restr_mask: mask,
                    },
                );
                if !FORWARD {
                    // Mainline: "There is an issue with not_thru flags on small loops."
                    label.set_not_thru(false);
                }
                label.update_with_distance(
                    INVALID_LABEL,
                    cost,
                    sortcost,
                    Cost::ZERO,
                    path_distance,
                    INVALID_RESTRICTION,
                );
                label.set_origin();
                if dest.is_some() {
                    label.set_destination();
                }
                this.labels.push(label);
                this.queue.add(idx, sortcost);
                Ok(())
            };

            add_label(self, None)?;
            let mut i = 0;
            while let Some(dest) = self.destinations.get(i).copied() {
                if dest.edge_id == edge_id {
                    add_label(self, Some(dest))?;
                }
                i += 1;
            }
        }
        Ok(())
    }

    /// Forms the path that ends at a destination label (`FormPath`).
    ///
    /// The reverse search's labels hold each edge's cost plus the transition at its end
    /// (looking forward along the route); the path moves each transition to the start
    /// of the edge after it.
    fn form_path(&mut self, dest: u32) -> Result<Vec<PathEdge>, ThorError> {
        let mut path = Vec::new();
        let mut index = Some(dest);
        if FORWARD {
            while let Some(i) = index {
                let label = *label_at(&self.labels, i)?;
                #[expect(clippy::cast_precision_loss, reason = "mainline stores it in a float")]
                let path_distance = label.path_distance() as f32;
                path.push(PathEdge {
                    edge_id: label.edge_id,
                    mode: label.mode(),
                    elapsed_cost: label.cost,
                    transition_cost: label.transition_cost,
                    path_distance,
                    restriction_index: label.restriction_idx,
                    start_node_is_recovered: false,
                });
                if label.is_ferry() {
                    self.has_ferry = true;
                }
                index = label.predecessor();
            }
            path.reverse();
        } else {
            let mut cost = Cost::ZERO;
            let mut previous_transition_cost = Cost::ZERO;
            while let Some(i) = index {
                let label = *label_at(&self.labels, i)?;
                let predecessor = label.predecessor();
                match predecessor {
                    None => cost += label.cost,
                    Some(p) => cost += label.cost - label_at(&self.labels, p)?.cost,
                }
                cost -= label.transition_cost;
                cost += previous_transition_cost;
                let edge_id = label
                    .opp_edge_id
                    .ok_or(ThorError::TileGone(label.edge_id))?;
                #[expect(clippy::cast_precision_loss, reason = "mainline stores it in a float")]
                let path_distance = label.path_distance() as f32;
                path.push(PathEdge {
                    edge_id,
                    mode: label.mode(),
                    elapsed_cost: cost,
                    transition_cost: previous_transition_cost,
                    path_distance,
                    restriction_index: label.restriction_idx,
                    start_node_is_recovered: false,
                });
                if label.is_ferry() {
                    self.has_ferry = true;
                }
                index = predecessor;
                previous_transition_cost = label.transition_cost;
            }
        }
        Ok(path)
    }
}
