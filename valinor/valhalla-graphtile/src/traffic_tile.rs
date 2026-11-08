//! # Traffic tile support
//!
//! This module provides data structures for working with Valhalla live traffic tiles.
//! These follow a different format from the routing graph:
//! a [`TrafficTileHeader`] followed by one [`TrafficSpeed`] per directed edge
//! of the matching graph tile.
//! This is the layout in mainline's `baldr/traffictile.h` (3.9.1).
//!
//! Live traffic is outside the scope of routing in Valinor:
//! [`select_speed`](crate::graph_tile::predicted_speeds::select_speed)
//! picks speeds as mainline does with no traffic tile.
//! This module is kept for reading and writing traffic tiles for other Valhalla systems.
//!
//! ## Shared memory
//!
//! Traffic tiles are meant to be memory mapped and updated in place by another process
//! while routers read them.
//! The types here are plain values:
//! [`TrafficSpeed`] is a `u64` bitfield (little endian) with no interior references,
//! so reading one means copying its 8 bytes out of the map, once,
//! with a volatile read, and then decoding the copy.
//! The traffic tile provider ([`crate::tile_provider::TrafficTileProvider`])
//! does exactly that, and documents the safety requirements of the memory map itself.
//!
//! Copying first is stronger than mainline,
//! which reads each bitfield member straight from volatile memory,
//! so a concurrent update can mix two versions of an edge's fields within one check.
//! Neither guarantees a single atomic 64-bit read:
//! [`TrafficSpeed`] has an alignment of 1 so that it can be read from any offset,
//! which leaves the compiler free to split the read
//! (in practice, the entries are 8-byte aligned in a tile and the read is a single load
//! on 64-bit targets).
//!
//! Nothing in this module is `unsafe`, and none of it panics.

use bitfield_struct::bitfield;
use nutype::nutype;
#[cfg(feature = "serde")]
use serde::{Serialize, Serializer, ser::SerializeStruct};
use thiserror::Error;
use zerocopy::{LE, U32, U64};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, Unaligned};

/// The bit-level representation signaling that the live speed is not known
/// (max value of a 7-bit number)
const UNKNOWN_TRAFFIC_SPEED_RAW: u8 = (1 << 7) - 1;
const UNKNOWN_TRAFFIC_SPEED_KPH: u8 = UNKNOWN_TRAFFIC_SPEED_RAW << 1;

/// The maximum allowed traffic speed, in kph.
///
/// Values larger than this are reserved, so you should cap speeds here.
pub const MAX_TRAFFIC_SPEED_KPH: u8 = (UNKNOWN_TRAFFIC_SPEED_RAW - 1) << 1;

/// The bit-level representation signaling that the road is closed
const CLOSED_TRAFFIC_SPEED_RAW: u8 = 0;

pub const UNKNOWN_CONGESTION_VAL: u8 = 0;
pub const MAX_CONGESTION_VAL: u8 = 63;

/// The Valhalla traffic tile version.
/// See the note on the `traffic_tile_version` field.
/// In Valhalla, this is the major version (`VALHALLA_VERSION_MAJOR`), which is 3 for 3.9.1.
pub const TRAFFIC_TILE_VERSION: u32 = 3;

/// The header for a traffic tile.
///
/// Every tile starts off with one of these.
#[derive(FromBytes, IntoBytes, Unaligned)]
#[repr(C)]
pub struct TrafficTileHeader {
    tile_id: U64<LE>,
    /// The last update date, in seconds since the epoch.
    last_update: U64<LE>,
    directed_edge_count: U32<LE>,
    /// An indicative version number of the traffic tile format.
    ///
    /// This is *not* intended to allow for easy migration between incompatible data formats.
    /// Changes are still expected to be applied in a backward-compatible way by using
    /// the spare bits available.
    ///
    /// This is more of a break-the-glass escape hatch for when the current format has
    /// reached its limits.
    traffic_tile_version: U32<LE>,
    // The traffic tile version used to be in the spare1 field ages ago.
    // I've kept the naming here to remain consistent with Valhalla.
    _spare2: U32<LE>,
    _spare3: U32<LE>,
}

impl TrafficTileHeader {
    #[inline]
    pub fn directed_edge_count(&self) -> u32 {
        self.directed_edge_count.get()
    }

    /// Gets the traffic tile version number.
    ///
    /// This is currently tied to the Valhalla major version number,
    /// and can be verified against [`TRAFFIC_TILE_VERSION`] to ensure that we understand the file.
    /// (Mainline treats every edge of a tile with another version as having no live speed.)
    #[inline]
    pub fn traffic_tile_version(&self) -> u32 {
        self.traffic_tile_version.get()
    }
}

