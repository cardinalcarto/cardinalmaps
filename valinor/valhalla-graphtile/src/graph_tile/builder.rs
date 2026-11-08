//! Rewriting graph tiles.
//!
//! [`GraphTileBuilder`] starts from an existing tile and writes a modified copy.
//! Every header field that locates or counts a section is recomputed
//! from the sections actually written, so a section can't be dropped or resized
//! while the header still describes the old layout.
//! (That is the class of bug behind mainline's `valhalla_add_landmarks` misrouting in 3.9.1.)
//! The tests check that mainline reads modified tiles back identically,
//! apart from the intended changes.
use super::{
    AccessRestriction, Admin, BuildId, DirectedEdge, DirectedEdgeExt, DiscretizedBoundingCircle,
    GraphTileBuildError, GraphTileView, NodeInfo, NodeTransition, OwnedGraphTileHandle, Sign,
    TileHasher, TransitDeparture, TransitRoute, TransitSchedule, TransitStop, TransitTransfer,
    TurnLane,
};
use crate::graph_tile::checksum::checksum;
use crate::graph_tile::header::{GraphTileHeaderBuilder, VERSION_LEN};
use crate::graph_tile::predicted_speeds::{
    BUCKETS_PER_WEEK, COEFFICIENT_COUNT, compress_speed_buckets, decode_base64_speed_coefficients,
};
use crate::{BIN_COUNT, GraphId};
use chrono::{DateTime, Utc};
use geo::Coord;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use std::borrow::Cow;
use std::collections::HashMap;
use zerocopy::{I16, Immutable, IntoBytes, LE, U32};

/// Encodes a writer version for the header.
///
/// Mainline reads the version as a NUL-terminated string,
/// and its own `set_version` keeps at most 15 bytes so the last byte is always NUL.
/// A longer version (or one with a NUL inside) would read differently in mainline,
/// so it's rejected.
fn writer_version_to_bytes(version: &str) -> Option<[u8; VERSION_LEN]> {
    let bytes = version.as_bytes();
    if bytes.len() < VERSION_LEN && !bytes.contains(&0) {
        let mut out = [0u8; VERSION_LEN];
        out[..bytes.len()].copy_from_slice(bytes);
        Some(out)
    } else {
        None
    }
}

/// A builder for constructing modified graph tiles.
///
/// # Design principles
///
/// We made some conscious design choices when building this.
/// Here is a brief discussion of these and their rationale for posterity.
///
/// ## Use of copy-on-write internal fields
///
/// One of the more common usage patterns is to construct the builder
/// from an existing graph tile handle.
/// To avoid needless copying (especially when the edits are localized to a small section of the tile),
/// we lazily copy as needed.
///
/// ## Late materialization of derived structures
///
/// Things like headers are not computed every time.
/// Rather, we keep enough information around to derive what the header fields should be.
///
/// ## Writer version
///
/// The builder keeps the writer version of the tile it started from,
/// as mainline's `GraphTileBuilder` does when it rewrites a tile
/// (mainline only stamps its own version into tiles it creates from scratch).
/// Nothing reads the version to decide compatibility.
/// Use [`GraphTileBuilder::with_version`] to set a different one.
pub struct GraphTileBuilder<'a> {
    writer_version: [u8; VERSION_LEN],
    graph_id: GraphId,
    dataset_id: u64,
    sw_corner: Coord<f32>,
    create_date: DateTime<Utc>,
    build_id: BuildId,
    /// The relative road density (0-15), kept from the original tile.
    density: u8,
    /// Whether the edge info carries elevation, kept from the original tile.
    has_elevation: bool,
    /// The cumulative edge counts of each bin, kept from the original tile.
    ///
    /// These describe `edge_bins` (the last one is its length),
    /// which the builder never changes.
    bin_offsets: [u32; BIN_COUNT],
    nodes: Cow<'a, [NodeInfo]>,
    transitions: Cow<'a, [NodeTransition]>,
    directed_edges: Cow<'a, [DirectedEdge]>,
    ext_directed_edges: Cow<'a, [DirectedEdgeExt]>,
    access_restrictions: Cow<'a, [AccessRestriction]>,
    transit_departures: Cow<'a, [TransitDeparture]>,
    transit_stops: Cow<'a, [TransitStop]>,
    transit_routes: Cow<'a, [TransitRoute]>,
    transit_schedules: Cow<'a, [TransitSchedule]>,
    transit_transfers: Cow<'a, [TransitTransfer]>,
    signs: Cow<'a, [Sign]>,
    turn_lanes: Cow<'a, [TurnLane]>,
    admins: Cow<'a, [Admin]>,
    edge_bins: Cow<'a, [GraphId]>,
    bounding_circles: Cow<'a, [DiscretizedBoundingCircle]>,
    complex_forward_restrictions_memory: Cow<'a, [u8]>,
    complex_reverse_restrictions_memory: Cow<'a, [u8]>,
    edge_info_memory: Cow<'a, [u8]>,
    text_memory: Cow<'a, [u8]>,
    lane_connectivity_memory: Cow<'a, [u8]>,
    /// The coefficient offset of each directed edge's speed profile.
    ///
    /// Either empty (no profiles) or one per directed edge.
    /// The tile reader guarantees this for tiles it reads,
    /// and the builder fills in the whole array when it adds the first profile.
    predicted_speed_offsets: Cow<'a, [U32<LE>]>,
    /// Raw profile memory (`n_profiles x COEFFICIENT_COUNT` entries back to back)
    predicted_speed_profile_memory: Cow<'a, [I16<LE>]>,
    /// Lazily initialized map for deduplicating speed profiles.
    /// None = not yet initialized; Some = ready for use.
    speed_profile_dedup: Option<HashMap<[i16; COEFFICIENT_COUNT], u32>>,
}

/// Builds a deduplication map from predicted speed profile memory.
///
/// Scans the flat profile memory (semantically a 2D array with fixed-width rows)
/// and creates a `HashMap` mapping each unique profile to its offset.
/// If duplicate profiles exist in the input, only the first occurrence is retained.
fn build_speed_profile_dedup_map(memory: &[I16<LE>]) -> HashMap<[i16; COEFFICIENT_COUNT], u32> {
    let mut map = HashMap::new();
    let (profiles, _) = memory.as_chunks::<COEFFICIENT_COUNT>();
    // Offsets are u32, so profiles past u32::MAX coefficients can't be referenced anyway.
    let offsets = (0..=u32::MAX).step_by(COEFFICIENT_COUNT);
    for (offset, profile) in offsets.zip(profiles) {
        // Only insert if this profile hasn't been seen before
        map.entry(profile.map(I16::get)).or_insert(offset);
    }
    map
}

