//! The edge walk (mainline's `RouteMatcher::FormPath`, `route_matcher.cc`):
//! the path of a shape that is exactly the shape of a route over the graph,
//! found by walking the shape from node to node.
//!
//! The walk starts from each of the origin's candidate edges (Loki correlates the shape's first
//! and last points as `options.locations`), follows the shape to the edge's end node,
//! and from each node tries the node's edges in order, taking the first whose end node is a
//! later shape point (within `LL_EPSILON`), whose length agrees with the shape's
//! (`length_comparison`), and whose own shape points are the shape's points in between
//! (`check_shape`). It ends at a node where one of the destination's candidate edges starts,
//! once the shape's length there agrees with the whole shape's within 5 meters.
//!
//! Everything that decides whether a walk succeeds, and the costs along it, follows mainline,
//! including what looks odd:
//!
//! - The walk is greedy: once an origin candidate's end node is found on the shape and the walk
//!   from there fails, no other origin candidate is tried.
//! - The edges followed from each shape point and level are remembered across the whole walk
//!   (`followed_edges`), not per node. A node reached at a shape point starts its loops over
//!   edges and transitions at the count recorded there, but takes them from the node's first
//!   (mainline's pointers start at the first, its counter at the record), so it tries only its
//!   first `edge_count - start_de` edges.
//! - The predecessor label for transition costs is shared by the whole walk and isn't put back
//!   when a branch fails, so a transition can be costed from an edge that a failed branch tried.
//! - The partial cost of the destination edge is taken at the time of the start of the walk.
//!
//! Mainline recurses once per edge; this walks with an explicit stack, so that a long route
//! can't overflow a thread's stack, and visits the same branches in the same order.

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares floats exactly"
)]

use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};
use valhalla_graphtile::{GraphId, RoadUse};
use valhalla_proto::location::HasTime;
use valhalla_proto::{Location, Options, PathEdge as CandidateEdge};
use valinor_sif::{
    AnyCosting, Cost, Costing, EdgeLabel, InternalTurn, LabelState, TimeInfo, TravelMode,
};

use crate::config::LEVELS;
use crate::error::ThorError;
use crate::geo::PointLL;
use crate::graph;
use crate::path::PathEdge;
use crate::search::point_of;
use crate::time::{graph_id, location_time_info_or, seconds_to_date};

/// The tolerance for the shape's length at the destination, in meters (`kTotalDistanceEpsilon`).
const TOTAL_DISTANCE_EPSILON: f32 = 5.0;

/// The smallest tolerance for an edge's length, in meters (`kMinLengthTolerance`).
const MIN_LENGTH_TOLERANCE: f32 = 10.0;

/// How close two points must be in degrees to be the same (`midgard::LL_EPSILON`, a float).
const LL_EPSILON: f32 = 0.00002;

/// Mainline's `midgard::equal`: whether `a` is within `epsilon` of `b`.
#[inline]
fn equal_f32(a: f32, b: f32, epsilon: f32) -> bool {
    let diff = a - b;
    let negative = diff < 0.0;
    (!negative && diff <= epsilon) || (negative && diff >= -epsilon)
}

/// [`equal_f32`] in double precision.
#[inline]
fn equal_f64(a: f64, b: f64, epsilon: f64) -> bool {
    let diff = a - b;
    let negative = diff < 0.0;
    (!negative && diff <= epsilon) || (negative && diff >= -epsilon)
}

/// Whether two points are the same within `LL_EPSILON` (`PointLL::ApproximatelyEqual`).
#[inline]
fn approximately_equal(a: PointLL, b: PointLL) -> bool {
    let epsilon = f64::from(LL_EPSILON);
    equal_f64(a.lng, b.lng, epsilon) && equal_f64(a.lat, b.lat, epsilon)
}