/// A measurement of moving vehicle speed.
/// Use this when the road is not closed.
///
/// # Examples
///
/// ```
/// # use valhalla_graphtile::traffic_tile::{SpeedValue, MAX_TRAFFIC_SPEED_KPH};
/// let valid_speed = SpeedValue::try_new(42).expect("This is a valid value");
/// assert!(SpeedValue::try_new(0).is_err(), "Zero is a reserved value (use closed helpers on TrafficSpeed and TrafficSpeedBuilder).");
/// assert!(SpeedValue::try_new(MAX_TRAFFIC_SPEED_KPH + 1).is_err(), "Values above this speed are reserved.");
/// ```
///
/// # Indicating closed segments
///
/// Values must be non-zero. To indicate a closed segment,
/// use helpers like [`TrafficSpeed::closed`] or [`TrafficSpeedBuilder::with_closed_segment`]
#[cfg_attr(feature = "serde", nutype(const_fn, derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize), validate(greater=0, less_or_equal=MAX_TRAFFIC_SPEED_KPH)))]
#[cfg_attr(not(feature = "serde"), nutype(const_fn, derive(Copy, Clone, Eq, PartialEq, Debug), validate(greater=0, less_or_equal=MAX_TRAFFIC_SPEED_KPH)))]
pub struct SpeedValue(u8);

impl SpeedValue {
    /// Converts the speed value in kph to the encoded format for storage in a traffic tile.
    ///
    /// The encoding uses 7 bits of precision and quantizes to a resolution of 2kph.
    #[inline]
    pub const fn into_encoded_value(self) -> u8 {
        self.into_inner() >> 1
    }
}

/// A measure of congestion along a segment.
/// Use this to indicate a known congestion level.
///
/// # Examples
///
/// ```
/// # use valhalla_graphtile::traffic_tile::{CongestionValue, MAX_CONGESTION_VAL};
/// let valid_congestion = CongestionValue::try_new(1).expect("This is the minimum allowed value");
/// assert!(CongestionValue::try_new(0).is_err(), "Zero is a reserved value (None signifies unknown congestion in the TrafficSpeed and TrafficSpeedBuilder)");
/// assert!(CongestionValue::try_new(MAX_CONGESTION_VAL + 1).is_err(), "Values above this are not supported");
/// ```
#[cfg_attr(feature = "serde", nutype(const_fn, derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize), validate(greater=0, less_or_equal=MAX_CONGESTION_VAL)))]
#[cfg_attr(not(feature = "serde"), nutype(const_fn, derive(Copy, Clone, Eq, PartialEq, Debug), validate(greater=0, less_or_equal=MAX_CONGESTION_VAL)))]
pub struct CongestionValue(u8);

/// One of the (up to) three segments a [`TrafficSpeed`] divides an edge into.
///
/// Mainline calls these subsegments 0, 1, and 2.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TrafficSegment {
    /// From the start of the edge to `breakpoint1`.
    First,
    /// From `breakpoint1` to `breakpoint2`.
    Second,
    /// From `breakpoint2` to the end of the edge.
    Third,
}

impl TrafficSegment {
    /// Every segment, in order along the edge.
    pub const ALL: [Self; 3] = [Self::First, Self::Second, Self::Third];
}

/// The traffic conditions along a single segment in a traffic tile.
#[derive(Eq, PartialEq, Copy, Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum SegmentTrafficInfo {
    /// There is no data for this segment.
    ///
    /// If the breakpoint is 0 or 255, this also indicates there is no data for any future segment.
    NoData { breakpoint: u8 },
    /// The segment is closed.
    Closed { breakpoint: u8 },
    /// Traffic is flowing at the given speed.
    Speed {
        /// The traffic speed in kph.
        speed_kph: u8,
        /// An optional congestion value ranging from 1 to 63.
        congestion: Option<u8>,
        /// The breakpoint along the edge.
        ///
        /// This is a value between 1 and 255, where 255 is the end of the edge.
        breakpoint: u8,
    },
}

/// Traffic speeds over an edge.
///
/// This stores speed and optional congestion and incident information for a single edge in the graph.
/// For long edges with variable traffic conditions (e.g., motorways),
/// this structure allows breaking the edge up into 3 segments,
/// each with their own speed and congestion info.
///
/// The accessors follow mainline's `TrafficSpeed` (`speed_valid`, `closed`, `get_overall_speed`).
/// Segment-level accessors take a [`TrafficSegment`] rather than mainline's index,
/// so there's no out-of-range case.
#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, PartialEq, Eq)]
pub struct TrafficSpeed {
    /// The overall speed, with a representable range of 0-255 kph, in 2kph increments.
    /// This is stored as a convenience to keep the pathfinding loop tight.
    /// (When traversing an entire edge, it just uses the overall speed
    /// and doesn't need to compute from the individual sections.)
    ///
    /// Magic values:
    /// * 0 - road closed
    /// * 127 (the max 7-bit number = 255 if it were a real speed) - unknown speed
    ///
    /// This value is assumed to be invalid (unknown speed) if `breakpoint1` is 0.
    #[bits(7)]
    overall_encoded_speed: u8,
    // Segment encoded speeds, which divide the edge into (up to) 3 segments.
    // These use the same 0-255kph range with 2kph increments
    #[bits(7)]
    encoded_speed1: u8,
    #[bits(7)]
    encoded_speed2: u8,
    #[bits(7)]
    encoded_speed3: u8,
    // Breakpoints. The position = length * (breakpointN / 255)
    #[bits(8)]
    breakpoint1: u8,
    #[bits(8)]
    breakpoint2: u8,
    // Congestion info per segment.
    // 0 = unknown
    // 1 = no congestion
    // 63 = max congestion
    #[bits(6)]
    congestion1: u8,
    #[bits(6)]
    congestion2: u8,
    #[bits(6)]
    congestion3: u8,
    // Are there incidents on this edge in the corresponding incident tile?
    #[bits(1)]
    has_incidents: u8,
    #[bits(1)]
    _spare: u8,
}

