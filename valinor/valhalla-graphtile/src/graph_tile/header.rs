//! # Graph tile header
//!
//! The fixed-size header at the start of every graph tile.
//! It holds the tile's identity, the record counts of the fixed-size sections,
//! and the byte offsets of the variable-size sections.
//!
//! The header is the only part of a tile that [`GraphTileView`](super::GraphTileView)
//! trusts to describe the rest,
//! so [`GraphTileHeader::validate`] checks that it is internally consistent
//! before any section is read.

use crate::graph_tile::GraphTileDecodingError;
use crate::graph_tile::checksum::{BuildId, TileHash, checksum};
use crate::tile_hierarchy::{STANDARD_LEVELS, TileLevel};
use crate::{AsCowStr, BIN_COUNT, GraphId};
use bitfield_struct::bitfield;
use chrono::{DateTime, Days, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Utc};
use geo::{Coord, coord};
use std::borrow::Cow;
use thiserror::Error;
use zerocopy::{F32, FromBytes, LE, U16, U32, U64};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

mod builder;

pub(super) use builder::GraphTileHeaderBuilder;

/// Remaining variable offset slots for growth.
/// See valhalla/baldr/graphtileheader.h for details.
const EMPTY_SLOTS: usize = 10;

/// The number of UTF-8 bytes in the fixed size version field.
pub(crate) const VERSION_LEN: usize = 16;

/// The version prefix of the tiles we support.
///
/// We only read tiles built by Valhalla 3.9.x (see the routing architecture docs).
/// Valhalla itself only logs a warning on a major version mismatch,
/// but the tile format has changed within major versions before,
/// so we refuse anything else outright.
const SUPPORTED_VERSION_PREFIX: &[u8] = b"3.9.";

/// Valhalla's pivot date (`kPivotDate` in `baldr/graphconstants.h`).
///
/// Tile creation dates count days from here.
const PIVOT_DATE: NaiveDate = NaiveDate::from_ymd_opt(2014, 1, 1).expect("valid date");

/// Ways in which a graph tile header can be invalid or unsupported.
///
/// These are all checked by [`GraphTileHeader::validate`],
/// before reading any data that the header describes.
#[derive(Debug, Error, PartialEq)]
pub enum GraphTileHeaderError {
    #[error("read the tile version: {version:?} is not a Valhalla 3.9.x version")]
    UnsupportedVersion { version: String },
    #[error("read the tile level: level {0} is not supported")]
    UnsupportedLevel(u8),
    #[error(
        "read the tile ID: tile {tile_id} does not exist on level {level}, which has {tile_count} tiles"
    )]
    TileIdOutOfRange {
        level: u8,
        tile_id: u64,
        tile_count: u32,
    },
    #[error(
        "read the tile's southwest corner: tile {tile_id} on level {level} starts at {expected:?}, but the header says {actual:?}"
    )]
    BaseCoordinateMismatch {
        level: u8,
        tile_id: u64,
        expected: Coord<f32>,
        actual: Coord<f32>,
    },
    #[error("read the tile size: the header says {header} bytes, but the tile has {actual} bytes")]
    TileSizeMismatch { header: usize, actual: usize },
    #[error("read the {section} section: it starts at byte {start}, after its end at byte {end}")]
    SectionEndsBeforeStart {
        section: &'static str,
        start: usize,
        end: usize,
    },
    #[error("read edge bin {bin}: it starts at entry {start}, after its end at entry {end}")]
    BinEndsBeforeStart {
        bin: usize,
        start: usize,
        end: usize,
    },
    #[error(
        "read the {section} section: the header says it starts at byte {expected}, but the preceding sections end at byte {actual}"
    )]
    SectionOffsetMismatch {
        section: &'static str,
        expected: usize,
        actual: usize,
    },
}

