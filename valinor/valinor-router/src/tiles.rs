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
//! - A tile directory is read the same way, but only up to [`DIRECTORY_BUILD_ID_SAMPLE`]
//!   headers (each is a file to open), because every directory tile is also checked
//!   when it is first used: one from another build, or without a build ID, is an error
//!   ([`GraphTileProviderError::BuildMismatch`], [`GraphTileProviderError::NoBuildId`]).
//!   That covers tiles that arrive after the sets were opened.
//!
//! Corrupt tile data is a different problem, which only hashing every tile detects
//! (`TarballTileProvider::verify_tileset`); run that once after a download, not on every open.
//!
//! # Tile handles
//!
//! Every search asks for tiles over and over, from every thread at once,
//! so the lookup must not take a lock (a shared LRU's mutex serialized all routing threads).
//! A tarball tile is validated the first time it is asked for, and its handle is kept
//! in a [`OnceLock`] of the tile index, which later lookups read without locking.
//! The handle is a view of the memory map (the tile's bytes stay in the map,
//! paged in and out by the operating system), so keeping one per tile used costs
//! [`size_of::<MmapGraphTileHandle>()`] plus a reference count, about 500 bytes:
//! some 100 MB if a long-running server has touched every tile of a planet,
//! and a few megabytes for a device's regions.
//! A directory tile is memory-mapped from its own file (see [`DirectoryGraphTileProvider`])
//! and kept the same way, so it costs the same plus one mapping.
//!
//! # Tiles that arrive later
//!
//! A phone keeps its tiles as a directory, and adds tile files while the router is open
//! (each written to a temporary file and renamed into place).
//! A tile that isn't in the index is looked for in the directories, in order;
//! one that none of them has is remembered as missing (a dead end, as for a tarball)
//! until [`TileSets::refresh`], which the app calls once new files are in place.
//! Those lookups go through a table of every possible tile ID, allocated in chunks
//! as they are touched, so they don't lock either.
//! Tiles that arrive later are not in [`TileSets::tile_ids`].

use core::fmt;
use core::hash::{BuildHasherDefault, Hasher};
use core::num::NonZeroUsize;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::UNIX_EPOCH;
use thiserror::Error;
use valhalla_graphtile::Access;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::handles::{GraphTileHandle, MmapGraphTileHandle};
use valhalla_graphtile::graph_tile::{
    AccessRestriction, Admin, BuildId, DirectedEdge, DirectedEdgeExt, DiscretizedBoundingCircle,
    EdgeInfo, GraphTile, GraphTileDecodingError, GraphTileHeader, GraphTileView, LookupError,
    NodeInfo, NodeTransition, OpposingEdgeIndex, TileHash, TileSections,
};
use valhalla_graphtile::tile_hierarchy::STANDARD_LEVELS;
use valhalla_graphtile::tile_provider::{
    DirectoryGraphTileProvider, GraphTileProvider, GraphTileProviderError, TarballTileProvider,
};

/// The most tiles of each set whose build ID [`TileSets::open`] reads (see the module docs).
pub const BUILD_ID_SAMPLE: usize = 4096;

/// The most tiles of each directory whose build ID [`TileSets::open`] reads
/// (fewer than for a tarball: each is a file to open, and every directory tile is checked
/// when it is first used anyway).
pub const DIRECTORY_BUILD_ID_SAMPLE: usize = 64;

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

/// A fast hasher for the tile index, which every tile lookup of every search goes through
/// (std's `SipHash` was a sixth of a route's time).
///
/// The keys are tile IDs from the router's own files, not from requests,
/// so the hash needn't resist collisions crafted by clients.
#[derive(Default)]
struct TileIdHasher(u64);

impl Hasher for TileIdHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u64(&mut self, word: u64) {
        // FxHash's step: mix the word in and multiply by an odd constant, so that every bit of
        // the ID reaches the high bits that hashbrown's buckets read.
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
}

/// Where a tile comes from, and its handle once the tile has been validated.
struct Slot {
    set: usize,
    handle: OnceLock<Arc<MmapGraphTileHandle>>,
}

