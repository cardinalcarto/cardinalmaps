//! # Graph Tile Handles
//!
//! This module contains several variations of owned handles to graph tile data.
//! These can be used in cases where longer-lived access is needed.
//! Each offers different performance, flexibility, and safety tradeoffs.

use super::TileSections;
use super::{
    AccessRestriction, Admin, DirectedEdge, DirectedEdgeExt, DiscretizedBoundingCircle, EdgeInfo,
    GraphTile, GraphTileDecodingError, GraphTileHeader, GraphTileView, LookupError, NodeInfo,
    NodeTransition, OpposingEdgeIndex,
};
use crate::{Access, GraphId};
use enumset::EnumSet;
use memmap2::MmapRaw;
use self_cell::self_cell;
use std::sync::Arc;
use thiserror::Error;
use zerocopy::FromBytes;

pub trait GraphTileHandle {
    fn tile_view(&'_ self) -> GraphTileView<'_>;
}

self_cell! {
    /// A read-only view of a Valhalla graph tile.
    ///
    /// The bytes are fully owned by the handle, and access is infallible after construction.
    pub struct OwnedGraphTileHandle {
        owner: Vec<u8>,
        #[covariant]
        dependent: GraphTileView,
    }
}

impl TryFrom<Vec<u8>> for OwnedGraphTileHandle {
    type Error = GraphTileDecodingError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        OwnedGraphTileHandle::try_new(value, |data| GraphTileView::try_from(data.as_slice()))
    }
}

impl GraphTile for OwnedGraphTileHandle {
    #[inline]
    fn graph_id(&self) -> GraphId {
        self.borrow_dependent().graph_id()
    }

    #[inline]
    fn may_contain_id(&self, id: GraphId) -> bool {
        self.borrow_dependent().may_contain_id(id)
    }

    #[inline]
    fn header(&self) -> &GraphTileHeader {
        self.borrow_dependent().header()
    }

    #[inline]
    fn get_node(&self, id: GraphId) -> Result<&NodeInfo, LookupError> {
        self.borrow_dependent().get_node(id)
    }

    #[inline]
    fn get_outbound_edges_from_node(&self, node_info: &NodeInfo) -> &[DirectedEdge] {
        self.borrow_dependent()
            .get_outbound_edges_from_node(node_info)
    }

    #[inline]
    fn get_transitions(&self, node: &NodeInfo) -> &[NodeTransition] {
        self.borrow_dependent().get_transitions(node)
    }

    #[inline]
    fn get_directed_edge(&self, id: GraphId) -> Result<&DirectedEdge, LookupError> {
        self.borrow_dependent().get_directed_edge(id)
    }

    #[inline]
    fn get_opp_edge_index(&self, graph_id: GraphId) -> Result<OpposingEdgeIndex, LookupError> {
        self.borrow_dependent().get_opp_edge_index(graph_id)
    }

    #[inline]
    fn get_ext_directed_edge(&self, id: GraphId) -> Result<&DirectedEdgeExt, LookupError> {
        self.borrow_dependent().get_ext_directed_edge(id)
    }

    #[inline]
    fn get_access_restrictions(
        &self,
        directed_edge_index: u32,
        access_modes: EnumSet<Access>,
    ) -> Vec<&AccessRestriction> {
        self.borrow_dependent()
            .get_access_restrictions(directed_edge_index, access_modes)
    }

    #[inline]
    fn get_predicted_speed(
        &self,
        directed_edge_index: usize,
        seconds_from_start_of_week: u32,
    ) -> Option<f32> {
        self.borrow_dependent()
            .get_predicted_speed(directed_edge_index, seconds_from_start_of_week)
    }