/// The longest shape length that still agrees with a length (`length_comparison`):
/// the length plus 5% of it (at least 10 meters and at most 25) for an exact match,
/// or 10% (at most 100 meters) otherwise.
#[inline]
fn length_comparison(length: f32, exact_match: bool) -> f32 {
    let (t, max_t) = if exact_match {
        (length * 0.05, 25.0)
    } else {
        (length * 0.1, 100.0)
    };
    let tolerance = if t < MIN_LENGTH_TOLERANCE {
        MIN_LENGTH_TOLERANCE
    } else if t > max_t {
        max_t
    } else {
        t
    };
    length + tolerance
}

/// A shape point's time (`time()`, 0 when unset).
#[inline]
pub(crate) fn time_of(location: &Location) -> f64 {
    match location.has_time {
        Some(HasTime::Time(t)) => t,
        None => 0.0,
    }
}

/// A node's time zone index, 0 for none.
#[inline]
fn tz_of(node: &NodeInfo) -> u32 {
    node.time_zone_index().map_or(0, |i| u32::from(i.get()))
}

/// A path edge with what the walk also needs to know (mainline's `PathInfo::is_shortcut`).
struct WalkedEdge {
    edge: PathEdge,
    is_shortcut: bool,
}

/// One of the destination's candidates, by the node the walk must reach for it
/// (an entry of mainline's `end_node_t`).
struct EndEdge {
    /// The start node of the candidate's edge, or its end node if the candidate is at its end.
    node: GraphId,
    /// The candidate.
    candidate: CandidateEdge,
    /// Meters along the edge to the destination (0 at the end node).
    distance: f32,
}

/// The map of end nodes (`GetEndEdges`), as mainline's `std::unordered_map::insert` fills it:
/// the first candidate for each node wins.
fn end_edges<P: GraphTileProvider>(
    provider: &P,
    destination: &Location,
) -> Result<Vec<EndEdge>, ThorError> {
    let mut end_nodes: Vec<EndEdge> = Vec::new();
    let candidates = destination
        .correlation
        .as_ref()
        .map_or(&[][..], |c| &c.edges);
    for candidate in candidates {
        // A destination at a node skips the edges leaving it.
        let Ok(edge_id) = GraphId::try_from_id(candidate.graph_id) else {
            continue;
        };
        if candidate.begin_node {
            continue;
        }
        let (node, distance) = if candidate.end_node {
            let tile = graph::tile(provider, edge_id)?.ok_or(ThorError::TileGone(edge_id))?;
            (graph::edge(&tile, edge_id)?.end_node_id(), 0.0)
        } else {
            // "Couldn't get the opposing edge tile."
            let tile = graph::tile(provider, edge_id)?.ok_or(ThorError::TileGone(edge_id))?;
            let (opp_id, opp_tile) =
                graph::opposing(provider, &tile, edge_id)?.ok_or(ThorError::TileGone(edge_id))?;
            let opp = graph::edge(&opp_tile, opp_id)?;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "mainline stores the double product in a float"
            )]
            let distance = (f64::from(opp.length()) * candidate.percent_along) as f32;
            (opp.end_node_id(), distance)
        };
        if !end_nodes.iter().any(|e| e.node == node) {
            end_nodes.push(EndEdge {
                node,
                candidate: candidate.clone(),
                distance,
            });
        }
    }
    if end_nodes.is_empty() {
        return Err(ThorError::EdgeWalk("No valid end edges are found"));
    }
    Ok(end_nodes)
}

/// The time at the start of the walk (`init_time_info`): the first origin candidate whose
/// end node has a time zone gives it, and a shape that has point times but no `date_time`
/// gets its first point's time as a date and time in that zone.
pub(crate) fn walk_time_info<P: GraphTileProvider>(
    provider: &P,
    first_edges: &[CandidateEdge],
    options: &mut Options,
    now: jiff::Timestamp,
) -> Result<TimeInfo, ThorError> {
    for candidate in first_edges {
        let Ok(edge_id) = GraphId::try_from_id(candidate.graph_id) else {
            continue;
        };
        let Some(tile) = graph::tile(provider, edge_id)? else {
            continue;
        };
        let end = graph::edge(&tile, edge_id)?.end_node_id();
        let Some(end_tile) = graph::tile_for(provider, &tile, end)? else {
            continue;
        };
        let tz = tz_of(graph::node(&end_tile, end)?);
        if valinor_sif::datetime::time_zone(tz).is_none() {
            continue;
        }
        return start_time_info(provider, options, tz, now);
    }
    Ok(TimeInfo::INVALID)
}

