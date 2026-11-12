//! # Tarball tile provider
//!
//! Reads graph tiles from a memory-mapped tarball extract,
//! as produced by Valhalla's `valhalla_build_extract`.
//! See [`TarballTileProvider`] for the format, platform caveats, and error handling.

use super::{GraphTileProvider, GraphTileProviderError};
use crate::GraphId;
use crate::graph_tile::handles::MmapGraphTileHandle;
use crate::graph_tile::{
    GraphTile, GraphTileView,
    handles::{MmapTilePointer, TileOffset},
};
use crate::tile_hierarchy::{STANDARD_LEVELS, TRANSIT_LEVEL};
use memmap2::{MmapOptions, MmapRaw};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use tar::Archive;
use thiserror::Error;
use zerocopy::{FromBytes as _, IntoBytes as _, LE, U32, U64};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, Unaligned};

// Offsets into an extract are 64-bit,
// and the pointer arithmetic over the memory map assumes that they fit in a `usize`.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("valhalla-graphtile only supports 64-bit platforms");

/// The name of the index entry in a tarball extract.
const INDEX_FILE_NAME: &str = "index.bin";

/// The reasons why a tarball extract can't be opened.
#[derive(Debug, Error)]
pub enum InvalidTarballError {
    /// The archive has no entries at all (for example, an empty file).
    #[error("read the first entry: the archive has no entries")]
    NoEntries,
    /// The header of the first tar entry could not be read.
    ///
    /// This happens for files that are not tar archives,
    /// and for downloads truncated within the first header.
    #[error("read the first entry: {0}")]
    FirstEntry(#[source] std::io::Error),
    /// The first entry of the archive is not `index.bin`.
    #[error("find {INDEX_FILE_NAME}: expected it as the first entry, but found {0:?}")]
    IndexNotFirst(String),
    /// The `index.bin` entry is larger than any valid index.
    #[error(
        "read {INDEX_FILE_NAME}: it claims {size} bytes, but an index of every possible tile only has {max} bytes"
    )]
    IndexTooLarge {
        /// The size of the entry in its tar header.
        size: u64,
        /// The size of an index that lists every tile in the hierarchy.
        max: u64,
    },
    /// An entry (the index, or a tile it lists) extends past the end of the file.
    ///
    /// This almost always means that the download was truncated.
    #[error(
        "locate {what}: it spans bytes {offset}..{end}, but the archive only has {archive_len} bytes (is it truncated?)"
    )]
    OutOfBounds {
        /// What was being located.
        what: TarballEntry,
        /// The start of the entry data.
        offset: u64,
        /// The end of the entry data (exclusive).
        end: u128,
        /// The length of the archive in bytes.
        archive_len: u64,
    },
}

/// Identifies an entry of a tarball, for error reporting.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TarballEntry {
    /// The `index.bin` entry.
    Index,
    /// A graph tile.
    Tile(GraphId),
}

impl core::fmt::Display for TarballEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Index => f.write_str(INDEX_FILE_NAME),
            Self::Tile(graph_id) => write!(f, "tile {graph_id}"),
        }
    }
}

/// A tile provider backed by a memory-mapped tarball archive.
///
/// This is the provider that production servers and mobile apps use.
///
/// # Extract format
///
/// An extract is an ordinary tar archive whose first entry is `index.bin`
/// (see [`TileIndexBinEntry`]),
/// followed by one entry per tile.
/// Only the index is used to find tiles;
/// the names of the tile entries are never read.
/// This matches Valhalla 3.9.1, which also loads extracts from the index alone
/// (and, since valhalla/valhalla#5995, does not keep the tile names either).
/// The format of `valhalla_build_extract` has not changed in 3.9.1:
/// the index entries are still 16 bytes (`<QLL`: offset, tile ID, and size),
/// and the index is still the first entry.
///
/// Valhalla can also load extracts without an index by scanning all entries,
/// or with the index somewhere other than first.
/// Valinor does not support either, so that opening an extract never scans it
/// (initialization fails with an error).
/// Modern Valhalla tooling like `valhalla_build_extract` always writes the index first,
/// but hand-rolled tarballs (e.g. from a `bash` script) may not have one.
///
/// # Platform support
///
/// Only 64-bit platforms are supported.
/// Extracts routinely exceed the address space of a 32-bit process,
/// and the traffic tile provider needs lock-free 64-bit loads and stores.
///
/// The provider is designed to run unchanged on servers and on phones,
/// but memory-mapped files come with platform-specific caveats.
/// None of them can be detected up front, so callers must arrange their storage accordingly:
///
/// - **Replace extracts atomically.**
///   No file locking is performed (Valhalla doesn't lock extracts either).
///   Writing to or truncating a mapped file makes pending reads fail with `SIGBUS`,
///   which crashes the process.
///   To update an extract, write the new file next to it and rename it over the old one;
///   open providers keep reading the old file until they are dropped.
/// - **Keep extracts readable while the app runs.**
///   On iOS, a file with the `NSFileProtectionComplete` data protection class
///   becomes unreadable shortly after the device locks,
///   and touching an unloaded page of it then raises `SIGBUS`.
///   Store extracts with `NSFileProtectionCompleteUntilFirstUserAuthentication` (or weaker)
///   if routing may run in the background.
///   On Android, the same applies to removable storage, which can disappear at any time;
///   keep extracts in app-specific internal storage.
/// - **Map real files.**
///   Android APK assets are not files (they may be compressed inside the APK),
///   so copy a bundled extract to app storage before opening it.
/// - **Mind the address space on iOS.**
///   iOS limits the virtual address space of an app,
///   so mapping a very large extract can fail with an I/O error (`ENOMEM`).
///   The `com.apple.developer.kernel.extended-virtual-addressing` entitlement raises the limit.
///   Mapping is lazy, so a large extract only costs memory for the pages that are read,
///   and the operating system can reclaim them under memory pressure.
///
/// # Corrupt and partial extracts
///
/// Opening an extract checks that the index is well-formed
/// and that every tile it lists lies within the file,
/// so a truncated download fails to open
/// instead of crashing on a later read.
/// Tiles that are not in the index are reported as
/// [`GraphTileProviderError::TileNotAvailable`],
/// which is expected for regional extracts.
/// To detect extracts mixed from different builds, or with corrupt tile data, see
/// [`TarballTileProvider::check_build_ids`] (cheap) and
/// [`TarballTileProvider::verify_tileset`] (reads every tile).
///
/// # Safety
///
/// The following actions are *definitely* unsafe:
///
/// - Changing the contents of the index file.
/// - By extension, changing the set of tiles mapped.
/// - Also by extension, changing the size of a tile.
/// - Truncating the file.
///
/// Violating any of the above will result in either failed tile fetches (with an error)
/// or a SIGBUS.
///
/// Additionally, extreme care must be taken when the file may be modified by an external process.
/// The current implementation is primarily designed around volatile memory access.
pub struct TarballTileProvider<const MUT: bool, const CACHE: bool> {
    /// The actual memory map.
    ///
    /// The mapping stays valid after the file handle that created it is closed,
    /// so we don't keep the file open (one less file descriptor per provider).
    mmap: Arc<MmapRaw>,
    /// An index of offsets and sizes which enables quick tile extraction from the memory map.
    ///
    /// Every entry has been checked to lie within the memory map.
    tile_index: HashMap<GraphId, TileOffset>,
    /// An immutable tile cache.
    ///
    /// While loading tiles from the tarball is zero-copy,
    /// it is not strictly zero-cost.
    /// A typical lookup requires these steps:
    ///
    /// - Hashmap look up by graph ID in `tile_index`
    /// - Construct a tile pointer
    /// - Create a slice to the raw bytes
    /// - Check the validity of the bytes before transmuting the pointer
    ///
    /// This overhead is very small,
    /// but a strenuous workload like map matching millions of short segments
    /// demonstrates that there is real overhead here.
    ///
    /// If (and only if!) the user guarantees that the underlying memory map will never be mutated,
    /// we can front-load all the sanity checks and convert every tile resolution
    /// into a single hashmap lookup.
    /// This only works if the underlying shared memory is immutable though,
    /// as mutation of memory backing a slice with a shared reference
    /// is undefined behavior: <https://doc.rust-lang.org/reference/behavior-considered-undefined.html#r-undefined.immutable>.
    tile_cache: HashMap<GraphId, Arc<MmapGraphTileHandle>>,
}