/// The Valhalla epoch: Midnight Jan 1, 2014, Eastern Standard Time.
///
/// This is only used to round trip [`GraphTileHeader::create_date`] through the tile builder.
const VALHALLA_EPOCH: DateTime<Utc> = DateTime::<FixedOffset>::from_naive_utc_and_offset(
    NaiveDateTime::new(
        NaiveDate::from_ymd_opt(2014, 1, 1).unwrap(),
        NaiveTime::from_num_seconds_from_midnight_opt(0, 0).unwrap(),
    ),
    FixedOffset::west_opt(5 * 3600).unwrap(),
)
.to_utc();

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
struct FirstBitfield {
    #[bits(46, from = bit_twiddling_helpers::conv_u64le::from_inner, into = bit_twiddling_helpers::conv_u64le::into_inner)]
    graph_id: U64<LE>,
    #[bits(4)]
    density: u8,
    // More relative statistics (0 - 15); name, speed, and exit quality seem unused in Valhalla?
    #[bits(4)]
    name_quality: u8,
    #[bits(4)]
    speed_quality: u8,
    #[bits(4)]
    exit_quality: u8,
    // NOTE: We don't use bool, because that doesn't have a `FromBytes` implementation.
    // We are only using a single bit though, so we *can* guarantee that the bit pattern is valid.
    // This will probably change once we have the ability to assume bit validity.
    // This is currently in beta: https://doc.rust-lang.org/beta/std/mem/trait.TransmuteFrom.html.
    #[bits(1)]
    has_elevation: u8,
    #[bits(1)]
    has_ext_directed_edge: u8,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
struct SecondBitfield {
    #[bits(21, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    node_count: U32<LE>,
    #[bits(21, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    directed_edge_count: U32<LE>,
    #[bits(21, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    predicted_speeds_count: U32<LE>,
    #[bits(1)]
    _spare: u8,
}

// NOTE: There are a lot of weird paddings which arose because of an incompatibility in padding
// between x86 and amd64 architectures.
// This was entirely accidental upstream and will eventually be cleaned up in v4.

#[bitfield(u32,
    repr = U32<LE>,
    from = bit_twiddling_helpers::conv_u32le::from_inner,
    into = bit_twiddling_helpers::conv_u32le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
struct TransitionCountBitfield {
    /// No, this is not a typo. This bit field has 22 bits.
    ///
    /// There can only be twice as many transitions as there are nodes,
    /// but in practice there are far fewer than this limit.
    #[bits(22, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    transition_count: U32<LE>,
    // Deprecated; scheduled for deletion in v4
    #[bits(10)]
    _spare: u16,
}

#[bitfield(u32,
    repr = U32<LE>,
    from = bit_twiddling_helpers::conv_u32le::from_inner,
    into = bit_twiddling_helpers::conv_u32le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
struct TurnLaneCountBitfield {
    #[bits(21, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    turn_lane_count: U32<LE>,
    #[bits(11)]
    _spare: u16,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
struct TransitRecordBitfield {
    #[bits(16, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    transfer_count: U16<LE>,
    #[bits(7)]
    _spare: u8,
    #[bits(24, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    departure_count: U32<LE>,
    #[bits(16, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    stop_count: U16<LE>,
    #[bits(1)]
    _spare: u8,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
struct MiscCountsBitFieldOne {
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    route_count: U16<LE>,
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    schedule_count: U16<LE>,
    #[bits(24, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    sign_count: U32<LE>,
    #[bits(16)]
    _spare: u16,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, PartialEq)]
struct MiscCountsBitFieldTwo {
    #[bits(24, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    access_restriction_count: U32<LE>,
    #[bits(16, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    admin_count: U16<LE>,
    #[bits(24)]
    _spare: u32,
}

// TODO: Unsure if packed is needed as well.
// Seems like it's technically not, since Valhalla is explicit about padding
/// Summary information about the graph tile.
///
/// This contains metadata like version,
/// number of nodes and edges,
/// and pointer offsets to other data.
#[derive(Clone, FromBytes, IntoBytes, Immutable, Unaligned, KnownLayout, Debug, PartialEq)]
#[repr(C)]
pub struct GraphTileHeader {
    bit_field_1: FirstBitfield,
    base_lon_lat: [F32<LE>; 2],
    pub(super) version: [u8; VERSION_LEN],
    /// The dataset ID (canonically, the last OSM changeset ID).
    pub dataset_id: U64<LE>,
    bit_field_2: SecondBitfield,
    transition_count_bitfield: TransitionCountBitfield,
    turn_lane_count_bitfield: TurnLaneCountBitfield,
    transit_record_bitfield: TransitRecordBitfield,
    misc_counts_bit_field_one: MiscCountsBitFieldOne,
    misc_counts_bit_field_two: MiscCountsBitFieldTwo,
    /// This can be used for custom information while retaining Valhalla compatibility.
    /// However, you must uphold the following invariants:
    ///
    /// - The header size doesn't change.
    /// - The ordering of *existing* fields in the data structure doesn't change.
    ///
    /// Valhalla calls this `spareword0_`.
    _reserved_1: U64<LE>,
    /// The tileset build ID in the high 16 bits,
    /// and a hash of this tile's data in the low 48 bits.
    ///
    /// Valhalla calls this `checksum_` (formerly `spareword1_`).
    checksum: U64<LE>,
    // Internal offsets that help us size the variable length data fields.
    complex_restriction_forward_offset: U32<LE>,
    complex_restriction_reverse_offset: U32<LE>,
    edge_info_offset: U32<LE>,
    text_list_offset: U32<LE>,
    /// The date the tile was created, as the number of days since Valhalla's pivot date
    /// (2014-01-01).
    ///
    /// Zero means that the date was not recorded,
    /// which is the case for every tile Valhalla 3.9 builds.
    create_date: U32<LE>,
    /// Offsets for each bin of the grid
    ///
    /// This is used for search/lookup from a geographic location.
    /// By dividing the tile into a 5x5 grid, we can reduce the number of iterations
    /// to find a specific edge.
    ///
    /// NOTE: Valhalla only actually does binning at level 2,
    /// so these values will be zeroes for other levels.
    pub(super) bin_offsets: [U32<LE>; BIN_COUNT],
    /// Offset to the beginning of the variable length data.
    lane_connectivity_offset: U32<LE>,
    /// Offset to the beginning of the variable length data.
    predicted_speeds_offset: U32<LE>,
    /// The size of the tile (in bytes)
    pub tile_size: U32<LE>,
    /// Offset to the beginning of the bounding circles.
    ///
    /// Zero means that there are no bounding circles.
    /// Early Valhalla 3.x versions filled unused offset slots with the tile size,
    /// so that value also means that there are no bounding circles.
    bounding_circles_offset: U32<LE>,
    /// See valhalla/baldr/graphtileheader.h for details.
    _empty_slots: [U32<LE>; EMPTY_SLOTS],
}

impl GraphTileHeader {
    /// The full Graph ID of this tile.
    #[inline]
    pub const fn graph_id(&self) -> GraphId {
        // SAFETY: We know that the bit field cannot contain a value
        // larger than the max allowed value (it's limited to 46 bits).
        unsafe { GraphId::from_id_unchecked(self.bit_field_1.graph_id()) }
    }