impl TrafficSpeed {
    /// Returns true if the edge has valid speed information.
    /// NB: This information may indicate that the road is closed!
    ///
    /// This is mainline's `speed_valid`.
    #[inline]
    pub const fn has_valid_speed(&self) -> bool {
        self.breakpoint1() != 0 && self.overall_encoded_speed() != UNKNOWN_TRAFFIC_SPEED_RAW
    }

    /// Gets the overall speed (the weighted average speed when traversing the full edge), in kph.
    ///
    /// Returns `None` if no speed value is encoded for this edge.
    #[inline]
    pub const fn overall_speed(&self) -> Option<u8> {
        if self.has_valid_speed() {
            Some(self.overall_encoded_speed() << 1)
        } else {
            None
        }
    }

    /// Gets the traffic flow information for a segment of the edge.
    /// Use this when traversing only part of the edge.
    #[inline]
    pub const fn segment_info(&self, segment: TrafficSegment) -> SegmentTrafficInfo {
        let (raw, breakpoint, congestion) = match segment {
            TrafficSegment::First => (
                self.encoded_speed1(),
                self.breakpoint1(),
                self.congestion1(),
            ),
            TrafficSegment::Second => (
                self.encoded_speed2(),
                self.breakpoint2(),
                self.congestion2(),
            ),
            TrafficSegment::Third => (
                self.encoded_speed3(),
                // The third segment runs to the end of the edge, if it exists.
                if self.breakpoint2() != 0 && self.breakpoint2() != 255 {
                    255
                } else {
                    0
                },
                self.congestion3(),
            ),
        };

        if breakpoint == 0 || raw == UNKNOWN_TRAFFIC_SPEED_RAW {
            SegmentTrafficInfo::NoData { breakpoint }
        } else if raw == CLOSED_TRAFFIC_SPEED_RAW {
            SegmentTrafficInfo::Closed { breakpoint }
        } else {
            SegmentTrafficInfo::Speed {
                speed_kph: raw << 1,
                congestion: match congestion {
                    UNKNOWN_CONGESTION_VAL => None,
                    value => Some(value),
                },
                breakpoint,
            }
        }
    }

    /// Returns true if the edge is completely closed.
    ///
    /// This is mainline's `closed()`.
    /// Note that this does not mean all segments are passable!
    /// See [`TrafficSpeed::is_segment_closed`] to get segment-level detail.
    ///
    /// TODO: I'm still a bit fuzzy on the validity of partial closures.
    /// See `test_builder_no_data_and_closure_edge_case` for documentation of at least one edge case.
    #[inline]
    pub const fn is_completely_closed(&self) -> bool {
        self.breakpoint1() != 0 && self.overall_encoded_speed() == CLOSED_TRAFFIC_SPEED_RAW
    }

    /// Returns true if a specific segment is closed.
    ///
    /// Note that it may be partially closed, so check [`TrafficSpeed::is_completely_closed`] too!
    ///
    /// This is mainline's `closed(subsegment)`,
    /// except that a segment that doesn't exist (its starting breakpoint is 0 or 255)
    /// is never closed here.
    /// Mainline only asks about segments that exist
    /// (in `TripLegBuilder`, the only caller), so the two agree wherever mainline looks.
    #[inline]
    pub const fn is_segment_closed(&self, segment: TrafficSegment) -> bool {
        if !self.has_valid_speed() {
            return false;
        }

        match segment {
            TrafficSegment::First => {
                self.encoded_speed1() == CLOSED_TRAFFIC_SPEED_RAW
                    || self.congestion1() == MAX_CONGESTION_VAL
            }
            TrafficSegment::Second => {
                self.breakpoint1() < 255
                    && self.breakpoint1() > 0
                    && (self.encoded_speed2() == CLOSED_TRAFFIC_SPEED_RAW
                        || self.congestion2() == MAX_CONGESTION_VAL)
            }
            TrafficSegment::Third => {
                self.breakpoint2() < 255
                    && self.breakpoint2() > 0
                    && (self.encoded_speed3() == CLOSED_TRAFFIC_SPEED_RAW
                        || self.congestion3() == MAX_CONGESTION_VAL)
            }
        }
    }

    // Builder methods

