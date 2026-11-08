//! # Graph tile providers
//!
//! This module contains implementations for several graph tile providers,
//! which all implement the [`GraphTileProvider`] trait:
//!
//! - [`TarballTileProvider`] reads a memory-mapped tarball extract
//!   (what production servers and mobile apps use).
//! - [`DirectoryGraphTileProvider`] reads a directory of tile files.
//! - [`TrafficTileProvider`] reads (and writes) live traffic tiles.
//!   It does not implement [`GraphTileProvider`], since traffic tiles are not graph tiles.
//!
//! # Partial tile sets
//!
//! A tile set does not have to cover the world.
//! A tile that is not in the set is reported as
//! [`GraphTileProviderError::TileNotAvailable`],
//! which callers should treat as a dead end rather than a failure
//! (see [`TileAvailability`]).
//! The spatial searches here skip missing tiles on their own.

use crate::GraphId;
use core::fmt::Display;
use dashmap::DashMap;
use geo::{CoordFloat, GeoFloat, Point};
use num_traits::FromPrimitive;
use std::sync::Arc;
use std::sync::Mutex;

mod directory;
mod error;
mod iterators;
mod tarball;
mod traffic;

use crate::graph_tile::{
    DirectedEdge, GraphTile, GraphTileView, LookupError, NodeInfo, OpposingEdgeIndex,
};
pub use directory::DirectoryGraphTileProvider;
pub use error::{
    GraphTileProviderError, GraphTileProviderErrorKind, InvalidSearchError, TileAvailability,
};
pub use iterators::GraphSearchResult;
use iterators::{ResultsWithinRadius, SearchArea, SearchItem};
pub use tarball::{
    InvalidIndexBinError, InvalidTarballError, TarballTileProvider, TileIndexBinEntry,
    parse_index_bin, serialize_index_bin,
};
pub use traffic::TrafficTileProvider;

/// The most edges that [`GraphTileProvider::get_shortcut`] walks before giving up.
///
/// The walk follows the edges inside a single shortcut,
/// which are far fewer than this in any real graph.
/// The bound only exists so that corrupt data cannot make the walk loop forever.
const MAX_SHORTCUT_WALK: usize = 1 << 16;

/// A generic trait for all graph tile providers.
///
/// This provides a standard interface so users don't need to care about the details
/// (e.g., if a tile is loaded from a file on disk, from a memory map, etc.).
/// Additionally, we get to define some pieces of shared logic that are provider-independent directly in the trait.
/// Monomorphization of generics ensures this is performant (unless you go out of your way to erase types).
///
/// # Missing tiles
///
/// Every method that loads a tile returns [`GraphTileProviderError::TileNotAvailable`]
/// when the tile is not part of the tile set.
/// This is not a sign of corrupt data:
/// a regional extract references tiles beyond its edge.
/// Use [`TileAvailability::if_available`] to treat it as a dead end.
pub trait GraphTileProvider {
    /// The tile handle type for owned handles.
    ///
    /// Code using the `GraphTileProvider` may rely on the handles being cheaply cloneable.
    type TileHandle: GraphTile + Clone;

    /// Gets the tile containing the given graph ID,
    /// and does some work in a closure which takes the reference as a parameter.
    ///
    /// # Performance
    ///
    /// This may or may not make use of caches,
    /// require filesystem access, etc.
    /// This is left up to the implementation.
    ///
    /// # Errors
    ///
    /// Returns [`GraphTileProviderError::TileNotAvailable`] if the tile is not in the tile set.
    /// This operation may also fail for other reasons, including I/O errors and corrupt tiles.
    /// Refer to [`GraphTileProviderError`] for details.
    fn with_tile_containing<F, T>(
        &self,
        graph_id: GraphId,
        process: F,
    ) -> Result<T, GraphTileProviderError>
    where
        F: FnOnce(&GraphTileView) -> T;

