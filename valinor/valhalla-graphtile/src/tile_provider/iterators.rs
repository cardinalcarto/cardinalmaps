//! # Graph iterators
//!
//! This module supports filtered iteration over graph tiles.
//! While this module appears to make quite heavy use of the Rust type system's abstractions,
//! the code has been benchmarked against large scale datasets to ensure performance matches
//! the same code as if it were inlined (tested using release builds with LTO).
//!
//! # Partial tile sets
//!
//! A regional extract only contains some tiles,
//! and the tiles at its edge reference features in tiles that are not present.
//! The searches here skip those references
//! (they report what they can reach, and nothing beyond the edge of the extract),
//! rather than failing the whole search.
//! Any other failure (I/O, corrupt data, and so on) is reported as an error item.

use crate::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use crate::spatial::{DistanceApproximator, bbox_with_center};
use crate::tile_hierarchy::STANDARD_LEVELS;
use crate::tile_provider::{
    GraphTileProvider, GraphTileProviderError, InvalidSearchError, TileAvailability,
};
use crate::{BIN_COUNT, GraphId};
use core::fmt::Display;
use core::marker::PhantomData;
use core::ops::RangeInclusive;
use geo::{
    Closest, ClosestPoint, Coord, CoordFloat, Distance, GeoFloat, HasDimensions, Haversine,
    LineString, Point, Validation, coord,
};
use num_traits::FromPrimitive;
use std::collections::VecDeque;

/// The number of edge bins along each side of a tile.
const BINS_PER_SIDE: usize = 5;
/// [`BINS_PER_SIDE`] as a float.
const BINS_PER_SIDE_F32: f32 = 5.0;
const _: () = assert!(
    BINS_PER_SIDE * BINS_PER_SIDE == BIN_COUNT,
    "the edge bin grid of a tile must be square"
);

/// The longest distance (in meters) that we give to
/// [`DistanceApproximator::is_probably_within_distance_of`].
///
/// That method is only accurate over short distances,
/// and it asserts (in debug builds) that it is used for less than this.
const APPROXIMATOR_MAX_METERS: f32 = 20_000.0;

/// Converts an `f32` into the float type of a search.
///
/// This is a helper to avoid ambiguity between [`From::from`] and [`num_traits::NumCast::from`].
#[inline]
fn from_f32<N: From<f32>>(value: f32) -> N {
    N::from(value)
}

/// Converts a float of any type into `f64`, for error reporting.
#[inline]
fn report<N: CoordFloat>(value: N) -> f64 {
    value.to_f64().unwrap_or(f64::NAN)
}

/// A validated circular search area.
#[derive(Copy, Clone, Debug)]
pub(super) struct SearchArea<N: CoordFloat> {
    center: Point<N>,
    radius: N,
    north: N,
    east: N,
    south: N,
    west: N,
}

impl<N: CoordFloat + FromPrimitive + From<f32>> SearchArea<N> {
    /// Validates the parameters of a search,
    /// and computes its bounding box.
    ///
    /// # Errors
    ///
    /// The center must be a WGS84 coordinate (with finite components),
    /// and the radius must be finite and non-negative.
    pub(super) fn new(center: Point<N>, radius: N) -> Result<Self, InvalidSearchError> {
        let (lon, lat) = (center.x(), center.y());
        // These comparisons are false for NaN, so they also reject non-finite values.
        let lon_ok = lon >= from_f32(-180.0) && lon <= from_f32(180.0);
        let lat_ok = lat >= from_f32(-90.0) && lat <= from_f32(90.0);
        if !(lon_ok && lat_ok) {
            return Err(InvalidSearchError::InvalidCenter {
                lon: report(lon),
                lat: report(lat),
            });
        }
        if !(radius >= N::zero() && radius.is_finite()) {
            return Err(InvalidSearchError::InvalidRadius(report(radius)));
        }

        let (north, east, south, west) = bbox_with_center(center, radius);
        if ![north, east, south, west].iter().all(|v| v.is_finite()) {
            return Err(InvalidSearchError::NonFiniteBoundingBox);
        }

        Ok(Self {
            center,
            radius,
            north,
            east,
            south,
            west,
        })
    }

