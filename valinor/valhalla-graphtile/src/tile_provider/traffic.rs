//! # Live traffic tile provider
//!
//! Reads (and optionally writes) live traffic speeds in a memory-mapped traffic extract,
//! as produced by `valhalla_build_extract --with-traffic`.
//!
//! A traffic extract has the same layout as a graph tile extract
//! (see [`TarballTileProvider`]),
//! but each tile is a [`TrafficTileHeader`] followed by one 64-bit [`TrafficSpeed`]
//! per directed edge of the matching graph tile.

use crate::GraphId;
use crate::graph_tile::{
    LookupError,
    handles::{MmapTilePointer, TileOffset},
};
use crate::tile_provider::{GraphTileProviderError, TarballTileProvider};
use crate::traffic_tile::{TRAFFIC_TILE_VERSION, TrafficSpeed, TrafficTileHeader};
use std::path::Path;
use zerocopy::{LE, U64};

/// The size of a traffic tile header in bytes.
const HEADER_SIZE: usize = size_of::<TrafficTileHeader>();
/// The size of one speed record in bytes.
///
/// Speeds are read and written as native `u64` values (not as [`TrafficSpeed`]),
/// since only an aligned 64-bit access is a single load or store
/// ([`TrafficSpeed`] has an alignment of 1).
const SPEED_SIZE: usize = size_of::<u64>();

/// [`HEADER_SIZE`] as a `u32`.
#[expect(clippy::cast_possible_truncation, reason = "the header is 32 bytes")]
const HEADER_SIZE_U32: u32 = HEADER_SIZE as u32;
/// [`SPEED_SIZE`] as a `u32`.
#[expect(clippy::cast_possible_truncation, reason = "a speed record is 8 bytes")]
const SPEED_SIZE_U32: u32 = SPEED_SIZE as u32;

const _: () = {
    assert!(SPEED_SIZE == size_of::<TrafficSpeed>());
    // Tiles start on a 512-byte tar block boundary, and the map starts on a page boundary,
    // so speed records are naturally aligned as long as the header size keeps them so.
    assert!(
        HEADER_SIZE.is_multiple_of(align_of::<u64>()),
        "speed records must be 8-byte aligned"
    );
};

/// The traffic tarball tile provider.
///
/// This provides an interface to querying and (in some cases)
/// writing data.
///
/// Every access is bounds-checked against the tile size recorded in the index,
/// so a corrupt header or a truncated extract results in an error, not an out-of-bounds read.
///
/// # Safety
///
/// This structure is entirely safe in read-only mode when it is the only consumer of the `mmap`,
/// and can guarantee that the file/shared memory are not modified externally.
/// Otherwise, the situation gets a bit more complicated.
///
/// Whether this struct (and the backing memory map) support mutability is determined
/// by the value of the const generic parameter `MUT`.
/// When `MUT` is `false` methods enabling mutation are hidden.
///
/// Given the inherently unsafe nature of working with shared mutable memory this way
/// (a random process can modify your memory _at any time_),
/// we use exclusively volatile memory operations internally.
/// Unfortunately, Rust does not have a great way to ensure everything is as specified as we'd like,
/// and there is not clear consensus on what mix of atomics and volatile are the "correct" way
/// to do shared `mmap`'d files.
///
/// Speed records are always read and written with a single, naturally aligned 64-bit access.
/// The crate only builds for 64-bit targets,
/// and writing additionally requires lock-free 64-bit atomics (checked at compile time),
/// so these accesses cannot tear on any supported platform.
pub struct TrafficTileProvider<const MUT: bool> {
    tarball_tile_provider: TarballTileProvider<MUT, false>,
}

impl<const MUT: bool> TrafficTileProvider<MUT> {
    /// Creates a new traffic tile provider from an existing tarball extract.
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
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, GraphTileProviderError> {
        // SAFETY: CACHE=false, so `init` doesn't create any long-lived references
        // into the memory map.
        let tarball_tile_provider = unsafe { TarballTileProvider::init(path)? };
        Ok(Self {
            tarball_tile_provider,
        })
    }

