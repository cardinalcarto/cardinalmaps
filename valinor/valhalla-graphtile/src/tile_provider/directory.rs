//! # Directory tile provider
//!
//! Reads graph tiles from a directory tree of tile files,
//! laid out the way Valhalla writes them (`mjolnir.tile_dir`):
//! `<root>/<level>/nnn/nnn/nnn.gph`.
//!
//! This is what a phone keeps its tiles in: files arrive one at a time
//! (an offline download, or tiles fetched for a route) while the router is open.
//!
//! # Tiles are memory-mapped
//!
//! Each tile file is mapped on its own, as a tarball is mapped as a whole,
//! so the tile's bytes are file-backed pages that the operating system pages in
//! when a search touches them and drops again under memory pressure.
//! Reading tiles into memory would instead make every byte of every used tile
//! anonymous memory (a city's local tile is tens of megabytes), which is what gets
//! an app killed for memory.
//! A mapped tile costs one mapping (a VMA) and its handle (about 500 bytes),
//! plus whatever pages are resident.
//!
//! # Writers
//!
//! A tile file must never be written in place or truncated while it may be mapped:
//! a mapped page past the new end of a truncated file faults (`SIGBUS`).
//! Write a new tile to a temporary file and rename it over the tile's path
//! (as [`DirectoryGraphTileProvider::overwrite_tile`] does).
//! A rename doesn't touch the old file, so mappings of it stay valid
//! (the old file lives on until its last mapping goes),
//! and the next mapping of the path sees the new file.
//!
//! # Tiles that appear later
//!
//! A tile without a file is [`GraphTileProviderError::TileNotAvailable`].
//! The provider remembers that a tile was missing (so a search that keeps reaching the
//! edge of the tiles doesn't keep asking the file system) until
//! [`DirectoryGraphTileProvider::refresh`] is called, which an app calls once new tile files
//! are in place.

use crate::GraphId;
use crate::graph_id::InvalidGraphIdError;
use crate::graph_tile::handles::{MmapGraphTileHandle, MmapTilePointer, TileOffset};
use crate::graph_tile::{
    GraphTile, GraphTileBuildError, GraphTileBuilder, GraphTileDecodingError, GraphTileHeader,
    GraphTileView,
};
use crate::tile_provider::{GraphTileProvider, GraphTileProviderError, LockTable};
use core::hash::{Hash, Hasher};
use core::num::NonZeroUsize;
use lru::LruCache;
use memmap2::MmapOptions;
use std::collections::hash_map::DefaultHasher;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::{fs::File, io::BufWriter};
use zerocopy::FromBytes;

/// The most shards the tile cache is split into (each has its own lock).
const MAX_SHARDS: usize = 16;

/// What the cache knows about a tile.
#[derive(Clone)]
enum Cached {
    /// The tile's file, mapped and validated.
    Tile(Arc<MmapGraphTileHandle>),
    /// The tile had no file in this generation (see [`DirectoryGraphTileProvider::refresh`]).
    Missing(u64),
}

/// A graph tile provider that is backed by a directory of tiles;
/// see the [module docs](self).
///
/// # Resource consumption
///
/// Up to `num_cached_tiles` tiles stay mapped in a cache (split into shards,
/// each with its own lock, so threads rarely wait for each other).
/// A handle that was given out keeps its tile mapped after the cache lets it go.
///
/// # Missing tiles
///
/// A tile without a file is reported as [`GraphTileProviderError::TileNotAvailable`].
/// Tile files are checked to contain the tile their path names.
pub struct DirectoryGraphTileProvider {
    base_directory: PathBuf,
    /// Held while a tile is loaded or overwritten,
    /// so that a load racing an overwrite can't cache the old file.
    lock_table: LockTable<GraphId>,
    shards: Box<[Mutex<LruCache<GraphId, Cached>>]>,
    /// Bumped by [`Self::refresh`]: a tile found missing in an older generation is looked for again.
    generation: AtomicU64,
}

impl DirectoryGraphTileProvider {
    /// Creates a provider for the tiles in `base_directory`,
    /// keeping up to `num_cached_tiles` tiles mapped.
    #[inline]
    pub fn new(base_directory: PathBuf, num_cached_tiles: NonZeroUsize) -> Self {
        let shard_count = num_cached_tiles.get().min(MAX_SHARDS);
        let per_shard = NonZeroUsize::new(num_cached_tiles.get().div_ceil(shard_count))
            .unwrap_or(NonZeroUsize::MIN);
        DirectoryGraphTileProvider {
            base_directory,
            lock_table: LockTable::new(),
            shards: (0..shard_count)
                .map(|_| Mutex::new(LruCache::new(per_shard)))
                .collect(),
            generation: AtomicU64::new(0),
        }
    }

    /// The directory the tiles are in.
    #[inline]
    pub fn base_directory(&self) -> &Path {
        &self.base_directory
    }