    /// The base IDs of all tiles (at every standard level) that intersect the search area.
    ///
    /// This is purely geometric; some of the tiles may not exist in the tile set.
    pub(super) fn candidate_tiles(&self) -> impl Iterator<Item = GraphId> + '_ {
        STANDARD_LEVELS.iter().flat_map(|level| {
            level.tiles_intersecting_bbox(self.north, self.east, self.south, self.west)
        })
    }

    /// The longitude range of the search area that overlaps a tile.
    ///
    /// When the search area crosses the antimeridian, its bounding box has `west > east`.
    /// A tile lies entirely on one side of the antimeridian,
    /// so only the part of the box on that side matters.
    fn lon_range_for_tile(&self, tile_west: N) -> (N, N) {
        if self.west > self.east {
            if tile_west >= N::zero() {
                (self.west, from_f32(180.0))
            } else {
                (from_f32(-180.0), self.east)
            }
        } else {
            (self.west, self.east)
        }
    }
}

/// A handle to an object in the graph in response to a search.
pub struct GraphSearchResult<T, N: CoordFloat + FromPrimitive, H: GraphTile> {
    handle: H,
    graph_id: GraphId,
    approx_distance: N,
    _marker: PhantomData<T>,
}

impl<T, N: CoordFloat + FromPrimitive, H: GraphTile> GraphSearchResult<T, N, H> {
    /// The full graph ID of the result.
    #[inline]
    pub fn graph_id(&self) -> GraphId {
        self.graph_id
    }

    /// The approximate distance to the object from the query point.
    ///
    /// The distance is calculated using the squares approximator ([`crate::spatial::DistanceApproximator`]),
    /// which is not quite as accurate as Haversine distance, but is significantly faster and has
    /// acceptable error over short distances.
    #[inline]
    pub fn approx_distance(&self) -> N {
        self.approx_distance
    }

    /// The tile that contains the result.
    #[inline]
    pub fn tile(&self) -> &H {
        &self.handle
    }
}

impl<N: CoordFloat + FromPrimitive, H: GraphTile> GraphSearchResult<NodeInfo, N, H> {
    /// Retrieves the node info for the graph node.
    ///
    /// # Performance
    ///
    /// This is a cheap operation, since this struct keeps an internal handle to the tile.
    ///
    /// # Errors
    ///
    /// The search only returns nodes that it found in the tile,
    /// so this does not fail in practice.
    /// It is fallible so that no tile access can panic.
    #[inline]
    pub fn node_info(&self) -> Result<&NodeInfo, crate::graph_tile::LookupError> {
        self.handle.get_node(self.graph_id)
    }
}

impl<N: CoordFloat + FromPrimitive, H: GraphTile> GraphSearchResult<DirectedEdge, N, H> {
    /// Retrieves the directed edge.
    ///
    /// # Performance
    ///
    /// This is a cheap operation, since this struct keeps an internal handle to the tile.
    ///
    /// # Errors
    ///
    /// The search only returns edges that it found in the tile,
    /// so this does not fail in practice.
    /// It is fallible so that no tile access can panic.
    #[inline]
    pub fn directed_edge(&self) -> Result<&DirectedEdge, crate::graph_tile::LookupError> {
        self.handle.get_directed_edge(self.graph_id)
    }
}

/// An item yielded by a spatial search.
pub(super) type SearchItem<T, N, H> = Result<GraphSearchResult<T, N, H>, GraphTileProviderError>;

/// The results of a spatial search within a single tile.
type TileSearchResults<T, N, H> = Result<Vec<GraphSearchResult<T, N, H>>, GraphTileProviderError>;

/// An iterator over results within a given radius.
///
/// Used internally and returned as an opaque `impl Iterator`.
pub(super) struct ResultsWithinRadius<
    'prov,
    F: Fn(&T) -> bool,
    T,
    P: GraphTileProvider,
    N: CoordFloat + FromPrimitive,
> {
    provider: &'prov P,
    filter_predicate: F,
    area: SearchArea<N>,
    /// The level 2 tiles that are yet to be searched.
    tile_ids: Vec<GraphId>,
    buffer: VecDeque<GraphSearchResult<T, N, P::TileHandle>>,
}