    /// The relative road density within this tile (0-15).
    #[inline]
    pub const fn density(&self) -> u8 {
        self.bit_field_1.density()
    }

    /// The date this tile was created, if the tile records one.
    ///
    /// Valhalla stores the number of days since its pivot date (2014-01-01),
    /// with zero meaning that no date was recorded.
    /// Valhalla 3.9's tile build leaves it at zero,
    /// so expect `None` for tiles built by mainline.
    #[inline]
    pub fn date_created(&self) -> Option<NaiveDate> {
        match self.create_date.get() {
            0 => None,
            days => PIVOT_DATE.checked_add_days(Days::new(u64::from(days))),
        }
    }

    /// The raw creation date as a timestamp, for the tile builder to copy through unchanged.
    ///
    /// This maps the "not recorded" value of zero to the epoch,
    /// so it is not public; use [`GraphTileHeader::date_created`] instead.
    #[inline]
    pub(crate) fn create_date(&self) -> DateTime<Utc> {
        VALHALLA_EPOCH + TimeDelta::days(i64::from(self.create_date.get()))
    }

    /// Does this tile include elevation data?
    #[inline]
    pub const fn has_elevation(&self) -> bool {
        self.bit_field_1.has_elevation() != 0
    }

    /// Does this tile include extended directed edge attributes?
    ///
    /// If true, read one extended directed edge for every regular directed edge.
    #[inline]
    pub const fn has_ext_directed_edge(&self) -> bool {
        self.bit_field_1.has_ext_directed_edge() != 0
    }

