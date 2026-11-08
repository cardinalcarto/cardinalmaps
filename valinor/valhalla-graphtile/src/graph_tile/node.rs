//! Nodes and node transitions.
//!
//! The layouts match `baldr/nodeinfo.h` and `baldr/nodetransition.h` in Valhalla 3.9.1
//! bit for bit.
//! As with [`DirectedEdge`](super::DirectedEdge),
//! decoding never panics:
//! fields whose bit width admits values that Valhalla never writes
//! return a [`Result`] with an [`UnknownValueError`] instead.

use crate::graph_tile::directed_edge::UnknownValueError;
use crate::{Access, GraphId};
use bitfield_struct::bitfield;
use core::num::NonZeroU16;
use enumset::EnumSet;
use geo::{Coord, coord};
use zerocopy::{LE, U16, U32, U64};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, Unaligned};

#[cfg(feature = "serde")]
use serde::Serialize;

/// Elevation is stored in quarter meters above [`NODE_MIN_ELEVATION`].
const NODE_ELEVATION_PRECISION: f32 = 0.25;
/// The lowest elevation Valhalla stores (`kNodeMinElevation`).
///
/// Valhalla stores anything at or below this
/// (including the elevation service's "no data" value) as zero,
/// so we read a stored zero as "no elevation".
const NODE_MIN_ELEVATION: f32 = -500.0;
/// The largest local edge index with per-index data (`kMaxLocalEdgeIndex`).
pub(super) const MAX_LOCAL_EDGE_INDEX: u8 = 7;
/// Expands the stored 8-bit heading back to degrees (`kHeadingExpandFactor`).
const HEADING_EXPAND_FACTOR: f32 = 359f32 / 255f32;

/// The type of intersection at a node.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum IntersectionType {
    /// A regular, unclassified intersection.
    Regular = 0,
    /// A false intersection, where only two edges meet.
    /// These are typically where a way is split, or where attributes change.
    False = 1,
    /// A dead end: only one edge connects to the node.
    DeadEnd = 2,
    /// A fork: all edges are links, or none are and the node is a motorway junction.
    Fork = 3,
}

impl IntersectionType {
    const fn try_from_bits(value: u8) -> Result<Self, UnknownValueError> {
        Ok(match value {
            0 => Self::Regular,
            1 => Self::False,
            2 => Self::DeadEnd,
            3 => Self::Fork,
            _ => return Err(UnknownValueError::new("intersection", value)),
        })
    }
}

/// The type of a node (barriers, transit, and other special points).
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum NodeType {
    /// A regular intersection of two or more roads.
    StreetIntersection = 0,
    /// A gate or rising bollard.
    Gate = 1,
    /// A fixed bollard.
    Bollard = 2,
    /// A toll booth or other fare collection point.
    TollBooth = 3,
    /// A transit egress (Valhalla doesn't yet distinguish bus and rail stops).
    TransitEgress = 4,
    /// A transit station.
    TransitStation = 5,
    /// A multi-use transit platform (rail and bus).
    MultiUseTransitPlatform = 6,
    /// A bike share location.
    BikeShare = 7,
    /// A parking location.
    Parking = 8,
    /// A motorway junction (OSM `highway=motorway_junction`).
    MotorwayJunction = 9,
    /// A border control point.
    BorderControl = 10,
    /// A toll gantry.
    TollGantry = 11,
    /// A sump buster.
    SumpBuster = 12,
    /// A building entrance.
    BuildingEntrance = 13,
    /// An elevator.
    Elevator = 14,
}

impl NodeType {
    const fn try_from_bits(value: u8) -> Result<Self, UnknownValueError> {
        Ok(match value {
            0 => Self::StreetIntersection,
            1 => Self::Gate,
            2 => Self::Bollard,
            3 => Self::TollBooth,
            4 => Self::TransitEgress,
            5 => Self::TransitStation,
            6 => Self::MultiUseTransitPlatform,
            7 => Self::BikeShare,
            8 => Self::Parking,
            9 => Self::MotorwayJunction,
            10 => Self::BorderControl,
            11 => Self::TollGantry,
            12 => Self::SumpBuster,
            13 => Self::BuildingEntrance,
            14 => Self::Elevator,
            _ => return Err(UnknownValueError::new("node type", value)),
        })
    }
}