impl<'prov, F: Fn(&T) -> bool, T, P: GraphTileProvider, N: CoordFloat + FromPrimitive + From<f32>>
    ResultsWithinRadius<'prov, F, T, P, N>
{
    pub(super) fn new(provider: &'prov P, filter_predicate: F, area: SearchArea<N>) -> Self {
        // We can use the edge bins at L2 which also contain L0 and L1 edges; clever, right?
        let tile_ids = area
            .candidate_tiles()
            .filter(|tile| tile.level() == 2)
            .collect();

        Self {
            provider,
            filter_predicate,
            area,
            tile_ids,
            buffer: VecDeque::new(),
        }
    }

    /// Advances to the next result,
    /// searching one tile at a time with `search_tile`.
    fn next_with<S>(&mut self, search_tile: S) -> Option<SearchItem<T, N, P::TileHandle>>
    where
        S: Fn(&P, P::TileHandle, &F, &SearchArea<N>) -> TileSearchResults<T, N, P::TileHandle>,
    {
        // A loop rather than recursion,
        // so that a long run of empty or missing tiles cannot overflow the stack.
        loop {
            if let Some(result) = self.buffer.pop_front() {
                return Some(Ok(result));
            }

            let base_id = self.tile_ids.pop()?;
            let tile_handle = match self
                .provider
                .get_handle_for_tile_containing(base_id)
                .if_available()
            {
                Ok(Some(tile_handle)) => tile_handle,
                // The tile is outside the extract; there is nothing to find there.
                Ok(None) => continue,
                Err(err) => return Some(Err(err)),
            };

            match search_tile(
                self.provider,
                tile_handle,
                &self.filter_predicate,
                &self.area,
            ) {
                Ok(results) => self.buffer.extend(results),
                Err(err) => return Some(Err(err)),
            }
        }
    }
}

// Node iterator
impl<F: Fn(&NodeInfo) -> bool, P: GraphTileProvider, N: CoordFloat + FromPrimitive + From<f32>>
    Iterator for ResultsWithinRadius<'_, F, NodeInfo, P, N>
{
    type Item = SearchItem<NodeInfo, N, P::TileHandle>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.next_with(nodes_within_radius)
    }
}

// Edge iterator
impl<
    F: Fn(&DirectedEdge) -> bool,
    P: GraphTileProvider,
    N: GeoFloat + CoordFloat + FromPrimitive + From<f32> + Display,
> Iterator for ResultsWithinRadius<'_, F, DirectedEdge, P, N>
{
    type Item = SearchItem<DirectedEdge, N, P::TileHandle>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.next_with(edges_within_radius)
    }
}

/// Keeps a "sticky" tile handle while visiting features in tile order,
/// so that consecutive features in the same tile only fetch it once.
///
/// It also remembers the last tile that was not available,
/// so that a run of references into a missing tile only looks it up once.
struct StickyTile<'a, P: GraphTileProvider> {
    provider: &'a P,
    tile: P::TileHandle,
    missing: Option<GraphId>,
}

impl<'a, P: GraphTileProvider> StickyTile<'a, P> {
    fn new(provider: &'a P, tile: P::TileHandle) -> Self {
        Self {
            provider,
            tile,
            missing: None,
        }
    }

    /// Switches to the tile containing `graph_id`.
    ///
    /// Returns `Ok(None)` if that tile is not available.
    fn get(&mut self, graph_id: GraphId) -> Result<Option<&P::TileHandle>, GraphTileProviderError> {
        if self.tile.may_contain_id(graph_id) {
            return Ok(Some(&self.tile));
        }

        let base_id = graph_id.tile_base_id();
        if self.missing == Some(base_id) {
            return Ok(None);
        }

        if let Some(tile) = self
            .provider
            .get_handle_for_tile_containing(graph_id)
            .if_available()?
        {
            self.tile = tile;
            Ok(Some(&self.tile))
        } else {
            self.missing = Some(base_id);
            Ok(None)
        }
    }
}