impl<'a> From<&'a OwnedGraphTileHandle> for GraphTileBuilder<'a> {
    fn from(value: &'a OwnedGraphTileHandle) -> Self {
        let GraphTileView {
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
        } = value.borrow_dependent();

        let (predicted_speed_offsets, predicted_speed_profile_memory) = match predicted_speeds {
            Some(ps) => {
                let (offsets, profiles) = ps.as_offsets_and_profiles();
                (Cow::Borrowed(offsets), Cow::Borrowed(profiles))
            }
            None => Default::default(),
        };

        GraphTileBuilder {
            writer_version: header.version,
            graph_id: header.graph_id(),
            dataset_id: header.dataset_id.get(),
            sw_corner: header.sw_corner(),
            create_date: header.create_date(),
            build_id: header.build_id(),
            density: header.density(),
            has_elevation: header.has_elevation(),
            bin_offsets: header.bin_offsets.map(U32::get),
            nodes: Cow::Borrowed(nodes),
            transitions: Cow::Borrowed(transitions),
            directed_edges: Cow::Borrowed(directed_edges),
            ext_directed_edges: Cow::Borrowed(ext_directed_edges),
            access_restrictions: Cow::Borrowed(access_restrictions),
            transit_departures: Cow::Borrowed(transit_departures),
            transit_stops: Cow::Borrowed(transit_stops),
            transit_routes: Cow::Borrowed(transit_routes),
            transit_schedules: Cow::Borrowed(transit_schedules),
            transit_transfers: Cow::Borrowed(transit_transfers),
            signs: Cow::Borrowed(signs),
            turn_lanes: Cow::Borrowed(turn_lanes),
            admins: Cow::Borrowed(admins),
            edge_bins: Cow::Borrowed(edge_bins),
            bounding_circles: Cow::Borrowed(bounding_circles),
            complex_forward_restrictions_memory: Cow::Borrowed(complex_forward_restrictions_memory),
            complex_reverse_restrictions_memory: Cow::Borrowed(complex_reverse_restrictions_memory),
            edge_info_memory: Cow::Borrowed(edge_info_memory),
            text_memory: Cow::Borrowed(text_memory),
            lane_connectivity_memory: Cow::Borrowed(lane_connectivity_memory),
            predicted_speed_offsets,
            predicted_speed_profile_memory,
            speed_profile_dedup: None,
        }
    }
}

