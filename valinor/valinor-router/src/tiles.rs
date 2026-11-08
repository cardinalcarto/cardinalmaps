//! One tile provider over several tile sets, and the check that they come from one build.
//!
//! A device opens all of its offline regions as one router (see `docs/flutter-api.md`),
//! and each region is a mainline `build_extract` tarball (or a tile directory)
//! cut from the same planet build.
//! Regions overlap where they meet, so a tile may be in more than one set;
//! since the sets come from one build, those copies are the same tile,
//! and the first set that has a tile supplies it.
//!
//! # The build check
//!
//! Every tile header carries the ID of the build it came from
//! (the top 16 bits of its `checksum`, see [`BuildId`]).
//! [`TileSets::open`] reads the headers of a sample of each set's tiles
//! and refuses the sets unless every sampled tile has the same build ID.
//!
//! - Each set's tiles are sorted by ID, and up to [`BUILD_ID_SAMPLE`] of them are read,
//!   evenly spaced from the first to the last.
//!   A region extract has a few hundred to a few thousand tiles, so all of them are read;
//!   a planet extract has a few hundred thousand, and reading every header
//!   would page in one page per tile (seconds from a cold disk), so a sample is enough:
//!   what the check guards against is regions from different builds,
//!   and a `build_extract` tarball holds one build.
//! - Only headers are read (one page per tile), not whole tiles.
//! - Tiles without a checksum (built before Valhalla 3.9) can't be checked, so they are refused.
//!
//! Corrupt tile data is a different problem, which only hashing every tile detects
//! (`TarballTileProvider::verify_tileset`); run that once after a download, not on every open.

use core::fmt;
use core::num::NonZeroUsize;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use thiserror::Error;
use valhalla_graphtile::Access;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::handles::{
    GraphTileHandle, MmapGraphTileHandle, OwnedGraphTileHandle,
};
use valhalla_graphtile::graph_tile::{
    AccessRestriction, Admin, BuildId, DirectedEdge, DirectedEdgeExt, DiscretizedBoundingCircle,
    EdgeInfo, GraphTile, GraphTileDecodingError, GraphTileHeader, GraphTileView, LookupError,
    NodeInfo, NodeTransition, OpposingEdgeIndex, TileHash, TileSections,
};
use valhalla_graphtile::tile_provider::{
    CachingTileProvider, DirectoryGraphTileProvider, GraphTileProvider, GraphTileProviderError,
    TarballTileProvider,
};

/// The most tiles of each set whose build ID [`TileSets::open`] reads (see the module docs).
pub const BUILD_ID_SAMPLE: usize = 4096;

/// How many tiles a directory set's own cache keeps.
///
/// The router's [`CachingTileProvider`] caches every set's tiles in front of it,
/// so the directory provider's cache would only hold second copies of the same tiles.
const DIRECTORY_CACHE: NonZeroUsize = NonZeroUsize::MIN;

/// Why tile sets couldn't be opened.
#[derive(Debug, Error)]
pub enum OpenError {
    /// No tile sets were given.
    #[error("open tile sets: none were given")]
    NoTileSets,
    /// A tile set path can't be read (it may not exist).
    #[error("open {path}: {source}")]
    Io {
        /// The tile set.
        path: PathBuf,
        /// What failed.
        #[source]
        source: std::io::Error,
    },
    /// A tile set's tarball, index, directory, or tile headers can't be read.
    #[error("open {path}: {source}")]
    Tiles {
        /// The tile set.
        path: PathBuf,
        /// What failed.
        #[source]
        source: GraphTileProviderError,
    },
    /// A tile set has no tiles.
    #[error("open {path}: it has no tiles")]
    Empty {
        /// The tile set.
        path: PathBuf,
    },
    /// A tile has no checksum (it was built before Valhalla 3.9), so its build can't be known.
    #[error("open {path}: tile {tile} has no build ID (it was built before Valhalla 3.9)")]
    NoBuildId {
        /// The tile set.
        path: PathBuf,
        /// The tile.
        tile: GraphId,
    },
    /// Tiles come from different builds.
    #[error(
        "open {path}: tile {tile} is from build {found}, but tile {first_tile} of {first_path} \
         is from build {expected}; every tile set must come from the same build"
    )]
    MixedBuilds {
        /// The set of the first tile that was read.
        first_path: PathBuf,
        /// The first tile that was read.
        first_tile: GraphId,
        /// Its build.
        expected: Build,
        /// The set of the tile from another build.
        path: PathBuf,
        /// The tile from another build.
        tile: GraphId,
        /// Its build.
        found: Build,
    },
}