    /// Constructs a new [`TrafficSpeed`] where a single speed is applied to the entire edge.
    ///
    /// For more complex cases where you want to break the edge up into segments
    /// with different speeds, use [`TrafficSpeedBuilder`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use valhalla_graphtile::traffic_tile::{CongestionValue, SpeedValue, TrafficSpeed};
    /// let speed = SpeedValue::try_new(42).expect("This is a valid speed");
    /// let tile_with_speed = TrafficSpeed::single_speed(speed, None);
    /// let tile_with_congestion = TrafficSpeed::single_speed(speed, Some(CongestionValue::try_new(12).expect("This should be a valid moderate congestion value")));
    /// ```
    #[inline]
    pub const fn single_speed(speed: SpeedValue, congestion: Option<CongestionValue>) -> Self {
        let encoded_speed = speed.into_encoded_value();
        Self::new()
            .with_overall_encoded_speed(encoded_speed)
            .with_encoded_speed1(encoded_speed)
            .with_congestion1(match congestion {
                None => 0,
                Some(congestion) => congestion.into_inner(),
            })
            .with_breakpoint1(255)
    }

    /// Constructs a new [`TrafficSpeed`] indicating that the entire edge is closed.
    ///
    /// For more complex cases where you want to break the edge up so it's only partially closed,
    /// use [`TrafficSpeedBuilder`].
    #[inline]
    pub const fn closed() -> Self {
        Self::new()
            .with_overall_encoded_speed(CLOSED_TRAFFIC_SPEED_RAW)
            .with_breakpoint1(255)
    }

    /// Returns a new instance indicating that there is an incident tile containing this edge.
    #[inline]
    #[must_use]
    pub const fn with_incident_tile_bit(self) -> Self {
        self.with_has_incidents(1)
    }
}

#[cfg(feature = "serde")]
impl Serialize for TrafficSpeed {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let num_fields = 7;
        let mut state = serializer.serialize_struct("TrafficSpeed", num_fields)?;

        state.serialize_field("has_valid_speed", &self.has_valid_speed())?;
        state.serialize_field("is_completely_closed", &self.is_completely_closed())?;
        state.serialize_field("overall_speed", &self.overall_speed())?;
        state.serialize_field("segment_info_0", &self.segment_info(TrafficSegment::First))?;
        state.serialize_field("segment_info_1", &self.segment_info(TrafficSegment::Second))?;
        state.serialize_field("segment_info_2", &self.segment_info(TrafficSegment::Third))?;
        state.serialize_field("has_incident_tile", &self.has_incidents())?;

        state.end()
    }
}

/// An error building a [`TrafficSpeed`].
#[derive(Error, Debug, PartialEq, Eq)]
pub enum TrafficSpeedBuilderError {
    #[error("build a traffic speed: no segments were added")]
    NoSegments,
    #[error("add a traffic speed segment: an edge has at most 3")]
    TooManySegments,
    #[error(
        "add a traffic speed segment of {new_section_length} m: with the {current_length} m added so far, the segments would be longer than the edge ({edge_length} m)"
    )]
    SectionLengthExceedsEdge {
        current_length: u32,
        new_section_length: u32,
        edge_length: u32,
    },
    #[error(
        "build a traffic speed: the segments cover {covered_length} m of the edge's {edge_length} m (add unknown segments for stretches with no data)"
    )]
    IncompleteCoverage {
        covered_length: u32,
        edge_length: u32,
    },
}

/// One segment added to a [`TrafficSpeedBuilder`].
#[derive(Copy, Clone, Debug)]
struct SegmentSpec {
    /// The speed in kph, [`CLOSED_TRAFFIC_SPEED_RAW`] for closed,
    /// or [`UNKNOWN_TRAFFIC_SPEED_KPH`] for unknown.
    /// (No [`SpeedValue`] can be either.)
    speed_kph: u8,
    congestion: u8,
    length: u32,
}

/// An incremental builder interface for [`TrafficSpeed`].
///
/// This builder does the work of transforming speed information into the Valhalla format,
/// and enforces that the lengths add up and you stay within the Valhalla constraints.
///
/// # Examples
///
/// You can use this builder to  mark an entire edge as either a single speed or completely closed,
/// but the [`TrafficSpeed::single_speed`] or [`TrafficSpeed::closed`] constructors
/// may be more convenient.
///
/// ```
/// # use valhalla_graphtile::traffic_tile::{TrafficSpeedBuilder, TrafficSpeed, SpeedValue};
/// const DESIRED_SPEED: u8 = 42;
/// // You can build a speed like this.
/// // Many steps are failable, so you'll get quick feedback when you try to do something illegal.
/// let built_speed = TrafficSpeedBuilder::with_edge_length(9000)
///     .with_speed_segment(SpeedValue::try_new(DESIRED_SPEED).unwrap(), None, 9000).unwrap()
///     .build().unwrap();
///
/// // If you're creating a single value that covers the whole edge,
/// // this is probably an easier method.
/// let speed = TrafficSpeed::single_speed(SpeedValue::try_new(DESIRED_SPEED).unwrap(), None);
/// assert_eq!(built_speed, speed);
/// ```
pub struct TrafficSpeedBuilder {
    edge_length: u32,
    segments: Vec<SegmentSpec>,
    /// The total length of `segments`, which never exceeds `edge_length`.
    current_length: u32,
}

