//! Directed edges: the per-edge records that routing reads for every edge it expands.
//!
//! The layout matches `baldr/directededge.h` in Valhalla 3.9.1 bit for bit.
//! Accessors keep Valhalla's semantics,
//! but return enums, sets, and options where Valhalla returns raw integers.
//! Decoding never panics, even on corrupt data:
//! fields whose bit width admits values that Valhalla never writes
//! return a [`Result`] with an [`UnknownValueError`] instead.

use crate::graph_tile::node::MAX_LOCAL_EDGE_INDEX;
use crate::{Access, GraphId, RoadClass, RoadUse, Surface};
use bitfield_struct::bitfield;
use core::fmt::{Debug, Formatter};
use enumset::EnumSet;
#[cfg(feature = "serde")]
use serde::{Serialize, Serializer, ser::SerializeStruct};
use thiserror::Error;
use zerocopy::{LE, U16, U32, U64};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, Unaligned};

/// A field held a value that Valhalla never writes,
/// which almost always means the tile is corrupt.
#[derive(Debug, Error, Copy, Clone, PartialEq, Eq)]
#[error("decode {field}: {value} is not a known value")]
pub struct UnknownValueError {
    field: &'static str,
    value: u8,
}

impl UnknownValueError {
    #[inline]
    pub(crate) const fn new(field: &'static str, value: u8) -> Self {
        Self { field, value }
    }

    /// The name of the field that held the unknown value.
    #[inline]
    pub const fn field(&self) -> &'static str {
        self.field
    }

    /// The raw value that was read.
    #[inline]
    pub const fn value(&self) -> u8 {
        self.value
    }
}

/// A set of local edge indexes (0 through 7) at a node.
///
/// Valhalla stores several per-transition flags as one bit per local edge index:
/// simple turn restrictions, name consistency, and whether there is an edge to the left
/// or right of a turn.
/// Indexes above 7 are never members,
/// which matches what Valhalla reads for them.
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct LocalEdgeMask(u8);

impl LocalEdgeMask {
    /// Wraps a raw bit mask, where bit `i` stands for local edge index `i`.
    #[inline]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// The raw bit mask, where bit `i` stands for local edge index `i`.
    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Is the local edge index in the set?
    ///
    /// Always `false` for indexes above 7.
    #[inline]
    pub const fn contains(self, local_edge_index: u8) -> bool {
        local_edge_index <= MAX_LOCAL_EDGE_INDEX && self.0 & (1 << local_edge_index) != 0
    }

    /// Is the set empty?
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Iterates over the local edge indexes in the set, in ascending order.
    #[inline]
    pub fn iter(self) -> impl Iterator<Item = u8> {
        (0..=MAX_LOCAL_EDGE_INDEX).filter(move |&i| self.contains(i))
    }
}

impl Debug for LocalEdgeMask {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// The kind of bicycle lane along an edge.
///
/// Higher values are more favorable to cycling.
/// On cycleways, footways, and paths, Valhalla gives the values an alternate meaning,
/// noted on each variant.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum CycleLane {
    /// No specified bicycle lane.
    None = 0,
    /// A lane shared with other traffic.
    /// On paths, a path shared with pedestrians.
    Shared = 1,
    /// A dedicated cycle lane.
    /// On paths, a path with segregated lanes.
    Dedicated = 2,
    /// A cycle lane physically separated from the main carriageway.
    /// On paths, a path with no pedestrians on it.
    Separated = 3,
}

impl CycleLane {
    const fn from_bits(value: u8) -> Self {
        // The field is two bits wide, so every value is a variant.
        match value & 0b11 {
            0 => Self::None,
            1 => Self::Shared,
            2 => Self::Dedicated,
            _ => Self::Separated,
        }
    }

    const fn into_bits(self) -> u8 {
        self as u8
    }
}

/// The SAC hiking difficulty scale of an edge (OSM `sac_scale`).
///
/// Variants are ordered from easiest to hardest,
/// so costing can compare against a maximum difficulty.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum SacScale {
    /// Not tagged with a hiking difficulty.
    None = 0,
    /// T1: hiking.
    Hiking = 1,
    /// T2: mountain hiking.
    MountainHiking = 2,
    /// T3: demanding mountain hiking.
    DemandingMountainHiking = 3,
    /// T4: alpine hiking.
    AlpineHiking = 4,
    /// T5: demanding alpine hiking.
    DemandingAlpineHiking = 5,
    /// T6: difficult alpine hiking.
    DifficultAlpineHiking = 6,
}

impl SacScale {
    const fn try_from_bits(value: u8) -> Result<Self, UnknownValueError> {
        Ok(match value {
            0 => Self::None,
            1 => Self::Hiking,
            2 => Self::MountainHiking,
            3 => Self::DemandingMountainHiking,
            4 => Self::AlpineHiking,
            5 => Self::DemandingAlpineHiking,
            6 => Self::DifficultAlpineHiking,
            _ => return Err(UnknownValueError::new("sac_scale", value)),
        })
    }
}

/// The HOV occupancy requirement of an HOV-only edge.
///
/// Only meaningful when [`DirectedEdge::is_hov_only`] is `true`.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum HovType {
    /// Two or more occupants.
    Hov2 = 0,
    /// Three or more occupants.
    Hov3 = 1,
}

impl HovType {
    const fn from_bits(value: u8) -> Self {
        if value & 1 == 0 {
            Self::Hov2
        } else {
            Self::Hov3
        }
    }

    const fn into_bits(self) -> u8 {
        self as u8
    }
}

/// Where an edge's [`speed`](DirectedEdge::speed) came from.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum SpeedType {
    /// Tagged maximum speed.
    Tagged = 0,
    /// Assigned from the road classification.
    Classified = 1,
}

impl SpeedType {
    const fn from_bits(value: u8) -> Self {
        if value & 1 == 0 {
            Self::Tagged
        } else {
            Self::Classified
        }
    }

    const fn into_bits(self) -> u8 {
        self as u8
    }
}

/// The type of turn from one edge onto another (Valhalla's `Turn::Type`).
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum TurnType {
    Straight = 0,
    SlightRight = 1,
    Right = 2,
    SharpRight = 3,
    /// A U-turn.
    Reverse = 4,
    SharpLeft = 5,
    Left = 6,
    SlightLeft = 7,
}

impl TurnType {
    const fn from_bits(value: u8) -> Self {
        // Three bits, so every value is a variant.
        match value & 0b111 {
            0 => Self::Straight,
            1 => Self::SlightRight,
            2 => Self::Right,
            3 => Self::SharpRight,
            4 => Self::Reverse,
            5 => Self::SharpLeft,
            6 => Self::Left,
            _ => Self::SlightLeft,
        }
    }
}

