//! # Routing Graph Tiles
//!
//! This module provides definitions for Valhalla-compatible routing graph tiles.
//! Access is defined by the [`GraphTile`] trait.
//! The memory layout, down to the bit level, is specified in the individual data structures.
//! Start at the [`GraphTileView`], which can reinterpret a byte slice safely as a tile,
//! and work down from there as needed.
//! For writing tiles, a safe builder API is provided in [`GraphTileBuilder`].

use core::ops::Deref;
use std::sync::Arc;
use thiserror::Error;
use zerocopy::{FromBytes, I16, Immutable, LE, U32};

use enumset::EnumSet;
use geo::{CoordFloat, Point, coord};
use num_traits::FromPrimitive;
#[cfg(test)]
use std::sync::LazyLock;
// To keep files manageable, we will keep internal modules specific to each mega-struct,
// and re-export the public type.
// Each struct should be tested on fixture tiles to a reasonable level of confidence.
// Leverage snapshot tests where possible, compare with Valhalla C++ results,
// and test the first and last elements of variable length vectors
// with snapshot tests.

mod access_restriction;
mod admin;
mod bounding_circle;
mod builder;
mod checksum;
mod directed_edge;
mod edge_info;
pub mod handles;
mod header;
mod node;
pub mod predicted_speeds;
mod sign;
mod transit;
mod turn_lane;
mod validation;

// Variable-length sections and the values stored in the text list.
mod complex_restriction;
mod lane_connectivity;
mod linguistic;
#[cfg(test)]
mod section_oracle;
mod sections;
mod tagged_value;
mod time_domain;

use crate::graph_tile::handles::OwnedGraphTileHandle;
use crate::graph_tile::predicted_speeds::{
    COEFFICIENT_COUNT, PredictedSpeedCodecError, PredictedSpeeds,
};
use crate::spatial::DistanceApproximator;
pub use crate::{
    Access,
    graph_id::{GraphId, InvalidGraphIdError},
};
pub use access_restriction::{AccessRestriction, AccessRestrictionType};
pub use admin::Admin;
pub use bounding_circle::{BoundingCircle, DiscretizedBoundingCircle};
pub use builder::GraphTileBuilder;
pub use checksum::{BuildId, TileHash, TileHasher};
pub use directed_edge::{
    CycleLane, DirectedEdge, DirectedEdgeExt, HovType, LocalEdgeMask, SacScale, SpeedType,
    TurnType, UnknownValueError,
};
pub use edge_info::EdgeInfo;
pub use header::{GraphTileHeader, GraphTileHeaderError};
pub use node::{IntersectionType, NodeInfo, NodeTransition, NodeType, Traversability};
pub use sign::{Sign, SignType};
pub use transit::{TransitDeparture, TransitRoute, TransitSchedule, TransitStop, TransitTransfer};
pub use turn_lane::TurnLane;
pub use validation::GraphTileDataError;

// Variable-length sections and the values stored in the text list.
pub use access_restriction::AccessRestrictionValue;
pub use complex_restriction::{
    ComplexRestriction, ComplexRestrictions, RestrictionDateTime, RestrictionType,
};
pub use edge_info::{EdgeName, EncodedElevation, NameInfo};
pub use lane_connectivity::{LaneConnectivity, LaneConnectivityLanes};
pub use linguistic::{
    Language, LinguisticMap, LinguisticRecord, LinguisticRecords, PronunciationAlphabet,
};
pub use sections::{AdminInfo, SectionDecodingError, SignInfo, TileSections};
pub use tagged_value::{
    Landmark, LandmarkType, LevelRanges, Levels, OsmNodeIds, TaggedValue, TaggedValueKind,
};
pub use time_domain::{ConditionalSpeedLimit, DayType, TimeDomain};
pub use turn_lane::{TurnLaneDirection, TurnLaneMask, TurnLanes};

/// Ways in which reading graph tile data can fail.
///
/// Tile data comes from outside the process (often from a download, on a phone),
/// so every way in which it can be truncated, corrupt, or unsupported is an error here,
/// never a panic.
#[derive(Debug, Error)]
pub enum GraphTileDecodingError {
    #[error("extract a slice of the correct length (the tile data is malformed)")]
    SliceArrayConversion(#[from] core::array::TryFromSliceError),
    #[error("extract a slice of the correct length (the tile data is malformed)")]
    SliceLength,
    #[error("interpret the bytes as this type (the byte sequence is not valid for it)")]
    ByteSequenceValidityError,
    #[error("read a graph ID: {0}")]
    GraphIdParseError(#[from] InvalidGraphIdError),
    #[error(
        "read the {field} section (this almost always means invalid data): {error_description}"
    )]
    CastError {
        field: String,
        error_description: String,
    },
    #[error(
        "consume all bytes in the graph tile: {0} bytes are left over (either the tile is invalid or the reader has a bug)"
    )]
    LeftoverBytesAfterReading(usize),
    /// The header is invalid, inconsistent, or from an unsupported version.
    #[error(transparent)]
    InvalidHeader(#[from] GraphTileHeaderError),
    /// The data sections are inconsistent with each other.
    #[error(transparent)]
    InvalidData(#[from] GraphTileDataError),
}

#[derive(Debug, Error)]
pub enum GraphTileBuildError {
    #[error("{0}")]
    InvalidGraphId(#[from] InvalidGraphIdError),
    #[error(
        "set the version string: {0:?} must be at most 15 bytes of UTF-8 without NUL (mainline reads it as a NUL-terminated string in 16 bytes)"
    )]
    InvalidVersionString(String),
    #[error("Invalid index: {0}.")]
    InvalidIndex(String),
    #[error(
        "Bitfield overflow: Value {value} for field {field} exceeds the allowed number of bits."
    )]
    BitfieldOverflow { field: String, value: usize },
    #[error("{0:?}")]
    PredictedSpeedCodec(#[from] PredictedSpeedCodecError),
    #[error(
        "Unable to cast an integer to another type (usually means data is too large for the type): {0:?}."
    )]
    TryFromInt(#[from] std::num::TryFromIntError),
    #[error("Error writing tile: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Poisoned cache lock when writing tile: {0}")]
    PoisonedCacheLock(String),
    #[error(
        "write bounding circles: there are {bounding_circles} circles for {edge_bins} edge bin entries (there must be one per entry, or none)"
    )]
    MismatchedBoundingCircles {
        edge_bins: usize,
        bounding_circles: usize,
    },
}

#[derive(Debug, Error)]
pub enum LookupError {
    #[error("Mismatched base; the graph ID cannot exist in this tile.")]
    MismatchedBase,
    #[error("The feature base graph ID matches this tile, but the index is invalid.")]
    InvalidIndex,
}

pub trait GraphTile {
    /// Gets the Graph ID of the tile.
    fn graph_id(&self) -> GraphId;

    /// Does the supplied graph ID belong in this tile?
    ///
    /// A true result does not necessarily guarantee that an object with this ID exists,
    /// but in that case, either you've probably, cooked up an invalid ID for fun,
    /// the graph tiles are invalid, or you've confused a graph ID and a node ID.
    fn may_contain_id(&self, id: GraphId) -> bool;

    /// Gets a reference to the [`GraphTileHeader`].
    fn header(&self) -> &GraphTileHeader;

    /// Gets a reference to a node in this tile with the given graph ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the graph ID cannot be contained in this tile
    /// or the index is invalid.
    fn get_node(&self, id: GraphId) -> Result<&NodeInfo, LookupError>;

    /// Gets a slice of all outbound directed edges from the node.
    ///
    /// No filtering is applied, and this includes edges of all types.
    /// (See also: [`get_transitions`](GraphTile::get_transitions)
    /// if you need edges that may leave the current hierarchy level).
    ///
    /// # Performance
    ///
    /// This does not perform any heap allocations and returns a continuous slice.
    ///
    /// # Correctness
    ///
    /// The node info must come from this tile.
    /// It does not contain tile information, so it is the responsibility of the caller
    /// to ensure this.
    /// Parsing a tile checks that the edges of each of its nodes are in range,
    /// so this never panics,
    /// but the result for a node from another tile is meaningless
    /// (and empty if its edges would be out of range for this tile).
    fn get_outbound_edges_from_node(&self, node_info: &NodeInfo) -> &[DirectedEdge];

    /// Gets a slice of all (outbound) transitions (links to other levels) from a node.
    ///
    /// # Correctness
    ///
    /// The node info must come from this tile,
    /// as for [`get_outbound_edges_from_node`](GraphTile::get_outbound_edges_from_node).
    fn get_transitions(&self, node: &NodeInfo) -> &[NodeTransition];

    /// Gets a reference to the directed edge in this tile by graph ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the graph ID cannot be contained in this tile
    /// or the index is invalid.
    fn get_directed_edge(&self, id: GraphId) -> Result<&DirectedEdge, LookupError>;

    /// Gets an index which can be used to retrieve the opposing edge for a given graph ID.
    ///
    /// As long as the original `graph_id` can exist within the tile,
    /// this method will return an index, even if it exists in another tile.
    /// However, access to other tiles may be required to actually get the edge,
    /// or create an unambiguous [`GraphID`].
    ///
    /// # Errors
    ///
    /// Returns a [`LookupError`] if the graph ID is not present in the tile.
    fn get_opp_edge_index(&self, graph_id: GraphId) -> Result<OpposingEdgeIndex, LookupError>;

    /// Gets a reference to an extended directed edge in this tile by graph ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the graph ID cannot be contained in this tile
    /// or the index is invalid.
    fn get_ext_directed_edge(&self, id: GraphId) -> Result<&DirectedEdgeExt, LookupError>;

    /// Gets access restrictions for a directed edge.
    ///
    /// The returned list includes restrictions that apply
    /// to *any* of the supplied access modes (ex: auto, bicycle, etc.).
    fn get_access_restrictions(
        &self,
        directed_edge_index: u32,
        access_modes: EnumSet<Access>,
    ) -> Vec<&AccessRestriction>;

    /// Gets predicted speed information for a directed edge.
    ///
    /// `seconds_from_start_of_week` is measured from midnight Sunday **local time**.
    /// The output is measured in kilometers per hour.
    /// Returns `None` if the edge at this index does not have predicted speed information.
    fn get_predicted_speed(
        &self,
        directed_edge_index: usize,
        seconds_from_start_of_week: u32,
    ) -> Option<f32>;

    /// Gets edge info for a directed edge.
    ///
    /// Note that this is NOT a zero-cost operation.
    /// While it does not immediately decode everything,
    /// several memory accesses and checks need to happen upfront.
    /// As such, it is recommended to avoid calling this during costing.
    ///
    /// # Errors
    ///
    /// Since this accepts a directed edge reference,
    /// two types of errors are possible:
    ///
    /// * An invalid/corrupt graph tile (a serious tile validity error)
    /// * An edge not belonging to the current tile (there is no way to verify this; it is the caller's responsibility)
    fn get_edge_info(
        &self,
        directed_edge: &DirectedEdge,
    ) -> Result<EdgeInfo<'_>, GraphTileDecodingError>;

    // Lower level "raw" accessors

    /// A raw slice of the tile's directed edges (i.e. for iteration).
    fn directed_edges(&self) -> &[DirectedEdge];

    /// A raw slice of the tile's nodes (i.e. for iteration).
    fn nodes(&self) -> &[NodeInfo];

    /// Administrative regions covered in this tile.
    fn admins(&self) -> &[Admin];

    /// Returns the list of edge IDs contained in the specified bin.
    ///
    /// The bin contents are stored as a single concatenated array with prefix offsets
    /// for each bin in the tile header. This method returns a slice of that array.
    ///
    /// Valhalla's edge bins speed up for finding edges by dividing the level 2 tiles into
    /// a grid and storing edges that cross through each bounding box.
    /// This limits the search space, since it's relatively expensive to deserialize edge geometry.
    ///
    /// While the directed graph edges always come in pairs,
    /// only one edge from the pair is included in the edge bins!
    ///
    /// Returns an empty slice if the tile has no edge bins (outside of level 2),
    /// or if the bin index is out of range ([`crate::BIN_COUNT`] or greater).
    fn edges_in_bin(&self, bin_index: usize) -> &[GraphId];