impl GraphTileBuilder<'_> {
    /// The Graph ID for the tile.
    ///
    /// This is guaranteed to always be a base ID.
    #[inline]
    pub fn graph_id(&self) -> GraphId {
        self.graph_id
    }

    /// Sets the version string to encode in the graph tile.
    ///
    /// This is purely metadata and is not used by Valhalla to determine compatibility.
    ///
    /// # Errors
    ///
    /// The string must be at most 15 bytes in UTF-8, with no NUL characters,
    /// since mainline reads it as a NUL-terminated string in a 16-byte field.
    /// Otherwise, this returns an error.
    #[inline]
    pub fn with_version(self, version: &str) -> Result<Self, GraphTileBuildError> {
        let writer_version = writer_version_to_bytes(version)
            .ok_or_else(|| GraphTileBuildError::InvalidVersionString(version.to_string()))?;

        Ok(Self {
            writer_version,
            ..self
        })
    }

    /// Sets the tileset build ID to stamp into the tile header.
    ///
    /// The build ID is derived from the hashes of every tile in the tileset
    /// (see [`BuildId::of_tileset`]),
    /// so a single tile cannot compute it.
    /// By default, the builder keeps the build ID of the tile it started from,
    /// which is what Valhalla does until it recomputes the build ID for the whole tileset.
    ///
    /// If you only know the build ID after building every tile
    /// (because it depends on their hashes),
    /// build the tiles first and then stamp the ID into each of them
    /// with [`GraphTileHeader::stamp_build_id`](super::GraphTileHeader::stamp_build_id).
    #[inline]
    #[must_use]
    pub fn with_build_id(self, build_id: BuildId) -> Self {
        Self { build_id, ..self }
    }

    /// Checks that a directed edge index is in this tile.
    fn check_edge_index(
        &self,
        directed_edge_index: usize,
        what: &str,
    ) -> Result<(), GraphTileBuildError> {
        if directed_edge_index < self.directed_edges.len() {
            Ok(())
        } else {
            Err(GraphTileBuildError::InvalidIndex(format!(
                "Attempted to set {what} for directed edge index {directed_edge_index}, but tile only has {} edges",
                self.directed_edges.len()
            )))
        }
    }

    /// Adds historical average (coarse granularity) speed information to a directed edge.
    ///
    /// Zero indicates that no data is available.
    /// This is what mainline's `valhalla_add_predicted_traffic` sets
    /// from the free and constrained flow columns of its CSV input.
    ///
    /// # Errors
    ///
    /// Fails if the directed edge index is out of bounds.
    pub fn with_average_speeds(
        mut self,
        directed_edge_index: usize,
        free_flow_speed: u8,
        constrained_speed: u8,
    ) -> Result<Self, GraphTileBuildError> {
        self.check_edge_index(directed_edge_index, "historical average speeds")?;
        if let Some(edge) = self.directed_edges.to_mut().get_mut(directed_edge_index) {
            edge.set_free_flow_speed(free_flow_speed);
            edge.set_constrained_speed(constrained_speed);
        }
        Ok(self)
    }

    /// Adds predicted speeds to a directed edge using pre-encoded DCT-II coefficients.
    ///
    /// This method is probably the least ergonomic in the family of predicted speed APIs,
    /// but it's also the most efficient (if you can use it directly)
    /// since it doesn't need to do any format shifting.
    /// All other predicted speed methods are convenience wrappers around this.
    ///
    /// Identical profiles are stored once, as mainline's `GraphTileBuilder::AddPredictedSpeed` does.
    /// If the edge already had a profile, it now points at the new one.
    ///
    /// See the [`crate::graph_tile::predicted_speeds`] module for more details
    /// on predicted speeds.
    ///
    /// # Errors
    ///
    /// Fails if the directed edge index is invalid,
    /// or you somehow managed to add more than 2 billion speed profiles to a single tile.
    pub fn with_predicted_speed_coefficients(
        mut self,
        directed_edge_index: usize,
        coefficients: &[i16; COEFFICIENT_COUNT],
    ) -> Result<Self, GraphTileBuildError> {
        self.check_edge_index(directed_edge_index, "predicted speeds")?;

        // Lazy initialization: populate the dedup map from existing profiles if needed
        let dedup_map = self.speed_profile_dedup.get_or_insert_with(|| {
            build_speed_profile_dedup_map(&self.predicted_speed_profile_memory)
        });
        let offset = if let Some(&existing_offset) = dedup_map.get(coefficients) {
            // Profile already exists; reuse the existing offset
            existing_offset
        } else {
            // New profile; append it to the memory and record the offset
            let new_offset = u32::try_from(self.predicted_speed_profile_memory.len())?;
            self.predicted_speed_profile_memory
                .to_mut()
                .extend(coefficients.map(I16::new));
            dedup_map.insert(*coefficients, new_offset);
            new_offset
        };

        // A tile with any profile has an offset for every edge (0 for edges without one).
        let edge_count = self.directed_edges.len();
        let offsets = self.predicted_speed_offsets.to_mut();
        if offsets.is_empty() {
            offsets.resize(edge_count, U32::new(0));
        }
        if let Some(slot) = offsets.get_mut(directed_edge_index) {
            *slot = U32::new(offset);
        }

        // Set the flag indicating that we have predicted speeds for this edge.
        if let Some(edge) = self.directed_edges.to_mut().get_mut(directed_edge_index) {
            edge.set_has_predicted_speed(true);
        }

        Ok(self)
    }

    /// Adds predicted speeds to a directed edge from a base64 encoded string
    /// containing the DCT-II coefficients.
    ///
    /// This is slightly less efficient than directly setting the coefficients using
    /// [`with_predicted_speed_coefficients`](GraphTileBuilder::with_predicted_speed_coefficients),
    /// but if you're working with text files or some other situation where you'd prefer not to use binary data,
    /// base64 encoding is a happy middle ground.
    ///
    /// See the [`crate::graph_tile::predicted_speeds`] module for more details
    /// on predicted speeds.
    ///
    /// # Errors
    ///
    /// This will fail if the directed edge index is out of bounds,
    /// the compressed speeds string doesn't decode to exactly 400 bytes,
    /// or you somehow managed to add more than 2 billion speed profiles to a single tile.
    #[inline]
    pub fn with_predicted_encoded_speeds(
        self,
        directed_edge_index: usize,
        compressed_speeds: &str,
    ) -> Result<Self, GraphTileBuildError> {
        self.with_predicted_speed_coefficients(
            directed_edge_index,
            &decode_base64_speed_coefficients(compressed_speeds)?,
        )
    }

    /// Adds predicted speeds to a directed edge from a list of samples
    /// representing evenly spaced intervals throughout a week.
    ///
    /// See the [`crate::graph_tile::predicted_speeds`] module for more details
    /// on predicted speeds.
    ///
    /// # Errors
    ///
    /// Fails if the directed edge index is invalid,
    /// or you somehow managed to add more than 2 billion speed profiles to a single tile.
    #[inline]
    pub fn with_predicted_speeds(
        self,
        directed_edge_index: usize,
        speeds: &[f32; BUCKETS_PER_WEEK],
    ) -> Result<Self, GraphTileBuildError> {
        self.with_predicted_speed_coefficients(directed_edge_index, &compress_speed_buckets(speeds))
    }

    /// Yields the raw bytes of a full tile in the order they should be stored on disk.
    ///
    /// This API is produces each section of the file
    /// (e.g. nodes, directed edges, names, etc.) lazily as requested.
    /// Thus, the most memory it will need at one time is bounded
    /// by the size of the largest section.
    ///
    /// # Errors
    ///
    /// Fails if anything is amiss in the tile.
    /// This should be pretty uncommon, but the following kinds of situations could theoretically
    /// trigger an error:
    ///
    /// * Including than the allowed number of some feature type (e.g. more than 2^24 access restrictions)
    /// * Managing to create a single tile larger than 4 gigabytes
    /// * Setting a creation date more than 11,758,979.59 years after January 1, 2014
    ///
    /// TL;DR, things that you really shouldn't do, or are a clear programming error.
    pub fn into_byte_iter(self) -> Result<impl Iterator<Item = Box<[u8]>>, GraphTileBuildError> {
        let mut tile = TileByteIter {
            header: Box::default(),
            stage: TileBuildStage::Header,
            nodes: self.nodes,
            transitions: self.transitions,
            directed_edges: self.directed_edges,
            ext_directed_edges: self.ext_directed_edges,
            access_restrictions: self.access_restrictions,
            transit_departures: self.transit_departures,
            transit_stops: self.transit_stops,
            transit_routes: self.transit_routes,
            transit_schedules: self.transit_schedules,
            transit_transfers: self.transit_transfers,
            signs: self.signs,
            turn_lanes: self.turn_lanes,
            admins: self.admins,
            edge_bins: self.edge_bins,
            bounding_circles: self.bounding_circles,
            complex_forward_restrictions_memory: self.complex_forward_restrictions_memory,
            complex_reverse_restrictions_memory: self.complex_reverse_restrictions_memory,
            edge_info_memory: self.edge_info_memory,
            text_memory: self.text_memory,
            lane_connectivity_memory: self.lane_connectivity_memory,
            predicted_speed_offsets: self.predicted_speed_offsets,
            predicted_speed_profile_memory: self.predicted_speed_profile_memory,
        };

        // The header holds a hash of everything after it,
        // so we hash the body before producing the header.
        let mut hasher = TileHasher::new();
        for section in tile.body_sections() {
            hasher.update(section);
        }
        let tile_hash = hasher.finish();

        // Every count and size comes from the sections as they will be written.
        let header = GraphTileHeaderBuilder {
            version: self.writer_version,
            graph_id: self.graph_id,
            density: self.density,
            has_elevation: self.has_elevation,
            has_ext_directed_edges: !tile.ext_directed_edges.is_empty(),
            sw_corner: self.sw_corner,
            dataset_id: self.dataset_id,
            node_count: tile.nodes.len(),
            directed_edge_count: tile.directed_edges.len(),
            // Profiles are only ever stored whole, so this is exact.
            predicted_speed_profile_count: tile.predicted_speed_profile_memory.len()
                / COEFFICIENT_COUNT,
            transition_count: tile.transitions.len(),
            turn_lane_count: tile.turn_lanes.len(),
            transfer_count: tile.transit_transfers.len(),
            departure_count: tile.transit_departures.len(),
            stop_count: tile.transit_stops.len(),
            route_count: tile.transit_routes.len(),
            schedule_count: tile.transit_schedules.len(),
            sign_count: tile.signs.len(),
            access_restriction_count: tile.access_restrictions.len(),
            admin_count: tile.admins.len(),
            create_date: self.create_date,
            bin_offsets: self.bin_offsets,
            bounding_circle_count: tile.bounding_circles.len(),
            checksum: checksum(self.build_id, tile_hash),
            complex_forward_restrictions_size: tile.complex_forward_restrictions_memory.len(),
            complex_reverse_restrictions_size: tile.complex_reverse_restrictions_memory.len(),
            edge_info_size: tile.edge_info_memory.len(),
            text_list_size: tile.text_memory.len(),
            lane_connectivity_size: tile.lane_connectivity_memory.len(),
        };
        tile.header = header.build()?.as_bytes().into();

        Ok(tile)
    }

    /// Serializes the tile as owned bytes.
    ///
    /// NOTE: This allocates somewhere on the order of 2x the final tile size
    /// while building the tile.
    /// Use other build methods if you need to conserve RAM.
    ///
    /// # Errors
    ///
    /// Refer to the documentation for [`GraphTileBuilder::into_byte_iter`] for error conditions.
    #[inline]
    pub fn into_bytes(self) -> Result<Vec<u8>, GraphTileBuildError> {
        Ok(self.into_byte_iter()?.flatten().collect())
    }
}

/// An iterator that lazily produces a full graph tile without too many extra allocations.
///
/// Specifically, while this iterator is not zero-cost,
/// it will only allocate one section (e.g., nodes, directed edges, etc.)
/// at a time.
///
/// This dramatically reduces the peak memory usage when serializing a tile directly
/// to an output stream (file, network, etc.).
struct TileByteIter<'a> {
    header: Box<[u8]>,

    /// The current stage of production.
    stage: TileBuildStage,

    // Body sections (matching into_bytes order)
    nodes: Cow<'a, [NodeInfo]>,
    transitions: Cow<'a, [NodeTransition]>,
    directed_edges: Cow<'a, [DirectedEdge]>,
    ext_directed_edges: Cow<'a, [DirectedEdgeExt]>,
    access_restrictions: Cow<'a, [AccessRestriction]>,
    transit_departures: Cow<'a, [TransitDeparture]>,
    transit_stops: Cow<'a, [TransitStop]>,
    transit_routes: Cow<'a, [TransitRoute]>,
    transit_schedules: Cow<'a, [TransitSchedule]>,
    transit_transfers: Cow<'a, [TransitTransfer]>,
    signs: Cow<'a, [Sign]>,
    turn_lanes: Cow<'a, [TurnLane]>,
    admins: Cow<'a, [Admin]>,
    edge_bins: Cow<'a, [GraphId]>,
    bounding_circles: Cow<'a, [DiscretizedBoundingCircle]>,
    complex_forward_restrictions_memory: Cow<'a, [u8]>,
    complex_reverse_restrictions_memory: Cow<'a, [u8]>,
    edge_info_memory: Cow<'a, [u8]>,
    text_memory: Cow<'a, [u8]>,
    lane_connectivity_memory: Cow<'a, [u8]>,
    predicted_speed_offsets: Cow<'a, [U32<LE>]>,
    predicted_speed_profile_memory: Cow<'a, [I16<LE>]>,
}