impl<const MUT: bool, const CACHE: bool> TarballTileProvider<MUT, CACHE> {
    /// Creates a new tarball tile provider from the given path.
    ///
    /// # Safety
    ///
    /// If `CACHE` is `true`, the caller must guarantee that the underlying memory map
    /// is never mutated during the lifetime of the object.
    /// Mutation results in undefined behavior.
    ///
    /// # Errors
    ///
    /// This may fail if the file can't be opened or mapped,
    /// if the index is invalid or missing,
    /// or if the index lists a tile outside of the file (a truncated download).
    /// It will also fail if `CACHE` is `true`, but a tile is malformed.
    pub(in crate::tile_provider) unsafe fn init<P: AsRef<Path>>(
        path: P,
    ) -> Result<Self, GraphTileProviderError> {
        const {
            assert!(
                !(MUT && CACHE),
                "TarballTileProvider<true, true> is not allowed; cache is not safe combined with mutability."
            );
        }

        // Open the file exactly once, and read the index through the same handle that we map.
        // Opening the path twice would let a concurrent rename (e.g. an update being installed)
        // pair the index of one file with the contents of another.
        let file = File::options().read(true).write(MUT).open(&path)?;
        let archive_len = file.metadata()?.len();
        let index_bytes = read_index(&file, archive_len)?;
        let index_entries = parse_index_bin(&index_bytes)?;

        // Index the index
        let mut tile_index: HashMap<GraphId, TileOffset> =
            HashMap::with_capacity(index_entries.len());
        for entry in index_entries {
            let (graph_id, offset) = entry.validated()?;
            let size = entry.size.get();
            check_bounds(
                TarballEntry::Tile(graph_id),
                offset,
                u64::from(size),
                archive_len,
            )?;
            let offsets = TileOffset { offset, size };
            match tile_index.entry(graph_id) {
                Entry::Occupied(_) => {
                    return Err(InvalidIndexBinError::DuplicateTile(graph_id).into());
                }
                Entry::Vacant(slot) => {
                    slot.insert(offsets);
                }
            }
        }

        let mmap = if MUT {
            MmapOptions::new().map_raw(&file)?
        } else {
            MmapOptions::new().map_raw_read_only(&file)?
        };
        // The file may have changed size between reading its metadata and mapping it.
        // Every index entry must lie within the mapping that we actually read from.
        let mapped_len = mmap.len() as u64;
        if mapped_len < archive_len {
            for (graph_id, offsets) in &tile_index {
                check_bounds(
                    TarballEntry::Tile(*graph_id),
                    offsets.offset,
                    u64::from(offsets.size),
                    mapped_len,
                )?;
            }
        }
        let mmap = Arc::new(mmap);

        let mut tile_cache = HashMap::new();

        if CACHE {
            tile_cache.reserve(tile_index.len());
            for (graph_id, tile_offset) in &tile_index {
                let tile_pointer = MmapTilePointer {
                    mmap: mmap.clone(),
                    offsets: *tile_offset,
                };
                // SAFETY: The offsets were checked to lie within the memory map above,
                // and the caller guarantees that it is never mutated (see the method docs).
                let tile_handle =
                    unsafe { MmapGraphTileHandle::try_from(tile_pointer) }.map_err(|source| {
                        GraphTileProviderError::InvalidTile {
                            graph_id: *graph_id,
                            source,
                        }
                    })?;
                check_tile_id(*graph_id, &tile_handle)?;
                tile_cache.insert(*graph_id, Arc::new(tile_handle));
            }
        }

        Ok(Self {
            mmap,
            tile_index,
            tile_cache,
        })
    }

    /// An iterator over all tile IDs contained in the tarball, in arbitrary order.
    #[inline]
    pub fn tile_ids(&self) -> impl Iterator<Item = &GraphId> {
        self.tile_index.keys()
    }

    /// The number of tiles in the tarball.
    #[inline]
    pub fn tile_count(&self) -> usize {
        self.tile_index.len()
    }

    /// Pointers to every tile, in the order the tiles are stored in the file.
    ///
    /// Reading tiles in this order makes I/O sequential.
    pub(super) fn tile_pointers_in_file_order(
        &self,
    ) -> impl Iterator<Item = (GraphId, MmapTilePointer)> + '_ {
        let mut tiles: Vec<_> = self.tile_index.iter().collect();
        tiles.sort_unstable_by_key(|(_, offsets)| offsets.offset);
        tiles.into_iter().map(|(graph_id, offsets)| {
            (
                *graph_id,
                MmapTilePointer {
                    mmap: self.mmap.clone(),
                    offsets: *offsets,
                },
            )
        })
    }

    /// Does the tarball contain the tile that the given graph ID belongs to?
    #[inline]
    fn contains_base_id(&self, graph_id: GraphId) -> bool {
        self.tile_index.contains_key(&graph_id.tile_base_id())
    }

    /// Gets a pointer for the tile containing this graph ID.
    ///
    /// The pointer is guaranteed to lie within the memory map.
    ///
    /// # Errors
    ///
    /// If the tile is not part of the tarball archive,
    /// this returns [`GraphTileProviderError::TileNotAvailable`].
    #[inline]
    pub fn get_pointer_for_tile_containing(
        &self,
        graph_id: GraphId,
    ) -> Result<MmapTilePointer, GraphTileProviderError> {
        let base_graph_id = graph_id.tile_base_id();
        let Some(offsets) = self.tile_index.get(&base_graph_id) else {
            return Err(GraphTileProviderError::TileNotAvailable(base_graph_id));
        };

        Ok(MmapTilePointer {
            mmap: self.mmap.clone(),
            offsets: *offsets,
        })
    }
}

/// Reads the `index.bin` entry at the start of the archive.
///
/// Only the first tar header and the index data are read;
/// the rest of the archive is never scanned.
fn read_index(file: &File, archive_len: u64) -> Result<Vec<u8>, GraphTileProviderError> {
    let mut archive = Archive::new(file);
    let mut entries = archive.entries().map_err(InvalidTarballError::FirstEntry)?;

    let mut entry = entries
        .next()
        .ok_or(InvalidTarballError::NoEntries)?
        .map_err(InvalidTarballError::FirstEntry)?;
    let entry_path = entry.path().map_err(InvalidTarballError::FirstEntry)?;
    if entry_path != Path::new(INDEX_FILE_NAME) {
        return Err(
            InvalidTarballError::IndexNotFirst(entry_path.to_string_lossy().into_owned()).into(),
        );
    }

    // Check the size before allocating:
    // a corrupt header must not be able to make us allocate an arbitrary amount of memory
    // (which would abort the process).
    let size = entry.size();
    let max = max_index_len();
    if size > max {
        return Err(InvalidTarballError::IndexTooLarge { size, max }.into());
    }
    check_bounds(
        TarballEntry::Index,
        entry.raw_file_position(),
        size,
        archive_len,
    )?;

    // The size is at most a few tens of megabytes, so this can't truncate on 64-bit platforms.
    let capacity = usize::try_from(size).unwrap_or(0);
    let mut index_bytes = Vec::with_capacity(capacity);
    entry.read_to_end(&mut index_bytes)?;

    Ok(index_bytes)
}