    /// Gets speed information for the edge identified by `graph_id`.
    ///
    /// # Safety
    ///
    /// The extract may be modified while it is mapped only by writers that
    ///
    /// - never change a traffic tile header, the set of tiles, or their sizes, and
    /// - store each speed record with a single aligned 64-bit store
    ///   (as Valhalla and [`TrafficTileProvider::update_speed_for_edge`] do).
    ///
    /// The extract must never be truncated while it is mapped (this raises `SIGBUS`).
    /// See the [type-level documentation](TrafficTileProvider) for details.
    ///
    /// # Errors
    ///
    /// Fails with [`GraphTileProviderError::TileNotAvailable`] if the extract has no traffic tile
    /// for the edge,
    /// with [`GraphTileProviderError::Lookup`] if the edge index is out of range for the tile,
    /// and with an error if the traffic tile is malformed
    /// (an unsupported version, or too small for its header or its edge count).
    #[inline]
    pub unsafe fn get_speeds_for_edge(
        &self,
        graph_id: GraphId,
    ) -> Result<TrafficSpeed, GraphTileProviderError> {
        // SAFETY: The caller upholds the conditions on concurrent writers (see the method docs).
        let speed_pointer = unsafe { self.get_pointer_for_edge(graph_id)? };

        // SAFETY: The pointer was bounds-checked against the tile, which lies within the map,
        // and it is 8 bytes long and 8-byte aligned (see `get_pointer_for_edge`),
        // so the size and alignment assertions in `read_volatile` hold.
        // Writers store whole records with aligned 64-bit stores, so the load cannot tear.
        let raw: u64 = unsafe { speed_pointer.read_volatile() };
        // The record is stored in little-endian byte order.
        Ok(TrafficSpeed::from_bits(U64::<LE>::new(u64::from_le(raw))))
    }

    /// Gets a pointer structure for the speed record of the given edge.
    ///
    /// The pointer lies within the traffic tile,
    /// is [`SPEED_SIZE`] bytes long, and is aligned for a `u64`.
    ///
    /// # Safety
    ///
    /// The header is read with a volatile read,
    /// so the caller must uphold the conditions on concurrent writers
    /// described in [`TrafficTileProvider::get_speeds_for_edge`].
    unsafe fn get_pointer_for_edge(
        &self,
        graph_id: GraphId,
    ) -> Result<MmapTilePointer, GraphTileProviderError> {
        let base_id = graph_id.tile_base_id();
        let tile_pointer = self
            .tarball_tile_provider
            .get_pointer_for_tile_containing(graph_id)?;
        let tile_size = tile_pointer.offsets.size;
        let check_size = |required: u64| {
            if required > u64::from(tile_size) {
                Err(GraphTileProviderError::TruncatedTrafficTile {
                    graph_id: base_id,
                    size: tile_size,
                    required,
                })
            } else {
                Ok(())
            }
        };

        check_size(u64::from(HEADER_SIZE_U32))?;
        let header_pointer = MmapTilePointer {
            mmap: tile_pointer.mmap.clone(),
            offsets: TileOffset {
                // Same offset
                offset: tile_pointer.offsets.offset,
                size: HEADER_SIZE_U32,
            },
        };

        // SAFETY: The header lies within the tile (checked above),
        // which lies within the memory map (checked when the index was loaded).
        // `TrafficTileHeader` has an alignment of 1, so the alignment assertion holds.
        // TBH this probably isn't possible to make completely safe
        // given the current architecture of Valhalla and fundamental unsafety of shared memory maps.
        // However, Valhalla (at the time of this writing) has no mechanism for hot swapping
        // the underlying graph, which means we can assume the header will never change
        // for a given tile during the life of the program (the caller guarantees this).
        let header: TrafficTileHeader = unsafe { header_pointer.read_volatile() };

        let version = header.traffic_tile_version();
        if version != TRAFFIC_TILE_VERSION {
            return Err(GraphTileProviderError::UnsupportedTrafficTileVersion {
                graph_id: base_id,
                version,
                expected: TRAFFIC_TILE_VERSION,
            });
        }

        let edge_index = graph_id.feature_index();
        if edge_index >= u64::from(header.directed_edge_count()) {
            return Err(GraphTileProviderError::Lookup(LookupError::InvalidIndex));
        }

        // Tile structure: header + [TrafficSpeed]
        // The feature index has 21 bits, so none of this can overflow.
        let record_offset = u64::from(HEADER_SIZE_U32) + u64::from(SPEED_SIZE_U32) * edge_index;
        // The header may claim more edges than the tile has room for.
        check_size(record_offset + u64::from(SPEED_SIZE_U32))?;

        Ok(MmapTilePointer {
            mmap: tile_pointer.mmap.clone(),
            offsets: TileOffset {
                offset: tile_pointer.offsets.offset + record_offset,
                size: SPEED_SIZE_U32,
            },
        })
    }

    /// An iterator over all tile IDs contained in the tarball, in arbitrary order.
    #[inline]
    pub fn tile_ids(&self) -> impl Iterator<Item = &GraphId> {
        self.tarball_tile_provider.tile_ids()
    }
}

impl TrafficTileProvider<false> {
    /// Creates a new traffic tile provider from an existing tarball extract.
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
        Self::new(path)
    }
}

impl TrafficTileProvider<true> {
    /// Creates a new traffic tile provider from an existing tarball extract.
    /// Write support is enabled by this constructor.
    ///
    /// # Errors
    ///
    /// The extract _must_ include an `index.bin` file as the first entry.
    /// If the file is not _valid_ (of the correct length and superficially correct structure),
    /// or lists a tile that lies (partly) outside of the file,
    /// this constructor will fail.
    /// It also fails if the file is not writable.
    ///
    /// However, no further checks are performed to ensure the correctness of the file
    /// (its entire _raison d'être_ is that you shouldn't have to scan the whole tarball),
    /// so an incorrect index will invariably lead to tile fetch errors.
    #[inline]
    pub fn new_mutable<P: AsRef<Path>>(path: P) -> Result<Self, GraphTileProviderError> {
        Self::new(path)
    }