    /// Forgets which tiles were missing, so that tile files added since are found.
    ///
    /// Call this after new tile files are in place (renamed to their paths).
    /// It is cheap: tiles that are mapped stay mapped,
    /// and missing tiles are looked for again the next time they are asked for.
    #[inline]
    pub fn refresh(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// The cache shard for a tile.
    fn shard(&self, graph_id: GraphId) -> &Mutex<LruCache<GraphId, Cached>> {
        let mut hasher = DefaultHasher::new();
        graph_id.hash(&mut hasher);
        // The remainder is less than the shard count, so it fits in a usize.
        let index = usize::try_from(hasher.finish() % self.shards.len() as u64).unwrap_or(0);
        &self.shards[index]
    }

    /// The cached state of a tile, if the cache knows it.
    fn cached(&self, base_graph_id: GraphId) -> Result<Option<Cached>, GraphTileProviderError> {
        let mut shard = self
            .shard(base_graph_id)
            .lock()
            .map_err(|_| GraphTileProviderError::PoisonedCacheLock)?;
        Ok(match shard.get(&base_graph_id) {
            Some(Cached::Missing(generation))
                if *generation != self.generation.load(Ordering::Acquire) =>
            {
                shard.pop(&base_graph_id);
                None
            }
            cached => cached.cloned(),
        })
    }

    fn cache(&self, base_graph_id: GraphId, value: Cached) -> Result<(), GraphTileProviderError> {
        self.shard(base_graph_id)
            .lock()
            .map_err(|_| GraphTileProviderError::PoisonedCacheLock)?
            .put(base_graph_id, value);
        Ok(())
    }

    /// Maps and validates a tile's file, bypassing (and not filling) the cache.
    ///
    /// The handle keeps the file mapped until it is dropped,
    /// even if the file is renamed over or deleted.
    ///
    /// # Errors
    ///
    /// [`GraphTileProviderError::TileNotAvailable`] if the tile has no file,
    /// [`GraphTileProviderError::Io`] if it can't be read or mapped,
    /// [`GraphTileProviderError::InvalidTile`] if it isn't a valid tile
    /// (for example, a truncated file),
    /// and [`GraphTileProviderError::TileIdMismatch`] if it is another tile.
    pub fn map_tile(
        &self,
        graph_id: GraphId,
    ) -> Result<Arc<MmapGraphTileHandle>, GraphTileProviderError> {
        let base_graph_id = graph_id.tile_base_id();
        let invalid = |source| GraphTileProviderError::InvalidTile {
            graph_id: base_graph_id,
            source,
        };
        let path = self.path_for_graph_id(base_graph_id)?;
        let file = File::open(path).map_err(|e| match e.kind() {
            ErrorKind::NotFound => GraphTileProviderError::TileNotAvailable(base_graph_id),
            _ => GraphTileProviderError::Io(e),
        })?;
        if !file.metadata()?.is_file() {
            return Err(GraphTileProviderError::TileNotAvailable(base_graph_id));
        }
        // The map is as long as the file is when it is mapped
        // (memmap2 reads the length from the open file), so every byte of it is backed.
        let mmap = MmapOptions::new().map_raw_read_only(&file)?;
        // Tile sizes are 32 bits (as in a tarball's index); an empty file can't be a tile.
        let size =
            u32::try_from(mmap.len()).map_err(|_| invalid(GraphTileDecodingError::SliceLength))?;
        if (size as usize) < size_of::<GraphTileHeader>() {
            return Err(invalid(GraphTileDecodingError::SliceLength));
        }
        let pointer = MmapTilePointer {
            mmap: Arc::new(mmap),
            offsets: TileOffset { offset: 0, size },
        };
        // SAFETY: The range is the whole map. Tile files are never written in place or
        // truncated, only replaced by a rename (the contract in the module docs),
        // and a rename leaves the mapped file as it is.
        let handle = unsafe { MmapGraphTileHandle::try_from(pointer) }.map_err(invalid)?;
        // A tile file in the wrong place would otherwise be silently used as another tile.
        let actual = handle.header().graph_id().tile_base_id();
        if actual != base_graph_id {
            return Err(GraphTileProviderError::TileIdMismatch {
                expected: base_graph_id,
                actual,
            });
        }
        Ok(Arc::new(handle))
    }

    /// Replaces the tile stored at the designated location
    /// with the tile produced by this builder.
    ///
    /// # Data race safety
    ///
    /// The implementation takes the following steps to ensure data race safety:
    ///
    /// - Acquires the tile's lock at the start of the call, so loads of this tile
    ///   through [`GraphTileProvider::get_handle_for_tile_containing`]
    ///   wait until this method exits.
    /// - Writes to a temporary file initially, so if anything goes wrong, the original file is not affected.
    ///   After successfully writing the temp file, it is atomically renamed on top of the original
    ///   (so mappings of the original stay valid; see the [module docs](self)).
    /// - Removes the tile from the internal cache (local to _this_ [`DirectoryGraphTileProvider`] instance!)
    ///   once the rename succeeds.
    /// - Releases the lock only after everything succeeds,
    ///   or fails mid-write (in which case, only the temporary file is affected).
    ///
    /// ## Edge cases
    ///
    /// Once you have a handle to a graph tile from [`GraphTileProvider::get_handle_for_tile_containing`],
    /// your handle will _not_ change or block writers: it keeps the old file mapped.
    /// So, if your workload involves both reads and writes,
    /// **you _must_ ensure that your tile references cannot be stale.**
    /// This probably means some sort of partition mutex in your processing loop.
    ///
    /// Additionally, if the write operation fails, your graph tile builder is gone.
    /// The original file on disk is not in a dirty state, but to retry again,
    /// the caller would need to reconstruct the same builder from scratch.
    ///
    /// # Errors
    ///
    /// This function may fail in a variety of cases that are returned as errors:
    ///
    /// - The tile was explicitly constructed with an invalid graph ID
    /// - The graph tile builder is invalid (see [`GraphTileBuilder::into_byte_iter`] for details)
    /// - Writing the temporary file fails
    /// - The atomic file rename fails
    /// - The internal tile cache lock is corrupted (this should never happen; if it does, it's an error in Valinor)
    pub fn overwrite_tile(
        &self,
        graph_tile_builder: GraphTileBuilder<'_>,
    ) -> Result<(), GraphTileBuildError> {
        let graph_id = graph_tile_builder.graph_id();

        let lock = self.lock_table.lock_for(graph_id);
        // The lock guards no data, so a panic while holding it can't leave anything inconsistent.
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);

        let path = self.path_for_graph_id(graph_id)?;

        // Write to a temp file
        let tmp_path = path.with_extension("tmp");
        let file = File::create(&tmp_path)?;
        let mut writer = BufWriter::new(file);

        for section in graph_tile_builder.into_byte_iter()? {
            writer.write_all(&section)?;
        }

        writer.flush()?;

        // Replace the original file atomically
        std::fs::rename(tmp_path, path)?;

        // Invalidate the cache
        self.shard(graph_id)
            .lock()
            .map_err(|e| GraphTileBuildError::PoisonedCacheLock(e.to_string()))?
            .pop(&graph_id);

        Ok(())
    }