// Single-bit flags are stored as `u8` and exposed as `bool` by the accessors on `DirectedEdge`.

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct FirstBitfield {
    #[bits(46, from = bit_twiddling_helpers::conv_u64le::from_inner, into = bit_twiddling_helpers::conv_u64le::into_inner)]
    end_node: U64<LE>,
    #[bits(8)]
    restrictions: u8,
    #[bits(7, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    opposing_edge_index: U32<LE>,
    #[bits(1)]
    is_edge_info_forward: u8,
    #[bits(1)]
    leaves_tile: u8,
    #[bits(1)]
    country_crossing: u8,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct SecondBitfield {
    #[bits(25, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    edge_info_offset: U32<LE>,
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    access_restrictions: U16<LE>,
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    start_restriction: U16<LE>,
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    end_restriction: U16<LE>,
    #[bits(1)]
    complex_restriction: u8,
    #[bits(1)]
    dest_only: u8,
    #[bits(1)]
    no_thru: u8,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct ThirdBitfield {
    #[bits(8)]
    speed: u8,
    #[bits(8)]
    free_flow_speed: u8,
    #[bits(8)]
    constrained_flow_speed: u8,
    #[bits(8)]
    truck_speed: u8,
    #[bits(8)]
    name_consistency: u8,
    // Six bits, but Valhalla only defines some of the values,
    // so this stays raw and is decoded fallibly.
    #[bits(6)]
    road_use: u8,
    #[bits(4)]
    lane_count: u8,
    #[bits(4)]
    density: u8,
    #[bits(3)]
    classification: RoadClass,
    #[bits(3)]
    surface: Surface,
    #[bits(1)]
    toll: u8,
    #[bits(1)]
    roundabout: u8,
    #[bits(1)]
    truck_route: u8,
    #[bits(1)]
    has_predicted_speed: u8,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct FourthBitfield {
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    forward_access: U16<LE>,
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    reverse_access: U16<LE>,
    #[bits(5)]
    max_up_slope: u8,
    #[bits(5)]
    max_down_slope: u8,
    #[bits(3)]
    sac_scale: u8,
    #[bits(2)]
    cycle_lane: CycleLane,
    #[bits(1)]
    is_bike_network: u8,
    #[bits(1)]
    use_sidepath: u8,
    #[bits(1)]
    bicycle_dismount: u8,
    #[bits(1)]
    has_sidewalk_left: u8,
    #[bits(1)]
    has_sidewalk_right: u8,
    #[bits(1)]
    has_shoulder: u8,
    #[bits(1)]
    has_lane_connectivity: u8,
    #[bits(1)]
    has_turn_lanes: u8,
    #[bits(1)]
    has_sign: u8,
    #[bits(1)]
    is_intersection_internal: u8,
    #[bits(1)]
    is_tunnel: u8,
    #[bits(1)]
    is_bridge: u8,
    #[bits(1)]
    has_traffic_signal: u8,
    // Formerly "seasonal", which Valhalla never used.
    // In 3.9.1 this is `spare1_`, free to be reclaimed.
    #[bits(1)]
    _spare1: u8,
    #[bits(1)]
    is_dead_end: u8,
    #[bits(1)]
    has_bss_connection: u8,
    #[bits(1)]
    has_stop_sign: u8,
    #[bits(1)]
    has_yield_sign: u8,
    #[bits(1)]
    hov_type: HovType,
    #[bits(1)]
    is_indoor: u8,
    #[bits(1)]
    is_lit: u8,
    #[bits(1)]
    is_dest_only_hgv: u8,
    #[bits(3)]
    _spare4: u8,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct FifthBitfield {
    // Three bits per local edge index.
    #[bits(24, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    turn_type: U32<LE>,
    #[bits(8)]
    edge_to_left: u8,
    #[bits(24, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    length: U32<LE>,
    #[bits(4)]
    weighted_grade: u8,
    #[bits(4)]
    curvature: u8,
}

#[bitfield(u32,
    repr = U32<LE>,
    from = bit_twiddling_helpers::conv_u32le::from_inner,
    into = bit_twiddling_helpers::conv_u32le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct StopImpact {
    // Three bits per local edge index.
    #[bits(24, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    impact_between_edges: U32<LE>,
    #[bits(8)]
    edge_to_right: u8,
}

/// Stores either the stop impact or the transit line identifier.
/// Since transit lines are schedule-based, they have no need for edge transition logic,
/// so we can freely share this field.
///
/// Mainline declares a union; both of its members are the same four bytes,
/// so this keeps the raw word and reads it as either.
/// (Deriving `IntoBytes` for a union needs `--cfg zerocopy_derive_union_into_bytes`,
/// which crates that depend on this one would all have to set,
/// and which cargokit's Android builds override with their own `RUSTFLAGS`.)
#[repr(transparent)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, Copy, Clone)]
struct StopOrLine(U32<LE>);

impl StopOrLine {
    const fn stop_impact(self) -> StopImpact {
        StopImpact::from_bits(self.0)
    }

    const fn line_id(self) -> u32 {
        self.0.get()
    }
}

#[bitfield(u32,
    repr = U32<LE>,
    from = bit_twiddling_helpers::conv_u32le::from_inner,
    into = bit_twiddling_helpers::conv_u32le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct SeventhBitField {
    #[bits(7)]
    local_level_edge_index: u8,
    #[bits(7)]
    local_level_opp_edge_index: u8,
    #[bits(7)]
    shortcut: u8,
    #[bits(7)]
    superseded: u8,
    #[bits(1)]
    is_shortcut: u8,
    #[bits(1)]
    speed_type: SpeedType,
    #[bits(1)]
    is_named: u8,
    #[bits(1)]
    link_tag: u8,
}

impl Debug for StopOrLine {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "StopOrLine {{ raw_value: {} }}", self.line_id())
    }
}

/// Reads the `width`-bit group for a local edge index out of a packed per-index field.
///
/// Returns `None` for indexes above [`MAX_LOCAL_EDGE_INDEX`].
#[inline]
const fn packed_per_index(packed: u32, width: u32, local_edge_index: u8) -> Option<u8> {
    if local_edge_index > MAX_LOCAL_EDGE_INDEX {
        return None;
    }
    let shift = width * local_edge_index as u32;
    let mask = (1u32 << width) - 1;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "The mask keeps at most three bits."
    )]
    Some(((packed >> shift) & mask) as u8)
}

/// Decodes a 5-bit max slope magnitude, in percent.
///
/// Values up to 15 are stored exactly (rounded up to whole percent).
/// With the high bit set, the low four bits count 4 % steps above 16 %,
/// so the largest representable slope is 76 %.
#[inline]
const fn decode_slope_magnitude(raw: u8) -> u8 {
    if raw & 0x10 == 0 {
        raw
    } else {
        16 + (raw & 0xf) * 4
    }
}

/// Converts a 12-bit access mask into a set of access modes.
#[inline]
fn access_set(bits: u16) -> EnumSet<Access> {
    // SAFETY: Every bitfield passed here is 12 bits wide,
    // and `Access` has exactly 12 variants, so no invalid bits can be set.
    unsafe { EnumSet::from_repr_unchecked(bits) }
}

/// A directed edge within the routing graph.
///
/// This struct contains only the essential edge information
/// which is needed for routing calculations.
/// Additional details can be found in the [`EdgeInfo`](super::EdgeInfo) struct,
/// which contains things like the encoded shape, OSM way ID,
/// and other info that is not necessary for making routing decisions.
///
/// Several accessors take a *local edge index*:
/// the index (0 through 7) of an edge among the regular edges at a node,
/// counted across all hierarchy levels.
/// See [`local_edge_index`](Self::local_edge_index).
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, Debug, Clone)]
#[repr(C)]
pub struct DirectedEdge {
    first_bitfield: FirstBitfield,
    second_bitfield: SecondBitfield,
    third_bitfield: ThirdBitfield,
    fourth_bitfield: FourthBitfield,
    fifth_bitfield: FifthBitfield,
    stop_impact_or_line: StopOrLine,
    seventh_bitfield: SeventhBitField,
}

impl DirectedEdge {
    // Topology

    /// The end node ID for this directed edge.
    #[inline]
    pub const fn end_node_id(&self) -> GraphId {
        // SAFETY: We know that the bit field cannot contain a value
        // larger than the max allowed value (it's limited to 46 bits).
        unsafe { GraphId::from_id_unchecked(self.first_bitfield.end_node()) }
    }

    /// Gets the index of the opposing directed edge at the end node of this directed edge.
    ///
    /// Can be used to find the start node of this directed edge.
    #[inline]
    pub const fn opposing_edge_index(&self) -> u32 {
        self.first_bitfield.opposing_edge_index().get()
    }

    /// Does this edge end outside the current tile?
    #[inline]
    pub const fn leaves_tile(&self) -> bool {
        self.first_bitfield.leaves_tile() != 0
    }

    /// The index of this edge among the regular edges at its start node,
    /// counted across all hierarchy levels.
    ///
    /// Look this edge up with it in the [`restrictions`](Self::restrictions)
    /// of an edge arriving at the start node,
    /// and in the per-edge data on the start [`NodeInfo`](super::NodeInfo)
    /// (such as [`heading`](super::NodeInfo::heading)).
    /// Only indexes 0 through 7 carry that data;
    /// Valhalla stores up to 127 here.
    #[inline]
    pub const fn local_edge_index(&self) -> u8 {
        self.seventh_bitfield.local_level_edge_index()
    }

    /// The local edge index of the opposing edge at the end node.
    ///
    /// Costing uses this to detect U-turns and to look up per-transition data
    /// (such as [`turn_type`](Self::turn_type)) when entering the next edge.
    #[inline]
    pub const fn opposing_local_index(&self) -> u8 {
        self.seventh_bitfield.local_level_opp_edge_index()
    }

    // Edge info

    /// Gets the offset into the variable sized edge info field.
    #[inline]
    pub const fn edge_info_offset(&self) -> u32 {
        self.second_bitfield.edge_info_offset().get()
    }

    /// Is the edge info forward or reverse?
    ///
    /// Refer to the [`EdgeInfo`](crate::graph_tile::EdgeInfo) docs for info on why this matters.
    #[inline]
    pub const fn edge_info_is_forward(&self) -> bool {
        self.first_bitfield.is_edge_info_forward() != 0
    }

    /// Does this edge have names?
    #[inline]
    pub const fn is_named(&self) -> bool {
        self.seventh_bitfield.is_named() != 0
    }

    /// The local edge indexes at the start node whose names match this edge's names.
    ///
    /// Guidance uses this to decide whether a road continues through an intersection.
    #[inline]
    pub const fn name_consistency(&self) -> LocalEdgeMask {
        LocalEdgeMask(self.third_bitfield.name_consistency())
    }

    /// Does the edge have sign records (such as exit signs or guide signs)?
    #[inline]
    pub const fn has_sign(&self) -> bool {
        self.fourth_bitfield.has_sign() != 0
    }

    /// Does the edge have lane connectivity records?
    #[inline]
    pub const fn has_lane_connectivity(&self) -> bool {
        self.fourth_bitfield.has_lane_connectivity() != 0
    }