/// The time info of the first shape point in a time zone (the end of `init_time_info`),
/// shared with map matching.
pub(crate) fn start_time_info<P: GraphTileProvider>(
    provider: &P,
    options: &mut Options,
    tz: u32,
    now: jiff::Timestamp,
) -> Result<TimeInfo, ThorError> {
    let Some(first) = options.shape.first_mut() else {
        return Ok(TimeInfo::INVALID);
    };
    let time = time_of(first);
    if first.date_time.is_empty() && time != -1.0 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mainline converts the double to a uint64"
        )]
        let seconds = time as u64;
        first.date_time = seconds_to_date(seconds, tz);
    }
    location_time_info_or(provider, first, tz, now)
}

/// Whether the shape points between `from` and `to` are the edge's own shape points
/// (`check_shape`). From the origin (`from` 0) the shape may start partway along the edge;
/// otherwise every point of the edge must be there.
fn check_shape<T: GraphTile>(
    tile: &T,
    edge_id: GraphId,
    edge: &DirectedEdge,
    shape: &[PointLL],
    from: usize,
    to: usize,
) -> Result<bool, ThorError> {
    if to - from == 1 && edge.length() == 0 {
        return Ok(true);
    }
    let info = tile.get_edge_info(edge)?;
    let edge_shape = info
        .decode_raw_shape::<f64>()
        .map_err(|e| ThorError::Shape(edge_id, e))?;
    let size = edge_shape.len();
    // `int32_t i = edge_shape.size() - (to - from)`.
    let Some(first) = size.checked_sub(to - from) else {
        return Ok(false);
    };
    if first < 1 || (from > 0 && first != 1) {
        return Ok(false);
    }
    let forward = edge.edge_info_is_forward();
    for (j, i) in (from + 1..to).zip(first..) {
        let shape_idx = if forward { i } else { size - 1 - i };
        let Some(c) = edge_shape.get(shape_idx) else {
            return Ok(false);
        };
        if !approximately_equal(shape[j], PointLL::new(c.x, c.y)) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Where a node's expansion is (the loops of mainline's `expand_from_node`).
#[derive(Clone, Copy)]
enum Phase {
    /// Checking the end, before the edges.
    Start,
    /// Trying the node's `i`th edge (counting from its first).
    Edges(u32),
    /// Waiting for the walk from the end of edge `i`, which cost this much.
    AfterEdge(u32, Cost),
    /// About to try the node's transitions.
    TransitionsStart,
    /// Trying the node's `i`th transition.
    Transitions(u32),
    /// Waiting for the walk from transition `i`.
    AfterTransition(u32),
}

/// A call of mainline's `expand_from_node`.
struct Frame<T> {
    /// The shape point at the node.
    correlated_index: usize,
    tile: T,
    node_id: GraphId,
    node: NodeInfo,
    from_transition: bool,
    phase: Phase,
    /// Where the loops over the node's edges and transitions started counting
    /// (`start_de` and `start_trans`): mainline counts from there, but takes the edges and
    /// transitions from the node's first, so a node resumed at `start_de` tries its first
    /// `edge_count - start_de` edges.
    start_edge: u32,
    start_transition: u32,
}

impl<T: GraphTile> Frame<T> {
    /// A call at a node.
    fn new(
        correlated_index: usize,
        tile: T,
        node_id: GraphId,
        from_transition: bool,
    ) -> Result<Self, ThorError> {
        let node = graph::node(&tile, node_id)?.clone();
        Ok(Self {
            correlated_index,
            tile,
            node_id,
            node,
            from_transition,
            phase: Phase::Start,
            start_edge: 0,
            start_transition: 0,
        })
    }
}

/// What the walk shares (mainline's arguments passed by reference).
struct Walk<'a, P> {
    provider: &'a P,
    costing: &'a AnyCosting,
    mode: TravelMode,
    shape: &'a [Location],
    points: Vec<PointLL>,
    /// The distance from the previous shape point, and from the first.
    distances: Vec<(f32, f32)>,
    time_info: TimeInfo,
    use_timestamps: bool,
    use_shortcuts: bool,
    end_nodes: Vec<EndEdge>,
    /// The last edge and transition followed from each shape point, by level.
    followed_edges: Vec<[(u32, u32); LEVELS]>,
}