/// The directions in which an edge can be traversed.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum Traversability {
    /// Not traversable in either direction.
    None = 0,
    /// Traversable in the forward direction only.
    Forward = 1,
    /// Traversable in the backward direction only.
    Backward = 2,
    /// Traversable in both directions.
    Both = 3,
}

impl Traversability {
    const fn from_bits(value: u8) -> Self {
        // Two bits, so every value is a variant.
        match value & 0b11 {
            0 => Self::None,
            1 => Self::Forward,
            2 => Self::Backward,
            _ => Self::Both,
        }
    }
}

// Single-bit flags are stored as `u8` and exposed as `bool` by the accessors on `NodeInfo`.

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct FirstBitfield {
    #[bits(22, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    lat_offset: U32<LE>,
    #[bits(4)]
    lat_offset7: u8,
    #[bits(22, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    lon_offset: U32<LE>,
    #[bits(4)]
    lon_offset7: u8,
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    access: U16<LE>,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct SecondBitfield {
    #[bits(21, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    edge_index: U32<LE>,
    #[bits(7)]
    edge_count: u8,
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    admin_index: U16<LE>,
    #[bits(9, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    time_zone_index: U16<LE>,
    #[bits(4)]
    intersection_type: u8,
    #[bits(4)]
    node_type: u8,
    #[bits(4)]
    density: u8,
    #[bits(1)]
    is_traffic_signal: u8,
    #[bits(1)]
    mode_change_allowed: u8,
    #[bits(1)]
    is_named_intersection: u8,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct ThirdBitfield {
    #[bits(21, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    transition_index: U32<LE>,
    #[bits(3)]
    transition_count: u8,
    // Two bits per local edge index.
    #[bits(16, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    local_driveability: U16<LE>,
    #[bits(3)]
    local_edge_count: u8,
    #[bits(1)]
    drive_on_right: u8,
    #[bits(1)]
    tagged_access: u8,
    #[bits(1)]
    private_access: u8,
    #[bits(1)]
    is_cash_only_toll: u8,
    #[bits(15, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    elevation: U16<LE>,
    // For compatibility when new time zones are added
    #[bits(1, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    time_zone_ext: U16<LE>,
    #[bits(1)]
    _spare: u8,
}

/// Information for a node within the graph.
///
/// The graph uses a forward star structure,
/// where nodes point to the first outbound directed edge,
/// and each directed edge points to the other end node of the edge.
///
/// Several accessors take a *local edge index*;
/// see [`DirectedEdge::local_edge_index`](super::DirectedEdge::local_edge_index).
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, Debug, Clone)]
#[repr(C)]
pub struct NodeInfo {
    first_bit_field: FirstBitfield,
    second_bit_field: SecondBitfield,
    third_bit_field: ThirdBitfield,
    /// Heading information.
    ///
    /// # Documentation from Valhalla (lightly cleaned up)
    ///
    /// For non-transit levels, it's the headings of up to kMaxLocalEdgeIndex+1 local edges
    /// (rounded to nearest 2 degrees) for all other levels.
    ///
    /// Sadly, we need to keep this for now because it's used in map matching.
    /// Otherwise we could remove it.
    /// Also for transit levels (while building data only), it can be used for either the connecting way
    /// ID for matching the connection point of the station to the edge, or an encoded lon/lat pair for
    /// the exact connection point.
    /// If the highest bit is set, it's a lon lat otherwise it's a way ID.
    headings: U64<LE>,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
pub struct NodeTransition {
    #[bits(46, from = bit_twiddling_helpers::conv_u64le::from_inner, into = bit_twiddling_helpers::conv_u64le::into_inner)]
    end_node_id: U64<LE>,
    #[bits(1)]
    up: u8,
    #[bits(17)]
    _spare: U32<LE>,
}

impl NodeTransition {
    /// The ID of the corresponding end node on another hierarchy level.
    #[inline]
    pub const fn corresponding_end_node_id(&self) -> GraphId {
        // SAFETY: We know that the bit field cannot contain a value
        // larger than the max allowed value (it's limited to 46 bits).
        unsafe { GraphId::from_id_unchecked(self.end_node_id()) }
    }