    /// Returns the bounding circles of the edges in the specified bin.
    ///
    /// When the tile has bounding circles,
    /// the result is parallel to [`edges_in_bin`](GraphTile::edges_in_bin):
    /// the circle at each position bounds the shape of the edge at the same position.
    /// Decode a circle with [`DiscretizedBoundingCircle::decode`],
    /// using the bin center from [`GraphTileHeader::bin_center`].
    ///
    /// Returns an empty slice if the tile has no bounding circles
    /// (which is always the case outside of level 2, and for tiles built before Valhalla 3.9),
    /// or if the bin index is out of range ([`crate::BIN_COUNT`] or greater).
    fn bounding_circles_in_bin(&self, bin_index: usize) -> &[DiscretizedBoundingCircle];

    /// Computes the row-major bin index for the given bin coordinates within
    /// the tile's bin grid.
    ///
    /// Every level uses the same 5x5 grid (Valhalla's `kBinsDim`),
    /// so this does not depend on the tile's data.
    ///
    /// Returns `None` if `x` or `y` is not less than [`crate::BINS_PER_SIDE`].
    fn bin_index_xy(&self, x: usize, y: usize) -> Option<usize>;

    /// Returns an iterator over all nodes near a specified point,
    /// within this tile, including the approximate distance from the point.
    ///
    /// No sorting or filtering of nodes is performed besides ensuring that they are close enough.
    /// The distance returned (second tuple element) is approximate,
    /// but should be accurate to within 1 meter for radii of up to 20km.
    ///
    /// # Performance
    ///
    /// This method is very fast for a few reasons:
    ///
    /// - It only considers nodes within a single tile (which you already have a reference to!)
    /// - The result is an unsorted iterator
    /// - Internally, an approximator is used to (very!) quickly weed out unrealistic candidates
    ///   before doing the heavier trigonometry.
    ///
    /// Currently, this requires a scan of all nodes in the tile.
    /// This is a lot faster than in may sound,
    /// but in the future, it would be interesting to explore k-d trees
    /// for things like map matching, which need to find candidates for a large number of points.
    ///
    /// A k-d tree also maps directly to the most common question,
    /// which is to retrieve the `k` nearest nodes.
    ///
    /// # Accuracy
    ///
    /// This isn't intended to be used to get nodes over large distances;
    /// see [`DistanceApproximator`] for the limits of the approximation.
    /// A NaN center or radius matches no nodes.
    #[inline]
    fn nodes_within_radius<N: CoordFloat + FromPrimitive, F: Fn(&NodeInfo) -> bool>(
        &self,
        filter_predicate: F,
        center: Point<N>,
        radius_in_meters: N,
    ) -> impl Iterator<Item = (GraphNode<'_>, N)> {
        let header = self.header();
        let level = header.graph_id().level();
        let tile_id = header.graph_id().tile_id();
        let sw = header.sw_corner();

        // Set up an approximator based on square distance.
        // This is good enough for reasonable distances (<1m for distances up to a few tens of km).
        let approximator = DistanceApproximator::new(center.into());
        let radius_squared = radius_in_meters * radius_in_meters;

        self.nodes()
            .iter()
            .enumerate()
            .filter_map(move |(idx, node_info)| {
                if !filter_predicate(node_info) {
                    return None;
                }

                let node_id =
                    GraphId::try_from_components(level, tile_id, u64::try_from(idx).ok()?).ok()?;
                let nc = node_info.coordinate(sw);

                // Converting from f64 never fails for f32 or f64.
                let nc_lon = N::from(nc.x)?;
                let nc_lat = N::from(nc.y)?;
                let sq_dist = approximator.distance_squared(coord! {x: nc_lon, y: nc_lat});
                if sq_dist <= radius_squared {
                    Some((GraphNode { node_id, node_info }, sq_dist.sqrt()))
                } else {
                    None
                }
            })
    }

    // Variable-length sections: signs, turn lanes, complex restrictions,
    // lane connectivity, and admins.
    // Only `sections` needs an implementation; the lookups are built on it.

    /// Borrowed views of the tile's variable-length sections.
    fn sections(&self) -> TileSections<'_>;

    /// Gets the signs for a directed edge (or, with `signs_on_node`, a node) index,
    /// as mainline's `GraphTile::GetSigns(idx, signs_on_node)` does.
    ///
    /// See [`TileSections::get_signs`].
    ///
    /// # Errors
    ///
    /// Returns an error if a sign's text is outside the text list or malformed.
    #[inline]
    fn get_signs(
        &self,
        index: u32,
        signs_on_node: bool,
    ) -> Result<Vec<SignInfo<'_>>, SectionDecodingError> {
        self.sections().get_signs(index, signs_on_node)
    }

    /// Gets the signs for a directed edge (or node) index and their pronunciations,
    /// as mainline's `GraphTile::GetSigns(idx, index_linguistic_map, signs_on_node)` does.
    ///
    /// See [`TileSections::get_signs_with_linguistics`].
    ///
    /// # Errors
    ///
    /// Returns an error if a sign's text is outside the text list or malformed.
    #[inline]
    fn get_signs_with_linguistics(
        &self,
        index: u32,
        signs_on_node: bool,
    ) -> Result<(Vec<SignInfo<'_>>, LinguisticMap<'_>), SectionDecodingError> {
        self.sections()
            .get_signs_with_linguistics(index, signs_on_node)
    }

    /// Gets the turn lanes at the end of a directed edge, if it has any
    /// (mainline's `GraphTile::turnlanes`).
    ///
    /// See [`TileSections::get_turn_lanes`].
    ///
    /// # Errors
    ///
    /// Returns an error if the turn lane text is outside the text list.
    #[inline]
    fn get_turn_lanes(
        &self,
        directed_edge_index: u32,
    ) -> Result<Option<TurnLanes<'_>>, SectionDecodingError> {
        self.sections().get_turn_lanes(directed_edge_index)
    }

    /// Iterates over the complex restrictions that apply to an edge and any of the given modes,
    /// as mainline's `GraphTile::GetComplexRestrictions(forward, id, modes)` does.
    ///
    /// See [`TileSections::get_complex_restrictions`].
    ///
    /// # Errors
    ///
    /// The iterator yields an error (and then stops) if the section is truncated.
    #[inline]
    fn get_complex_restrictions(
        &self,
        forward: bool,
        edge_id: GraphId,
        modes: EnumSet<Access>,
    ) -> impl Iterator<Item = Result<ComplexRestriction<'_>, SectionDecodingError>> {
        self.sections()
            .get_complex_restrictions(forward, edge_id, modes)
    }

    /// Gets the lane connections that end on a directed edge
    /// (mainline's `GraphTile::GetLaneConnectivity`).
    #[inline]
    fn get_lane_connectivity(&self, directed_edge_index: u32) -> &[LaneConnectivity] {
        self.sections().get_lane_connectivity(directed_edge_index)
    }

    /// Gets the admin at an index with its names resolved
    /// (mainline's `GraphTile::admininfo`).
    ///
    /// # Errors
    ///
    /// Returns an error if the index is out of bounds
    /// or the names are outside the text list.
    #[inline]
    fn get_admin_info(&self, index: usize) -> Result<AdminInfo<'_>, SectionDecodingError> {
        self.sections().get_admin_info(index)
    }
}

impl<T> GraphTile for Arc<T>
where
    T: GraphTile,
{
    #[inline]
    fn graph_id(&self) -> GraphId {
        self.deref().graph_id()
    }

    #[inline]
    fn may_contain_id(&self, id: GraphId) -> bool {
        self.deref().may_contain_id(id)
    }

    #[inline]
    fn header(&self) -> &GraphTileHeader {
        self.deref().header()
    }

    #[inline]
    fn get_node(&self, id: GraphId) -> Result<&NodeInfo, LookupError> {
        self.deref().get_node(id)
    }

    #[inline]
    fn get_outbound_edges_from_node(&self, node_info: &NodeInfo) -> &[DirectedEdge] {
        self.deref().get_outbound_edges_from_node(node_info)
    }

    #[inline]
    fn get_transitions(&self, node: &NodeInfo) -> &[NodeTransition] {
        self.deref().get_transitions(node)
    }

    #[inline]
    fn get_directed_edge(&self, id: GraphId) -> Result<&DirectedEdge, LookupError> {
        self.deref().get_directed_edge(id)
    }

    #[inline]
    fn get_opp_edge_index(&self, graph_id: GraphId) -> Result<OpposingEdgeIndex, LookupError> {
        self.deref().get_opp_edge_index(graph_id)
    }

    #[inline]
    fn get_ext_directed_edge(&self, id: GraphId) -> Result<&DirectedEdgeExt, LookupError> {
        self.deref().get_ext_directed_edge(id)
    }

    #[inline]
    fn get_access_restrictions(
        &self,
        directed_edge_index: u32,
        access_modes: EnumSet<Access>,
    ) -> Vec<&AccessRestriction> {
        self.deref()
            .get_access_restrictions(directed_edge_index, access_modes)
    }

    #[inline]
    fn get_predicted_speed(
        &self,
        directed_edge_index: usize,
        seconds_from_start_of_week: u32,
    ) -> Option<f32> {
        self.deref()
            .get_predicted_speed(directed_edge_index, seconds_from_start_of_week)
    }

    #[inline]
    fn get_edge_info(
        &self,
        directed_edge: &DirectedEdge,
    ) -> Result<EdgeInfo<'_>, GraphTileDecodingError> {
        self.deref().get_edge_info(directed_edge)
    }

    #[inline]
    fn directed_edges(&self) -> &[DirectedEdge] {
        self.deref().directed_edges()
    }

    #[inline]
    fn nodes(&self) -> &[NodeInfo] {
        self.deref().nodes()
    }

    #[inline]
    fn admins(&self) -> &[Admin] {
        self.deref().admins()
    }

    #[inline]
    fn edges_in_bin(&self, bin_index: usize) -> &[GraphId] {
        self.deref().edges_in_bin(bin_index)
    }

    #[inline]
    fn bounding_circles_in_bin(&self, bin_index: usize) -> &[DiscretizedBoundingCircle] {
        self.deref().bounding_circles_in_bin(bin_index)
    }

    #[inline]
    fn bin_index_xy(&self, x: usize, y: usize) -> Option<usize> {
        self.deref().bin_index_xy(x, y)
    }

    // Variable-length sections
    #[inline]
    fn sections(&self) -> TileSections<'_> {
        self.deref().sections()
    }
}

/// A node in a graph tile.
///
/// Nodes represent intersections between edges in the graph.
/// These are not directly analogous to OSM nodes; most nodes in OSM don't exist in the graph,
/// as they are only needed for geometry, and would make traversal / path finding slow.
pub struct GraphNode<'a> {
    /// The Graph ID identifying this node.
    pub node_id: GraphId,
    pub node_info: &'a NodeInfo,
}