    /// Does the edge have turn lane records (at the end of the edge)?
    #[inline]
    pub const fn has_turn_lanes(&self) -> bool {
        self.fourth_bitfield.has_turn_lanes() != 0
    }

    // Hierarchy and shortcuts

    /// Is this edge a shortcut?
    #[inline]
    pub const fn is_shortcut(&self) -> bool {
        self.seventh_bitfield.is_shortcut() != 0
    }

    /// Gets the index of this shortcut among the shortcuts leaving its start node
    /// (starting from _zero_, unlike Valhalla).
    ///
    /// Returns `None` if this edge is not a shortcut,
    /// or if it is one beyond the seven that Valhalla can number.
    #[inline]
    pub const fn shortcut_index(&self) -> Option<u32> {
        let mask = self.seventh_bitfield.shortcut();
        if !self.is_shortcut() || mask == 0 {
            None
        } else {
            Some(mask.trailing_zeros())
        }
    }

    /// Determines if a directed edge is a valid _potential_ member of a shortcut.
    ///
    /// This is used for forming and resolving shortcuts only.
    /// Returns `false` if it is already a shortcut!
    #[inline]
    pub fn can_form_shortcut(&self) -> bool {
        if self.is_shortcut() || self.has_bss_connection() {
            return false;
        }
        // An unknown use is not one of the excluded uses, as in Valhalla.
        !matches!(
            self.road_use(),
            // Transit-related uses
            Ok(RoadUse::TransitConnection
                | RoadUse::PlatformConnection
                | RoadUse::EgressConnection
                // Roads under construction
                | RoadUse::Construction)
        )
    }

    /// Is this edge superseded by a shortcut?
    #[inline]
    pub const fn is_superseded(&self) -> bool {
        self.seventh_bitfield.superseded() != 0
    }

    /// Gets the index of the shortcut edge that supersedes this edge
    /// (within the end node's outbound edge list).
    ///
    /// Returns `None` if [`Self::is_superseded`] is false.
    /// Otherwise, returns an index into the list (starting from _zero_, unlike Valhalla!).
    #[inline]
    pub const fn superseded_index(&self) -> Option<u32> {
        let mask = self.seventh_bitfield.superseded();
        if mask == 0 {
            None
        } else {
            Some(mask.trailing_zeros())
        }
    }

    // Classification

    /// The way the edge is used.
    ///
    /// # Errors
    ///
    /// Returns an error if the tile holds a use that Valhalla does not define,
    /// which only happens with corrupt data.
    /// [`GraphTileView`](crate::graph_tile::GraphTileView) rejects such tiles when it parses them,
    /// so this is always `Ok` for edges read through a parsed tile.
    #[inline]
    pub fn road_use(&self) -> Result<RoadUse, UnknownValueError> {
        let raw = self.third_bitfield.road_use();
        RoadUse::try_from_bits(raw).ok_or(UnknownValueError::new("use", raw))
    }

    /// Is this a road (a regular road or a service road)?
    #[inline]
    pub fn is_road(&self) -> bool {
        matches!(self.road_use(), Ok(RoadUse::Road | RoadUse::ServiceRoad))
    }

    /// Is this a transit line (bus or rail)?
    #[inline]
    pub fn is_transit_line(&self) -> bool {
        matches!(self.road_use(), Ok(RoadUse::Rail | RoadUse::Bus))
    }

    /// The classification of the road.
    #[inline]
    pub const fn classification(&self) -> RoadClass {
        self.third_bitfield.classification()
    }

    /// Is this edge a link (OSM `*_link`: a ramp or turn channel)?
    #[inline]
    pub const fn is_link(&self) -> bool {
        self.seventh_bitfield.link_tag() != 0
    }

    /// Is this edge internal to an intersection?
    ///
    /// These are short edges joining the carriageways of divided roads at an intersection.
    /// Guidance folds them into the surrounding maneuver
    /// rather than treating them as roads of their own.
    #[inline]
    pub const fn is_intersection_internal(&self) -> bool {
        self.fourth_bitfield.is_intersection_internal() != 0
    }

    /// The generalized road surface type.
    #[inline]
    pub const fn surface(&self) -> Surface {
        self.third_bitfield.surface()
    }

    /// Is the surface unpaved ([`Surface::Compacted`] or worse)?
    #[inline]
    pub const fn is_unpaved(&self) -> bool {
        self.third_bitfield.surface() as u8 >= Surface::Compacted as u8
    }

    /// The SAC hiking difficulty of the edge.
    ///
    /// # Errors
    ///
    /// The field has room for one value that Valhalla never writes;
    /// reading it returns an error, which only happens with corrupt data.
    #[inline]
    pub const fn sac_scale(&self) -> Result<SacScale, UnknownValueError> {
        SacScale::try_from_bits(self.fourth_bitfield.sac_scale())
    }

    /// The number of lanes.
    ///
    /// Valhalla writes at least 1 for every edge.
    #[inline]
    pub const fn lane_count(&self) -> u8 {
        self.third_bitfield.lane_count()
    }

    /// The relative road density along the edge (0 through 15).
    #[inline]
    pub const fn density(&self) -> u8 {
        self.third_bitfield.density()
    }

    /// Is this edge a ferry?
    ///
    /// This is a convenience for checking [`road_use`](Self::road_use).
    #[inline]
    pub fn is_ferry(&self) -> bool {
        matches!(self.road_use(), Ok(RoadUse::Ferry | RoadUse::RailFerry))
    }

    // Structures

    /// Is this edge part of a tunnel?
    #[inline]
    pub const fn is_tunnel(&self) -> bool {
        self.fourth_bitfield.is_tunnel() != 0
    }

    /// Is this edge part of a bridge?
    #[inline]
    pub const fn is_bridge(&self) -> bool {
        self.fourth_bitfield.is_bridge() != 0
    }

    /// Is this edge indoors?
    #[inline]
    pub const fn is_indoor(&self) -> bool {
        self.fourth_bitfield.is_indoor() != 0
    }

    /// Is this edge lit?
    #[inline]
    pub const fn is_lit(&self) -> bool {
        self.fourth_bitfield.is_lit() != 0
    }

    /// Is this edge part of a roundabout?
    #[inline]
    pub const fn roundabout(&self) -> bool {
        self.third_bitfield.roundabout() != 0
    }

    /// Does the edge cross into a new country?
    #[inline]
    pub const fn country_crossing(&self) -> bool {
        self.first_bitfield.country_crossing() != 0
    }

    /// Does this edge lead to a dead end (no other drivable roads)?
    #[inline]
    pub const fn is_dead_end(&self) -> bool {
        self.fourth_bitfield.is_dead_end() != 0
    }

    /// Does this lead to (or come out from) a bike share station?
    ///
    /// These edges connect a bike share station node to the road network;
    /// bike share routing switches modes across them.
    #[inline]
    pub const fn has_bss_connection(&self) -> bool {
        self.fourth_bitfield.has_bss_connection() != 0
    }

    // Controls at the end of the edge

    /// Is there a traffic signal at the end of this edge?
    #[inline]
    pub const fn has_traffic_signal(&self) -> bool {
        self.fourth_bitfield.has_traffic_signal() != 0
    }

    /// Is there a stop sign at the end of this edge?
    #[inline]
    pub const fn has_stop_sign(&self) -> bool {
        self.fourth_bitfield.has_stop_sign() != 0
    }

    /// Is there a yield (give way) sign at the end of this edge?
    #[inline]
    pub const fn has_yield_sign(&self) -> bool {
        self.fourth_bitfield.has_yield_sign() != 0
    }

    // Access

    /// Gets the set of access modes allowed to traverse this edge forward.
    ///
    /// Note that "forward" is relative to the two nodes that the edge connects.
    /// All edges in Valhalla are directed and come in pairs.
    /// This is _not_ related to the OSM way direction.
    #[inline]
    pub fn forward_access(&self) -> EnumSet<Access> {
        access_set(self.fourth_bitfield.forward_access().get())
    }

    /// Gets the set of access modes allowed to traverse this edge in reverse.
    ///
    /// Note that "reverse" is relative to the two nodes that the edge connects.
    /// All edges in Valhalla are directed and come in pairs.
    /// This is _not_ related to the OSM way direction.
    #[inline]
    pub fn reverse_access(&self) -> EnumSet<Access> {
        access_set(self.fourth_bitfield.reverse_access().get())
    }

    /// Is this edge HOV-only (HOV access forward, but no general auto access)?
    #[inline]
    pub fn is_hov_only(&self) -> bool {
        let access = self.forward_access();
        access.contains(Access::HOV) && !access.contains(Access::Auto)
    }

    /// The occupancy requirement, if this edge is HOV-only.
    ///
    /// Returns `None` unless [`is_hov_only`](Self::is_hov_only) is `true`.
    #[inline]
    pub fn hov_type(&self) -> Option<HovType> {
        self.is_hov_only().then(|| self.fourth_bitfield.hov_type())
    }