/// The size of an `index.bin` that lists every tile in the hierarchy (including transit).
///
/// No valid index can be larger, since each tile is listed at most once.
fn max_index_len() -> u64 {
    let tiles: u64 = STANDARD_LEVELS
        .iter()
        .chain(core::iter::once(&*TRANSIT_LEVEL))
        .map(|level| u64::from(level.tiling_system.tile_count()))
        .sum();
    tiles * INDEX_ENTRY_SIZE as u64
}

/// Makes sure that `size` bytes starting at `offset` lie within an archive of `archive_len` bytes.
fn check_bounds(
    what: TarballEntry,
    offset: u64,
    size: u64,
    archive_len: u64,
) -> Result<(), InvalidTarballError> {
    // Computed in 128 bits so that a corrupt offset can't overflow.
    let end = u128::from(offset) + u128::from(size);
    if end > u128::from(archive_len) {
        return Err(InvalidTarballError::OutOfBounds {
            what,
            offset,
            end,
            archive_len,
        });
    }
    Ok(())
}

/// Makes sure that a tile says (in its header) that it is the tile it was stored as.
fn check_tile_id<T: GraphTile>(expected: GraphId, tile: &T) -> Result<(), GraphTileProviderError> {
    let actual = tile.header().graph_id().tile_base_id();
    if actual == expected {
        Ok(())
    } else {
        Err(GraphTileProviderError::TileIdMismatch { expected, actual })
    }
}

impl<const MUT: bool> GraphTileProvider for TarballTileProvider<MUT, false> {
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
        let tile_pointer = self.get_pointer_for_tile_containing(graph_id)?;
        // SAFETY: The pointer lies within the memory map (checked when the index was loaded).
        // The backing memory must not be mutated while the slice is alive,
        // which is the documented contract of this provider (see the type-level docs).
        let tile_bytes = unsafe { tile_pointer.as_tile_bytes() };
        let base_id = graph_id.tile_base_id();
        let tile = GraphTileView::try_from(tile_bytes).map_err(|source| {
            GraphTileProviderError::InvalidTile {
                graph_id: base_id,
                source,
            }
        })?;
        check_tile_id(base_id, &tile)?;
        Ok(process(&tile))
    }

    #[inline]
    fn get_handle_for_tile_containing(
        &self,
        graph_id: GraphId,
    ) -> Result<Self::TileHandle, GraphTileProviderError> {
        let pointer = self.get_pointer_for_tile_containing(graph_id)?;
        let base_id = graph_id.tile_base_id();

        // SAFETY: The pointer lies within the memory map (checked when the index was loaded).
        // The backing memory is assumed to never be mutated during the lifetime of the returned handle,
        // which is the documented contract of this provider (see the type-level docs).
        let handle = unsafe { MmapGraphTileHandle::try_from(pointer) }.map_err(|source| {
            GraphTileProviderError::InvalidTile {
                graph_id: base_id,
                source,
            }
        })?;
        check_tile_id(base_id, &handle)?;
        Ok(Arc::new(handle))
    }

    #[inline]
    fn contains_tile(&self, graph_id: GraphId) -> Result<bool, GraphTileProviderError> {
        Ok(self.contains_base_id(graph_id))
    }
}

impl<const MUT: bool> GraphTileProvider for TarballTileProvider<MUT, true> {
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
        let base_id = graph_id.tile_base_id();
        let tile = self
            .tile_cache
            .get(&base_id)
            .ok_or(GraphTileProviderError::TileNotAvailable(base_id))?;
        Ok(process(tile.borrow_dependent()))
    }

    #[inline]
    fn get_handle_for_tile_containing(
        &self,
        graph_id: GraphId,
    ) -> Result<Self::TileHandle, GraphTileProviderError> {
        let base_id = graph_id.tile_base_id();
        let tile = self
            .tile_cache
            .get(&base_id)
            .ok_or(GraphTileProviderError::TileNotAvailable(base_id))?;
        Ok(tile.clone())
    }

    #[inline]
    fn contains_tile(&self, graph_id: GraphId) -> Result<bool, GraphTileProviderError> {
        Ok(self.contains_base_id(graph_id))
    }
}

impl TarballTileProvider<false, false> {
    /// Creates a new tarball tile provider from an existing extract.
    ///
    /// This constructor creates the memory mapping in read-only mode,
    /// but it does not strictly assume that the underlying memory is immutable.
    /// This constructor is safe, but underlying memory access is subject to conditions
    /// which are noted in the various access methods on pointers.
    /// Refer to the [`TarballTileProvider`] documentation for a fuller safety discussion.
    ///
    /// If you guarantee that the tarball bytes will _never_ be mutated in any way
    /// during the life of the provider, you can use [`TarballTileProvider::new_immutable`]
    /// for better performance over the long run.
    ///
    /// # Performance
    ///
    /// This reads the first tar header and the index, and nothing else.
    ///
    /// # Errors
    ///
    /// The extract _must_ include an `index.bin` file as the first entry.
    /// If the file is not _valid_ (of the correct length and superficially correct structure),
    /// or lists a tile that lies (partly) outside of the file,
    /// this constructor will fail.
    ///
    /// However, no further checks are performed to ensure the correctness of the file
    /// (its entire _raison d'être_ is that you shouldn't have to scan the whole tarball),
    /// so an incorrect index will invariably lead to tile fetch errors.
    #[inline]
    pub fn new_readonly<P: AsRef<Path>>(path: P) -> Result<Self, GraphTileProviderError> {
        // SAFETY: CACHE=false is guaranteed by the const generic
        unsafe { Self::init(path) }
    }
}

impl TarballTileProvider<false, true> {
    /// Creates a new tarball tile provider from an immutable extract.
    ///
    /// Normally there is a small amount of overhead when accessing graph tiles due to validation
    /// to ensure that the memory layout is correct (it's zero copy, but not zero cost).
    /// You pay this cost on every tile access for tile providers constructed with
    /// [`TarballTileProvider::new_readonly`] or [`TarballTileProvider::new_mutable`].
    ///
    /// This constructor front-loads the validation work so that subsequent tile access
    /// is O(1).
    /// In exchange, this requires stronger guarantees, which is why it's marked unsafe.
    ///
    /// # Performance
    ///
    /// Opening decodes the header and section layout of every tile,
    /// which touches at least one page of each tile.
    /// For a planet extract on a server this is negligible,
    /// but on a phone, [`TarballTileProvider::new_readonly`] opens much faster,
    /// since it decodes each tile only when it is used.
    ///
    /// # Safety
    ///
    /// If you use this constructor, you *must* guarantee that the backing memory and file
    /// are immutable during the lifetime of the object.
    /// Any mutation of the underlying memory results in undefined behavior.
    ///
    /// If you cannot make these guarantees, but do not require mutability,
    /// use [`TarballTileProvider::new_readonly`] instead.
    ///
    /// # Errors
    ///
    /// The extract _must_ include an `index.bin` file as the first entry.
    /// If the file is not _valid_ (of the correct length and superficially correct structure),
    /// or lists a tile that lies (partly) outside of the file,
    /// this constructor will fail.
    ///
    /// This will also fail if any tile is obviously malformed,
    /// or is stored under the wrong tile ID.
    #[inline]
    pub unsafe fn new_immutable<P: AsRef<Path>>(path: P) -> Result<Self, GraphTileProviderError> {
        // SAFETY: Immutability requirement documented in docstring
        unsafe { Self::init(path) }
    }
}

impl TarballTileProvider<true, false> {
    /// Creates a new tarball tile provider from an existing extract.
    /// Write support is enabled by this constructor.
    ///
    /// The file must be writable.
    ///
    /// # Errors
    ///
    /// The extract _must_ include an `index.bin` file as the first entry.
    /// If the file is not _valid_ (of the correct length and superficially correct structure),
    /// or lists a tile that lies (partly) outside of the file,
    /// this constructor will fail.
    ///
    /// However, no further checks are performed to ensure the correctness of the file
    /// (its entire _raison d'être_ is that you shouldn't have to scan the whole tarball),
    /// so an incorrect index will invariably lead to tile fetch errors.
    #[inline]
    pub fn new_mutable<P: AsRef<Path>>(path: P) -> Result<Self, GraphTileProviderError> {
        // SAFETY: CACHE=false is guaranteed by the const generic
        unsafe { Self::init(path) }
    }