/// A helper function that finds nodes using L2 edge bins as a spatial filter.
///
/// This function uses edge bins to prune the search space without iterating over every node in the tile
/// (and in some cases, skipping tile levels entirely).
///
/// Nodes in tiles that are not available are skipped.
fn nodes_within_radius<P: GraphTileProvider, F, N: CoordFloat + FromPrimitive + From<f32>>(
    provider: &P,
    tile: P::TileHandle,
    node_filter_predicate: &F,
    area: &SearchArea<N>,
) -> TileSearchResults<NodeInfo, N, P::TileHandle>
where
    F: Fn(&NodeInfo) -> bool,
{
    let approximator = DistanceApproximator::new(area.center.into());
    let radius_squared = area.radius * area.radius;
    let tile_graph_id = tile.graph_id();

    // Use edge bins to get a set of candidate *edges*.
    let initial_edge_ids = prefilter_edge_ids(&tile, area)?;

    let mut results = Vec::with_capacity(initial_edge_ids.len());
    let mut unresolved_edge_ids = Vec::new();
    let mut deferred_node_lookups = Vec::with_capacity(initial_edge_ids.len());

    // Records a node in `tile`, or defers it if it lives in another tile.
    let visit_node = |node_id: GraphId,
                      tile: &P::TileHandle,
                      results: &mut Vec<_>,
                      deferred: &mut Vec<GraphId>|
     -> Result<(), GraphTileProviderError> {
        if tile.may_contain_id(node_id) {
            if let Some(result) = filter_node(
                &approximator,
                node_id,
                tile,
                node_filter_predicate,
                radius_squared,
            )? {
                results.push(result);
            }
        } else {
            deferred.push(node_id);
        }
        Ok(())
    };

    // First, we iterate over the very rough list of candidate edge IDs from the edge bins
    // (a spatial filter) and find the start and end node ID (or as much of it as we can).
    for edge_id in initial_edge_ids {
        if !tile.may_contain_id(edge_id) {
            // Edge is in another tile.
            unresolved_edge_ids.push(edge_id);
            continue;
        }

        let end_node_id = tile.get_directed_edge(edge_id)?.end_node_id();
        visit_node(end_node_id, &tile, &mut results, &mut deferred_node_lookups)?;

        // To get the start node, we need to follow the opposing edge.
        let opp_edge_index = tile.get_opp_edge_index(edge_id)?;
        if opp_edge_index.end_node_id.tile_base_id() == tile_graph_id {
            // The opposing edge is contained within the tile
            let opp_edge_id = provider.graph_id_for_opposing_edge_index(opp_edge_index, &tile)?;
            let start_node_id = tile.get_directed_edge(opp_edge_id)?.end_node_id();
            visit_node(
                start_node_id,
                &tile,
                &mut results,
                &mut deferred_node_lookups,
            )?;
        }
        // Otherwise, the opposing edge is in a different tile.
        // Its end node (our start node) is the end node of some edge in that tile,
        // so that tile's own search finds it when it intersects the search area.
    }

    // Sort unresolved edge IDs by tile to minimize tile handle churn.
    unresolved_edge_ids.sort_unstable_by_key(GraphId::tile_base_id);

    // Resolve edges from other tiles.
    let mut sticky = StickyTile::new(provider, tile);
    for edge_id in unresolved_edge_ids {
        // An edge in a tile that is not in the extract can't be resolved; skip it.
        let Some(tile) = sticky.get(edge_id)? else {
            continue;
        };

        let end_node_id = tile.get_directed_edge(edge_id)?.end_node_id();
        visit_node(end_node_id, tile, &mut results, &mut deferred_node_lookups)?;

        let opp_edge_index = tile.get_opp_edge_index(edge_id)?;
        if opp_edge_index.end_node_id.tile_base_id() == tile.graph_id() {
            // Fast path: the opposing edge is also contained within the tile!
            let opp_edge_id = provider.graph_id_for_opposing_edge_index(opp_edge_index, tile)?;
            let start_node_id = tile.get_directed_edge(opp_edge_id)?.end_node_id();
            visit_node(
                start_node_id,
                tile,
                &mut results,
                &mut deferred_node_lookups,
            )?;
        }
    }

    // Sort and deduplicate node IDs to avoid processing the same node multiple times
    // and to ensure cache-friendly access patterns.
    deferred_node_lookups.sort_unstable();
    deferred_node_lookups.dedup();

    for node_id in deferred_node_lookups {
        // A node in a tile that is not in the extract is beyond its edge; skip it.
        let Some(tile) = sticky.get(node_id)? else {
            continue;
        };

        if let Some(result) = filter_node(
            &approximator,
            node_id,
            tile,
            node_filter_predicate,
            radius_squared,
        )? {
            results.push(result);
        }
    }

    results.sort_unstable_by_key(|result| result.graph_id);
    results.dedup_by_key(|result| result.graph_id);

    Ok(results)
}