    #[inline]
    fn get_edge_info(
        &self,
        directed_edge: &DirectedEdge,
    ) -> Result<EdgeInfo<'_>, GraphTileDecodingError> {
        self.borrow_dependent().get_edge_info(directed_edge)
    }

    #[inline]
    fn directed_edges(&self) -> &[DirectedEdge] {
        self.borrow_dependent().directed_edges()
    }

    #[inline]
    fn nodes(&self) -> &[NodeInfo] {
        self.borrow_dependent().nodes
    }

    #[inline]
    fn admins(&self) -> &[Admin] {
        self.borrow_dependent().admins()
    }

    #[inline]
    fn edges_in_bin(&self, bin_index: usize) -> &[GraphId] {
        self.borrow_dependent().edges_in_bin(bin_index)
    }

    #[inline]
    fn bounding_circles_in_bin(&self, bin_index: usize) -> &[DiscretizedBoundingCircle] {
        self.borrow_dependent().bounding_circles_in_bin(bin_index)
    }

    #[inline]
    fn bin_index_xy(&self, x: usize, y: usize) -> Option<usize> {
        self.borrow_dependent().bin_index_xy(x, y)
    }

    // Variable-length sections
    #[inline]
    fn sections(&self) -> TileSections<'_> {
        self.borrow_dependent().sections()
    }
}

self_cell! {
    /// A read-only view of a graph tile backed by a memory-mapped pointer.
    pub struct MmapGraphTileHandle {
        owner: MmapTilePointer,
        #[covariant]
        dependent: GraphTileView,
    }
}

impl GraphTile for MmapGraphTileHandle {
    #[inline]
    fn graph_id(&self) -> GraphId {
        self.borrow_dependent().graph_id()
    }

    #[inline]
    fn may_contain_id(&self, id: GraphId) -> bool {
        self.borrow_dependent().may_contain_id(id)
    }

    #[inline]
    fn header(&self) -> &GraphTileHeader {
        self.borrow_dependent().header()
    }

    #[inline]
    fn get_node(&self, id: GraphId) -> Result<&NodeInfo, LookupError> {
        self.borrow_dependent().get_node(id)
    }

    #[inline]
    fn get_outbound_edges_from_node(&self, node_info: &NodeInfo) -> &[DirectedEdge] {
        self.borrow_dependent()
            .get_outbound_edges_from_node(node_info)
    }

    #[inline]
    fn get_transitions(&self, node: &NodeInfo) -> &[NodeTransition] {
        self.borrow_dependent().get_transitions(node)
    }

    #[inline]
    fn get_directed_edge(&self, id: GraphId) -> Result<&DirectedEdge, LookupError> {
        self.borrow_dependent().get_directed_edge(id)
    }

    #[inline]
    fn get_opp_edge_index(&self, graph_id: GraphId) -> Result<OpposingEdgeIndex, LookupError> {
        self.borrow_dependent().get_opp_edge_index(graph_id)
    }

    #[inline]
    fn get_ext_directed_edge(&self, id: GraphId) -> Result<&DirectedEdgeExt, LookupError> {
        self.borrow_dependent().get_ext_directed_edge(id)
    }

    #[inline]
    fn get_access_restrictions(
        &self,
        directed_edge_index: u32,
        access_modes: EnumSet<Access>,
    ) -> Vec<&AccessRestriction> {
        self.borrow_dependent()
            .get_access_restrictions(directed_edge_index, access_modes)
    }

    #[inline]
    fn get_predicted_speed(
        &self,
        directed_edge_index: usize,
        seconds_from_start_of_week: u32,
    ) -> Option<f32> {
        self.borrow_dependent()
            .get_predicted_speed(directed_edge_index, seconds_from_start_of_week)
    }

    #[inline]
    fn get_edge_info(
        &self,
        directed_edge: &DirectedEdge,
    ) -> Result<EdgeInfo<'_>, GraphTileDecodingError> {
        self.borrow_dependent().get_edge_info(directed_edge)
    }

    #[inline]
    fn directed_edges(&self) -> &[DirectedEdge] {
        self.borrow_dependent().directed_edges()
    }

    #[inline]
    fn nodes(&self) -> &[NodeInfo] {
        self.borrow_dependent().nodes
    }

    #[inline]
    fn admins(&self) -> &[Admin] {
        self.borrow_dependent().admins()
    }

    #[inline]
    fn edges_in_bin(&self, bin_index: usize) -> &[GraphId] {
        self.borrow_dependent().edges_in_bin(bin_index)
    }

    #[inline]
    fn bounding_circles_in_bin(&self, bin_index: usize) -> &[DiscretizedBoundingCircle] {
        self.borrow_dependent().bounding_circles_in_bin(bin_index)
    }

    #[inline]
    fn bin_index_xy(&self, x: usize, y: usize) -> Option<usize> {
        self.borrow_dependent().bin_index_xy(x, y)
    }

    // Variable-length sections
    #[inline]
    fn sections(&self) -> TileSections<'_> {
        self.borrow_dependent().sections()
    }
}

