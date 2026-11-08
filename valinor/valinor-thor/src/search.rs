//! What the path searches share: their inputs, candidate edges, and small helpers.

use jiff::Timestamp;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::{LatLng, Location, PathEdge as CandidateEdge};
use valinor_sif::{Costing, EdgeCostResult, TimeContext, TimeInfo};

use crate::error::ThorError;
use crate::geo::PointLL;
use crate::time::graph_id;

/// Labels to reserve in each search tree at first (mainline reserves
/// `max_reserved_labels_count_*`, a million or more; the storage is kept between searches).
pub(crate) const INITIAL_LABELS: usize = 1 << 16;

/// The most labels a search keeps room for between requests, as mainline does
/// (`max_reserved_labels_count_bidir_astar` and `max_reserved_labels_count_astar`
/// default to a million and two million); a longer search's storage is given back.
pub(crate) const MAX_RESERVED_LABELS_BIDIRECTIONAL: usize = 1_000_000;
pub(crate) const MAX_RESERVED_LABELS_UNIDIRECTIONAL: usize = 2_000_000;

/// Empties a label store, giving back room beyond `max` (mainline's `Clear`).
pub(crate) fn clear_labels<T>(labels: &mut Vec<T>, max: usize) {
    labels.clear();
    if labels.capacity() > max {
        labels.shrink_to(max);
    }
}

/// The number of buckets in the queues (`kBucketCount`).
pub(crate) const BUCKET_COUNT: u32 = 20_000;

/// The queue bucket size (`DynamicCost::UnitSize`, 1 for every costing Thor has).
pub(crate) const UNIT_SIZE: u32 = 1;

/// What a search reads besides the locations.
pub(crate) struct SearchContext<'a, P, C> {
    pub(crate) provider: &'a P,
    pub(crate) costing: &'a C,
    /// The current time, for `current` date and times.
    pub(crate) now: Timestamp,
}

impl<P: GraphTileProvider, C: Costing> SearchContext<'_, P, C> {
    /// The cost of a whole edge at a time.
    #[inline]
    pub(crate) fn edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        time: &TimeInfo,
    ) -> Result<EdgeCostResult, ThorError> {
        Ok(self
            .costing
            .edge_cost(edge, edge_id, tile, time.seconds_of_week())?)
    }

    /// The cost of part of an edge at a time, with the user's cost factors
    /// (`PartialEdgeCost` with the edge's ID).
    #[inline]
    pub(crate) fn partial_edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        time: &TimeInfo,
        start: f32,
        end: f32,
    ) -> Result<EdgeCostResult, ThorError> {
        Ok(self.costing.partial_edge_cost(
            edge,
            edge_id,
            Some(edge_id),
            tile,
            time.seconds_of_week(),
            start,
            end,
        )?)
    }
}

/// A point from a protobuf `LatLng` (missing coordinates are 0, as in C++).
#[inline]
pub(crate) fn point_of(ll: Option<&LatLng>) -> PointLL {
    let lat = match ll.and_then(|l| l.has_lat.as_ref()) {
        Some(HasLat::Lat(lat)) => *lat,
        None => 0.0,
    };
    let lng = match ll.and_then(|l| l.has_lng.as_ref()) {
        Some(HasLng::Lng(lng)) => *lng,
        None => 0.0,
    };
    PointLL::new(lng, lat)
}

/// A location's candidate edges (empty without a correlation).
#[inline]
pub(crate) fn candidates(location: &Location) -> &[CandidateEdge] {
    location
        .correlation
        .as_ref()
        .map_or(&[], |c| c.edges.as_slice())
}

/// The correlated point of a location's first candidate edge
/// (`correlation().edges(0).ll()`), which aims the searches.
pub(crate) fn first_candidate_point(
    location: &Location,
    index: usize,
) -> Result<PointLL, ThorError> {
    candidates(location)
        .first()
        .map(|e| point_of(e.ll.as_ref()))
        .ok_or(ThorError::NoCandidates(index))
}

/// The percent along of a path's edge among a location's candidates
/// (`find_percent_along`, narrowed to a float).
pub(crate) fn find_percent_along(location: &Location, edge_id: GraphId) -> Result<f32, ThorError> {
    for e in candidates(location) {
        if e.graph_id == edge_id.value() {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "mainline returns the double as a float"
            )]
            return Ok(e.percent_along as f32);
        }
    }
    Err(ThorError::MissingCandidate(edge_id))
}

/// The candidate's graph ID.
#[inline]
pub(crate) fn candidate_id(edge: &CandidateEdge) -> Result<GraphId, ThorError> {
    graph_id(edge.graph_id)
}

/// `cost.cost += distance`: a float plus a double, rounded back to a float.
#[inline]
pub(crate) fn add_double(cost: f32, distance: f64) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "C++ compound assignment of a double to a float"
    )]
    let sum = (f64::from(cost) + distance) as f32;
    sum
}

/// The time to check access at: the time info's time (0 without one) in its zone,
/// as bidirectional searches pass it.
#[inline]
pub(crate) fn time_context(time: &TimeInfo) -> TimeContext {
    TimeContext::new(time.current_time(), time.timezone_index)
}
