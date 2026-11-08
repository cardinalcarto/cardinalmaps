//! # Directory tile provider
//!
//! Reads graph tiles from a directory tree of tile files,
//! laid out the way Valhalla writes them (`mjolnir.tile_dir`).

use crate::GraphId;
use crate::graph_id::InvalidGraphIdError;
use crate::graph_tile::{
    GraphTile, GraphTileBuildError, GraphTileBuilder, GraphTileDecodingError, GraphTileHeader,
};
use crate::graph_tile::{GraphTileView, handles::OwnedGraphTileHandle};
use crate::tile_provider::{GraphTileProvider, GraphTileProviderError, LockTable};
use core::num::NonZeroUsize;
use lru::LruCache;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::{fs::File, io::BufWriter};
use zerocopy::FromBytes;

/// A graph tile provider that is backed by a directory of tiles.
///
/// # Resource consumption
///
/// To minimize file handle churn and re-validation of the mapped tile memory,
/// this includes an internal LRU cache.
/// This is configurable with a max number of cached tiles.
/// Any cached tiles will remain in memory.
///
/// # Missing tiles
///
/// A tile without a file is reported as [`GraphTileProviderError::TileNotAvailable`].
/// Tile files are checked to contain the tile their path names.
pub struct DirectoryGraphTileProvider {
    base_directory: PathBuf,
    lock_table: LockTable<GraphId>,
    // TODO: This is a bit hackish for now, but even so it speeds things up MASSIVELY for many workloads!
    lru_cache: Mutex<LruCache<GraphId, Arc<OwnedGraphTileHandle>>>,
}

impl DirectoryGraphTileProvider {
    /// Creates a provider for the tiles in `base_directory`,
    /// keeping up to `num_cached_tiles` decoded tiles in memory.
    #[inline]
    pub fn new(base_directory: PathBuf, num_cached_tiles: NonZeroUsize) -> Self {
        DirectoryGraphTileProvider {
            base_directory,
            lock_table: LockTable::new(),
            lru_cache: Mutex::new(LruCache::new(num_cached_tiles)),
        }
    }

    /// Replaces the tile stored at the designated location
    /// with the tile produced by this builder.
    ///
    /// # Data race safety
    ///
    /// The implementation takes the following steps to ensure data race safety:
    ///
    /// - Acquires a lock at the start of the call, ensuring that calls to [`GraphTileProvider::get_handle_for_tile_containing`]
    ///   block until this method exits.
    /// - Writes to a temporary file initially, so if anything goes wrong, the original file is not affected.
    ///   After successfully writing the temp file, it is atomically renamed on top of the original.
    /// - Clears the internal tile cache (local to _this_ [`DirectoryGraphTileProvider`] instance!)
    ///   once the rename succeeds.
    /// - Releases the lock, unblocking access to this tile, only after everything succeeds,
    ///   or fails mid-write (in which case, only the temporary file is affected).
    ///
    /// In this way, a (single!) [`DirectoryGraphTileProvider`] should be safe to use in async contexts,
    /// without fear of races from writers.
    /// Additionally, since readers always have to wait for a writer to finish,
    /// readers will never get a partially written tile, nor will they get a stale tile
    /// if there is an open write operation.
    ///
    /// ## Edge cases
    ///
    /// Once you have a handle to a graph tile from [`GraphTileProvider::get_handle_for_tile_containing`],
    /// the lock is freed and a writer may then proceed.
    /// Your handle will _not_ change or block writers,
    /// creating an opportunity for edge cases (especially if you plan to create a builder from this tile
    /// to write changes!).
    ///
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

        let mut cache = self
            .lru_cache
            .lock()
            .map_err(|e| GraphTileBuildError::PoisonedCacheLock(e.to_string()))?;
        // Invalidate the cache
        cache.pop(&graph_id);

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
    type TileHandle = Arc<OwnedGraphTileHandle>;

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
    ) -> Result<Arc<OwnedGraphTileHandle>, GraphTileProviderError> {
        // TODO: Do we want to move the base ID check inside file_path?
        let base_graph_id = graph_id.tile_base_id();

        let lock = self.lock_table.lock_for(base_graph_id);
        // The lock guards no data, so a panic while holding it can't leave anything inconsistent.
        let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);

        let mut cache = self
            .lru_cache
            .lock()
            .map_err(|_| GraphTileProviderError::PoisonedCacheLock)?;
        let tile = cache
            .try_get_or_insert(base_graph_id, || {
                // Build up the path from the base directory + tile ID components
                let path = self.path_for_graph_id(base_graph_id)?;
                // Open the file and read all bytes into a buffer
                // NOTE: Does not handle compressed tiles
                let data = std::fs::read(path).map_err(|e| match e.kind() {
                    ErrorKind::NotFound => GraphTileProviderError::TileNotAvailable(base_graph_id),
                    _ => GraphTileProviderError::Io(e),
                })?;
                let tile = OwnedGraphTileHandle::try_from(data).map_err(|source| {
                    GraphTileProviderError::InvalidTile {
                        graph_id: base_graph_id,
                        source,
                    }
                })?;
                // A tile file in the wrong place would otherwise be silently used as another tile.
                let actual = tile.header().graph_id().tile_base_id();
                if actual != base_graph_id {
                    return Err(GraphTileProviderError::TileIdMismatch {
                        expected: base_graph_id,
                        actual,
                    });
                }
                Ok::<_, GraphTileProviderError>(Arc::new(tile))
            })
            .cloned()?;

        Ok(tile)
    }

    /// Checks for the tile file in the directory.
    ///
    /// # Performance
    ///
    /// This needs a filesystem lookup (`stat`) for tiles that are not in the cache.
    fn contains_tile(&self, graph_id: GraphId) -> Result<bool, GraphTileProviderError> {
        let base_graph_id = graph_id.tile_base_id();
        let cached = self
            .lru_cache
            .lock()
            .map_err(|_| GraphTileProviderError::PoisonedCacheLock)?
            .contains(&base_graph_id);
        if cached {
            return Ok(true);
        }

        Ok(self.path_for_graph_id(base_graph_id)?.try_exists()?)
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
}
