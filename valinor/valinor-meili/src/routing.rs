//! Routing between candidates (mainline's `meili/routing.cc`):
//! a one-to-many Dijkstra search (with an A* heuristic toward the next measurement)
//! from one candidate to all the candidates of the next measurement.
//!
//! Costs are route distances in meters, not Sif's costs:
//! Sif is asked only whether an edge is allowed (and for its travel time),
//! and turns cost what Meili's own turn cost table says.
//! The search settles nodes and destinations rather than edges,
//! so a candidate at a node is reached when the node is.
//!
//! The arithmetic follows mainline's, rounding to `float` where it does,
//! with mainline's double bucket queue: which of two nearly equal routes wins decides
//! the matched edges.
//! What differs is the order destinations sharing an edge are put in the queue
//! (mainline iterates an `unordered_set`; here they are in index order),
//! which only matters between labels with equal costs, and that the search stops as soon as
//! every destination is found (mainline can keep an emptied entry in its destination map,
//! which makes it search on until the queue runs dry, without changing what it found).

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares floats exactly"
)]

use std::collections::HashMap;
use valhalla_graphtile::double_bucket_queue::{BucketQueue, SortCost};
use valhalla_graphtile::graph_reader::GraphReadError;
use valhalla_graphtile::graph_reader::{GraphReader, Tile, edge, node, node_ll, shape};
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::midgard::{DistanceApproximator, PointLL, narrow};
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};
use valhalla_graphtile::{GraphId, RoadClass, RoadUse};
use valhalla_proto::location::Type as LocationType;
use valinor_sif::{
    AnyCosting, Cost, Costing, DisallowMask, EdgeAccess, EdgeLabel, InternalTurn,
    Label as SifLabel, LabelState, TimeContext, TravelMode,
};

use crate::candidate_search::Candidate;
use crate::error::MeiliError;
use crate::geometry::{clamp_heading, turn_degree180};

/// No destination (`kInvalidDestination`).
pub(crate) const INVALID_DEST: u16 = u16::MAX;
/// No label (`baldr::kInvalidLabel`).
pub(crate) const INVALID_LABEL: u32 = u32::MAX;
/// What a label without an edge reports as its edge ID to costing, which doesn't read it.
const PLACEHOLDER_ID: GraphId = match GraphId::try_from_components(0, 0, 0) {
    Ok(id) => id,
    Err(_) => panic!("0/0/0 is a valid graph ID"),
};

/// A label of Meili's search (mainline's `meili::Label`, a `sif::EdgeLabel` with a node or
/// destination, the part of the edge covered, and the turn cost so far).
///
/// A label reaches either a node or a destination (a candidate along an edge).
/// Labels that start a search copy the last label of the route that reached the origin,
/// so that costing sees the edge the trace arrived on (for U-turns and turn restrictions).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Label {
    pub(crate) node_id: Option<GraphId>,
    pub(crate) dest: u16,
    pub(crate) edge_id: Option<GraphId>,
    pub(crate) source: f32,
    pub(crate) target: f32,
    pub(crate) cost: Cost,
    pub(crate) turn_cost: f32,
    sortcost: f32,
    pub(crate) predecessor: u32,
    pub(crate) restriction_idx: u8,
    mode: TravelMode,
    /// What costing reads about the label's edge; `None` for a label without one
    /// (mainline's default-constructed label, whose fields are all zero).
    edge: Option<EdgeLabel>,
}

impl Label {
    /// Mainline's default `Label()`: no node, edge, or predecessor, and all else zero.
    const fn dummy() -> Self {
        Self {
            node_id: None,
            dest: 0,
            edge_id: None,
            source: 0.0,
            target: 0.0,
            cost: Cost::ZERO,
            turn_cost: 0.0,
            sortcost: 0.0,
            predecessor: INVALID_LABEL,
            restriction_idx: 0,
            mode: TravelMode::Drive,
            edge: None,
        }
    }