/// A tileset build ID, for messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Build(pub BuildId);

impl fmt::Display for Build {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.value())
    }
}

/// One tile set: a tarball extract or a tile directory.
enum TileSet {
    Tarball(TarballTileProvider<false, false>),
    Directory(DirectoryGraphTileProvider),
}

impl TileSet {
    fn header(&self, id: GraphId) -> Result<GraphTileHeader, GraphTileProviderError> {
        match self {
            Self::Tarball(tiles) => tiles.tile_header(id),
            Self::Directory(tiles) => tiles.tile_header(id),
        }
    }
}

/// Several tile sets as one [`GraphTileProvider`]; see the [module docs](self).
pub struct TileSets {
    sets: Vec<TileSet>,
    /// The set that supplies each tile (by base ID).
    index: HashMap<GraphId, usize>,
    build_id: BuildId,
    last_modified: u32,
}

impl TileSets {
    /// Opens tile sets (`build_extract` tarballs or tile directories) as one provider,
    /// after checking that they come from one build (see the [module docs](self)).
    ///
    /// # Errors
    ///
    /// See [`OpenError`].
    pub fn open<P: AsRef<Path>>(paths: &[P]) -> Result<Self, OpenError> {
        if paths.is_empty() {
            return Err(OpenError::NoTileSets);
        }
        let mut sets = Vec::with_capacity(paths.len());
        let mut index = HashMap::new();
        let mut first: Option<(&Path, GraphId, BuildId)> = None;
        let mut last_modified = 0;
        for (number, path) in paths.iter().enumerate() {
            let path = path.as_ref();
            let tiles_error = |source| OpenError::Tiles {
                path: path.to_path_buf(),
                source,
            };
            let metadata = std::fs::metadata(path).map_err(|source| OpenError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let (set, mut ids) = if metadata.is_dir() {
                let tiles = DirectoryGraphTileProvider::new(path.to_path_buf(), DIRECTORY_CACHE);
                let ids = tiles.tile_ids().map_err(tiles_error)?;
                (TileSet::Directory(tiles), ids)
            } else {
                let tiles = TarballTileProvider::new_readonly(path).map_err(tiles_error)?;
                let ids: Vec<GraphId> = tiles.tile_ids().copied().collect();
                (TileSet::Tarball(tiles), ids)
            };
            if ids.is_empty() {
                return Err(OpenError::Empty {
                    path: path.to_path_buf(),
                });
            }
            ids.sort_unstable();

            for &tile in sample(&ids) {
                let header = set.header(tile).map_err(tiles_error)?;
                let build_id = header.build_id();
                if build_id == BuildId::default() && header.tile_hash() == TileHash::default() {
                    return Err(OpenError::NoBuildId {
                        path: path.to_path_buf(),
                        tile,
                    });
                }
                match &first {
                    None => first = Some((path, tile, build_id)),
                    Some((_, _, expected)) if *expected == build_id => {}
                    Some((first_path, first_tile, expected)) => {
                        return Err(OpenError::MixedBuilds {
                            first_path: first_path.to_path_buf(),
                            first_tile: *first_tile,
                            expected: Build(*expected),
                            path: path.to_path_buf(),
                            tile,
                            found: Build(build_id),
                        });
                    }
                }
            }

            for id in ids {
                if let Entry::Vacant(entry) = index.entry(id) {
                    entry.insert(number);
                }
            }
            last_modified = last_modified.max(modified_seconds(&metadata));
            sets.push(set);
        }
        // Every set has a tile, so the first set's first tile set `first`.
        let build_id = first.map_or(BuildId::default(), |(_, _, build_id)| build_id);
        Ok(Self {
            sets,
            index,
            build_id,
            last_modified,
        })
    }

    /// The build every tile comes from.
    #[inline]
    pub fn build_id(&self) -> BuildId {
        self.build_id
    }

    /// The IDs of every tile in the sets, in no particular order.
    #[inline]
    pub fn tile_ids(&self) -> impl Iterator<Item = GraphId> + '_ {
        self.index.keys().copied()
    }

    /// When the newest set was last modified, in seconds since the Unix epoch
    /// (mainline's `tileset_last_modified`, which it reads from its one tile extract or directory).
    #[inline]
    pub fn last_modified(&self) -> u32 {
        self.last_modified
    }