    /// Gets a tile containing the given graph ID.
    ///
    /// The result is an owned handle to a graph tile,
    /// which implements the [`GraphTile`] trait.
    ///
    /// # Performance
    ///
    /// Implementations SHOULD ensure that this method incurs only minimal (near zero)
    /// overhead.
    ///
    /// # Errors
    ///
    /// Returns [`GraphTileProviderError::TileNotAvailable`] if the tile is not in the tile set.
    /// This operation may also fail for other reasons, including I/O errors and corrupt tiles.
    /// Refer to [`GraphTileProviderError`] for details.
    fn get_handle_for_tile_containing(
        &self,
        graph_id: GraphId,
    ) -> Result<Self::TileHandle, GraphTileProviderError>;

    /// Does the tile set contain the tile that the given graph ID belongs to?
    ///
    /// This only checks for the presence of the tile, without loading or validating it,
    /// so loading a tile that is present may still fail (for example, if it is corrupt).
    ///
    /// # Errors
    ///
    /// Implementations which need I/O to answer may fail with an I/O error.
    fn contains_tile(&self, graph_id: GraphId) -> Result<bool, GraphTileProviderError>;

    /// Enumerate base tile Graph IDs across all hierarchy levels that intersect a circle around
    /// `center` with radius `radius`,
    /// and are present in the tile set (see [`GraphTileProvider::contains_tile`]).
    ///
    /// `center` is a WGS84 coordinate (longitude and latitude in degrees),
    /// and `radius` is specified in meters.
    ///
    /// # Errors
    ///
    /// Fails with [`GraphTileProviderError::InvalidSearch`] if `center` is not a valid coordinate
    /// or `radius` is negative or not finite,
    /// and with any error from [`GraphTileProvider::contains_tile`].
    #[inline]
    fn tiles_within_radius<N: CoordFloat + FromPrimitive + From<f32>>(
        &self,
        center: Point<N>,
        radius: N,
    ) -> Result<Vec<GraphId>, GraphTileProviderError> {
        let area = SearchArea::new(center, radius)?;
        let mut tiles = Vec::new();
        for tile_id in area.candidate_tiles() {
            if self.contains_tile(tile_id)? {
                tiles.push(tile_id);
            }
        }
        Ok(tiles)
    }

    /// Returns the [`GraphId`] for an opposing edge.
    ///
    /// The input for this typically comes from [`GraphTile::get_opp_edge_index`].
    /// Since the opposite edge may be in another tile, a [`GraphTileProvider`] is required
    /// to fully resolve it.
    ///
    /// # Performance
    ///
    /// If the opposing edge is contained in the same tile as `tile_hint`,
    /// this method can take a fast path; otherwise, it will need to fetch a tile.
    ///
    /// # Errors
    ///
    /// This can fail if an underlying tile load fails
    /// (including [`GraphTileProviderError::TileNotAvailable`]
    /// when the opposing edge is outside of the extract),
    /// or the `OpposingEdgeIndex` points to an invalid edge (e.g., corrupt / invalid tiles).
    fn graph_id_for_opposing_edge_index<T: GraphTile>(
        &self,
        OpposingEdgeIndex {
            end_node_id,
            opposing_edge_index,
        }: OpposingEdgeIndex,
        tile_hint: &T,
    ) -> Result<GraphId, GraphTileProviderError> {
        // Widen before adding, so corrupt indexes can't overflow.
        let opposing_edge_id = |tile_id: GraphId, node_edge_idx: u32| {
            tile_id.with_feature_index(u64::from(node_edge_idx) + u64::from(opposing_edge_index))
        };

        match tile_hint.get_node(end_node_id).map(NodeInfo::edge_index) {
            // Fast path: same tile
            Ok(node_edge_idx) => Ok(opposing_edge_id(
                tile_hint.header().graph_id(),
                node_edge_idx,
            )?),
            // Slow path: fetch from another tile
            Err(LookupError::MismatchedBase) => self.with_tile_containing(end_node_id, |tile| {
                let node_edge_idx = tile.get_node(end_node_id)?.edge_index();
                Ok(opposing_edge_id(tile.header().graph_id(), node_edge_idx)?)
            })?,
            Err(e) => Err(e.into()),
        }
    }