    /// Mainline's checked constructor.
    #[expect(clippy::too_many_arguments, reason = "mainline's constructor")]
    fn new(
        node_id: Option<GraphId>,
        dest: u16,
        edge_id: GraphId,
        source: f32,
        target: f32,
        cost: Cost,
        turn_cost: f32,
        sortcost: f32,
        predecessor: u32,
        edge: &DirectedEdge,
        mode: TravelMode,
        restriction_idx: u8,
    ) -> Result<Self, MeiliError> {
        if !(0.0 <= source && source <= target && target <= 1.0) {
            return Err(MeiliError::Logic("invalid label source or target"));
        }
        if cost.cost < 0.0 {
            return Err(MeiliError::Logic("invalid label cost"));
        }
        if turn_cost < 0.0 {
            return Err(MeiliError::Logic("invalid label turn cost"));
        }
        Ok(Self {
            node_id,
            dest,
            edge_id: Some(edge_id),
            source,
            target,
            cost,
            turn_cost,
            sortcost,
            predecessor,
            restriction_idx,
            mode,
            edge: Some(EdgeLabel::new(
                edge_id,
                edge,
                LabelState {
                    predecessor: None,
                    mode,
                    path_distance: 0,
                    closure_pruning: true,
                    has_measured_speed: false,
                    internal_turn: InternalTurn::NoTurn,
                    destonly: false,
                },
            )),
        })
    }

    /// A copy that starts a search (`InitAsOrigin`): costs and the covered part are zeroed,
    /// with no predecessor, and the node or destination given; the edge is kept.
    fn as_origin(self, mode: TravelMode, dest: u16, node_id: Option<GraphId>) -> Self {
        Self {
            source: 0.0,
            target: 0.0,
            turn_cost: 0.0,
            sortcost: 0.0,
            cost: Cost::ZERO,
            predecessor: INVALID_LABEL,
            mode,
            dest,
            node_id,
            ..self
        }
    }

    /// The previous label of the route, if any.
    #[inline]
    pub(crate) fn predecessor(&self) -> Option<u32> {
        (self.predecessor != INVALID_LABEL).then_some(self.predecessor)
    }
}

impl SortCost for Label {
    #[inline]
    fn sortcost(&self) -> f32 {
        self.sortcost
    }
}

/// What costing reads, from the label's edge or (without one) mainline's zeroed fields.
impl SifLabel for Label {
    #[inline]
    fn edge_id(&self) -> GraphId {
        // Costing's access checks don't read the predecessor's ID (only elevator transition
        // costs do, which Meili doesn't use), so a label without an edge reports its edge
        // label's placeholder.
        self.edge.map_or(PLACEHOLDER_ID, |e| e.edge_id())
    }
    #[inline]
    fn predecessor(&self) -> Option<u32> {
        Self::predecessor(self)
    }
    #[inline]
    fn opp_local_idx(&self) -> u8 {
        self.edge.map_or(0, |e| e.opp_local_idx())
    }
    #[inline]
    fn restrictions(&self) -> u8 {
        self.edge.map_or(0, |e| e.restrictions())
    }
    #[inline]
    fn road_use(&self) -> RoadUse {
        self.edge.map_or(RoadUse::Road, |e| e.road_use())
    }
    #[inline]
    fn classification(&self) -> RoadClass {
        self.edge
            .map_or(RoadClass::Motorway, |e| e.classification())
    }
    #[inline]
    fn toll(&self) -> bool {
        self.edge.is_some_and(|e| e.toll())
    }
    #[inline]
    fn bridge(&self) -> bool {
        self.edge.is_some_and(|e| e.bridge())
    }
    #[inline]
    fn tunnel(&self) -> bool {
        self.edge.is_some_and(|e| e.tunnel())
    }
    #[inline]
    fn unpaved(&self) -> bool {
        self.edge.is_some_and(|e| e.unpaved())
    }
    #[inline]
    fn deadend(&self) -> bool {
        self.edge.is_some_and(|e| e.deadend())
    }
    #[inline]
    fn destonly(&self) -> bool {
        self.edge.is_some_and(|e| e.destonly())
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
        self.edge.is_some_and(|e| e.closure_pruning())
    }
    #[inline]
    fn has_measured_speed(&self) -> bool {
        false
    }
    #[inline]
    fn internal_turn(&self) -> InternalTurn {
        InternalTurn::NoTurn
    }
}

/// A label's place in the search (mainline's `Status`).
#[derive(Clone, Copy, Debug)]
struct Status {
    label_idx: u32,
    permanent: bool,
}

/// The queue and statuses of a search (the transient part of mainline's `LabelSet`;
/// the labels themselves are kept by the state the search starts from).
#[derive(Debug, Default)]
struct LabelSet {
    queue: BucketQueue,
    node_status: HashMap<GraphId, Status>,
    dest_status: Vec<Option<Status>>,
}