/// A view over a single tile in the Valhalla hierarchical tile graph.
///
/// Access should normally go through the [`GraphTile`] trait.
///
/// # Cloning
///
/// Cloning this struct is cheap, since the internal types are all references.
#[derive(Clone)]
pub struct GraphTileView<'a> {
    /// Header with various metadata about the tile and internal sizes.
    header: &'a GraphTileHeader,
    /// The list of nodes in the graph tile.
    nodes: &'a [NodeInfo],
    /// The list of transitions between nodes on different levels.
    transitions: &'a [NodeTransition],
    directed_edges: &'a [DirectedEdge],
    ext_directed_edges: &'a [DirectedEdgeExt],
    access_restrictions: &'a [AccessRestriction],
    transit_departures: &'a [TransitDeparture],
    transit_stops: &'a [TransitStop],
    transit_routes: &'a [TransitRoute],
    transit_schedules: &'a [TransitSchedule],
    transit_transfers: &'a [TransitTransfer],
    signs: &'a [Sign],
    turn_lanes: &'a [TurnLane],
    admins: &'a [Admin],
    edge_bins: &'a [GraphId],
    /// One bounding circle per edge bin entry (or empty if the tile has none).
    bounding_circles: &'a [DiscretizedBoundingCircle],
    complex_forward_restrictions_memory: &'a [u8],
    complex_reverse_restrictions_memory: &'a [u8],
    // Variable length byte arrays (indexed into at various other points; hidden with public safe accessors)...
    // These are all slices which DO have a known size at runtime, but not at compile time.
    edge_info_memory: &'a [u8],
    text_memory: &'a [u8],
    lane_connectivity_memory: &'a [u8],
    predicted_speeds: Option<PredictedSpeeds<'a>>,
    // TODO: Stop one stops(?)
    // TODO: Route one stops(?)
    // TODO: Operator one stops(?)
}

impl GraphTile for GraphTileView<'_> {
    #[inline]
    fn graph_id(&self) -> GraphId {
        self.header.graph_id()
    }

    #[inline]
    fn may_contain_id(&self, id: GraphId) -> bool {
        id.tile_base_id() == self.graph_id().tile_base_id()
    }

    #[inline]
    fn header(&self) -> &GraphTileHeader {
        self.header
    }

    #[inline]
    fn get_node(&self, id: GraphId) -> Result<&NodeInfo, LookupError> {
        if self.may_contain_id(id) {
            self.nodes
                .get(id.feature_index() as usize)
                .ok_or(LookupError::InvalidIndex)
        } else {
            Err(LookupError::MismatchedBase)
        }
    }

    #[inline]
    fn get_outbound_edges_from_node(&self, node_info: &NodeInfo) -> &[DirectedEdge] {
        let start = node_info.edge_index() as usize;
        let count = usize::from(node_info.edge_count());
        // Parsing checked this range for every node in the tile,
        // so this only falls back to an empty slice for a node from another tile.
        self.directed_edges
            .get(start..start + count)
            .unwrap_or_default()
    }

    #[inline]
    fn get_transitions(&self, node: &NodeInfo) -> &[NodeTransition] {
        let start = node.transition_index() as usize;
        let count = usize::from(node.transition_count());
        // As above, parsing checked this range for every node in the tile.
        self.transitions
            .get(start..start + count)
            .unwrap_or_default()
    }

    #[inline]
    fn get_directed_edge(&self, id: GraphId) -> Result<&DirectedEdge, LookupError> {
        if self.may_contain_id(id) {
            self.directed_edges
                .get(id.feature_index() as usize)
                .ok_or(LookupError::InvalidIndex)
        } else {
            Err(LookupError::MismatchedBase)
        }
    }

    fn get_opp_edge_index(&self, graph_id: GraphId) -> Result<OpposingEdgeIndex, LookupError> {
        let edge = self.get_directed_edge(graph_id)?;

        // The edge might leave the tile, so we have to do a complicated lookup
        let end_node_id = edge.end_node_id();
        let opposing_edge_index = edge.opposing_edge_index();

        Ok(OpposingEdgeIndex {
            end_node_id,
            opposing_edge_index,
        })
    }

    fn get_ext_directed_edge(&self, id: GraphId) -> Result<&DirectedEdgeExt, LookupError> {
        if self.may_contain_id(id) {
            self.ext_directed_edges
                .get(id.feature_index() as usize)
                .ok_or(LookupError::InvalidIndex)
        } else {
            Err(LookupError::MismatchedBase)
        }
    }

    fn get_access_restrictions(
        &self,
        directed_edge_index: u32,
        access_modes: EnumSet<Access>,
    ) -> Vec<&AccessRestriction> {
        // Find the point such that all elements below index are less than directed_edge_index
        // (NOTE: the list is pre-sorted during tile building)
        let index = self
            .access_restrictions
            .partition_point(|e| e.edge_index() < directed_edge_index);

        self.access_restrictions
            .iter()
            // Iterate over the relevant objects (this is an empty iterator if index >= length)
            .skip(index)
            .take_while(|e| e.edge_index() == directed_edge_index)
            .filter(|e| !e.affected_access_modes().is_disjoint(access_modes))
            .collect()
    }

    fn get_predicted_speed(
        &self,
        directed_edge_index: usize,
        seconds_from_start_of_week: u32,
    ) -> Option<f32> {
        let predicted_speeds = self.predicted_speeds.as_ref()?;
        if self
            .directed_edges
            .get(directed_edge_index)?
            .has_predicted_speed()
        {
            predicted_speeds.speed(directed_edge_index, seconds_from_start_of_week)
        } else {
            None
        }
    }

    fn get_edge_info(
        &self,
        directed_edge: &DirectedEdge,
    ) -> Result<EdgeInfo<'_>, GraphTileDecodingError> {
        let offset = directed_edge.edge_info_offset() as usize;
        // Parsing checked this offset for every edge in the tile,
        // so this can only fail for an edge from another tile.
        let edge_info = self.edge_info_memory.get(offset..).ok_or(
            GraphTileDataError::EdgeInfoLookupOutOfBounds {
                offset,
                size: self.edge_info_memory.len(),
            },
        )?;

        EdgeInfo::try_from((edge_info, self.text_memory))
    }

    #[inline]
    fn directed_edges(&self) -> &[DirectedEdge] {
        self.directed_edges
    }

    #[inline]
    fn nodes(&self) -> &[NodeInfo] {
        self.nodes
    }

    #[inline]
    fn admins(&self) -> &[Admin] {
        self.admins
    }

    #[inline]
    fn edges_in_bin(&self, bin_index: usize) -> &[GraphId] {
        // Parsing checked that the bin offsets are monotonic
        // and that the last one is the length of the edge bins,
        // so this only falls back to an empty slice for an out-of-range bin.
        self.header
            .edge_bin_offsets(bin_index)
            .and_then(|(start, end)| self.edge_bins.get(start..end))
            .unwrap_or_default()
    }

    #[inline]
    fn bounding_circles_in_bin(&self, bin_index: usize) -> &[DiscretizedBoundingCircle] {
        // There is either one circle per edge bin entry or none at all.
        // As above, the range is in bounds whenever there are circles.
        self.header
            .edge_bin_offsets(bin_index)
            .and_then(|(start, end)| self.bounding_circles.get(start..end))
            .unwrap_or_default()
    }

    #[inline]
    fn bin_index_xy(&self, x: usize, y: usize) -> Option<usize> {
        (x < crate::BINS_PER_SIDE && y < crate::BINS_PER_SIDE).then(|| y * crate::BINS_PER_SIDE + x)
    }

    // Variable-length sections
    #[inline]
    fn sections(&self) -> TileSections<'_> {
        TileSections {
            signs: self.signs,
            turn_lanes: self.turn_lanes,
            admins: self.admins,
            complex_forward: self.complex_forward_restrictions_memory,
            complex_reverse: self.complex_reverse_restrictions_memory,
            text_list: self.text_memory,
            lane_connectivity: self.lane_connectivity_memory,
        }
    }
}

/// Reads the sections of a tile in order, consuming them from the front of the remaining bytes.
///
/// This is not *strictly* the same as how Valhalla parses the tile in `GraphTile::Initialize`
/// (which jumps to the header offsets of the variable-size sections),
/// but it is functionally identical for a valid tile,
/// and [`SectionReader::expect_offset`] checks that the two agree.
///
/// One helpful side effect of this approach is that we can be certain
/// that there are no accidentally overlapping borrows!
/// Additionally, at the end, we can check that we've consumed all bytes,
/// thus ensuring that we have either mapped every byte or else something is wrong.
struct SectionReader<'a> {
    remaining: &'a [u8],
    tile_len: usize,
}

impl<'a> SectionReader<'a> {
    /// Reads the next `count` elements of type `T`.
    #[inline]
    fn read<T>(
        &mut self,
        field: &'static str,
        count: usize,
    ) -> Result<&'a [T], GraphTileDecodingError>
    where
        T: FromBytes + Immutable,
    {
        let (section, remaining) = <[T]>::ref_from_prefix_with_elems(self.remaining, count)
            .map_err(|e| GraphTileDecodingError::CastError {
                field: field.to_string(),
                error_description: e.to_string(),
            })?;
        self.remaining = remaining;
        Ok(section)
    }

    /// Checks that the next section starts at the byte offset the header gives for it.
    ///
    /// Valhalla reads the bounding circles and everything after them from header offsets alone,
    /// so this catches tiles whose header disagrees with their contents.
    /// For example, `valhalla_add_landmarks` in Valhalla 3.9.1 drops the edge bins and circles
    /// from the tiles it rewrites but leaves their offsets in the header.
    #[inline]
    fn expect_offset(
        &self,
        section: &'static str,
        expected: usize,
    ) -> Result<(), GraphTileHeaderError> {
        let actual = self.tile_len - self.remaining.len();
        if actual == expected {
            Ok(())
        } else {
            Err(GraphTileHeaderError::SectionOffsetMismatch {
                section,
                expected,
                actual,
            })
        }
    }
}