    /// Is the edge destination-only (ex: private roads)?
    #[inline]
    pub const fn dest_only(&self) -> bool {
        self.second_bitfield.dest_only() != 0
    }

    /// Is the edge destination-only for heavy goods vehicles specifically?
    #[inline]
    pub const fn dest_only_hgv(&self) -> bool {
        self.fourth_bitfield.is_dest_only_hgv() != 0
    }

    /// Does the edge lead to a "no-through" region?
    #[inline]
    pub const fn no_thru(&self) -> bool {
        self.second_bitfield.no_thru() != 0
    }

    /// Is this edge part of a toll road?
    #[inline]
    pub const fn toll(&self) -> bool {
        self.third_bitfield.toll() != 0
    }

    /// Is this edge part of a truck route?
    #[inline]
    pub const fn truck_route(&self) -> bool {
        self.third_bitfield.truck_route() != 0
    }

    // Restrictions

    /// The local edge indexes at the end node that may not be entered from this edge
    /// (simple turn restrictions, such as OSM `no_left_turn`).
    ///
    /// Restrictions that span more than two edges or carry conditions
    /// are complex restrictions instead;
    /// see [`start_restriction`](Self::start_restriction).
    #[inline]
    pub const fn restrictions(&self) -> LocalEdgeMask {
        LocalEdgeMask(self.first_bitfield.restrictions())
    }

    /// The access modes for which this edge has access restriction records
    /// (such as height, weight, or time-based limits).
    ///
    /// The records themselves are in the tile's access restriction list.
    #[inline]
    pub fn access_restrictions(&self) -> EnumSet<Access> {
        access_set(self.second_bitfield.access_restrictions().get())
    }

    /// The access modes for which a complex restriction starts on this edge.
    #[inline]
    pub fn start_restriction(&self) -> EnumSet<Access> {
        access_set(self.second_bitfield.start_restriction().get())
    }

    /// The access modes for which a complex restriction ends on this edge.
    #[inline]
    pub fn end_restriction(&self) -> EnumSet<Access> {
        access_set(self.second_bitfield.end_restriction().get())
    }

    /// Is this edge part of a complex restriction (anywhere along it, not just its ends)?
    #[inline]
    pub const fn part_of_complex_restriction(&self) -> bool {
        self.second_bitfield.complex_restriction() != 0
    }

    // Speeds

    /// The default speed of the edge, in kph.
    ///
    /// This is the tagged or classification-based speed
    /// (see [`speed_type`](Self::speed_type)),
    /// and the final fallback when no flow, predicted, or live speed applies.
    /// Valhalla clamps it to 140 kph when building tiles.
    #[inline]
    pub const fn speed(&self) -> u8 {
        self.third_bitfield.speed()
    }

    /// Where [`speed`](Self::speed) came from.
    #[inline]
    pub const fn speed_type(&self) -> SpeedType {
        self.seventh_bitfield.speed_type()
    }

    /// The historical speed of the edge when there is no traffic (night time), in kph.
    ///
    /// Zero means there is no free flow speed;
    /// Valhalla only uses this speed when it is at least 1 kph.
    #[inline]
    pub const fn free_flow_speed(&self) -> u8 {
        self.third_bitfield.free_flow_speed()
    }

    /// Set the estimated speed of the edge when there is no traffic, in kph.
    #[inline]
    pub const fn set_free_flow_speed(&mut self, speed: u8) {
        self.third_bitfield.set_free_flow_speed(speed);
    }

    /// The historical speed of the edge during the day (7:00 to 19:00), in kph.
    ///
    /// Zero means there is no constrained flow speed;
    /// Valhalla only uses this speed when it is at least 1 kph.
    #[inline]
    pub const fn constrained_flow_speed(&self) -> u8 {
        self.third_bitfield.constrained_flow_speed()
    }

    /// Set the estimated speed of the edge when there is traffic, in kph.
    #[inline]
    pub const fn set_constrained_speed(&mut self, speed: u8) {
        self.third_bitfield.set_constrained_flow_speed(speed);
    }

    /// Does the edge have any flow speed data
    /// (free flow, constrained flow, or predicted)?
    #[inline]
    pub const fn has_flow_speed(&self) -> bool {
        self.free_flow_speed() > 0
            || self.constrained_flow_speed() > 0
            || self.has_predicted_speed()
    }

    /// The speed of the edge for trucks, in kph.
    ///
    /// Zero means there is no truck-specific speed.
    /// Otherwise, truck costing caps the edge speed at this value.
    #[inline]
    pub const fn truck_speed(&self) -> u8 {
        self.third_bitfield.truck_speed()
    }

    /// Set the estimated speed of the edge for trucks, in kph.
    #[inline]
    pub const fn set_truck_speed(&mut self, speed: u8) {
        self.third_bitfield.set_truck_speed(speed);
    }

    /// Does this edge have predicted speeds?
    #[inline]
    pub const fn has_predicted_speed(&self) -> bool {
        self.third_bitfield.has_predicted_speed() != 0
    }

    /// Sets whether the edge has predicted speed information.
    ///
    /// This should ALWAYS be called in tandem with [`crate::graph_tile::GraphTileBuilder::with_predicted_speeds`]
    /// or [`crate::graph_tile::GraphTileBuilder::with_predicted_encoded_speeds`],
    #[inline]
    pub(crate) fn set_has_predicted_speed(&mut self, value: bool) {
        self.third_bitfield.set_has_predicted_speed(value.into());
    }

    // Geometry and elevation

    /// The length of the edge (in meters)
    #[inline]
    pub const fn length(&self) -> u32 {
        self.fifth_bitfield.length().get()
    }

    /// The weighted grade factor (0 through 15), where 6 is flat.
    ///
    /// Valhalla computes this as `grade * 0.6 + 6.5` (truncated),
    /// where `grade` is the length-weighted average grade in percent,
    /// so each step is about 1.7 %.
    /// Costing models index speed and penalty tables with it.
    #[inline]
    pub const fn weighted_grade(&self) -> u8 {
        self.fifth_bitfield.weighted_grade()
    }

    /// The maximum upward slope along the edge, in percent (0 through 76).
    ///
    /// Slopes below 16 % are exact (rounded up to whole percent);
    /// steeper slopes are rounded up to a multiple of 4 %.
    #[inline]
    pub const fn max_up_slope(&self) -> u8 {
        decode_slope_magnitude(self.fourth_bitfield.max_up_slope())
    }

    /// The maximum downward slope along the edge, in percent (-76 through 0).
    ///
    /// As in Valhalla, the value is negative (or zero).
    /// The precision is the same as for [`max_up_slope`](Self::max_up_slope).
    #[inline]
    pub const fn max_down_slope(&self) -> i8 {
        // At most 76, so the cast is lossless.
        -(decode_slope_magnitude(self.fourth_bitfield.max_down_slope()).cast_signed())
    }

    /// The curvature factor (0 through 15), where higher values are curvier.
    #[inline]
    pub const fn curvature(&self) -> u8 {
        self.fifth_bitfield.curvature()
    }

    // Cycling and walking

    /// The kind of cycle lane along the edge.
    #[inline]
    pub const fn cycle_lane(&self) -> CycleLane {
        self.fourth_bitfield.cycle_lane()
    }

    /// Is this edge part of a bicycle network?
    #[inline]
    pub const fn is_bike_network(&self) -> bool {
        self.fourth_bitfield.is_bike_network() != 0
    }

    /// Is there a cycle path alongside this edge that cyclists should use instead
    /// (OSM `bicycle=use_sidepath`)?
    #[inline]
    pub const fn use_sidepath(&self) -> bool {
        self.fourth_bitfield.use_sidepath() != 0
    }

    /// Do cyclists need to dismount on this edge?
    #[inline]
    pub const fn dismount(&self) -> bool {
        self.fourth_bitfield.bicycle_dismount() != 0
    }

    /// Is there a sidewalk to the left of the edge?
    #[inline]
    pub const fn has_sidewalk_left(&self) -> bool {
        self.fourth_bitfield.has_sidewalk_left() != 0
    }

    /// Is there a sidewalk to the right of the edge?
    #[inline]
    pub const fn has_sidewalk_right(&self) -> bool {
        self.fourth_bitfield.has_sidewalk_right() != 0
    }

    /// Does the edge have a shoulder?
    #[inline]
    pub const fn has_shoulder(&self) -> bool {
        self.fourth_bitfield.has_shoulder() != 0
    }

    // Transitions onto this edge
    //
    // These are stored on the edge being entered.
    // The local edge index identifies the edge being left:
    // pass the `opposing_local_index` of the edge you arrive on,
    // which is the index of its opposing edge at this edge's start node.
    // This is how Valhalla's costing models read them.