    /// Is the transition up to a higher level (towards level 0)?
    /// If not, it is down to a lower level.
    #[inline]
    pub const fn is_up(&self) -> bool {
        self.up() != 0
    }
}

impl NodeInfo {
    // Position

    /// Gets the coordinate of the node.
    /// The data is stored as a relative offset internally,
    /// so a reference coordinate (namely the SW corner of the tile)
    /// is required to compute the absolute position.
    /// You can get this from the graph tile header.
    #[inline]
    #[expect(clippy::cast_possible_truncation)]
    pub fn coordinate(&self, sw_corner: Coord<f32>) -> Coord<f32> {
        // NOTE: We are trying something a bit unorthodox here and attempting to save space
        // on storage of vast numbers of coordinates.
        // We also know the sw_corner resolution doesn't require f64.
        // But we still do all the internal math in f64 for better precision.
        let lat_offset = f64::from(self.first_bit_field.lat_offset().get()) * 1e-6f64
            + f64::from(self.first_bit_field.lat_offset7()) * 1e-7f64;
        let lon_offset = f64::from(self.first_bit_field.lon_offset().get()) * 1e-6f64
            + f64::from(self.first_bit_field.lon_offset7()) * 1e-7f64;
        sw_corner + coord! {x: lon_offset as f32, y: lat_offset as f32}
    }

    /// Gets the coordinate of the node in double precision,
    /// exactly as Valhalla's `NodeInfo::latlng` computes it:
    /// the corner (single precision in the header, widened) plus the offsets in degrees.
    ///
    /// Use this where results must match Valhalla's to the last bit
    /// (for example, Loki compares projected points with node positions exactly).
    #[inline]
    pub fn coordinate_f64(&self, sw_corner: Coord<f32>) -> Coord<f64> {
        let lat_offset = f64::from(self.first_bit_field.lat_offset().get()) * 1e-6f64
            + f64::from(self.first_bit_field.lat_offset7()) * 1e-7f64;
        let lon_offset = f64::from(self.first_bit_field.lon_offset().get()) * 1e-6f64
            + f64::from(self.first_bit_field.lon_offset7()) * 1e-7f64;
        coord! {x: f64::from(sw_corner.x) + lon_offset, y: f64::from(sw_corner.y) + lat_offset}
    }

    /// The elevation at the node, in meters.
    ///
    /// Returns `None` when the tile has no elevation for this node.
    /// That covers tiles built without elevation data,
    /// and nodes where the elevation source had no data.
    ///
    /// Valhalla stores elevation in quarter meters above -500 m,
    /// as `(elevation + 500) / 0.25` truncated and clamped to 15 bits,
    /// and stores anything at or below -500 m as zero.
    /// Valhalla itself would decode a stored zero as -500 m;
    /// since no land is that low, we read it as missing data instead.
    /// Other values range from -499.75 m to 7691.75 m.
    #[inline]
    pub fn elevation(&self) -> Option<f32> {
        let raw = self.third_bit_field.elevation().get();
        (raw != 0).then(|| NODE_MIN_ELEVATION + NODE_ELEVATION_PRECISION * f32::from(raw))
    }

    // Edges and transitions

    /// The index within the node's tile of its first outbound directed edge.
    ///
    /// This can be combined with [`edge_count`](NodeInfo::edge_count)
    /// to enumerate all outbound edges.
    /// [`GraphTile::get_outbound_edges_from_node`](crate::graph_tile::GraphTile::get_outbound_edges_from_node)
    /// provides a more convenient API for this use case.
    #[inline]
    pub const fn edge_index(&self) -> u32 {
        self.second_bit_field.edge_index().get()
    }