    /// Gets the opposing edge.
    ///
    /// All edges in a Valhalla graph are directed and stored as a pair.
    /// This function makes it easy to get the opposite one.
    ///
    /// # Errors
    ///
    /// This may fail under the following circumstances:
    ///
    /// - Failing to fetch the tile containing the graph ID
    /// - The index within the tile being invalid
    /// - Failing to fetch the tile containing the end node of the edge (or the end node therein);
    ///   at the edge of an extract, this is [`GraphTileProviderError::TileNotAvailable`]
    /// - Corrupt end node information in the tile
    ///
    /// # Performance
    ///
    /// This method will try to look up the edge in the tile view (`tile_hint`) first,
    /// to save a tile fetch (which might have a high overhead in some tile providers).
    fn get_opposing_edge_id<T: GraphTile>(
        &self,
        edge_id: GraphId,
        tile_hint: &T,
    ) -> Result<GraphId, GraphTileProviderError> {
        // This code is ugly because we can't make a generic closure,
        // and we can't prove to the compiler that the tile type will always be the same.
        // Thus we have essentially the same logic twice.
        match tile_hint.get_opp_edge_index(edge_id) {
            // Fast path (the edge is within this tile)
            Ok(idx) => self.graph_id_for_opposing_edge_index(idx, tile_hint),
            // Slow path (need to fetch the tile of the edge)
            Err(LookupError::MismatchedBase) => self.with_tile_containing(edge_id, |tile| {
                let idx = tile.get_opp_edge_index(edge_id)?;
                self.graph_id_for_opposing_edge_index(idx, tile)
            })?,
            Err(e) => Err(e.into()),
        }
    }