    /// Flushes outstanding memory map modifications to disk.
    ///
    /// If this method returns with an `Ok` result,
    /// the data has been persisted to disk.
    /// Modifications may also be _eventually_ flushed in other situations,
    /// such as when the memory is unmapped again.
    /// But this is the only way to ensure durability.
    ///
    /// # Errors
    ///
    /// Returns a relevant I/O error if the flush fails.
    #[inline]
    pub fn flush(&self) -> std::io::Result<()> {
        self.tarball_tile_provider.flush()
    }

    /// Updates the speed data stored for an edge.
    ///
    /// NOTE: this will update the memory map immediately,
    /// but it does not guarantee that the data has been durably stored until you call
    /// [`TrafficTileProvider<true>::flush`]!
    ///
    /// # Safety
    ///
    /// The same conditions as for [`TrafficTileProvider::get_speeds_for_edge`] apply.
    /// The store is a single aligned 64-bit store, so concurrent readers never see a torn record.
    /// See the [type-level documentation](TrafficTileProvider) for details.
    ///
    /// # Errors
    ///
    /// Fails in the same cases as [`TrafficTileProvider::get_speeds_for_edge`].
    #[inline]
    pub unsafe fn update_speed_for_edge(
        &self,
        graph_id: GraphId,
        speed: TrafficSpeed,
    ) -> Result<(), GraphTileProviderError> {
        // SAFETY: The caller upholds the conditions on concurrent writers (see the method docs).
        let speed_pointer = unsafe { self.get_pointer_for_edge(graph_id)? };

        // SAFETY: The pointer was bounds-checked against the tile, which lies within the map,
        // and it is 8 bytes long and 8-byte aligned (see `get_pointer_for_edge`),
        // so the size and alignment assertions in `write_volatile` hold.
        // The map is writable, since `MUT` is true.
        // The record is stored in little-endian byte order.
        unsafe { speed_pointer.write_volatile(speed.into_bits().get().to_le()) };
        Ok(())
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use crate::GraphId;
    use crate::tile_provider::TrafficTileProvider;
    use crate::traffic_tile::{SpeedValue, TrafficSpeed};
    use std::path::PathBuf;

    #[test]
    fn test_get_speed() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-traffic.tar");
        let provider =
            TrafficTileProvider::new_readonly(path).expect("Unable to init tile provider");

        // This edge has no speed info
        let graph_id = GraphId::try_from_components(0, 3015, 0).expect("Unable to create graph ID");
        let edge_speed = unsafe {
            provider
                .get_speeds_for_edge(graph_id)
                .expect("Unable to get tile")
        };

        assert!(!edge_speed.has_valid_speed());

        // This edge DOES have speed info
        let graph_id =
            GraphId::try_from_components(0, 3015, 42).expect("Unable to create graph ID");
        let edge_speed = unsafe {
            provider
                .get_speeds_for_edge(graph_id)
                .expect("Unable to get speed")
        };

        assert_eq!(edge_speed.overall_speed(), Some(32));
    }

    #[test]
    fn test_set_speed() {
        const DESIRED_SPEED: u8 = 32;

        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-traffic.tar");

        // let tmp_path = PathBuf::from("/Users/ianthetechie/valhalla-docker/valhalla/traffic.tar");
        let tmp_dir = option_env!("RUNNER_TEMP").unwrap_or("/tmp");
        let tmp_path = PathBuf::from(tmp_dir).join("traffic-test-set-speed.tar");
        std::fs::copy(fixture_path, &tmp_path).expect("Failed to copy");
        let provider =
            TrafficTileProvider::new_mutable(tmp_path).expect("Unable to init tile provider");
        let graph_id =
            GraphId::try_from_components(0, 3015, 42).expect("Unable to create graph ID");
        let edge_speed = unsafe {
            provider
                .get_speeds_for_edge(graph_id)
                .expect("Unable to get speed")
        };

        assert_eq!(
            edge_speed.overall_speed(),
            Some(32),
            "It looks like the traffic tar may have been cached from a previous run"
        );

        unsafe {
            provider
                .update_speed_for_edge(
                    graph_id,
                    TrafficSpeed::single_speed(SpeedValue::try_new(DESIRED_SPEED).unwrap(), None),
                )
                .expect("Failed to set speed for edge");
        }

        let edge_speed = unsafe {
            provider
                .get_speeds_for_edge(graph_id)
                .expect("Unable to get speed")
        };
        assert!(edge_speed.has_valid_speed());
        assert_eq!(edge_speed.overall_speed(), Some(DESIRED_SPEED));
    }
}