/// Returns the node as a search result if it passes the filter and is within the radius.
///
/// # Errors
///
/// Fails if `node_id` is not a node of `tile`.
fn filter_node<F, N: CoordFloat + FromPrimitive + From<f32>, T: GraphTile + Clone>(
    approximator: &DistanceApproximator<N>,
    node_id: GraphId,
    tile: &T,
    node_filter_predicate: &F,
    radius_squared: N,
) -> Result<Option<GraphSearchResult<NodeInfo, N, T>>, GraphTileProviderError>
where
    F: Fn(&NodeInfo) -> bool,
{
    let node = tile.get_node(node_id)?;

    // Apply the filter predicate first.
    if !node_filter_predicate(node) {
        return Ok(None);
    }

    // Check distance.
    let sw = tile.header().sw_corner();
    let sq_dist = approximator.distance_squared(convert_coord(node.coordinate(sw)));

    Ok((sq_dist <= radius_squared).then(|| GraphSearchResult {
        handle: tile.clone(),
        graph_id: node_id,
        approx_distance: sq_dist.sqrt(),
        _marker: PhantomData,
    }))
}

/// Converts an `f32` coordinate into the float type of a search.
#[inline]
fn convert_coord<N: CoordFloat + From<f32>>(c: Coord<f32>) -> Coord<N> {
    coord! { x: from_f32(c.x), y: from_f32(c.y) }
}

/// A helper function that finds edges using L2 edge bins as a spatial filter.
///
/// Edges in tiles that are not available are skipped.
/// When an edge is available but its opposing edge is not
/// (the edge leaves the extract), only the available edge is returned.
fn edges_within_radius<
    P: GraphTileProvider,
    F,
    N: GeoFloat + CoordFloat + FromPrimitive + From<f32> + Display,