    /// Gets the shortcut edge that includes the given edge, if any.
    ///
    /// `Ok(None)` indicates that either no shortcut was found for the provided edge,
    /// OR multiple shortcuts were found.
    ///
    /// This mirrors `GraphReader::GetShortcut` in Valhalla.
    ///
    /// # Errors
    ///
    /// Fails if a tile along the way can't be loaded
    /// (including [`GraphTileProviderError::TileNotAvailable`] at the edge of an extract),
    /// or if the tile data is inconsistent.
    fn get_shortcut(&self, id: GraphId) -> Result<Option<GraphId>, GraphTileProviderError> {
        if id.level() >= 2 {
            // Shortcuts don't exist on these levels
            return Ok(None);
        }

        self.with_tile_containing(id, |tile| {
            if tile.get_directed_edge(id)?.is_shortcut() {
                // If this edge is already a shortcut, return it as-is.
                return Ok(Some(id));
            }

            // Walk backwards along the opposing directed edge until a shortcut beginning is found
            // or until ambiguity / boundary conditions are encountered.
            // The initial case uses the opposing directed edge of the starting edge;
            // later iterations use the continuing edge at the end node of the previous one.
            let first_de_id = self.get_opposing_edge_id(id, tile)?;
            let mut cont_de_id = first_de_id;
            // Were there any shortcuts at the node where the current edge starts?
            let mut shortcut_at_node = false;

            for _ in 0..MAX_SHORTCUT_WALK {
                // The following is definitely not perfectly optimized, but it's "fast enough"
                // as far as we can tell from some stress tests (there's a fairly reasonable fast path
                // when everything is in the current tile, though it is not branch-free).
                // Get end node of the continuing edge and its node info
                let (end_node_id, opp_index) = match tile.get_directed_edge(cont_de_id) {
                    Ok(de) => (de.end_node_id(), de.opposing_edge_index()),
                    Err(LookupError::MismatchedBase) => {
                        self.with_tile_containing(cont_de_id, |tile| {
                            let de = tile.get_directed_edge(cont_de_id)?;
                            Ok::<_, GraphTileProviderError>((
                                de.end_node_id(),
                                de.opposing_edge_index(),
                            ))
                        })??
                    }
                    Err(e) => return Err(e.into()),
                };

                // Get the node info (may reside in a different tile if the edge leaves tile)
                let node_edge_index = match tile.get_node(end_node_id) {
                    Ok(node) => node.edge_index(),
                    Err(LookupError::MismatchedBase) => self
                        .with_tile_containing(end_node_id, |tile| {
                            tile.get_node(end_node_id).map(NodeInfo::edge_index)
                        })??,
                    Err(e) => return Err(e.into()),
                };

                // Construct the opposing edge id at the end node (the edge we "arrived on" at this node)
                let arriving_edge_index = u64::from(node_edge_index) + u64::from(opp_index);
                let arriving_edge_id = end_node_id.with_feature_index(arriving_edge_index)?;

                // Inspect the arriving edge to see if it is superseded (i.e., shortcut starts here)
                let superseded_idx = match tile.get_directed_edge(arriving_edge_id) {
                    Ok(de) => de.superseded_index(),
                    Err(LookupError::MismatchedBase) => {
                        self.with_tile_containing(arriving_edge_id, |tile| {
                            let de = tile.get_directed_edge(arriving_edge_id)?;
                            Ok::<_, GraphTileProviderError>(de.superseded_index())
                        })??
                    }
                    Err(e) => return Err(e.into()),
                };

                if let Some(idx) = superseded_idx {
                    // Calculate the shortcut index within the node's outbound edges
                    let shortcut_abs_index = u64::from(node_edge_index) + u64::from(idx);
                    return Ok(Some(end_node_id.with_feature_index(shortcut_abs_index)?));
                }

                if shortcut_at_node {
                    // Found a shortcut at the node, but it doesn't supersede the arriving edge.
                    // This implies we started outside a shortcut's internal edges.
                    return Ok(None);
                }

                // Continue from the end node, skipping the edge we arrived on.
                let (next_cont, saw_shortcut) = match tile.get_node(end_node_id) {
                    // Fast path (stay within the same tile)
                    Ok(node) => find_continuing_edge(tile, node, arriving_edge_index)?,
                    // Slow path (need to load another tile)
                    Err(LookupError::MismatchedBase) => {
                        self.with_tile_containing(end_node_id, |tile| {
                            let node = tile.get_node(end_node_id)?;
                            find_continuing_edge(tile, node, arriving_edge_index)
                        })??
                    }
                    Err(e) => return Err(e.into()),
                };
                shortcut_at_node = saw_shortcut;

                match next_cont {
                    // No clear continuing edge
                    None => return Ok(None),
                    // Loop detected; no shortcut found
                    Some(next) if next == first_de_id => return Ok(None),
                    Some(next) => cont_de_id = next,
                }
            }

            Err(GraphTileProviderError::UnterminatedShortcutWalk(id))
        })?
    }