/// A chunk of [`LateTiles`], allocated when first touched.
type LateChunk = OnceLock<Box<[LateSlot]>>;

/// How many tile IDs a chunk of [`LateTiles`] covers.
const LATE_CHUNK: usize = 1024;

/// A tile that wasn't in the index when the sets were opened.
#[derive(Default)]
struct LateSlot {
    /// The tile, once a directory has it.
    handle: OnceLock<Arc<MmapGraphTileHandle>>,
    /// One more than the generation in which no directory had the tile (0: not looked for).
    missing: AtomicU64,
}

/// The tiles that may arrive in directories after the sets were opened,
/// in a table of every tile ID of each level, allocated a chunk at a time.
///
/// A chunk covers 1024 consecutive tile IDs (on level 2, most of a row of 0.25 degree tiles)
/// and costs some 24 kilobytes once touched; the table of chunks is about 25 kilobytes.
struct LateTiles {
    levels: Vec<Box<[LateChunk]>>,
    /// Bumped by [`TileSets::refresh`], which makes missing tiles be looked for again.
    generation: AtomicU64,
}

impl LateTiles {
    fn new() -> Self {
        let levels = STANDARD_LEVELS
            .iter()
            .map(|level| {
                (0..(level.tiling_system.tile_count() as usize).div_ceil(LATE_CHUNK))
                    .map(|_| OnceLock::new())
                    .collect()
            })
            .collect();
        Self {
            levels,
            generation: AtomicU64::new(0),
        }
    }

    /// The slot of a base tile ID (`None` for a level or tile that doesn't exist).
    fn slot(&self, base_id: GraphId) -> Option<&LateSlot> {
        let chunks = self.levels.get(usize::from(base_id.level()))?;
        let tile = usize::try_from(base_id.tile_id()).ok()?;
        let chunk = chunks
            .get(tile / LATE_CHUNK)?
            .get_or_init(|| (0..LATE_CHUNK).map(|_| LateSlot::default()).collect());
        chunk.get(tile % LATE_CHUNK)
    }
}

/// Several tile sets as one [`GraphTileProvider`]; see the [module docs](self).
pub struct TileSets {
    sets: Vec<TileSet>,
    /// The set that supplies each tile (by base ID).
    index: HashMap<GraphId, Slot, BuildHasherDefault<TileIdHasher>>,
    /// Tiles not in the index, if there are directories they may arrive in.
    late: Option<LateTiles>,
    build_id: BuildId,
    last_modified: u32,
}