/// The stage of the tile production.
///
/// Stages are rendered in the order specified below
/// (variant order matters!!).
#[derive(Copy, Clone, Debug, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
enum TileBuildStage {
    Header,
    Nodes,
    Transitions,
    DirectedEdges,
    ExtDirectedEdges,
    AccessRestrictions,
    TransitDepartures,
    TransitStops,
    TransitRoutes,
    TransitSchedules,
    TransitTransfers,
    Signs,
    TurnLanes,
    Admins,
    EdgeBins,
    BoundingCircles,
    ComplexForwardRestrictions,
    ComplexReverseRestrictions,
    EdgeInfo,
    TextMemory,
    LaneConnectivity,
    PredictedSpeedOffsets,
    PredictedSpeedProfiles,
    Done,
}

impl TileBuildStage {
    fn next(self) -> TileBuildStage {
        let current: u8 = self.into();

        TileBuildStage::try_from(current + 1).unwrap_or(TileBuildStage::Done)
    }
}

impl TileByteIter<'_> {
    /// The raw bytes of every section after the header, in the order they are stored on disk.
    ///
    /// This must stay in sync with [`TileBuildStage`] and [`Iterator::next`].
    fn body_sections(&self) -> [&[u8]; 22] {
        [
            self.nodes.as_bytes(),
            self.transitions.as_bytes(),
            self.directed_edges.as_bytes(),
            self.ext_directed_edges.as_bytes(),
            self.access_restrictions.as_bytes(),
            self.transit_departures.as_bytes(),
            self.transit_stops.as_bytes(),
            self.transit_routes.as_bytes(),
            self.transit_schedules.as_bytes(),
            self.transit_transfers.as_bytes(),
            self.signs.as_bytes(),
            self.turn_lanes.as_bytes(),
            self.admins.as_bytes(),
            self.edge_bins.as_bytes(),
            self.bounding_circles.as_bytes(),
            self.complex_forward_restrictions_memory.as_bytes(),
            self.complex_reverse_restrictions_memory.as_bytes(),
            self.edge_info_memory.as_bytes(),
            self.text_memory.as_bytes(),
            self.lane_connectivity_memory.as_bytes(),
            self.predicted_speed_offsets.as_bytes(),
            self.predicted_speed_profile_memory.as_bytes(),
        ]
    }
}

impl Iterator for TileByteIter<'_> {
    type Item = Box<[u8]>;

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = match self.stage {
            TileBuildStage::Header => std::mem::take(&mut self.header),
            TileBuildStage::Nodes => bytes_from_items(&std::mem::take(&mut self.nodes)),
            TileBuildStage::Transitions => bytes_from_items(&std::mem::take(&mut self.transitions)),
            TileBuildStage::DirectedEdges => {
                bytes_from_items(&std::mem::take(&mut self.directed_edges))
            }
            TileBuildStage::ExtDirectedEdges => {
                bytes_from_items(&std::mem::take(&mut self.ext_directed_edges))
            }
            TileBuildStage::AccessRestrictions => {
                bytes_from_items(&std::mem::take(&mut self.access_restrictions))
            }
            TileBuildStage::TransitDepartures => {
                bytes_from_items(&std::mem::take(&mut self.transit_departures))
            }
            TileBuildStage::TransitStops => {
                bytes_from_items(&std::mem::take(&mut self.transit_stops))
            }
            TileBuildStage::TransitRoutes => {
                bytes_from_items(&std::mem::take(&mut self.transit_routes))
            }
            TileBuildStage::TransitSchedules => {
                bytes_from_items(&std::mem::take(&mut self.transit_schedules))
            }
            TileBuildStage::TransitTransfers => {
                bytes_from_items(&std::mem::take(&mut self.transit_transfers))
            }
            TileBuildStage::Signs => bytes_from_items(&std::mem::take(&mut self.signs)),
            TileBuildStage::TurnLanes => bytes_from_items(&std::mem::take(&mut self.turn_lanes)),
            TileBuildStage::Admins => bytes_from_items(&std::mem::take(&mut self.admins)),
            TileBuildStage::EdgeBins => bytes_from_items(&std::mem::take(&mut self.edge_bins)),
            TileBuildStage::BoundingCircles => {
                bytes_from_items(&std::mem::take(&mut self.bounding_circles))
            }
            TileBuildStage::ComplexForwardRestrictions => bytes_from_items(&std::mem::take(
                &mut self.complex_forward_restrictions_memory,
            )),
            TileBuildStage::ComplexReverseRestrictions => bytes_from_items(&std::mem::take(
                &mut self.complex_reverse_restrictions_memory,
            )),
            TileBuildStage::EdgeInfo => {
                bytes_from_items(&std::mem::take(&mut self.edge_info_memory))
            }
            TileBuildStage::TextMemory => bytes_from_items(&std::mem::take(&mut self.text_memory)),
            TileBuildStage::LaneConnectivity => {
                bytes_from_items(&std::mem::take(&mut self.lane_connectivity_memory))
            }
            TileBuildStage::PredictedSpeedOffsets => {
                bytes_from_items(&std::mem::take(&mut self.predicted_speed_offsets))
            }
            TileBuildStage::PredictedSpeedProfiles => {
                bytes_from_items(&std::mem::take(&mut self.predicted_speed_profile_memory))
            }
            TileBuildStage::Done => {
                // No more sections
                return None;
            }
        };

        self.stage = self.stage.next();
        Some(bytes)
    }
}

/// Copies a section's bytes out.
///
/// Callers pass a section they've just taken out of the iterator,
/// so its memory is released as soon as the copy is made.
fn bytes_from_items<T: IntoBytes + Immutable>(items: &[T]) -> Box<[u8]> {
    items.as_bytes().into()
}

#[cfg(test)]
mod tests {
    use crate::graph_tile::predicted_speeds::COEFFICIENT_COUNT;
    use crate::graph_tile::{
        BuildId, GraphTile, GraphTileBuildError, GraphTileBuilder, GraphTileHeader,
        OwnedGraphTileHandle, TileHash,
    };
    use std::path::Path;
    use walkdir::WalkDir;