    /// Flushes outstanding memory map modifications to disk.
    ///
    /// See [`MmapRaw::flush`] for more details.
    ///
    /// # Errors
    ///
    /// This may fail due to any of the usual reasons I/O operations fail.
    #[inline]
    pub fn flush(&self) -> std::io::Result<()> {
        self.mmap.flush()
    }
}

/// A tile index entry enabling efficient random access into a tarball archive.
///
/// # The `index.bin` file
///
/// Tarballs were designed for an era of tape drives, where access was essentially sequential.
/// This gives tarballs some interesting properties, like being a series of entries that can
/// be written sequentially without requiring (necessarily) a header at the start with full info.
/// They can even be concatenated together!
///
/// But I digress... Tarballs *can* be accessed randomly by seeking to specific offsets in the file,
/// but you need to know where each file is located.
/// Valhalla has a convention of writing an `index.bin` file as the first entry of the archive
/// which contains these offsets.
/// Once you parse this, you have the keys to random access of any graph tile within a single file
/// (typically a memory map).
///
/// The `index.bin` file is just a series of these structs written out sequentially;
/// no padding or headers, just bytes.
/// This is `tile_index_entry` in Valhalla's `graphreader.cc`,
/// and `INDEX_BIN_FORMAT` (`<QLL`) in `valhalla_build_extract`.
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, Debug, PartialEq)]
#[repr(C)]
pub struct TileIndexBinEntry {
    /// Byte offset from the beginning of the tar
    offset: U64<LE>,
    /// Just the level and tile index, hence fitting in 32 bits
    tile_id: U32<LE>,
    /// The size of the tile in bytes.
    size: U32<LE>,
}

/// The size of one [`TileIndexBinEntry`] in bytes.
const INDEX_ENTRY_SIZE: usize = size_of::<TileIndexBinEntry>();

/// The block size of a tar archive.
/// The data of each tar entry starts on a block boundary.
const TAR_BLOCK_SIZE: u64 = 512;

/// The reasons why an `index.bin` file or one of its entries is not valid.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InvalidIndexBinError {
    #[error("the index has no entries")]
    Empty,
    #[error(
        "the index length ({0} bytes) is not a multiple of the entry size ({INDEX_ENTRY_SIZE} bytes)"
    )]
    Length(usize),
    #[error("{0} is not a base tile ID; expected the feature index to be zero")]
    FeatureIndex(GraphId),
    #[error("graph ID {0} has reserved bits set")]
    ReservedBits(u64),
    #[error(
        "the entry for {graph_id} has offset {offset}; expected a non-zero multiple of {TAR_BLOCK_SIZE}"
    )]
    Offset { graph_id: GraphId, offset: u64 },
    #[error("the index lists tile {0} more than once")]
    DuplicateTile(GraphId),
}

impl TileIndexBinEntry {
    /// Creates an index entry for a tile located at the given byte offset within a tarball.
    ///
    /// # Errors
    ///
    /// This will fail if `graph_id` is not a base tile ID
    /// (the feature index and reserved bits must be zero),
    /// or if `offset` is zero or does not lie on a 512-byte boundary
    /// (tar entry data is always aligned to 512-byte blocks).
    pub fn new(graph_id: GraphId, offset: u64, size: u32) -> Result<Self, InvalidIndexBinError> {
        let tile_id = base_tile_id(graph_id)?;
        check_offset(graph_id, offset)?;

        Ok(Self {
            offset: U64::new(offset),
            tile_id: U32::new(tile_id),
            size: U32::new(size),
        })
    }

    /// Validates a parsed entry, and returns its graph ID and offset.
    ///
    /// [`parse_index_bin`] does not examine the entry values,
    /// so the read path must call this before it uses an entry.
    /// The checks are the same as in [`TileIndexBinEntry::new`].
    fn validated(&self) -> Result<(GraphId, u64), InvalidIndexBinError> {
        let graph_id = self.graph_id()?;
        let offset = self.offset.get();
        check_offset(graph_id, offset)?;

        Ok((graph_id, offset))
    }

    fn graph_id(&self) -> Result<GraphId, InvalidIndexBinError> {
        // SAFETY: We know that the bit field cannot contain a value
        // larger than the max allowed value (it's limited to 46 bits).
        // Therefore, this is guaranteed to be a valid Graph ID bit pattern.
        let graph_id = unsafe { GraphId::from_id_unchecked(self.tile_id.into()) };
        base_tile_id(graph_id)?;

        Ok(graph_id)
    }
}

/// Makes sure that `graph_id` identifies a full tile,
/// and returns the packed level and tile ID that `index.bin` stores.
fn base_tile_id(graph_id: GraphId) -> Result<u32, InvalidIndexBinError> {
    if graph_id.feature_index() != 0 {
        return Err(InvalidIndexBinError::FeatureIndex(graph_id));
    }

    // A base tile ID packs the level and tile ID into the low 25 bits,
    // so this conversion only fails for IDs with reserved bits set
    // (such IDs can only be produced by the unchecked GraphId constructors).
    u32::try_from(graph_id.value())
        .map_err(|_| InvalidIndexBinError::ReservedBits(graph_id.value()))
}

/// Makes sure that `offset` can be the start of the data of a tar entry.
///
/// A tar entry starts with a 512-byte header, so its data cannot be at offset zero.
fn check_offset(graph_id: GraphId, offset: u64) -> Result<(), InvalidIndexBinError> {
    if offset == 0 || !offset.is_multiple_of(TAR_BLOCK_SIZE) {
        return Err(InvalidIndexBinError::Offset { graph_id, offset });
    }

    Ok(())
}

/// Parses an `index.bin` file to enable random access.
///
/// See [`TileIndexBinEntry`] for a description of the tile format.
/// This function only checks the length of the input.
/// It does not validate the values in the entries.
///
/// # Errors
///
/// The input must be non-zero in length and consist of a whole number of entries;
/// anything else fails with an [`InvalidIndexBinError`].
pub fn parse_index_bin(index_bytes: &[u8]) -> Result<&[TileIndexBinEntry], InvalidIndexBinError> {
    if index_bytes.is_empty() {
        return Err(InvalidIndexBinError::Empty);
    }

    // The entry type is unaligned,
    // so the only possible failure is a length that is not a whole number of entries.
    <[TileIndexBinEntry]>::ref_from_bytes(index_bytes)
        .map_err(|_| InvalidIndexBinError::Length(index_bytes.len()))
}