impl TileSets {
    /// Opens tile sets (`build_extract` tarballs or tile directories) as one provider,
    /// after checking that they come from one build (see the [module docs](self)).
    ///
    /// `directory_cache` is no longer used: directory tiles are mapped and kept
    /// as tarball tiles are.
    ///
    /// # Performance
    ///
    /// A tarball is memory-mapped, and only its index and the sampled headers are read.
    ///
    /// # Errors
    ///
    /// See [`OpenError`].
    pub fn open<P: AsRef<Path>>(
        paths: &[P],
        directory_cache: NonZeroUsize,
    ) -> Result<Self, OpenError> {
        if paths.is_empty() {
            return Err(OpenError::NoTileSets);
        }
        let mut sets = Vec::with_capacity(paths.len());
        let mut index = HashMap::default();
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
                let tiles = DirectoryGraphTileProvider::new(path.to_path_buf(), directory_cache);
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

            let limit = match set {
                TileSet::Tarball(_) => BUILD_ID_SAMPLE,
                TileSet::Directory(_) => DIRECTORY_BUILD_ID_SAMPLE,
            };
            for &tile in sample(&ids, limit) {
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
                    entry.insert(Slot {
                        set: number,
                        handle: OnceLock::new(),
                    });
                }
            }
            last_modified = last_modified.max(modified_seconds(&metadata));
            sets.push(set);
        }
        // Every set has a tile, so the first set's first tile set `first`.
        let build_id = first.map_or(BuildId::default(), |(_, _, build_id)| build_id);
        let late = sets
            .iter()
            .any(|set| matches!(set, TileSet::Directory(_)))
            .then(LateTiles::new);
        Ok(Self {
            sets,
            index,
            late,
            build_id,
            last_modified,
        })
    }

    /// The build every tile comes from.
    #[inline]
    pub fn build_id(&self) -> BuildId {
        self.build_id
    }

    /// The IDs of every tile in the sets when they were opened, in no particular order
    /// (tiles that arrive in a directory later are not listed).
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

    /// Makes tiles that arrived in the directories since the sets were opened
    /// (or since the last refresh) available: tiles that were missing are looked for again.
    ///
    /// Call it once new tile files are in place. It is cheap, and safe while searches run.
    pub fn refresh(&self) {
        if let Some(late) = &self.late {
            late.generation.fetch_add(1, Ordering::AcqRel);
        }
        for set in &self.sets {
            if let TileSet::Directory(tiles) = set {
                tiles.refresh();
            }
        }
    }

    /// The tile's handle from its slot (validated on first use).
    #[inline]
    fn handle(&self, graph_id: GraphId) -> Result<TileRef<'_>, GraphTileProviderError> {
        let base_id = graph_id.tile_base_id();
        // Not `ok_or`: building (and dropping) the error on every hit is measurable here.
        let Some(slot) = self.index.get(&base_id) else {
            return match &self.late {
                Some(late) => self.late_handle(late, base_id).map(TileRef),
                None => Err(GraphTileProviderError::TileNotAvailable(base_id)),
            };
        };
        if let Some(handle) = slot.handle.get() {
            return Ok(TileRef(handle));
        }
        let handle = match self.sets.get(slot.set) {
            Some(TileSet::Tarball(tiles)) => tiles.get_handle_for_tile_containing(base_id)?,
            Some(TileSet::Directory(tiles)) => self.checked(tiles.map_tile(base_id)?)?,
            // `open` only makes slots for the sets it keeps.
            None => return Err(GraphTileProviderError::TileNotAvailable(base_id)),
        };
        // Two threads may validate the same tile at once; the first to finish keeps it.
        Ok(TileRef(slot.handle.get_or_init(|| handle)))
    }

    /// A tile that wasn't in the index: from the first directory that has it now.
    #[cold]
    fn late_handle<'a>(
        &'a self,
        late: &'a LateTiles,
        base_id: GraphId,
    ) -> Result<&'a MmapGraphTileHandle, GraphTileProviderError> {
        let Some(slot) = late.slot(base_id) else {
            return Err(GraphTileProviderError::TileNotAvailable(base_id));
        };
        if let Some(handle) = slot.handle.get() {
            return Ok(handle);
        }
        // Read before looking, so that a refresh during the lookup isn't lost.
        let generation = late.generation.load(Ordering::Acquire);
        if slot.missing.load(Ordering::Acquire) == generation + 1 {
            return Err(GraphTileProviderError::TileNotAvailable(base_id));
        }
        for set in &self.sets {
            if let TileSet::Directory(tiles) = set {
                match tiles.map_tile(base_id) {
                    Ok(handle) => {
                        let handle = self.checked(handle)?;
                        return Ok(slot.handle.get_or_init(|| handle));
                    }
                    Err(GraphTileProviderError::TileNotAvailable(_)) => {}
                    // A corrupt file isn't remembered: replacing it fixes it.
                    Err(e) => return Err(e),
                }
            }
        }
        slot.missing.store(generation + 1, Ordering::Release);
        Err(GraphTileProviderError::TileNotAvailable(base_id))
    }

    /// A directory tile, if it is from the sets' build (see the [module docs](self)).
    fn checked(
        &self,
        tile: Arc<MmapGraphTileHandle>,
    ) -> Result<Arc<MmapGraphTileHandle>, GraphTileProviderError> {
        let header = tile.header();
        let graph_id = header.graph_id().tile_base_id();
        let found = header.build_id();
        if found == BuildId::default() && header.tile_hash() == TileHash::default() {
            return Err(GraphTileProviderError::NoBuildId(graph_id));
        }
        if found != self.build_id {
            return Err(GraphTileProviderError::BuildMismatch {
                graph_id,
                expected: self.build_id.value(),
                found: found.value(),
            });
        }
        Ok(tile)
    }

    /// Whether a tile that wasn't in the index is in a directory now.
    #[cold]
    fn late_contains(
        &self,
        late: &LateTiles,
        base_id: GraphId,
    ) -> Result<bool, GraphTileProviderError> {
        let Some(slot) = late.slot(base_id) else {
            return Ok(false);
        };
        if slot.handle.get().is_some() {
            return Ok(true);
        }
        let generation = late.generation.load(Ordering::Acquire);
        if slot.missing.load(Ordering::Acquire) == generation + 1 {
            return Ok(false);
        }
        for set in &self.sets {
            if let TileSet::Directory(tiles) = set
                && tiles.contains_tile(base_id)?
            {
                return Ok(true);
            }
        }
        slot.missing.store(generation + 1, Ordering::Release);
        Ok(false)
    }
}