fn label_index(labels: &[Label]) -> Result<u32, MeiliError> {
    u32::try_from(labels.len()).map_err(|_| MeiliError::Logic("too many labels"))
}

impl LabelSet {
    fn reset(&mut self, max_cost: f32, destinations: usize) {
        self.queue.reuse(0.0, max_cost, 1);
        self.node_status.clear();
        self.dest_status.clear();
        self.dest_status.resize(destinations, None);
    }

    /// Adds an origin label at a destination (`put(dest, mode, edgelabel)`).
    fn put_origin_dest(
        &mut self,
        labels: &mut Vec<Label>,
        dest: u16,
        mode: TravelMode,
        edgelabel: Option<&Label>,
    ) -> Result<(), MeiliError> {
        let slot = self
            .dest_status
            .get_mut(usize::from(dest))
            .ok_or(MeiliError::Logic("invalid destination"))?;
        if slot.is_none() {
            let idx = label_index(labels)?;
            *slot = Some(Status {
                label_idx: idx,
                permanent: false,
            });
            let label = edgelabel
                .copied()
                .unwrap_or_else(Label::dummy)
                .as_origin(mode, dest, None);
            labels.push(label);
            self.queue.add(idx, label.sortcost);
        }
        Ok(())
    }

    /// Adds an origin label at a node (`put(nodeid, mode, edgelabel)`).
    fn put_origin_node(
        &mut self,
        labels: &mut Vec<Label>,
        node_id: GraphId,
        mode: TravelMode,
        edgelabel: Option<&Label>,
    ) -> Result<(), MeiliError> {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.node_status.entry(node_id) {
            let idx = label_index(labels)?;
            entry.insert(Status {
                label_idx: idx,
                permanent: false,
            });
            let label = edgelabel.copied().unwrap_or_else(Label::dummy).as_origin(
                mode,
                INVALID_DEST,
                Some(node_id),
            );
            labels.push(label);
            self.queue.add(idx, label.sortcost);
        }
        Ok(())
    }

    /// Adds or improves a label (`put(nodeid, ...)` when `label.node_id` is set,
    /// else `put(dest, ...)`).
    fn put(&mut self, labels: &mut Vec<Label>, label: Label) -> Result<(), MeiliError> {
        let found = if let Some(node_id) = label.node_id {
            self.node_status.get(&node_id).copied()
        } else {
            *self
                .dest_status
                .get(usize::from(label.dest))
                .ok_or(MeiliError::Logic("invalid destination"))?
        };
        let Some(status) = found else {
            let idx = label_index(labels)?;
            labels.push(label);
            self.queue.add(idx, label.sortcost);
            let status = Status {
                label_idx: idx,
                permanent: false,
            };
            if let Some(node_id) = label.node_id {
                self.node_status.insert(node_id, status);
            } else if let Some(slot) = self.dest_status.get_mut(usize::from(label.dest)) {
                *slot = Some(status);
            }
            return Ok(());
        };
        let existing = labels
            .get_mut(status.label_idx as usize)
            .ok_or(MeiliError::Logic("a status points past the labels"))?;
        if !status.permanent && label.sortcost < existing.sortcost {
            self.queue
                .decrease(status.label_idx, existing.sortcost, label.sortcost);
            *existing = label;
        }
        Ok(())
    }

    /// Takes the next label and makes it permanent (`pop`).
    fn pop(&mut self, labels: &[Label]) -> Result<Option<u32>, MeiliError> {
        let Some(idx) = self.queue.pop(labels) else {
            return Ok(None);
        };
        let label = labels
            .get(idx as usize)
            .ok_or(MeiliError::Logic("the queue points past the labels"))?;
        let status = if let Some(node_id) = label.node_id {
            self.node_status.get_mut(&node_id)
        } else {
            self.dest_status
                .get_mut(usize::from(label.dest))
                .and_then(Option::as_mut)
        }
        .ok_or(MeiliError::Logic(
            "all labels in the queue should have a status",
        ))?;
        if status.label_idx != idx {
            return Err(MeiliError::Logic("a status is out of sync with the queue"));
        }
        if status.permanent {
            return Err(MeiliError::Logic(
                "the principle of optimality is violated during routing",
            ));
        }
        status.permanent = true;
        Ok(Some(idx))
    }
}

/// Destinations by node and by edge (`node_dests` and `edge_dests`),
/// each list in increasing destination order.
#[derive(Debug, Default)]
struct Destinations {
    by_node: HashMap<GraphId, Vec<u16>>,
    by_edge: HashMap<GraphId, Vec<u16>>,
}