/// Serializes tile index entries into the bytes of an `index.bin` file.
///
/// This is the write-side twin of [`parse_index_bin`]:
/// the entries are written out sequentially with no padding or headers
/// (see [`TileIndexBinEntry`] for a description of the file format).
/// Bytes returned by this function always parse back into the same entries.
///
/// # Performance
///
/// The entries already have the exact memory layout of the file format,
/// so this is a zero-copy transmutation of the slice.
///
/// # Errors
///
/// An index without any entries is malformed
/// (an extract containing no tiles is meaningless, and [`parse_index_bin`] rejects it),
/// so serializing an empty slice fails with [`InvalidIndexBinError::Empty`].
pub fn serialize_index_bin(entries: &[TileIndexBinEntry]) -> Result<&[u8], InvalidIndexBinError> {
    if entries.is_empty() {
        return Err(InvalidIndexBinError::Empty);
    }

    Ok(entries.as_bytes())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::graph_tile::{GraphTile, GraphTileView};
    use crate::tile_provider::DirectoryGraphTileProvider;
    use proptest::collection::vec;
    use proptest::{prop_assert, prop_assert_eq, proptest};
    use std::num::NonZeroUsize;
    use std::path::PathBuf;

    /// Bytes taken from the start of a large extract generated by official Valhalla tooling.
    const INDEX_BIN_FIXTURE: &[u8] = &[
        // Tile 1
        0x00, 0x5a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x60, 0x4f, 0x00, 0x00, 0x9c, 0x08, 0x12,
        0x00, // Tile 2
        0x00, 0x6a, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x52, 0x00, 0x00, 0xc8, 0x67, 0x01,
        0x00, // Tile 3
        0x00, 0xd8, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x28, 0x52, 0x00, 0x00, 0x28, 0x9d, 0x1b,
        0x00,
    ];

    #[test]
    fn test_parse_index_bin_invalid() {
        assert_eq!(
            parse_index_bin(&[]),
            Err(InvalidIndexBinError::Empty),
            "Empty indexes are invalid"
        );
        assert_eq!(
            parse_index_bin(&[0x00, 0x5a]),
            Err(InvalidIndexBinError::Length(2)),
            "Index must contain at least one full entry"
        );
        assert_eq!(
            parse_index_bin(&[
                // Contains one extra byte
                0x00, 0x5a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x60, 0x4f, 0x00, 0x00, 0x9c, 0x08,
                0x12, 0x00, 0x00
            ]),
            Err(InvalidIndexBinError::Length(17)),
            "Index must be completely parseable with no bytes leftover"
        );
    }

    #[test]
    fn test_validate_parsed_entry() {
        let graph_id = GraphId::try_from_components(0, 2540, 0).expect("Unable to create graph ID");
        let index_entries = parse_index_bin(INDEX_BIN_FIXTURE).expect("Unable to parse fixture");
        assert_eq!(index_entries[0].validated(), Ok((graph_id, 23040)));

        // Change the low byte of the first offset (23040 = 0x5a00) to make it unaligned
        let mut bytes = INDEX_BIN_FIXTURE.to_vec();
        bytes[0] = 0x01;
        let index_entries = parse_index_bin(&bytes).expect("Unable to parse modified fixture");
        assert_eq!(
            index_entries[0].validated(),
            Err(InvalidIndexBinError::Offset {
                graph_id,
                offset: 23041
            }),
            "Offsets must lie on a 512-byte boundary"
        );

        // Zero the first offset
        let mut bytes = INDEX_BIN_FIXTURE.to_vec();
        bytes[..8].fill(0);
        let index_entries = parse_index_bin(&bytes).expect("Unable to parse modified fixture");
        assert_eq!(
            index_entries[0].validated(),
            Err(InvalidIndexBinError::Offset {
                graph_id,
                offset: 0
            }),
            "Zero offsets are invalid"
        );

        // Set bit 25 of the first tile ID (the lowest bit of the feature index)
        let mut bytes = INDEX_BIN_FIXTURE.to_vec();
        bytes[11] |= 0x02;
        let index_entries = parse_index_bin(&bytes).expect("Unable to parse modified fixture");
        assert_eq!(
            index_entries[0].validated(),
            Err(InvalidIndexBinError::FeatureIndex(
                graph_id
                    .with_feature_index(1)
                    .expect("Unable to create graph ID")
            )),
            "Graph IDs with a non-zero feature index are invalid"
        );
    }

    #[test]
    fn test_parse_index_bin() {
        let index_entries = parse_index_bin(INDEX_BIN_FIXTURE).expect("Unable ta parse fixture");

        assert_eq!(index_entries.len(), 3);

        // All sizes were verified on disk from the original tiles when constructing the test

        // First entry
        assert_eq!(index_entries[0].offset.get(), 23040);
        assert_eq!(index_entries[0].graph_id().unwrap(), unsafe {
            GraphId::from_components_unchecked(0, 2540, 0)
        });
        assert_eq!(index_entries[0].size.get(), 1_181_852);

        // Second entry
        assert_eq!(index_entries[1].offset.get(), 1_206_784);
        assert_eq!(index_entries[1].graph_id().unwrap(), unsafe {
            GraphId::from_components_unchecked(0, 2628, 0)
        });
        assert_eq!(index_entries[1].size.get(), 92_104);

        // Third entry
        assert_eq!(index_entries[2].offset.get(), 1_300_480);
        assert_eq!(index_entries[2].graph_id().unwrap(), unsafe {
            GraphId::from_components_unchecked(0, 2629, 0)
        });
        assert_eq!(index_entries[2].size.get(), 1_809_704);
    }

    #[test]
    fn test_tile_index_bin_entry_new() {
        let graph_id = GraphId::try_from_components(0, 2540, 0).expect("Unable to create graph ID");
        let entry = TileIndexBinEntry::new(graph_id, 23040, 1_181_852)
            .expect("Unable to create index entry");

        assert_eq!(entry.offset.get(), 23040);
        assert_eq!(entry.graph_id().unwrap(), graph_id);
        assert_eq!(entry.size.get(), 1_181_852);
    }

    #[test]
    fn test_tile_index_bin_entry_new_invalid() {
        let graph_id = GraphId::try_from_components(0, 2540, 0).expect("Unable to create graph ID");

        let feature_id = graph_id
            .with_feature_index(1)
            .expect("Unable to create graph ID");
        assert_eq!(
            TileIndexBinEntry::new(feature_id, 512, 42),
            Err(InvalidIndexBinError::FeatureIndex(feature_id)),
            "Graph IDs with a non-zero feature index are invalid"
        );

        // Only constructible via the unchecked APIs,
        // but the constructor still shouldn't let it through silently.
        // SAFETY: The test does not use this ID to index into a tile.
        // It only gives the ID to the constructor, which must reject it.
        let reserved_id = unsafe { GraphId::from_id_unchecked(U64::new(1 << 46)) };
        assert_eq!(
            TileIndexBinEntry::new(reserved_id, 512, 42),
            Err(InvalidIndexBinError::ReservedBits(1 << 46)),
            "Graph IDs with reserved bits set are invalid"
        );

        for offset in [0, 256, 23041] {
            assert_eq!(
                TileIndexBinEntry::new(graph_id, offset, 42),
                Err(InvalidIndexBinError::Offset { graph_id, offset }),
                "Offsets must be non-zero and lie on a 512-byte boundary"
            );
        }
    }

    #[test]
    fn test_serialize_index_bin_empty() {
        assert_eq!(
            serialize_index_bin(&[]),
            Err(InvalidIndexBinError::Empty),
            "Empty indexes are invalid"
        );
    }

    #[test]
    fn test_serialize_index_bin() {
        // The same three entries as in the fixture (see test_parse_index_bin)
        let entries = [
            TileIndexBinEntry::new(
                GraphId::try_from_components(0, 2540, 0).expect("Unable to create graph ID"),
                23040,
                1_181_852,
            )
            .expect("Unable to create index entry"),
            TileIndexBinEntry::new(
                GraphId::try_from_components(0, 2628, 0).expect("Unable to create graph ID"),
                1_206_784,
                92_104,
            )
            .expect("Unable to create index entry"),
            TileIndexBinEntry::new(
                GraphId::try_from_components(0, 2629, 0).expect("Unable to create graph ID"),
                1_300_480,
                1_809_704,
            )
            .expect("Unable to create index entry"),
        ];

        let bytes = serialize_index_bin(&entries).expect("Unable to serialize index entries");

        assert_eq!(bytes, INDEX_BIN_FIXTURE);
    }

    proptest! {
        #[test]
        fn test_index_bin_roundtrip(
            // Levels are 3 bits, tile IDs are 22 bits, and offsets are counted in 512-byte blocks
            raw_entries in vec((0u8..=7, 0u64..1 << 22, 1u64..=u64::MAX / 512, 0u32..=u32::MAX), 1..64)
        ) {
            let entries: Vec<TileIndexBinEntry> = raw_entries
                .into_iter()
                .map(|(level, tile_id, offset_block, size)| {
                    let graph_id = GraphId::try_from_components(level, tile_id, 0)
                        .expect("All components are within their field limits");
                    TileIndexBinEntry::new(graph_id, offset_block * 512, size)
                        .expect("Offsets are non-zero and 512-aligned by construction")
                })
                .collect();

            let bytes = serialize_index_bin(&entries).expect("The entry list is non-empty");
            let parsed = parse_index_bin(bytes).expect("Serialized bytes always parse");

            prop_assert_eq!(parsed, entries.as_slice());
            // The read path accepts every entry that the write path creates
            for entry in parsed {
                prop_assert!(entry.validated().is_ok());
            }
        }
    }

    // Tragically, memory maps are not supported by Miri!
    #[cfg(not(miri))]
    #[test]
    fn test_get_tile() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles.tar");
        let provider =
            TarballTileProvider::new_readonly(path).expect("Unable to init tile provider");
        let graph_id = GraphId::try_from_components(0, 3015, 0).expect("Unable to create graph ID");
        let tile_pointer = provider
            .get_pointer_for_tile_containing(graph_id)
            .expect("Unable to get tile");
        let tile_bytes = unsafe { tile_pointer.as_tile_bytes() };
        let tile = GraphTileView::try_from(tile_bytes).expect("Unable to deserialize tile");

        // Minimally test that we got the correct tile
        assert_eq!(tile.header().graph_id(), graph_id);
        assert_eq!(tile.header().graph_id().value(), graph_id.value());
    }

    // Tragically, memory maps are not supported by Miri!
    #[cfg(not(miri))]
    #[test]
    fn test_tiles_are_identical_to_directory() {
        // This test uses the directory tile provider as an oracle
        // to make sure the tarball reader is working as expected.
        // Probably goes without saying, but the tarball was created using
        // valhalla_build_extract from the andorra-tiles directory.

        // Indexes are all intentionally non-zero to test that the provider impl fetches based on the base ID
        let tile_ids = &[
            GraphId::try_from_components(0, 3015, 1).expect("Unable to create graph ID"),
            GraphId::try_from_components(1, 47_701, 1).expect("Unable to create graph ID"),
            GraphId::try_from_components(2, 762_485, 1).expect("Unable to create graph ID"),
            GraphId::try_from_components(2, 762_486, 1).expect("Unable to create graph ID"),
            GraphId::try_from_components(2, 763_925, 1).expect("Unable to create graph ID"),
            GraphId::try_from_components(2, 763_926, 1).expect("Unable to create graph ID"),
            GraphId::try_from_components(2, 763_927, 1).expect("Unable to create graph ID"),
        ];

        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles");
        let directory_provider =
            DirectoryGraphTileProvider::new(base, NonZeroUsize::new(1).unwrap());

        let tarball_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles.tar");
        let tarball_provider = TarballTileProvider::new_readonly(tarball_path.clone())
            .expect("Unable to init tile provider");
        let immutable_tarball_provider = unsafe {
            TarballTileProvider::new_immutable(tarball_path).expect("Unable to init tile provider")
        };

        for graph_id in tile_ids {
            let directory_tile = directory_provider
                .get_handle_for_tile_containing(*graph_id)
                .expect("Unable to get tile");
            let tarball_tile_pointer = tarball_provider
                .get_pointer_for_tile_containing(*graph_id)
                .expect("Unable to get tile");
            let tarball_tile_bytes = unsafe { tarball_tile_pointer.as_tile_bytes() };

            assert_eq!(directory_tile.borrow_owner(), tarball_tile_bytes);

            let immutable_tarball_tile_pointer = immutable_tarball_provider
                .get_handle_for_tile_containing(*graph_id)
                .expect("Unable to get tile");
            let immutable_tarball_tile_bytes = unsafe {
                immutable_tarball_tile_pointer
                    .borrow_owner()
                    .as_tile_bytes()
            };
            assert_eq!(directory_tile.borrow_owner(), immutable_tarball_tile_bytes);
        }
    }
}