/// The tiles to read build IDs from: all of them, or `limit` spread evenly.
fn sample(ids: &[GraphId], limit: usize) -> impl Iterator<Item = &GraphId> {
    let count = ids.len().min(limit);
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

/// A tile from either kind of set (memory-mapped either way), borrowed from the [`TileSets`].
///
/// It is a plain reference, so that copying it (which the searches do for every
/// edge) touches no shared memory: cloning an `Arc` would make every routing thread write
/// the same reference counts of the same hot tiles, which cost more than the searches.
#[derive(Clone, Copy)]
pub struct TileRef<'a>(&'a MmapGraphTileHandle);

impl TileRef<'_> {
    #[inline]
    fn view(&self) -> &GraphTileView<'_> {
        self.0.borrow_dependent()
    }
}

impl GraphTileHandle for TileRef<'_> {
    #[inline]
    fn tile_view(&'_ self) -> GraphTileView<'_> {
        self.view().clone()
    }
}

impl GraphTile for TileRef<'_> {
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

impl<'a> GraphTileProvider for &'a TileSets {
    type TileHandle = TileRef<'a>;

    #[inline]
    fn with_tile_containing<F, T>(
        &self,
        graph_id: GraphId,
        process: F,
    ) -> Result<T, GraphTileProviderError>
    where
        F: FnOnce(&GraphTileView) -> T,
    {
        Ok(process(self.handle(graph_id)?.view()))
    }

    #[inline]
    fn get_handle_for_tile_containing(
        &self,
        graph_id: GraphId,
    ) -> Result<Self::TileHandle, GraphTileProviderError> {
        // Through the `&'a TileSets`, so the handle borrows the sets for `'a`.
        let sets: &'a TileSets = self;
        sets.handle(graph_id)
    }

    #[inline]
    fn contains_tile(&self, graph_id: GraphId) -> Result<bool, GraphTileProviderError> {
        let base_id = graph_id.tile_base_id();
        if self.index.contains_key(&base_id) {
            return Ok(true);
        }
        match &self.late {
            Some(late) => self.late_contains(late, base_id),
            None => Ok(false),
        }
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
    fn kept_tarball_handles_are_small() {
        // The module docs' estimate of what a validated tarball tile costs.
        let size = size_of::<MmapGraphTileHandle>() + size_of::<Slot>() + size_of::<GraphId>();
        assert!(size <= 600, "{size} bytes");
    }

    #[test]
    fn samples_every_tile_of_small_sets() {
        for count in [1, 2, 3, 100, BUILD_ID_SAMPLE as u64] {
            let ids = ids(count);
            assert_eq!(
                sample(&ids, BUILD_ID_SAMPLE).copied().collect::<Vec<_>>(),
                ids,
                "{count}"
            );
        }
        assert_eq!(sample(&[], BUILD_ID_SAMPLE).count(), 0);
    }

    #[test]
    fn samples_large_sets_evenly_from_first_to_last() {
        for count in [BUILD_ID_SAMPLE as u64 + 1, 10_000, 250_000] {
            let ids = ids(count);
            let sampled: Vec<_> = sample(&ids, BUILD_ID_SAMPLE).copied().collect();
            assert_eq!(sampled.len(), BUILD_ID_SAMPLE, "{count}");
            assert_eq!(sampled.first(), ids.first());
            assert_eq!(sampled.last(), ids.last());
            // Distinct and in order.
            assert!(sampled.windows(2).all(|w| w[0] < w[1]), "{count}");
        }
    }

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../valhalla-graphtile/fixtures")
    }

