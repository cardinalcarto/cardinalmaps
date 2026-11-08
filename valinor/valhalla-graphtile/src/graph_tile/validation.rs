//! # Cross-section checks for graph tiles
//!
//! [`GraphTileHeader::validate`](super::GraphTileHeader::validate) checks the header on its own,
//! and reading the sections checks that each one fits in the tile.
//! This module checks the references *between* sections,
//! such as a node's range of outbound edges, or an edge's offset into the edge info.
//!
//! [`GraphTileView`](super::GraphTileView) runs these checks once, when it parses a tile,
//! so that its accessors can index into the sections without any risk of panicking.
//!
//! # Performance
//!
//! These checks visit every node, directed edge, and access restriction once.
//! Each visit is a few integer comparisons on data that parsing has just mapped,
//! so this is cheap next to anything that reads the tile,
//! but it is not free for very large tiles.

use super::predicted_speeds::COEFFICIENT_COUNT;
use super::{AccessRestriction, DirectedEdge, NodeInfo};
use crate::RoadUse;
use thiserror::Error;
use zerocopy::{LE, U32, U64};

/// Ways in which the data sections of a graph tile can be inconsistent with each other.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphTileDataError {
    #[error(
        "read node {node}: its outbound edges {start}..{end} run past the {edge_count} directed edges in the tile"
    )]
    NodeEdgesOutOfBounds {
        node: usize,
        start: usize,
        end: usize,
        edge_count: usize,
    },
    #[error(
        "read node {node}: its transitions {start}..{end} run past the {transition_count} transitions in the tile"
    )]
    NodeTransitionsOutOfBounds {
        node: usize,
        start: usize,
        end: usize,
        transition_count: usize,
    },
    #[error(
        "read directed edge {edge}: its edge info offset {offset} is outside the {size}-byte edge info section"
    )]
    EdgeInfoOffsetOutOfBounds {
        edge: usize,
        offset: usize,
        size: usize,
    },
    /// Raised when looking up the edge info of an edge from another tile,
    /// since parsing rules this out for the tile's own edges.
    #[error(
        "read the edge info at offset {offset}: it is outside the {size}-byte edge info section (is the edge from another tile?)"
    )]
    EdgeInfoLookupOutOfBounds { offset: usize, size: usize },
    #[error("read directed edge {edge}: {value} is not a road use")]
    InvalidRoadUse { edge: usize, value: u8 },
    #[error(
        "read access restriction {index}: its edge index {edge_index} is smaller than the one before it (the restrictions must be sorted by edge index)"
    )]
    UnsortedAccessRestrictions { index: usize, edge_index: u32 },
    #[error(
        "read the predicted speeds of directed edge {edge}: offset {offset} leaves less than a whole profile in the {profile_count} speed profiles"
    )]
    InvalidPredictedSpeedOffset {
        edge: usize,
        offset: usize,
        profile_count: usize,
    },
}

/// Checks that every node's outbound edges and transitions are within their sections.
///
/// # Errors
///
/// Returns an error for the first node whose ranges run past the end of a section.
pub(super) fn check_nodes(
    nodes: &[NodeInfo],
    edge_count: usize,
    transition_count: usize,
) -> Result<(), GraphTileDataError> {
    for (node, info) in nodes.iter().enumerate() {
        // These are 21- or 22-bit indexes and 7-bit counts, so the sums cannot overflow.
        let start = info.edge_index() as usize;
        let end = start + usize::from(info.edge_count());
        if end > edge_count {
            return Err(GraphTileDataError::NodeEdgesOutOfBounds {
                node,
                start,
                end,
                edge_count,
            });
        }

        let start = info.transition_index() as usize;
        let end = start + usize::from(info.transition_count());
        if end > transition_count {
            return Err(GraphTileDataError::NodeTransitionsOutOfBounds {
                node,
                start,
                end,
                transition_count,
            });
        }
    }
    Ok(())
}

/// Checks every directed edge's references into other sections,
/// and that its road use is valid.
///
/// `predicted_speed_offsets` is empty if the tile has no predicted speeds.
/// Otherwise, it has one offset per directed edge.
///
/// # Errors
///
/// Returns an error for the first edge with an invalid field.
pub(super) fn check_directed_edges(
    edges: &[DirectedEdge],
    edge_info_size: usize,
    predicted_speed_offsets: &[U32<LE>],
    predicted_speed_profile_count: usize,
) -> Result<(), GraphTileDataError> {
    let has_predicted_speeds = !predicted_speed_offsets.is_empty();
    for (edge, directed_edge) in edges.iter().enumerate() {
        // An edge info record is never empty, so it must start before the end of the section.
        let offset = directed_edge.edge_info_offset() as usize;
        if offset >= edge_info_size {
            return Err(GraphTileDataError::EdgeInfoOffsetOutOfBounds {
                edge,
                offset,
                size: edge_info_size,
            });
        }

        let value = raw_road_use(directed_edge);
        if VALID_ROAD_USES & (1 << value) == 0 {
            return Err(GraphTileDataError::InvalidRoadUse { edge, value });
        }

        // Like Valhalla, only read the offsets of edges that claim to have predicted speeds.
        // A tile without predicted speeds has no offsets,
        // and lookups on it return no speed regardless of this flag.
        if has_predicted_speeds
            && directed_edge.has_predicted_speed()
            && let Some(offset) = predicted_speed_offsets.get(edge)
        {
            // Offsets count 16-bit coefficients from the start of the profiles.
            // Like Valhalla, a profile may start at any offset
            // (Valhalla only ever writes multiples of the profile size),
            // as long as a whole profile follows it.
            let offset = offset.get() as usize;
            if offset + COEFFICIENT_COUNT > predicted_speed_profile_count * COEFFICIENT_COUNT {
                return Err(GraphTileDataError::InvalidPredictedSpeedOffset {
                    edge,
                    offset,
                    profile_count: predicted_speed_profile_count,
                });
            }
        }
    }
    Ok(())
}