/// Adds `dest` to a list built in increasing destination order (an `unordered_set` insert).
fn insert_dest(list: &mut Vec<u16>, dest: u16) {
    if list.last() != Some(&dest) {
        list.push(dest);
    }
}

/// The search's reusable storage.
#[derive(Debug, Default)]
pub(crate) struct Router {
    set: LabelSet,
    dests: Destinations,
    /// The label that reached each destination, by destination index.
    pub(crate) results: Vec<Option<u32>>,
}

/// One search's inputs (mainline's `find_shortest_path` arguments).
pub(crate) struct Search<'a, P: GraphTileProvider> {
    pub(crate) graph: GraphReader<'a, P>,
    pub(crate) costing: &'a AnyCosting,
    /// The origin (index 0) and the destinations.
    pub(crate) destinations: &'a [&'a Candidate],
    /// The origin's measurement type (for immediate U-turns).
    pub(crate) origin_type: LocationType,
    /// Centered at the next measurement, for the heuristic.
    pub(crate) approximator: DistanceApproximator,
    pub(crate) radius: f32,
    /// The last label of the route to the origin, if any.
    pub(crate) edgelabel: Option<Label>,
    pub(crate) turn_cost_table: &'a [f32; 181],
    pub(crate) max_dist: f32,
    pub(crate) max_time: f32,
}

/// `max(0, distance - search_radius)` from a node to the next measurement:
/// a destination may be anywhere within the radius.
#[inline]
fn heuristic(approximator: &DistanceApproximator, search_radius: f32, ll: PointLL) -> f32 {
    let search_rad2 = search_radius * search_radius;
    let d2 = narrow(approximator.distance_squared(ll));
    if d2 < search_rad2 {
        0.0
    } else {
        d2.sqrt() - search_radius
    }
}

/// An edge's start and end nodes (`GraphReader::GetDirectedEdgeNodes`):
/// `None` if the edge's tile isn't there; a `None` start node if the end node's tile isn't.
pub(crate) fn edge_nodes<P: GraphTileProvider>(
    graph: GraphReader<'_, P>,
    edge_id: GraphId,
) -> Result<Option<(Option<GraphId>, GraphId)>, GraphReadError> {
    let Some(tile) = graph.tile(edge_id)? else {
        return Ok(None);
    };
    let de = edge(&tile, edge_id)?;
    let end = de.end_node_id();
    let start = match graph.opposing(edge_id, &tile)? {
        Some((opp_id, opp_tile)) => Some(edge(&opp_tile, opp_id)?.end_node_id()),
        None => None,
    };
    Ok(Some((start, end)))
}

/// The heading of a shape at its last point toward the one before it (`at_back`),
/// or at its first point toward the second, clamped as mainline does; 0 without two points.
fn end_heading(points: &[PointLL], at_back: bool) -> u16 {
    let n = points.len();
    if n < 2 {
        return 0;
    }
    let (a, b) = if at_back {
        (points[n - 1], points[n - 2])
    } else {
        (points[0], points[1])
    };
    clamp_heading(a.heading(b))
}

/// The heading into the node at the end of `label`'s edge (`get_inbound_edgelabel_heading`):
/// from the node's headings when the opposing local index has one, else from the shape.
fn inbound_heading<P: GraphTileProvider>(
    graph: GraphReader<'_, P>,
    label: &Label,
    node: &NodeInfo,
) -> Result<u16, GraphReadError> {
    let idx = label.opp_local_idx();
    if idx < 8 {
        return Ok(node.heading(idx).unwrap_or(0));
    }
    // Mainline dereferences a missing tile here; there is no heading to read.
    let Some(edge_id) = label.edge_id else {
        return Ok(0);
    };
    let Some(tile) = graph.tile(edge_id)? else {
        return Ok(0);
    };
    let de = edge(&tile, edge_id)?;
    let points = shape(&tile.get_edge_info(de)?)?;
    Ok(end_heading(&points, de.edge_info_is_forward()))
}

/// The heading out of a node along `de` (`get_outbound_edge_heading`).
fn outbound_heading<T: GraphTile>(
    tile: &T,
    de: &DirectedEdge,
    node: &NodeInfo,
) -> Result<u16, GraphReadError> {
    let idx = de.local_edge_index();
    if idx < 8 {
        return Ok(node.heading(idx).unwrap_or(0));
    }
    let points = shape(&tile.get_edge_info(de)?)?;
    Ok(end_heading(&points, !de.edge_info_is_forward()))
}