impl MmapGraphTileHandle {
    /// Creates a new graph tile handle backed by the memory mapped tile pointer.
    ///
    /// # Safety
    ///
    /// The mapped memory must not be mutated, truncated, or unmapped
    /// for the lifetime of the handle.
    /// (A range that does not fit in the map is read as an empty, invalid tile,
    /// and invalid tile data is a regular decoding error.)
    ///
    /// # Errors
    ///
    /// Fails if the mapped bytes are not a valid graph tile
    /// (see [`GraphTileView::try_from`]).
    ///
    /// # Performance
    ///
    /// This does not perform any copies,
    /// but it does validate the tile (see [`GraphTileView::try_from`]).
    pub unsafe fn try_from(tile_pointer: MmapTilePointer) -> Result<Self, GraphTileDecodingError> {
        MmapGraphTileHandle::try_new(tile_pointer, |ptr| {
            // SAFETY: The caller guarantees that the mapped memory is not mutated, truncated,
            // or unmapped while the handle (which owns the pointer) is alive.
            let tile_bytes = unsafe { ptr.as_tile_bytes() };
            GraphTileView::try_from(tile_bytes)
        })
    }
}

/// An encapsulating type that can be used to get the actual bytes for a tile.
///
/// This provides a single type that can "own" the backing bytes for tile memory,
/// which we leverage in the `self_cell` to create a zero-copy view.
pub struct MmapTilePointer {
    /// A handle to the memory map that can be shared across threads.
    pub(crate) mmap: Arc<MmapRaw>,
    /// The offsets of the represented data within the memory map.
    pub(crate) offsets: TileOffset,
}

/// The ways in which a [`MmapTilePointer`] can fail to describe a usable range.
///
/// Tile providers check their ranges when they load the index,
/// so these indicate a bug rather than corrupt data.
/// They are errors rather than panics so that a bug can't take down an offline app.
#[derive(Debug, Error, Copy, Clone, PartialEq, Eq)]
pub enum MmapAccessError {
    #[error(
        "access {size} bytes at offset {offset}: the range runs past the end of the {map_len}-byte map"
    )]
    OutOfBounds {
        offset: u64,
        size: u32,
        map_len: usize,
    },
    #[error("access a {size}-byte range as a {type_size}-byte value: the sizes differ")]
    SizeMismatch { size: u32, type_size: usize },
    #[error("access offset {offset} as a value aligned to {align} bytes: it is misaligned")]
    Misaligned { offset: u64, align: usize },
}

impl MmapTilePointer {
    /// The byte offset of the range within the map,
    /// after checking that the whole range lies within the map.
    #[inline]
    fn checked_start(&self) -> Result<usize, MmapAccessError> {
        let map_len = self.mmap.len();
        let out_of_bounds = MmapAccessError::OutOfBounds {
            offset: self.offsets.offset,
            size: self.offsets.size,
            map_len,
        };
        let start = usize::try_from(self.offsets.offset).map_err(|_| out_of_bounds)?;
        let end = start
            .checked_add(self.offsets.size as usize)
            .ok_or(out_of_bounds)?;
        if end > map_len {
            return Err(out_of_bounds);
        }
        Ok(start)
    }

    /// The byte offset of the range within the map,
    /// after checking that it holds exactly one naturally aligned `T`.
    #[inline]
    fn checked_value_start<T>(&self) -> Result<usize, MmapAccessError> {
        if self.offsets.size as usize != size_of::<T>() {
            return Err(MmapAccessError::SizeMismatch {
                size: self.offsets.size,
                type_size: size_of::<T>(),
            });
        }
        let start = self.checked_start()?;
        // The map itself is page aligned, so this is the alignment of the address.
        if !(self.mmap.as_ptr() as usize + start).is_multiple_of(align_of::<T>()) {
            return Err(MmapAccessError::Misaligned {
                offset: self.offsets.offset,
                align: align_of::<T>(),
            });
        }
        Ok(start)
    }

