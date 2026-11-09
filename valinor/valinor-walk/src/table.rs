//! Access and egress stop tables: full edge-based Sif, once per query.
//!
//! These tables are time-independent and shared by every forward and backward search
//! of a query, so unlike transfers they need not mirror anything exactly.
//! They use real edge-based costing (turn restrictions, transition costs,
//! complex restrictions), run forward from an origin (access) or backward from a
//! destination (egress), and record for each reachable stop how long the walk takes
//! and how to rebuild it.
//!
//! Seeds carry an initial cost and may come from several candidate edges of a location.
//! A future park-and-ride layer would seed this search from the parking edges reached by an
//! earlier layer's table, each with [`TableSeed::initial`] set to the time already spent.

use core::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};
use valinor_sif::{
    Cost, Costing, CostingError, EdgeAccess, EdgeLabel, InternalTurn, Label, LabelState,
    TimeContext, TravelMode,
};
use valinor_transit::{Seconds, StopId, StopTable};

use crate::error::WalkError;
use crate::graph::LazyGraph;
use crate::snap::StopSnaps;

/// Which table a [`StopTables`] search fills, and thus which way it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessSide {
    /// Forward from the origin to each stop.
    Access,
    /// Backward from the destination, to each stop from which it can be reached.
    Egress,
}

/// A candidate edge to start from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableSeed {
    /// The directed edge of travel at the location.
    pub edge: GraphId,
    /// The fraction along it where the location is, in the edge's direction.
    pub percent_along: f32,
    /// What the rider has already spent, in seconds (zero, unless a previous layer led here).
    pub initial: f32,
}

/// Limits for one table.
pub struct TableRequest<'a> {
    /// Access or egress.
    pub side: AccessSide,
    /// The candidate edges of the location.
    pub seeds: &'a [TableSeed],
    /// The longest walk to consider, in seconds; stops beyond it are left out of the table.
    pub max_seconds: u32,
    /// Asks the search to stop.
    pub interrupt: &'a dyn Fn() -> bool,
}

/// What the table search did, for profiling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableStats {
    /// Edge labels settled.
    pub labels_settled: u64,
    /// Stops reached.
    pub stops_reached: u64,
    /// Every byte retained by this search's arrays, from [`StopTables::memory_parts`].
    pub bytes: usize,
    /// Whether the search stopped because every attached stop's entry was final.
    pub terminated_early: bool,
}

/// One edge of a leg, in real travel order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccessEdge {
    /// The directed edge traveled.
    pub edge: GraphId,
    /// The cost of the transition onto this edge (zero at the start).
    pub transition: Cost,
    /// The cost of traversing it, charging only the part walked of a partial first or last edge.
    pub traversal: Cost,
}

/// A walk from an origin to a stop, or a stop to a destination, in real travel order.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessLeg {
    /// Whether this is an access or an egress leg.
    pub side: AccessSide,
    /// The stop at the end (access) or the beginning (egress) of the leg.
    pub stop: StopId,
    /// The edges, in real travel order.
    pub edges: Vec<AccessEdge>,
    /// The fraction along the first edge where the leg starts.
    pub begin_percent: f32,
    /// The fraction along the last edge where the leg ends.
    pub end_percent: f32,
    /// The leg's total seconds.
    pub secs: f32,
}

const NONE: u32 = u32::MAX;

/// How often to check for an interrupt, in labels settled.
const INTERRUPT_INTERVAL: u32 = 2048;

#[derive(Clone, Copy, Debug)]
struct TableLabel {
    inner: EdgeLabel,
    /// Cumulative, in search order, including this label's edge.
    cost: Cost,
    transition: Cost,
    /// The edge of travel (the opposing edge of `inner`'s for an egress search).
    real_edge: GraphId,
    /// Dense number of `inner`'s edge, for status lookups.
    dense_edge: u32,
    /// Where `inner`'s edge ends: the node the search expands from.
    end_node: GraphId,
    /// The part of the real edge this label charges: 0 to 1 unless a seed starts mid-edge.
    span: (f32, f32),
    settled: bool,
    /// Destination-only exemptions carried along the path.
    destonly_mask: u8,
}

