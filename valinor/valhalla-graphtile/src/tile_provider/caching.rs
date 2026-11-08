//! A bounded, thread-safe cache of parsed tiles in front of another provider.
//!
//! Providers that read tiles straight from their bytes (the uncached
//! [`TarballTileProvider`](super::TarballTileProvider), or the directory provider)
//! validate a tile every time a handle is made for it, which costs about 15 µs per tile.
//! A router asks for the same few tiles over and over (Loki's search walks bins tile by tile,
//! and Thor's searches cross the same tiles for every edge),
//! so [`CachingTileProvider`] keeps the handles it has made and hands out clones.
//!
//! - It is bounded: at most `capacity` tiles are kept, and the least recently used one is
//!   dropped first. A handle that is still in use elsewhere stays valid after it is dropped
//!   from the cache (handles are reference counted).
//! - It is thread-safe: one cache can be shared by every request on every thread.
//!   The lock is held only to look up or insert a handle, never while a tile is parsed,
//!   so a slow parse doesn't block other threads
//!   (two threads that miss the same tile at once may both parse it; the second insert wins).
//! - Tiles that are not in the tile set are remembered too,
//!   since searches at the edge of an extract ask for them repeatedly.
//! - It is synchronous (no async runtime), so it works offline on a device.
//!
//! Other errors (I/O errors and corrupt tiles) are not cached: they are returned every time.

use super::{GraphTileProvider, GraphTileProviderError};
use crate::GraphId;
use crate::graph_tile::GraphTileView;
use crate::graph_tile::handles::GraphTileHandle;
use core::num::NonZeroUsize;
use lru::LruCache;
use std::sync::Mutex;

/// A cached lookup: the tile's handle, or `None` if the tile is not in the tile set.
type Entry<H> = Option<H>;

/// A [`GraphTileProvider`] that caches the tile handles of another one.
///
/// See the [module docs](self).
pub struct CachingTileProvider<P: GraphTileProvider> {
    inner: P,
    cache: Mutex<LruCache<GraphId, Entry<P::TileHandle>>>,
}

impl<P: GraphTileProvider> CachingTileProvider<P> {
    /// Wraps `inner`, keeping at most `capacity` tiles (including tiles known to be missing).
    #[inline]
    pub fn new(inner: P, capacity: NonZeroUsize) -> Self {
        Self {
            inner,
            cache: Mutex::new(LruCache::new(capacity)),
        }
    }

    /// The provider whose tiles are cached.
    #[inline]
    pub fn inner(&self) -> &P {
        &self.inner
    }

    /// Drops every cached tile.
    ///
    /// # Errors
    ///
    /// Fails with [`GraphTileProviderError::PoisonedCacheLock`] if another thread panicked
    /// while holding the lock.
    pub fn clear(&self) -> Result<(), GraphTileProviderError> {
        self.cache
            .lock()
            .map_err(|_| GraphTileProviderError::PoisonedCacheLock)?
            .clear();
        Ok(())
    }

    /// How many tiles (and missing tiles) are cached.
    ///
    /// # Errors
    ///
    /// As for [`clear`](Self::clear).
    pub fn len(&self) -> Result<usize, GraphTileProviderError> {
        Ok(self
            .cache
            .lock()
            .map_err(|_| GraphTileProviderError::PoisonedCacheLock)?
            .len())
    }

    /// Whether nothing is cached.
    ///
    /// # Errors
    ///
    /// As for [`clear`](Self::clear).
    pub fn is_empty(&self) -> Result<bool, GraphTileProviderError> {
        Ok(self.len()? == 0)
    }

    fn lookup(&self, graph_id: GraphId) -> Result<P::TileHandle, GraphTileProviderError> {
        let base_id = graph_id.tile_base_id();
        let cached = self
            .cache
            .lock()
            .map_err(|_| GraphTileProviderError::PoisonedCacheLock)?
            .get(&base_id)
            .cloned();
        let entry = if let Some(entry) = cached {
            entry
        } else {
            // Parse outside of the lock.
            let entry = match self.inner.get_handle_for_tile_containing(base_id) {
                Ok(handle) => Some(handle),
                Err(GraphTileProviderError::TileNotAvailable(_)) => None,
                Err(e) => return Err(e),
            };
            self.cache
                .lock()
                .map_err(|_| GraphTileProviderError::PoisonedCacheLock)?
                .put(base_id, entry.clone());
            entry
        };
        entry.ok_or(GraphTileProviderError::TileNotAvailable(base_id))
    }
}