impl<P: GraphTileProvider> Walk<'_, P> {
    /// The seconds since the first shape point of a shape point's time.
    fn timestamp(&self, index: usize) -> f32 {
        let first = self.shape.first().map_or(0.0, time_of);
        let at = self.shape.get(index).map_or(0.0, time_of);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline stores the double difference in a float"
        )]
        let secs = (at - first) as f32;
        secs
    }

    /// The end node entry for a node, if the walk can end there.
    fn end_node(&self, node: GraphId) -> Option<&EndEdge> {
        self.end_nodes.iter().find(|e| e.node == node)
    }

    /// Walks the shape from a node (mainline's recursive `expand_from_node`), returning the node
    /// the walk ended at, or `None` if it can't be walked from there.
    ///
    /// `path`, `elapsed`, and `prev_label` are shared by the whole walk, as in mainline.
    #[expect(clippy::too_many_lines, reason = "mainline's function, step by step")]
    fn expand_from_node(
        &mut self,
        start: Frame<P::TileHandle>,
        prev_label: &mut EdgeLabel,
        elapsed: &mut Cost,
        path: &mut Vec<WalkedEdge>,
    ) -> Result<Option<GraphId>, ThorError> {
        let total_distance = self.distances.last().map_or(0.0, |d| d.1);
        let mut stack = vec![start];
        // The result of the frame just popped, for its caller.
        let mut returned: Option<bool> = None;
        while let Some(frame) = stack.last_mut() {
            let ci = frame.correlated_index;
            let level = usize::from(frame.node_id.level());
            match (frame.phase, returned.take()) {
                (Phase::Start, _) => {
                    if let Some(end) = self.end_node(frame.node_id)
                        && equal_f32(
                            self.distances[ci].1 + end.distance,
                            total_distance,
                            TOTAL_DISTANCE_EPSILON,
                        )
                    {
                        // Can't end on a shortcut.
                        if path.last().is_some_and(|e| e.is_shortcut) {
                            stack.pop();
                            returned = Some(false);
                            continue;
                        }
                        return Ok(Some(frame.node_id));
                    }
                    let followed = self
                        .followed_edges
                        .get(ci)
                        .and_then(|f| f.get(level))
                        .ok_or(ThorError::EdgeWalk("a node is on the transit level"))?;
                    frame.start_edge = followed.0;
                    frame.phase = Phase::Edges(followed.0);
                }
                (Phase::Edges(i), _) => {
                    if i >= u32::from(frame.node.edge_count()) {
                        frame.phase = Phase::TransitionsStart;
                        continue;
                    }
                    self.followed_edges[ci][level].0 = i;
                    frame.phase = Phase::Edges(i + 1);
                    // Mainline's `de` and `edge_id` start at the node's first edge, whatever
                    // `start_de` is.
                    let offset = i - frame.start_edge;
                    let edge_id = frame
                        .node_id
                        .with_feature_index(u64::from(frame.node.edge_index()) + u64::from(offset))
                        .map_err(GraphTileProviderError::from)?;
                    let de = graph::edge(&frame.tile, edge_id)?.clone();

                    // Skip shortcuts and transit connection edges.
                    if (!self.use_shortcuts && de.is_shortcut())
                        || matches!(de.road_use(), Ok(RoadUse::TransitConnection))
                    {
                        continue;
                    }
                    // Don't loop back over the last edge or two (edges shorter than the
                    // tolerance can make loops).
                    let n = path.len();
                    if n > 1
                        && (path[n - 2].edge.edge_id == edge_id
                            || path[n - 1].edge.edge_id == edge_id)
                    {
                        continue;
                    }
                    let end_node_id = de.end_node_id();
                    let Some(end_node_tile) =
                        graph::tile_for(self.provider, &frame.tile, end_node_id)?
                    else {
                        continue;
                    };
                    let de_end_ll =
                        graph::node_ll(&end_node_tile, graph::node(&end_node_tile, end_node_id)?);
                    #[expect(clippy::cast_precision_loss, reason = "mainline's float")]
                    let de_length_f = de.length() as f32;
                    let de_length = length_comparison(de_length_f, true);

                    // Follow the shape until it reaches the edge's end node,
                    // or is longer than the edge.
                    let mut index = ci + 1;
                    let mut length = 0.0_f32;
                    while index < self.shape.len() {
                        length += self.distances[index].0;
                        if length > de_length {
                            break;
                        }
                        if approximately_equal(self.points[index], de_end_ll)
                            && de_length_f < length_comparison(length, true)
                            && check_shape(&frame.tile, edge_id, &de, &self.points, ci, index)?
                        {
                            let node = &frame.node;
                            let offset_time_info =
                                self.time_info.forward(elapsed.secs, tz_of(node));
                            let transition_cost = self.costing.transition_cost(
                                &de,
                                &frame.tile,
                                node,
                                &*prev_label,
                                self.provider,
                                false,
                            )?;
                            let edge_cost = self.costing.edge_cost(
                                &de,
                                edge_id,
                                &frame.tile,
                                offset_time_info.seconds_of_week(),
                            )?;
                            let cost = transition_cost + edge_cost.cost;
                            *elapsed += cost;
                            if self.use_timestamps {
                                elapsed.secs = self.timestamp(index);
                            }
                            path.push(WalkedEdge {
                                edge: PathEdge {
                                    edge_id,
                                    mode: self.mode,
                                    elapsed_cost: *elapsed,
                                    transition_cost,
                                    path_distance: 0.0,
                                    restriction_index: u8::MAX,
                                    start_node_is_recovered: false,
                                    is_disconnected: false,
                                },
                                is_shortcut: de.is_shortcut(),
                            });
                            let turn = self.costing.base().turn_type(
                                valinor_sif::Label::opp_local_idx(&*prev_label),
                                node,
                                &de,
                                None,
                            );
                            *prev_label = EdgeLabel::new(
                                edge_id,
                                &de,
                                LabelState {
                                    predecessor: None,
                                    mode: self.mode,
                                    path_distance: 0,
                                    closure_pruning: true,
                                    has_measured_speed: !edge_cost.flow_sources.is_empty(),
                                    internal_turn: turn,
                                    destonly: false,
                                },
                            );
                            frame.phase = Phase::AfterEdge(i, cost);
                            let child = Frame::new(index, end_node_tile, end_node_id, false)?;
                            stack.push(child);
                            break;
                        }
                        index += 1;
                    }
                }
                (Phase::AfterEdge(i, cost), Some(false)) => {
                    // The walk failed along this edge: take it back and try the next edge.
                    *elapsed -= cost;
                    path.pop();
                    frame.phase = Phase::Edges(i + 1);
                }
                (Phase::TransitionsStart, _) => {
                    let start_trans = self.followed_edges[ci][level].1;
                    if !frame.from_transition && frame.node.transition_count() > 0 {
                        frame.start_transition = start_trans;
                        frame.phase = Phase::Transitions(start_trans);
                    } else {
                        stack.pop();
                        returned = Some(false);
                    }
                }
                (Phase::Transitions(i), _) => {
                    if i >= u32::from(frame.node.transition_count()) {
                        stack.pop();
                        returned = Some(false);
                        continue;
                    }
                    // As with edges, from the node's first transition.
                    let transitions = frame.tile.get_transitions(&frame.node);
                    let Some(transition) = usize::try_from(i - frame.start_transition)
                        .ok()
                        .and_then(|t| transitions.get(t))
                    else {
                        return Err(ThorError::EdgeWalk(
                            "a node's transition is not in its tile",
                        ));
                    };
                    self.followed_edges[ci][level].1 = i;
                    let end_node_id = transition.corresponding_end_node_id();
                    let Some(end_node_tile) = graph::tile(self.provider, end_node_id)? else {
                        frame.phase = Phase::Transitions(i + 1);
                        continue;
                    };
                    frame.phase = Phase::AfterTransition(i);
                    let child = Frame::new(ci, end_node_tile, end_node_id, true)?;
                    stack.push(child);
                }
                (Phase::AfterTransition(i), Some(false)) => {
                    frame.phase = Phase::Transitions(i + 1);
                }
                (Phase::AfterEdge(..) | Phase::AfterTransition(_), _) => {
                    return Err(ThorError::EdgeWalk("a walk returned nothing"));
                }
            }
        }
        Ok(None)
    }
}