/// Whether the search may take `de` after `pred` (`IsEdgeAllowed`): without a predecessor
/// edge, Loki's edge filter decides; continuing on the predecessor's own edge is allowed;
/// otherwise costing's access check does. That check always runs, for the restriction index.
fn is_edge_allowed<T: GraphTile>(
    costing: &AnyCosting,
    de: &DirectedEdge,
    edge_id: GraphId,
    pred: &Label,
    tile: &T,
) -> (bool, u8) {
    let valid_pred = (pred.edge_id.is_none()
        && costing.edge_filter(de, tile, DisallowMask::SHORTCUT))
        || Some(edge_id) == pred.edge_id;
    let mut access = EdgeAccess::new(0);
    let restricted = !costing.allowed(de, edge_id, tile, pred, TimeContext::NONE, &mut access);
    (valid_pred || !restricted, access.restriction_idx)
}

/// The travel time of a whole edge (`EdgeCost(edge, edgeid, tile).secs`).
fn edge_secs<T: GraphTile>(
    costing: &AnyCosting,
    de: &DirectedEdge,
    edge_id: GraphId,
    tile: &T,
) -> Result<f32, MeiliError> {
    Ok(costing
        .edge_cost_untimed(de, edge_id, tile)
        .map_err(MeiliError::EdgeCost)?
        .cost
        .secs)
}

/// `cost < max_dist && (max_time < 0 || secs < max_time)`.
#[inline]
fn within_limits(search: &Search<'_, impl GraphTileProvider>, cost: Cost) -> bool {
    cost.cost < search.max_dist && (search.max_time < 0.0 || cost.secs < search.max_time)
}

impl Router {
    /// Finds the routes from `destinations[0]` to the others (`find_shortest_path`),
    /// appending the labels to `labels` and leaving the label that reached each destination
    /// in [`Router::results`].
    ///
    /// # Errors
    ///
    /// Fails if a tile can't be read or a consistency check fails.
    pub(crate) fn find_shortest_path<P: GraphTileProvider>(
        &mut self,
        search: &Search<'_, P>,
        labels: &mut Vec<Label>,
    ) -> Result<(), MeiliError> {
        let Self {
            set,
            dests,
            results,
        } = self;
        let count = search.destinations.len();
        if count > usize::from(INVALID_DEST) {
            return Err(MeiliError::Logic("too many destinations"));
        }
        set.reset(search.max_dist, count);
        results.clear();
        results.resize(count, None);
        let mode = search.costing.base().travel_mode();
        let graph = search.graph;

        // `set_origin`.
        let origin_idx = 0_u16;
        let origin = search
            .destinations
            .first()
            .ok_or(MeiliError::Logic("no origin"))?;
        for candidate in &origin.edges {
            if candidate.begin_node || candidate.end_node {
                let Some((start, end)) = edge_nodes(graph, candidate.edge_id)? else {
                    continue;
                };
                let node_id = if candidate.begin_node {
                    start
                } else {
                    Some(end)
                };
                if let Some(node_id) = node_id {
                    let Some(tile) = graph.tile(node_id)? else {
                        continue;
                    };
                    if !search.costing.node_allowed(node(&tile, node_id)?) {
                        continue;
                    }
                    set.put_origin_node(labels, node_id, mode, search.edgelabel.as_ref())?;
                }
            } else {
                set.put_origin_dest(labels, origin_idx, mode, search.edgelabel.as_ref())?;
            }
        }

        // `set_destinations` (the origin is a destination too).
        dests.by_node.clear();
        dests.by_edge.clear();
        for (dest, location) in search.destinations.iter().enumerate() {
            let dest =
                u16::try_from(dest).map_err(|_| MeiliError::Logic("too many destinations"))?;
            for candidate in &location.edges {
                if candidate.begin_node || candidate.end_node {
                    let Some((start, end)) = edge_nodes(graph, candidate.edge_id)? else {
                        continue;
                    };
                    let node_id = if candidate.begin_node {
                        start
                    } else {
                        Some(end)
                    };
                    if let Some(node_id) = node_id {
                        insert_dest(dests.by_node.entry(node_id).or_default(), dest);
                    }
                } else {
                    insert_dest(dests.by_edge.entry(candidate.edge_id).or_default(), dest);
                }
            }
        }

        while let Some(label_idx) = set.pop(labels)? {
            let label = labels[label_idx as usize];
            if let Some(node_id) = label.node_id {
                if let Some(found) = dests.by_node.remove(&node_id) {
                    for dest in found {
                        results[usize::from(dest)] = Some(label_idx);
                    }
                }
                if dests.by_node.is_empty() && dests.by_edge.is_empty() {
                    break;
                }
                self::expand(
                    search, set, dests, labels, node_id, label_idx, &label, false,
                )?;
            } else {
                let dest = label.dest;
                results[usize::from(dest)] = Some(label_idx);
                for candidate in &search.destinations[usize::from(dest)].edges {
                    if let Some(list) = dests.by_edge.get_mut(&candidate.edge_id) {
                        list.retain(|&d| d != dest);
                        if list.is_empty() {
                            dests.by_edge.remove(&candidate.edge_id);
                        }
                    }
                }
                if dests.by_edge.is_empty() && dests.by_node.is_empty() {
                    break;
                }
                if dest != origin_idx {
                    continue;
                }
                expand_origin(search, set, dests, labels, label_idx, &label)?;
            }
        }
        Ok(())
    }
}