    /// The hash of this tile's data (everything after the header).
    ///
    /// This is unique per tile, but reproducible across builds of the same data.
    /// See [`TileHash`] for how it is computed.
    #[inline]
    pub const fn tile_hash(&self) -> TileHash {
        TileHash::from_checksum(self.checksum.get())
    }

    /// The ID of the tileset build that this tile belongs to.
    ///
    /// This is the same for every tile in a tileset.
    /// See [`BuildId`] for how it is computed.
    #[inline]
    pub const fn build_id(&self) -> BuildId {
        BuildId::from_checksum(self.checksum.get())
    }

    /// Stamps a tileset build ID into the header of a serialized tile.
    ///
    /// This only rewrites the build ID; the tile hash and everything else stay the same.
    /// This mirrors Valhalla's `set_tileset_build_id`,
    /// which runs after every tile in a tileset has been written:
    /// compute the build ID from all tile hashes with [`BuildId::of_tileset`],
    /// then stamp it into every tile.
    ///
    /// # Errors
    ///
    /// Fails if `tile` is too short to contain a header.
    pub fn stamp_build_id(
        tile: &mut [u8],
        build_id: BuildId,
    ) -> Result<(), GraphTileDecodingError> {
        let (header, _) =
            Self::mut_from_prefix(tile).map_err(|e| GraphTileDecodingError::CastError {
                field: "header".to_string(),
                error_description: e.to_string(),
            })?;
        header.checksum = checksum(build_id, header.tile_hash()).into();
        Ok(())
    }

    /// Does this tile include bounding circles for the edges in its edge bins?
    #[inline]
    pub const fn has_bounding_circles(&self) -> bool {
        let offset = self.bounding_circles_offset.get();
        offset != 0 && offset != self.tile_size.get()
    }

    /// The byte offset of the bounding circles, if the tile has any.
    #[inline]
    pub(crate) const fn bounding_circles_offset(&self) -> Option<usize> {
        if self.has_bounding_circles() {
            Some(self.bounding_circles_offset.get() as usize)
        } else {
            None
        }
    }

    /// The byte offset of the complex forward restrictions,
    /// which immediately follow the edge bins (and bounding circles, if any).
    #[inline]
    pub(crate) const fn complex_forward_restrictions_offset(&self) -> usize {
        self.complex_restriction_forward_offset.get() as usize
    }