/// Walks a shape along the graph (mainline's `RouteMatcher::FormPath`).
///
/// On success, returns the path's edges (one leg) and adds the origin and destination
/// candidates the path uses to the first and last shape points' correlations,
/// as mainline does; `Ok(None)` is a walk that found no path.
///
/// # Errors
///
/// Where mainline throws: a shape of fewer than two points, a destination with no candidate
/// to end on, invalid candidates, missing tiles where the walk needs them, and tile data that
/// can't be read. Mainline's Thor reports these, and a walk that finds no path, as error 443.
#[expect(clippy::too_many_lines, reason = "mainline's function, step by step")]
pub(crate) fn form_path<P: GraphTileProvider>(
    provider: &P,
    costing: &AnyCosting,
    options: &mut Options,
    use_timestamps: bool,
    use_shortcuts: bool,
    now: jiff::Timestamp,
) -> Result<Option<Vec<PathEdge>>, ThorError> {
    if options.shape.len() < 2 {
        return Err(ThorError::EdgeWalk("Invalid shape - less than 2 points"));
    }
    let points: Vec<PointLL> = options
        .shape
        .iter()
        .map(|l| point_of(l.ll.as_ref()))
        .collect();
    let mut distances = Vec::with_capacity(points.len());
    distances.push((0.0_f32, 0.0_f32));
    let mut total_distance = 0.0_f32;
    for pair in points.windows(2) {
        #[expect(clippy::cast_possible_truncation, reason = "mainline's float")]
        let d = pair[1].distance(pair[0]) as f32;
        total_distance += d;
        distances.push((d, total_distance));
    }

    let (Some(origin), Some(destination)) = (
        options.locations.first().cloned(),
        options.locations.last().cloned(),
    ) else {
        return Err(ThorError::NoCandidates(0));
    };
    let end_nodes = end_edges(provider, &destination)?;
    let origin_edges = origin
        .correlation
        .as_ref()
        .map_or(&[][..], |c| &c.edges[..]);
    let time_info = walk_time_info(provider, origin_edges, options, now)?;

    let mode = costing.base().travel_mode();
    let mut walk = Walk {
        provider,
        costing,
        mode,
        shape: &options.shape,
        points,
        distances,
        time_info,
        use_timestamps,
        use_shortcuts,
        end_nodes,
        followed_edges: vec![[(0, 0); LEVELS]; options.shape.len()],
    };

    // Walk from each origin candidate in turn.
    let mut found: Option<(Vec<PathEdge>, CandidateEdge, CandidateEdge)> = None;
    'candidates: for edge in origin_edges {
        // An origin at a node skips the edges that end there.
        if edge.end_node {
            continue;
        }
        let begin_id =
            graph_id(edge.graph_id).map_err(|_| ThorError::EdgeWalk("Invalid begin edge id"))?;
        let begin_tile =
            graph::tile(provider, begin_id)?.ok_or(ThorError::EdgeWalk("Begin tile is null"))?;
        let de = graph::edge(&begin_tile, begin_id)?.clone();
        let end_node_id = de.end_node_id();
        let end_node_tile = graph::tile_for(provider, &begin_tile, end_node_id)?
            .ok_or(ThorError::EdgeWalk("End node tile is null"))?;
        let de_end_ll = graph::node_ll(&end_node_tile, graph::node(&end_node_tile, end_node_id)?);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline stores the double product in a float"
        )]
        let de_remaining_length = (f64::from(de.length()) * (1.0 - edge.percent_along)) as f32;
        let de_length = length_comparison(de_remaining_length, true);
        #[expect(clippy::cast_possible_truncation, reason = "mainline's float")]
        let percent_along = edge.percent_along as f32;

        let mut path: Vec<WalkedEdge> = Vec::new();
        let mut elapsed = Cost::ZERO;
        let mut index = 1;
        let mut length = 0.0_f32;
        while index < walk.shape.len() {
            length += walk.distances[index].0;
            if length > de_length {
                break;
            }
            if approximately_equal(walk.points[index], de_end_ll)
                && de_remaining_length < length_comparison(length, true)
                && check_shape(&begin_tile, begin_id, &de, &walk.points, 0, index)?
            {
                // The first edge: from the origin to its end, at the start time.
                let offset_time_info = time_info;
                let partial = costing.partial_edge_cost(
                    &de,
                    begin_id,
                    Some(begin_id),
                    &begin_tile,
                    offset_time_info.seconds_of_week(),
                    percent_along,
                    1.0,
                )?;
                elapsed += partial.cost;
                if use_timestamps {
                    elapsed.secs = walk.timestamp(index);
                }
                path.push(WalkedEdge {
                    edge: PathEdge {
                        edge_id: begin_id,
                        mode,
                        elapsed_cost: elapsed,
                        transition_cost: Cost::ZERO,
                        path_distance: 0.0,
                        restriction_index: u8::MAX,
                        start_node_is_recovered: false,
                        is_disconnected: false,
                    },
                    is_shortcut: false,
                });
                let mut prev_label = EdgeLabel::new(
                    begin_id,
                    &de,
                    LabelState {
                        predecessor: None,
                        mode,
                        path_distance: 0,
                        closure_pruning: true,
                        has_measured_speed: !partial.flow_sources.is_empty(),
                        internal_turn: InternalTurn::NoTurn,
                        destonly: false,
                    },
                );

                let frame = Frame::new(index, end_node_tile, end_node_id, false)?;
                let Some(end_node) =
                    walk.expand_from_node(frame, &mut prev_label, &mut elapsed, &mut path)?
                else {
                    // The walk from this candidate's end failed: mainline gives up.
                    return Ok(None);
                };
                let Some(end) = walk.end_node(end_node) else {
                    return Ok(None);
                };
                // A destination at a node: the candidate the path's last edge is.
                let mut end_edge = end.candidate.clone();
                let end_distance_node = end.node;
                if end.candidate.end_node {
                    let last = path.last().map(|e| e.edge.edge_id.value());
                    end_edge = destination
                        .correlation
                        .as_ref()
                        .and_then(|c| c.edges.iter().find(|e| Some(e.graph_id) == last))
                        .cloned()
                        .ok_or(ThorError::EdgeWalk(
                            "Could not find destination candidate in shape-walked path",
                        ))?;
                }
                if !end_edge.end_node {
                    let end_id = graph_id(end_edge.graph_id)?;
                    let end_tile = graph::tile(provider, end_id)?
                        .ok_or(ThorError::EdgeWalk("End edge tile is null"))?;
                    let end_de = graph::edge(&end_tile, end_id)?.clone();
                    // The end edge starts at the node the walk ended at, in the edge's tile.
                    let node = graph::node(&end_tile, end_distance_node)?.clone();
                    let transition_cost = costing.transition_cost(
                        &end_de,
                        &end_tile,
                        &node,
                        &prev_label,
                        provider,
                        false,
                    )?;
                    #[expect(clippy::cast_possible_truncation, reason = "mainline's float")]
                    let end_pct = end_edge.percent_along as f32;
                    // At the start time (mainline reuses the first edge's time info here).
                    let partial = costing.partial_edge_cost(
                        &end_de,
                        end_id,
                        Some(end_id),
                        &end_tile,
                        offset_time_info.seconds_of_week(),
                        0.0,
                        end_pct,
                    )?;
                    elapsed += transition_cost + partial.cost;
                    if use_timestamps {
                        elapsed.secs = walk.timestamp(walk.shape.len() - 1);
                    }
                    path.push(WalkedEdge {
                        edge: PathEdge {
                            edge_id: end_id,
                            mode,
                            elapsed_cost: elapsed,
                            transition_cost,
                            path_distance: 0.0,
                            restriction_index: u8::MAX,
                            start_node_is_recovered: false,
                            is_disconnected: false,
                        },
                        is_shortcut: false,
                    });
                }
                found = Some((
                    path.into_iter().map(|e| e.edge).collect(),
                    edge.clone(),
                    end_edge,
                ));
                break 'candidates;
            }
            index += 1;
        }

        // A trivial route on one edge, if the shape never got longer than the edge.
        if length <= de_length
            && let Some(end) = walk
                .end_nodes
                .iter()
                .find(|end| end.candidate.graph_id == edge.graph_id)
        {
            #[expect(clippy::cast_possible_truncation, reason = "mainline's float")]
            let end_pct = end.candidate.percent_along as f32;
            let partial = costing.partial_edge_cost(
                &de,
                begin_id,
                Some(begin_id),
                &begin_tile,
                time_info.seconds_of_week(),
                percent_along,
                end_pct,
            )?;
            elapsed += partial.cost;
            if use_timestamps {
                elapsed.secs = walk.timestamp(walk.shape.len() - 1);
            }
            found = Some((
                vec![PathEdge {
                    edge_id: begin_id,
                    mode,
                    elapsed_cost: elapsed,
                    transition_cost: Cost::ZERO,
                    path_distance: 0.0,
                    restriction_index: u8::MAX,
                    start_node_is_recovered: false,
                    is_disconnected: false,
                }],
                edge.clone(),
                end.candidate.clone(),
            ));
            break;
        }
    }

    let Some((path, begin, end)) = found else {
        return Ok(None);
    };
    // The candidates the path uses, on the shape's first and last points.
    if let Some(first) = options.shape.first_mut() {
        first.correlation.get_or_insert_default().edges.push(begin);
    }
    if let Some(last) = options.shape.last_mut() {
        last.correlation.get_or_insert_default().edges.push(end);
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_lengths_as_mainline() {
        // 5% with a floor of 10 m and a cap of 25 m.
        assert_eq!(length_comparison(100.0, true), 110.0);
        assert_eq!(length_comparison(300.0, true), 315.0);
        assert_eq!(length_comparison(1000.0, true), 1025.0);
        assert_eq!(length_comparison(1000.0, false), 1100.0);
        assert_eq!(length_comparison(2000.0, false), 2100.0);
    }

    #[test]
    fn compares_points_within_ll_epsilon() {
        let a = PointLL::new(1.0, 42.0);
        assert!(approximately_equal(a, PointLL::new(1.000_019, 42.0)));
        assert!(!approximately_equal(a, PointLL::new(1.000_021, 42.0)));
        assert!(approximately_equal(a, PointLL::new(1.0, 41.999_981)));
        assert!(equal_f32(10.0, 15.0, 5.0));
        assert!(!equal_f32(10.0, 15.1, 5.0));
    }
}