macro_rules! delegate_label {
    ($($method:ident -> $ret:ty),* $(,)?) => {
        $(#[inline] fn $method(&self) -> $ret { self.inner.$method() })*
    };
}

impl Label for TableLabel {
    delegate_label!(
        edge_id -> GraphId,
        predecessor -> Option<u32>,
        opp_local_idx -> u8,
        restrictions -> u8,
        road_use -> valhalla_graphtile::RoadUse,
        classification -> valhalla_graphtile::RoadClass,
        toll -> bool,
        bridge -> bool,
        tunnel -> bool,
        unpaved -> bool,
        deadend -> bool,
        destonly -> bool,
        mode -> TravelMode,
        path_distance -> u32,
        closure_pruning -> bool,
        has_measured_speed -> bool,
        internal_turn -> InternalTurn,
    );
}

/// Where a stop is attached to a directed edge.
#[derive(Clone, Copy, Debug)]
struct EdgeStop {
    stop: u32,
    percent: f32,
    next: u32,
    /// Which of the stop's two directed edges this is, as one bit of a visit mask.
    side: u8,
}

/// The bit of a stop's visit mask for the directed edge it was snapped along.
const SIDE_ALONG: u8 = 1;
/// The bit of a stop's visit mask for the edge's opposing edge.
const SIDE_OPPOSING: u8 = 2;
/// A visit mask with both of a stop's directed edges settled.
const SIDE_BOTH: u8 = SIDE_ALONG | SIDE_OPPOSING;

/// No label for this directed edge in the current search.
const UNSET: u32 = u32::MAX;

/// Set in a directed edge's state once its label has been settled.
const SETTLED: u32 = 1 << 31;

/// How many directed edges one page of [`EdgeStates`] covers, as a power of two.
const EDGE_PAGE_BITS: u32 = 8;
const EDGE_PAGE: usize = 1 << EDGE_PAGE_BITS;

/// A label index and [`SETTLED`] flag per directed edge, paged so only touched regions cost memory.
///
/// A search only touches edges near its seeds, while the dense numbers run over whole tiles.
#[derive(Debug, Default)]
struct EdgeStates {
    /// By page of dense edge numbers: its place in `storage`, or [`NO_PAGE`].
    page_of: Vec<u32>,
    storage: Vec<u32>,
    /// The pages of `page_of` in use by this search, to clear them cheaply.
    touched: Vec<u32>,
}

const NO_PAGE: u32 = u32::MAX;

impl EdgeStates {
    /// Forgets the current search's states, and covers `edges` dense edge numbers.
    fn reset(&mut self, edges: usize) {
        for &page in &self.touched {
            self.page_of[page as usize] = NO_PAGE;
        }
        self.touched.clear();
        self.grow(edges);
    }

    fn grow(&mut self, edges: usize) {
        let pages = edges.div_ceil(EDGE_PAGE);
        if self.page_of.len() < pages {
            self.page_of.resize(pages, NO_PAGE);
        }
    }

    /// Bytes retained.
    fn bytes(&self) -> usize {
        self.page_of.capacity() * size_of::<u32>()
            + self.storage.capacity() * size_of::<u32>()
            + self.touched.capacity() * size_of::<u32>()
    }

    /// Gives back storage beyond what this search used, unless that is small.
    fn trim(&mut self) {
        // `trim` can't shrink below the length, and the storage keeps its length between searches.
        let keep = (self.touched.len() * EDGE_PAGE).max(RETAINED_EDGE_STATES);
        self.storage.truncate(keep);
        trim(&mut self.storage, keep, RETAINED_EDGE_STATES);
    }

    #[inline]
    fn get(&self, edge: u32) -> u32 {
        match self.page_of.get((edge >> EDGE_PAGE_BITS) as usize) {
            Some(&page) if page != NO_PAGE => {
                self.storage[(page as usize) * EDGE_PAGE + (edge as usize & (EDGE_PAGE - 1))]
            }
            _ => UNSET,
        }
    }

    fn set(&mut self, edge: u32, state: u32) {
        let index = (edge >> EDGE_PAGE_BITS) as usize;
        let mut page = self.page_of[index];
        if page == NO_PAGE {
            page = u32::try_from(self.touched.len()).unwrap_or(NO_PAGE);
            let start = self.touched.len() * EDGE_PAGE;
            if self.storage.len() < start + EDGE_PAGE {
                self.storage.resize(start + EDGE_PAGE, UNSET);
            } else {
                self.storage[start..start + EDGE_PAGE].fill(UNSET);
            }
            self.touched.push(u32::try_from(index).unwrap_or(NO_PAGE));
            self.page_of[index] = page;
        }
        let slot = (page as usize) * EDGE_PAGE + (edge as usize & (EDGE_PAGE - 1));
        self.storage[slot] = state;
    }

    /// Makes an edge as if it had no label in this search.
    fn forget(&mut self, edge: u32) {
        if self.get(edge) != UNSET {
            self.set(edge, UNSET);
        }
    }
}

/// The first of a directed edge's attached stops, sized by the edges with stops.
///
/// Keyed by the edge's raw graph ID, so one index can serve every worker of a large build
/// without tying them to one worker's dense numbering.
#[derive(Debug, Default)]
struct AttachedEdges {
    /// `(raw graph ID, first attachment)`, open addressed; a power of two long, or empty.
    slots: Vec<(u64, u32)>,
    len: usize,
}

impl AttachedEdges {
    /// No graph ID is all ones: the ID fits in 46 bits.
    const EMPTY: (u64, u32) = (u64::MAX, NONE);

    fn bytes(&self) -> usize {
        self.slots.capacity() * size_of::<(u64, u32)>()
    }

    #[inline]
    fn start(&self, edge: u64) -> usize {
        // Fibonacci hashing: graph IDs are clustered, so spread them over the table.
        let hashed = (edge.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize;
        hashed & (self.slots.len() - 1)
    }

    /// The first attachment of `edge`, or [`NONE`].
    #[inline]
    fn get(&self, edge: u64) -> u32 {
        if self.slots.is_empty() {
            return NONE;
        }
        let mask = self.slots.len() - 1;
        let mut at = self.start(edge);
        loop {
            let (key, value) = self.slots[at];
            if key == edge {
                return value;
            }
            if key == Self::EMPTY.0 {
                return NONE;
            }
            at = (at + 1) & mask;
        }
    }

    fn insert(&mut self, edge: u64, value: u32) {
        if (self.len + 1) * 2 > self.slots.len() {
            let old = core::mem::take(&mut self.slots);
            self.slots = vec![Self::EMPTY; (old.len() * 2).max(64)];
            for (key, stored) in old {
                if key != Self::EMPTY.0 {
                    self.place(key, stored);
                }
            }
        }
        if self.place(edge, value) {
            self.len += 1;
        }
    }

    /// Stores a value, returning whether the key is new.
    fn place(&mut self, edge: u64, value: u32) -> bool {
        let mask = self.slots.len() - 1;
        let mut at = self.start(edge);
        loop {
            let key = self.slots[at].0;
            if key == edge || key == Self::EMPTY.0 {
                self.slots[at] = (edge, value);
                return key == Self::EMPTY.0;
            }
            at = (at + 1) & mask;
        }
    }
}

/// Every bound stop's place on the street network, built once and shared by a build's workers.
///
/// A table search reaches stops through this index, which depends only on the snaps and the tiles.
/// [`StopTables::bind`] builds one for its own use; a build that binds many workers to the same
/// stops builds it once with [`StopAttachments::new`] and gives each [`StopTables::bind_shared`],
/// so a planet-sized stop list is held once rather than once per thread.
#[derive(Debug)]
pub struct StopAttachments {
    snaps: Arc<StopSnaps>,
    edges: AttachedEdges,
    edge_stops: Vec<EdgeStop>,
    attached_stops: usize,
}

impl StopAttachments {
    /// Attaches every snapped stop in `snaps` to the street network.
    ///
    /// # Errors
    ///
    /// Refuses snaps from a different tile build, or tiles it can't read.
    pub fn new<P: GraphTileProvider, C: Costing>(
        provider: P,
        costing: C,
        snaps: Arc<StopSnaps>,
    ) -> Result<Self, WalkError> {
        let mut graph = LazyGraph::numbering_only(provider, costing);
        Self::build(&mut graph, snaps)
    }

    fn build<P: GraphTileProvider, C: Costing>(
        graph: &mut LazyGraph<P, C>,
        snaps: Arc<StopSnaps>,
    ) -> Result<Self, WalkError> {
        let mut attachments = Self {
            snaps,
            edges: AttachedEdges::default(),
            edge_stops: Vec::new(),
            attached_stops: 0,
        };
        let snaps = Arc::clone(&attachments.snaps);
        for stop in 0..snaps.stop_count() {
            // Keys are graph IDs, so dense numbers are only needed for one stop at a time;
            // forgetting them keeps a planet of stops under the dense numbering's limit.
            if stop.is_multiple_of(4096) {
                graph.forget_tiles();
            }
            let stop_u = u32::try_from(stop).map_err(|_| WalkError::TooLarge)?;
            let Some(point) = snaps.point(StopId::new(stop_u)) else {
                continue;
            };
            // A snap's tile can be absent: an offline area holds only part of the network,
            // and a stop outside it is unreachable, exactly as if it had no snap.
            let Some(build) = graph.build_id_of(point.edge)? else {
                continue;
            };
            if build != snaps.build_id() {
                return Err(WalkError::BuildMismatch {
                    snaps: snaps.build_id(),
                    tiles: build,
                });
            }
            let Some(description) = graph.describe_edge(point.edge)? else {
                continue;
            };
            attachments.attach(point.edge.value(), stop_u, point.percent_along, SIDE_ALONG);
            attachments.attach(
                description.opposing_id.value(),
                stop_u,
                1.0 - point.percent_along,
                SIDE_OPPOSING,
            );
            attachments.attached_stops += 1;
        }
        Ok(attachments)
    }

    fn attach(&mut self, edge: u64, stop: u32, percent: f32, side: u8) {
        let next = self.edges.get(edge);
        let index = u32::try_from(self.edge_stops.len()).unwrap_or(NONE);
        self.edge_stops.push(EdgeStop {
            stop,
            percent,
            next,
            side,
        });
        self.edges.insert(edge, index);
    }

    /// The snaps these attachments were built from.
    pub fn snaps(&self) -> &Arc<StopSnaps> {
        &self.snaps
    }

    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.edges.bytes() + self.edge_stops.capacity() * size_of::<EdgeStop>()
    }
}

/// How many labels a search keeps room for between queries (72 bytes each, about 5 MB).
const RETAINED_LABELS: usize = 1 << 16;
/// How many edge states a search keeps room for between queries (4 bytes each).
const RETAINED_EDGE_STATES: usize = 1 << 20;
/// How many queue entries a search keeps room for between queries (8 bytes each).
const RETAINED_QUEUE: usize = 1 << 14;

/// Gives back a buffer's room beyond `used`, unless it is within `floor`.
fn trim<T>(buffer: &mut Vec<T>, used: usize, floor: usize) {
    let keep = used.max(floor);
    if buffer.capacity() > keep {
        buffer.shrink_to(keep);
    }
}

#[derive(Clone, Copy, Debug)]
struct StopBest {
    stamp: u32,
    key: f32,
    secs: f32,
    label: u32,
    percent: f32,
}

/// A queue key: the sort cost, with `total_cmp` order.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Key(f32);

impl Eq for Key {}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// A walk limit in the units costs are in.
#[inline]
fn limit_secs(max_seconds: u32) -> f32 {
    #[expect(clippy::cast_precision_loss, reason = "a walk limit, far below 2^24")]
    let limit = max_seconds as f32;
    limit
}

/// Records one of a stop's directed edges settling this search, returning whether that was the
/// second of its two.
#[inline]
fn note_visit(visits: &mut (u32, u8), epoch: u32, side: u8) -> bool {
    if visits.0 != epoch {
        *visits = (epoch, 0);
    }
    let before = visits.1;
    visits.1 |= side;
    before != SIDE_BOTH && visits.1 == SIDE_BOTH
}

/// `a - b`, in each component of a cost.
#[inline]
fn sub_cost(a: Cost, b: Cost) -> Cost {
    Cost {
        cost: a.cost - b.cost,
        secs: a.secs - b.secs,
    }
}

/// Edge-based access and egress searches for one worker.
///
/// Keeps its storage between queries. The minimized objective is Sif's `cost`,
/// and tables report that path's `secs` (as Thor does for a walking route).
///
/// # Performance
///
/// One per worker, as with [`WalkSearch`](crate::WalkSearch).
/// A query runs this once for access and once for egress.
pub struct StopTables<P: GraphTileProvider, C: Costing> {
    graph: LazyGraph<P, C>,
    /// Where the bound stops are on the network, and the snaps they came from.
    attachments: Option<Arc<StopAttachments>>,

    labels: Vec<TableLabel>,
    edge_states: EdgeStates,
    epoch: u32,
    queue: BinaryHeap<Reverse<(Key, u32)>>,
    stop_best: Vec<StopBest>,
    /// `(epoch, number of the stop's two directed edges settled so far)`.
    stop_visits: Vec<(u32, u8)>,
    /// How many stops still have a directed edge left to settle in this search.
    stops_pending: usize,
    terminate_when_stops_settled: bool,
    touched_stops: Vec<u32>,
    side: AccessSide,
    stats: TableStats,
}

impl<P: GraphTileProvider, C: Costing> StopTables<P, C> {
    /// A table search over `provider`, costed by `costing`.
    pub fn new(provider: P, costing: C) -> Self {
        Self {
            graph: LazyGraph::numbering_only(provider, costing),
            attachments: None,
            labels: Vec::new(),
            edge_states: EdgeStates::default(),
            epoch: 0,
            queue: BinaryHeap::new(),
            stop_best: Vec::new(),
            stop_visits: Vec::new(),
            stops_pending: 0,
            terminate_when_stops_settled: true,
            touched_stops: Vec::new(),
            side: AccessSide::Access,
            stats: TableStats::default(),
        }
    }

    /// Sets the stops to reach.
    ///
    /// # Errors
    ///
    /// Refuses snaps from a different tile build, or tiles it can't read.
    pub fn bind(&mut self, snaps: Arc<StopSnaps>) -> Result<(), WalkError> {
        if self
            .attachments
            .as_ref()
            .is_some_and(|a| Arc::ptr_eq(&a.snaps, &snaps))
        {
            return Ok(());
        }
        // As in `WalkSearch::bind`: a failure below must not leave the old attachments in place.
        self.attachments = None;
        let attachments = StopAttachments::build(&mut self.graph, snaps)?;
        self.bind_shared(Arc::new(attachments));
        Ok(())
    }

    /// Sets the stops to reach to attachments built once for many workers.
    ///
    /// # Performance
    ///
    /// Sizes this worker's per-stop arrays for all the stops, and nothing else.
    pub fn bind_shared(&mut self, attachments: Arc<StopAttachments>) {
        if self
            .attachments
            .as_ref()
            .is_some_and(|a| Arc::ptr_eq(a, &attachments))
        {
            return;
        }
        let stops = attachments.snaps.stop_count();
        self.stop_best.clear();
        self.stop_best.resize(
            stops,
            StopBest {
                stamp: 0,
                key: f32::INFINITY,
                secs: 0.0,
                label: NONE,
                percent: 0.0,
            },
        );
        self.stop_visits.clear();
        self.stop_visits.resize(stops, (0, 0));
        self.attachments = Some(attachments);
    }

    /// Gives back the tile slots and dense numbers this search has built up.
    ///
    /// A long-running build calls this between batches of searches, so that a worker's memory
    /// stays tied to the area it is working on rather than growing with everywhere it has been.
    /// Labels and dense numbers don't survive it; [`Self::leg`] needs the search it read.
    pub fn forget_tiles(&mut self) {
        self.graph.forget_tiles();
    }

    /// Computes a table for `stop_count` stops within `max_seconds` of the request's seeds.
    ///
    /// # Performance
    ///
    /// Runs once per query per direction; reuses its storage between queries.
    /// Allocates a table of `stop_count` entries; a caller only reading the few stops a search
    /// reached should use [`Self::search`] and [`Self::reached_stops`] instead.
    ///
    /// # Errors
    ///
    /// Fails on unreadable tiles or a costing error; an interrupt is reported as `Ok(None)`.
    pub fn compute(
        &mut self,
        stop_count: usize,
        request: &TableRequest<'_>,
    ) -> Result<Option<StopTable>, WalkError> {
        if !self.search(request)? {
            return Ok(None);
        }
        let mut table = StopTable::new(stop_count);
        let mut reached = 0;
        for (stop, secs) in self.reached_stops() {
            if table.set(stop, secs).is_ok() {
                reached += 1;
            }
        }
        self.stats.stops_reached += reached;
        Ok(Some(table))
    }

    /// Runs one search, reporting what it reached only through [`Self::reached_stops`].
    ///
    /// Returns `Ok(false)` if `request.interrupt` asks to stop.
    ///
    /// # Performance
    ///
    /// Like [`Self::compute`], but allocates nothing proportional to the stop count.
    ///
    /// # Errors
    ///
    /// Fails on unreadable tiles or a costing error.
    pub fn search(&mut self, request: &TableRequest<'_>) -> Result<bool, WalkError> {
        self.side = request.side;
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stop_best.iter_mut().for_each(|s| s.stamp = 0);
            self.stop_visits.fill((0, 0));
            self.epoch = 1;
        }
        self.labels.clear();
        trim(&mut self.labels, 0, RETAINED_LABELS);
        self.queue.clear();
        if self.queue.capacity() > RETAINED_QUEUE {
            self.queue.shrink_to(RETAINED_QUEUE);
        }
        self.touched_stops.clear();
        self.stats = TableStats::default();
        self.stops_pending = self.attachments.as_ref().map_or(0, |a| a.attached_stops);
        self.edge_states.reset(self.graph.edge_count());
        self.edge_states.trim();

        for &seed in request.seeds {
            self.add_seed(seed, request)?;
        }
        let mut since_check = 0;
        while let Some(Reverse((_, index))) = self.queue.pop() {
            since_check += 1;
            if since_check >= INTERRUPT_INTERVAL {
                since_check = 0;
                if (request.interrupt)() {
                    return Ok(false);
                }
            }
            self.settle(index, request)?;
            if self.terminate_when_stops_settled && self.stops_pending == 0 {
                self.stats.terminated_early = true;
                break;
            }
        }
        // After an unusually large query, keep only what its legs still read.
        let used = self.labels.len();
        trim(&mut self.labels, used, RETAINED_LABELS);
        self.edge_states.trim();
        Ok(true)
    }

    /// What the last search reached, as `(stop, whole seconds)`, in the order it reached them.
    ///
    /// # Performance
    ///
    /// Allocates nothing and visits only the stops the search found.
    pub fn reached_stops(&self) -> impl Iterator<Item = (StopId, Seconds)> + '_ {
        self.touched_stops.iter().filter_map(|&stop| {
            let best = self.stop_best.get(stop as usize)?;
            let secs = best.secs.round();
            if secs < 0.0 || !secs.is_finite() {
                return None;
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a non-negative whole number of seconds, far below the limit"
            )]
            let secs = Seconds::try_new(secs as u32).unwrap_or(Seconds::ZERO);
            Some((StopId::new(stop), secs))
        })
    }

    fn ensure_edge_state(&mut self) {
        self.edge_states.grow(self.graph.edge_count());
    }

    /// The tile holding `id`; slots keep each tile's handle after its first use.
    fn tile_handle(&mut self, id: GraphId) -> Result<Option<P::TileHandle>, WalkError> {
        self.graph.tile(id)
    }

    /// The opposing edge of `edge_id` and its tile, or `None` at the edge of the tile set.
    fn opposite_of(
        &mut self,
        edge_id: GraphId,
        tile: &P::TileHandle,
    ) -> Result<Option<(GraphId, P::TileHandle, DirectedEdge)>, WalkError> {
        let edge = tile
            .get_directed_edge(edge_id)
            .map_err(|e| WalkError::Lookup(edge_id, e))?;
        let end = edge.end_node_id();
        let Some(end_tile) = self.tile_handle(end)? else {
            return Ok(None);
        };
        let end_node = end_tile
            .get_node(end)
            .map_err(|e| WalkError::Lookup(end, e))?;
        let opp_id = end
            .with_feature_index(
                u64::from(end_node.edge_index()) + u64::from(edge.opposing_edge_index()),
            )
            .map_err(GraphTileProviderError::from)?;
        let opposing = end_tile
            .get_directed_edge(opp_id)
            .map_err(|e| WalkError::Lookup(opp_id, e))?
            .clone();
        Ok(Some((opp_id, end_tile, opposing)))
    }

    fn add_seed(&mut self, seed: TableSeed, request: &TableRequest<'_>) -> Result<(), WalkError> {
        let Some(tile) = self.tile_handle(seed.edge)? else {
            return Ok(());
        };
        let edge = tile
            .get_directed_edge(seed.edge)
            .map_err(|e| WalkError::Lookup(seed.edge, e))?
            .clone();
        let Some((opp_id, _opp_tile, opp_edge)) = self.opposite_of(seed.edge, &tile)? else {
            return Ok(());
        };
        // An access seed charges `[percent, 1]` of its edge, going forward;
        // an egress seed charges `[0, percent]` of the edge it arrives along.
        let (begin, end) = match request.side {
            AccessSide::Access => (seed.percent_along, 1.0),
            AccessSide::Egress => (0.0, seed.percent_along),
        };
        let costing = &self.graph.costing;
        let mode = costing.base().travel_mode();
        let mut access = EdgeAccess::new(0);
        let mut neutral = EdgeLabel::new(
            seed.edge,
            &edge,
            LabelState {
                mode,
                ..LabelState::default()
            },
        );
        // No predecessor to turn from: without this, a seed edge whose own simple turn restrictions
        // happen to have the bit of its local index would be refused as if turning onto itself.
        neutral.set_deadend(true);
        neutral.clear_restrictions();
        if !costing.allowed(
            &edge,
            seed.edge,
            &tile,
            &neutral,
            TimeContext::NONE,
            &mut access,
        ) {
            return Ok(());
        }
        let partial = costing.partial_edge_cost(
            &edge,
            seed.edge,
            Some(seed.edge),
            &tile,
            None,
            begin,
            end,
        )?;
        let (label_edge, label_id) = match request.side {
            AccessSide::Access => (edge, seed.edge),
            AccessSide::Egress => (opp_edge, opp_id),
        };
        let inner = EdgeLabel::new(
            label_id,
            &label_edge,
            LabelState {
                mode,
                destonly: access.edge_destonly,
                ..LabelState::default()
            },
        );
        let Some(dense_edge) = self.graph.dense_edge(label_id)? else {
            return Ok(());
        };
        self.ensure_edge_state();
        if seed.initial > limit_secs(request.max_seconds) {
            return Ok(());
        }
        // A seed takes no edge status (as Thor's origin edges), so a route can come back
        // around onto its own edge to reach a stop behind the start on a one-way street.
        self.push_seed(TableLabel {
            inner,
            cost: partial.cost + Cost::new(seed.initial, seed.initial),
            transition: Cost::ZERO,
            real_edge: seed.edge,
            dense_edge,
            end_node: label_edge.end_node_id(),
            span: (begin, end),
            settled: false,
            destonly_mask: access.destonly_access_restr_mask,
        });
        Ok(())
    }

    /// Adds or improves the label of a directed edge in the queue.
    ///
    /// The caller has checked the limit at the start of the edge: a stop partway along an edge
    /// can be within it even when the far end of the edge isn't.
    fn push_label(&mut self, label: TableLabel) {
        let state = self.edge_states.get(label.dense_edge);
        if state != UNSET {
            if state & SETTLED != 0 {
                return;
            }
            let Some(existing) = self.labels.get_mut(state as usize) else {
                return;
            };
            if label.cost.cost < existing.cost.cost {
                *existing = label;
                self.queue.push(Reverse((Key(label.cost.cost), state)));
            }
            return;
        }
        let index = u32::try_from(self.labels.len()).unwrap_or(NONE);
        self.edge_states.set(label.dense_edge, index);
        self.labels.push(label);
        self.queue.push(Reverse((Key(label.cost.cost), index)));
    }

    /// Queues a seed's label, which has no edge status.
    fn push_seed(&mut self, label: TableLabel) {
        let index = u32::try_from(self.labels.len()).unwrap_or(NONE);
        self.labels.push(label);
        self.queue.push(Reverse((Key(label.cost.cost), index)));
    }

    /// Settles a label, reaches the stops on its edge, and expands from its far node.
    fn settle(&mut self, index: u32, request: &TableRequest<'_>) -> Result<(), WalkError> {
        let Some(label) = self.labels.get(index as usize).copied() else {
            return Ok(());
        };
        if label.settled {
            return Ok(());
        }
        if label.inner.predecessor().is_some() {
            let state = self.edge_states.get(label.dense_edge);
            if state == UNSET || state & !SETTLED != index {
                return Ok(());
            }
            self.edge_states.set(label.dense_edge, state | SETTLED);
        }
        if let Some(stored) = self.labels.get_mut(index as usize) {
            stored.settled = true;
        }
        self.stats.labels_settled += 1;
        self.reach_stops(index, &label, request.max_seconds)?;

        let forward = self.side == AccessSide::Access;
        let Some(node_tile) = self.tile_handle(label.end_node)? else {
            return Ok(());
        };
        let node = node_tile
            .get_node(label.end_node)
            .map_err(|e| WalkError::Lookup(label.end_node, e))?;
        if !self.graph.costing.node_allowed(node) {
            return Ok(());
        }
        let mut expanded = false;
        let mut uturn = None;
        let first = u64::from(node.edge_index());
        let tile_base = label.end_node.tile_base_id();
        for (i, edge) in node_tile
            .get_outbound_edges_from_node(node)
            .iter()
            .enumerate()
        {
            let edge_id = tile_base
                .with_feature_index(first + i as u64)
                .map_err(GraphTileProviderError::from)?;
            if label.inner.opp_local_idx() == edge.local_edge_index() {
                uturn = Some(edge_id);
                continue;
            }
            expanded |= self.expand(
                &label, index, node, edge_id, edge, &node_tile, forward, request,
            )?;
        }
        for transition in node_tile.get_transitions(node) {
            let end = transition.corresponding_end_node_id();
            let Some(trans_tile) = self.tile_handle(end)? else {
                continue;
            };
            let trans_node = trans_tile
                .get_node(end)
                .map_err(|e| WalkError::Lookup(end, e))?;
            let first = u64::from(trans_node.edge_index());
            for (i, edge) in trans_tile
                .get_outbound_edges_from_node(trans_node)
                .iter()
                .enumerate()
            {
                let edge_id = end
                    .tile_base_id()
                    .with_feature_index(first + i as u64)
                    .map_err(GraphTileProviderError::from)?;
                expanded |= self.expand(
                    &label,
                    index,
                    trans_node,
                    edge_id,
                    edge,
                    &trans_tile,
                    forward,
                    request,
                )?;
            }
        }
        if !expanded && let Some(uturn_id) = uturn {
            // A dead end: only a U-turn remains.
            let edge = node_tile
                .get_directed_edge(uturn_id)
                .map_err(|e| WalkError::Lookup(uturn_id, e))?;
            let mut deadend = label;
            deadend.inner.set_deadend(true);
            self.expand(
                &deadend, index, node, uturn_id, edge, &node_tile, forward, request,
            )?;
        }
        Ok(())
    }

    /// Considers `edge` from the end node of `pred` (Thor's `ExpandInner`, for one edge).
    #[expect(clippy::too_many_arguments, reason = "one edge's context, as in Thor")]
    #[expect(
        clippy::too_many_lines,
        reason = "the access, cost, and label steps of one edge"
    )]
    fn expand(
        &mut self,
        pred: &TableLabel,
        pred_index: u32,
        node: &NodeInfo,
        edge_id: GraphId,
        edge: &DirectedEdge,
        tile: &P::TileHandle,
        forward: bool,
        request: &TableRequest<'_>,
    ) -> Result<bool, WalkError> {
        if edge.is_shortcut() {
            return Ok(false);
        }
        let Some((opp_id, opp_tile, opp_edge)) = self.opposite_of(edge_id, tile)? else {
            return Ok(false);
        };
        let (traversed, traversed_id, traversed_tile): (&DirectedEdge, GraphId, &P::TileHandle) =
            if forward {
                (edge, edge_id, tile)
            } else {
                (&opp_edge, opp_id, &opp_tile)
            };
        if self.is_permanent(edge_id) {
            return Ok(true);
        }
        let costing = &self.graph.costing;
        let mut access = EdgeAccess::new(pred.destonly_mask);
        let allowed = if forward {
            costing.allowed(edge, edge_id, tile, pred, TimeContext::NONE, &mut access)
        } else {
            costing.allowed_reverse(
                edge,
                pred,
                &opp_edge,
                opp_id,
                &opp_tile,
                TimeContext::NONE,
                &mut access,
            )
        };
        if !allowed {
            return Ok(false);
        }
        let labels = self.labels.as_slice();
        let (edge_states, graph) = (&mut self.edge_states, &self.graph);
        let restricted = costing
            .base()
            .restricted(
                edge,
                pred,
                labels,
                tile,
                edge_id,
                forward,
                TimeContext::NONE,
                |id| {
                    if let Some(dense) = graph.known_dense_edge(id) {
                        edge_states.forget(dense);
                    }
                },
            )
            .map_err(|e| WalkError::Costing(CostingError::Section(e)))?;
        if restricted {
            return Ok(false);
        }
        let edge_cost = costing.edge_cost(traversed, traversed_id, traversed_tile, None)?;
        let transition = if forward {
            costing.transition_cost(
                edge,
                tile,
                node,
                pred,
                &self.graph.provider,
                access.edge_destonly,
            )?
        } else {
            let Some(pred_tile) = self.tile_handle(pred.real_edge)? else {
                return Ok(false);
            };
            let opp_pred_edge = pred_tile
                .get_directed_edge(pred.real_edge)
                .map_err(|e| WalkError::Lookup(pred.real_edge, e))?;
            self.graph.costing.transition_cost_reverse(
                edge.local_edge_index(),
                node,
                &opp_edge,
                opp_pred_edge,
                pred,
                &opp_tile,
                &self.graph.provider,
                !edge_cost.flow_sources.is_empty(),
                pred.internal_turn(),
                access.edge_destonly,
            )?
        };
        let at_start = pred.cost + transition;
        if at_start.secs > limit_secs(request.max_seconds) {
            return Ok(true);
        }
        let cost = at_start + edge_cost.cost;
        let inner = EdgeLabel::new(
            edge_id,
            edge,
            LabelState {
                predecessor: Some(pred_index),
                mode: self.graph.costing.base().travel_mode(),
                destonly: access.edge_destonly,
                has_measured_speed: !edge_cost.flow_sources.is_empty(),
                ..LabelState::default()
            },
        );
        let Some(dense_edge) = self.graph.dense_edge(edge_id)? else {
            return Ok(false);
        };
        self.ensure_edge_state();
        self.push_label(TableLabel {
            inner,
            cost,
            transition,
            real_edge: traversed_id,
            dense_edge,
            end_node: edge.end_node_id(),
            span: (0.0, 1.0),
            settled: false,
            destonly_mask: access.destonly_access_restr_mask,
        });
        Ok(true)
    }

    /// Notes that one of a stop's two directed edges has settled.
    ///
    /// Apart from a seed's partial labels (which don't count here), a stop's entry only changes
    /// when one of its two edges settles. A complex restriction can make an edge settle again,
    /// always no cheaper than the first time, so an edge counts once however often it settles.
    /// Once both have settled, the entry is final, and when every stop's is, the search can stop.
    fn visit_stop(&mut self, stop: u32, side: u8) {
        let Some(visits) = self.stop_visits.get_mut(stop as usize) else {
            return;
        };
        if note_visit(visits, self.epoch, side) {
            self.stops_pending = self.stops_pending.saturating_sub(1);
        }
    }

    /// Whether a directed edge is already settled in this search.
    fn is_permanent(&self, id: GraphId) -> bool {
        self.graph.known_dense_edge(id).is_some_and(|dense| {
            let state = self.edge_states.get(dense);
            state != UNSET && state & SETTLED != 0
        })
    }

    /// Offers the stops attached to a settled label's real edge.
    fn reach_stops(
        &mut self,
        index: u32,
        label: &TableLabel,
        max_seconds: u32,
    ) -> Result<(), WalkError> {
        let Some(attachments) = self.attachments.clone() else {
            return Ok(());
        };
        let mut at = attachments.edges.get(label.real_edge.value());
        if at == NONE {
            return Ok(());
        }
        let Some(tile) = self.tile_handle(label.real_edge)? else {
            return Ok(());
        };
        let edge = tile
            .get_directed_edge(label.real_edge)
            .map_err(|e| WalkError::Lookup(label.real_edge, e))?;
        let forward = self.side == AccessSide::Access;
        let (begin, end) = label.span;
        // The label's cost includes all of `span`, so swap that for the part up to the stop.
        let costing = &self.graph.costing;
        let whole = if begin <= 0.0 && end >= 1.0 {
            costing.edge_cost(edge, label.real_edge, &tile, None)?.cost
        } else {
            costing
                .partial_edge_cost(
                    edge,
                    label.real_edge,
                    Some(label.real_edge),
                    &tile,
                    None,
                    begin,
                    end,
                )?
                .cost
        };
        while at != NONE {
            let Some(entry) = attachments.edge_stops.get(at as usize).copied() else {
                break;
            };
            at = entry.next;
            // A seed takes no edge status, so a later label can still come back around onto
            // the seed's edge and improve a stop on it: only a whole edge's one label counts.
            if label.inner.predecessor().is_some() {
                self.visit_stop(entry.stop, entry.side);
            }
            let inside = if forward {
                begin <= entry.percent
            } else {
                entry.percent <= end
            };
            if !inside {
                continue;
            }
            let (part_begin, part_end) = if forward {
                (begin, entry.percent)
            } else {
                (entry.percent, end)
            };
            let part = self
                .graph
                .costing
                .partial_edge_cost(
                    edge,
                    label.real_edge,
                    Some(label.real_edge),
                    &tile,
                    None,
                    part_begin,
                    part_end.max(part_begin),
                )?
                .cost;
            let cost = sub_cost(label.cost, whole) + part;
            if cost.secs > limit_secs(max_seconds) {
                continue;
            }
            let best = &mut self.stop_best[entry.stop as usize];
            if best.stamp != self.epoch || cost.cost < best.key {
                if best.stamp != self.epoch {
                    self.touched_stops.push(entry.stop);
                }
                *best = StopBest {
                    stamp: self.epoch,
                    key: cost.cost,
                    secs: cost.secs,
                    label: index,
                    percent: entry.percent,
                };
            }
        }
        Ok(())
    }

    /// Whether a search ends as soon as every attached stop has its final entry (the default).
    ///
    /// Turning this off floods the whole network, for measurement.
    pub fn set_terminate_when_stops_settled(&mut self, enabled: bool) {
        self.terminate_when_stops_settled = enabled;
    }

    /// What the last search did, and everything this search retains right now.
    pub fn stats(&self) -> TableStats {
        TableStats {
            bytes: self.memory_parts().iter().map(|(_, bytes)| bytes).sum(),
            ..self.stats
        }
    }

    /// How many tiles this search's graph has given dense numbers to.
    pub fn graph_tile_count(&self) -> usize {
        self.graph.stats().tile_slots
    }

    /// The candidate edges of a walk that starts or ends at a bound stop: both ways along its edge.
    ///
    /// # Errors
    ///
    /// Fails on unreadable tiles.
    pub fn stop_seeds(&mut self, stop: StopId) -> Result<Vec<TableSeed>, WalkError> {
        let Some(point) = self.attachments.as_ref().and_then(|a| a.snaps.point(stop)) else {
            return Ok(Vec::new());
        };
        let mut seeds = vec![TableSeed {
            edge: point.edge,
            percent_along: point.percent_along,
            initial: 0.0,
        }];
        let Some(tile) = self.tile_handle(point.edge)? else {
            return Ok(seeds);
        };
        if let Some((opposing, _, _)) = self.opposite_of(point.edge, &tile)? {
            seeds.push(TableSeed {
                edge: opposing,
                percent_along: 1.0 - point.percent_along,
                initial: 0.0,
            });
        }
        Ok(seeds)
    }

    /// Bytes retained by each part of this search, for finding what to shrink.
    pub fn memory_parts(&self) -> Vec<(&'static str, usize)> {
        vec![
            ("labels", self.labels.capacity() * size_of::<TableLabel>()),
            ("edge_state", self.edge_states.bytes()),
            (
                "stop attachments",
                self.attachments.as_ref().map_or(0, |a| a.bytes()),
            ),
            (
                "stop_best",
                self.stop_best.capacity() * size_of::<StopBest>(),
            ),
            (
                "stop_visits",
                self.stop_visits.capacity() * size_of::<(u32, u8)>(),
            ),
            (
                "queue",
                self.queue.capacity() * size_of::<Reverse<(Key, u32)>>(),
            ),
            (
                "touched_stops",
                self.touched_stops.capacity() * size_of::<u32>(),
            ),
            ("graph memo", self.graph.stats().bytes),
        ]
    }

    /// Follows a stop's best label to its leg, in real travel order.
    ///
    /// Returns `None` if the stop wasn't reached by the last [`compute`](Self::compute).
    pub fn leg(&self, stop: StopId) -> Option<AccessLeg> {
        let best = self.stop_best.get(stop.into_inner() as usize)?;
        if best.stamp != self.epoch {
            return None;
        }
        // Search order: from the seed to the stop's edge.
        let mut chain = Vec::new();
        let mut at = Some(best.label);
        while let Some(index) = at {
            let label = self.labels.get(index as usize)?;
            chain.push(*label);
            at = label.inner.predecessor();
        }
        chain.reverse();
        let forward = self.side == AccessSide::Access;

        let mut edges = Vec::with_capacity(chain.len());
        let mut previous = Cost::ZERO;
        for (i, label) in chain.iter().enumerate() {
            // The last label charged its whole edge, but only part of it is walked.
            let cumulative = if i + 1 == chain.len() {
                Cost::new(best.key, best.secs)
            } else {
                label.cost
            };
            edges.push(AccessEdge {
                edge: label.real_edge,
                transition: label.transition,
                traversal: sub_cost(sub_cost(cumulative, previous), label.transition),
            });
            previous = cumulative;
        }
        let seed_span = chain.first()?.span;
        let (begin_percent, end_percent) = if forward {
            (seed_span.0, best.percent)
        } else {
            edges.reverse();
            // A label holds the transition onto its edge in search order, which for egress is
            // the turn after that edge in real order: move each to the edge the rider enters.
            let mut entered = Cost::ZERO;
            for edge in &mut edges {
                entered = core::mem::replace(&mut edge.transition, entered);
            }
            (best.percent, seed_span.1)
        };
        Some(AccessLeg {
            side: self.side,
            stop,
            edges,
            begin_percent,
            end_percent,
            secs: best.secs,
        })
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, reason = "tests, with small sizes")]
mod sparse_tests {
    use std::collections::HashMap;