    /// Returns a slice view over the data mapped in memory.
    ///
    /// The range is checked against the length of the map,
    /// and a range that does not fit returns an empty slice,
    /// which every reader of tile data rejects as truncated.
    /// (Tile providers check their ranges when they load the index,
    /// so this only guards against bugs.)
    ///
    /// # Safety
    ///
    /// The backing memory map must not be mutated during the lifetime of the returned slice.
    /// While some mutation of bytes may technically work,
    /// changing the values of memory underneath a slice is undefined behavior in Rust.
    ///
    /// Truncating the file after mapping it will cause SIGBUS or similar.
    #[inline]
    pub(crate) unsafe fn as_tile_bytes(&self) -> &[u8] {
        let Ok(start) = self.checked_start() else {
            return &[];
        };
        // SAFETY: The range lies within the map (checked above),
        // so the pointer arithmetic stays within one allocation,
        // and the caller guarantees that the memory is not mutated or unmapped
        // while the slice is alive.
        unsafe {
            core::slice::from_raw_parts(self.mmap.as_ptr().add(start), self.offsets.size as usize)
        }
    }

    /// Reads from the mapped range and interprets it as type `T`.
    ///
    /// The instance of `T` is created from a _bitwise_ copy of the data.
    /// This uses [`core::ptr::read_volatile`] under the hood to make sure the I/O actually happens.
    /// `T` must be [`FromBytes`], so every bit pattern is a valid `T`.
    ///
    /// # Errors
    ///
    /// Fails if the range is not exactly `size_of::<T>()` bytes,
    /// does not lie within the map, or is not naturally aligned for `T`.
    ///
    /// # Safety
    ///
    /// The approach taken here mirrors that of Valhalla,
    /// which I'm not sure is fundamentally sound,
    /// though it may be *practically* acceptable on x86_64 for the places it's used
    /// (traffic tile access via a shared memory map).
    ///
    /// The following will likely result in a SIGBUS:
    ///
    /// - Truncating the memory mapped file.
    /// - Remapping or resizing the map.
    ///
    /// Finally, while this does force a volatile read,
    /// NO ATTEMPT IS MADE AT SYNCHRONIZATION!
    /// This is, regrettably, how Valhalla currently does things.
    /// There should probably be atomics involved to synchronize writes,
    /// but that requires some internal Valhalla evolution,
    /// which would result in some breaking changes.
    ///
    /// As far as I can tell, this is *practically* acceptable if the writer
    /// is always storing values of a size that are guaranteed to be atomic
    /// by the underlying architecture, provided that:
    ///
    /// - No logical field ever crosses a _byte_ boundary, OR
    /// - All reads and writes are naturally aligned AND integer operations are atomic for that size
    ///   on the architecture AND no logical field crosses a boundary between two such larger width integers
    ///
    /// Concretely, this means that **writers must use a larger size integer type
    /// if fields can span across byte boundaries (as they do in traffic tiles)
    /// as the byte-level reads would permit tearing.**
    #[inline]
    pub(crate) unsafe fn read_volatile<T: FromBytes>(&self) -> Result<T, MmapAccessError> {
        let start = self.checked_value_start::<T>()?;
        // SAFETY: The range holds exactly one `T`, lies within the map, and is aligned for `T`
        // (all checked above), and every bit pattern is a valid `T`.
        // The caller upholds the conditions on concurrent writers described above.
        Ok(unsafe { self.mmap.as_ptr().add(start).cast::<T>().read_volatile() })
    }