impl<P> GraphTileProvider for CachingTileProvider<P>
where
    P: GraphTileProvider,
    P::TileHandle: GraphTileHandle,
{
    type TileHandle = P::TileHandle;

    #[inline]
    fn with_tile_containing<F, T>(
        &self,
        graph_id: GraphId,
        process: F,
    ) -> Result<T, GraphTileProviderError>
    where
        F: FnOnce(&GraphTileView) -> T,
    {
        let handle = self.lookup(graph_id)?;
        Ok(process(&handle.tile_view()))
    }

    #[inline]
    fn get_handle_for_tile_containing(
        &self,
        graph_id: GraphId,
    ) -> Result<Self::TileHandle, GraphTileProviderError> {
        self.lookup(graph_id)
    }

    #[inline]
    fn contains_tile(&self, graph_id: GraphId) -> Result<bool, GraphTileProviderError> {
        let base_id = graph_id.tile_base_id();
        if let Some(entry) = self
            .cache
            .lock()
            .map_err(|_| GraphTileProviderError::PoisonedCacheLock)?
            .peek(&base_id)
        {
            return Ok(entry.is_some());
        }
        self.inner.contains_tile(base_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_tile::GraphTile;
    use crate::tile_provider::{DirectoryGraphTileProvider, TarballTileProvider};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name)
    }

    fn tile_ids() -> Vec<GraphId> {
        [
            (0, 3015),
            (1, 47701),
            (2, 763_926),
            (2, 763_925),
            (2, 762_485),
        ]
        .into_iter()
        .map(|(level, tile)| GraphId::try_from_components(level, tile, 0).unwrap())
        .collect()
    }

    #[test]
    fn caches_handles_and_missing_tiles() {
        let inner = TarballTileProvider::<false, false>::new_readonly(fixture("andorra-tiles.tar"))
            .unwrap();
        let provider = CachingTileProvider::new(inner, NonZeroUsize::new(16).unwrap());
        assert!(provider.is_empty().unwrap());
        for id in tile_ids() {
            let first = provider.get_handle_for_tile_containing(id).unwrap();
            let second = provider.get_handle_for_tile_containing(id).unwrap();
            // The same parsed tile, not a second parse.
            assert!(Arc::ptr_eq(&first, &second));
            assert_eq!(first.graph_id(), id);
            let count = provider
                .with_tile_containing(id, |tile| tile.nodes().len())
                .unwrap();
            assert_eq!(count, first.nodes().len());
            assert!(provider.contains_tile(id).unwrap());
        }
        let missing = GraphId::try_from_components(2, 1, 0).unwrap();
        assert!(!provider.contains_tile(missing).unwrap());
        for _ in 0..2 {
            assert!(matches!(
                provider.get_handle_for_tile_containing(missing),
                Err(GraphTileProviderError::TileNotAvailable(id)) if id == missing
            ));
        }
        assert!(!provider.contains_tile(missing).unwrap());
        assert_eq!(provider.len().unwrap(), 6);
        provider.clear().unwrap();
        assert!(provider.is_empty().unwrap());
    }

    #[test]
    fn stays_within_its_capacity() {
        let inner = DirectoryGraphTileProvider::new(
            fixture("andorra-tiles"),
            NonZeroUsize::new(1).unwrap(),
        );
        let provider = CachingTileProvider::new(inner, NonZeroUsize::new(2).unwrap());
        let ids = tile_ids();
        let kept = provider.get_handle_for_tile_containing(ids[0]).unwrap();
        for id in &ids {
            provider.get_handle_for_tile_containing(*id).unwrap();
            assert!(provider.len().unwrap() <= 2);
        }
        // A handle dropped from the cache is still usable.
        assert_eq!(kept.graph_id(), ids[0]);
    }

    #[test]
    fn is_shared_between_threads() {
        let inner = TarballTileProvider::<false, false>::new_readonly(fixture("andorra-tiles.tar"))
            .unwrap();
        let provider = Arc::new(CachingTileProvider::new(
            inner,
            NonZeroUsize::new(3).unwrap(),
        ));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let provider = Arc::clone(&provider);
                thread::spawn(move || {
                    let ids = tile_ids();
                    for round in 0..50 {
                        let id = ids[(i + round) % ids.len()];
                        let tile = provider.get_handle_for_tile_containing(id).unwrap();
                        assert_eq!(tile.graph_id(), id);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert!(provider.len().unwrap() <= 3);
    }
}