    /// The number of outbound edges on the same level.
    ///
    /// This count _does_ include shortcuts and other non-road edges,
    /// unlike [`local_edge_count`](NodeInfo::local_edge_count).
    /// Transitions are recorded separately in [`self.transition_count`](NodeInfo::transition_count).
    ///
    /// This can be combined with [`edge_index`](NodeInfo::edge_index)
    /// to enumerate all outbound edges.
    /// [`GraphTile::get_outbound_edges_from_node`](crate::graph_tile::GraphTile::get_outbound_edges_from_node)
    /// provides a more convenient API for this use case.
    #[inline]
    pub const fn edge_count(&self) -> u8 {
        self.second_bit_field.edge_count()
    }

    /// The number of regular edges at this node across all hierarchy levels,
    /// up to 8.
    ///
    /// Unlike [`edge_count`](NodeInfo::edge_count),
    /// this excludes shortcuts, transit edges, transit connections, and transitions.
    /// It counts the same node on every level,
    /// so it is the same on each level's copy of the node.
    /// These edges are the ones numbered by
    /// [`DirectedEdge::local_edge_index`](super::DirectedEdge::local_edge_index),
    /// and the ones with [`heading`](Self::heading)
    /// and [`local_driveability`](Self::local_driveability) data.
    #[inline]
    pub const fn local_edge_count(&self) -> u8 {
        // Every node has at least one edge,
        // so to save bits (this field only has three), zero stands for one.
        self.third_bit_field.local_edge_count() + 1
    }

    /// The index of the first transition from this node.
    /// This index is into the `transitions` vector on
    /// the graph tile.
    ///
    /// On the transit level, Valhalla stores the transit stop index here instead.
    #[inline]
    pub const fn transition_index(&self) -> u32 {
        self.third_bit_field.transition_index().get()
    }

    /// The number of transitions from this node
    #[inline]
    pub const fn transition_count(&self) -> u8 {
        self.third_bit_field.transition_count()
    }

