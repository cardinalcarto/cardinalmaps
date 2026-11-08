//! # Lane connectivity
//!
//! Lane connectivity (from OSM `type=connectivity` relations) says which lanes of an incoming way
//! lead to which lanes of an edge.
//! Each record is 24 bytes (mainline's `LaneConnectivity`),
//! and the records are sorted by the edge they lead to.

use bitfield_struct::bitfield;
use core::fmt::{self, Display, Formatter};
use zerocopy::{LE, U64};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The number of bits per lane number.
const LANE_BITS: u32 = 4;

/// The maximum number of lanes in a lane list (`kMaxLanesPerConnection`).
const MAX_LANES: u32 = (1 << LANE_BITS) - 1;

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, KnownLayout)]
struct ConnectionBitfield {
    #[bits(22)]
    to: u32,
    #[bits(42)]
    from: u64,
}

/// A list of up to 15 lane numbers, 4 bits each (mainline's `LaneConnectivityLanes`).
///
/// Lanes are numbered from 1; a zero slot is empty.
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, KnownLayout, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct LaneConnectivityLanes(U64<LE>);

impl LaneConnectivityLanes {
    /// The raw packed value.
    #[inline]
    pub fn raw(&self) -> u64 {
        self.0.get()
    }

    /// Iterates over the lane numbers, skipping empty slots
    /// (as mainline's `LaneConnectivityLanes::to_string` does).
    #[inline]
    pub fn lanes(&self) -> impl Iterator<Item = u8> + use<> {
        let value = self.raw();
        (0..MAX_LANES).filter_map(move |n| {
            // Masked to 4 bits, so the truncation is exact.
            #[allow(clippy::cast_possible_truncation)]
            let lane = ((value >> (n * LANE_BITS)) & u64::from(MAX_LANES)) as u8;
            (lane != 0).then_some(lane)
        })
    }
}

impl Display for LaneConnectivityLanes {
    /// Formats the lanes as mainline does (for example, `1|2|3`).
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (i, lane) in self.lanes().enumerate() {
            if i > 0 {
                f.write_str("|")?;
            }
            write!(f, "{lane}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for LaneConnectivityLanes {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "LaneConnectivityLanes({self})")
    }
}

/// Lane connectivity from an incoming way to a directed edge.
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, KnownLayout, Clone)]
#[repr(C)]
pub struct LaneConnectivity {
    connection: ConnectionBitfield,
    to_lanes: LaneConnectivityLanes,
    from_lanes: LaneConnectivityLanes,
}

impl LaneConnectivity {
    /// The index (within the tile) of the directed edge the connection leads to.
    #[inline]
    pub fn to(&self) -> u32 {
        self.connection.to()
    }

    /// The OSM way ID of the incoming way.
    #[inline]
    pub fn from(&self) -> u64 {
        self.connection.from()
    }

    /// The lanes of the edge the connection leads to.
    #[inline]
    pub const fn to_lanes(&self) -> LaneConnectivityLanes {
        self.to_lanes
    }

    /// The lanes of the incoming way.
    #[inline]
    pub const fn from_lanes(&self) -> LaneConnectivityLanes {
        self.from_lanes
    }
}

impl fmt::Debug for LaneConnectivity {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaneConnectivity")
            .field("to", &self.to())
            .field("from", &self.from())
            .field("to_lanes", &self.to_lanes)
            .field("from_lanes", &self.from_lanes)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::graph_tile::section_oracle::load_fixture_tiles;
    use crate::graph_tile::{GraphTile, TileSections};
    use zerocopy::FromBytes;

    /// Packs lanes the way mainline's `LaneConnectivityLanes` constructor does.
    pub(crate) fn pack_lanes(lanes: &[u8]) -> u64 {
        lanes.iter().zip(0u32..).fold(0, |acc, (&lane, n)| {
            acc | u64::from(lane) << (n * LANE_BITS)
        })
    }

    pub(crate) fn encode(to: u32, from: u64, to_lanes: &[u8], from_lanes: &[u8]) -> Vec<u8> {
        let mut out = (u64::from(to) | from << 22).to_le_bytes().to_vec();
        out.extend(pack_lanes(to_lanes).to_le_bytes());
        out.extend(pack_lanes(from_lanes).to_le_bytes());
        out
    }

    #[test]
    fn test_layout() {
        assert_eq!(size_of::<LaneConnectivity>(), 24);
        let bytes = encode(4_194_303, (1 << 42) - 1, &[1, 2, 15], &[3, 0, 4]);
        let lc = LaneConnectivity::ref_from_bytes(&bytes).unwrap();
        assert_eq!(lc.to(), 4_194_303);
        assert_eq!(lc.from(), (1 << 42) - 1);
        assert_eq!(lc.to_lanes().to_string(), "1|2|15");
        // Empty slots are skipped, as in mainline.
        assert_eq!(lc.from_lanes().to_string(), "3|4");
        assert_eq!(lc.from_lanes().lanes().collect::<Vec<_>>(), vec![3, 4]);
    }

    #[test]
    fn test_lookup() {
        let mut bytes = encode(3, 100, &[1], &[1]);
        bytes.extend(encode(5, 101, &[1], &[2]));
        bytes.extend(encode(5, 102, &[2], &[3]));
        bytes.extend(encode(9, 103, &[1], &[1]));
        let sections = TileSections {
            signs: &[],
            turn_lanes: &[],
            admins: &[],
            complex_forward: &[],
            complex_reverse: &[],
            text_list: &[],
            lane_connectivity: &bytes,
        };
        let found = sections.get_lane_connectivity(5);
        assert_eq!(
            found.iter().map(LaneConnectivity::from).collect::<Vec<_>>(),
            vec![101, 102]
        );
        assert!(sections.get_lane_connectivity(4).is_empty());
        assert!(sections.get_lane_connectivity(10).is_empty());

        // Trailing bytes that do not make a whole record (like the tile's alignment padding)
        // are ignored, as in mainline.
        let padded = TileSections {
            lane_connectivity: &bytes[..24 * 2 + 4],
            ..sections
        };
        assert_eq!(padded.lane_connectivity().len(), 2);
        assert_eq!(padded.get_lane_connectivity(5).len(), 1);
    }

    #[test]
    fn test_fixture_padding() {
        // The fixture tiles have no lane connectivity (mainline finds none either),
        // but some have a few bytes of padding where it would be
        // (up to 8 here, since more than one build step pads the end of the tile).
        let tiles = load_fixture_tiles();
        let padding = tiles
            .iter()
            .map(|t| t.sections().lane_connectivity.len())
            .collect::<Vec<_>>();
        assert!(padding.iter().any(|&p| p > 0));
        assert!(
            padding.iter().all(|&p| p < size_of::<LaneConnectivity>()),
            "{padding:?}"
        );
        for tile in &tiles {
            assert!(tile.sections().lane_connectivity().is_empty());
            assert!(tile.get_lane_connectivity(0).is_empty());
        }
    }
}