    /// The type of turn onto this edge,
    /// coming from the edge whose opposing edge has the given local index at the start node.
    ///
    /// Returns `None` for local edge indexes above 7, which carry no turn data.
    #[inline]
    pub const fn turn_type(&self, local_edge_index: u8) -> Option<TurnType> {
        match packed_per_index(self.fifth_bitfield.turn_type().get(), 3, local_edge_index) {
            Some(bits) => Some(TurnType::from_bits(bits)),
            None => None,
        }
    }

    /// The local edge indexes (of the edge being left, as for [`turn_type`](Self::turn_type))
    /// for which there is another edge to the left of the turn onto this edge.
    #[inline]
    pub const fn edge_to_left(&self) -> LocalEdgeMask {
        LocalEdgeMask(self.fifth_bitfield.edge_to_left())
    }

    /// The local edge indexes (of the edge being left, as for [`turn_type`](Self::turn_type))
    /// for which there is another edge to the right of the turn onto this edge.
    ///
    /// Meaningless for transit lines, which store a [`line_id`](Self::line_id) here instead.
    #[inline]
    pub const fn edge_to_right(&self) -> LocalEdgeMask {
        LocalEdgeMask(self.stop_impact_or_line.stop_impact().edge_to_right())
    }

    /// The stop impact (0 through 7) of the transition onto this edge,
    /// indexed as for [`turn_type`](Self::turn_type).
    ///
    /// Higher values mean a stop is more likely
    /// (for example, when crossing a busier road).
    /// Returns `None` for local edge indexes above 7, which carry no stop impact.
    /// Meaningless for transit lines, which store a [`line_id`](Self::line_id) here instead.
    #[inline]
    pub const fn stop_impact(&self, local_edge_index: u8) -> Option<u8> {
        packed_per_index(
            self.stop_impact_or_line
                .stop_impact()
                .impact_between_edges()
                .get(),
            3,
            local_edge_index,
        )
    }

    /// The transit line ID, for transit lines (see [`is_transit_line`](Self::is_transit_line)).
    ///
    /// Returns `None` for other edges,
    /// which store [`stop_impact`](Self::stop_impact) data in the same bits.
    #[inline]
    pub fn line_id(&self) -> Option<u32> {
        self.is_transit_line()
            .then(|| self.stop_impact_or_line.line_id())
    }
}

// The bitfield struct macros break serde field attributes, so we roll our own for now.
#[cfg(feature = "serde")]
impl Serialize for DirectedEdge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        fn access_string(set: EnumSet<Access>) -> String {
            set.iter().map(|v| v.as_char()).collect()
        }

        let num_fields = 60;

        let mut state = serializer.serialize_struct("DirectedEdge", num_fields)?;

        state.serialize_field("end_node", &self.end_node_id().value())?;
        state.serialize_field("opposing_edge_index", &self.opposing_edge_index())?;
        state.serialize_field("leaves_tile", &self.leaves_tile())?;
        state.serialize_field("local_edge_index", &self.local_edge_index())?;
        state.serialize_field("opposing_local_index", &self.opposing_local_index())?;

        state.serialize_field("forward", &self.edge_info_is_forward())?;
        state.serialize_field("is_named", &self.is_named())?;
        state.serialize_field("name_consistency", &self.name_consistency())?;
        state.serialize_field("has_sign", &self.has_sign())?;
        state.serialize_field("has_lane_connectivity", &self.has_lane_connectivity())?;
        state.serialize_field("has_turn_lanes", &self.has_turn_lanes())?;

        state.serialize_field("is_shortcut", &self.is_shortcut())?;
        state.serialize_field("shortcut_idx", &self.shortcut_index())?;
        state.serialize_field("superseded_idx", &self.superseded_index())?;

        state.serialize_field("use", &self.road_use().ok())?;
        state.serialize_field("classification", &self.classification())?;
        state.serialize_field("is_link", &self.is_link())?;
        state.serialize_field("is_internal_intersection", &self.is_intersection_internal())?;
        state.serialize_field("surface", &self.surface())?;
        state.serialize_field("sac_scale", &self.sac_scale().ok())?;
        state.serialize_field("lane_count", &self.lane_count())?;
        state.serialize_field("density", &self.density())?;

        state.serialize_field("is_tunnel", &self.is_tunnel())?;
        state.serialize_field("is_bridge", &self.is_bridge())?;
        state.serialize_field("is_indoor", &self.is_indoor())?;
        state.serialize_field("is_lit", &self.is_lit())?;
        state.serialize_field("roundabout", &self.roundabout())?;
        state.serialize_field("country_crossing", &self.country_crossing())?;
        state.serialize_field("is_dead_end", &self.is_dead_end())?;
        state.serialize_field("has_bss_connection", &self.has_bss_connection())?;

        state.serialize_field("has_traffic_signal", &self.has_traffic_signal())?;
        state.serialize_field("has_stop_sign", &self.has_stop_sign())?;
        state.serialize_field("has_yield_sign", &self.has_yield_sign())?;

        state.serialize_field("forward_access", &access_string(self.forward_access()))?;
        state.serialize_field("reverse_access", &access_string(self.reverse_access()))?;
        state.serialize_field("hov_type", &self.hov_type())?;
        state.serialize_field("dest_only", &self.dest_only())?;
        state.serialize_field("dest_only_hgv", &self.dest_only_hgv())?;
        state.serialize_field("no_thru", &self.no_thru())?;
        state.serialize_field("toll", &self.toll())?;
        state.serialize_field("truck_route", &self.truck_route())?;

        state.serialize_field("restrictions", &self.restrictions())?;
        state.serialize_field(
            "access_restrictions",
            &access_string(self.access_restrictions()),
        )?;
        state.serialize_field(
            "start_restriction",
            &access_string(self.start_restriction()),
        )?;
        state.serialize_field("end_restriction", &access_string(self.end_restriction()))?;
        state.serialize_field("complex_restriction", &self.part_of_complex_restriction())?;

        state.serialize_field("speed", &self.speed())?;
        state.serialize_field("speed_type", &self.speed_type())?;
        state.serialize_field("free_flow_speed", &self.free_flow_speed())?;
        state.serialize_field("constrained_flow_speed", &self.constrained_flow_speed())?;
        state.serialize_field("truck_speed", &self.truck_speed())?;
        state.serialize_field("has_predicted_speed", &self.has_predicted_speed())?;

        state.serialize_field("length", &self.length())?;
        state.serialize_field("weighted_grade", &self.weighted_grade())?;
        state.serialize_field("max_up_slope", &self.max_up_slope())?;
        state.serialize_field("max_down_slope", &self.max_down_slope())?;
        state.serialize_field("curvature", &self.curvature())?;

        state.serialize_field("cycle_lane", &self.cycle_lane())?;
        state.serialize_field("is_bike_network", &self.is_bike_network())?;
        state.serialize_field("use_sidepath", &self.use_sidepath())?;
        state.serialize_field("dismount", &self.dismount())?;
        state.serialize_field("has_sidewalk_left", &self.has_sidewalk_left())?;
        state.serialize_field("has_sidewalk_right", &self.has_sidewalk_right())?;
        state.serialize_field("has_shoulder", &self.has_shoulder())?;

        state.end()
    }
}

/// Extended directed edge attributes.
///
/// This is extra space to add directed edge attributes without breaking backward compatibility.
/// Valhalla 3.9.1 defines no fields here (all 64 bits are spare),
/// so there are no accessors.
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, Debug, Clone)]
pub struct DirectedEdgeExt(U64<LE>);

#[cfg(test)]
pub(super) mod test {
    use super::{
        CycleLane, DirectedEdge, HovType, LocalEdgeMask, SacScale, SpeedType, TurnType,
        UnknownValueError, decode_slope_magnitude,
    };
    use crate::graph_tile::handles::OwnedGraphTileHandle;
    use crate::graph_tile::{GraphTile, TEST_GRAPH_TILE_L0, TEST_GRAPH_TILE_WITH_FLOW};
    use crate::{Access, GraphId, RoadUse};
    use enumset::EnumSet;
    use proptest::prelude::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use zerocopy::{FromBytes, IntoBytes};

    /// One record (edge, node, or transition) dumped by `tools/oracle/cpp/records.cc`.
    pub(in crate::graph_tile) struct MainlineRecord {
        pub(in crate::graph_tile) tile: GraphId,
        pub(in crate::graph_tile) index: usize,
        columns: HashMap<String, String>,
    }

    impl MainlineRecord {
        pub(in crate::graph_tile) fn str(&self, column: &str) -> &str {
            self.columns
                .get(column)
                .unwrap_or_else(|| panic!("no column {column}"))
        }

        pub(in crate::graph_tile) fn int(&self, column: &str) -> i64 {
            self.str(column)
                .parse()
                .unwrap_or_else(|e| panic!("column {column}: {e}"))
        }

        pub(in crate::graph_tile) fn flag(&self, column: &str) -> bool {
            match self.int(column) {
                0 => false,
                1 => true,
                other => panic!("column {column}: {other} is not a flag"),
            }
        }