/// Checks that the access restrictions are sorted by edge index.
///
/// Valhalla writes them sorted and finds an edge's restrictions with a binary search.
/// We do the same, so an unsorted list would silently drop restrictions.
///
/// # Errors
///
/// Returns an error for the first restriction that is out of order.
pub(super) fn check_access_restrictions(
    restrictions: &[AccessRestriction],
) -> Result<(), GraphTileDataError> {
    let mut previous = 0;
    for (index, restriction) in restrictions.iter().enumerate() {
        let edge_index = restriction.edge_index();
        if edge_index < previous {
            return Err(GraphTileDataError::UnsortedAccessRestrictions { index, edge_index });
        }
        previous = edge_index;
    }
    Ok(())
}

/// A bit set of the valid 6-bit road use values,
/// so that checking an edge is a shift and a mask rather than a match.
const VALID_ROAD_USES: u64 = {
    let mut mask = 0u64;
    let mut value = 0u8;
    while value < 64 {
        if RoadUse::try_from_bits(value).is_some() {
            mask |= 1 << value;
        }
        value += 1;
    }
    mask
};

/// Reads the raw 6-bit road use field of a directed edge.
///
/// [`DirectedEdge::road_use`] maps invalid values to [`RoadUse::Other`]
/// (see [`RoadUse::try_from_bits`]),
/// so validation needs the raw value instead.
///
/// The field is bits 40 to 45 of the edge's third 64-bit word,
/// after the speed, free flow speed, constrained flow speed, truck speed,
/// and name consistency bytes.
/// See `baldr/directededge.h`.
fn raw_road_use(edge: &DirectedEdge) -> u8 {
    // The size of this transmute is checked at compile time.
    let words: &[U64<LE>; 6] = zerocopy::transmute_ref!(edge);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is masked to 6 bits"
    )]
    let value = ((words[2].get() >> 40) & 0x3f) as u8;
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_tile::{GraphTile, TEST_GRAPH_TILE_L0, TEST_GRAPH_TILE_L2};
    use zerocopy::IntoBytes;

    #[test]
    fn test_raw_road_use_matches_accessor() {
        for tile in [&*TEST_GRAPH_TILE_L0, &*TEST_GRAPH_TILE_L2] {
            for edge in tile.directed_edges() {
                let raw = raw_road_use(edge);
                assert_eq!(RoadUse::try_from_bits(raw), Some(edge.road_use()));
            }
        }
    }

    #[test]
    fn test_invalid_road_use_is_rejected() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let mut edges = tile.directed_edges()[..3].to_vec();
        // Use 12 is unassigned in Valhalla 3.9.1.
        let bytes = edges[1].as_mut_bytes();
        let mut word = [0u8; 8];
        word.copy_from_slice(&bytes[16..24]);
        let word = (u64::from_le_bytes(word) & !(0x3f << 40)) | (12 << 40);
        bytes[16..24].copy_from_slice(&word.to_le_bytes());
        assert_eq!(raw_road_use(&edges[1]), 12);
        // The accessor falls back to `Other` rather than panicking.
        assert_eq!(edges[1].road_use(), RoadUse::Other);

        assert_eq!(
            check_directed_edges(&edges, usize::MAX, &[], 0),
            Err(GraphTileDataError::InvalidRoadUse { edge: 1, value: 12 })
        );
    }

    #[test]
    fn test_access_restriction_order() {
        let tile = TEST_GRAPH_TILE_L0.borrow_dependent();
        let restrictions = tile.access_restrictions;
        assert!(restrictions.len() > 2);
        assert_eq!(check_access_restrictions(restrictions), Ok(()));

        let mut reversed = restrictions.to_vec();
        reversed.reverse();
        let first_drop = reversed
            .windows(2)
            .position(|w| w[1].edge_index() < w[0].edge_index())
            .expect("the fixture has restrictions on more than one edge");
        assert_eq!(
            check_access_restrictions(&reversed),
            Err(GraphTileDataError::UnsortedAccessRestrictions {
                index: first_drop + 1,
                edge_index: reversed[first_drop + 1].edge_index(),
            })
        );
    }
}