    /// Yields an iterator over all nodes within a given radius of a point.
    ///
    /// No sorting or filtering of nodes is performed besides ensuring that they are close enough.
    /// The distance returned (second tuple element) is approximate,
    /// but should be accurate to within 1 meter for radii of up to 20km.
    ///
    /// Tiles that are not in the tile set are skipped
    /// (nodes beyond the edge of an extract are not reported).
    ///
    /// # Performance
    ///
    /// This method is designed to give results with as little overhead as possible:
    ///
    /// - The filter predicate applies before any distance checks.
    /// - It only buffers results internally for one tile at a time.
    /// - The iteration order is not guaranteed to follow any particular sorting.
    /// - Internally, an approximator is used to (very!) quickly weed out unrealistic candidates
    ///   before doing the heavier trigonometry.
    ///
    /// This isn't intended to be used to get nodes over large distances;
    /// the work grows with the area of the search.
    ///
    /// # Errors
    ///
    /// Fails up front with [`GraphTileProviderError::InvalidSearch`]
    /// if `center` is not a valid WGS84 coordinate
    /// or `radius_in_meters` is negative or not finite.
    ///
    /// The iterator may yield error items if a visited tile could not be loaded
    /// (e.g., a filesystem error) or is corrupt (not a valid Valhalla graph tile).
    #[inline]
    fn nodes_within_radius<N: CoordFloat + FromPrimitive + From<f32>, F: Fn(&NodeInfo) -> bool>(
        &self,
        filter_predicate: F,
        center: Point<N>,
        radius_in_meters: N,
    ) -> Result<
        impl Iterator<Item = SearchItem<NodeInfo, N, Self::TileHandle>>,
        GraphTileProviderError,
    >
    where
        Self: Sized,
    {
        let area = SearchArea::new(center, radius_in_meters)?;
        Ok(ResultsWithinRadius::new(self, filter_predicate, area))
    }

    /// Yields an iterator over all edges within a given radius of a point.
    ///
    /// No sorting or filtering of nodes is performed besides ensuring that they are close enough.
    ///
    /// Tiles that are not in the tile set are skipped.
    /// For an edge that leaves the extract,
    /// the edge is reported but its opposing edge (which is in the missing tile) is not.
    ///
    /// # Performance
    ///
    /// This method is designed to give results with as little overhead as possible:
    ///
    /// - The filter predicate applies before any distance checks.
    /// - It only buffers results internally for one tile at a time.
    /// - The iteration order is not guaranteed to follow any particular sorting.
    /// - Internally, an approximator is used to (very!) quickly weed out unrealistic candidates
    ///   before doing the heavier trigonometry.
    ///
    /// This isn't intended to be used to get edges over large distances;
    /// the work grows with the area of the search.
    ///
    /// # Errors
    ///
    /// Fails up front with [`GraphTileProviderError::InvalidSearch`]
    /// if `center` is not a valid WGS84 coordinate
    /// or `radius_in_meters` is negative or not finite.
    ///
    /// The iterator may yield error items if a visited tile could not be loaded
    /// (e.g., a filesystem error) or is corrupt (not a valid Valhalla graph tile).
    #[inline]
    fn edges_within_radius<
        N: GeoFloat + CoordFloat + FromPrimitive + From<f32> + Display,
        F: Fn(&DirectedEdge) -> bool,
    >(
        &self,
        filter_predicate: F,
        center: Point<N>,
        radius_in_meters: N,
    ) -> Result<
        impl Iterator<Item = SearchItem<DirectedEdge, N, Self::TileHandle>>,
        GraphTileProviderError,
    >
    where
        Self: Sized,
    {
        let area = SearchArea::new(center, radius_in_meters)?;

        // TODO: Decide if we want to return LITERALLY every edge or whether we should allow also getting just one of a pair.
        // Fetching opposing edges (current behavior) adds some overhead,
        // but the results are easier to understand.
        Ok(ResultsWithinRadius::new(self, filter_predicate, area))
    }
}

/// Finds the single edge that continues a shortcut walk from a node, if it is uniquely determined.
///
/// `skip_edge_index` is the index (within the tile) of the edge that the walk arrived on.
/// Also reports whether any shortcut edges leave the node.
fn find_continuing_edge(
    tile: &GraphTileView,
    node: &NodeInfo,
    skip_edge_index: u64,
) -> Result<(Option<GraphId>, bool), GraphTileProviderError> {
    let edges = tile.get_outbound_edges_from_node(node);

    let mut candidate: Option<u64> = None; // absolute index within the tile
    let mut saw_shortcut = false;
    for (abs_index, de) in (u64::from(node.edge_index())..).zip(edges) {
        if de.is_shortcut() {
            saw_shortcut = true;
        }
        if abs_index == skip_edge_index || !de.can_form_shortcut() {
            continue;
        }
        if candidate.is_some() {
            // More than one possible continuing edge
            return Ok((None, saw_shortcut));
        }
        candidate = Some(abs_index);
    }

    let cont = candidate
        .map(|idx| tile.header().graph_id().with_feature_index(idx))
        .transpose()?;
    Ok((cont, saw_shortcut))
}