impl TrafficSpeedBuilder {
    /// Constructs a new builder for an edge of the given length (in meters).
    #[inline]
    pub fn with_edge_length(edge_length: u32) -> Self {
        Self {
            edge_length,
            segments: Vec::with_capacity(3),
            current_length: 0,
        }
    }

    fn with_segment(
        mut self,
        speed_kph: u8,
        congestion: u8,
        length: u32,
    ) -> Result<Self, TrafficSpeedBuilderError> {
        if self.segments.len() == 3 {
            return Err(TrafficSpeedBuilderError::TooManySegments);
        }
        let current_length = self
            .current_length
            .checked_add(length)
            .filter(|&total| total <= self.edge_length)
            .ok_or(TrafficSpeedBuilderError::SectionLengthExceedsEdge {
                current_length: self.current_length,
                new_section_length: length,
                edge_length: self.edge_length,
            })?;
        self.segments.push(SegmentSpec {
            speed_kph,
            congestion,
            length,
        });
        self.current_length = current_length;
        Ok(self)
    }

    /// Adds a new segment to the builder with traffic flowing at some non-zero rate.
    ///
    /// The congestion information is optional;
    /// a `None` value indicates unknown/missing data.
    /// Use a value of 1 to indicate no congestion.
    ///
    /// # Errors
    ///
    /// This will fail under the following conditions:
    ///
    /// - The `length` makes the accumulated total greater than the edge length
    /// - There are already 3 segments (Valhalla limit)
    #[inline]
    pub fn with_speed_segment(
        self,
        speed: SpeedValue,
        congestion: Option<CongestionValue>,
        length: u32,
    ) -> Result<Self, TrafficSpeedBuilderError> {
        let congestion = match congestion {
            Some(value) => value.into_inner(),
            None => UNKNOWN_CONGESTION_VAL,
        };
        self.with_segment(speed.into_inner(), congestion, length)
    }

    /// Adds a new segment to the builder where traffic is completely stationary or the road is closed.
    ///
    /// # Errors
    ///
    /// This will fail under the following conditions:
    ///
    /// - The `length` makes the accumulated total greater than the edge length
    /// - There are already 3 segments (Valhalla limit)
    #[inline]
    pub fn with_closed_segment(self, length: u32) -> Result<Self, TrafficSpeedBuilderError> {
        self.with_segment(CLOSED_TRAFFIC_SPEED_RAW, UNKNOWN_CONGESTION_VAL, length)
    }

    /// Adds a new segment to the builder with no traffic data.
    ///
    /// # Errors
    ///
    /// This will fail under the following conditions:
    ///
    /// - The `length` makes the accumulated total greater than the edge length
    /// - There are already 3 segments (Valhalla limit)
    #[inline]
    pub fn with_unknown_segment(self, length: u32) -> Result<Self, TrafficSpeedBuilderError> {
        self.with_segment(UNKNOWN_TRAFFIC_SPEED_KPH, UNKNOWN_CONGESTION_VAL, length)
    }

    /// The length of segments added so far (in meters).
    #[inline]
    pub fn current_length(&self) -> u32 {
        self.current_length
    }

    /// Creates a [`TrafficSpeed`] from the builder.
    ///
    /// # Errors
    ///
    /// Fails if no segments were added,
    /// or the segments don't cover the whole edge.
    pub fn build(self) -> Result<TrafficSpeed, TrafficSpeedBuilderError> {
        if self.segments.is_empty() {
            return Err(TrafficSpeedBuilderError::NoSegments);
        }

        if self.current_length < self.edge_length {
            return Err(TrafficSpeedBuilderError::IncompleteCoverage {
                covered_length: self.current_length,
                edge_length: self.edge_length,
            });
        }

        // Compute a weighted average overall speed over the segments with known speeds
        // (closed segments count as 0 kph).
        // The numerator is at most 254 * 2^32, which fits in a u64.
        let (num, denom) = self
            .segments
            .iter()
            .filter(|segment| segment.speed_kph != UNKNOWN_TRAFFIC_SPEED_KPH)
            .fold((0u64, 0u64), |(acc_s, acc_l), segment| {
                (
                    acc_s + u64::from(segment.speed_kph) * u64::from(segment.length),
                    acc_l + u64::from(segment.length),
                )
            });

        let Some(overall_speed) = num.checked_div(denom) else {
            // This can only happen if the entire edge is marked as unknown.
            // This is valid, so we return a blank speed.
            return Ok(TrafficSpeed::default());
        };
        // A weighted average is no more than the largest speed averaged, which fits in a u8.
        let overall_speed = u8::try_from(overall_speed).unwrap_or(MAX_TRAFFIC_SPEED_KPH);

        let mut result = TrafficSpeed::new().with_overall_encoded_speed(overall_speed >> 1);

        // breakpointN is the progress along the edge at the end of segment N, scaled to 0-255.
        let mut segments = self.segments.iter();
        let mut end = 0;
        if let Some(segment) = segments.next() {
            end += segment.length;
            result = result
                .with_encoded_speed1(segment.speed_kph >> 1)
                .with_congestion1(segment.congestion)
                .with_breakpoint1(normalize_progress(end, self.edge_length));
        }
        if let Some(segment) = segments.next() {
            end += segment.length;
            result = result
                .with_encoded_speed2(segment.speed_kph >> 1)
                .with_congestion2(segment.congestion)
                .with_breakpoint2(normalize_progress(end, self.edge_length));
        }
        if let Some(segment) = segments.next() {
            // The third segment always ends at the end of the edge, so it has no breakpoint.
            result = result
                .with_encoded_speed3(segment.speed_kph >> 1)
                .with_congestion3(segment.congestion);
        }

        Ok(result)
    }
}