impl<'a> TryFrom<&'a [u8]> for GraphTileView<'a> {
    type Error = GraphTileDecodingError;

    /// Parses and validates a graph tile.
    ///
    /// This checks everything that the view's accessors rely on,
    /// so that none of them can index out of bounds or panic on a parsed tile:
    ///
    /// - the header, with [`GraphTileHeader::validate`]
    ///   (the version, tile ID, tile size, and section offsets);
    /// - that every section fits in the tile, starts where the header says,
    ///   and that the sections cover the tile exactly;
    /// - and the references between sections (see [`GraphTileDataError`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the tile is truncated, corrupt, or from an unsupported version.
    ///
    /// # Performance
    ///
    /// This does not copy any tile data,
    /// but it does visit every node, directed edge, and access restriction once.
    fn try_from(bytes: &'a [u8]) -> Result<Self, Self::Error> {
        let tile_len = bytes.len();
        let (header, remaining) = GraphTileHeader::ref_from_prefix(bytes).map_err(|e| {
            GraphTileDecodingError::CastError {
                field: "header".to_string(),
                error_description: e.to_string(),
            }
        })?;
        header.validate(tile_len)?;

        let mut reader = SectionReader {
            remaining,
            tile_len,
        };

        // The fixed-size sections, whose sizes come from the header's record counts.
        // The counts are at most 24 bits wide, so none of these sizes can overflow.
        let nodes = reader.read::<NodeInfo>("node_info", header.node_count() as usize)?;
        let transitions =
            reader.read::<NodeTransition>("transitions", header.transition_count() as usize)?;
        let directed_edges =
            reader.read::<DirectedEdge>("directed_edges", header.directed_edge_count() as usize)?;
        let directed_edge_ext_count = if header.has_ext_directed_edge() {
            header.directed_edge_count() as usize
        } else {
            0
        };
        let ext_directed_edges =
            reader.read::<DirectedEdgeExt>("directed_edge_ext", directed_edge_ext_count)?;
        let access_restrictions = reader.read::<AccessRestriction>(
            "access_restrictions",
            header.access_restriction_count() as usize,
        )?;
        let transit_departures = reader
            .read::<TransitDeparture>("transit_departures", header.departure_count() as usize)?;
        let transit_stops =
            reader.read::<TransitStop>("transit_stops", usize::from(header.stop_count()))?;
        let transit_routes =
            reader.read::<TransitRoute>("transit_routes", usize::from(header.route_count()))?;
        let transit_schedules = reader
            .read::<TransitSchedule>("transit_schedules", usize::from(header.schedule_count()))?;
        let transit_transfers = reader
            .read::<TransitTransfer>("transit_transfers", usize::from(header.transfer_count()))?;
        let signs = reader.read::<Sign>("signs", header.sign_count() as usize)?;
        let turn_lanes =
            reader.read::<TurnLane>("turn_lanes", header.turn_lane_count() as usize)?;
        let admins = reader.read::<Admin>("admins", usize::from(header.admin_count()))?;
        let edge_bins = reader.read::<GraphId>("edge_bins", header.edge_bins_count())?;

        // The sections from here on have explicit offsets in the header.
        // Valhalla writes the bounding circles right after the edge bins.
        if let Some(offset) = header.bounding_circles_offset() {
            reader.expect_offset("bounding_circles", offset)?;
        }
        let bounding_circles = reader.read::<DiscretizedBoundingCircle>(
            "bounding_circles",
            header.bounding_circle_count(),
        )?;

        // The header has already checked that these offsets are monotonic,
        // so checking the first one places all of them.
        reader.expect_offset(
            "complex_forward_restrictions",
            header.complex_forward_restrictions_offset(),
        )?;
        let complex_forward_restrictions_memory = reader.read::<u8>(
            "complex_forward_restrictions",
            header.complex_forward_restrictions_size()?,
        )?;
        let complex_reverse_restrictions_memory = reader.read::<u8>(
            "complex_reverse_restrictions",
            header.complex_reverse_restrictions_size()?,
        )?;
        let edge_info_memory = reader.read::<u8>("edge_info_memory", header.edge_info_size()?)?;
        let text_memory = reader.read::<u8>("text_memory", header.text_list_size()?)?;
        let lane_connectivity_memory =
            reader.read::<u8>("lane_connectivity", header.lane_connectivity_size()?)?;

        let (predicted_speed_offsets, predicted_speed_profiles) =
            if let Some(offset) = header.predicted_speeds_offset() {
                reader.expect_offset("predicted_speeds", offset)?;
                // Curiously, these seem to actually be unaligned in a Valhalla tile I generated!
                let offsets = reader.read::<U32<LE>>(
                    "predicted_speed_offsets",
                    header.directed_edge_count() as usize,
                )?;
                let profiles = reader.read::<I16<LE>>(
                    "predicted_speed_profiles",
                    header.predicted_speeds_count() as usize * COEFFICIENT_COUNT,
                )?;
                (offsets, profiles)
            } else {
                (&[][..], &[][..])
            };

        // TODO: Transit info
        // - Stop one stops(?)
        // - Route one stops(?)
        // - Operator one stops(?)

        if !reader.remaining.is_empty() {
            return Err(GraphTileDecodingError::LeftoverBytesAfterReading(
                reader.remaining.len(),
            ));
        }

        validation::check_nodes(nodes, directed_edges.len(), transitions.len())?;
        validation::check_directed_edges(
            directed_edges,
            edge_info_memory.len(),
            predicted_speed_offsets,
            header.predicted_speeds_count() as usize,
        )?;
        validation::check_access_restrictions(access_restrictions)?;

        let predicted_speeds = header
            .predicted_speeds_offset()
            .map(|_| PredictedSpeeds::new(predicted_speed_offsets, predicted_speed_profiles));

        Ok(Self {
            header,
            nodes,
            transitions,
            directed_edges,
            ext_directed_edges,
            access_restrictions,
            transit_departures,
            transit_stops,
            transit_routes,
            transit_schedules,
            transit_transfers,
            signs,
            turn_lanes,
            admins,
            edge_bins,
            bounding_circles,
            complex_forward_restrictions_memory,
            complex_reverse_restrictions_memory,
            edge_info_memory,
            text_memory,
            lane_connectivity_memory,
            predicted_speeds,
        })
    }
}

/// A pair which can be used to get an opposing edge from a graph tile provider.
pub struct OpposingEdgeIndex {
    /// The ID of the end node.
    ///
    /// NOTE: This might be in another tile from the original edge!
    pub(crate) end_node_id: GraphId,
    /// The index of the opposing directed edge at the end node of this directed edge.
    pub(crate) opposing_edge_index: u32,
}

#[cfg(test)]
const TEST_GRAPH_TILE_ID_L0: GraphId = unsafe { GraphId::from_components_unchecked(0, 3015, 0) };
#[cfg(test)]
const TEST_GRAPH_TILE_ID_L2: GraphId = unsafe { GraphId::from_components_unchecked(2, 762485, 0) };

#[cfg(test)]
static TEST_GRAPH_TILE_L0: LazyLock<OwnedGraphTileHandle> = LazyLock::new(|| {
    let relative_path = TEST_GRAPH_TILE_ID_L0
        .file_path("gph")
        .expect("Unable to get relative path");
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("andorra-tiles")
        .join(relative_path);
    let bytes = std::fs::read(path).expect("Unable to read file");

    OwnedGraphTileHandle::try_from(bytes).expect("Unable to get tile")
});

#[cfg(test)]
static TEST_GRAPH_TILE_L2: LazyLock<OwnedGraphTileHandle> = LazyLock::new(|| {
    let relative_path = TEST_GRAPH_TILE_ID_L2
        .file_path("gph")
        .expect("Unable to get relative path");
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("andorra-tiles")
        .join(relative_path);
    let bytes = std::fs::read(path).expect("Unable to read file");

    OwnedGraphTileHandle::try_from(bytes).expect("Unable to get tile")
});