    fn assert_round_trip_unmodified_equals_original(path: &Path) {
        const HEADER_SIZE: usize = size_of::<GraphTileHeader>();

        let in_bytes = std::fs::read(path).expect("Unable to read file");
        let tile_handle =
            OwnedGraphTileHandle::try_from(in_bytes.clone()).expect("Unable to get tile handle");

        let builder = GraphTileBuilder::from(&tile_handle);
        let out_bytes = builder.into_bytes().expect("Unable to serialize tile");

        let in_header_slice: [u8; HEADER_SIZE] = in_bytes[0..HEADER_SIZE]
            .try_into()
            .expect("oops; couldn't parse header?");
        let in_header: GraphTileHeader = zerocopy::transmute!(in_header_slice);

        let out_header_slice: [u8; HEADER_SIZE] = out_bytes[0..HEADER_SIZE]
            .try_into()
            .expect("oops; couldn't parse header?");
        let out_header: GraphTileHeader = zerocopy::transmute!(out_header_slice);

        assert_eq!(
            in_header, out_header,
            "Failed round-trip builder test (header) for fixture at {path:?}"
        );

        assert_eq!(
            in_header_slice, out_header_slice,
            "Failed round-trip builder test (header; slice) for fixture at {path:?}"
        );

        assert_eq!(
            in_bytes, out_bytes,
            "Failed round-trip builder test (whole tile) for fixture at {path:?}"
        );
    }