    /// The fixture's tiles, by path.
    const TILES: [&str; 7] = [
        "0/003/015.gph",
        "1/047/701.gph",
        "2/000/762/485.gph",
        "2/000/762/486.gph",
        "2/000/763/925.gph",
        "2/000/763/926.gph",
        "2/000/763/927.gph",
    ];

    fn tile_id(path: &str) -> GraphId {
        let id = |level, tile| GraphId::try_from_components(level, tile, 0).unwrap();
        match path {
            "0/003/015.gph" => id(0, 3015),
            "1/047/701.gph" => id(1, 47_701),
            "2/000/762/485.gph" => id(2, 762_485),
            "2/000/762/486.gph" => id(2, 762_486),
            "2/000/763/925.gph" => id(2, 763_925),
            "2/000/763/926.gph" => id(2, 763_926),
            _ => id(2, 763_927),
        }
    }

    /// A temporary directory, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "valinor-tiles-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// Puts a file in place as an app must: written aside, then renamed into place.
        fn install(&self, relative: &str, bytes: &[u8]) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let temp = path.with_extension("part");
            std::fs::write(&temp, bytes).unwrap();
            std::fs::rename(&temp, &path).unwrap();
        }

        fn with_fixture_tiles(self, tiles: &[&str]) -> Self {
            for tile in tiles {
                self.install(tile, &fixture_tile(tile));
            }
            self
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fixture_tile(path: &str) -> Vec<u8> {
        std::fs::read(fixtures().join("andorra-tiles").join(path)).unwrap()
    }

    fn open(dir: &TempDir) -> TileSets {
        TileSets::open(&[&dir.0], NonZeroUsize::MIN).unwrap()
    }

    fn get(sets: &TileSets, path: &str) -> Result<GraphId, GraphTileProviderError> {
        sets.get_handle_for_tile_containing(tile_id(path))
            .map(|tile| tile.header().graph_id())
    }

    #[test]
    fn tiles_that_arrive_after_open_are_found() {
        let dir = TempDir::new("arrive").with_fixture_tiles(&TILES[..2]);
        let sets = open(&dir);
        let late = TILES[2];
        assert!(matches!(
            get(&sets, late),
            Err(GraphTileProviderError::TileNotAvailable(_))
        ));
        assert!(!(&sets).contains_tile(tile_id(late)).unwrap());

        dir.install(late, &fixture_tile(late));
        // Remembered as missing until a refresh.
        assert!(get(&sets, late).is_err());
        assert!(!(&sets).contains_tile(tile_id(late)).unwrap());
        sets.refresh();
        assert!((&sets).contains_tile(tile_id(late)).unwrap());
        assert_eq!(get(&sets, late).unwrap(), tile_id(late));

        // A tile nobody asked for yet is found without a refresh.
        dir.install(TILES[3], &fixture_tile(TILES[3]));
        assert_eq!(get(&sets, TILES[3]).unwrap(), tile_id(TILES[3]));
        // The tiles listed are those of the open.
        assert_eq!(sets.tile_ids().count(), 2);
    }

    #[test]
    fn late_tiles_from_another_build_are_refused() {
        let dir = TempDir::new("late-build").with_fixture_tiles(&TILES[1..]);
        let sets = open(&dir);
        // The same tile from another build.
        let newer =
            std::fs::read(fixtures().join("andorra-tiles-with-traffic/0/003/015.gph")).unwrap();
        dir.install(TILES[0], &newer);
        assert!(matches!(
            get(&sets, TILES[0]),
            Err(GraphTileProviderError::BuildMismatch { graph_id, expected, found })
                if graph_id == tile_id(TILES[0])
                    && expected == sets.build_id().value()
                    && found != expected
        ));
        // Errors aren't remembered: the right file is used once it is in place.
        dir.install(TILES[0], &fixture_tile(TILES[0]));
        assert_eq!(get(&sets, TILES[0]).unwrap(), tile_id(TILES[0]));
    }

    #[test]
    fn late_tiles_without_a_build_id_are_refused() {
        let dir = TempDir::new("late-old").with_fixture_tiles(&TILES[1..]);
        let sets = open(&dir);
        let mut bytes = fixture_tile(TILES[0]);
        let level0 =
            DirectoryGraphTileProvider::new(fixtures().join("andorra-tiles"), NonZeroUsize::MIN)
                .tile_header(tile_id(TILES[0]))
                .unwrap();
        let checksum = (u64::from(level0.build_id().value()) << 48) | level0.tile_hash().value();
        let position = bytes
            .windows(8)
            .position(|w| w == checksum.to_le_bytes())
            .unwrap();
        bytes[position..position + 8].fill(0);
        dir.install(TILES[0], &bytes);
        assert!(matches!(
            get(&sets, TILES[0]),
            Err(GraphTileProviderError::NoBuildId(id)) if id == tile_id(TILES[0])
        ));
    }

    #[test]
    fn corrupt_tiles_are_errors() {
        let dir = TempDir::new("corrupt").with_fixture_tiles(&TILES[1..]);
        let sets = open(&dir);
        let tile = fixture_tile(TILES[0]);
        for length in [0, 10, 300, tile.len() / 2, tile.len() - 1] {
            dir.install(TILES[0], &tile[..length]);
            assert!(
                matches!(
                    get(&sets, TILES[0]),
                    Err(GraphTileProviderError::InvalidTile { .. })
                ),
                "{length} bytes"
            );
        }
        // A tile listed at open that is truncated later (by rename) is an error too.
        dir.install(TILES[1], &fixture_tile(TILES[1])[..1000]);
        assert!(get(&sets, TILES[1]).is_err());
    }

    #[test]
    fn mapped_tiles_outlive_their_files() {
        let dir = TempDir::new("outlive").with_fixture_tiles(&TILES);
        let sets = open(&dir);
        let tile = (&sets)
            .get_handle_for_tile_containing(tile_id(TILES[4]))
            .unwrap();
        let edges = tile.directed_edges().len();
        // Replaced by another tile's bytes, then deleted: the mapping is of the old file.
        dir.install(TILES[4], &fixture_tile(TILES[5]));
        std::fs::remove_file(dir.0.join(TILES[4])).unwrap();
        sets.refresh();
        let again = (&sets)
            .get_handle_for_tile_containing(tile_id(TILES[4]))
            .unwrap();
        assert_eq!(again.directed_edges().len(), edges);
        for edge in tile.directed_edges() {
            tile.get_edge_info(edge).unwrap();
        }
    }

    #[test]
    fn threads_read_while_tiles_arrive() {
        let dir = TempDir::new("threads").with_fixture_tiles(&TILES[..1]);
        let sets = open(&dir);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..500 {
                        for path in TILES {
                            match get(&sets, path) {
                                Ok(id) => assert_eq!(id, tile_id(path)),
                                Err(GraphTileProviderError::TileNotAvailable(_)) => {}
                                Err(e) => panic!("{e}"),
                            }
                        }
                    }
                });
            }
            for path in &TILES[1..] {
                dir.install(path, &fixture_tile(path));
                sets.refresh();
            }
        });
        sets.refresh();
        for path in TILES {
            assert_eq!(get(&sets, path).unwrap(), tile_id(path));
        }
    }

    #[test]
    fn tarballs_have_no_late_tiles() {
        let sets =
            TileSets::open(&[fixtures().join("andorra-tiles.tar")], NonZeroUsize::MIN).unwrap();
        assert!(sets.late.is_none());
        let elsewhere = GraphId::try_from_components(2, 1, 0).unwrap();
        assert!(matches!(
            (&sets).get_handle_for_tile_containing(elsewhere),
            Err(GraphTileProviderError::TileNotAvailable(_))
        ));
        assert!(!(&sets).contains_tile(elsewhere).unwrap());
    }
}
