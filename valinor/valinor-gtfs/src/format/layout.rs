//! The file's records: one [`FileHeader`], a directory of [`SectionEntry`], and flat tables.
//!
//! All fields are little-endian and aligned to one byte,
//! so `<[T]>::ref_from_bytes` casts any correctly sized slice without copying,
//! and no platform can read them differently.
//!
//! The first sixteen bytes are stable across versions:
//! `magic`, `version`, and `checksum` are how a loader tells an old or foreign file
//! from a corrupt one, whatever the rest of the layout becomes.

use crc32fast::Hasher;
use zerocopy::little_endian::{F32, I32, I64, U16, U32, U64};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The first eight bytes of every compiled timetable file.
pub const MAGIC: [u8; 8] = *b"VLNRGTFS";

/// The only format version this build reads.
pub const FORMAT_VERSION: u32 = 6;

/// A transfer time for a stop that `transfers.txt` doesn't set one for, as MOTIS does: 2 minutes.
pub const DEFAULT_TRANSFER_SECONDS: u32 = 120;

/// The longest calendar a file may hold, in days (over a hundred years).
pub(crate) const MAX_SERVICE_DAYS: u32 = 40_000;

/// A reference that points at nothing, where a real value would index a table.
pub(crate) const NONE: u32 = u32::MAX;

/// The number of bytes at the front of the file that the checksum does not cover.
pub(crate) const UNCHECKSUMMED_PREFIX: usize = 16;

/// The checksum of a whole file, skipping the bytes the checksum doesn't cover.
pub(crate) fn checksum_of(bytes: &[u8]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(bytes.get(UNCHECKSUMMED_PREFIX..).unwrap_or(&[]));
    hasher.finalize()
}

/// The file's fixed header.
///
/// `magic`, `version`, and `checksum` must stay the first sixteen bytes in every version.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct FileHeader {
    pub(crate) magic: [u8; 8],
    pub(crate) version: U32,
    /// CRC-32 of every byte after the first [`UNCHECKSUMMED_PREFIX`].
    pub(crate) checksum: U32,
    pub(crate) file_len: U64,
    pub(crate) section_count: U32,
    /// How many of the [`StopRow`]s are timetable stops; they come first.
    pub(crate) boardable_stops: U32,
}

/// Where one table lives.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct SectionEntry {
    pub(crate) id: U32,
    pub(crate) _reserved: U32,
    pub(crate) offset: U64,
    pub(crate) byte_len: U64,
}

/// The tables of a file. Each appears exactly once, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum SectionId {
    Feeds = 1,
    Zones,
    ZoneTransitions,
    NoonOffsets,
    Strings,
    StringOffsets,
    Agencies,
    Routes,
    RoutePatterns,
    Stops,
    ChildStops,
    Services,
    ServiceBits,
    Patterns,
    PatternStops,
    Trips,
    Times,
    Rules,
    TripRiders,
    StopHeadsigns,
    StopSequences,
    StopDists,
    Shapes,
    ShapePoints,
    TripIndex,
    RouteIndex,
    StopIndex,
    TransferPairs,
    TripSegments,
}

impl SectionId {
    /// Every section, in file order.
    pub(crate) const ALL: [Self; 29] = [
        Self::Feeds,
        Self::Zones,
        Self::ZoneTransitions,
        Self::NoonOffsets,
        Self::Strings,
        Self::StringOffsets,
        Self::Agencies,
        Self::Routes,
        Self::RoutePatterns,
        Self::Stops,
        Self::ChildStops,
        Self::Services,
        Self::ServiceBits,
        Self::Patterns,
        Self::PatternStops,
        Self::Trips,
        Self::Times,
        Self::Rules,
        Self::TripRiders,
        Self::StopHeadsigns,
        Self::StopSequences,
        Self::StopDists,
        Self::Shapes,
        Self::ShapePoints,
        Self::TripIndex,
        Self::RouteIndex,
        Self::StopIndex,
        Self::TransferPairs,
        Self::TripSegments,
    ];

    /// The section's name, for error messages.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Feeds => "feeds",
            Self::Zones => "zones",
            Self::ZoneTransitions => "zone transitions",
            Self::NoonOffsets => "noon offsets",
            Self::Strings => "strings",
            Self::StringOffsets => "string offsets",
            Self::Agencies => "agencies",
            Self::Routes => "routes",
            Self::RoutePatterns => "route patterns",
            Self::Stops => "stops",
            Self::ChildStops => "child stops",
            Self::Services => "services",
            Self::ServiceBits => "service bits",
            Self::Patterns => "patterns",
            Self::PatternStops => "pattern stops",
            Self::Trips => "trips",
            Self::Times => "times",
            Self::Rules => "rules",
            Self::TripRiders => "trip riders",
            Self::StopHeadsigns => "stop headsigns",
            Self::StopSequences => "stop sequences",
            Self::StopDists => "stop distances",
            Self::Shapes => "shapes",
            Self::ShapePoints => "shape points",
            Self::TripIndex => "trip index",
            Self::RouteIndex => "route index",
            Self::StopIndex => "stop index",
            Self::TransferPairs => "transfer pairs",
            Self::TripSegments => "trip segments",
        }
    }
}