/// Expands from a node (mainline's `expand` lambda): along each edge it may take, to the
/// destinations on the edge and to its end node, and through the node's transitions to the
/// same node on other levels (once).
#[expect(clippy::too_many_arguments, reason = "mainline's lambda captures")]
#[expect(clippy::too_many_lines, reason = "mainline's `expand`, step by step")]
fn expand<P: GraphTileProvider>(
    search: &Search<'_, P>,
    set: &mut LabelSet,
    dests: &Destinations,
    labels: &mut Vec<Label>,
    node_id: GraphId,
    label_idx: u32,
    label: &Label,
    from_transition: bool,
) -> Result<(), MeiliError> {
    let graph = search.graph;
    let Some(tile) = graph.tile(node_id)? else {
        return Ok(());
    };
    let ni = node(&tile, node_id)?;
    if !search.costing.node_allowed(ni) {
        return Ok(());
    }
    let inbound = if label.edge_id.is_some() {
        inbound_heading(graph, label, ni)?
    } else {
        0
    };
    let mode = search.costing.base().travel_mode();
    let first = u64::from(ni.edge_index());
    for (i, de) in tile.get_outbound_edges_from_node(ni).iter().enumerate() {
        let edge_id = node_id
            .with_feature_index(first + u64::try_from(i).unwrap_or(u64::MAX))
            .map_err(GraphTileProviderError::from)?;
        if de.is_shortcut() || de.road_use().is_ok_and(|u| u == RoadUse::TransitConnection) {
            continue;
        }
        let (allowed, restriction_idx) = is_edge_allowed(search.costing, de, edge_id, label, &tile);
        if !allowed {
            continue;
        }
        let mut turn_cost = label.turn_cost;
        if label.edge_id.is_some() {
            let outbound = outbound_heading(&tile, de, ni)?;
            let degree = turn_degree180(inbound, outbound)
                .ok_or(MeiliError::Logic("expect angles to be within [0, 360)"))?;
            turn_cost += search.turn_cost_table[degree];
        }
        let secs = edge_secs(search.costing, de, edge_id, &tile)?;
        let length = de.length();
        if let Some(found) = dests.by_edge.get(&edge_id) {
            for &dest in found {
                for candidate in &search.destinations[usize::from(dest)].edges {
                    if candidate.edge_id != edge_id {
                        continue;
                    }
                    let pa = candidate.percent_along;
                    // `float + uint32 * double` and `float + float * double`, stored in floats.
                    let cost = Cost {
                        cost: narrow(f64::from(label.cost.cost) + f64::from(length) * pa),
                        secs: narrow(f64::from(label.cost.secs) + f64::from(secs) * pa),
                    };
                    if within_limits(search, cost) {
                        let next = Label::new(
                            None,
                            dest,
                            edge_id,
                            0.0,
                            narrow(pa),
                            cost,
                            turn_cost,
                            cost.cost,
                            label_idx,
                            de,
                            mode,
                            restriction_idx,
                        )?;
                        set.put(labels, next)?;
                    }
                }
            }
        }
        let end_node = de.end_node_id();
        let end_tile = if de.leaves_tile() {
            graph.tile(end_node)?
        } else {
            Some(tile.clone())
        };
        if let Some(end_tile) = end_tile {
            #[expect(
                clippy::cast_precision_loss,
                reason = "mainline adds the uint32 to a float"
            )]
            let cost = Cost {
                cost: label.cost.cost + length as f32,
                secs: label.cost.secs + secs,
            };
            if within_limits(search, cost) {
                let end_ll = node_ll(&end_tile, node(&end_tile, end_node)?);
                let sortcost = cost.cost + heuristic(&search.approximator, search.radius, end_ll);
                let next = Label::new(
                    Some(end_node),
                    INVALID_DEST,
                    edge_id,
                    0.0,
                    1.0,
                    cost,
                    turn_cost,
                    sortcost,
                    label_idx,
                    de,
                    mode,
                    restriction_idx,
                )?;
                set.put(labels, next)?;
            }
        }
    }
    if !from_transition && ni.transition_count() > 0 {
        for transition in tile.get_transitions(ni) {
            let end = transition.corresponding_end_node_id();
            expand(search, set, dests, labels, end, label_idx, label, true)?;
        }
    }
    Ok(())
}