    /// The center of an edge bin, computed exactly as Valhalla's loki and mjolnir do.
    ///
    /// Use this to decode the bin's bounding circles
    /// (see [`DiscretizedBoundingCircle::decode`](crate::graph_tile::DiscretizedBoundingCircle::decode)).
    /// Bins are numbered in row-major order from the southwest corner.
    ///
    /// Valhalla adds a single precision offset within the tile to the tile's corner;
    /// we do the same so that decoded circles match Valhalla's bit for bit.
    ///
    /// Returns `None` if the bin index is out of range,
    /// or if the tile is not on a standard Valhalla hierarchy level.
    pub fn bin_center(&self, bin_index: usize) -> Option<Coord<f64>> {
        let level = self.graph_id().level();
        let tiling = &STANDARD_LEVELS
            .iter()
            .find(|lvl| lvl.level == level)?
            .tiling_system;
        let bins_per_side = usize::from(tiling.n_subdivisions);
        if bin_index >= bins_per_side * bins_per_side {
            return None;
        }
        let subdivision_size = tiling.tile_size / f32::from(tiling.n_subdivisions);
        // These are both less than the number of subdivisions, which is a u8.
        let col = f32::from(u8::try_from(bin_index % bins_per_side).ok()?);
        let row = f32::from(u8::try_from(bin_index / bins_per_side).ok()?);
        let lng_offset = col * subdivision_size + subdivision_size / 2.0;
        let lat_offset = row * subdivision_size + subdivision_size / 2.0;
        let sw = self.sw_corner();
        Some(coord! {
            x: f64::from(sw.x) + f64::from(lng_offset),
            y: f64::from(sw.y) + f64::from(lat_offset),
        })
    }

    /// The coordinate of the southwest corner of this graph tile.
    #[inline]
    pub const fn sw_corner(&self) -> Coord<f32> {
        coord! {x: self.base_lon_lat[0].get(), y: self.base_lon_lat[1].get()}
    }

    /// Gets the writer version used to generate this graph tile.
    pub fn version(&self) -> Cow<'_, str> {
        self.version.as_cow_str()
    }

    #[inline]
    pub const fn node_count(&self) -> u32 {
        self.bit_field_2.node_count().get()
    }

    /// The number of directed edges in this graph tile.
    #[inline]
    pub const fn directed_edge_count(&self) -> u32 {
        self.bit_field_2.directed_edge_count().get()
    }

    /// The number of predicted speed records in this graph tile.
    #[inline]
    pub const fn predicted_speeds_count(&self) -> u32 {
        self.bit_field_2.predicted_speeds_count().get()
    }

    /// The number of node transitions in this graph tile.
    #[inline]
    pub const fn transition_count(&self) -> u32 {
        self.transition_count_bitfield.transition_count().get()
    }

    /// The number of turn lanes in this graph tile.
    #[inline]
    pub const fn turn_lane_count(&self) -> u32 {
        self.turn_lane_count_bitfield.turn_lane_count().get()
    }

    /// The number of transit transfers in this graph tile.
    #[inline]
    pub const fn transfer_count(&self) -> u16 {
        self.transit_record_bitfield.transfer_count().get()
    }

    /// The number of transit departures in this graph tile.
    #[inline]
    pub const fn departure_count(&self) -> u32 {
        self.transit_record_bitfield.departure_count().get()
    }

    /// The number of transit stops in this graph tile.
    #[inline]
    pub const fn stop_count(&self) -> u16 {
        self.transit_record_bitfield.stop_count().get()
    }

    /// The number of transit routes in this graph tile.
    #[inline]
    pub const fn route_count(&self) -> u16 {
        self.misc_counts_bit_field_one.route_count().get()
    }

    /// The number of transit schedules in this graph tile.
    #[inline]
    pub const fn schedule_count(&self) -> u16 {
        self.misc_counts_bit_field_one.schedule_count().get()
    }

    /// The number of signs in this graph tile.
    #[inline]
    pub const fn sign_count(&self) -> u32 {
        self.misc_counts_bit_field_one.sign_count().get()
    }

    /// The number of access restrictions in this graph tile.
    #[inline]
    pub const fn access_restriction_count(&self) -> u32 {
        self.misc_counts_bit_field_two
            .access_restriction_count()
            .get()
    }

    /// The number of admin records in this graph tile.
    #[inline]
    pub const fn admin_count(&self) -> u16 {
        self.misc_counts_bit_field_two.admin_count().get()
    }

    // These size calculation helpers avoid exposing the raw offsets.
    // The calculations come from graphtile.cc, where they do the same math in `GraphTile::Initialize`
    // directly.