/// Helpers for building tarball extracts in tests.
#[cfg(test)]
pub(super) mod test_support {
    use super::{INDEX_ENTRY_SIZE, INDEX_FILE_NAME, TileIndexBinEntry, serialize_index_bin};
    use crate::GraphId;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tar::{Archive, Builder, Header};

    /// The tiles of the `andorra-tiles` fixture.
    pub(in crate::tile_provider) fn fixture_tile_ids() -> Vec<GraphId> {
        [
            (0, 3015),
            (1, 47_701),
            (2, 762_485),
            (2, 762_486),
            (2, 763_925),
            (2, 763_926),
            (2, 763_927),
        ]
        .into_iter()
        .map(|(level, tile_id)| {
            GraphId::try_from_components(level, tile_id, 0).expect("valid tile ID")
        })
        .collect()
    }

    /// Reads a tile of the `andorra-tiles` fixture.
    pub(in crate::tile_provider) fn fixture_tile(graph_id: GraphId) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles")
            .join(graph_id.file_path("gph").expect("valid tile ID"));
        std::fs::read(path).expect("Unable to read fixture tile")
    }

    fn append(builder: &mut Builder<Vec<u8>>, path: &Path, data: &[u8]) {
        let mut header = Header::new_ustar();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder
            .append_data(&mut header, path, data)
            .expect("Unable to append to tarball");
    }

    /// Builds an extract in the layout of `valhalla_build_extract`:
    /// `index.bin` first, then each tile, in the given order.
    ///
    /// Each tile is listed in the index under the given ID,
    /// whatever its bytes say.
    pub(in crate::tile_provider) fn build_extract(tiles: &[(GraphId, Vec<u8>)]) -> Vec<u8> {
        build_extract_with_first_entry(None, tiles)
    }

    /// Like [`build_extract`], but with an extra entry before the index.
    pub(in crate::tile_provider) fn build_extract_with_first_entry(
        first: Option<(&str, &[u8])>,
        tiles: &[(GraphId, Vec<u8>)],
    ) -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        if let Some((name, data)) = first {
            append(&mut builder, Path::new(name), data);
        }
        let index_len = tiles.len() * INDEX_ENTRY_SIZE;
        append(
            &mut builder,
            Path::new(INDEX_FILE_NAME),
            &vec![0u8; index_len],
        );
        for (graph_id, data) in tiles {
            append(
                &mut builder,
                &graph_id.file_path("gph").expect("valid tile ID"),
                data,
            );
        }
        let mut bytes = builder.into_inner().expect("Unable to finish tarball");

        // Fill in the index with the actual data offsets (as `write_index_to_tar` does)
        let mut index_pos = None;
        let mut entries = Vec::new();
        let mut archive = Archive::new(bytes.as_slice());
        for entry in archive.entries().expect("Unable to read tarball") {
            let entry = entry.expect("Unable to read entry");
            let path = entry.path().expect("Unable to read path").into_owned();
            if path == Path::new(INDEX_FILE_NAME) {
                index_pos = Some(entry.raw_file_position());
            } else if index_pos.is_some() {
                let (graph_id, _) = &tiles[entries.len()];
                entries.push(
                    TileIndexBinEntry::new(
                        *graph_id,
                        entry.raw_file_position(),
                        u32::try_from(entry.size()).expect("small tile"),
                    )
                    .expect("valid entry"),
                );
            }
        }
        let index_pos = usize::try_from(index_pos.expect("index entry")).expect("small tarball");
        if !entries.is_empty() {
            let index = serialize_index_bin(&entries).expect("non-empty index");
            bytes[index_pos..index_pos + index_len].copy_from_slice(index);
        }
        bytes
    }

    /// Rewrites the size of the first tar entry (and its header checksum).
    pub(in crate::tile_provider) fn set_first_entry_size(bytes: &mut [u8], size: u64) {
        let mut header = Header::new_ustar();
        header.as_mut_bytes().copy_from_slice(&bytes[..512]);
        header.set_size(size);
        header.set_cksum();
        bytes[..512].copy_from_slice(header.as_bytes());
    }

    /// A temporary file that is removed when dropped.
    pub(in crate::tile_provider) struct TempFile(PathBuf);

    impl TempFile {
        pub(in crate::tile_provider) fn new(bytes: &[u8]) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "valinor-graphtile-test-{}-{}.tar",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&path, bytes).expect("Unable to write temporary file");
            Self(path)
        }

        pub(in crate::tile_provider) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