    /// Writes the value to the mapped range.
    ///
    /// This will fail to compile if lock-free atomics are not supported for the current target
    /// for types of `size_of::<T>()`.
    /// This is statically checked at compile time.
    ///
    /// # Errors
    ///
    /// Fails if the range is not exactly `size_of::<T>()` bytes,
    /// does not lie within the map, or is not naturally aligned for `T`
    /// (no major platform guarantees atomicity for misaligned stores).
    ///
    /// # Safety
    ///
    /// The map must be writable.
    ///
    /// The following will likely result in a SIGBUS:
    ///
    /// - Truncating the memory mapped file.
    /// - Remapping or resizing the map.
    ///
    /// Language specs indicate that unsynchronized concurrent reads and writes are UB.
    /// By making reads and writes volatile, we mitigate one source of uncertainty,
    /// instructing the compiler that it can never optimize these away.
    /// However, shared memory maps have NO inherent synchronization.
    /// The check for lock-free atomics on the target for this value size _typically_ indicates
    /// that loads and stores **to memory** are atomic for naturally aligned pointers.
    /// This implies nothing about _ordering_, but it does protect against torn writes.
    #[inline]
    pub(crate) unsafe fn write_volatile<T>(&self, value: T) -> Result<(), MmapAccessError> {
        const {
            let has_atomic = match size_of::<T>() {
                1 => cfg!(target_has_atomic = "8"),
                2 => cfg!(target_has_atomic = "16"),
                4 => cfg!(target_has_atomic = "32"),
                8 => cfg!(target_has_atomic = "64"),
                16 => cfg!(target_has_atomic = "128"),
                _ => false,
            };
            assert!(
                has_atomic,
                "This type does not have a lock-free atomic support (it's probably too wide) on your current target."
            );
        };

        let start = self.checked_value_start::<T>()?;
        // SAFETY: The range holds exactly one `T`, lies within the map, and is aligned for `T`
        // (all checked above).
        // The caller guarantees that the map is writable,
        // and upholds the conditions on concurrent access described above.
        unsafe {
            self.mmap
                .as_mut_ptr()
                .add(start)
                .cast::<T>()
                .write_volatile(value);
        }
        Ok(())
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use memmap2::MmapOptions;

    fn pointer(len: usize, offset: u64, size: u32) -> MmapTilePointer {
        MmapTilePointer {
            mmap: Arc::new(MmapOptions::new().len(len).map_anon().expect("map").into()),
            offsets: TileOffset { offset, size },
        }
    }

    #[test]
    fn test_out_of_range_pointers_are_errors() {
        // SAFETY: The anonymous maps are private to the test and never mutated concurrently.
        unsafe {
            assert_eq!(pointer(64, 0, 64).as_tile_bytes().len(), 64);
            assert!(pointer(64, 1, 64).as_tile_bytes().is_empty());
            assert!(pointer(64, u64::MAX, 1).as_tile_bytes().is_empty());

            assert_eq!(pointer(64, 8, 8).read_volatile::<u64>(), Ok(0));
            assert_eq!(pointer(64, 8, 8).write_volatile(7u64), Ok(()));
            assert_eq!(
                pointer(64, 4, 8).read_volatile::<u64>(),
                Err(MmapAccessError::Misaligned {
                    offset: 4,
                    align: 8
                })
            );
            assert_eq!(
                pointer(64, 8, 4).read_volatile::<u64>(),
                Err(MmapAccessError::SizeMismatch {
                    size: 4,
                    type_size: 8
                })
            );
            assert_eq!(
                pointer(64, 64, 8).write_volatile(1u64),
                Err(MmapAccessError::OutOfBounds {
                    offset: 64,
                    size: 8,
                    map_len: 64
                })
            );
        }
    }

    #[test]
    fn test_write_then_read() {
        let map = pointer(64, 16, 8);
        // SAFETY: The anonymous map is private to the test and never mutated concurrently.
        unsafe {
            map.write_volatile(0x0123_4567_89ab_cdef_u64)
                .expect("in range");
            assert_eq!(map.read_volatile::<u64>(), Ok(0x0123_4567_89ab_cdef));
        }
    }
}

/// A tile offset, used for internal storage out of the parsed index.
#[derive(Copy, Clone)]
pub(crate) struct TileOffset {
    /// Byte offset from the beginning of the tar
    pub(crate) offset: u64,
    /// The size of the tile in bytes.
    pub(crate) size: u32,
}