    use super::*;

    /// A small deterministic stream of numbers.
    fn numbers(seed: u64) -> impl Iterator<Item = u64> {
        let mut state = seed;
        core::iter::repeat_with(move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            state >> 33
        })
    }

    #[test]
    fn edge_states_behave_like_a_map_and_forget_each_search() {
        let mut states = EdgeStates::default();
        let mut expected = HashMap::new();
        states.reset(1 << 20);
        for (i, n) in numbers(7).take(20_000).enumerate() {
            // Clustered edges, as a real search touches them.
            let edge = u32::try_from(n % (1 << 20)).expect("small") & !0x3f | (i as u32 & 0x3f);
            if n % 3 == 0 {
                states.forget(edge);
                expected.remove(&edge);
            } else {
                let state = i as u32 | if n % 2 == 0 { SETTLED } else { 0 };
                states.set(edge, state);
                expected.insert(edge, state);
            }
            assert_eq!(
                states.get(edge),
                expected.get(&edge).copied().unwrap_or(UNSET)
            );
        }
        for (&edge, &state) in &expected {
            assert_eq!(states.get(edge), state);
        }
        states.reset(1 << 20);
        for &edge in expected.keys() {
            assert_eq!(states.get(edge), UNSET, "a new search starts empty");
        }
    }

    /// An edge settled twice, as a complex restriction allows, is one edge.
    #[test]
    fn a_resettled_edge_counts_once_towards_a_stop() {
        let mut visits = (0, 0);
        assert!(!note_visit(&mut visits, 7, SIDE_ALONG));
        assert!(
            !note_visit(&mut visits, 7, SIDE_ALONG),
            "the same edge again is not the stop's second"
        );
        assert!(note_visit(&mut visits, 7, SIDE_OPPOSING));
        assert!(
            !note_visit(&mut visits, 7, SIDE_ALONG),
            "complete only once"
        );
        assert!(
            !note_visit(&mut visits, 8, SIDE_OPPOSING),
            "a new epoch starts over"
        );
    }

    #[test]
    fn edge_states_read_unset_past_their_range() {
        let mut states = EdgeStates::default();
        states.reset(100);
        assert_eq!(states.get(1 << 30), UNSET);
        states.forget(1 << 30);
    }

    #[test]
    fn attached_edges_keep_every_key_as_they_grow() {
        let mut attached = AttachedEdges::default();
        assert_eq!(attached.get(5), NONE, "an empty table has no key");
        let mut expected = HashMap::new();
        for (i, n) in numbers(11).take(5_000).enumerate() {
            let edge = (n % 20_000) * (1 << 25);
            attached.insert(edge, i as u32);
            expected.insert(edge, i as u32);
            assert_eq!(attached.get(edge), i as u32);
        }
        for (&edge, &value) in &expected {
            assert_eq!(attached.get(edge), value);
        }
    }

    #[test]
    fn trim_keeps_what_was_used_and_the_floor() {
        let mut buffer: Vec<u32> = Vec::with_capacity(1000);
        buffer.resize(100, 0);
        trim(&mut buffer, 100, 200);
        assert!(buffer.capacity() >= 200 && buffer.capacity() < 1000);
        assert_eq!(buffer.len(), 100);
    }
}

#[cfg(test)]
mod trim_tests {
    use super::*;

    /// After a large search the next, smaller one gives the storage back.
    #[test]
    fn trim_returns_edge_state_storage() {
        let mut states = EdgeStates::default();
        states.reset(1 << 28);
        // A large query touches many pages.
        for page in 0..200_000_u32 {
            states.set(page << EDGE_PAGE_BITS, 7);
        }
        let grown = states.storage.capacity();
        // The next, small, query.
        states.reset(1 << 28);
        states.trim();
        states.set(0, 1);
        states.trim();
        let after = states.storage.capacity();
        assert!(
            after <= 2 * RETAINED_EDGE_STATES,
            "kept {after} slots after trimming; it grew to {grown}",
        );
        assert_eq!(states.get(0), 1, "what is in use survives");
    }
}