>(
    provider: &P,
    tile: P::TileHandle,
    edge_filter_predicate: &F,
    area: &SearchArea<N>,
) -> TileSearchResults<DirectedEdge, N, P::TileHandle>
where
    F: Fn(&DirectedEdge) -> bool,
{
    let approximator = DistanceApproximator::new(area.center.into());
    let tile_graph_id = tile.graph_id();

    // Build up the set of edge IDs.
    // Our decision for the moment is to make the API easy to use, which means we make a break
    // from how Loki does things, only returning one edge or the other based on a filter.
    // To make our iterator easy to use, we're rolling with all (sortof) edges.
    //
    // The "sortof" qualifier is because edge bins don't include inbound edges for tweeners,
    // which is great, since this isn't what we're searching for anyway.
    // This will give us a set of all edge IDs that are possibly relevant in both directions.
    // This enables the caller to filter based on what they will actually use
    // in terms of forward edges, and removes some of the 4D mental chess burden.
    let initial_edge_ids = prefilter_edge_ids(&tile, area)?;

    let mut edge_ids = Vec::with_capacity(initial_edge_ids.len() * 2);
    let mut unresolved_opp_edge_indexes = Vec::new();
    let mut unresolved_opp_edge_index_lookups = Vec::new();

    // First, we iterate over the very rough list of candidate edge IDs from the edge bins
    // (a spatial filter) and calculate the opposing edge ID (or as much of it as we can).
    for edge_id in initial_edge_ids {
        if tile.may_contain_id(edge_id) {
            let opp_edge_index = tile.get_opp_edge_index(edge_id)?;
            if opp_edge_index.end_node_id.tile_base_id() == tile_graph_id {
                // Fast path: the edge and its opposing edge are fully contained within the tile.
                let opp_edge_id =
                    provider.graph_id_for_opposing_edge_index(opp_edge_index, &tile)?;
                edge_ids.push(edge_id);
                edge_ids.push(opp_edge_id);
            } else {
                // The edge starts in this tile but ends in another.
                // We eventually need to hit another tile, but we can at least build up the index struct.
                unresolved_opp_edge_indexes.push((edge_id, opp_edge_index));
            }
        } else {
            // The edge doesn't start in this tile (e.g. an L0 or L1 edge); defer processing until later
            unresolved_opp_edge_index_lookups.push(edge_id);
        }
    }

    // Sort the lists of unresolved items so that we don't need to keep churning through tile handles
    unresolved_opp_edge_indexes.sort_unstable_by_key(|(_, index)| index.end_node_id.tile_base_id());
    unresolved_opp_edge_index_lookups.sort_unstable_by_key(GraphId::tile_base_id);

    // Set up a tile handle that will be "sticky" (low churn)
    // as we iterate through the "foreign" edges in tile order
    let mut other_tile = StickyTile::new(provider, tile.clone());

    // Resolve opposing edge indexes which originate in the current tile but end in another.
    for (edge_id, opp_edge_index) in unresolved_opp_edge_indexes {
        // The edge itself is in this tile, so it is always a candidate.
        edge_ids.push(edge_id);
        // Its opposing edge is only a candidate if its tile is in the extract.
        if let Some(end_tile) = other_tile.get(opp_edge_index.end_node_id)? {
            edge_ids.push(provider.graph_id_for_opposing_edge_index(opp_edge_index, end_tile)?);
        }
    }

    // Resolve cases where neither edge is in the current L2 tile
    for edge_id in unresolved_opp_edge_index_lookups {
        // If the edge's own tile is not in the extract, there is nothing to report.
        let Some(edge_tile) = other_tile.get(edge_id)? else {
            continue;
        };

        edge_ids.push(edge_id);
        if let Some(opp_edge_id) = provider
            .get_opposing_edge_id(edge_id, edge_tile)
            .if_available()?
        {
            edge_ids.push(opp_edge_id);
        }
    }
    drop(other_tile);

    // Sort the final vector to ensure we hit tiles in a more cache-friendly order
    // and don't churn tile handles.
    edge_ids.sort_unstable();

    // It's possible for an edge to show up multiple times.
    // This efficiently removes all dupes as the vector is sorted.
    edge_ids.dedup();

    let mut results = Vec::with_capacity(edge_ids.len());

    // Final "sticky" tile handle; we'll iterate in order, so we only need to remember the last one.
    let mut sticky = StickyTile::new(provider, tile);

    // Loop through the pre-filtered edges (only a subset of levels with binning; otherwise all edges)
    // NOTE: We may currently invoke calculate_distance on both edges in the pair.
    // There is probably a way to avoid doing this, but initial attempts at stashing the opposing edge ID
    // in the vector and using a HashMap to save distance calculations didn't show any improvement.
    for edge_id in edge_ids {
        // Every edge here was found in an available tile above,
        // but skipping is the right answer if the tile has since become unavailable.
        let Some(tile) = sticky.get(edge_id)? else {
            continue;
        };

        let edge = tile.get_directed_edge(edge_id)?;
        if edge_filter_predicate(edge)
            && let Some(d) = calculate_distance(tile, edge_id, edge, &approximator, area)?
            && d <= area.radius
        {
            results.push(GraphSearchResult {
                handle: tile.clone(),
                graph_id: edge_id,
                approx_distance: d,
                _marker: PhantomData,
            });
        }
    }

    Ok(results)
}

/// Returns the range of bins along one axis that overlap `[min, max]`.
///
/// `origin` is the coordinate of the tile's south-west corner along the axis.
/// Values outside the tile are clamped to its first or last bin.
fn bin_range<N: CoordFloat>(
    min: N,
    max: N,
    origin: N,
    bin_size: N,
) -> Result<RangeInclusive<usize>, InvalidSearchError> {
    let last = BINS_PER_SIDE - 1;
    let to_bin = |value: N| -> Result<usize, InvalidSearchError> {
        let bin = ((value - origin) / bin_size).floor();
        if !bin.is_finite() {
            return Err(InvalidSearchError::NonFiniteBoundingBox);
        }
        if bin <= N::zero() {
            return Ok(0);
        }
        // A finite value only fails to convert if it is larger than `usize::MAX`.
        Ok(bin.to_usize().map_or(last, |bin| bin.min(last)))
    };

    Ok(to_bin(min)?..=to_bin(max)?)
}