/// Scales progress along an edge (`pos` of `len` meters) to the range 0-255.
///
/// Progress beyond the end of the edge counts as the end,
/// and a zero-length edge has no progress.
#[inline]
fn normalize_progress(pos: u32, len: u32) -> u8 {
    if len == 0 {
        return 0;
    }
    let scaled = (u64::from(pos.min(len)) * 255) / u64::from(len);
    u8::try_from(scaled).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod test {
    use super::*;
    use proptest::proptest;
    use zerocopy::IntoBytes;

    #[test]
    fn test_traffic_speed_round_trip() {
        const DESIRED_SPEED: u8 = 42;

        let speed = TrafficSpeed::single_speed(SpeedValue::try_new(DESIRED_SPEED).unwrap(), None)
            .with_incident_tile_bit();

        let speed_as_bytes = speed.as_bytes();
        assert_eq!(speed_as_bytes, [149, 10, 0, 240, 15, 0, 0, 64]);
        let speed_as_u64 = speed.into_bits().get();
        assert_eq!(speed_as_u64.to_le_bytes(), speed_as_bytes);

        let mut x: u64 = 0;
        let ptr = &raw mut x;

        // SAFETY: `ptr` points to a live, aligned, writable u64,
        // and TrafficSpeed is 8 bytes with no invalid bit patterns and an alignment of 1,
        // so reading one from the u64's storage is valid.
        let round_tripped_speed: TrafficSpeed = unsafe {
            // Explicit LE write
            core::ptr::write(ptr, speed_as_u64.to_le());
            // Simulates a read from the memory map
            core::ptr::read_volatile(ptr.cast::<TrafficSpeed>())
        };

        assert!(round_tripped_speed.has_valid_speed());
        assert_eq!(round_tripped_speed.overall_speed(), Some(DESIRED_SPEED));
        assert_eq!(round_tripped_speed.breakpoint1(), 255);
        assert_eq!(round_tripped_speed.has_incidents(), 1);
    }

    #[test]
    fn test_builder_two_segments() {
        let speed = TrafficSpeedBuilder::with_edge_length(1000)
            // The first half of the edge is moving at 42kph with light congestion
            .with_speed_segment(
                SpeedValue::try_new(42).unwrap(),
                Some(CongestionValue::try_new(12).unwrap()),
                500,
            )
            .unwrap()
            // Higher speed now with less congestion
            .with_speed_segment(
                SpeedValue::try_new(86).unwrap(),
                Some(CongestionValue::try_new(1).unwrap()),
                500,
            )
            .unwrap()
            .build()
            .unwrap();

        assert!(!speed.is_completely_closed());
        // The overall speed will be in the middle
        assert_eq!(speed.overall_speed(), Some(64));

        assert_eq!(
            speed.segment_info(TrafficSegment::First),
            SegmentTrafficInfo::Speed {
                speed_kph: 42,
                congestion: Some(12),
                breakpoint: 127,
            }
        );
        assert_eq!(
            speed.segment_info(TrafficSegment::Second),
            SegmentTrafficInfo::Speed {
                speed_kph: 86,
                congestion: Some(1),
                breakpoint: 255,
            }
        );
        assert_eq!(
            speed.segment_info(TrafficSegment::Third),
            SegmentTrafficInfo::NoData {
                breakpoint: 0, // This segment doesn't exist
            }
        );

        // No segments are closed
        assert!(!speed.is_segment_closed(TrafficSegment::First));
        assert!(!speed.is_segment_closed(TrafficSegment::Second));
        assert!(!speed.is_segment_closed(TrafficSegment::Third));

        assert_eq!(speed.as_bytes(), &[160, 202, 10, 240, 247, 207, 4, 0]);
    }

    #[test]
    fn test_builder_two_segments_partial_closure() {
        let speed = TrafficSpeedBuilder::with_edge_length(1000)
            // The first half of the edge is moving at 42kph with light congestion
            .with_speed_segment(
                SpeedValue::try_new(42).unwrap(),
                Some(CongestionValue::try_new(12).unwrap()),
                500,
            )
            .unwrap()
            .with_closed_segment(500)
            .unwrap()
            .build()
            .unwrap();

        assert!(!speed.is_completely_closed());
        // The overall speed will be ~half (quantization error means we can't represent odd values)
        assert_eq!(speed.overall_speed(), Some(20));

        assert_eq!(
            speed.segment_info(TrafficSegment::First),
            SegmentTrafficInfo::Speed {
                speed_kph: 42,
                congestion: Some(12),
                breakpoint: 127,
            }
        );
        assert_eq!(
            speed.segment_info(TrafficSegment::Second),
            SegmentTrafficInfo::Closed { breakpoint: 255 }
        );
        assert_eq!(
            speed.segment_info(TrafficSegment::Third),
            SegmentTrafficInfo::NoData {
                breakpoint: 0, // This segment doesn't exist
            }
        );

        // The first segment is open, but the second segment (index 1) is closed
        assert!(!speed.is_segment_closed(TrafficSegment::First));
        assert!(speed.is_segment_closed(TrafficSegment::Second));
        assert!(!speed.is_segment_closed(TrafficSegment::Third));

        assert_eq!(speed.as_bytes(), &[138, 10, 0, 240, 247, 207, 0, 0]);
    }

    #[test]
    fn test_builder_three_segments() {
        let speed = TrafficSpeedBuilder::with_edge_length(1000)
            // The first half of the edge is moving at 42kph with light congestion
            .with_speed_segment(
                SpeedValue::try_new(42).unwrap(),
                Some(CongestionValue::try_new(12).unwrap()),
                500,
            )
            .unwrap()
            // Higher speed now with less congestion
            .with_speed_segment(
                SpeedValue::try_new(86).unwrap(),
                Some(CongestionValue::try_new(1).unwrap()),
                250,
            )
            .unwrap()
            // And a third segment for good measure...
            .with_speed_segment(
                SpeedValue::try_new(34).unwrap(),
                Some(CongestionValue::try_new(47).unwrap()),
                250,
            )
            .unwrap()
            .build()
            .unwrap();

        assert!(!speed.is_completely_closed());
        // The overall speed will be in the middle
        assert_eq!(speed.overall_speed(), Some(50));

        assert_eq!(
            speed.segment_info(TrafficSegment::First),
            SegmentTrafficInfo::Speed {
                speed_kph: 42,
                congestion: Some(12),
                breakpoint: 127,
            }
        );
        assert_eq!(
            speed.segment_info(TrafficSegment::Second),
            SegmentTrafficInfo::Speed {
                speed_kph: 86,
                congestion: Some(1),
                breakpoint: 191,
            }
        );
        assert_eq!(
            speed.segment_info(TrafficSegment::Third),
            SegmentTrafficInfo::Speed {
                speed_kph: 34,
                congestion: Some(47),
                breakpoint: 255,
            }
        );

        // No segments are closed
        assert!(!speed.is_segment_closed(TrafficSegment::First));
        assert!(!speed.is_segment_closed(TrafficSegment::Second));
        assert!(!speed.is_segment_closed(TrafficSegment::Third));

        assert_eq!(speed.as_bytes(), &[153, 202, 42, 242, 247, 203, 4, 47]);
    }

    #[test]
    fn test_builder_no_data() {
        let speed = TrafficSpeedBuilder::with_edge_length(1000)
            .with_unknown_segment(1000)
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(speed, TrafficSpeed::default());
    }

    #[test]
    fn test_builder_partial_no_data() {
        let speed = TrafficSpeedBuilder::with_edge_length(1000)
            .with_unknown_segment(500)
            .unwrap()
            // .with_closed_segment(500)
            .with_speed_segment(SpeedValue::try_new(42).unwrap(), None, 500)
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(
            speed.segment_info(TrafficSegment::First),
            SegmentTrafficInfo::NoData { breakpoint: 127 }
        );
        assert_eq!(
            speed.segment_info(TrafficSegment::Second),
            SegmentTrafficInfo::Speed {
                speed_kph: 42,
                congestion: None,
                breakpoint: 255,
            }
        );
        assert_eq!(
            speed.segment_info(TrafficSegment::Third),
            SegmentTrafficInfo::NoData { breakpoint: 0 }
        );
    }

    #[test]
    fn test_builder_no_data_and_closure_edge_case() {
        let speed = TrafficSpeedBuilder::with_edge_length(1000)
            .with_unknown_segment(500)
            .unwrap()
            .with_closed_segment(500)
            .unwrap()
            .build()
            .unwrap();

        // As the Valhalla logic is currently written,
        // such an edge is marked as closed since the first breakpoint is nonzero
        // but the overall edge speed is zero.
        // Detecting this scenario in the builder and setting the overall speed to "unknown"
        // would render the entire tile as not having valid speed (so it's just ignored as if there were no data).
        // That's not quite right either ;)
        // This is a bit of an edge case in Valhalla IMO.
        // This unit test documents the current, somewhat confusing behavior.
        assert!(speed.is_completely_closed());
        assert_eq!(
            speed.segment_info(TrafficSegment::First),
            SegmentTrafficInfo::NoData { breakpoint: 127 }
        );
        assert_eq!(
            speed.segment_info(TrafficSegment::Second),
            SegmentTrafficInfo::Closed { breakpoint: 255 }
        );
        assert_eq!(
            speed.segment_info(TrafficSegment::Third),
            SegmentTrafficInfo::NoData { breakpoint: 0 }
        );
    }

    #[test]
    fn test_builder_errors() {
        assert_eq!(
            TrafficSpeedBuilder::with_edge_length(10).build(),
            Err(TrafficSpeedBuilderError::NoSegments)
        );
        assert_eq!(
            TrafficSpeedBuilder::with_edge_length(10)
                .with_unknown_segment(4)
                .unwrap()
                .build(),
            Err(TrafficSpeedBuilderError::IncompleteCoverage {
                covered_length: 4,
                edge_length: 10
            })
        );
        assert_eq!(
            TrafficSpeedBuilder::with_edge_length(10)
                .with_closed_segment(4)
                .unwrap()
                .with_closed_segment(7)
                .err(),
            Some(TrafficSpeedBuilderError::SectionLengthExceedsEdge {
                current_length: 4,
                new_section_length: 7,
                edge_length: 10
            })
        );
        assert_eq!(
            TrafficSpeedBuilder::with_edge_length(3)
                .with_closed_segment(1)
                .and_then(|b| b.with_closed_segment(1))
                .and_then(|b| b.with_closed_segment(1))
                .and_then(|b| b.with_closed_segment(0))
                .err(),
            Some(TrafficSpeedBuilderError::TooManySegments)
        );
    }

    #[test]
    fn test_builder_lengths_near_u32_max() {
        // Lengths that would overflow a u32 sum are an error, not a panic.
        let builder = TrafficSpeedBuilder::with_edge_length(u32::MAX)
            .with_speed_segment(
                SpeedValue::try_new(MAX_TRAFFIC_SPEED_KPH).unwrap(),
                None,
                u32::MAX,
            )
            .unwrap();
        assert!(matches!(
            builder.with_closed_segment(1),
            Err(TrafficSpeedBuilderError::SectionLengthExceedsEdge { .. })
        ));

        // And the weighted average of the longest segments doesn't overflow.
        let speed = TrafficSpeedBuilder::with_edge_length(u32::MAX)
            .with_speed_segment(
                SpeedValue::try_new(MAX_TRAFFIC_SPEED_KPH).unwrap(),
                None,
                u32::MAX - 1,
            )
            .unwrap()
            .with_speed_segment(SpeedValue::try_new(MAX_TRAFFIC_SPEED_KPH).unwrap(), None, 1)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(speed.overall_speed(), Some(MAX_TRAFFIC_SPEED_KPH));
        assert_eq!(
            speed.segment_info(TrafficSegment::Second),
            SegmentTrafficInfo::Speed {
                speed_kph: MAX_TRAFFIC_SPEED_KPH,
                congestion: None,
                breakpoint: 255
            }
        );
    }

    #[test]
    fn test_zero_length_edge() {
        let speed = TrafficSpeedBuilder::with_edge_length(0)
            .with_speed_segment(SpeedValue::try_new(42).unwrap(), None, 0)
            .unwrap()
            .build()
            .unwrap();
        // Nothing to average over, so there is no speed.
        assert_eq!(speed, TrafficSpeed::default());
        assert!(!speed.has_valid_speed());
    }

    #[test]
    fn test_default_has_no_data() {
        let speed = TrafficSpeed::default();
        assert!(!speed.has_valid_speed());
        assert!(!speed.is_completely_closed());
        for segment in TrafficSegment::ALL {
            assert!(!speed.is_segment_closed(segment));
            assert!(matches!(
                speed.segment_info(segment),
                SegmentTrafficInfo::NoData { .. }
            ));
        }
    }

    proptest! {
        #[test]
        fn prop_zero_progress_is_always_zero(len: u32) {
            assert_eq!(normalize_progress(0, len), 0);
        }

        #[test]
        fn prop_progress_eq_nonzero_len_is_max(value in 1u32..) {
            assert_eq!(normalize_progress(value, value), 255);
        }

        #[test]
        fn prop_progress_past_the_end_is_the_end(len in 1u32.., extra: u32) {
            assert_eq!(normalize_progress(len.saturating_add(extra), len), 255);
        }

        /// Any bit pattern decodes without panicking.
        #[test]
        fn prop_any_bits_decode(bits: u64) {
            let speed = TrafficSpeed::from_bits(bits.into());
            for segment in TrafficSegment::ALL {
                let _ = speed.segment_info(segment);
                let _ = speed.is_segment_closed(segment);
            }
            assert_eq!(speed.overall_speed().is_some(), speed.has_valid_speed());
            // The first segment is closed exactly when segment_info says so,
            // unless it's closed by congestion alone.
            if speed.has_valid_speed() && speed.congestion1() != MAX_CONGESTION_VAL {
                assert_eq!(
                    speed.is_segment_closed(TrafficSegment::First),
                    matches!(
                        speed.segment_info(TrafficSegment::First),
                        SegmentTrafficInfo::Closed { .. }
                    )
                );
            }
        }
    }
}