    fn set_of(&self, graph_id: GraphId) -> Result<&TileSet, GraphTileProviderError> {
        let base_id = graph_id.tile_base_id();
        self.index
            .get(&base_id)
            .and_then(|&number| self.sets.get(number))
            .ok_or(GraphTileProviderError::TileNotAvailable(base_id))
    }
}

/// The tiles to read build IDs from: all of them, or [`BUILD_ID_SAMPLE`] spread evenly.
fn sample(ids: &[GraphId]) -> impl Iterator<Item = &GraphId> {
    let count = ids.len().min(BUILD_ID_SAMPLE);
    let last = ids.len().saturating_sub(1);
    let steps = count.saturating_sub(1).max(1);
    // From the first tile (i = 0) to the last (i = count - 1), without repeats
    // (the step between samples is at least 1 when there are more tiles than samples).
    (0..count).filter_map(move |i| ids.get(i * last / steps))
}

/// A file's modification time in seconds since the epoch (0 if it is unknown).
fn modified_seconds(metadata: &std::fs::Metadata) -> u32 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| u32::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// A tile from either kind of set.
#[derive(Clone)]
pub enum TileHandle {
    /// A tile in a memory-mapped tarball.
    Mmap(Arc<MmapGraphTileHandle>),
    /// A tile read from a file of a tile directory.
    Owned(Arc<OwnedGraphTileHandle>),
}

impl TileHandle {
    #[inline]
    fn view(&self) -> &GraphTileView<'_> {
        match self {
            Self::Mmap(handle) => handle.borrow_dependent(),
            Self::Owned(handle) => handle.borrow_dependent(),
        }
    }
}

impl GraphTileHandle for TileHandle {
    #[inline]
    fn tile_view(&'_ self) -> GraphTileView<'_> {
        self.view().clone()
    }
}

impl GraphTile for TileHandle {
    #[inline]
    fn graph_id(&self) -> GraphId {
        self.view().graph_id()
    }

    #[inline]
    fn may_contain_id(&self, id: GraphId) -> bool {
        self.view().may_contain_id(id)
    }

    #[inline]
    fn header(&self) -> &GraphTileHeader {
        self.view().header()
    }

    #[inline]
    fn get_node(&self, id: GraphId) -> Result<&NodeInfo, LookupError> {
        self.view().get_node(id)
    }

    #[inline]
    fn get_outbound_edges_from_node(&self, node_info: &NodeInfo) -> &[DirectedEdge] {
        self.view().get_outbound_edges_from_node(node_info)
    }

    #[inline]
    fn get_transitions(&self, node: &NodeInfo) -> &[NodeTransition] {
        self.view().get_transitions(node)
    }

    #[inline]
    fn get_directed_edge(&self, id: GraphId) -> Result<&DirectedEdge, LookupError> {
        self.view().get_directed_edge(id)
    }

    #[inline]
    fn get_opp_edge_index(&self, graph_id: GraphId) -> Result<OpposingEdgeIndex, LookupError> {
        self.view().get_opp_edge_index(graph_id)
    }

    #[inline]
    fn get_ext_directed_edge(&self, id: GraphId) -> Result<&DirectedEdgeExt, LookupError> {
        self.view().get_ext_directed_edge(id)
    }

    #[inline]
    fn get_access_restrictions(
        &self,
        directed_edge_index: u32,
        access_modes: enumset::EnumSet<Access>,
    ) -> Vec<&AccessRestriction> {
        self.view()
            .get_access_restrictions(directed_edge_index, access_modes)
    }

    #[inline]
    fn edge_access_restrictions(&self, directed_edge_index: u32) -> &[AccessRestriction] {
        self.view().edge_access_restrictions(directed_edge_index)
    }

    #[inline]
    fn get_predicted_speed(
        &self,
        directed_edge_index: usize,
        seconds_from_start_of_week: u32,
    ) -> Option<f32> {
        self.view()
            .get_predicted_speed(directed_edge_index, seconds_from_start_of_week)
    }

    #[inline]
    fn get_edge_info(
        &self,
        directed_edge: &DirectedEdge,
    ) -> Result<EdgeInfo<'_>, GraphTileDecodingError> {
        self.view().get_edge_info(directed_edge)
    }

    #[inline]
    fn directed_edges(&self) -> &[DirectedEdge] {
        self.view().directed_edges()
    }

    #[inline]
    fn nodes(&self) -> &[NodeInfo] {
        self.view().nodes()
    }