/// One GTFS feed in the file.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct FeedRow {
    /// The namespace of every GTFS identifier of this feed.
    pub(crate) id: U32,
    /// `feed_info.txt`'s `feed_version`, or the empty string.
    pub(crate) version: U32,
    /// The feed's time zone, as a row of the zone table.
    pub(crate) zone: U32,
    pub(crate) first_service_date: U32,
    pub(crate) last_service_date: U32,
}

/// One named time zone that a feed or stop uses.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct ZoneRow {
    pub(crate) name: U32,
    pub(crate) transitions_start: U32,
    pub(crate) transition_count: U32,
}

/// From `at_unix` onward, until the next row, the zone is `offset` seconds east of UTC.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct ZoneTransitionRow {
    pub(crate) at_unix: I64,
    pub(crate) offset: I32,
}

/// From service date `first_date` until the next row, local noon is `noon_offset` seconds east
/// of UTC, which fixes where each service day starts.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct NoonOffsetRow {
    pub(crate) first_date: U32,
    pub(crate) noon_offset: I32,
}

/// An agency.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct AgencyRow {
    pub(crate) feed: U32,
    pub(crate) gtfs_id: U32,
    pub(crate) name: U32,
    pub(crate) url: U32,
    pub(crate) phone: U32,
    pub(crate) fare_url: U32,
}

/// A GTFS route, which may have several patterns.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct RouteRow {
    pub(crate) feed: U32,
    pub(crate) agency: U32,
    pub(crate) gtfs_id: U32,
    pub(crate) short_name: U32,
    pub(crate) long_name: U32,
    pub(crate) desc: U32,
    pub(crate) url: U32,
    pub(crate) patterns_start: U32,
    pub(crate) pattern_count: U32,
    pub(crate) route_type: U16,
    pub(crate) color: ColorRow,
    pub(crate) text_color: ColorRow,
    pub(crate) _pad: [u8; 2],
}

/// An optional RGB color.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct ColorRow {
    pub(crate) present: u8,
    pub(crate) rgb: [u8; 3],
}

/// A row of `stops.txt`.
///
/// Rows that riders can board come first, and their index is their `StopId`.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct StopRow {
    pub(crate) feed: U32,
    pub(crate) gtfs_id: U32,
    pub(crate) name: U32,
    pub(crate) code: U32,
    pub(crate) platform: U32,
    /// Latitude in 1e-7 degrees, or `i32::MIN` when the stop has no coordinates.
    pub(crate) lat_e7: I32,
    pub(crate) lon_e7: I32,
    pub(crate) parent: U32,
    pub(crate) zone: U32,
    pub(crate) min_change: U32,
    pub(crate) children_start: U32,
    pub(crate) child_count: U32,
    pub(crate) location_type: u8,
    pub(crate) wheelchair_boarding: u8,
    /// [`STOP_NO_CHANGE`] or zero.
    pub(crate) flags: u8,
    pub(crate) _pad: u8,
}

/// A stop flag: `transfer_type=3` forbids changing here.
pub(crate) const STOP_NO_CHANGE: u8 = 1;

/// A calendar: `first_date + i` runs when bit `i` is set.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct ServiceRow {
    pub(crate) feed: U32,
    pub(crate) gtfs_id: U32,
    pub(crate) first_date: U32,
    pub(crate) day_count: U32,
    pub(crate) bits_start: U32,
}

/// One stop sequence of a route, with one set of attributes: a `valinor-transit` route.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct PatternRow {
    pub(crate) route: U32,
    pub(crate) stops_start: U32,
    pub(crate) stop_count: U32,
    pub(crate) trips_start: U32,
    pub(crate) trip_count: U32,
    pub(crate) attributes: u8,
    pub(crate) _pad: [u8; 3],
}

/// The attribute bit for a pattern on which bicycles are allowed.
pub(crate) const PATTERN_BIKES_ALLOWED: u8 = 1;
/// The attribute bit for a pattern whose trips aren't marked `wheelchair_accessible=2`.
pub(crate) const PATTERN_WHEELCHAIR_ACCESSIBLE: u8 = 2;