/// A keyed lock.
///
/// This enables more granular locking than over an entire data structure.
pub(crate) struct LockTable<K>(DashMap<K, Arc<Mutex<()>>>);

impl<K: core::hash::Hash + Eq + Clone> LockTable<K> {
    pub fn new() -> Self {
        Self(DashMap::new())
    }

    pub fn lock_for(&self, k: K) -> Arc<Mutex<()>> {
        self.0
            .entry(k)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use crate::GraphId;
    use crate::graph_tile::{GraphTile, LookupError};
    use crate::tile_hierarchy::STANDARD_LEVELS;
    use crate::tile_provider::{
        DirectoryGraphTileProvider, GraphTileProvider, GraphTileProviderError,
        GraphTileProviderErrorKind, InvalidSearchError, TileAvailability,
    };
    use geo::{Destination, HasDimensions, Haversine, LineString, Validation, coord, point};
    use rand::rngs::ThreadRng;
    use rand::{RngExt, rng};
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use zerocopy::IntoBytes;

    fn fixture_provider() -> DirectoryGraphTileProvider {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles");
        DirectoryGraphTileProvider::new(base, NonZeroUsize::new(4).unwrap())
    }

    #[test]
    fn haversine_antimeridian_wraps() {
        // Wrapping from east -> west
        let projected = Haversine.destination(point!(x: 179.9, y: 0.0), 90.0, 50_000.0);
        assert!(projected.x() < -179.0);
        assert!(projected.x() > -180.0);

        // Wrapping the other way
        let projected = Haversine.destination(point!(x: -179.9, y: 0.0), 270.0, 50_000.0);
        assert!(projected.x() > 179.0);
        assert!(projected.x() < 180.0);
    }

    #[test]
    fn error_kinds() {
        let missing = GraphId::try_from_components(2, 1, 0).unwrap();
        let not_available = GraphTileProviderError::TileNotAvailable(missing);
        assert!(not_available.is_not_available());
        assert_eq!(
            not_available.kind(),
            GraphTileProviderErrorKind::NotAvailable
        );

        let lookup = GraphTileProviderError::Lookup(LookupError::InvalidIndex);
        assert!(!lookup.is_not_available());
        assert_eq!(lookup.kind(), GraphTileProviderErrorKind::InvalidData);

        let io = GraphTileProviderError::Io(std::io::Error::other("boom"));
        assert_eq!(io.kind(), GraphTileProviderErrorKind::Io);

        let search = GraphTileProviderError::InvalidSearch(InvalidSearchError::InvalidRadius(-1.0));
        assert_eq!(search.kind(), GraphTileProviderErrorKind::InvalidInput);

        assert_eq!(
            GraphTileProviderError::PoisonedCacheLock.kind(),
            GraphTileProviderErrorKind::Internal
        );
    }

    #[test]
    fn if_available() {
        let missing = GraphId::try_from_components(2, 1, 0).unwrap();
        assert_eq!(
            Ok::<_, GraphTileProviderError>(1).if_available().unwrap(),
            Some(1)
        );
        assert_eq!(
            Err::<u8, _>(GraphTileProviderError::TileNotAvailable(missing))
                .if_available()
                .unwrap(),
            None
        );
        assert!(
            Err::<u8, _>(GraphTileProviderError::Lookup(LookupError::InvalidIndex))
                .if_available()
                .is_err()
        );
    }

    #[test]
    fn missing_tile_is_not_available() {
        let provider = fixture_provider();
        // A tile in the Pacific ocean
        let missing = GraphId::try_from_components(2, 1, 5).unwrap();
        let Err(err) = provider.get_handle_for_tile_containing(missing) else {
            panic!("The tile is not in the fixtures");
        };
        assert!(
            matches!(err, GraphTileProviderError::TileNotAvailable(id) if id == missing.tile_base_id()),
            "Unexpected error: {err:?}"
        );
        assert!(!provider.contains_tile(missing).unwrap());
        assert!(
            provider
                .contains_tile(GraphId::try_from_components(2, 762_485, 3).unwrap())
                .unwrap()
        );
    }

    #[test]
    fn tiles_within_radius_lists_present_tiles() {
        let provider = fixture_provider();
        // Andorra la Vella
        let tiles = provider
            .tiles_within_radius(point!(x: 1.5218, y: 42.5063), 5_000.0)
            .expect("valid search");
        assert!(!tiles.is_empty());
        for tile in &tiles {
            assert!(provider.contains_tile(*tile).unwrap());
        }
        // Every level is represented
        for level in 0..=2 {
            assert!(tiles.iter().any(|t| t.level() == level), "level {level}");
        }

        // Nowhere near the fixtures
        assert_eq!(
            provider
                .tiles_within_radius(point!(x: -150.0, y: -40.0), 5_000.0)
                .unwrap(),
            vec![]
        );
    }

    #[test]
    fn searches_reject_invalid_parameters() {
        let provider = fixture_provider();
        for (center, radius) in [
            (point!(x: f64::NAN, y: 42.5), 25.0),
            (point!(x: 1.5, y: f64::INFINITY), 25.0),
            (point!(x: 1.5, y: 42.5), f64::NAN),
            (point!(x: 1.5, y: 42.5), f64::INFINITY),
            (point!(x: 1.5, y: 42.5), -1.0),
            (point!(x: 200.0, y: 42.5), 25.0),
        ] {
            assert!(matches!(
                provider.nodes_within_radius(|_| true, center, radius),
                Err(GraphTileProviderError::InvalidSearch(_))
            ));
            assert!(matches!(
                provider.edges_within_radius(|_| true, center, radius),
                Err(GraphTileProviderError::InvalidSearch(_))
            ));
            assert!(matches!(
                provider.tiles_within_radius(center, radius),
                Err(GraphTileProviderError::InvalidSearch(_))
            ));
        }
    }

    #[test]
    fn large_radius_does_not_panic() {
        // The distance approximator asserts (in debug builds) that it is only used for short distances.
        // Searches with large radii must still work.
        let provider = fixture_provider();
        let center = point!(x: 1.5218, y: 42.5063);
        let edges = provider
            .edges_within_radius(|_| true, center, 25_000.0)
            .expect("valid search")
            .collect::<Result<Vec<_>, _>>()
            .expect("search succeeds");
        assert!(!edges.is_empty());
        assert!(edges.iter().all(|e| e.approx_distance() <= 25_000.0));
    }

    #[test]
    fn test_nodes_within_radius() {
        let mut rng = rng();

        let provider = fixture_provider();
        let graph_id = GraphId::try_from_components(0, 3015, 0).expect("Unable to create graph ID");
        provider
            .with_tile_containing(graph_id, |tile_view| {
                let sw = tile_view.header().sw_corner();

                for (idx, node) in tile_view.nodes().iter().enumerate().filter(|_| {
                    if cfg!(miri) {
                        rng.random_bool(0.01)
                    } else {
                        true
                    }
                }) {
                    let expected_id = tile_view
                        .header()
                        .graph_id()
                        .with_feature_index(idx as u64)
                        .unwrap();
                    let results = provider
                        .nodes_within_radius(|_| true, node.coordinate(sw).into(), 25.0)
                        .expect("valid search")
                        .collect::<Result<Vec<_>, _>>()
                        .expect("Error searching for nodes");

                    assert!(
                        results.iter().any(|node_ref| {
                            node_ref.graph_id() == expected_id
                                && node_ref.approx_distance() == 0.0
                                && node_ref.node_info().expect("node in tile").as_bytes()
                                    == node.as_bytes()
                        }),
                        "Expected to find a match for the node"
                    );

                    assert!(
                        results
                            .iter()
                            .all(|result| result.approx_distance() <= 25.0),
                        "Expected all nodes to be within the search radius"
                    );
                }
            })
            .expect("Unable to get tile");
    }

    // Helper used by following tests
    fn sanity_check_edges_within_radius<T: GraphTileProvider>(
        provider: &T,
        tile_id: GraphId,
        mut rng: ThreadRng,
    ) {
        provider.with_tile_containing(tile_id, |tile_view| {
            let sw = tile_view.header().sw_corner();

            for (idx, edge) in tile_view
                .directed_edges()
                .iter()
                .enumerate()
                .filter(|(_, edge)| edge.length() > 0 && if cfg!(miri) { rng.random_bool(0.01) } else { true })
            {
                // Skip edges with invalid or empty geometry; the search intentionally skips them too
                let shape_coords: Vec<geo::Coord<f32>> = tile_view
                    .get_edge_info(edge)
                    .expect("Missing edge info")
                    .decode_raw_shape()
                    .expect("Unable to decode shape");
                let ls = LineString::new(shape_coords.clone());
                if !ls.is_valid() || ls.is_empty() {
                    continue;
                }

                // Choose a query point on the edge:
                // 1) Prefer the end node when it is in this tile (fast and guaranteed inside tile)
                // 2) Otherwise, choose a coordinate from the edge geometry that lies inside this tile's bbox
                //    (fall back to the first coordinate if none are inside)
                let query_point = if let Ok(node) = tile_view.get_node(edge.end_node_id()) {
                    node.coordinate(sw).into()
                } else {
                    let level_info = &STANDARD_LEVELS[tile_view.header().graph_id().level() as usize];
                    let tile_size = level_info.tiling_system.tile_size;
                    let ne = coord! { x: sw.x + tile_size, y: sw.y + tile_size };

                    let inside = shape_coords.iter().find(|c| c.x >= sw.x && c.x <= ne.x && c.y >= sw.y && c.y <= ne.y);
                    let pick = inside.copied().unwrap_or_else(|| shape_coords[0]);
                    geo::Point::new(pick.x, pick.y)
                };

                let graph_id = tile_view
                    .header()
                    .graph_id()
                    .with_feature_index(idx as u64)
                    .unwrap();

                let results = provider
                    .edges_within_radius(|_| true, query_point, 25.0)
                    .expect("valid search")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("Error searching for edges");

                // Expect to find this exact edge at near-zero distance (allow tiny numeric error)
                assert!(
                    results.iter().any(|edge_ref| {
                        edge_ref.approx_distance() <= 1.0
                            && edge_ref.graph_id() == graph_id
                            && edge_ref.directed_edge().expect("edge in tile").as_bytes() == edge.as_bytes()
                    }),
                    "Expected to find a match for the edge idx {graph_id}. edge = {edge:?} linestring = {ls:?} query_point = {query_point:?} sw = {sw:?}",
                );

                // All reported edges should be within the requested radius
                assert!(
                    results.iter().all(|result| result.approx_distance() <= 25.0),
                    "Expected all edges to be within the search radius"
                );
            }
        })
        .expect("Unable to get tile");
    }

    #[test]
    fn test_edges_within_radius_level0() {
        let graph_id = GraphId::try_from_components(0, 3015, 0).expect("Unable to create graph ID");
        sanity_check_edges_within_radius(&fixture_provider(), graph_id, rng());
    }

    #[test]
    fn test_edges_within_radius_level2() {
        // Use a known level 2 tile from fixtures to exercise edge bins path
        let graph_id =
            GraphId::try_from_components(2, 762_485, 0).expect("Unable to create graph ID");
        sanity_check_edges_within_radius(&fixture_provider(), graph_id, rng());
    }
}