    /// The driveability (traversability by car) of the regular edge
    /// with the given local edge index.
    ///
    /// Returns `None` for local edge indexes above 7, which carry no data.
    #[inline]
    pub const fn local_driveability(&self, local_edge_index: u8) -> Option<Traversability> {
        if local_edge_index > MAX_LOCAL_EDGE_INDEX {
            return None;
        }
        let shift = 2 * local_edge_index;
        let raw = self.third_bit_field.local_driveability().get() >> shift;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Traversability::from_bits keeps only the low two bits."
        )]
        Some(Traversability::from_bits(raw as u8))
    }

    /// Gets the heading of the regular edge with the given local edge index,
    /// in degrees clockwise from north (0 to 359).
    ///
    /// Headings are stored in 8 bits, so they are accurate to about 1.4 degrees.
    /// Returns `None` for local edge indexes above 7, which carry no heading
    /// (where Valhalla returns 0).
    #[inline]
    pub fn heading(&self, local_edge_index: u8) -> Option<u16> {
        if local_edge_index > MAX_LOCAL_EDGE_INDEX {
            return None;
        }
        let shift = u32::from(local_edge_index) * 8;
        let raw = (self.headings.get() >> shift) & 0xff;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "At most 255 * 359 / 255, which fits."
        )]
        Some((f32::from(raw as u8) * HEADING_EXPAND_FACTOR).round() as u16)
    }

    // Classification

    /// The type of intersection.
    ///
    /// # Errors
    ///
    /// The field has room for values that Valhalla never writes;
    /// reading one returns an error, which only happens with corrupt data.
    #[inline]
    pub const fn intersection_type(&self) -> Result<IntersectionType, UnknownValueError> {
        IntersectionType::try_from_bits(self.second_bit_field.intersection_type())
    }

    /// The type of node.
    ///
    /// # Errors
    ///
    /// The field has room for a value that Valhalla never writes;
    /// reading it returns an error, which only happens with corrupt data.
    #[inline]
    pub const fn node_type(&self) -> Result<NodeType, UnknownValueError> {
        NodeType::try_from_bits(self.second_bit_field.node_type())
    }

    /// Is this a transit node (a multi-use transit platform)?
    #[inline]
    pub const fn is_transit(&self) -> bool {
        matches!(self.node_type(), Ok(NodeType::MultiUseTransitPlatform))
    }

    /// Is this node eligible for contraction when building shortcuts?
    ///
    /// True if the node has at least two edges
    /// and is not a fork, gate, toll booth, toll gantry, or sump buster.
    /// As in Valhalla, unknown intersection and node types don't block contraction.
    #[inline]
    pub const fn can_contract(&self) -> bool {
        self.edge_count() >= 2
            && !matches!(self.intersection_type(), Ok(IntersectionType::Fork))
            && !matches!(
                self.node_type(),
                Ok(NodeType::Gate
                    | NodeType::TollBooth
                    | NodeType::TollGantry
                    | NodeType::SumpBuster)
            )
    }

    /// The relative road density at the node (0 - 15).
    #[inline]
    pub const fn density(&self) -> u8 {
        self.second_bit_field.density()
    }

    /// Is there a traffic signal at this node?
    #[inline]
    pub const fn is_traffic_signal(&self) -> bool {
        self.second_bit_field.is_traffic_signal() != 0
    }

    /// Is this a named intersection?
    #[inline]
    pub const fn is_named_intersection(&self) -> bool {
        self.second_bit_field.is_named_intersection() != 0
    }

    // Access

    /// The access modes allowed to pass through this node.
    #[inline]
    pub fn access(&self) -> EnumSet<Access> {
        // SAFETY: The access bits are length 12, so invalid representations are impossible.
        unsafe { EnumSet::from_repr_unchecked(self.first_bit_field.access().get()) }
    }

    /// Was access information explicitly tagged
    /// (any of `access`, `motor_vehicle`, `foot`, and so on)?
    #[inline]
    pub const fn tagged_access(&self) -> bool {
        self.third_bit_field.tagged_access() != 0
    }

    /// Is access through the node private for all travel modes?
    #[inline]
    pub const fn private_access(&self) -> bool {
        self.third_bit_field.private_access() != 0
    }

    /// Is the node a cash-only toll (booth or barrier)?
    #[inline]
    pub const fn cash_only_toll(&self) -> bool {
        self.third_bit_field.is_cash_only_toll() != 0
    }

    /// Is a mode change allowed at this node?
    ///
    /// The access data tells us which modes are allowed at the node.
    /// Examples include transit stops, bike share locations, and parking locations.
    #[inline]
    pub const fn mode_change_allowed(&self) -> bool {
        self.second_bit_field.mode_change_allowed() != 0
    }

    // Regional data

    /// The index of the admin region containing this node (in the tile's admin list).
    #[inline]
    pub const fn admin_index(&self) -> u16 {
        self.second_bit_field.admin_index().get()
    }

    /// The node's time zone, as an index into Valhalla's fixed table of time zone names
    /// (`tz_name_to_id` in mainline's `src/baldr/datetime.cc`).
    ///
    /// Returns `None` if no time zone was assigned (index 0).
    #[inline]
    pub const fn time_zone_index(&self) -> Option<NonZeroU16> {
        NonZeroU16::new(
            self.second_bit_field.time_zone_index().get()
                | (self.third_bit_field.time_zone_ext().get() << 9),
        )
    }

    /// Do you drive on the right hand side of the road
    /// along edges originating at this node?
    #[inline]
    pub const fn drive_on_right(&self) -> bool {
        self.third_bit_field.drive_on_right() != 0
    }
}