/// One run of a trip. A frequency-based trip has one row per expanded run.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct TripRow {
    pub(crate) pattern: U32,
    pub(crate) service: U32,
    pub(crate) times_start: U32,
    /// Per-position rule bytes, or [`NONE`] when every position is regular and exact.
    pub(crate) rules_start: U32,
}

/// Seconds from the start of the service day. Stops past midnight are simply larger.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct TimePair {
    pub(crate) arrival: I32,
    pub(crate) departure: I32,
}

/// The low two bits of a rule byte: GTFS `pickup_type`.
pub(crate) const RULE_PICKUP_MASK: u8 = 0b0011;
/// The next two bits of a rule byte: GTFS `drop_off_type`.
pub(crate) const RULE_DROP_OFF_SHIFT: u8 = 2;
/// A bit of a rule byte: this stop's time is approximate
/// (GTFS `timepoint` is `0`, or the time was interpolated).
pub(crate) const RULE_APPROXIMATE: u8 = 1 << 4;
/// Every bit a rule byte may use.
pub(crate) const RULE_VALID_BITS: u8 = 0b1_1111;

/// What a rider sees about a trip, out of the way of the data that is read to route it.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct TripRiderRow {
    pub(crate) gtfs_id: U32,
    pub(crate) headsign: U32,
    pub(crate) block: U32,
    pub(crate) shape: U32,
    /// Per-position strings, or [`NONE`] when the trip has no stop headsigns.
    pub(crate) stop_headsigns_start: U32,
    /// The per-position GTFS `stop_sequence` values.
    pub(crate) stop_sequences_start: U32,
    /// Per-position distances along the shape, or [`NONE`] when the trip has no shape.
    pub(crate) stop_dists_start: U32,
    pub(crate) wheelchair: u8,
    pub(crate) bikes: u8,
    /// GTFS `direction_id`, or [`DIRECTION_NONE`].
    pub(crate) direction: u8,
    /// [`RUN_SCHEDULED`], [`RUN_EXACT_FREQUENCY`], or [`RUN_HEADWAY_FREQUENCY`].
    pub(crate) frequency_run: u8,
}

/// A trip with its own schedule.
pub(crate) const RUN_SCHEDULED: u8 = 0;
/// One run of a `frequencies.txt` trip with `exact_times=1`, whose start times are real.
pub(crate) const RUN_EXACT_FREQUENCY: u8 = 1;
/// One run of a `frequencies.txt` trip with `exact_times=0`,
/// whose start times are a grid the compiler laid out from the headway.
pub(crate) const RUN_HEADWAY_FREQUENCY: u8 = 2;

/// The direction byte of a trip without a `direction_id`.
pub(crate) const DIRECTION_NONE: u8 = 0xff;

/// A shape: a run of [`ShapePointRow`].
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct ShapeRow {
    pub(crate) feed: U32,
    pub(crate) gtfs_id: U32,
    pub(crate) points_start: U32,
    pub(crate) point_count: U32,
}

/// A point of a shape, with its distance along the shape.
///
/// Distances are in the feed's own units when it gives them
/// and in meters when they were computed, and are only ever compared within one trip.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct ShapePointRow {
    pub(crate) lat_e7: I32,
    pub(crate) lon_e7: I32,
    pub(crate) dist: F32,
}

/// A `transfers.txt` rule for walking between two different stops, sorted by `(from, to)`.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct TransferPairRow {
    /// The stop the walk starts at, as a `StopId`.
    pub(crate) from: U32,
    /// The stop the walk ends at, as a `StopId`.
    pub(crate) to: U32,
    /// The walk costs at least this many seconds; [`NONE`] means the walk is forbidden.
    pub(crate) floor: U32,
}

/// One original trip's stretch of a combined stay-seated trip, sorted by `(trip, first_position)`.
///
/// A combined trip's rider row holds its first original trip;
/// each stretch here has what that one original trip's riders see.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct TripSegmentRow {
    /// The combined trip's row.
    pub(crate) trip: U32,
    pub(crate) first_position: U32,
    /// Inclusive.
    pub(crate) last_position: U32,
    pub(crate) gtfs_id: U32,
    /// The original trip's route, as a [`RouteRow`].
    pub(crate) route: U32,
    pub(crate) headsign: U32,
    /// The original trip's shape, or [`NONE`].
    pub(crate) shape: U32,
    pub(crate) wheelchair: u8,
    pub(crate) bikes: u8,
    /// GTFS `direction_id`, or [`DIRECTION_NONE`].
    pub(crate) direction: u8,
    pub(crate) _pad: u8,
}

/// Maps a feed-scoped GTFS identifier to a row, sorted by feed and then by string.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
pub(crate) struct IdIndexRow {
    pub(crate) feed: U32,
    pub(crate) id: U32,
    pub(crate) row: U32,
}