/// Returns a pre-filtered list of edge IDs in a tile,
/// based on the search area, using edge bins as a rough index.
///
/// This function assumes that tiles at level 2 have valid edge bins
/// (a standard step in the Valhalla tile build).
/// If edge bins don't exist at level 2, this function will fail to find any edges.
///
/// # Errors
///
/// Fails if the bin range can't be computed
/// (only possible for a non-finite search area or tile corner),
/// or if a feature ID can't be constructed for a tile at another level.
fn prefilter_edge_ids<T: GraphTile, N: CoordFloat + FromPrimitive + From<f32>>(
    tile: &T,
    area: &SearchArea<N>,
) -> Result<Vec<GraphId>, GraphTileProviderError> {
    let header = tile.header();
    let graph_id = header.graph_id();
    let sw = header.sw_corner();

    if graph_id.level() != 2 {
        // No bins available at this level, so we need to scan the whole tile
        return (0..u64::from(header.directed_edge_count()))
            .map(|idx| Ok(graph_id.with_feature_index(idx)?))
            .collect();
    }

    // Compute which bins intersect the query radius within this tile
    let tile_size = STANDARD_LEVELS[2].tiling_system.tile_size;
    // The following code has a lot of conversions, but profiling demonstrates ~zero overhead
    // even in a hot loop.
    let bin_size: N = from_f32(tile_size / BINS_PER_SIDE_F32);

    let sw_lon: N = from_f32(sw.x);
    let sw_lat: N = from_f32(sw.y);
    let ne_lon = sw_lon + from_f32(tile_size);
    let ne_lat = sw_lat + from_f32(tile_size);

    // Get the bounding box of the search, and clamp it to the tile
    let (west, east) = area.lon_range_for_tile(sw_lon);
    let bbox_min_lon = west.max(sw_lon);
    let bbox_max_lon = east.min(ne_lon);
    let bbox_min_lat = area.south.max(sw_lat);
    let bbox_max_lat = area.north.min(ne_lat);

    // Figure out which bins we need to look into
    // TODO: Double check the Valhalla code for this... they seem to expand the search in some cases.
    // Since we are already creating a bounding box that's buffered,
    // it's unclear to me that we actually NEED to do that.
    let x_bins = bin_range(bbox_min_lon, bbox_max_lon, sw_lon, bin_size)?;
    let y_bins = bin_range(bbox_min_lat, bbox_max_lat, sw_lat, bin_size)?;

    // Collect candidate edges from bins
    let mut edge_ids: Vec<GraphId> = Vec::new();
    for bin_y in y_bins {
        for bin_x in x_bins.clone() {
            let bin_index = tile.bin_index_xy(bin_x, bin_y);
            edge_ids.extend(tile.edges_in_bin(bin_index));
        }
    }

    Ok(edge_ids)
}