    #[test]
    fn round_trip_from_builder_without_changes() {
        let base_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles");
        for entry in WalkDir::new(base_dir) {
            let entry = entry.expect("Unable to traverse directory");

            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("gph"))
            {
                assert_round_trip_unmodified_equals_original(entry.path());

                if cfg!(miri) {
                    // Only run a single tile test under miri; it's too expensive.
                    return;
                }
            }
        }
    }

    #[test]
    fn round_trip_from_builder_without_changes_including_traffic() {
        let base_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles-with-traffic");
        for entry in WalkDir::new(base_dir) {
            let entry = entry.expect("Unable to traverse directory");

            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("gph"))
            {
                assert_round_trip_unmodified_equals_original(entry.path());

                if cfg!(miri) {
                    // Only run a single tile test under miri; it's too expensive.
                    return;
                }
            }
        }
    }

    #[test]
    fn add_predicted_speeds() {
        let base_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles");

        let in_bytes = std::fs::read(base_dir.join("0").join("003").join("015.gph"))
            .expect("Unable to read file");
        let tile_handle =
            OwnedGraphTileHandle::try_from(in_bytes.clone()).expect("Unable to get tile handle");

        // Enrich the tile with the exact same speed information that we did using the Valhalla CSV tools.
        //
        // 0/3015/0,50,40,
        // 0/3015/42,100,42,BcUACQAEACEADP/7/9sAGf/fAAwAGwARAAX/+AAAAB0AFAAS//AAF//+ACwACQAqAAAAKP/6AEMABABsAAsBBAAq/rcAAP+bAAz/zv/s/9MABf/Y//X/8//9/+wACf/P//EADv/8//L//P/y//H////7AAwAFf/5//oADgAZAAQAFf/3/+8AB//yAB8ABv/0AAUAEf/8//QAFAAG//b////j//v//QAT//7/+f/kABMABwABAAv/6//8//cAEwAAABT/8v/6//wAAAAQ//cACwAFAAT/1//sAAEADAABAAYAE//9AAn/7gAH/+AAFQAB//4AC//o/+gAE//+AAAAFf/l//kABP/+//kACAAG//cAHv/qAB0AAv/4/+v/+wALAAMABP/3AAT/8wAIAAr/9wAK//j/+wAEAAD/+P/8//v/8f/2//L//AALAAcABgAG//gAAv/5AAoAHv//AAcAFf/zABD/7AAUAAv/7v/8AAgACAAN//0ADP/iABD/9f/3//7/+P/3AAQADP//AAMABw==
        // 0/3015/7,12,34,CKD/4f/r/+H/6//g/+v/4P/q/9//6v/f/+r/3v/q/93/6f/b/+n/2v/o/9f/5//V/+b/0f/l/8z/4//G/+D/vf/d/67/1/+V/8z/Xf+u/nz+GwLJAEcAqwAWAFwABwA8AAAAK//8ACD/+AAZ//YAE//0AA//8gAM//AACf/uAAb/7AAE/+kAAv/m////4v/9/93/+f/U//T/xf/q/5//y/6PAQMAngApADkAFwAfABAAEwAMAAwACQAHAAcAAwAGAAAABf/9AAT/+wAD//kAA//2AAL/9AAC//EAAf/tAAH/6AAA/+EAAP/U////tv///x8AFADL//8AQ///ACf//gAa//4AE//+AA7//QAL//0ACP/9AAb//AAE//wAAv/8AAD/+//+//v//P/6//r/+v/2//n/8v/4/+v/9f/b/+//nP+TALoADAAyAAMAHgAAABX//gAQ//0ADf/9AAv//AAJ//sAB//7AAb/+gAF//kABP/4AAP/9wAC//YAAf/1AAD/8//+//D//P/q//f/2w==
        let enriched = || {
            GraphTileBuilder::from(&tile_handle)
            // Typical flow speeds
            .with_average_speeds(0, 50, 40).unwrap()
            .with_average_speeds(42, 100, 42).unwrap()
            // No, this isn't supposed to make sense; it's test data :P
            .with_average_speeds(7, 12, 34).unwrap()
            // Granular predicted (historical) speeds
            .with_predicted_encoded_speeds(7, "CKD/4f/r/+H/6//g/+v/4P/q/9//6v/f/+r/3v/q/93/6f/b/+n/2v/o/9f/5//V/+b/0f/l/8z/4//G/+D/vf/d/67/1/+V/8z/Xf+u/nz+GwLJAEcAqwAWAFwABwA8AAAAK//8ACD/+AAZ//YAE//0AA//8gAM//AACf/uAAb/7AAE/+kAAv/m////4v/9/93/+f/U//T/xf/q/5//y/6PAQMAngApADkAFwAfABAAEwAMAAwACQAHAAcAAwAGAAAABf/9AAT/+wAD//kAA//2AAL/9AAC//EAAf/tAAH/6AAA/+EAAP/U////tv///x8AFADL//8AQ///ACf//gAa//4AE//+AA7//QAL//0ACP/9AAb//AAE//wAAv/8AAD/+//+//v//P/6//r/+v/2//n/8v/4/+v/9f/b/+//nP+TALoADAAyAAMAHgAAABX//gAQ//0ADf/9AAv//AAJ//sAB//7AAb/+gAF//kABP/4AAP/9wAC//YAAf/1AAD/8//+//D//P/q//f/2w==").unwrap()
            .with_predicted_encoded_speeds(42, "BcUACQAEACEADP/7/9sAGf/fAAwAGwARAAX/+AAAAB0AFAAS//AAF//+ACwACQAqAAAAKP/6AEMABABsAAsBBAAq/rcAAP+bAAz/zv/s/9MABf/Y//X/8//9/+wACf/P//EADv/8//L//P/y//H////7AAwAFf/5//oADgAZAAQAFf/3/+8AB//yAB8ABv/0AAUAEf/8//QAFAAG//b////j//v//QAT//7/+f/kABMABwABAAv/6//8//cAEwAAABT/8v/6//wAAAAQ//cACwAFAAT/1//sAAEADAABAAYAE//9AAn/7gAH/+AAFQAB//4AC//o/+gAE//+AAAAFf/l//kABP/+//kACAAG//cAHv/qAB0AAv/4/+v/+wALAAMABP/3AAT/8wAIAAr/9wAK//j/+wAEAAD/+P/8//v/8f/2//L//AALAAcABgAG//gAAv/5AAoAHv//AAcAFf/zABD/7AAUAAv/7v/8AAgACAAN//0ADP/iABD/9f/3//7/+P/3AAQADP//AAMABw==").unwrap()
        };
        let out_bytes = enriched().into_bytes().unwrap();

        // Adding speeds changes this tile's hash, and thus the build ID of the whole tileset.
        // Like valhalla_add_predicted_traffic, we compute the new build ID
        // from the hashes of every tile in the set
        // (the other tiles are unchanged, so we take their hashes from the original fixtures),
        // then stamp it into the tile we changed.
        let out_hash = OwnedGraphTileHandle::try_from(out_bytes.clone())
            .expect("Unable to parse output tile")
            .header()
            .tile_hash();
        let mut tile_hashes = vec![out_hash];
        for entry in WalkDir::new(&base_dir) {
            let entry = entry.expect("Unable to traverse directory");
            if entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "gph")
                && !entry.path().ends_with("0/003/015.gph")
            {
                let bytes = std::fs::read(entry.path()).expect("Unable to read file");
                let header = OwnedGraphTileHandle::try_from(bytes)
                    .expect("Unable to get tile handle")
                    .header()
                    .clone();
                tile_hashes.push(header.tile_hash());
            }
        }
        assert_eq!(
            tile_hashes.len(),
            7,
            "Expected every tile in the fixture tileset"
        );
        let build_id = BuildId::of_tileset(tile_hashes);
        assert_ne!(
            build_id,
            tile_handle.header().build_id(),
            "Changing a tile should change the build ID"
        );
        let mut out_bytes = out_bytes;
        GraphTileHeader::stamp_build_id(&mut out_bytes, build_id).expect("Unable to stamp");

        let base_dir_with_traffic = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles-with-traffic");
        let expected_out_bytes =
            std::fs::read(base_dir_with_traffic.join("0").join("003").join("015.gph"))
                .expect("Unable to read file");

        if out_bytes != expected_out_bytes {
            let path = std::env::temp_dir().join("valinor-add-predicted-speeds-failed.gph");
            std::fs::write(&path, &out_bytes).expect("Failed to write failed output");
            eprintln!("wrote the mismatched tile to {}", path.display());
        }

        assert_eq!(out_bytes, expected_out_bytes);

        // Supplying the build ID up front must give the same result.
        let out_bytes_with_build_id = enriched().with_build_id(build_id).into_bytes().unwrap();
        assert_eq!(out_bytes_with_build_id, expected_out_bytes);
    }

    #[test]
    fn modified_tile_keeps_edge_bins_and_bounding_circles() {
        // Unlike Valhalla's StoreTileData, rewriting a tile must keep the edge bins
        // and bounding circles, with offsets that agree with the new layout.
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles")
            .join("2")
            .join("000")
            .join("762")
            .join("485.gph");
        let original =
            OwnedGraphTileHandle::try_from(std::fs::read(path).expect("Unable to read file"))
                .expect("Unable to get tile handle");
        assert!(original.header().has_bounding_circles());

        let out_bytes = GraphTileBuilder::from(&original)
            .with_predicted_speed_coefficients(3, &[7; COEFFICIENT_COUNT])
            .unwrap()
            .into_bytes()
            .unwrap();
        // Parsing checks that the circles start right after the bins
        // and end right where the complex restrictions start.
        let modified = OwnedGraphTileHandle::try_from(out_bytes.clone())
            .expect("Unable to parse the modified tile");
        assert!(modified.header().has_bounding_circles());
        assert_eq!(modified.header().predicted_speeds_count(), 1);
        assert_ne!(modified.header().tile_hash(), original.header().tile_hash());
        assert_eq!(modified.header().build_id(), original.header().build_id());
        assert_eq!(
            modified.header().tile_hash(),
            TileHash::of_tile_data(&out_bytes[size_of::<GraphTileHeader>()..])
        );
        for bin in 0..crate::BIN_COUNT {
            assert_eq!(modified.edges_in_bin(bin), original.edges_in_bin(bin));
            assert_eq!(
                modified.bounding_circles_in_bin(bin),
                original.bounding_circles_in_bin(bin)
            );
        }
    }

    #[test]
    fn predicted_speeds_deduplication() {
        let base_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("andorra-tiles");

        let in_bytes = std::fs::read(base_dir.join("0").join("003").join("015.gph"))
            .expect("Unable to read file");
        let tile_handle =
            OwnedGraphTileHandle::try_from(in_bytes.clone()).expect("Unable to get tile handle");

        // Define two different speed profiles
        let profile_a: [i16; COEFFICIENT_COUNT] = [1; 200];
        let profile_b: [i16; COEFFICIENT_COUNT] = [2; 200];

        // Add the same profile to multiple edges
        let builder = GraphTileBuilder::from(&tile_handle)
            .with_predicted_speed_coefficients(0, &profile_a)
            .unwrap()
            .with_predicted_speed_coefficients(1, &profile_a)
            .unwrap() // Duplicate of edge 0
            .with_predicted_speed_coefficients(2, &profile_b)
            .unwrap()
            .with_predicted_speed_coefficients(3, &profile_a)
            .unwrap() // Duplicate of edge 0
            .with_predicted_speed_coefficients(4, &profile_b)
            .unwrap(); // Duplicate of edge 2

        let out_bytes = builder.into_bytes().unwrap();
        let out_handle =
            OwnedGraphTileHandle::try_from(out_bytes).expect("Unable to get output tile handle");
        let out_view = out_handle.borrow_dependent();

        // Verify that we have predicted speeds
        let predicted_speeds = out_view
            .predicted_speeds
            .as_ref()
            .expect("Expected predicted speeds to be present");

        // Verify that edges 0, 1, and 3 share the same offset (profile_a)
        let (offsets, profiles) = predicted_speeds.as_offsets_and_profiles();

        // We added 5 edges but only 2 unique profiles (profile_a and profile_b)
        // So we should have exactly 2 profiles in memory
        let profile_count = profiles.len() / COEFFICIENT_COUNT;
        assert_eq!(
            profile_count, 2,
            "Expected exactly 2 unique speed profiles to be stored"
        );
        assert_eq!(
            offsets[0].get(),
            offsets[1].get(),
            "Edges 0 and 1 should share the same speed profile offset"
        );
        assert_eq!(
            offsets[0].get(),
            offsets[3].get(),
            "Edges 0 and 3 should share the same speed profile offset"
        );

        // Verify that edges 2 and 4 share the same offset (profile_b)
        assert_eq!(
            offsets[2].get(),
            offsets[4].get(),
            "Edges 2 and 4 should share the same speed profile offset"
        );

        // Verify that the two profiles have different offsets
        assert_ne!(
            offsets[0].get(),
            offsets[2].get(),
            "Edges 0 and 2 have different profiles, so they should have different offsets"
        );
    }

    /// Modified tiles that mainline reads back (see `tools/oracle/README.md`).
    mod mainline {
        use super::*;
        use crate::graph_tile::GraphTileView;
        use crate::graph_tile::predicted_speeds::{
            BUCKETS_PER_WEEK, DEFAULT_FLOW_MASK, FlowSource, SPEED_BUCKET_SIZE_SECONDS,
            SpeedBucket, select_speed,
        };
        use std::path::PathBuf;
        use zerocopy::IntoBytes;

        /// The profile in mainline's predicted speed decoding test.
        const MAINLINE_TEST: &str = "BcUACQAEACEADP/7/9sAGf/fAAwAGwARAAX/+AAAAB0AFAAS//AAF//+ACwACQAqAAAAKP/6AEMABABsAAsBBAAq/rcAAP+bAAz/zv/s/9MABf/Y//X/8//9/+wACf/P//EADv/8//L//P/y//H////7AAwAFf/5//oADgAZAAQAFf/3/+8AB//yAB8ABv/0AAUAEf/8//QAFAAG//b////j//v//QAT//7/+f/kABMABwABAAv/6//8//cAEwAAABT/8v/6//wAAAAQ//cACwAFAAT/1//sAAEADAABAAYAE//9AAn/7gAH/+AAFQAB//4AC//o/+gAE//+AAAAFf/l//kABP/+//kACAAG//cAHv/qAB0AAv/4/+v/+wALAAMABP/3AAT/8wAIAAr/9wAK//j/+wAEAAD/+P/8//v/8f/2//L//AALAAcABgAG//gAAv/5AAoAHv//AAcAFf/zABD/7AAUAAv/7v/8AAgACAAN//0ADP/iABD/9f/3//7/+P/3AAQADP//AAMABw==";

        /// The level 2 tile we modify: small, with edge bins and bounding circles.
        const LEVEL_2_TILE: [&str; 4] = ["2", "000", "763", "927.gph"];

        fn fixtures() -> PathBuf {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
        }

        fn read_tile(path: PathBuf) -> OwnedGraphTileHandle {
            OwnedGraphTileHandle::try_from(std::fs::read(path).expect("read tile"))
                .expect("parse tile")
        }

        fn level_2_path(dir: &str) -> PathBuf {
            LEVEL_2_TILE
                .iter()
                .fold(fixtures().join(dir), |path, part| path.join(part))
        }

        /// The edges the modification touches, for a tile with `edge_count` edges.
        fn modified_edges(edge_count: usize) -> [usize; 5] {
            [0, 1, 2, 3, edge_count - 1]
        }

        /// Every way the builder can change a tile, applied to the level 2 tile.
        fn modify(original: &OwnedGraphTileHandle) -> Vec<u8> {
            let [first, second, third, fourth, last] =
                modified_edges(original.directed_edges().len());
            let sawtooth: [f32; BUCKETS_PER_WEEK] =
                core::array::from_fn(|i| f32::from(u8::try_from(i % 100).expect("small")));
            let coefficients =
                crate::graph_tile::predicted_speeds::decode_base64_speed_coefficients(
                    MAINLINE_TEST,
                )
                .expect("valid");
            GraphTileBuilder::from(original)
                .with_average_speeds(first, 50, 40)
                .unwrap()
                .with_predicted_encoded_speeds(second, MAINLINE_TEST)
                .unwrap()
                .with_predicted_speeds(third, &sawtooth)
                .unwrap()
                // The same profile as the second edge, so it's stored once.
                .with_predicted_speed_coefficients(fourth, &coefficients)
                .unwrap()
                .with_average_speeds(fourth, 12, 34)
                .unwrap()
                .with_average_speeds(last, 0, 33)
                .unwrap()
                .into_bytes()
                .unwrap()
        }

        /// The modified level 2 tile is the committed fixture that mainline reads.
        ///
        /// Set `VALINOR_UPDATE_FIXTURES=1` to rewrite the fixture,
        /// then rerun the mainline comparison in `tools/oracle/README.md`.
        #[test]
        fn modified_level_2_tile_is_the_fixture() {
            let original = read_tile(level_2_path("andorra-tiles"));
            assert!(original.header().has_bounding_circles());
            let out = modify(&original);
            let path = level_2_path("builder-modified");
            if std::env::var_os("VALINOR_UPDATE_FIXTURES").is_some() {
                std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
                std::fs::write(&path, &out).expect("write fixture");
            }
            assert!(
                std::fs::read(&path).expect("read fixture") == out,
                "the builder's output changed; see the test docs"
            );
        }

        /// Valinor reads every section of the modified tile identically,
        /// apart from the intended changes.
        #[test]
        fn modified_level_2_tile_keeps_everything_else() {
            let original = read_tile(level_2_path("andorra-tiles"));
            let modified = OwnedGraphTileHandle::try_from(modify(&original)).expect("parse");
            let (a, b): (&GraphTileView, &GraphTileView) =
                (original.borrow_dependent(), modified.borrow_dependent());

            assert_eq!(a.nodes.as_bytes(), b.nodes.as_bytes());
            assert_eq!(a.transitions.as_bytes(), b.transitions.as_bytes());
            assert_eq!(
                a.ext_directed_edges.as_bytes(),
                b.ext_directed_edges.as_bytes()
            );
            assert_eq!(
                a.access_restrictions.as_bytes(),
                b.access_restrictions.as_bytes()
            );
            assert_eq!(a.signs.as_bytes(), b.signs.as_bytes());
            assert_eq!(a.turn_lanes.as_bytes(), b.turn_lanes.as_bytes());
            assert_eq!(a.admins.as_bytes(), b.admins.as_bytes());
            assert_eq!(a.edge_bins.as_bytes(), b.edge_bins.as_bytes());
            assert_eq!(a.bounding_circles.as_bytes(), b.bounding_circles.as_bytes());
            assert_eq!(
                a.complex_forward_restrictions_memory,
                b.complex_forward_restrictions_memory
            );
            assert_eq!(
                a.complex_reverse_restrictions_memory,
                b.complex_reverse_restrictions_memory
            );
            assert_eq!(a.edge_info_memory, b.edge_info_memory);
            assert_eq!(a.text_memory, b.text_memory);
            assert_eq!(a.lane_connectivity_memory, b.lane_connectivity_memory);
            for bin in 0..crate::BIN_COUNT {
                assert_eq!(a.edges_in_bin(bin), b.edges_in_bin(bin));
                assert_eq!(
                    a.bounding_circles_in_bin(bin),
                    b.bounding_circles_in_bin(bin)
                );
            }

            let modified_edges = modified_edges(a.directed_edges().len());
            for (i, (x, y)) in a
                .directed_edges()
                .iter()
                .zip(b.directed_edges())
                .enumerate()
            {
                if !modified_edges.contains(&i) {
                    assert_eq!(x.as_bytes(), y.as_bytes(), "edge {i}");
                }
            }

            let ha = a.header();
            let hb = b.header();
            assert_eq!(ha.version(), hb.version());
            assert_eq!(ha.graph_id(), hb.graph_id());
            assert_eq!(ha.density(), hb.density());
            assert_eq!(ha.has_elevation(), hb.has_elevation());
            assert_eq!(ha.sw_corner(), hb.sw_corner());
            assert_eq!(ha.create_date(), hb.create_date());
            assert_eq!(ha.build_id(), hb.build_id());
            assert_eq!(ha.predicted_speeds_count(), 0);
            // Two distinct profiles; the fourth edge shares the second's.
            assert_eq!(hb.predicted_speeds_count(), 2);
        }

        /// Mainline reads the modified tile, and everything it reads matches the original
        /// apart from the intended changes.
        ///
        /// `mainline-compare.txt` is the C++ helper's `compare` of the original level 2 tile
        /// with the fixture, which reads every section through mainline's accessors.
        #[test]
        fn mainline_reads_the_modified_tile_identically() {
            let compare = std::fs::read_to_string(
                fixtures()
                    .join("builder-modified")
                    .join("mainline-compare.txt"),
            )
            .expect("read mainline-compare.txt");
            let fixture_bytes = std::fs::read(level_2_path("builder-modified")).expect("read");
            let fixture = OwnedGraphTileHandle::try_from(fixture_bytes.clone()).expect("parse");
            let view = fixture.borrow_dependent();
            let modified_edges = modified_edges(view.directed_edges().len());

            let lines: Vec<&str> = compare.lines().collect();
            let [first, differences @ .., last] = &lines[..] else {
                panic!("too short");
            };
            assert!(first.starts_with("compare 2/763927: "), "{first}");
            assert_eq!(*last, format!("{} differences", differences.len() / 2));
            let mut checked_speeds = 0;
            for line in differences {
                let (sign, value) = line.split_at(2);
                assert!(sign == "- " || sign == "+ ", "{line}");
                let words: Vec<&str> = value.split(' ').collect();
                match words[..] {
                    ["header", "counts", "nodes", ..] => {
                        let predicted = if sign == "- " { "0" } else { "2" };
                        assert_eq!(words[6..8], ["predicted", predicted], "{line}");
                    }
                    [
                        "header",
                        "tile_checksum",
                        hash,
                        "build_id",
                        build_id,
                        "end_offset",
                        end,
                    ] => {
                        if sign == "+ " {
                            let header = fixture.header();
                            assert_eq!(hash, header.tile_hash().value().to_string());
                            assert_eq!(build_id, header.build_id().value().to_string());
                            assert_eq!(end, fixture_bytes.len().to_string());
                        }
                    }
                    ["edge", index, kind, ..] => {
                        let index: usize = index.parse().expect("edge index");
                        assert!(modified_edges.contains(&index), "{line}");
                        if kind == "get_speed" {
                            if sign == "+ " {
                                assert_mainline_speeds(view, index, &words[3..]);
                                checked_speeds += 1;
                            }
                        } else {
                            // The directed edge itself, as raw bytes.
                            assert_eq!(words.len(), 3, "{line}");
                        }
                    }
                    _ => panic!("unexpected difference: {line}"),
                }
            }
            assert_eq!(checked_speeds, modified_edges.len());
        }

        /// Checks mainline's `get_speed ...` line for an edge of the fixture against Valinor.
        fn assert_mainline_speeds(view: &GraphTileView, index: usize, words: &[&str]) {
            let [
                "default",
                default,
                "night",
                night,
                "day",
                day,
                "predicted",
                predicted @ ..,
            ] = words
            else {
                panic!("bad speeds {words:?}");
            };
            let edge = &view.directed_edges()[index];
            let ps = view.predicted_speeds.as_ref();
            let select = |mask, seconds| {
                select_speed(edge, mask, seconds, false, |s| {
                    ps.expect("profiles").try_speed(index, u64::from(s))
                })
                .expect("speed")
                .kph
                .to_string()
            };
            assert_eq!(*default, select(DEFAULT_FLOW_MASK, None), "edge {index}");
            assert_eq!(
                *night,
                select(DEFAULT_FLOW_MASK, Some(3600)),
                "edge {index}"
            );
            assert_eq!(
                *day,
                select(DEFAULT_FLOW_MASK, Some(43_200)),
                "edge {index}"
            );
            if edge.has_predicted_speed() {
                assert_eq!(predicted.len(), BUCKETS_PER_WEEK);
                for (bucket, expected) in SpeedBucket::all().zip(predicted) {
                    let seconds = u64::from(SPEED_BUCKET_SIZE_SECONDS)
                        * u64::try_from(bucket.index()).expect("small");
                    assert_eq!(
                        *expected,
                        select(FlowSource::Predicted.into(), Some(seconds)),
                        "edge {index} {bucket:?}"
                    );
                }
            } else {
                assert!(predicted.is_empty());
            }
        }
    }

    #[test]
    fn version_must_fit_mainlines_string() {
        let tile = OwnedGraphTileHandle::try_from(
            std::fs::read(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("fixtures/andorra-tiles/2/000/763/927.gph"),
            )
            .expect("read"),
        )
        .expect("parse");
        assert_eq!(tile.header().version(), "3.9.1");

        let out = GraphTileBuilder::from(&tile)
            .with_version("3.9.1-valinor-x")
            .unwrap()
            .into_bytes()
            .unwrap();
        let out = OwnedGraphTileHandle::try_from(out).expect("parse");
        assert_eq!(out.header().version(), "3.9.1-valinor-x");

        for bad in ["sixteen bytes!!!", "nul\0inside"] {
            assert!(matches!(
                GraphTileBuilder::from(&tile).with_version(bad),
                Err(GraphTileBuildError::InvalidVersionString(_))
            ));
        }
    }

    #[test]
    fn edge_indices_out_of_range_are_errors() {
        let tile = OwnedGraphTileHandle::try_from(
            std::fs::read(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("fixtures/andorra-tiles/2/000/763/927.gph"),
            )
            .expect("read"),
        )
        .expect("parse");
        let count = tile.directed_edges().len();
        for index in [count, usize::MAX] {
            assert!(matches!(
                GraphTileBuilder::from(&tile).with_average_speeds(index, 1, 1),
                Err(GraphTileBuildError::InvalidIndex(_))
            ));
            assert!(matches!(
                GraphTileBuilder::from(&tile)
                    .with_predicted_speed_coefficients(index, &[0; COEFFICIENT_COUNT]),
                Err(GraphTileBuildError::InvalidIndex(_))
            ));
        }
    }

    /// Adding profiles to a tile that already has some
    /// reuses identical existing profiles and keeps the others.
    #[test]
    fn add_profiles_to_a_tile_with_profiles() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/andorra-tiles-with-traffic/0/003/015.gph");
        let tile =
            OwnedGraphTileHandle::try_from(std::fs::read(path).expect("read")).expect("parse");
        let view = tile.borrow_dependent();
        let speeds = view.predicted_speeds.as_ref().expect("profiles");
        let profile_7 = speeds.coefficients(7).unwrap().map(zerocopy::I16::get);
        let profile_42 = speeds.coefficients(42).unwrap().map(zerocopy::I16::get);
        let new_profile = [3; COEFFICIENT_COUNT];

        let out = GraphTileBuilder::from(&tile)
            // An existing profile on a new edge: no new profile.
            .with_predicted_speed_coefficients(100, &profile_7)
            .unwrap()
            // A new profile replacing an edge's existing one.
            .with_predicted_speed_coefficients(42, &new_profile)
            .unwrap()
            .into_bytes()
            .unwrap();
        let out = OwnedGraphTileHandle::try_from(out).expect("parse");
        let out_view = out.borrow_dependent();
        assert_eq!(out.header().predicted_speeds_count(), 3);
        let out_speeds = out_view.predicted_speeds.as_ref().expect("profiles");
        let get = |edge| {
            out_speeds
                .coefficients(edge)
                .unwrap()
                .map(zerocopy::I16::get)
        };
        assert_eq!(get(7), profile_7);
        assert_eq!(get(100), profile_7);
        assert_eq!(get(42), new_profile);
        assert_ne!(get(42), profile_42);
        assert!(out_view.directed_edges()[100].has_predicted_speed());
        // Edges the builder didn't touch are unchanged.
        for (i, (a, b)) in view
            .directed_edges()
            .iter()
            .zip(out_view.directed_edges())
            .enumerate()
        {
            if i != 100 {
                assert_eq!(
                    zerocopy::IntoBytes::as_bytes(a),
                    zerocopy::IntoBytes::as_bytes(b),
                    "edge {i}"
                );
            }
        }
    }
}