    /// The size (in units) occupied by the edge bins structure.
    #[inline]
    pub const fn edge_bins_count(&self) -> usize {
        self.bin_offsets[BIN_COUNT - 1].get() as usize
    }

    /// The number of bounding circles.
    ///
    /// When present, there is exactly one bounding circle for every edge bin entry.
    #[inline]
    pub const fn bounding_circle_count(&self) -> usize {
        if self.has_bounding_circles() {
            self.edge_bins_count()
        } else {
            0
        }
    }

    /// Gets the start and end offset for the given bin index.
    /// This offset can be used to index into the array of edge IDs in the `GraphTile` struct.
    ///
    /// The returned tuple is in `(start, end)` order.
    /// The end index is "one past the end" (the start of the next bin).
    ///
    /// Returns `None` if the bin index is out of range (25 or greater),
    /// or if the bin's offsets are out of order
    /// (which [`GraphTileHeader::validate`] rules out).
    #[inline]
    pub const fn edge_bin_offsets(&self, bin_index: usize) -> Option<(usize, usize)> {
        if bin_index >= BIN_COUNT {
            return None;
        }
        let start = if bin_index == 0 {
            0usize
        } else {
            self.bin_offsets[bin_index - 1].get() as usize
        };
        let end = self.bin_offsets[bin_index].get() as usize;
        if start <= end {
            Some((start, end))
        } else {
            None
        }
    }

    /// The size in bytes of the section between two header offsets.
    ///
    /// # Errors
    ///
    /// Fails if the section would end before it starts.
    #[inline]
    fn section_size(
        section: &'static str,
        start: u32,
        end: u32,
    ) -> Result<usize, GraphTileHeaderError> {
        match end.checked_sub(start) {
            Some(size) => Ok(size as usize),
            None => Err(GraphTileHeaderError::SectionEndsBeforeStart {
                section,
                start: start as usize,
                end: end as usize,
            }),
        }
    }

    /// The size (in bytes) occupied by the complex forward restrictions.
    ///
    /// # Errors
    ///
    /// Fails if the section's offsets are out of order.
    #[inline]
    pub(crate) fn complex_forward_restrictions_size(&self) -> Result<usize, GraphTileHeaderError> {
        Self::section_size(
            "complex_forward_restrictions",
            self.complex_restriction_forward_offset.get(),
            self.complex_restriction_reverse_offset.get(),
        )
    }

    /// The size (in bytes) occupied by the complex reverse restrictions.
    ///
    /// # Errors
    ///
    /// Fails if the section's offsets are out of order.
    #[inline]
    pub(crate) fn complex_reverse_restrictions_size(&self) -> Result<usize, GraphTileHeaderError> {
        Self::section_size(
            "complex_reverse_restrictions",
            self.complex_restriction_reverse_offset.get(),
            self.edge_info_offset.get(),
        )
    }

    /// The size (in bytes) occupied by the edge info data structure.
    ///
    /// # Errors
    ///
    /// Fails if the section's offsets are out of order.
    #[inline]
    pub(crate) fn edge_info_size(&self) -> Result<usize, GraphTileHeaderError> {
        Self::section_size(
            "edge_info",
            self.edge_info_offset.get(),
            self.text_list_offset.get(),
        )
    }

    /// The size (in bytes) occupied by the text list.
    ///
    /// # Errors
    ///
    /// Fails if the section's offsets are out of order.
    #[inline]
    pub(crate) fn text_list_size(&self) -> Result<usize, GraphTileHeaderError> {
        Self::section_size(
            "text_list",
            self.text_list_offset.get(),
            self.lane_connectivity_offset.get(),
        )
    }