/// Computes the distance from the search center to an edge,
/// or `None` if the edge is certainly too far away (or has no usable shape).
///
/// # Errors
///
/// Fails if the edge info or shape of the edge can't be decoded.
fn calculate_distance<T: GraphTile, N: GeoFloat + FromPrimitive + From<f32> + Display>(
    tile: &T,
    edge_id: GraphId,
    edge: &DirectedEdge,
    approximator: &DistanceApproximator<N>,
    area: &SearchArea<N>,
) -> Result<Option<N>, GraphTileProviderError> {
    let shape_error = |source| GraphTileProviderError::InvalidShape { edge_id, source };

    // Decoded lazily, since the shortcut below often makes it unnecessary.
    let mut edge_info = None;

    // Try to short circuit if there's no way we're close to the edge...
    // To do this, we'll leverage the property that, for a LineString of length M,
    // all points must be within radius M of every other point
    // (the shortest path is a straight line between 2 points).
    let any_coord = if let Ok(end_node) = tile.get_node(edge.end_node_id()) {
        // Path 1: If we can cheaply get the end node (this is always faster than getting the start node
        // due to the singly linked list pointer structure),
        // use that coordinate
        convert_coord(end_node.coordinate(tile.header().sw_corner()))
    } else {
        // Path 2: Just decode the first coordinate of its geometry.
        let ei = tile.get_edge_info(edge)?;
        let first = ei.decode_first_coordinate().map_err(shape_error)?;
        edge_info = Some(ei);
        first
    };

    // Use the approximator to weed out lines which couldn't possibly be
    // close to the edge geometry.
    // Given one endpoint of the edge, we set the threshold at length + the search distance.
    // This is a conservative heuristic, since most edges will not be straight lines.
    // The approximator is only meant for short distances,
    // so we skip the shortcut (and do the exact computation) beyond that.
    // In a real-world dataset, this cuts about 2/3 of the runtime.
    if let Some(length) = N::from_u32(edge.length()) {
        let threshold = area.radius + length;
        if threshold < from_f32(APPROXIMATOR_MAX_METERS)
            && !approximator.is_probably_within_distance_of(any_coord, threshold)
        {
            return Ok(None);
        }
    }

    // Note: it doesn't matter if the edge info is forward or reverse here; we just need the closest point.
    let ei = match edge_info {
        Some(ei) => ei,
        None => tile.get_edge_info(edge)?,
    };
    let shape = LineString::new(ei.decode_raw_shape().map_err(shape_error)?);

    if !shape.is_valid() || shape.is_empty() {
        tracing::debug!(
            edge = format!("{edge:?}"),
            reason = format!("Shape is invalid or empty: {:?}", shape)
        );
        return Ok(None);
    }

    let query_point = area.center;
    let closest = shape.closest_point(&query_point);
    Ok(match closest {
        Closest::SinglePoint(c) | Closest::Intersection(c) => {
            // TODO: We might be able to use an approximator here too??
            Some(Haversine.distance(query_point, Point::new(c.x(), c.y())))
        }
        Closest::Indeterminate => {
            tracing::error!(edge = format!("{edge:?}"), lon = %query_point.x(), lat = %query_point.y(), reason = format!("Indeterminate closest_point result for linestring: {:?}", shape));
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::point;

    #[test]
    fn search_area_rejects_invalid_parameters() {
        for (lon, lat) in [
            (f64::NAN, 0.0),
            (0.0, f64::NAN),
            (f64::INFINITY, 0.0),
            (0.0, f64::NEG_INFINITY),
            (180.5, 0.0),
            (0.0, -90.5),
        ] {
            let err = SearchArea::new(point!(x: lon, y: lat), 25.0).expect_err("invalid center");
            assert!(
                matches!(err, InvalidSearchError::InvalidCenter { .. }),
                "({lon}, {lat}) gave {err:?}"
            );
        }

        for radius in [f64::NAN, f64::INFINITY, -1.0] {
            let err = SearchArea::new(point!(x: 1.5, y: 42.5), radius).expect_err("invalid radius");
            assert!(
                matches!(err, InvalidSearchError::InvalidRadius(_)),
                "{radius} gave {err:?}"
            );
        }

        // Edge cases which are valid
        for (lon, lat) in [(180.0, 90.0), (-180.0, -90.0), (0.0, 0.0)] {
            SearchArea::new(point!(x: lon, y: lat), 0.0).expect("valid search area");
        }
        for (lon, lat) in [(180.0f32, 90.0f32), (-180.0, -90.0)] {
            SearchArea::new(point!(x: lon, y: lat), 0.0).expect("valid search area");
        }
    }

    #[test]
    fn search_area_lon_range_across_antimeridian() {
        let area = SearchArea::new(point!(x: 179.99, y: 0.0), 5_000.0).expect("valid area");
        assert!(area.west > area.east, "the bbox should wrap");

        let (w, e) = area.lon_range_for_tile(179.75);
        assert_eq!((w, e), (area.west, 180.0));
        let (w, e) = area.lon_range_for_tile(-180.0);
        assert_eq!((w, e), (-180.0, area.east));

        // Both sides of the antimeridian are candidates
        let tiles: Vec<_> = area.candidate_tiles().filter(|t| t.level() == 2).collect();
        let level = &STANDARD_LEVELS[2];
        let n_cols = u64::from(level.tiling_system.n_cols);
        assert!(tiles.iter().any(|t| t.tile_id() % n_cols == 0));
        assert!(tiles.iter().any(|t| t.tile_id() % n_cols == n_cols - 1));
    }

    #[test]
    fn bin_range_clamps() {
        // A box entirely inside a 0.25 degree tile at the origin, with 0.05 degree bins
        assert_eq!(bin_range(0.06, 0.11, 0.0, 0.05), Ok(1..=2));
        // Out of range on both sides
        assert_eq!(bin_range(-10.0, 10.0, 0.0, 0.05), Ok(0..=4));
        // Huge values clamp instead of overflowing
        assert_eq!(bin_range(0.0, 1e300, 0.0, 0.05), Ok(0..=4));
        assert_eq!(
            bin_range(f64::NAN, 0.1, 0.0, 0.05),
            Err(InvalidSearchError::NonFiniteBoundingBox)
        );
    }
}