    #[inline]
    fn admins(&self) -> &[Admin] {
        self.view().admins()
    }

    #[inline]
    fn edges_in_bin(&self, bin_index: usize) -> &[GraphId] {
        self.view().edges_in_bin(bin_index)
    }

    #[inline]
    fn bounding_circles_in_bin(&self, bin_index: usize) -> &[DiscretizedBoundingCircle] {
        self.view().bounding_circles_in_bin(bin_index)
    }

    #[inline]
    fn bin_index_xy(&self, x: usize, y: usize) -> Option<usize> {
        self.view().bin_index_xy(x, y)
    }

    #[inline]
    fn sections(&self) -> TileSections<'_> {
        self.view().sections()
    }
}

impl GraphTileProvider for TileSets {
    type TileHandle = TileHandle;

    #[inline]
    fn with_tile_containing<F, T>(
        &self,
        graph_id: GraphId,
        process: F,
    ) -> Result<T, GraphTileProviderError>
    where
        F: FnOnce(&GraphTileView) -> T,
    {
        match self.set_of(graph_id)? {
            TileSet::Tarball(tiles) => tiles.with_tile_containing(graph_id, process),
            TileSet::Directory(tiles) => tiles.with_tile_containing(graph_id, process),
        }
    }

    #[inline]
    fn get_handle_for_tile_containing(
        &self,
        graph_id: GraphId,
    ) -> Result<Self::TileHandle, GraphTileProviderError> {
        match self.set_of(graph_id)? {
            TileSet::Tarball(tiles) => tiles
                .get_handle_for_tile_containing(graph_id)
                .map(TileHandle::Mmap),
            TileSet::Directory(tiles) => tiles
                .get_handle_for_tile_containing(graph_id)
                .map(TileHandle::Owned),
        }
    }

    #[inline]
    fn contains_tile(&self, graph_id: GraphId) -> Result<bool, GraphTileProviderError> {
        Ok(self.index.contains_key(&graph_id.tile_base_id()))
    }
}

/// The router's tiles, shared by Loki and Thor: one cache in front of every set.
#[derive(Clone)]
pub struct SharedTiles(Arc<CachingTileProvider<TileSets>>);

impl SharedTiles {
    pub(crate) fn new(sets: TileSets, cache_size: NonZeroUsize) -> Self {
        Self(Arc::new(CachingTileProvider::new(sets, cache_size)))
    }

    /// The tile sets.
    #[inline]
    pub fn sets(&self) -> &TileSets {
        self.0.inner()
    }
}

impl GraphTileProvider for SharedTiles {
    type TileHandle = TileHandle;

    #[inline]
    fn with_tile_containing<F, T>(
        &self,
        graph_id: GraphId,
        process: F,
    ) -> Result<T, GraphTileProviderError>
    where
        F: FnOnce(&GraphTileView) -> T,
    {
        self.0.with_tile_containing(graph_id, process)
    }

    #[inline]
    fn get_handle_for_tile_containing(
        &self,
        graph_id: GraphId,
    ) -> Result<Self::TileHandle, GraphTileProviderError> {
        self.0.get_handle_for_tile_containing(graph_id)
    }

    #[inline]
    fn contains_tile(&self, graph_id: GraphId) -> Result<bool, GraphTileProviderError> {
        self.0.contains_tile(graph_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(count: u64) -> Vec<GraphId> {
        (0..count)
            .map(|tile| GraphId::try_from_components(2, tile, 0).unwrap())
            .collect()
    }

    #[test]
    fn samples_every_tile_of_small_sets() {
        for count in [1, 2, 3, 100, BUILD_ID_SAMPLE as u64] {
            let ids = ids(count);
            assert_eq!(sample(&ids).copied().collect::<Vec<_>>(), ids, "{count}");
        }
        assert_eq!(sample(&[]).count(), 0);
    }

    #[test]
    fn samples_large_sets_evenly_from_first_to_last() {
        for count in [BUILD_ID_SAMPLE as u64 + 1, 10_000, 250_000] {
            let ids = ids(count);
            let sampled: Vec<_> = sample(&ids).copied().collect();
            assert_eq!(sampled.len(), BUILD_ID_SAMPLE, "{count}");
            assert_eq!(sampled.first(), ids.first());
            assert_eq!(sampled.last(), ids.last());
            // Distinct and in order.
            assert!(sampled.windows(2).all(|w| w[0] < w[1]), "{count}");
        }
    }
}