    /// The IDs of the tiles in the directory, sorted.
    ///
    /// A tile is a file at the path [`GraphId::file_path`] gives its ID
    /// (`<level>/<digits>/.../<digits>.gph`).
    /// Other files are ignored, and so are symbolic links to directories,
    /// so that a link cycle can't make the walk go on forever.
    ///
    /// # Performance
    ///
    /// This reads every directory in the tree, but opens no tiles.
    ///
    /// # Errors
    ///
    /// Fails if a directory in the tree can't be read.
    pub fn tile_ids(&self) -> Result<Vec<GraphId>, GraphTileProviderError> {
        let mut ids = Vec::new();
        let mut stack = vec![self.base_directory.clone()];
        while let Some(directory) = stack.pop() {
            for entry in std::fs::read_dir(&directory)? {
                let entry = entry?;
                let path = entry.path();
                // `file_type` doesn't follow links; `metadata` does (for links to tiles).
                if entry.file_type()?.is_dir() {
                    stack.push(path);
                } else if path.is_file()
                    && let Some(id) = self.tile_id_of_path(&path)
                {
                    ids.push(id);
                }
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    /// The ID of the tile at `path`, if `path` is exactly where that tile's file belongs.
    fn tile_id_of_path(&self, path: &Path) -> Option<GraphId> {
        let relative = path.strip_prefix(&self.base_directory).ok()?;
        if relative.extension()? != "gph" {
            return None;
        }
        let stem = relative.with_extension("");
        let mut components = stem.iter();
        let level = components.next()?.to_str()?.parse::<u8>().ok()?;
        let mut digits = String::new();
        for component in components {
            digits.push_str(component.to_str()?);
        }
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let id = GraphId::try_from_components(level, digits.parse().ok()?, 0).ok()?;
        // Only the canonical path (the right number of digits and directories) names the tile.
        (id.file_path("gph").ok()? == relative).then_some(id)
    }

    /// The header of a tile (for example, to read its build ID),
    /// read without the rest of the tile.
    ///
    /// This bypasses (and doesn't fill) the cache.
    ///
    /// # Errors
    ///
    /// [`GraphTileProviderError::TileNotAvailable`] if the tile has no file,
    /// [`GraphTileProviderError::Io`] if it can't be read,
    /// [`GraphTileProviderError::InvalidTile`] if it is too short to hold a header,
    /// and [`GraphTileProviderError::TileIdMismatch`] if the header names another tile.
    pub fn tile_header(
        &self,
        graph_id: GraphId,
    ) -> Result<GraphTileHeader, GraphTileProviderError> {
        let base_graph_id = graph_id.tile_base_id();
        let path = self.path_for_graph_id(base_graph_id)?;
        let file = File::open(path).map_err(|e| match e.kind() {
            ErrorKind::NotFound => GraphTileProviderError::TileNotAvailable(base_graph_id),
            _ => GraphTileProviderError::Io(e),
        })?;
        let size = size_of::<GraphTileHeader>();
        let mut bytes = Vec::with_capacity(size);
        // `size` is a small constant, so it always fits in a u64.
        file.take(size as u64).read_to_end(&mut bytes)?;
        let header = GraphTileHeader::read_from_bytes(&bytes).map_err(|_| {
            GraphTileProviderError::InvalidTile {
                graph_id: base_graph_id,
                source: GraphTileDecodingError::SliceLength,
            }
        })?;
        let actual = header.graph_id().tile_base_id();
        if actual != base_graph_id {
            return Err(GraphTileProviderError::TileIdMismatch {
                expected: base_graph_id,
                actual,
            });
        }
        Ok(header)
    }

    /// Computes the path for the given graph ID.
    ///
    /// NOTE: This function assumes that the graph ID is already a base ID.
    fn path_for_graph_id(&self, graph_id: GraphId) -> Result<PathBuf, InvalidGraphIdError> {
        Ok(self.base_directory.join(graph_id.file_path("gph")?))
    }
}

impl GraphTileProvider for DirectoryGraphTileProvider {
    type TileHandle = Arc<MmapGraphTileHandle>;

    #[inline]
    fn with_tile_containing<F, T>(
        &self,
        graph_id: GraphId,
        process: F,
    ) -> Result<T, GraphTileProviderError>
    where
        F: FnOnce(&GraphTileView) -> T,
    {
        let tile = self.get_handle_for_tile_containing(graph_id)?;
        Ok(process(tile.borrow_dependent()))
    }

    fn get_handle_for_tile_containing(
        &self,
        graph_id: GraphId,
    ) -> Result<Arc<MmapGraphTileHandle>, GraphTileProviderError> {
        let base_graph_id = graph_id.tile_base_id();
        match self.cached(base_graph_id)? {
            Some(Cached::Tile(tile)) => return Ok(tile),
            Some(Cached::Missing(_)) => {
                return Err(GraphTileProviderError::TileNotAvailable(base_graph_id));
            }
            None => {}
        }

        let lock = self.lock_table.lock_for(base_graph_id);
        // The lock guards no data, so a panic while holding it can't leave anything inconsistent.
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        // Another thread may have loaded it while this one waited.
        if let Some(Cached::Tile(tile)) = self.cached(base_graph_id)? {
            return Ok(tile);
        }
        let generation = self.generation.load(Ordering::Acquire);
        match self.map_tile(base_graph_id) {
            Ok(tile) => {
                self.cache(base_graph_id, Cached::Tile(Arc::clone(&tile)))?;
                Ok(tile)
            }
            Err(GraphTileProviderError::TileNotAvailable(id)) => {
                // Read before looking, so a refresh during the lookup isn't lost.
                self.cache(base_graph_id, Cached::Missing(generation))?;
                Err(GraphTileProviderError::TileNotAvailable(id))
            }
            // Corrupt or unreadable tiles aren't cached: replacing the file fixes them.
            Err(e) => Err(e),
        }
    }

    /// Checks for the tile file in the directory.
    ///
    /// # Performance
    ///
    /// This needs a filesystem lookup (`stat`) for tiles that are not in the cache.
    fn contains_tile(&self, graph_id: GraphId) -> Result<bool, GraphTileProviderError> {
        let base_graph_id = graph_id.tile_base_id();
        match self.cached(base_graph_id)? {
            Some(Cached::Tile(_)) => return Ok(true),
            Some(Cached::Missing(_)) => return Ok(false),
            None => {}
        }
        Ok(self.path_for_graph_id(base_graph_id)?.is_file())
    }
}

#[cfg(test)]
mod test {
    use super::DirectoryGraphTileProvider;
    use crate::GraphId;
    use crate::graph_tile::GraphTile;
    use crate::tile_hierarchy::STANDARD_LEVELS;
    use crate::tile_provider::{GraphTileProvider, GraphTileProviderError};
    use core::num::NonZeroUsize;
    use rand::{
        distr::{Distribution, Uniform},
        rng,
    };
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_get_tile() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles");
        let provider = DirectoryGraphTileProvider::new(base, NonZeroUsize::new(1).unwrap());
        let graph_id = GraphId::try_from_components(0, 3015, 0).expect("Unable to create graph ID");
        let tile = provider
            .get_handle_for_tile_containing(graph_id)
            .expect("Unable to get tile");

        // Minimally test that we got the correct tile
        assert_eq!(tile.header().graph_id(), graph_id);
        assert_eq!(tile.header().graph_id().value(), graph_id.value());
    }

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles")
    }

    fn fixture_ids() -> Vec<GraphId> {
        let mut ids: Vec<GraphId> = [
            (0, 3015),
            (1, 47701),
            (2, 762_485),
            (2, 762_486),
            (2, 763_925),
            (2, 763_926),
            (2, 763_927),
        ]
        .into_iter()
        .map(|(level, tile)| GraphId::try_from_components(level, tile, 0).unwrap())
        .collect();
        ids.sort_unstable();
        ids
    }

    /// A fresh, empty directory that is removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "valinor-directory-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn lists_tiles() {
        let provider =
            DirectoryGraphTileProvider::new(fixture_dir(), NonZeroUsize::new(1).unwrap());
        assert_eq!(provider.tile_ids().unwrap(), fixture_ids());
    }