/// Expands from the origin when it is along an edge: to the destinations ahead on each of
/// its edges, and to each edge's end node.
fn expand_origin<P: GraphTileProvider>(
    search: &Search<'_, P>,
    set: &mut LabelSet,
    dests: &Destinations,
    labels: &mut Vec<Label>,
    label_idx: u32,
    label: &Label,
) -> Result<(), MeiliError> {
    let graph = search.graph;
    let mode = search.costing.base().travel_mode();
    let origin = search.destinations[0];
    let allows_immediate_uturn = matches!(
        search.origin_type,
        LocationType::KBreak | LocationType::KVia
    );
    for origin_edge in &origin.edges {
        let edge_id = origin_edge.edge_id;
        let Some(start_tile): Option<Tile<P>> = graph.tile(edge_id)? else {
            continue;
        };
        let de = edge(&start_tile, edge_id)?;
        let (allowed, restriction_idx) =
            is_edge_allowed(search.costing, de, edge_id, label, &start_tile);
        if !allowed {
            continue;
        }
        if !allows_immediate_uturn
            && label.edge_id.is_some()
            && label.edge_id != Some(edge_id)
            && label.opp_local_idx() == de.local_edge_index()
        {
            continue;
        }
        let turn_cost = label.turn_cost;
        let secs = edge_secs(search.costing, de, edge_id, &start_tile)?;
        #[expect(
            clippy::cast_precision_loss,
            reason = "mainline multiplies the uint32 by a float"
        )]
        let length = de.length() as f32;
        if let Some(found) = dests.by_edge.get(&edge_id) {
            for &other in found {
                for dest_edge in &search.destinations[usize::from(other)].edges {
                    if dest_edge.edge_id == edge_id
                        && origin_edge.percent_along <= dest_edge.percent_along
                    {
                        let segment = narrow(dest_edge.percent_along - origin_edge.percent_along);
                        let cost = Cost {
                            cost: label.cost.cost + length * segment,
                            secs: label.cost.secs + secs * segment,
                        };
                        if within_limits(search, cost) {
                            let next = Label::new(
                                None,
                                other,
                                edge_id,
                                narrow(origin_edge.percent_along),
                                narrow(dest_edge.percent_along),
                                cost,
                                turn_cost,
                                cost.cost,
                                label_idx,
                                de,
                                mode,
                                restriction_idx,
                            )?;
                            set.put(labels, next)?;
                        }
                    }
                }
            }
        }
        let f = narrow(1.0 - origin_edge.percent_along);
        let cost = Cost {
            cost: label.cost.cost + length * f,
            secs: label.cost.secs + secs * f,
        };
        if within_limits(search, cost) {
            let end_node = de.end_node_id();
            let Some(end_tile) = graph.tile(end_node)? else {
                continue;
            };
            let end_ll = node_ll(&end_tile, node(&end_tile, end_node)?);
            let sortcost = cost.cost + heuristic(&search.approximator, search.radius, end_ll);
            let next = Label::new(
                Some(end_node),
                INVALID_DEST,
                edge_id,
                narrow(origin_edge.percent_along),
                1.0,
                cost,
                turn_cost,
                sortcost,
                label_idx,
                de,
                mode,
                restriction_idx,
            )?;
            set.put(labels, next)?;
        }
    }
    Ok(())
}