        /// Reads an 8-digit per-local-index column (index 0 first).
        pub(in crate::graph_tile) fn digits(&self, column: &str) -> Vec<u8> {
            let s = self.str(column);
            assert_eq!(s.len(), 8, "column {column}");
            s.bytes().map(|b| b - b'0').collect()
        }
    }

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
    }

    /// Loads the mainline 3.9.1 reference records of one kind (`edge`, `node`, or `transition`).
    pub(in crate::graph_tile) fn mainline_records(kind: &str) -> Vec<MainlineRecord> {
        let text = std::fs::read_to_string(fixtures().join("mainline-3.9.1-records.txt"))
            .expect("read mainline records");
        let mut header: Option<Vec<String>> = None;
        let mut records = Vec::new();
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            if line.starts_with('#') {
                fields.next();
                if fields.next() == Some(kind) {
                    header = Some(fields.map(str::to_string).collect());
                }
                continue;
            }
            if fields.next() != Some(kind) {
                continue;
            }
            let names = header.as_ref().expect("header before rows");
            let values: Vec<&str> = fields.collect();
            assert_eq!(names.len(), values.len(), "{line}");
            let columns: HashMap<String, String> = names
                .iter()
                .cloned()
                .zip(values.iter().map(|v| (*v).to_string()))
                .collect();
            let level: u8 = columns["level"].parse().expect("level");
            let tile_id: u64 = columns["tile"].parse().expect("tile");
            let index: usize = columns["idx"].parse().expect("idx");
            let tile = GraphId::try_from_components(level, tile_id, 0).expect("tile id");
            records.push(MainlineRecord {
                tile,
                index,
                columns,
            });
        }
        assert!(!records.is_empty(), "no {kind} records");
        records
    }

    /// Loads every fixture tile that the records refer to.
    pub(in crate::graph_tile) fn load_tiles(
        records: &[MainlineRecord],
    ) -> HashMap<GraphId, OwnedGraphTileHandle> {
        let mut tiles = HashMap::new();
        for record in records {
            tiles.entry(record.tile).or_insert_with(|| {
                let path = fixtures()
                    .join("andorra-tiles")
                    .join(record.tile.file_path("gph").expect("tile path"));
                OwnedGraphTileHandle::try_from(std::fs::read(path).expect("read tile"))
                    .expect("parse tile")
            });
        }
        tiles
    }

    fn per_index<T>(f: impl Fn(u8) -> Option<T>) -> Vec<T> {
        (0..8).map(|i| f(i).expect("index in range")).collect()
    }

    fn mask_digits(mask: LocalEdgeMask) -> Vec<u8> {
        (0..8).map(|i| u8::from(mask.contains(i))).collect()
    }

    /// Checks every accessor against mainline 3.9.1 for a sample of edges from every
    /// fixture tile (all three levels), including every edge that shows a value
    /// not seen earlier in its tile.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "One assertion per mainline accessor."
    )]
    fn test_accessors_match_mainline() {
        let records = mainline_records("edge");
        let tiles = load_tiles(&records);
        let levels: std::collections::BTreeSet<u8> =
            records.iter().map(|r| r.tile.level()).collect();
        assert_eq!(levels.len(), 3, "records cover all three levels");

        for r in &records {
            let tile = &tiles[&r.tile];
            let e = &tile.directed_edges()[r.index];
            let ctx = format!("edge {}/{}/{}", r.tile.level(), r.tile.tile_id(), r.index);
            let int = |c: &str| r.int(c);
            let access = |c: &str| {
                EnumSet::<Access>::from_repr(u16::try_from(int(c)).expect("12-bit access"))
            };

            assert_eq!(
                i64::try_from(e.end_node_id().value()).unwrap(),
                int("end_node"),
                "{ctx}"
            );
            assert_eq!(
                i64::from(e.restrictions().bits()),
                int("restrictions"),
                "{ctx}"
            );
            assert_eq!(
                i64::from(e.opposing_edge_index()),
                int("opp_index"),
                "{ctx}"
            );
            assert_eq!(e.edge_info_is_forward(), r.flag("forward"), "{ctx}");
            assert_eq!(e.leaves_tile(), r.flag("leaves_tile"), "{ctx}");
            assert_eq!(e.country_crossing(), r.flag("country_crossing"), "{ctx}");
            assert_eq!(
                i64::from(e.edge_info_offset()),
                int("edge_info_offset"),
                "{ctx}"
            );
            assert_eq!(
                e.access_restrictions(),
                access("access_restriction"),
                "{ctx}"
            );
            assert_eq!(e.start_restriction(), access("start_restriction"), "{ctx}");
            assert_eq!(e.end_restriction(), access("end_restriction"), "{ctx}");
            assert_eq!(
                e.part_of_complex_restriction(),
                r.flag("complex_restriction"),
                "{ctx}"
            );
            assert_eq!(e.dest_only(), r.flag("dest_only"), "{ctx}");
            assert_eq!(e.no_thru(), r.flag("not_thru"), "{ctx}");
            assert_eq!(i64::from(e.speed()), int("speed"), "{ctx}");
            assert_eq!(
                i64::from(e.free_flow_speed()),
                int("free_flow_speed"),
                "{ctx}"
            );
            assert_eq!(
                i64::from(e.constrained_flow_speed()),
                int("constrained_flow_speed"),
                "{ctx}"
            );
            assert_eq!(i64::from(e.truck_speed()), int("truck_speed"), "{ctx}");
            assert_eq!(
                i64::from(e.name_consistency().bits()),
                int("name_consistency"),
                "{ctx}"
            );
            assert_eq!(
                i64::from(e.road_use().expect("known use") as u8),
                int("use"),
                "{ctx}"
            );
            assert_eq!(i64::from(e.lane_count()), int("lane_count"), "{ctx}");
            assert_eq!(i64::from(e.density()), int("density"), "{ctx}");
            assert_eq!(
                i64::from(e.classification().discriminant()),
                int("classification"),
                "{ctx}"
            );
            assert_eq!(i64::from(e.surface() as u8), int("surface"), "{ctx}");
            assert_eq!(e.toll(), r.flag("toll"), "{ctx}");
            assert_eq!(e.roundabout(), r.flag("roundabout"), "{ctx}");
            assert_eq!(e.truck_route(), r.flag("truck_route"), "{ctx}");
            assert_eq!(
                e.has_predicted_speed(),
                r.flag("has_predicted_speed"),
                "{ctx}"
            );
            assert_eq!(e.forward_access(), access("forward_access"), "{ctx}");
            assert_eq!(e.reverse_access(), access("reverse_access"), "{ctx}");
            assert_eq!(i64::from(e.max_up_slope()), int("max_up_slope"), "{ctx}");
            assert_eq!(
                i64::from(e.max_down_slope()),
                int("max_down_slope"),
                "{ctx}"
            );
            assert_eq!(
                i64::from(e.sac_scale().expect("known sac scale") as u8),
                int("sac_scale"),
                "{ctx}"
            );
            assert_eq!(i64::from(e.cycle_lane() as u8), int("cycle_lane"), "{ctx}");
            assert_eq!(e.is_bike_network(), r.flag("bike_network"), "{ctx}");
            assert_eq!(e.use_sidepath(), r.flag("use_sidepath"), "{ctx}");
            assert_eq!(e.dismount(), r.flag("dismount"), "{ctx}");
            assert_eq!(e.has_sidewalk_left(), r.flag("sidewalk_left"), "{ctx}");
            assert_eq!(e.has_sidewalk_right(), r.flag("sidewalk_right"), "{ctx}");
            assert_eq!(e.has_shoulder(), r.flag("shoulder"), "{ctx}");
            assert_eq!(
                e.has_lane_connectivity(),
                r.flag("lane_connectivity"),
                "{ctx}"
            );
            assert_eq!(e.has_turn_lanes(), r.flag("turn_lanes"), "{ctx}");
            assert_eq!(e.has_sign(), r.flag("sign"), "{ctx}");
            assert_eq!(e.is_intersection_internal(), r.flag("internal"), "{ctx}");
            assert_eq!(e.is_tunnel(), r.flag("tunnel"), "{ctx}");
            assert_eq!(e.is_bridge(), r.flag("bridge"), "{ctx}");
            assert_eq!(e.has_traffic_signal(), r.flag("traffic_signal"), "{ctx}");
            assert_eq!(e.is_dead_end(), r.flag("dead_end"), "{ctx}");
            assert_eq!(e.has_bss_connection(), r.flag("bss_connection"), "{ctx}");
            assert_eq!(e.has_stop_sign(), r.flag("stop_sign"), "{ctx}");
            assert_eq!(e.has_yield_sign(), r.flag("yield_sign"), "{ctx}");
            // Mainline reports the raw bit; we only expose it on HOV-only edges.
            assert_eq!(e.is_hov_only(), r.flag("is_hov_only"), "{ctx}");
            if e.is_hov_only() {
                assert_eq!(
                    i64::from(e.hov_type().expect("hov only") as u8),
                    int("hov_type"),
                    "{ctx}"
                );
            } else {
                assert_eq!(e.hov_type(), None, "{ctx}");
            }
            assert_eq!(e.is_indoor(), r.flag("indoor"), "{ctx}");
            assert_eq!(e.is_lit(), r.flag("lit"), "{ctx}");
            assert_eq!(e.dest_only_hgv(), r.flag("dest_only_hgv"), "{ctx}");
            assert_eq!(
                per_index(|i| e.turn_type(i).map(|t| t as u8)),
                r.digits("turn_type"),
                "{ctx}"
            );
            assert_eq!(
                mask_digits(e.edge_to_left()),
                r.digits("edge_to_left"),
                "{ctx}"
            );
            assert_eq!(i64::from(e.length()), int("length"), "{ctx}");
            assert_eq!(
                i64::from(e.weighted_grade()),
                int("weighted_grade"),
                "{ctx}"
            );
            assert_eq!(i64::from(e.curvature()), int("curvature"), "{ctx}");
            assert_eq!(e.is_transit_line(), r.flag("is_transit_line"), "{ctx}");
            if e.is_transit_line() {
                assert_eq!(e.line_id().map(i64::from), Some(int("line_id")), "{ctx}");
            } else {
                assert_eq!(e.line_id(), None, "{ctx}");
                assert_eq!(
                    per_index(|i| e.stop_impact(i)),
                    r.digits("stop_impact"),
                    "{ctx}"
                );
                assert_eq!(
                    mask_digits(e.edge_to_right()),
                    r.digits("edge_to_right"),
                    "{ctx}"
                );
            }
            assert_eq!(
                i64::from(e.local_edge_index()),
                int("local_edge_idx"),
                "{ctx}"
            );
            assert_eq!(
                i64::from(e.opposing_local_index()),
                int("opp_local_idx"),
                "{ctx}"
            );
            assert_eq!(e.is_shortcut(), r.flag("is_shortcut"), "{ctx}");
            let shortcut = int("shortcut");
            if e.is_shortcut() && shortcut != 0 {
                // Mainline stores a one-hot mask; we report the zero-based bit index.
                assert_eq!(
                    e.shortcut_index().map(|i| 1i64 << i),
                    Some(shortcut),
                    "{ctx}"
                );
            } else {
                assert_eq!(e.shortcut_index(), None, "{ctx}");
            }
            let superseded = int("superseded");
            assert_eq!(e.is_superseded(), superseded != 0, "{ctx}");
            assert_eq!(
                e.superseded_index().map_or(0, |i| 1i64 << i),
                superseded,
                "{ctx}"
            );
            assert_eq!(i64::from(e.speed_type() as u8), int("speed_type"), "{ctx}");
            assert_eq!(e.is_named(), r.flag("named"), "{ctx}");
            assert_eq!(e.is_link(), r.flag("link"), "{ctx}");
            assert_eq!(e.is_road(), r.flag("is_road"), "{ctx}");
            assert_eq!(e.can_form_shortcut(), r.flag("can_form_shortcut"), "{ctx}");
            assert_eq!(e.is_unpaved(), r.flag("unpaved"), "{ctx}");
            assert_eq!(e.has_flow_speed(), r.flag("has_flow_speed"), "{ctx}");
        }
    }

    /// Spot checks that pin a few interesting edges from the fixtures,
    /// with values cross-checked against mainline (see `test_accessors_match_mainline`).
    #[test]
    fn test_sample_values() {
        let records = mainline_records("edge");
        let tiles = load_tiles(&records);
        let edges = |level: u8, tile_id: u64| {
            let id = GraphId::try_from_components(level, tile_id, 0).unwrap();
            tiles[&id].directed_edges()
        };

        // A hiking trail in the level 2 tile covering most of Andorra.
        let hiking = edges(2, 763_926)
            .iter()
            .filter(|e| e.sac_scale() == Ok(SacScale::DemandingMountainHiking))
            .count();
        assert!(hiking > 0, "fixtures include T3 hiking trails");

        // The first edge of the level 0 tile (also pinned in the debug snapshot):
        // the first shortcut leaving its start node.
        let first = &edges(0, 3015)[0];
        assert!(first.is_shortcut());
        assert_eq!(first.shortcut_index(), Some(0));
        assert!(!first.can_form_shortcut());
        assert_eq!(first.speed(), 68);
        assert_eq!(first.truck_speed(), 68);
        assert_eq!(first.speed_type(), SpeedType::Classified);
        assert_eq!(first.max_up_slope(), 0);
        assert_eq!(first.max_down_slope(), -3);
        assert_eq!(first.weighted_grade(), 4);
        assert_eq!(first.curvature(), 14);
        assert_eq!(first.turn_type(0), Some(TurnType::Reverse));
        assert_eq!(first.turn_type(1), Some(TurnType::Straight));
        assert_eq!(first.turn_type(8), None);
        assert_eq!(first.cycle_lane(), CycleLane::None);
        assert!(first.is_bridge());
        assert!(first.is_bike_network());
        assert!(first.is_named());
        assert!(first.is_road());
        assert_eq!(first.name_consistency(), LocalEdgeMask::from_bits(1));
    }

    #[test]
    fn test_parse_directed_edges_count() {
        let tile = &*TEST_GRAPH_TILE_L0;

        assert_eq!(
            tile.directed_edges().len(),
            tile.header().directed_edge_count() as usize
        );
    }

    #[test]
    fn test_parse_nodes() {
        let tile = &*TEST_GRAPH_TILE_L0;

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!("first_directed_edge", tile.directed_edges()[0]);
            insta::assert_debug_snapshot!(
                "last_directed_edge",
                tile.directed_edges().last().unwrap()
            );
        }
    }

    #[test]
    fn test_slope_decoding() {
        // Exact below 16 %, then 4 % steps up to 76 %.
        assert_eq!(decode_slope_magnitude(0), 0);
        assert_eq!(decode_slope_magnitude(15), 15);
        assert_eq!(decode_slope_magnitude(0x10), 16);
        assert_eq!(decode_slope_magnitude(0x11), 20);
        assert_eq!(decode_slope_magnitude(0x1f), 76);
    }

    #[test]
    fn test_local_edge_mask() {
        let mask = LocalEdgeMask::from_bits(0b1000_0101);
        assert!(mask.contains(0));
        assert!(!mask.contains(1));
        assert!(mask.contains(2));
        assert!(mask.contains(7));
        assert!(!mask.contains(8));
        assert!(!mask.contains(u8::MAX));
        assert_eq!(mask.iter().collect::<Vec<_>>(), vec![0, 2, 7]);
        assert_eq!(format!("{mask:?}"), "{0, 2, 7}");
        assert!(LocalEdgeMask::default().is_empty());
    }

    /// Builds an edge from raw words, in tile byte order.
    fn edge_from_words(words: [u64; 5], stop_or_line: u32, seventh: u32) -> DirectedEdge {
        let mut bytes = Vec::with_capacity(48);
        for w in words {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        bytes.extend_from_slice(&stop_or_line.to_le_bytes());
        bytes.extend_from_slice(&seventh.to_le_bytes());
        DirectedEdge::read_from_bytes(&bytes).expect("48 bytes")
    }

    #[test]
    fn test_unknown_enum_values_are_errors() {
        // sac_scale is bits 34..37 of the fourth word; 7 is not a SAC scale.
        let edge = edge_from_words([0, 0, 0, 7 << 34, 0], 0, 0);
        assert_eq!(
            edge.sac_scale(),
            Err(UnknownValueError::new("sac_scale", 7))
        );
        assert_eq!(
            edge.sac_scale().unwrap_err().to_string(),
            "decode sac_scale: 7 is not a known value"
        );

        // use is bits 40..46 of the third word; 12 is not a use.
        let edge = edge_from_words([0, 0, 12 << 40, 0, 0], 0, 0);
        assert_eq!(edge.road_use(), Err(UnknownValueError::new("use", 12)));
        assert!(!edge.is_road());
        assert!(!edge.is_transit_line());
        assert!(edge.can_form_shortcut());
    }

    #[test]
    fn test_transit_line_id() {
        // A bus line (use 51) stores its line ID where other edges store stop impact.
        let edge = edge_from_words([0, 0, 51 << 40, 0, 0], 0xdead_beef, 0);
        assert!(edge.is_transit_line());
        assert_eq!(edge.road_use(), Ok(RoadUse::Bus));
        assert_eq!(edge.line_id(), Some(0xdead_beef));
    }

    #[test]
    fn test_hov_type() {
        // Forward access is the low 12 bits of the fourth word; HOV is bit 7, auto bit 0.
        // hov_type is bit 57.
        let hov3 = edge_from_words([0, 0, 0, (1 << 7) | (1 << 57), 0], 0, 0);
        assert!(hov3.is_hov_only());
        assert_eq!(hov3.hov_type(), Some(HovType::Hov3));
        let hov_and_auto = edge_from_words([0, 0, 0, (1 << 7) | 1 | (1 << 57), 0], 0, 0);
        assert!(!hov_and_auto.is_hov_only());
        assert_eq!(hov_and_auto.hov_type(), None);
    }

    /// Pins the bit position of every single-bit flag to the field order in
    /// `baldr/directededge.h` (3.9.1).
    /// The fixtures never set some of these flags (country crossings, shoulders, indoor edges,
    /// and others), so the mainline comparison alone can't catch a misplaced bit.
    #[test]
    fn test_flag_bit_positions() {
        type Flag = (usize, u32, &'static str, fn(&DirectedEdge) -> bool);
        // (word, bit, name, accessor); words 0 through 4 are the 64-bit words,
        // 5 is the stop impact (or line ID) word, and 6 is the last 32-bit word.
        let flags: &[Flag] = &[
            (0, 61, "forward", DirectedEdge::edge_info_is_forward),
            (0, 62, "leaves_tile", DirectedEdge::leaves_tile),
            (0, 63, "ctry_crossing", DirectedEdge::country_crossing),
            (
                1,
                61,
                "complex_restriction",
                DirectedEdge::part_of_complex_restriction,
            ),
            (1, 62, "dest_only", DirectedEdge::dest_only),
            (1, 63, "not_thru", DirectedEdge::no_thru),
            (2, 60, "toll", DirectedEdge::toll),
            (2, 61, "roundabout", DirectedEdge::roundabout),
            (2, 62, "truck_route", DirectedEdge::truck_route),
            (
                2,
                63,
                "has_predicted_speed",
                DirectedEdge::has_predicted_speed,
            ),
            (3, 39, "bike_network", DirectedEdge::is_bike_network),
            (3, 40, "use_sidepath", DirectedEdge::use_sidepath),
            (3, 41, "dismount", DirectedEdge::dismount),
            (3, 42, "sidewalk_left", DirectedEdge::has_sidewalk_left),
            (3, 43, "sidewalk_right", DirectedEdge::has_sidewalk_right),
            (3, 44, "shoulder", DirectedEdge::has_shoulder),
            (3, 45, "lane_conn", DirectedEdge::has_lane_connectivity),
            (3, 46, "turnlanes", DirectedEdge::has_turn_lanes),
            (3, 47, "sign", DirectedEdge::has_sign),
            (3, 48, "internal", DirectedEdge::is_intersection_internal),
            (3, 49, "tunnel", DirectedEdge::is_tunnel),
            (3, 50, "bridge", DirectedEdge::is_bridge),
            (3, 51, "traffic_signal", DirectedEdge::has_traffic_signal),
            // Bit 52 is spare1_ (formerly "seasonal").
            (3, 53, "deadend", DirectedEdge::is_dead_end),
            (3, 54, "bss_connection", DirectedEdge::has_bss_connection),
            (3, 55, "stop_sign", DirectedEdge::has_stop_sign),
            (3, 56, "yield_sign", DirectedEdge::has_yield_sign),
            // Bit 57 is hov_type_ (see test_hov_type).
            (3, 58, "indoor", DirectedEdge::is_indoor),
            (3, 59, "lit", DirectedEdge::is_lit),
            (3, 60, "dest_only_hgv", DirectedEdge::dest_only_hgv),
            (6, 28, "is_shortcut", DirectedEdge::is_shortcut),
            (6, 30, "named", DirectedEdge::is_named),
            (6, 31, "link", DirectedEdge::is_link),
        ];
        let edge_with_bit = |word: usize, bit: u32| {
            let mut words = [0u64; 5];
            let (mut stop, mut seventh) = (0u32, 0u32);
            match word {
                0..=4 => words[word] = 1 << bit,
                5 => stop = 1 << bit,
                _ => seventh = 1 << bit,
            }
            edge_from_words(words, stop, seventh)
        };
        for &(word, bit, name, _) in flags {
            let edge = edge_with_bit(word, bit);
            for &(_, _, other, accessor) in flags {
                assert_eq!(
                    accessor(&edge),
                    other == name,
                    "{other} with only {name} set"
                );
            }
        }
        // The retired seasonal bit sets nothing.
        let spare = edge_with_bit(3, 52);
        assert!(flags.iter().all(|&(_, _, _, accessor)| !accessor(&spare)));
        // Speed type is the other single-bit field in the last word.
        assert_eq!(edge_with_bit(6, 29).speed_type(), SpeedType::Classified);
        assert_eq!(edge_with_bit(6, 0).speed_type(), SpeedType::Tagged);
    }

    /// Flow speeds, which the plain fixtures don't have.
    /// Values are from mainline 3.9.1 (`records` run on `andorra-tiles-with-traffic`).
    #[test]
    fn test_flow_speeds() {
        let edges = TEST_GRAPH_TILE_WITH_FLOW.directed_edges();
        let speeds = |i: usize| {
            let e = &edges[i];
            (
                e.free_flow_speed(),
                e.constrained_flow_speed(),
                e.has_predicted_speed(),
                e.has_flow_speed(),
                e.speed(),
            )
        };
        assert_eq!(speeds(0), (50, 40, false, true, 68));
        assert_eq!(speeds(7), (12, 34, true, true, 75));
        assert_eq!(speeds(42), (100, 42, true, true, 60));
        assert_eq!(speeds(1), (0, 0, false, false, 75));
    }

    proptest! {
        /// No accessor panics on arbitrary bytes.
        #[test]
        fn accessors_never_panic(bytes in proptest::collection::vec(any::<u8>(), 48)) {
            let e = DirectedEdge::read_from_bytes(&bytes).unwrap();
            let _ = (
                e.end_node_id(), e.opposing_edge_index(), e.leaves_tile(), e.local_edge_index(),
                e.opposing_local_index(), e.edge_info_offset(), e.edge_info_is_forward(),
                e.is_named(), e.name_consistency(), e.has_sign(), e.has_lane_connectivity(),
                e.has_turn_lanes(), e.is_shortcut(), e.shortcut_index(), e.can_form_shortcut(),
                e.is_superseded(), e.superseded_index(), e.road_use(), e.is_road(),
                e.is_transit_line(), e.classification(), e.is_link(),
            );
            let _ = (
                e.is_intersection_internal(), e.surface(), e.is_unpaved(), e.sac_scale(),
                e.lane_count(), e.density(), e.is_ferry(), e.is_tunnel(), e.is_bridge(),
                e.is_indoor(), e.is_lit(), e.roundabout(), e.country_crossing(), e.is_dead_end(),
                e.has_bss_connection(), e.has_traffic_signal(), e.has_stop_sign(),
                e.has_yield_sign(), e.forward_access(), e.reverse_access(), e.is_hov_only(),
            );
            let _ = (
                e.hov_type(), e.dest_only(), e.dest_only_hgv(), e.no_thru(), e.toll(),
                e.truck_route(), e.restrictions(), e.access_restrictions(), e.start_restriction(),
                e.end_restriction(), e.part_of_complex_restriction(), e.speed(), e.speed_type(),
                e.free_flow_speed(), e.constrained_flow_speed(), e.has_flow_speed(),
                e.truck_speed(), e.has_predicted_speed(), e.length(), e.weighted_grade(),
            );
            let _ = (
                e.max_up_slope(), e.max_down_slope(), e.curvature(), e.cycle_lane(),
                e.is_bike_network(), e.use_sidepath(), e.dismount(), e.has_sidewalk_left(),
                e.has_sidewalk_right(), e.has_shoulder(), e.edge_to_left(), e.edge_to_right(),
                e.line_id(), format!("{e:?}"),
            );
            for i in 0..=u8::MAX {
                prop_assert_eq!(e.turn_type(i).is_some(), i < 8);
                prop_assert_eq!(e.stop_impact(i).is_some(), i < 8);
                if let Some(impact) = e.stop_impact(i) {
                    prop_assert!(impact <= 7);
                }
            }
            prop_assert!(e.max_up_slope() <= 76);
            prop_assert!((-76..=0).contains(&e.max_down_slope()));
            prop_assert_eq!(e.as_bytes(), &bytes[..]);
        }

        /// Decoding inverts mainline's `set_max_up_slope` encoding
        /// to within its documented precision.
        #[test]
        fn slope_round_trip(slope in 0.0f32..100.0) {
            // Port of DirectedEdge::set_max_up_slope from mainline directededge.cc.
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let raw: u8 = if slope < 16.0 {
                slope.ceil() as u8
            } else if slope < 76.0 {
                0x10 | ((slope - 16.0) * 0.25).ceil() as u8
            } else {
                0x1f
            };
            let decoded = f32::from(decode_slope_magnitude(raw));
            if slope < 76.0 {
                prop_assert!(decoded >= slope, "{decoded} < {slope}");
                let step = if slope < 16.0 { 1.0 } else { 4.0 };
                prop_assert!(decoded - slope < step, "{} is too far above {}", decoded, slope);
            } else {
                prop_assert!((decoded - 76.0).abs() < f32::EPSILON);
            }
        }
    }
}