    /// The size (in bytes) occupied by the lane connectivity data.
    ///
    /// # Errors
    ///
    /// Fails if the section's offsets are out of order.
    #[inline]
    pub(crate) fn lane_connectivity_size(&self) -> Result<usize, GraphTileHeaderError> {
        // Like Valhalla, only trust the predicted speed offset when there are predicted speeds.
        let end = if self.predicted_speeds_count() > 0 {
            self.predicted_speeds_offset.get()
        } else {
            self.tile_size.get()
        };
        Self::section_size(
            "lane_connectivity",
            self.lane_connectivity_offset.get(),
            end,
        )
    }

    /// The byte offset of the predicted speeds, if the tile has any.
    #[inline]
    pub(crate) const fn predicted_speeds_offset(&self) -> Option<usize> {
        if self.predicted_speeds_count() > 0 {
            Some(self.predicted_speeds_offset.get() as usize)
        } else {
            None
        }
    }

    /// Is this a version of the tile format that we can read?
    ///
    /// We only support tiles built by Valhalla 3.9.x,
    /// whose version field starts with `3.9.` and a digit.
    #[inline]
    pub fn has_supported_version(&self) -> bool {
        // The version is a NUL-terminated string in a fixed-size field.
        let len = self
            .version
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(VERSION_LEN);
        self.version[..len]
            .strip_prefix(SUPPORTED_VERSION_PREFIX)
            .and_then(<[u8]>::first)
            .is_some_and(u8::is_ascii_digit)
    }

    /// The tile hierarchy level that this tile belongs to,
    /// if it is a level we support.
    #[inline]
    fn tile_level(&self) -> Option<&'static TileLevel> {
        let level = self.graph_id().level();
        STANDARD_LEVELS.iter().find(|lvl| lvl.level == level)
    }