/// Test data note: this relies on a binary fixture
/// where speed info is added to the Andorra tiles using the `valhalla_add_predicted_traffic`
/// utility.
///
/// The contents of the file `0/003/015.csv` used to generate the test fixture are as follows:
///
/// ```csv
/// 0/3015/0,50,40,
/// 0/3015/42,100,42,BcUACQAEACEADP/7/9sAGf/fAAwAGwARAAX/+AAAAB0AFAAS//AAF//+ACwACQAqAAAAKP/6AEMABABsAAsBBAAq/rcAAP+bAAz/zv/s/9MABf/Y//X/8//9/+wACf/P//EADv/8//L//P/y//H////7AAwAFf/5//oADgAZAAQAFf/3/+8AB//yAB8ABv/0AAUAEf/8//QAFAAG//b////j//v//QAT//7/+f/kABMABwABAAv/6//8//cAEwAAABT/8v/6//wAAAAQ//cACwAFAAT/1//sAAEADAABAAYAE//9AAn/7gAH/+AAFQAB//4AC//o/+gAE//+AAAAFf/l//kABP/+//kACAAG//cAHv/qAB0AAv/4/+v/+wALAAMABP/3AAT/8wAIAAr/9wAK//j/+wAEAAD/+P/8//v/8f/2//L//AALAAcABgAG//gAAv/5AAoAHv//AAcAFf/zABD/7AAUAAv/7v/8AAgACAAN//0ADP/iABD/9f/3//7/+P/3AAQADP//AAMABw==
/// 0/3015/7,12,34,CKD/4f/r/+H/6//g/+v/4P/q/9//6v/f/+r/3v/q/93/6f/b/+n/2v/o/9f/5//V/+b/0f/l/8z/4//G/+D/vf/d/67/1/+V/8z/Xf+u/nz+GwLJAEcAqwAWAFwABwA8AAAAK//8ACD/+AAZ//YAE//0AA//8gAM//AACf/uAAb/7AAE/+kAAv/m////4v/9/93/+f/U//T/xf/q/5//y/6PAQMAngApADkAFwAfABAAEwAMAAwACQAHAAcAAwAGAAAABf/9AAT/+wAD//kAA//2AAL/9AAC//EAAf/tAAH/6AAA/+EAAP/U////tv///x8AFADL//8AQ///ACf//gAa//4AE//+AA7//QAL//0ACP/9AAb//AAE//wAAv/8AAD/+//+//v//P/6//r/+v/2//n/8v/4/+v/9f/b/+//nP+TALoADAAyAAMAHgAAABX//gAQ//0ADf/9AAv//AAJ//sAB//7AAb/+gAF//kABP/4AAP/9wAC//YAAf/1AAD/8//+//D//P/q//f/2w==
/// ```
#[cfg(test)]
static TEST_GRAPH_TILE_WITH_FLOW: LazyLock<OwnedGraphTileHandle> = LazyLock::new(|| {
    let relative_path = TEST_GRAPH_TILE_ID_L0
        .file_path("gph")
        .expect("Unable to get relative path");
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("andorra-tiles-with-traffic")
        .join(relative_path);
    let bytes = std::fs::read(path).expect("Unable to read file");

    OwnedGraphTileHandle::try_from(bytes).expect("Unable to get tile")
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Access;
    use crate::graph_tile::predicted_speeds::BUCKETS_PER_WEEK;
    use crate::graph_tile::{
        GraphTile, TEST_GRAPH_TILE_ID_L0, TEST_GRAPH_TILE_L0, TEST_GRAPH_TILE_L2,
        TEST_GRAPH_TILE_WITH_FLOW,
    };
    use enumset::{EnumSet, enum_set};
    use proptest::prelude::*;
    use rand::{RngExt, rng};
    use zerocopy::IntoBytes;

    #[test]
    fn test_get_opp_edge_index() {
        let tile = &*TEST_GRAPH_TILE_L0;

        let edges: Vec<_> = (0..u64::from(tile.header().directed_edge_count()))
            .flat_map(|index| {
                let edge_id = TEST_GRAPH_TILE_ID_L0
                    .with_feature_index(index)
                    .expect("Invalid graph ID.");
                let OpposingEdgeIndex {
                    end_node_id,
                    opposing_edge_index,
                } = tile
                    .get_opp_edge_index(edge_id)
                    .expect("Unable to get opp edge index.");

                // Currently the fixture has no edges exiting the tile, but we play it safe.
                if let Ok(node) = tile.get_node(end_node_id) {
                    // This is not the way you'd normally want to build these up;
                    // you should use GraphTileProvider's convenience methods almost all the time.
                    // This just makes the test independent and self-contained using only tile methods.
                    Some(node.edge_index() + opposing_edge_index)
                } else {
                    None
                }
            })
            .collect();

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!(edges);
        }
    }

    #[test]
    fn test_get_access_restrictions() {
        let tile = &*TEST_GRAPH_TILE_L0;

        let mut found_restriction_count = 0;
        let mut found_partial_subset_count = 0;
        for edge_index in 0..tile.header().directed_edge_count() {
            // Sanity check edge index with no access mode filter
            let all_edge_restrictions = tile.get_access_restrictions(edge_index, EnumSet::all());
            for restriction in &all_edge_restrictions {
                found_restriction_count += 1;
                assert_eq!(restriction.edge_index(), edge_index);
            }

            // In the tile fixture, all restrictions apply to trucks
            assert_eq!(
                all_edge_restrictions,
                tile.get_access_restrictions(edge_index, enum_set!(Access::Truck))
            );

            // Empty set should yield no access restrictions
            assert!(
                tile.get_access_restrictions(edge_index, EnumSet::empty())
                    .is_empty()
            );

            // This set includes one mode that matches every element in the fixture tile,
            // and another mode that matches none.
            for restriction in
                tile.get_access_restrictions(edge_index, enum_set!(Access::Auto | Access::Reserved))
            {
                found_partial_subset_count += 1;
                assert_eq!(restriction.edge_index(), edge_index);
            }
        }

        // Hard-coded bits based on the test fixture
        // (cross-checked with GraphTile::GetAccessRestrictions in Valhalla 3.9.1)
        assert_eq!(found_restriction_count, 20);
        assert_eq!(found_partial_subset_count, 20);
    }

    #[test]
    fn test_edge_bins() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();
        assert!(
            tile_view.edge_bins.is_empty(),
            "The level 0 tile has no edge bins"
        );

        let tile = &*TEST_GRAPH_TILE_L2;
        let tile_view = tile.borrow_dependent();
        assert!(
            !tile_view.edge_bins.is_empty(),
            "The level 2 tile should have edge bins"
        );

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            // Graph IDs serialize as their raw value,
            // but only with the serde feature, so we snapshot the raw values directly.
            let edge_bins: Vec<u64> = tile_view.edge_bins.iter().map(GraphId::value).collect();
            insta::assert_yaml_snapshot!("edge_bins", edge_bins);
        }
    }

    #[test]
    fn test_bin_accessors_integrity() {
        // Level without bins should yield empty slices across all bins
        let l0 = &*TEST_GRAPH_TILE_L0;
        let l0_view = l0.borrow_dependent();
        let mut total = 0usize;
        for i in 0..crate::BIN_COUNT {
            let bin = l0_view.edges_in_bin(i);
            assert!(bin.is_empty(), "L0 has no bins, bin {i} should be empty");
            total += bin.len();
        }
        assert_eq!(total, 0);

        // Level with bins should have slices that sum to the backing array size
        let l2 = &*TEST_GRAPH_TILE_L2;
        let l2_view = l2.borrow_dependent();
        let mut total = 0usize;
        for i in 0..crate::BIN_COUNT {
            total += l2_view.edges_in_bin(i).len();
        }
        assert_eq!(total, l2_view.edge_bins.len());

        // Bins that don't exist are empty, rather than a panic.
        for i in [crate::BIN_COUNT, crate::BIN_COUNT + 1, usize::MAX] {
            assert!(l2_view.edges_in_bin(i).is_empty());
            assert!(l2_view.bounding_circles_in_bin(i).is_empty());
        }

        // Bins are numbered in row-major order from the southwest corner,
        // as in Valhalla (`row * kBinsDim + col`).
        let n = crate::BINS_PER_SIDE;
        assert_eq!(n, 5);
        assert_eq!(l2_view.bin_index_xy(0, 0), Some(0));
        assert_eq!(l2_view.bin_index_xy(n - 1, 0), Some(4));
        assert_eq!(l2_view.bin_index_xy(0, n - 1), Some(20));
        assert_eq!(l2_view.bin_index_xy(n - 1, n - 1), Some(24));
        let mut seen = [false; crate::BIN_COUNT];
        for y in 0..n {
            for x in 0..n {
                let index = l0_view.bin_index_xy(x, y).expect("in range");
                assert!(!seen[index], "bin ({x}, {y})");
                seen[index] = true;
            }
        }
    }

    #[test]
    fn test_bin_index_xy_rejects_out_of_range_coordinates() {
        // Before this was fixed, (5, 0) aliased bin (0, 1).
        let tile = &*TEST_GRAPH_TILE_L2;
        assert_eq!(tile.bin_index_xy(5, 0), None);
        assert_eq!(tile.bin_index_xy(0, 5), None);
        assert_eq!(tile.bin_index_xy(usize::MAX, usize::MAX), None);
    }

    #[test]
    fn test_bounding_circles_absent_outside_level_2() {
        let tile = &*TEST_GRAPH_TILE_L0;
        assert!(!tile.header().has_bounding_circles());
        for i in 0..crate::BIN_COUNT {
            assert!(tile.bounding_circles_in_bin(i).is_empty());
        }
    }

    #[test]
    fn test_bounding_circles() {
        let tile = &*TEST_GRAPH_TILE_L2;
        let header = tile.header();
        assert!(header.has_bounding_circles());

        // Every edge bin entry has exactly one circle.
        let mut total = 0;
        for i in 0..crate::BIN_COUNT {
            let circles = tile.bounding_circles_in_bin(i);
            assert_eq!(circles.len(), tile.edges_in_bin(i).len(), "bin {i}");
            // Valhalla 3.9.1 was able to compute a circle for every edge in this tile.
            assert!(circles.iter().all(DiscretizedBoundingCircle::is_valid));
            total += circles.len();
        }
        assert_eq!(total, header.edge_bins_count());

        // The expected values below are printed by Valhalla 3.9.1 itself,
        // using GraphTile::GetBoundingCircles and DiscretizedBoundingCircle::get
        // with the bin center computed as in loki's search.
        // We expect bit-for-bit equality.
        let bin_center = header.bin_center(18).expect("valid bin");
        assert_eq!(
            bin_center,
            coord! {x: 1.425_000_011_920_929, y: 42.425_000_011_920_93}
        );
        let edges = tile.edges_in_bin(18);
        let circles = tile.bounding_circles_in_bin(18);
        assert_eq!(edges.len(), 5);
        assert_eq!(
            edges[0],
            TEST_GRAPH_TILE_ID_L2.with_feature_index(0).unwrap()
        );
        assert_eq!(
            (
                circles[0].x_offset(),
                circles[0].y_offset(),
                circles[0].radius_index()
            ),
            (5211, 4883, 60)
        );
        assert_eq!(
            circles[0].decode(bin_center),
            Some(BoundingCircle {
                center: coord! {x: 1.442_567_769_282_198_3, y: 42.434_154_802_719_654},
                radius_meters: 1000.0,
            })
        );
        assert_eq!(
            edges[2],
            TEST_GRAPH_TILE_ID_L2.with_feature_index(21).unwrap()
        );
        assert_eq!(
            circles[2].decode(bin_center),
            Some(BoundingCircle {
                center: coord! {x: 1.445_749_021_623_728, y: 42.442_943_401_886_424},
                radius_meters: 300.0,
            })
        );

        let bin_center = header.bin_center(24).expect("valid bin");
        assert_eq!(
            bin_center,
            coord! {x: 1.475_000_008_940_696_7, y: 42.475_000_008_940_7}
        );
        let edges = tile.edges_in_bin(24);
        let circles = tile.bounding_circles_in_bin(24);
        assert_eq!(edges.len(), 1580);
        assert_eq!(
            edges[0],
            TEST_GRAPH_TILE_ID_L2.with_feature_index(83).unwrap()
        );
        assert_eq!(
            (
                circles[0].x_offset(),
                circles[0].y_offset(),
                circles[0].radius_index()
            ),
            (3555, 2046, 6)
        );
        assert_eq!(
            circles[0].decode(bin_center),
            Some(BoundingCircle {
                center: coord! {x: 1.466_480_999_584_143_7, y: 42.451_174_302_601_67},
                radius_meters: 7.0,
            })
        );
    }

    #[test]
    fn test_bin_centers() {
        let header = TEST_GRAPH_TILE_L2.header();
        // Valhalla adds single precision offsets (0.05f increments) to the double precision corner.
        assert_eq!(
            header.bin_center(0),
            Some(coord! {x: 1.275_000_000_372_529, y: 42.275_000_000_372_53})
        );
        assert_eq!(
            header.bin_center(2),
            Some(coord! {x: 1.375, y: 42.275_000_000_372_53})
        );
        assert_eq!(
            header.bin_center(13),
            Some(coord! {x: 1.425_000_011_920_929, y: 42.375})
        );
        assert_eq!(header.bin_center(crate::BIN_COUNT), None);
    }

    #[test]
    fn test_rejects_bounding_circles_at_wrong_offset() {
        // valhalla_add_landmarks in Valhalla 3.9.1 drops the edge bins and bounding circles
        // but keeps their offsets in the header.
        // Simulate a tile whose header says the circles start somewhere else.
        let tile = &*TEST_GRAPH_TILE_L2;
        let mut bytes = tile.borrow_owner().clone();
        let offset = tile
            .header()
            .bounding_circles_offset()
            .expect("has circles");
        let wrong = u32::try_from(offset + 4).unwrap();
        // The bounding circle offset is the 58th 32-bit word of the header.
        bytes[228..232].copy_from_slice(&wrong.to_le_bytes());
        assert!(matches!(
            GraphTileView::try_from(bytes.as_slice()),
            Err(GraphTileDecodingError::InvalidHeader(GraphTileHeaderError::SectionOffsetMismatch {
                section: "bounding_circles",
                expected,
                actual,
            })) if expected == offset + 4 && actual == offset
        ));
    }

    /// Reproduces the tiles that `valhalla_add_landmarks` writes in Valhalla 3.9.1.
    ///
    /// It rewrites tiles with `GraphTileBuilder::StoreTileData`,
    /// which drops the edge bins and bounding circles,
    /// shifts the later section offsets down to match,
    /// but keeps the header's bin offsets and bounding circle offset.
    /// Mainline then reads the start of the complex restrictions and edge info as edge bins.
    /// We must refuse such a tile instead.
    ///
    /// We build the reproduction from a good fixture tile instead of committing a corrupted one
    /// (a real one is 1.7 MB).
    /// A real corrupted level 2 tile from such a build fails in the same way.
    #[test]
    fn test_rejects_tile_with_dropped_edge_bins() {
        let tile = &*TEST_GRAPH_TILE_L2;
        let header = tile.header();
        let circles_start = header.bounding_circles_offset().expect("has circles");
        let bins_start = circles_start - header.edge_bins_count() * size_of::<GraphId>();
        let circles_end =
            circles_start + header.bounding_circle_count() * size_of::<DiscretizedBoundingCircle>();
        assert_eq!(circles_end, header.complex_forward_restrictions_offset());
        let removed = circles_end - bins_start;

        let original = tile.borrow_owner();
        let mut bytes = original[..bins_start].to_vec();
        bytes.extend_from_slice(&original[circles_end..]);
        // Shift complex restrictions (forward and reverse), edge info, text list,
        // lane connectivity, and the tile size down.
        // The predicted speed offset is zero (this tile has none) and stays that way.
        assert_eq!(header.predicted_speeds_count(), 0);
        for field_offset in [96, 100, 104, 108, 216, 224] {
            let mut word = [0u8; 4];
            word.copy_from_slice(&bytes[field_offset..field_offset + 4]);
            let shifted = u32::from_le_bytes(word) - u32::try_from(removed).unwrap();
            bytes[field_offset..field_offset + 4].copy_from_slice(&shifted.to_le_bytes());
        }

        // The edge bins still line up with the stale circle offset,
        // so the mismatch shows up where the complex restrictions should start.
        assert!(matches!(
            GraphTileView::try_from(bytes.as_slice()),
            Err(GraphTileDecodingError::InvalidHeader(GraphTileHeaderError::SectionOffsetMismatch {
                section: "complex_forward_restrictions",
                expected,
                actual,
            })) if expected == bins_start && actual == circles_end
        ));
    }

    #[test]
    fn test_rejects_missing_edge_bins() {
        // The level 0 tile has no edge bins.
        // Claim four entries in the last bin (so the offsets stay monotonic).
        // The tile must be rejected rather than read with every later section shifted.
        let tile = &*TEST_GRAPH_TILE_L0;
        let mut bytes = tile.borrow_owner().clone();
        // The last bin offset is at byte 116 + 24 * 4.
        bytes[212..216].copy_from_slice(&4u32.to_le_bytes());
        assert!(matches!(
            GraphTileView::try_from(bytes.as_slice()),
            Err(GraphTileDecodingError::InvalidHeader(
                GraphTileHeaderError::SectionOffsetMismatch {
                    section: "complex_forward_restrictions",
                    ..
                }
            ))
        ));
    }

    #[test]
    fn test_edge_info() {
        let tile = &*TEST_GRAPH_TILE_L0;

        let first_edge_info = tile
            .get_edge_info(&tile.directed_edges().first().unwrap())
            .expect("Unable to get edge info.");

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!("first_edge_info", first_edge_info);
            insta::assert_debug_snapshot!(
                "first_edge_info_decoded_shape",
                first_edge_info.decode_raw_shape::<f64>()
            );
            insta::assert_debug_snapshot!(
                "first_edge_info_decoded_shape_f32",
                first_edge_info.decode_raw_shape::<f32>()
            );
            insta::assert_debug_snapshot!("first_edge_info_names", first_edge_info.get_names());
        }

        assert_eq!(first_edge_info.way_id(), 0);

        let last_edge_info = tile
            .get_edge_info(tile.directed_edges().last().unwrap())
            .expect("Unable to get edge info.");

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!("last_edge_info", last_edge_info);
        }

        assert_eq!(last_edge_info.way_id(), 762_680_030);

        // This edge has two different street names (checked with EdgeInfo::GetNames in Valhalla 3.9.1).
        let other_edge_info = tile
            .get_edge_info(&tile.directed_edges()[52])
            .expect("Unable to get edge info.");

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!("other_edge_info", other_edge_info);
            insta::assert_debug_snapshot!("other_edge_info_names", other_edge_info.get_names());
        }

        assert_eq!(other_edge_info.way_id(), 704_181_817);
    }

    #[test]
    fn test_predicted_speed_access_when_absent_in_tile() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();

        // The tile was built without speed information
        assert_eq!(tile_view.get_predicted_speed(0, 0), None);

        // The index is clearly invalid
        assert_eq!(tile_view.get_predicted_speed(usize::MAX, 0), None);
    }

    #[test]
    fn test_traffic_flow_speeds() {
        let tile = &*TEST_GRAPH_TILE_WITH_FLOW;
        let tile_view = tile.borrow_dependent();

        const EDGE_INDEX: usize = 0;
        let edge = &tile_view.directed_edges()[EDGE_INDEX];

        assert_eq!(edge.free_flow_speed(), 50);
        assert_eq!(edge.constrained_flow_speed(), 40);
        assert_eq!(edge.has_predicted_speed(), false);

        // This edge doesn't have any predicted speeds.
        assert_eq!(tile_view.get_predicted_speed(EDGE_INDEX, 0), None);
    }

    #[test]
    fn test_predicted_traffic_speeds() {
        let tile = &*TEST_GRAPH_TILE_WITH_FLOW;
        let tile_view = tile.borrow_dependent();

        // We specifically added predicted speeds to this random edge index.
        // See the test case info in the comment for TEST_GRAPH_TILE_WITH_FLOW.
        const EDGE_INDEX: usize = 42;
        let edge = &tile_view.directed_edges()[EDGE_INDEX];

        assert_eq!(edge.free_flow_speed(), 100);
        assert_eq!(edge.constrained_flow_speed(), 42);
        assert_eq!(edge.has_predicted_speed(), true);

        // A few predicted speed samples.
        // See predicted_speeds.rs for the array of speeds that we expected to encode.
        assert!(
            tile_view
                .get_predicted_speed(EDGE_INDEX, 0)
                .is_some_and(|s| { (s - 36f32).abs() < 0.5 })
        );

        // 60 * 5 = number of seconds in a 5 minute block. We encode data in 5 min chunks.
        assert!(
            tile_view
                .get_predicted_speed(EDGE_INDEX, 42 * 60 * 5)
                .is_some_and(|s| { (s - 45f32).abs() < 0.5 })
        );

        assert!(
            tile_view
                .get_predicted_speed(EDGE_INDEX, 20 * 60 * 5)
                .is_some_and(|s| { (s - 42f32).abs() < 0.5 })
        );

        // There are 2016 total buckets per week; this indexes into the last one (zero indexed)
        assert!(
            tile_view
                .get_predicted_speed(EDGE_INDEX, 2016 * 60 * 5 - 1)
                .is_some_and(|s| { (s - 29f32).abs() < 0.5 })
        );
    }

    #[test]
    fn test_predicted_traffic_speeds_second_profile() {
        let tile = &*TEST_GRAPH_TILE_WITH_FLOW;
        let tile_view = tile.borrow_dependent();

        // We specifically added predicted speeds to another random edge index.
        // See the test case info in the comment for TEST_GRAPH_TILE_WITH_FLOW.
        const EDGE_INDEX: usize = 7;
        let edge = &tile_view.directed_edges()[EDGE_INDEX];

        assert_eq!(edge.free_flow_speed(), 12);
        // No, this isn't intended to make sense; it's test data ;)
        assert_eq!(edge.constrained_flow_speed(), 34);
        assert_eq!(edge.has_predicted_speed(), true);

        // Reconstruct the entire array of speeds.
        let speeds: [i64; BUCKETS_PER_WEEK] = std::array::from_fn(|i| {
            tile_view
                .get_predicted_speed(EDGE_INDEX, (i as u32) * 300)
                .unwrap() as i64
        });

        // Laying it all out, this highlights that the encoding is indeed quite.... lossy ;)
        // This was originally a series going from 0 to 100 in a loop, sawtooth fashion.
        // DCT is optimized for high compressibility of datasets which don't vary so wildly.
        // As one would expect for datasets like speed variance over time.
        assert_eq!(
            speeds,
            [
                0, 0, 0, 1, 3, 4, 6, 8, 9, 11, 12, 13, 13, 14, 14, 14, 15, 15, 15, 16, 17, 18, 20,
                21, 23, 25, 27, 28, 30, 31, 32, 33, 33, 34, 34, 34, 34, 35, 35, 36, 37, 38, 40, 41,
                43, 45, 47, 49, 50, 52, 53, 53, 54, 54, 54, 54, 54, 54, 54, 55, 56, 57, 59, 61, 63,
                66, 68, 70, 72, 73, 74, 74, 75, 74, 74, 73, 72, 72, 72, 72, 74, 75, 78, 81, 84, 87,
                91, 94, 97, 99, 100, 99, 98, 94, 90, 84, 77, 69, 61, 52, 43, 35, 27, 20, 14, 9, 5,
                3, 1, 1, 2, 4, 6, 8, 11, 13, 16, 18, 20, 22, 23, 24, 24, 25, 25, 25, 26, 26, 27,
                27, 28, 29, 30, 31, 32, 34, 35, 36, 37, 39, 40, 41, 42, 44, 45, 46, 47, 48, 48, 49,
                50, 51, 51, 52, 52, 53, 54, 55, 56, 57, 58, 60, 61, 63, 64, 66, 68, 69, 70, 71, 72,
                72, 72, 73, 72, 72, 72, 73, 73, 74, 75, 77, 80, 82, 85, 88, 91, 94, 96, 98, 98, 98,
                96, 93, 89, 84, 77, 70, 62, 53, 45, 36, 28, 21, 15, 9, 5, 2, 1, 0, 1, 2, 4, 7, 10,
                13, 16, 18, 21, 23, 24, 25, 26, 26, 26, 26, 26, 26, 26, 26, 27, 28, 29, 30, 32, 33,
                35, 36, 38, 39, 41, 42, 43, 44, 45, 46, 47, 47, 48, 48, 49, 50, 50, 51, 52, 53, 54,
                55, 56, 58, 59, 61, 62, 64, 65, 67, 68, 69, 70, 71, 71, 72, 72, 72, 72, 72, 72, 73,
                74, 75, 76, 78, 80, 83, 86, 88, 91, 94, 96, 97, 97, 97, 95, 92, 88, 83, 77, 70, 62,
                54, 45, 37, 29, 22, 15, 10, 5, 2, 0, 0, 0, 2, 4, 6, 9, 13, 16, 19, 21, 23, 25, 26,
                26, 27, 27, 26, 26, 26, 26, 26, 26, 27, 28, 29, 31, 33, 34, 36, 38, 40, 42, 43, 44,
                45, 46, 47, 47, 47, 48, 48, 48, 49, 50, 50, 51, 52, 54, 55, 56, 58, 60, 61, 63, 64,
                66, 67, 68, 69, 70, 70, 71, 71, 71, 71, 71, 72, 72, 73, 74, 75, 77, 79, 81, 83, 86,
                89, 91, 94, 95, 97, 97, 96, 95, 92, 88, 83, 77, 70, 62, 54, 46, 37, 30, 22, 16, 10,
                5, 2, 0, 0, 0, 1, 3, 6, 9, 12, 15, 18, 21, 23, 25, 26, 27, 27, 27, 27, 26, 26, 26,
                26, 26, 26, 27, 29, 30, 32, 34, 36, 38, 40, 42, 43, 45, 46, 46, 47, 47, 47, 48, 48,
                48, 48, 49, 50, 51, 52, 53, 55, 56, 58, 60, 62, 63, 65, 66, 68, 68, 69, 70, 70, 70,
                70, 70, 70, 71, 71, 71, 72, 73, 75, 77, 79, 81, 84, 87, 89, 92, 94, 96, 96, 97, 96,
                94, 91, 87, 82, 76, 69, 62, 54, 46, 38, 30, 23, 16, 10, 6, 3, 0, 0, 0, 1, 3, 5, 8,
                12, 15, 18, 21, 23, 25, 26, 27, 27, 27, 27, 27, 26, 26, 26, 26, 26, 27, 28, 30, 32,
                34, 36, 38, 40, 42, 44, 45, 46, 47, 47, 48, 48, 48, 48, 48, 48, 49, 49, 50, 52, 53,
                55, 56, 58, 60, 62, 64, 65, 67, 68, 69, 69, 70, 70, 70, 70, 70, 70, 70, 71, 71, 72,
                73, 75, 77, 79, 82, 84, 87, 90, 92, 94, 96, 96, 96, 96, 94, 91, 87, 82, 76, 69, 62,
                54, 46, 38, 30, 23, 16, 11, 6, 3, 1, 0, 0, 0, 2, 5, 8, 11, 15, 18, 21, 23, 25, 27,
                28, 28, 28, 28, 27, 27, 26, 26, 26, 26, 27, 28, 30, 31, 33, 36, 38, 40, 42, 44, 45,
                46, 47, 48, 48, 48, 48, 48, 48, 48, 49, 49, 50, 51, 53, 54, 56, 58, 60, 62, 64, 66,
                67, 68, 69, 70, 70, 70, 70, 70, 70, 70, 70, 71, 71, 72, 73, 75, 77, 79, 82, 85, 87,
                90, 92, 94, 96, 96, 96, 95, 93, 90, 86, 81, 76, 69, 62, 54, 46, 38, 30, 23, 17, 11,
                7, 3, 1, 0, 0, 0, 2, 5, 8, 11, 14, 17, 20, 23, 25, 27, 28, 28, 28, 28, 27, 27, 26,
                26, 26, 26, 27, 28, 29, 31, 33, 35, 38, 40, 42, 44, 45, 47, 48, 48, 48, 48, 48, 48,
                48, 48, 48, 49, 50, 51, 52, 54, 56, 58, 60, 62, 64, 66, 67, 68, 69, 70, 70, 70, 70,
                70, 70, 70, 70, 70, 71, 71, 73, 75, 77, 79, 82, 85, 87, 90, 93, 95, 96, 97, 97, 96,
                93, 90, 86, 81, 75, 69, 61, 54, 46, 38, 31, 23, 17, 11, 7, 3, 1, 0, 0, 0, 2, 4, 7,
                11, 14, 17, 20, 23, 25, 27, 28, 28, 28, 28, 28, 27, 27, 26, 26, 26, 27, 28, 29, 31,
                33, 35, 37, 40, 42, 44, 45, 47, 48, 48, 49, 49, 49, 48, 48, 48, 48, 49, 49, 51, 52,
                54, 55, 58, 60, 62, 64, 66, 67, 69, 70, 70, 71, 71, 71, 70, 70, 70, 70, 70, 70, 71,
                73, 74, 77, 79, 82, 85, 88, 90, 93, 95, 96, 97, 97, 96, 94, 90, 86, 81, 75, 68, 61,
                54, 46, 38, 31, 24, 17, 12, 7, 4, 1, 0, 0, 1, 2, 5, 7, 10, 14, 17, 20, 22, 25, 26,
                27, 28, 28, 28, 28, 27, 27, 26, 26, 26, 27, 28, 29, 31, 32, 35, 37, 39, 41, 44, 45,
                47, 48, 48, 49, 49, 49, 49, 48, 48, 48, 49, 49, 50, 52, 53, 55, 57, 59, 62, 64, 66,
                68, 69, 70, 71, 71, 71, 71, 70, 70, 70, 69, 69, 70, 71, 72, 74, 76, 79, 82, 85, 88,
                91, 93, 95, 97, 97, 97, 96, 94, 90, 86, 81, 75, 68, 61, 53, 45, 38, 30, 23, 17, 12,
                7, 4, 1, 0, 0, 1, 2, 5, 7, 10, 14, 17, 20, 22, 25, 26, 28, 28, 29, 29, 28, 28, 27,
                27, 26, 26, 27, 28, 29, 30, 32, 34, 37, 39, 41, 43, 45, 47, 48, 49, 49, 49, 49, 49,
                49, 49, 49, 49, 49, 50, 52, 53, 55, 57, 59, 62, 64, 66, 68, 69, 70, 71, 71, 71, 71,
                71, 70, 70, 70, 70, 70, 71, 72, 74, 76, 79, 82, 85, 88, 91, 93, 95, 97, 98, 97, 96,
                94, 91, 86, 81, 75, 68, 61, 53, 45, 37, 30, 23, 17, 12, 7, 4, 2, 0, 0, 1, 2, 5, 7,
                10, 13, 17, 20, 22, 24, 26, 28, 28, 29, 29, 28, 28, 27, 27, 27, 27, 27, 28, 29, 31,
                32, 34, 37, 39, 41, 43, 45, 47, 48, 49, 49, 49, 49, 49, 49, 49, 49, 49, 49, 50, 51,
                53, 55, 57, 59, 61, 64, 66, 67, 69, 70, 71, 71, 71, 71, 71, 70, 70, 70, 70, 70, 71,
                72, 74, 76, 79, 81, 85, 88, 91, 93, 96, 97, 98, 98, 96, 94, 91, 86, 81, 75, 68, 60,
                53, 45, 37, 30, 23, 17, 12, 7, 4, 2, 1, 1, 1, 3, 5, 7, 10, 13, 16, 19, 22, 24, 26,
                27, 28, 28, 28, 28, 28, 27, 27, 27, 27, 27, 28, 29, 31, 32, 34, 37, 39, 41, 43, 45,
                46, 48, 49, 49, 49, 49, 49, 49, 49, 49, 49, 50, 50, 51, 53, 55, 57, 59, 61, 64, 66,
                68, 69, 70, 71, 72, 72, 71, 71, 70, 70, 70, 69, 70, 70, 72, 73, 76, 78, 81, 84, 88,
                91, 94, 96, 97, 98, 98, 97, 94, 91, 86, 81, 75, 68, 60, 53, 45, 37, 30, 23, 17, 12,
                7, 4, 2, 1, 1, 1, 3, 5, 7, 10, 13, 16, 19, 22, 24, 26, 27, 28, 28, 28, 28, 28, 28,
                27, 27, 27, 27, 28, 29, 30, 32, 34, 36, 38, 41, 43, 45, 46, 48, 49, 49, 50, 50, 50,
                49, 49, 49, 49, 50, 50, 51, 53, 54, 56, 59, 61, 63, 65, 67, 69, 70, 71, 72, 72, 72,
                71, 71, 70, 70, 70, 70, 71, 72, 73, 76, 78, 81, 84, 88, 91, 93, 96, 97, 98, 98, 97,
                94, 91, 86, 81, 75, 68, 60, 52, 44, 37, 29, 23, 17, 12, 7, 4, 2, 1, 1, 2, 3, 5, 8,
                11, 13, 16, 19, 22, 24, 26, 27, 28, 28, 28, 28, 28, 28, 27, 27, 27, 28, 28, 29, 31,
                32, 34, 36, 38, 41, 43, 45, 46, 48, 49, 49, 50, 50, 50, 50, 49, 49, 49, 50, 50, 51,
                53, 54, 56, 58, 61, 63, 65, 67, 69, 70, 71, 72, 72, 72, 71, 71, 70, 70, 70, 70, 70,
                71, 73, 75, 78, 81, 84, 87, 90, 93, 96, 97, 98, 98, 97, 95, 91, 87, 81, 75, 68, 60,
                52, 44, 37, 29, 23, 17, 12, 7, 4, 2, 1, 1, 2, 3, 5, 8, 11, 13, 16, 19, 21, 23, 25,
                27, 27, 28, 28, 28, 28, 28, 28, 28, 28, 28, 29, 30, 31, 32, 34, 36, 38, 40, 42, 44,
                46, 47, 48, 49, 49, 50, 50, 50, 50, 50, 50, 50, 51, 52, 53, 54, 56, 58, 61, 63, 65,
                67, 69, 70, 71, 72, 72, 72, 72, 71, 71, 70, 70, 70, 71, 72, 73, 75, 78, 81, 84, 87,
                90, 93, 96, 97, 98, 98, 97, 95, 91, 87, 81, 75, 68, 60, 52, 44, 36, 29, 22, 16, 11,
                7, 4, 2, 1, 1, 2, 4, 6, 8, 11, 13, 16, 19, 21, 23, 25, 26, 27, 28, 28, 28, 28, 28,
                28, 28, 28, 28, 29, 30, 31, 32, 34, 36, 38, 40, 42, 44, 45, 47, 48, 49, 49, 50, 50,
                50, 50, 50, 50, 50, 51, 52, 53, 54, 56, 58, 60, 63, 65, 67, 69, 70, 71, 72, 72, 72,
                72, 71, 71, 70, 70, 70, 70, 71, 73, 75, 77, 80, 83, 87, 90, 93, 96, 97, 99, 99, 97,
                95, 92, 87, 82, 75, 68, 60, 52, 44, 36, 29, 22, 16, 11, 7, 4, 2, 1, 2, 2, 4, 6, 8,
                11, 14, 16, 19, 21, 23, 25, 26, 27, 27, 28, 28, 28, 28, 28, 28, 28, 28, 29, 30, 31,
                33, 34, 36, 38, 40, 42, 44, 45, 47, 48, 49, 49, 50, 50, 50, 50, 50, 50, 51, 51, 52,
                53, 54, 56, 58, 60, 62, 64, 66, 68, 70, 71, 72, 72, 72, 72, 72, 71, 71, 70, 70, 71,
                72, 73, 75, 77, 80, 83, 86, 90, 93, 95, 97, 98, 99, 98, 95, 92, 88, 82, 75, 68, 60,
                52, 44, 36, 29, 22, 16, 11, 7, 4, 2, 2, 2, 3, 4, 6, 9, 11, 14, 17, 19, 21, 23, 25,
                26, 26, 27, 27, 27, 28, 28, 28, 28, 28, 29, 29, 30, 31, 33, 34, 36, 38, 40, 42, 43,
                45, 46, 48, 49, 49, 50, 50, 50, 50, 50, 51, 51, 51, 52, 53, 54, 56, 58, 60, 62, 64,
                66, 68, 70, 71, 72, 72, 73, 73, 72, 72, 71, 71, 71, 71, 72, 73, 75, 77, 80, 83, 86,
                89, 93, 95, 97, 99, 99, 98, 96, 93, 88, 82, 76, 68, 60, 52, 44, 36, 28, 21, 15, 10,
                6, 4, 2, 2, 2, 3, 5, 7, 9, 12, 14, 17, 19, 21, 23, 24, 25, 26, 26, 27, 27, 27, 28,
                28, 28, 29, 29, 30, 31, 32, 33, 35, 36, 38, 39, 41, 43, 44, 46, 47, 48, 49, 50, 50,
                51, 51, 51, 51, 52, 52, 53, 53, 54, 55, 57, 59, 61, 63, 65, 67, 69, 71, 72, 73, 73,
                73, 73, 73, 72, 72, 71, 71, 71, 72, 73, 75, 78, 81, 85, 88, 92, 95, 98, 100, 101,
                100, 98, 95, 90, 83, 76, 68, 59, 50, 41, 33, 25, 19, 13, 9, 6, 5, 4, 5, 6, 8, 9,
                11, 12, 12
            ]
        )
    }

    #[test]
    fn test_get_nodes_within_radius() {
        let mut rng = rng();
        let tile = &*TEST_GRAPH_TILE_WITH_FLOW;
        let tile_view = tile.borrow_dependent();
        let sw = tile_view.header().sw_corner();

        for (idx, node) in tile_view.nodes().into_iter().enumerate().filter(|_| {
            if cfg!(miri) {
                rng.random_bool(0.1)
            } else {
                true
            }
        }) {
            assert!(
                tile_view
                    .nodes_within_radius(|_| true, node.coordinate(sw).into(), 25.0)
                    .any(|(result_node, distance)| {
                        result_node.node_id
                            == tile
                                .header()
                                .graph_id()
                                .with_feature_index(idx as u64)
                                .unwrap()
                            && distance == 0.0
                    }),
                "Expected to find a match for the node"
            );
            assert!(
                tile_view
                    .nodes_within_radius(|_| true, node.coordinate(sw).into(), 25.0)
                    .all(|(_, distance)| { distance <= 25.0 }),
                "Expected all nodes to be within the search radius"
            );
        }
    }

    /// Reads every fixture tile, with its path relative to the fixtures directory.
    fn fixture_tiles() -> Vec<(String, Vec<u8>)> {
        let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let mut tiles: Vec<_> = ["andorra-tiles", "andorra-tiles-with-traffic"]
            .into_iter()
            .flat_map(|dir| walkdir::WalkDir::new(fixtures.join(dir)))
            .map(|entry| entry.expect("readable fixture directory"))
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "gph"))
            .map(|entry| {
                let name = entry
                    .path()
                    .strip_prefix(&fixtures)
                    .expect("under the fixtures directory")
                    .display()
                    .to_string();
                (name, std::fs::read(entry.path()).expect("readable fixture"))
            })
            .collect();
        tiles.sort();
        tiles
    }

    /// Overwrites `width` bits at `shift` in the little-endian 64-bit word at `offset`.
    fn patch_bits(bytes: &mut [u8], offset: usize, shift: u32, width: u32, value: u64) {
        let mut word = [0u8; 8];
        word.copy_from_slice(&bytes[offset..offset + 8]);
        let mask = ((1u64 << width) - 1) << shift;
        let word = (u64::from_le_bytes(word) & !mask) | ((value << shift) & mask);
        bytes[offset..offset + 8].copy_from_slice(&word.to_le_bytes());
    }

    /// The byte offset of a node in a tile.
    fn node_offset(index: usize) -> usize {
        size_of::<GraphTileHeader>() + index * size_of::<NodeInfo>()
    }

    /// The byte offset of a directed edge in a tile.
    fn directed_edge_offset(header: &GraphTileHeader, index: usize) -> usize {
        node_offset(header.node_count() as usize)
            + header.transition_count() as usize * size_of::<NodeTransition>()
            + index * size_of::<DirectedEdge>()
    }

    #[test]
    fn test_all_fixture_tiles_parse() {
        let tiles = fixture_tiles();
        // Seven tiles in the plain fixture and one with predicted speeds.
        assert_eq!(tiles.len(), 8);
        for (name, bytes) in &tiles {
            let view = GraphTileView::try_from(bytes.as_slice())
                .unwrap_or_else(|e| panic!("{name} should parse: {e}"));
            let header = view.header();
            // Cross-checked with GraphTileHeader.version and .date_created
            // in the Valhalla 3.9.1 Python bindings.
            assert_eq!(header.version(), "3.9.1", "{name}");
            assert!(header.has_supported_version());
            assert_eq!(header.date_created(), None, "{name}");
            exercise_accessors(&view);
        }
    }

    #[test]
    fn test_rejects_unsupported_version() {
        let mut bytes = TEST_GRAPH_TILE_L0.borrow_owner().clone();
        // The version is the 16 bytes after the graph ID and base coordinate.
        bytes[16..32].copy_from_slice(b"3.5.0\0\0\0\0\0\0\0\0\0\0\0");
        assert!(matches!(
            GraphTileView::try_from(bytes.as_slice()),
            Err(GraphTileDecodingError::InvalidHeader(
                GraphTileHeaderError::UnsupportedVersion { version }
            )) if version == "3.5.0"
        ));
    }

    #[test]
    fn test_rejects_truncated_tile() {
        let bytes = TEST_GRAPH_TILE_L0.borrow_owner();
        assert!(matches!(
            GraphTileView::try_from(&bytes[..bytes.len() - 1]),
            Err(GraphTileDecodingError::InvalidHeader(
                GraphTileHeaderError::TileSizeMismatch { header, actual }
            )) if header == bytes.len() && actual == bytes.len() - 1
        ));
        assert!(matches!(
            GraphTileView::try_from(&bytes[..size_of::<GraphTileHeader>() - 1]),
            Err(GraphTileDecodingError::CastError { field, .. }) if field == "header"
        ));
        assert!(GraphTileView::try_from(&[][..]).is_err());
    }

    #[test]
    fn test_rejects_node_edges_out_of_bounds() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let edge_count = tile.header().directed_edge_count() as usize;
        let mut bytes = tile.borrow_owner().clone();
        // The edge index is the low 21 bits of the node's second word,
        // and the edge count is the next 7 bits.
        let second_word = node_offset(3) + 8;
        patch_bits(&mut bytes, second_word, 0, 21, (edge_count - 1) as u64);
        patch_bits(&mut bytes, second_word, 21, 7, 2);
        assert_eq!(
            GraphTileView::try_from(bytes.as_slice())
                .err()
                .map(|e| e.to_string()),
            Some(
                GraphTileDataError::NodeEdgesOutOfBounds {
                    node: 3,
                    start: edge_count - 1,
                    end: edge_count + 1,
                    edge_count,
                }
                .to_string()
            )
        );
    }

    #[test]
    fn test_rejects_node_transitions_out_of_bounds() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let transition_count = tile.header().transition_count() as usize;
        let mut bytes = tile.borrow_owner().clone();
        // The transition index is the low 21 bits of the node's third word,
        // and the transition count is the next 3 bits.
        let third_word = node_offset(0) + 16;
        patch_bits(&mut bytes, third_word, 0, 21, transition_count as u64);
        patch_bits(&mut bytes, third_word, 21, 3, 1);
        assert!(matches!(
            GraphTileView::try_from(bytes.as_slice()),
            Err(GraphTileDecodingError::InvalidData(
                GraphTileDataError::NodeTransitionsOutOfBounds { node: 0, start, end, transition_count: count }
            )) if start == transition_count && end == transition_count + 1 && count == transition_count
        ));
    }

    #[test]
    fn test_rejects_edge_info_offset_out_of_bounds() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let header = tile.header();
        let size = header.edge_info_size().expect("valid header");
        let last = header.directed_edge_count() as usize - 1;
        let mut bytes = tile.borrow_owner().clone();
        // The edge info offset is the low 25 bits of the edge's second word.
        patch_bits(
            &mut bytes,
            directed_edge_offset(header, last) + 8,
            0,
            25,
            size as u64,
        );
        assert!(matches!(
            GraphTileView::try_from(bytes.as_slice()),
            Err(GraphTileDecodingError::InvalidData(
                GraphTileDataError::EdgeInfoOffsetOutOfBounds { edge, offset, size: s }
            )) if edge == last && offset == size && s == size
        ));
    }

    #[test]
    fn test_rejects_invalid_road_use() {
        let tile = &*TEST_GRAPH_TILE_L2;
        let header = tile.header();
        let mut bytes = tile.borrow_owner().clone();
        // The road use is bits 40 to 45 of the edge's third word.
        // 63 is unassigned in Valhalla 3.9.1.
        patch_bits(&mut bytes, directed_edge_offset(header, 7) + 16, 40, 6, 63);
        assert!(matches!(
            GraphTileView::try_from(bytes.as_slice()),
            Err(GraphTileDecodingError::InvalidData(
                GraphTileDataError::InvalidRoadUse { edge: 7, value: 63 }
            ))
        ));
    }

    #[test]
    fn test_rejects_invalid_predicted_speed_offset() {
        let tile = &*TEST_GRAPH_TILE_WITH_FLOW;
        let header = tile.header();
        let offsets_start = header
            .predicted_speeds_offset()
            .expect("has predicted speeds");
        let profile_count = header.predicted_speeds_count() as usize;
        // Edge 42 has predicted speeds (see TEST_GRAPH_TILE_WITH_FLOW).
        let edge_offset = offsets_start + 42 * size_of::<u32>();
        for (offset, expected) in [
            // Not the start of a profile, but mainline reads a whole profile from it.
            (1, false),
            // Less than a whole profile left.
            ((profile_count - 1) * COEFFICIENT_COUNT + 1, true),
            // One past the last profile.
            (profile_count * COEFFICIENT_COUNT, true),
            // The last profile.
            ((profile_count - 1) * COEFFICIENT_COUNT, false),
        ] {
            let mut bytes = tile.borrow_owner().clone();
            bytes[edge_offset..edge_offset + 4]
                .copy_from_slice(&u32::try_from(offset).unwrap().to_le_bytes());
            let result = GraphTileView::try_from(bytes.as_slice());
            if expected {
                assert!(matches!(
                    result,
                    Err(GraphTileDecodingError::InvalidData(
                        GraphTileDataError::InvalidPredictedSpeedOffset { edge: 42, offset: o, profile_count: p }
                    )) if o == offset && p == profile_count
                ));
            } else {
                assert!(result.is_ok());
            }
        }
    }

    #[test]
    fn test_accessors_on_foreign_data() {
        let tile = &*TEST_GRAPH_TILE_L0;

        // A node from "another tile", whose edges and transitions are out of range for this one.
        let mut node_bytes = tile.nodes()[0].as_bytes().to_vec();
        patch_bits(&mut node_bytes, 8, 0, 21, (1 << 21) - 1);
        patch_bits(&mut node_bytes, 8, 21, 7, 3);
        patch_bits(&mut node_bytes, 16, 0, 21, (1 << 21) - 1);
        patch_bits(&mut node_bytes, 16, 21, 3, 1);
        let node = NodeInfo::read_from_bytes(&node_bytes).expect("node-sized");
        assert!(tile.get_outbound_edges_from_node(&node).is_empty());
        assert!(tile.get_transitions(&node).is_empty());

        // An edge from "another tile", whose edge info offset is out of range for this one.
        let mut edge_bytes = tile.directed_edges()[0].as_bytes().to_vec();
        patch_bits(&mut edge_bytes, 8, 0, 25, (1 << 25) - 1);
        let edge = DirectedEdge::read_from_bytes(&edge_bytes).expect("edge-sized");
        assert!(matches!(
            tile.get_edge_info(&edge),
            Err(GraphTileDecodingError::InvalidData(
                GraphTileDataError::EdgeInfoLookupOutOfBounds { offset, .. }
            )) if offset == (1 << 25) - 1
        ));

        // Edge indexes past the end of the tile have no predicted speed.
        let with_flow = &*TEST_GRAPH_TILE_WITH_FLOW;
        let edge_count = with_flow.header().directed_edge_count() as usize;
        assert!(with_flow.get_predicted_speed(42, 0).is_some());
        assert_eq!(with_flow.get_predicted_speed(edge_count, 0), None);
        assert_eq!(with_flow.get_predicted_speed(usize::MAX, 0), None);
    }

    /// Calls every accessor that reads tile data, on every element of the tile.
    ///
    /// This is for the robustness tests below:
    /// once a tile parses, nothing here may panic, whatever the tile's contents.
    fn exercise_accessors(view: &GraphTileView<'_>) {
        let header = view.header();
        let _ = (header.version(), header.date_created(), header.density());
        for bin in 0..=crate::BIN_COUNT {
            let edges = view.edges_in_bin(bin);
            let circles = view.bounding_circles_in_bin(bin);
            assert!(circles.is_empty() || circles.len() == edges.len());
            if let Some(center) = header.bin_center(bin) {
                for circle in circles {
                    let _ = circle.decode(center);
                }
            }
        }
        for node in view.nodes() {
            for edge in view.get_outbound_edges_from_node(node) {
                let _ = (edge.road_use(), edge.classification(), edge.surface());
            }
            let _ = view.get_transitions(node).len();
        }
        for (index, _) in view.directed_edges().iter().enumerate() {
            // TODO: also call `get_edge_info` once `EdgeInfo::try_from` is panic-free.
            // It currently slices past the end of the edge info for corrupt shape sizes.
            let _ = view.get_predicted_speed(index, 0);
            let _ = view.get_access_restrictions(
                u32::try_from(index).expect("edge indexes fit in u32"),
                EnumSet::all(),
            );
            let id = view
                .graph_id()
                .with_feature_index(index as u64)
                .expect("edge indexes fit in a graph ID");
            assert!(view.get_directed_edge(id).is_ok());
            let _ = view.get_opp_edge_index(id);
            let _ = view.get_ext_directed_edge(id);
        }
        let center = header.sw_corner();
        let _ = view
            .nodes_within_radius(
                |_| true,
                Point::new(f64::from(center.x), f64::from(center.y)),
                1_000.0,
            )
            .count();
    }

    /// The fixture tiles used for the robustness tests:
    /// a small level 2 tile, a level 2 tile with bounding circles,
    /// and a level 0 tile with predicted speeds.
    static FUZZ_TILES: LazyLock<Vec<Vec<u8>>> = LazyLock::new(|| {
        let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        [
            "andorra-tiles/2/000/763/927.gph",
            "andorra-tiles/2/000/762/485.gph",
            "andorra-tiles-with-traffic/0/003/015.gph",
        ]
        .into_iter()
        .map(|path| std::fs::read(fixtures.join(path)).expect("readable fixture"))
        .collect()
    });

    /// Parses possibly corrupt tile data, and if it parses, exercises every accessor.
    fn parse_and_exercise(bytes: &[u8]) {
        if let Ok(view) = GraphTileView::try_from(bytes) {
            exercise_accessors(&view);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn truncated_tiles_never_panic(tile in 0..3usize, len in any::<prop::sample::Index>()) {
            let bytes = &FUZZ_TILES[tile];
            let len = len.index(bytes.len());
            // Every truncation changes the length, so it can never parse.
            prop_assert!(GraphTileView::try_from(&bytes[..len]).is_err());
        }

        #[test]
        fn corrupted_tiles_never_panic(
            tile in 0..3usize,
            edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..16),
        ) {
            let mut bytes = FUZZ_TILES[tile].clone();
            for (index, value) in edits {
                let index = index.index(bytes.len());
                bytes[index] = value;
            }
            parse_and_exercise(&bytes);
        }

        #[test]
        fn corrupted_headers_never_panic(
            tile in 0..3usize,
            edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8),
        ) {
            // Most random edits to the identity fields (graph ID, base coordinate, and version)
            // fail fast, so focus on the counts and offsets that follow them.
            let mut bytes = FUZZ_TILES[tile].clone();
            let counts = 40..size_of::<GraphTileHeader>();
            for (index, value) in edits {
                bytes[counts.start + index.index(counts.len())] = value;
            }
            parse_and_exercise(&bytes);
        }

        #[test]
        fn corrupted_sections_never_panic(
            tile in 0..3usize,
            edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..64),
        ) {
            // With the header intact, more tiles parse, so this mostly exercises the accessors
            // on corrupt nodes, edges, and other records.
            let mut bytes = FUZZ_TILES[tile].clone();
            let body = size_of::<GraphTileHeader>()..bytes.len();
            for (index, value) in edits {
                bytes[body.start + index.index(body.len())] = value;
            }
            parse_and_exercise(&bytes);
        }
    }
}