#[cfg(test)]
mod test {
    use super::{IntersectionType, NodeInfo, NodeType, Traversability};
    use crate::Access;
    use crate::graph_tile::directed_edge::UnknownValueError;
    use crate::graph_tile::directed_edge::test::{load_tiles, mainline_records};
    use crate::graph_tile::{GraphTile, TEST_GRAPH_TILE_L0};
    use enumset::EnumSet;
    use proptest::prelude::*;
    use zerocopy::{FromBytes, IntoBytes};

    /// Checks every accessor against mainline 3.9.1 for a sample of nodes from every
    /// fixture tile (all three levels), including every node that shows a value
    /// not seen earlier in its tile.
    #[test]
    fn test_accessors_match_mainline() {
        let records = mainline_records("node");
        let tiles = load_tiles(&records);
        let levels: std::collections::BTreeSet<u8> =
            records.iter().map(|r| r.tile.level()).collect();
        assert_eq!(levels.len(), 3, "records cover all three levels");

        for r in &records {
            let tile = &tiles[&r.tile];
            let n = &tile.nodes()[r.index];
            let ctx = format!("node {}/{}/{}", r.tile.level(), r.tile.tile_id(), r.index);
            let int = |c: &str| r.int(c);
            let float = |c: &str| -> f64 { r.str(c).parse().expect("float") };

            // Coordinates are f32, so allow for its precision at these magnitudes.
            let coord = n.coordinate(tile.header().sw_corner());
            assert!((f64::from(coord.x) - float("lng")).abs() < 5e-6, "{ctx}");
            assert!((f64::from(coord.y) - float("lat")).abs() < 5e-6, "{ctx}");
            assert_eq!(i64::from(n.edge_index()), int("edge_index"), "{ctx}");
            assert_eq!(i64::from(n.edge_count()), int("edge_count"), "{ctx}");
            assert_eq!(
                n.access(),
                EnumSet::<Access>::from_repr(u16::try_from(int("access")).unwrap()),
                "{ctx}"
            );
            assert_eq!(
                i64::from(n.intersection_type().expect("known intersection") as u8),
                int("intersection"),
                "{ctx}"
            );
            assert_eq!(i64::from(n.admin_index()), int("admin_index"), "{ctx}");
            assert_eq!(
                n.time_zone_index().map_or(0, |i| i64::from(i.get())),
                int("timezone"),
                "{ctx}"
            );
            let driveability: Vec<u8> = (0..8)
                .map(|i| n.local_driveability(i).expect("index in range") as u8)
                .collect();
            assert_eq!(driveability, r.digits("local_driveability"), "{ctx}");
            assert_eq!(i64::from(n.density()), int("density"), "{ctx}");
            assert_eq!(
                i64::from(n.node_type().expect("known node type") as u8),
                int("type"),
                "{ctx}"
            );
            assert_eq!(
                i64::from(n.local_edge_count()),
                int("local_edge_count"),
                "{ctx}"
            );
            assert_eq!(n.drive_on_right(), r.flag("drive_on_right"), "{ctx}");
            // Every fixture node has elevation data, so mainline's value is exact.
            let elevation = n.elevation().expect("fixture nodes have elevation");
            assert!(
                (f64::from(elevation) - float("elevation")).abs() < 1e-9,
                "{ctx}"
            );
            assert_eq!(n.tagged_access(), r.flag("tagged_access"), "{ctx}");
            assert_eq!(n.private_access(), r.flag("private_access"), "{ctx}");
            assert_eq!(n.cash_only_toll(), r.flag("cash_only_toll"), "{ctx}");
            assert_eq!(n.mode_change_allowed(), r.flag("mode_change"), "{ctx}");
            assert_eq!(
                n.is_named_intersection(),
                r.flag("named_intersection"),
                "{ctx}"
            );
            assert_eq!(n.is_traffic_signal(), r.flag("traffic_signal"), "{ctx}");
            assert_eq!(
                i64::from(n.transition_index()),
                int("transition_index"),
                "{ctx}"
            );
            assert_eq!(
                i64::from(n.transition_count()),
                int("transition_count"),
                "{ctx}"
            );
            let headings: Vec<String> = (0..8)
                .map(|i| n.heading(i).expect("index in range").to_string())
                .collect();
            assert_eq!(headings.join(","), r.str("headings"), "{ctx}");
            assert_eq!(n.can_contract(), r.flag("can_contract"), "{ctx}");
            assert_eq!(n.is_transit(), r.flag("is_transit"), "{ctx}");
        }
    }