    /// Checks that the header describes a tile that we can read,
    /// and that it is internally consistent.
    ///
    /// `tile_len` is the length in bytes of the whole tile, including the header.
    ///
    /// Once this succeeds, the section size helpers cannot fail,
    /// and the edge bin offsets are monotonic.
    /// This does not check anything outside the header;
    /// [`GraphTileView`](super::GraphTileView) checks the sections themselves as it reads them.
    ///
    /// # Errors
    ///
    /// Returns the first problem found.
    /// The checks run in a fixed order:
    /// the version, the level and tile ID, the base coordinate, the tile size,
    /// the variable-size section offsets, and finally the edge bin offsets.
    pub fn validate(&self, tile_len: usize) -> Result<(), GraphTileHeaderError> {
        if !self.has_supported_version() {
            return Err(GraphTileHeaderError::UnsupportedVersion {
                version: self.version().into_owned(),
            });
        }

        let graph_id = self.graph_id();
        let level = graph_id.level();
        // Transit tiles (level 3) are out of scope; see the routing architecture docs.
        let tile_level = self
            .tile_level()
            .ok_or(GraphTileHeaderError::UnsupportedLevel(level))?;
        let tiling = &tile_level.tiling_system;
        let tile_id = graph_id.tile_id();
        let Some(expected) = tiling.base_coordinate(tile_id) else {
            return Err(GraphTileHeaderError::TileIdOutOfRange {
                level,
                tile_id,
                tile_count: tiling.tile_count(),
            });
        };

        // Valhalla derives the base coordinate from the tile ID in single precision
        // (`Tiles::Base`), so a tile built by Valhalla matches it bit for bit.
        // Checking it also rules out non-finite coordinates.
        let actual = self.sw_corner();
        if expected.x.to_bits() != actual.x.to_bits() || expected.y.to_bits() != actual.y.to_bits()
        {
            return Err(GraphTileHeaderError::BaseCoordinateMismatch {
                level,
                tile_id,
                expected,
                actual,
            });
        }

        let header_tile_size = self.tile_size.get() as usize;
        if header_tile_size != tile_len {
            return Err(GraphTileHeaderError::TileSizeMismatch {
                header: header_tile_size,
                actual: tile_len,
            });
        }

        // Each helper checks that its section does not end before it starts.
        // Together, they check that the offsets increase monotonically up to the tile size.
        self.complex_forward_restrictions_size()?;
        self.complex_reverse_restrictions_size()?;
        self.edge_info_size()?;
        self.text_list_size()?;
        self.lane_connectivity_size()?;
        if self.predicted_speeds_count() > 0 {
            Self::section_size(
                "predicted_speeds",
                self.predicted_speeds_offset.get(),
                self.tile_size.get(),
            )?;
        }

        let mut start = 0;
        for (bin, offset) in self.bin_offsets.iter().enumerate() {
            let end = offset.get() as usize;
            if end < start {
                return Err(GraphTileHeaderError::BinEndsBeforeStart { bin, start, end });
            }
            start = end;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::GraphTileHeader;
    use crate::graph_tile::{BuildId, GraphTile, TEST_GRAPH_TILE_L0, TEST_GRAPH_TILE_L2, TileHash};
    use zerocopy::FromBytes;

    #[test]
    fn test_parse_header() {
        let tile = &*TEST_GRAPH_TILE_L0;

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!(tile.header());
        }
    }

    #[test]
    fn test_checksum_fields() {
        // Cross-checked with the Valhalla 3.9.1 Python bindings
        // (GraphTileHeader.build_id and GraphTileHeader.tile_checksum).
        let l0 = TEST_GRAPH_TILE_L0.header();
        assert_eq!(l0.build_id(), BuildId::new(53174));
        assert_eq!(l0.tile_hash().value(), 0x1eff_7de6_d018);
        let l2 = TEST_GRAPH_TILE_L2.header();
        assert_eq!(l2.build_id(), BuildId::new(53174));
        assert_eq!(l2.tile_hash().value(), 0x57bf_7e07_0414);

        // The stored hash is the hash of everything after the header.
        for tile in [&*TEST_GRAPH_TILE_L0, &*TEST_GRAPH_TILE_L2] {
            let bytes = tile.borrow_owner();
            assert_eq!(
                TileHash::of_tile_data(&bytes[size_of::<GraphTileHeader>()..]),
                tile.header().tile_hash()
            );
        }
    }

    #[test]
    fn test_stamp_build_id() {
        let mut bytes = TEST_GRAPH_TILE_L0.borrow_owner().clone();
        let original = TEST_GRAPH_TILE_L0.header().clone();
        GraphTileHeader::stamp_build_id(&mut bytes, BuildId::new(0xbeef)).unwrap();
        let (stamped, _) = GraphTileHeader::ref_from_prefix(&bytes).unwrap();
        assert_eq!(stamped.build_id(), BuildId::new(0xbeef));
        assert_eq!(stamped.tile_hash(), original.tile_hash());
        // Nothing else changes.
        assert_eq!(&bytes[..88], &TEST_GRAPH_TILE_L0.borrow_owner()[..88]);
        assert_eq!(&bytes[96..], &TEST_GRAPH_TILE_L0.borrow_owner()[96..]);

        assert!(GraphTileHeader::stamp_build_id(&mut bytes[..100], BuildId::new(1)).is_err());
    }

    #[test]
    fn test_bounding_circle_presence() {
        // Mainline treats a bounding circle offset equal to the tile size as absent
        // (early 3.x versions filled unused offset slots with the tile size).
        let mut header = TEST_GRAPH_TILE_L2.header().clone();
        assert!(header.has_bounding_circles());
        assert_eq!(header.bounding_circle_count(), header.edge_bins_count());
        header.bounding_circles_offset = header.tile_size;
        assert!(!header.has_bounding_circles());
        assert_eq!(header.bounding_circles_offset(), None);
        assert_eq!(header.bounding_circle_count(), 0);
        header.bounding_circles_offset = 0.into();
        assert!(!header.has_bounding_circles());
        assert_eq!(header.bounding_circle_count(), 0);
    }

    #[test]
    fn test_parse_header_l2() {
        let tile = &*TEST_GRAPH_TILE_L2;

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!(tile.header());
        }
    }
}