    #[test]
    fn lists_only_files_at_tile_paths() {
        let dir = TempDir::new("list");
        let tile = std::fs::read(fixture_dir().join("0/003/015.gph")).unwrap();
        let write = |relative: &str| {
            let path = dir.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, &tile).unwrap();
        };
        write("0/003/015.gph");
        // Not tile paths: too few or too many digits, a missing directory, other names.
        write("0/03/015.gph");
        write("0/003/0150.gph");
        write("0/003015.gph");
        write("0/003/015.gph.tmp");
        write("0/003/01x.gph");
        write("x/003/015.gph");
        write("9/003/015.gph");
        write("index.bin");
        std::fs::create_dir_all(dir.0.join("2/000/empty.gph")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir.0, dir.0.join("1")).unwrap();

        let provider =
            DirectoryGraphTileProvider::new(dir.0.clone(), NonZeroUsize::new(1).unwrap());
        assert_eq!(
            provider.tile_ids().unwrap(),
            vec![GraphId::try_from_components(0, 3015, 0).unwrap()]
        );

        let missing =
            DirectoryGraphTileProvider::new(dir.0.join("missing"), NonZeroUsize::new(1).unwrap());
        assert!(matches!(
            missing.tile_ids(),
            Err(GraphTileProviderError::Io(_))
        ));
    }

    #[test]
    fn reads_single_headers() {
        let provider =
            DirectoryGraphTileProvider::new(fixture_dir(), NonZeroUsize::new(1).unwrap());
        for id in fixture_ids() {
            let header = provider
                .tile_header(id.with_feature_index(2).unwrap())
                .unwrap();
            let tile = provider.get_handle_for_tile_containing(id).unwrap();
            assert_eq!(&header, tile.header());
        }

        let dir = TempDir::new("headers");
        let tile = std::fs::read(fixture_dir().join("0/003/015.gph")).unwrap();
        std::fs::create_dir_all(dir.0.join("0/003")).unwrap();
        // A tile in another tile's place, and a file too short to hold a header.
        std::fs::write(dir.0.join("0/003/016.gph"), &tile).unwrap();
        std::fs::write(dir.0.join("0/003/017.gph"), &tile[..100]).unwrap();
        let provider =
            DirectoryGraphTileProvider::new(dir.0.clone(), NonZeroUsize::new(1).unwrap());
        let id = |tile| GraphId::try_from_components(0, tile, 0).unwrap();
        assert!(matches!(
            provider.tile_header(id(3016)),
            Err(GraphTileProviderError::TileIdMismatch { expected, actual })
                if expected == id(3016) && actual == id(3015)
        ));
        assert!(matches!(
            provider.tile_header(id(3017)),
            Err(GraphTileProviderError::InvalidTile { graph_id, .. }) if graph_id == id(3017)
        ));
        assert!(matches!(
            provider.tile_header(id(3015)),
            Err(GraphTileProviderError::TileNotAvailable(missing)) if missing == id(3015)
        ));
    }

    #[test]
    fn test_get_opp_edge() {
        let mut rng = rng();

        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles");
        let provider = DirectoryGraphTileProvider::new(base, NonZeroUsize::new(1).unwrap());
        let graph_id = GraphId::try_from_components(0, 3015, 0).expect("Unable to create graph ID");
        let tile = provider
            .get_handle_for_tile_containing(graph_id)
            .expect("Unable to get tile");

        // Cross-check the default implementation of the opposing edge ID function.
        // We only check a subset because it takes too long otherwise.
        let range = Uniform::try_from(0..u64::from(tile.header().directed_edge_count())).unwrap();
        for index in range.sample_iter(&mut rng).take(100) {
            let edge_id = graph_id
                .with_feature_index(index)
                .expect("Invalid graph ID.");
            let opp_edge_index = tile
                .get_opp_edge_index(edge_id)
                .expect("Unable to get opp edge index.");
            let opp_edge_id_1 = provider
                .graph_id_for_opposing_edge_index(opp_edge_index, tile.borrow_dependent())
                .expect("Unable to get the full ID for the opposing edge index!");

            let opp_edge_id_2 = provider
                .get_opposing_edge_id(edge_id, tile.borrow_dependent())
                .expect("Unable to get opposing edge.");
            assert_eq!(opp_edge_id_1, opp_edge_id_2);
        }
    }

    #[test]
    fn test_get_shortcut_fixture() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles");
        let provider = DirectoryGraphTileProvider::new(base, NonZeroUsize::new(1).unwrap());

        // This test relies on the state of the andorra-tiles fixtures.
        // TODO: Once we get full-on tile construction from scratch, build a test case rather than relying on the fixture.
        // This test was created with Valhalla as an oracle by adding the following code to valhalla_ways_to_edges.cc
        // and scanning for a few representative shortcuts:
        // if (edge->is_shortcut()) {
        //   auto shortcuts = reader.RecoverShortcut(edge_id);
        //
        //   std::ostringstream oss;
        //   oss << "[";
        //   for (size_t i = 0; i < shortcuts.size(); ++i) {
        //     if (i > 0)
        //       oss << ", ";
        //     oss << std::to_string(shortcuts[i]);
        //   }
        //   oss << "]";
        //
        //   LOG_INFO("Shortcut " + std::to_string(edge_id) + " contains edges: " + oss.str());
        // }
        let expected_shortcut_id = GraphId::try_from_components(0, 3015, 0).unwrap();
        let graph_id = GraphId::try_from_components(0, 3015, 5).unwrap();
        let shortcut_id = provider
            .get_shortcut(graph_id)
            .expect("Unable to get shortcut")
            .unwrap();

        assert_eq!(shortcut_id, expected_shortcut_id);

        let graph_id = GraphId::try_from_components(0, 3015, 386).unwrap();
        let shortcut_id = provider
            .get_shortcut(graph_id)
            .expect("Unable to get shortcut")
            .unwrap();

        assert_eq!(shortcut_id, expected_shortcut_id);
    }

    // TODO: An explicit test for a case spanning tiles would be nice, but the extract is too small.

    #[cfg(not(miri))]
    #[test]
    fn test_level_zero_shortcut_sanity() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles");
        let provider = DirectoryGraphTileProvider::new(base, NonZeroUsize::new(1).unwrap());

        let base_id = GraphId::try_from_components(0, 3015, 0).unwrap();
        provider.with_tile_containing(base_id, |tile| {
            let mut found_non_superseded_edge_with_shortcuts = false;
            let mut edges_inside_shortcuts = 0;
            let mut unique_shortcut_ids = HashSet::new();
            for (idx, edge) in tile.directed_edges().iter().enumerate() {
                let graph_id = base_id
                    .with_feature_index(idx as u64)
                    .unwrap();

                // Property test style assertions through all edges in this (small) level zero tile
                let shortcut_id = if edge.is_superseded() {
                    let shortcut_id = provider
                        .get_shortcut(graph_id)
                        .unwrap();

                    assert!(
                        shortcut_id.is_some(),
                        "Superseded edges must be traceable back to a shortcut"
                    );
                    shortcut_id
                } else {
                    found_non_superseded_edge_with_shortcuts = true;
                    provider
                        .get_shortcut(graph_id)
                        .unwrap()
                };

                if let Some(shortcut_id) = shortcut_id {
                    // NOTE: This fixture also assumes we never go beyond a single tile
                    let shortcut_edge = tile
                        .get_directed_edge(shortcut_id)
                        .unwrap();
                    let edge_geom = tile.get_edge_info(edge).unwrap().decode_raw_shape::<f32>().unwrap();
                    let mut shortcut_edge_geom = tile.get_edge_info(shortcut_edge).unwrap().decode_raw_shape::<f32>().unwrap();

                    if edge.edge_info_is_forward() != shortcut_edge.edge_info_is_forward() {
                        shortcut_edge_geom.reverse();
                    }

                    assert!(shortcut_edge_geom.len() >= edge_geom.len(), "Shortcuts should be at least as long as the underlying edge geom");

                    if shortcut_edge_geom.len() == edge_geom.len() {
                        assert_eq!(shortcut_edge_geom, edge_geom);
                    } else {
                        // Make sure the shortcut edge contains the underlying geom
                        let mut found_match = false;
                        for s_offset in 0..shortcut_edge_geom.len() {
                            if shortcut_edge_geom[s_offset..].starts_with(&edge_geom) {
                                found_match = true;
                                break;
                            }
                        }

                        assert!(found_match, "Couldn't find any matching sub-geometry. Shortcut = {shortcut_edge_geom:?}; Edge = {edge_geom:?}");
                    }

                    // Shortcuts resolve to themselves; only count the edges they contain.
                    if edge.is_shortcut() {
                        assert_eq!(shortcut_id, graph_id, "Shortcuts should resolve to themselves");
                    } else {
                        edges_inside_shortcuts += 1;
                    }
                    unique_shortcut_ids.insert(shortcut_id);
                }
            }

            assert!(found_non_superseded_edge_with_shortcuts, "Expected to find non-superseded edges with shortcuts too");
            // These match GraphReader::RecoverShortcut in Valhalla 3.9.1
            // over every shortcut in this tile (see the C++ snippet above):
            // there are 530 shortcuts, which contain 2710 edges in this tile.
            assert_eq!(unique_shortcut_ids.len(), 530);
            // We do a precise check here (even though it would break with a fixture tile rebuild)
            // to ensure we don't accidentally break the get_shortcut method in a subtle way
            // so that some shortcuts aren't reported.
            assert_eq!(edges_inside_shortcuts, 2710);
        })
        .expect("Unable to get tile");
    }

    #[test]
    fn test_hierarchy_properties() {
        // This test isn't quite so much about Valinor's correctness as it is about documenting
        // how the tile structure works.
        // Specifically, there is only a single "copy" of a feature in the routing graph,
        // and all features of a specific road class exist in specific tile levels.
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles");
        let provider = DirectoryGraphTileProvider::new(base, NonZeroUsize::new(1).unwrap());
        let graph_ids = vec![
            GraphId::try_from_components(0, 3015, 0).expect("Unable to create graph ID"),
            GraphId::try_from_components(1, 47_701, 0).expect("Unable to create graph ID"),
            GraphId::try_from_components(2, 762_485, 0).expect("Unable to create graph ID"),
        ];

        for graph_id in graph_ids {
            let level = graph_id.level();
            let tile = provider
                .get_handle_for_tile_containing(graph_id)
                .expect("Unable to get tile");

            // Minimally test that we got the correct tile
            assert_eq!(tile.header().graph_id(), graph_id);
            assert_eq!(tile.header().graph_id().value(), graph_id.value());

            // Make sure the edges contain the types of edges we expect
            let level_info = &STANDARD_LEVELS[level as usize];
            assert_eq!(level_info.level, level);

            for edge in tile.directed_edges() {
                if level == 2 {
                    assert!(
                        !edge.is_shortcut(),
                        "We never expect to see a shortcut at level 2"
                    );
                }

                assert!(
                    edge.classification() <= level_info.minimum_road_class,
                    "Violated minimum road class restriction for this level"
                );

                if level > 0 {
                    assert!(
                        edge.classification()
                            > STANDARD_LEVELS[(level - 1) as usize].minimum_road_class,
                        "Unexpected feature with classification {:?} in tile level {level}",
                        edge.classification()
                    );
                }
            }
        }
    }

    /// Puts a tile file in place as an app must: written aside, then renamed into place.
    fn install(dir: &std::path::Path, relative: &str, bytes: &[u8]) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let temp = path.with_extension("part");
        std::fs::write(&temp, bytes).unwrap();
        std::fs::rename(&temp, &path).unwrap();
    }

    /// Reads every record a search might read, so that a bad mapping would show.
    fn read_everything(tile: &impl GraphTile) -> usize {
        let mut count = 0;
        for edge in tile.directed_edges() {
            if let Ok(info) = tile.get_edge_info(edge) {
                count += info
                    .decode_raw_shape::<f32>()
                    .map_or(0, |shape| shape.len());
            }
        }
        count + tile.nodes().len()
    }

    #[test]
    fn missing_tiles_appear_after_a_refresh() {
        let dir = TempDir::new("appear");
        let tile = std::fs::read(fixture_dir().join("0/003/015.gph")).unwrap();
        let provider =
            DirectoryGraphTileProvider::new(dir.0.clone(), NonZeroUsize::new(4).unwrap());
        let id = GraphId::try_from_components(0, 3015, 0).unwrap();
        assert!(matches!(
            provider.get_handle_for_tile_containing(id),
            Err(GraphTileProviderError::TileNotAvailable(missing)) if missing == id
        ));
        assert!(!provider.contains_tile(id).unwrap());

        install(&dir.0, "0/003/015.gph", &tile);
        // Remembered as missing until the app says that files arrived.
        assert!(provider.get_handle_for_tile_containing(id).is_err());
        provider.refresh();
        assert!(provider.contains_tile(id).unwrap());
        let handle = provider.get_handle_for_tile_containing(id).unwrap();
        assert_eq!(handle.header().graph_id(), id);
        // Mapping it bypassing the cache gives the same tile.
        assert_eq!(provider.map_tile(id).unwrap().header(), handle.header());
    }

    #[test]
    fn replaced_and_deleted_files_leave_mapped_tiles_intact() {
        let dir = TempDir::new("replace");
        let original = std::fs::read(fixture_dir().join("0/003/015.gph")).unwrap();
        // The same tile from another build (mainline stamped a new build ID into it).
        let replacement = std::fs::read(
            fixture_dir()
                .with_file_name("andorra-tiles-with-traffic")
                .join("0/003/015.gph"),
        )
        .unwrap();
        install(&dir.0, "0/003/015.gph", &original);
        let provider =
            DirectoryGraphTileProvider::new(dir.0.clone(), NonZeroUsize::new(4).unwrap());
        let id = GraphId::try_from_components(0, 3015, 0).unwrap();
        let old = provider.get_handle_for_tile_containing(id).unwrap();
        let old_build = old.header().build_id();
        let records = read_everything(&old);

        install(&dir.0, "0/003/015.gph", &replacement);
        // The old mapping still reads the old file.
        assert_eq!(old.header().build_id(), old_build);
        assert_eq!(read_everything(&old), records);
        // A new mapping reads the new one.
        let new = provider.map_tile(id).unwrap();
        assert_ne!(new.header().build_id(), old_build);

        std::fs::remove_file(dir.0.join("0/003/015.gph")).unwrap();
        assert_eq!(read_everything(&old), records);
        assert_eq!(read_everything(&new), read_everything(&new));
        assert!(matches!(
            provider.map_tile(id),
            Err(GraphTileProviderError::TileNotAvailable(_))
        ));
    }

    #[test]
    fn corrupt_and_truncated_files_are_errors() {
        let dir = TempDir::new("corrupt");
        let tile = std::fs::read(fixture_dir().join("0/003/015.gph")).unwrap();
        let provider =
            DirectoryGraphTileProvider::new(dir.0.clone(), NonZeroUsize::new(4).unwrap());
        let id = GraphId::try_from_components(0, 3015, 0).unwrap();
        let header = size_of::<crate::graph_tile::GraphTileHeader>();
        for length in [
            0,
            1,
            header - 1,
            header,
            header + 1,
            4096,
            tile.len() / 2,
            tile.len() - 1,
        ] {
            install(&dir.0, "0/003/015.gph", &tile[..length]);
            assert!(
                matches!(
                    provider.get_handle_for_tile_containing(id),
                    Err(GraphTileProviderError::InvalidTile { graph_id, .. }) if graph_id == id
                ),
                "{length} bytes"
            );
        }
        // Corrupt bytes anywhere: an error or a tile, never a panic.
        let mut rng = rng();
        let positions = Uniform::try_from(0..tile.len()).unwrap();
        for _ in 0..200 {
            let mut bytes = tile.clone();
            for position in positions.sample_iter(&mut rng).take(8) {
                bytes[position] ^= 0xa5;
            }
            install(&dir.0, "0/003/015.gph", &bytes);
            if let Ok(handle) = provider.map_tile(id) {
                read_everything(&handle);
            }
        }
        // A directory where the file should be is no tile.
        std::fs::remove_file(dir.0.join("0/003/015.gph")).unwrap();
        std::fs::create_dir_all(dir.0.join("0/003/015.gph")).unwrap();
        assert!(matches!(
            provider.map_tile(id),
            Err(GraphTileProviderError::TileNotAvailable(_))
        ));
        // Errors aren't remembered: a good file is read as soon as it is in place.
        std::fs::remove_dir(dir.0.join("0/003/015.gph")).unwrap();
        install(&dir.0, "0/003/015.gph", &tile[..100]);
        assert!(provider.get_handle_for_tile_containing(id).is_err());
        install(&dir.0, "0/003/015.gph", &tile);
        assert!(provider.get_handle_for_tile_containing(id).is_ok());
    }

    #[test]
    fn threads_read_while_tiles_arrive() {
        let dir = TempDir::new("threads");
        let provider =
            DirectoryGraphTileProvider::new(dir.0.clone(), NonZeroUsize::new(3).unwrap());
        let ids = fixture_ids();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..200 {
                        for &id in &ids {
                            match provider.get_handle_for_tile_containing(id) {
                                Ok(tile) => assert_eq!(tile.header().graph_id(), id),
                                Err(GraphTileProviderError::TileNotAvailable(_)) => {}
                                Err(e) => panic!("{e}"),
                            }
                        }
                    }
                });
            }
            for &id in &ids {
                let relative = id.file_path("gph").unwrap();
                let bytes = std::fs::read(fixture_dir().join(&relative)).unwrap();
                install(&dir.0, relative.to_str().unwrap(), &bytes);
                provider.refresh();
            }
        });
        provider.refresh();
        for id in ids {
            assert!(provider.get_handle_for_tile_containing(id).is_ok());
        }
    }
}