// Tragically, memory maps are not supported by Miri!
#[cfg(all(test, not(miri)))]
mod extract_tests {
    use super::test_support::{
        TempFile, build_extract, build_extract_with_first_entry, fixture_tile, fixture_tile_ids,
        set_first_entry_size,
    };
    use super::*;
    use crate::graph_tile::GraphTile;
    use crate::tile_provider::{GraphTileProviderErrorKind, TileAvailability};
    use geo::{Coord, Point};
    use proptest::prelude::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// The tile left out of the partial extract.
    fn missing_tile() -> GraphId {
        GraphId::try_from_components(2, 763_926, 0).expect("valid tile ID")
    }

    fn partial_extract() -> TempFile {
        let tiles: Vec<_> = fixture_tile_ids()
            .into_iter()
            .filter(|id| *id != missing_tile())
            .map(|id| (id, fixture_tile(id)))
            .collect();
        TempFile::new(&build_extract(&tiles))
    }

    fn fixture_tarball_bytes() -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles.tar");
        std::fs::read(path).expect("Unable to read fixture")
    }

    #[test]
    fn build_extract_matches_fixture_layout() {
        // The helper must produce extracts that the provider reads the same way as
        // the fixture built by valhalla_build_extract.
        let tiles: Vec<_> = fixture_tile_ids()
            .into_iter()
            .map(|id| (id, fixture_tile(id)))
            .collect();
        let file = TempFile::new(&build_extract(&tiles));
        let built = TarballTileProvider::new_readonly(file.path()).expect("valid extract");
        let fixture = TarballTileProvider::new_readonly(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("andorra-tiles.tar"),
        )
        .expect("valid extract");
        assert_eq!(built.tile_count(), fixture.tile_count());
        for (id, bytes) in &tiles {
            let pointer = built.get_pointer_for_tile_containing(*id).expect("tile");
            // SAFETY: The test owns the file, and nothing mutates it.
            assert_eq!(unsafe { pointer.as_tile_bytes() }, bytes.as_slice());
        }
    }

    #[test]
    fn partial_extract_reports_missing_tiles() {
        let file = partial_extract();
        let provider = TarballTileProvider::new_readonly(file.path()).expect("valid extract");
        // SAFETY: The test owns the file, and nothing mutates it.
        let immutable =
            unsafe { TarballTileProvider::new_immutable(file.path()) }.expect("valid extract");

        let feature = missing_tile().with_feature_index(42).expect("valid ID");
        assert!(!provider.contains_tile(feature).unwrap());
        assert!(!immutable.contains_tile(feature).unwrap());
        for result in [
            provider.get_handle_for_tile_containing(feature).map(|_| ()),
            provider.with_tile_containing(feature, |_| ()),
            immutable
                .get_handle_for_tile_containing(feature)
                .map(|_| ()),
            immutable.with_tile_containing(feature, |_| ()),
        ] {
            match result {
                Err(GraphTileProviderError::TileNotAvailable(id)) => {
                    assert_eq!(id, missing_tile());
                }
                other => panic!("Expected the tile to be unavailable, got {other:?}"),
            }
        }
        assert!(matches!(
            provider.get_pointer_for_tile_containing(feature),
            Err(GraphTileProviderError::TileNotAvailable(_))
        ));

        // Everything else is still there
        for id in fixture_tile_ids() {
            if id != missing_tile() {
                assert!(provider.contains_tile(id).unwrap());
                provider.get_handle_for_tile_containing(id).expect("tile");
            }
        }

        // The tile is not a candidate for searches
        let tiles = provider
            .tiles_within_radius(Point::new(1.6, 42.55), 20_000.0)
            .expect("valid search");
        assert!(!tiles.contains(&missing_tile()));
        assert!(!tiles.is_empty());
    }

    /// Edges in the partial extract that lead into the missing tile,
    /// with a shape coordinate that lies in the edge's own tile.
    fn boundary_edges(provider: &TarballTileProvider<false, false>) -> Vec<(GraphId, Coord<f64>)> {
        let mut out = Vec::new();
        for tile_id in fixture_tile_ids() {
            if tile_id == missing_tile() {
                continue;
            }
            let tile = provider
                .get_handle_for_tile_containing(tile_id)
                .expect("tile");
            let sw = tile.header().sw_corner();
            let size = STANDARD_LEVELS[usize::from(tile_id.level())]
                .tiling_system
                .tile_size;
            for (idx, edge) in tile.directed_edges().iter().enumerate() {
                if edge.end_node_id().tile_base_id() != missing_tile() || edge.length() == 0 {
                    continue;
                }
                let shape: Vec<Coord<f32>> = tile
                    .get_edge_info(edge)
                    .expect("edge info")
                    .decode_raw_shape()
                    .expect("shape");
                let inside = shape
                    .iter()
                    .find(|c| c.x >= sw.x && c.x < sw.x + size && c.y >= sw.y && c.y < sw.y + size);
                if let Some(c) = inside {
                    let edge_id = tile_id.with_feature_index(idx as u64).expect("valid ID");
                    out.push((
                        edge_id,
                        Coord {
                            x: f64::from(c.x),
                            y: f64::from(c.y),
                        },
                    ));
                }
            }
        }
        out
    }

    #[test]
    fn radius_searches_skip_missing_tiles() {
        let file = partial_extract();
        let partial = TarballTileProvider::new_readonly(file.path()).expect("valid extract");
        let full = TarballTileProvider::new_readonly(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("andorra-tiles.tar"),
        )
        .expect("valid extract");

        let boundary = boundary_edges(&partial);
        assert!(
            boundary.len() > 10,
            "The fixture should have edges crossing into the missing tile"
        );

        // A boundary edge's opposing edge is in the missing tile
        let (edge_id, _) = boundary[0];
        let tile = partial
            .get_handle_for_tile_containing(edge_id)
            .expect("tile");
        assert!(matches!(
            partial.get_opposing_edge_id(edge_id, &tile),
            Err(GraphTileProviderError::TileNotAvailable(id)) if id == missing_tile()
        ));
        assert_eq!(
            partial
                .get_opposing_edge_id(edge_id, &tile)
                .if_available()
                .expect("no other error"),
            None
        );

        let mut found_boundary_edges = 0;
        for (edge_id, coord) in boundary.iter().step_by(boundary.len() / 10) {
            let center = Point::from(*coord);
            let radius = 250.0;

            let partial_edges = partial
                .edges_within_radius(|_| true, center, radius)
                .expect("valid search")
                .collect::<Result<Vec<_>, _>>()
                .expect("Searching a partial extract must not fail");
            let full_edges: HashSet<_> = full
                .edges_within_radius(|_| true, center, radius)
                .expect("valid search")
                .map(|r| r.expect("search").graph_id())
                .collect();

            for result in &partial_edges {
                assert_ne!(result.graph_id().tile_base_id(), missing_tile());
                assert!(full_edges.contains(&result.graph_id()));
                assert!(result.approx_distance() <= radius);
            }
            if partial_edges.iter().any(|r| r.graph_id() == *edge_id) {
                found_boundary_edges += 1;
            }

            let partial_nodes = partial
                .nodes_within_radius(|_| true, center, radius)
                .expect("valid search")
                .collect::<Result<Vec<_>, _>>()
                .expect("Searching a partial extract must not fail");
            let full_nodes: HashSet<_> = full
                .nodes_within_radius(|_| true, center, radius)
                .expect("valid search")
                .map(|r| r.expect("search").graph_id())
                .collect();
            for result in &partial_nodes {
                assert_ne!(result.graph_id().tile_base_id(), missing_tile());
                assert!(full_nodes.contains(&result.graph_id()));
                result.node_info().expect("node in tile");
            }
        }
        assert!(
            found_boundary_edges > 0,
            "The searches should find the edges that lead out of the extract"
        );
    }

    #[test]
    fn truncated_extract_fails_to_open() {
        let bytes = fixture_tarball_bytes();
        let full = TarballTileProvider::new_readonly(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("andorra-tiles.tar"),
        )
        .expect("valid extract");
        let data_end = full
            .tile_index
            .values()
            .map(|o| o.offset + u64::from(o.size))
            .max()
            .expect("tiles");
        let data_end = usize::try_from(data_end).expect("small fixture");

        // Truncated within the last tile
        let file = TempFile::new(&bytes[..data_end - 1]);
        match TarballTileProvider::new_readonly(file.path()) {
            Err(GraphTileProviderError::InvalidTarball(InvalidTarballError::OutOfBounds {
                what: TarballEntry::Tile(_),
                ..
            })) => {}
            Err(e) => panic!("Unexpected error: {e}"),
            Ok(_) => panic!("A truncated extract must not open"),
        }

        // Truncated anywhere else, down to nothing
        for len in [data_end / 2, 4096, 1024, 600, 512, 100, 0] {
            let file = TempFile::new(&bytes[..len]);
            let Err(err) = TarballTileProvider::new_readonly(file.path()) else {
                panic!("An extract truncated to {len} bytes must not open");
            };
            assert_eq!(err.kind(), GraphTileProviderErrorKind::InvalidData, "{err}");
            // SAFETY: The test owns the file, and nothing mutates it.
            assert!(unsafe { TarballTileProvider::new_immutable(file.path()) }.is_err());
            assert!(crate::tile_provider::TrafficTileProvider::new_readonly(file.path()).is_err());
        }

        let file = TempFile::new(&[]);
        assert!(matches!(
            TarballTileProvider::new_readonly(file.path()),
            Err(GraphTileProviderError::InvalidTarball(
                InvalidTarballError::NoEntries
            ))
        ));
    }

    #[test]
    fn index_must_come_first() {
        let tiles = vec![(missing_tile(), fixture_tile(missing_tile()))];
        let bytes = build_extract_with_first_entry(Some(("README", b"hello")), &tiles);
        let file = TempFile::new(&bytes);
        match TarballTileProvider::new_readonly(file.path()) {
            Err(GraphTileProviderError::InvalidTarball(InvalidTarballError::IndexNotFirst(
                name,
            ))) => assert_eq!(name, "README"),
            Err(e) => panic!("Unexpected error: {e}"),
            Ok(_) => panic!("The index must be the first entry"),
        }
    }

    #[test]
    fn index_size_is_checked_before_reading() {
        let tiles = vec![(missing_tile(), fixture_tile(missing_tile()))];
        let bytes = build_extract(&tiles);

        // Larger than any valid index (without this check, we'd try to allocate 4 GiB)
        let mut huge = bytes.clone();
        set_first_entry_size(&mut huge, 1 << 32);
        let file = TempFile::new(&huge);
        assert!(matches!(
            TarballTileProvider::new_readonly(file.path()),
            Err(GraphTileProviderError::InvalidTarball(
                InvalidTarballError::IndexTooLarge { size, .. }
            )) if size == 1 << 32
        ));

        // Plausible, but larger than the file
        let mut too_long = bytes;
        set_first_entry_size(&mut too_long, 16 * 1024 * 1024);
        let file = TempFile::new(&too_long);
        assert!(matches!(
            TarballTileProvider::new_readonly(file.path()),
            Err(GraphTileProviderError::InvalidTarball(
                InvalidTarballError::OutOfBounds {
                    what: TarballEntry::Index,
                    ..
                }
            ))
        ));
    }

    #[test]
    fn duplicate_index_entries_are_rejected() {
        let id = missing_tile();
        let tiles = vec![(id, fixture_tile(id)), (id, fixture_tile(id))];
        let file = TempFile::new(&build_extract(&tiles));
        assert!(matches!(
            TarballTileProvider::new_readonly(file.path()),
            Err(GraphTileProviderError::InvalidIndexBin(
                InvalidIndexBinError::DuplicateTile(dup)
            )) if dup == id
        ));
    }

    #[test]
    fn tile_stored_under_the_wrong_id_is_detected() {
        let a = GraphId::try_from_components(2, 762_485, 0).expect("valid tile ID");
        let b = GraphId::try_from_components(2, 762_486, 0).expect("valid tile ID");
        // The index lists a's bytes under b's ID, and vice versa
        let tiles = vec![(a, fixture_tile(b)), (b, fixture_tile(a))];
        let file = TempFile::new(&build_extract(&tiles));
        let provider = TarballTileProvider::new_readonly(file.path()).expect("valid index");
        assert!(matches!(
            provider.get_handle_for_tile_containing(a.with_feature_index(3).expect("valid ID")),
            Err(GraphTileProviderError::TileIdMismatch { expected, actual }) if expected == a && actual == b
        ));
        assert!(matches!(
            provider.with_tile_containing(b, |_| ()),
            Err(GraphTileProviderError::TileIdMismatch { expected, actual }) if expected == b && actual == a
        ));
        // SAFETY: The test owns the file, and nothing mutates it.
        assert!(matches!(
            unsafe { TarballTileProvider::new_immutable(file.path()) },
            Err(GraphTileProviderError::TileIdMismatch { .. })
        ));
    }

    #[test]
    fn corrupt_tile_is_an_error() {
        let id = GraphId::try_from_components(2, 763_927, 0).expect("valid tile ID");
        // Too short to even hold a header
        let tiles = vec![(id, vec![0u8; 100])];
        let file = TempFile::new(&build_extract(&tiles));
        let provider = TarballTileProvider::new_readonly(file.path()).expect("valid index");
        assert!(matches!(
            provider.get_handle_for_tile_containing(id),
            Err(GraphTileProviderError::InvalidTile { graph_id, .. }) if graph_id == id
        ));
        assert!(matches!(
            provider
                .get_handle_for_tile_containing(id)
                .map(|_| ())
                .if_available(),
            Err(GraphTileProviderError::InvalidTile { .. })
        ));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Corrupting the tar header or the index never panics
        /// (the extract either fails to open, or its tiles load or fail with errors).
        #[test]
        fn corrupt_header_or_index_never_panics(
            corruptions in prop::collection::vec((0usize..1024, any::<u8>()), 1..8)
        ) {
            let ids = [
                GraphId::try_from_components(2, 763_927, 0).expect("valid tile ID"),
                GraphId::try_from_components(2, 762_486, 0).expect("valid tile ID"),
            ];
            let tiles: Vec<_> = ids.iter().map(|id| (*id, fixture_tile(*id))).collect();
            let mut bytes = build_extract(&tiles);
            for (pos, value) in corruptions {
                bytes[pos] = value;
            }
            let file = TempFile::new(&bytes);
            if let Ok(provider) = TarballTileProvider::new_readonly(file.path()) {
                let tile_ids: Vec<_> = provider.tile_ids().copied().collect();
                for id in tile_ids {
                    let _ = provider.get_handle_for_tile_containing(id);
                }
                for id in ids {
                    let _ = provider.get_handle_for_tile_containing(id);
                }
            }
        }
    }
}