    /// Checks transitions against mainline 3.9.1 for a sample from every fixture tile.
    #[test]
    fn test_transitions_match_mainline() {
        let records = mainline_records("transition");
        let tiles = load_tiles(&records);
        for r in &records {
            let t = &tiles[&r.tile].borrow_dependent().transitions[r.index];
            let ctx = format!(
                "transition {}/{}/{}",
                r.tile.level(),
                r.tile.tile_id(),
                r.index
            );
            assert_eq!(
                i64::try_from(t.corresponding_end_node_id().value()).unwrap(),
                r.int("end_node_value"),
                "{ctx}"
            );
            assert_eq!(t.is_up(), r.flag("up"), "{ctx}");
        }
    }

    #[test]
    fn test_parse_nodes_count() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();

        assert_eq!(tile_view.nodes.len(), tile.header().node_count() as usize);
    }

    #[test]
    fn test_parse_nodes() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!("first_node", tile_view.nodes[0]);
            insta::assert_debug_snapshot!("last_node", tile_view.nodes.last().unwrap());
        }

        // Sanity check our coordinate parsing
        let node = &tile_view.nodes[0];
        let coord = node.coordinate(tile.header().sw_corner());
        // (cross-checked with NodeInfo::latlng in Valhalla 3.9.1)
        assert!((f64::from(coord.x) - 1.443_632_1).abs() < 5e-6);
        assert!((f64::from(coord.y) - 42.495_322_5).abs() < 5e-6);

        // Every mode except the reserved bit (access 2047 in Valhalla 3.9.1).
        assert_eq!(node.access(), EnumSet::all() - Access::Reserved);

        // The rest is cross-checked with Valhalla 3.9.1 (tools/oracle/cpp/records.cc).
        assert_eq!(node.edge_count(), 2);
        assert_eq!(node.local_edge_count(), 2);
        assert_eq!(node.intersection_type(), Ok(IntersectionType::False));
        assert_eq!(node.node_type(), Ok(NodeType::StreetIntersection));
        assert_eq!(
            node.time_zone_index().map(core::num::NonZeroU16::get),
            Some(293)
        );
        assert_eq!(node.elevation(), Some(1_331.5));
        assert_eq!(node.heading(0), Some(166));
        assert_eq!(node.heading(1), Some(353));
        assert_eq!(node.heading(8), None);
        assert_eq!(node.local_driveability(0), Some(Traversability::Both));
        assert_eq!(node.local_driveability(2), Some(Traversability::None));
        assert_eq!(node.local_driveability(8), None);
    }

    /// Builds a node from raw words, in tile byte order.
    fn node_from_words(words: [u64; 4]) -> NodeInfo {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        NodeInfo::read_from_bytes(&bytes).expect("32 bytes")
    }

    #[test]
    fn test_missing_elevation() {
        // A stored zero is how Valhalla records "no data" (anything at or below -500 m).
        assert_eq!(node_from_words([0; 4]).elevation(), None);
        // Elevation is bits 47..62 of the third word, in quarter meters above -500 m.
        assert_eq!(
            node_from_words([0, 0, 1 << 47, 0]).elevation(),
            Some(-499.75)
        );
        assert_eq!(
            node_from_words([0, 0, 2000 << 47, 0]).elevation(),
            Some(0.0)
        );
        assert_eq!(
            node_from_words([0, 0, 0x7fff << 47, 0]).elevation(),
            Some(7_691.75)
        );
    }

    #[test]
    fn test_unknown_enum_values_are_errors() {
        // Intersection type is bits 49..53 and node type bits 53..57 of the second word.
        let node = node_from_words([0, (4 << 49) | (15 << 53), 0, 0]);
        assert_eq!(
            node.intersection_type(),
            Err(UnknownValueError::new("intersection", 4))
        );
        assert_eq!(
            node.node_type(),
            Err(UnknownValueError::new("node type", 15))
        );
        assert!(!node.is_transit());
    }

    #[test]
    fn test_time_zone_extension_bit() {
        // The low nine bits are bits 40..49 of the second word;
        // the tenth is bit 62 of the third word.
        let node = node_from_words([0, 5 << 40, 1 << 62, 0]);
        assert_eq!(
            node.time_zone_index().map(core::num::NonZeroU16::get),
            Some(512 + 5)
        );
        assert_eq!(node_from_words([0; 4]).time_zone_index(), None);
    }

    /// Pins the bit position of every single-bit flag to the field order in
    /// `baldr/nodeinfo.h` (3.9.1).
    /// The fixtures never set some of these (mode changes and named intersections),
    /// so the mainline comparison alone can't catch a misplaced bit.
    #[test]
    fn test_flag_bit_positions() {
        type Flag = (usize, u32, &'static str, fn(&NodeInfo) -> bool);
        let flags: &[Flag] = &[
            (1, 61, "traffic_signal", NodeInfo::is_traffic_signal),
            (1, 62, "mode_change", NodeInfo::mode_change_allowed),
            (1, 63, "named", NodeInfo::is_named_intersection),
            (2, 43, "drive_on_right", NodeInfo::drive_on_right),
            (2, 44, "tagged_access", NodeInfo::tagged_access),
            (2, 45, "private_access", NodeInfo::private_access),
            (2, 46, "cash_only_toll", NodeInfo::cash_only_toll),
        ];
        for &(word, bit, name, _) in flags {
            let mut words = [0u64; 4];
            words[word] = 1 << bit;
            let node = node_from_words(words);
            for &(_, _, other, accessor) in flags {
                assert_eq!(
                    accessor(&node),
                    other == name,
                    "{other} with only {name} set"
                );
            }
        }
    }

    #[test]
    fn test_parse_transitions_count() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();

        assert_eq!(
            tile_view.transitions.len(),
            tile.header().transition_count() as usize
        );
    }

    #[test]
    fn test_parse_transitions() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!("first_transition", tile_view.transitions[0]);
            insta::assert_debug_snapshot!("last_transition", tile_view.transitions.last().unwrap());
        }
    }

    proptest! {
        /// No accessor panics on arbitrary bytes.
        #[test]
        fn accessors_never_panic(bytes in proptest::collection::vec(any::<u8>(), 32)) {
            let n = NodeInfo::read_from_bytes(&bytes).unwrap();
            let _ = (
                n.coordinate(geo::coord! { x: 0.0, y: 0.0 }), n.elevation(), n.edge_index(),
                n.edge_count(), n.local_edge_count(), n.transition_index(), n.transition_count(),
                n.intersection_type(), n.node_type(), n.is_transit(), n.can_contract(),
                n.density(), n.is_traffic_signal(), n.is_named_intersection(), n.access(),
                n.tagged_access(), n.private_access(), n.cash_only_toll(),
                n.mode_change_allowed(), n.admin_index(), n.time_zone_index(),
                n.drive_on_right(), format!("{n:?}"),
            );
            for i in 0..=u8::MAX {
                prop_assert_eq!(n.local_driveability(i).is_some(), i < 8);
                prop_assert_eq!(n.heading(i).is_some(), i < 8);
                if let Some(heading) = n.heading(i) {
                    prop_assert!(heading <= 359);
                }
            }
            prop_assert!((1..=8).contains(&n.local_edge_count()));
            if let Some(elevation) = n.elevation() {
                prop_assert!((-499.75..=7_691.75).contains(&elevation));
            }
            prop_assert_eq!(n.as_bytes(), &bytes[..]);
        }
    }
}
