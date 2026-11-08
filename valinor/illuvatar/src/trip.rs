//! Read-only views over a trip leg, mirroring mainline's `EnhancedTripLeg` (`enhancedtrippath.cc`).
//!
//! Mainline wraps each protobuf message in a class with helper predicates.
//! Here the same helpers are extension traits on the generated `prost` types,
//! plus free functions for the index arithmetic over a leg's nodes.
//!
//! Enumerations stay as the raw `i32` values that `prost` stores,
//! so values this build does not know compare unequal to every known value,
//! exactly as in mainline.

use crate::turn::{
    SIMILAR_TURN_DEGREE_THRESHOLD, TurnType, is_fork_forward, is_forward, is_relative_straight,
    is_similar_turn_degree, is_wider_forward, turn_degree, turn_degree_delta,
};
use std::sync::LazyLock;
use valhalla_proto::trip_leg::{Edge, IntersectingEdge, Node, Traversability, Use, node};
use valhalla_proto::{RoadClass, TravelMode, TripLeg, TurnLane, tagged_value, turn_lane};

/// Mainline's backward cone for intersecting edges, exclusive on both ends.
const BACKWARD_TURN_DEGREE_LOWER_BOUND: u32 = 124;
const BACKWARD_TURN_DEGREE_UPPER_BOUND: u32 = 236;
/// The maximum road class delta for an intersecting edge to count as significant.
const SIGNIFICANT_ROAD_CLASS_THRESHOLD: i32 = 2;
/// The maximum turn degree delta for two edges to count as similarly straight.
const SIMILAR_STRAIGHT_THRESHOLD: u32 = 30;
/// The buffer between straight deltas when deciding which edge is the straightest.
const IS_STRAIGHTEST_BUFFER: u32 = 10;

/// The edge that mainline's `mutable_edge()` would create for a node without one.
static DEFAULT_EDGE: LazyLock<Edge> = LazyLock::new(Edge::default);

/// Turn lane direction bits (`baldr/turnlanes.h`).
pub(crate) mod lane {
    pub(crate) const EMPTY: u16 = 0;
    pub(crate) const NONE: u16 = 1 << 0;
    pub(crate) const THROUGH: u16 = 1 << 1;
    pub(crate) const SHARP_LEFT: u16 = 1 << 2;
    pub(crate) const LEFT: u16 = 1 << 3;
    pub(crate) const SLIGHT_LEFT: u16 = 1 << 4;
    pub(crate) const SLIGHT_RIGHT: u16 = 1 << 5;
    pub(crate) const RIGHT: u16 = 1 << 6;
    pub(crate) const SHARP_RIGHT: u16 = 1 << 7;
    pub(crate) const REVERSE: u16 = 1 << 8;
}

/// Right and left intersecting edge counts at a node (`IntersectingEdgeCounts`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct IntersectingEdgeCounts {
    pub(crate) right: u32,
    pub(crate) right_similar: u32,
    pub(crate) right_traversable_outbound: u32,
    pub(crate) right_similar_traversable_outbound: u32,
    pub(crate) left: u32,
    pub(crate) left_similar: u32,
    pub(crate) left_traversable_outbound: u32,
    pub(crate) left_similar_traversable_outbound: u32,
}

/// Index arithmetic over a leg's nodes (`EnhancedTripLeg`).
///
/// A node's edge leaves that node, so the last node has no edge of its own.
pub(crate) trait LegExt {
    /// The node at an index.
    fn node_at(&self, index: usize) -> Option<&Node>;
    /// The edge entering the node at `node_index`, `delta` edges back (`GetPrevEdge`).
    fn prev_edge(&self, node_index: usize, delta: usize) -> Option<&Edge>;
    /// The edge leaving the node at `node_index` (`GetCurrEdge`).
    fn curr_edge(&self, node_index: usize) -> Option<&Edge>;
    /// The edge leaving the node `delta` nodes after `node_index` (`GetNextEdge`).
    fn next_edge(&self, node_index: usize, delta: usize) -> Option<&Edge>;
    /// The index of the last node; zero for an empty leg.
    fn last_node_index(&self) -> usize;
    /// Whether an index is that of the last node.
    fn is_last_node_index(&self, index: usize) -> bool;
    /// The country code of the admin region at a node, or empty if there is none.
    fn country_code(&self, node_index: usize) -> &str;
    /// The state code of the admin region at a node, or empty if there is none.
    fn state_code(&self, node_index: usize) -> &str;
    /// The elapsed time at a node, in seconds.
    fn elapsed_seconds(&self, node_index: usize) -> f64;
    /// The leg length in kilometers or miles (`EnhancedTripLeg::GetLength`).
    fn length(&self, miles: bool) -> f32;
}

impl LegExt for TripLeg {
    #[inline]
    fn node_at(&self, index: usize) -> Option<&Node> {
        self.node.get(index)
    }

    #[inline]
    fn prev_edge(&self, node_index: usize, delta: usize) -> Option<&Edge> {
        let index = node_index.checked_sub(delta)?;
        let node = self.node.get(index)?;
        Some(node.edge.as_ref().unwrap_or(&DEFAULT_EDGE))
    }

    #[inline]
    fn curr_edge(&self, node_index: usize) -> Option<&Edge> {
        self.next_edge(node_index, 0)
    }

    #[inline]
    fn next_edge(&self, node_index: usize, delta: usize) -> Option<&Edge> {
        let index = node_index.checked_add(delta)?;
        if index + 1 >= self.node.len() {
            return None;
        }
        let node = self.node.get(index)?;
        Some(node.edge.as_ref().unwrap_or(&DEFAULT_EDGE))
    }

    #[inline]
    fn last_node_index(&self) -> usize {
        self.node.len().saturating_sub(1)
    }

    #[inline]
    fn is_last_node_index(&self, index: usize) -> bool {
        index < self.node.len() && index + 1 == self.node.len()
    }

    fn country_code(&self, node_index: usize) -> &str {
        self.node
            .get(node_index)
            .and_then(|node| self.admin.get(usize::try_from(node.admin_index).ok()?))
            .map_or("", |admin| admin.country_code.as_str())
    }

    fn state_code(&self, node_index: usize) -> &str {
        self.node
            .get(node_index)
            .and_then(|node| self.admin.get(usize::try_from(node.admin_index).ok()?))
            .map_or("", |admin| admin.state_code.as_str())
    }

    fn elapsed_seconds(&self, node_index: usize) -> f64 {
        self.node
            .get(node_index)
            .and_then(|node| node.cost.as_ref())
            .and_then(|cost| cost.elapsed_cost.as_ref())
            .map_or(0.0, |cost| cost.seconds)
    }

    fn length(&self, miles: bool) -> f32 {
        // Sum in node order as single precision, as mainline does.
        let km = self
            .node
            .iter()
            .filter_map(|node| node.edge.as_ref())
            .fold(0.0_f32, |sum, edge| sum + edge.length_km);
        if miles { km * MILES_PER_KM } else { km }
    }
}

/// Mainline's `midgard::kMilePerKm`.
pub(crate) const MILES_PER_KM: f32 = 0.621_371;

/// Mutable access to a leg's edges, for the passes that update turn lanes and headings.
pub(crate) fn edge_mut(leg: &mut TripLeg, node_index: usize) -> Option<&mut Edge> {
    // Like `mutable_edge()`, this creates the edge if the node has none.
    leg.node
        .get_mut(node_index)
        .map(|node| node.edge.get_or_insert_default())
}

/// Predicates on a path edge (`EnhancedTripLeg_Edge`).
pub(crate) trait EdgeExt {
    fn is_unnamed(&self) -> bool;
    fn is_road_use(&self) -> bool;
    fn is_ramp_use(&self) -> bool;
    fn is_turn_channel_use(&self) -> bool;
    fn is_cycleway_use(&self) -> bool;
    fn is_mountain_bike_use(&self) -> bool;
    fn is_footway_use(&self) -> bool;
    fn is_pedestrian_crossing_use(&self) -> bool;
    fn is_elevator_use(&self) -> bool;
    fn is_steps_use(&self) -> bool;
    fn is_escalator_use(&self) -> bool;
    fn is_ferry_use(&self) -> bool;
    fn is_rail_ferry_use(&self) -> bool;
    fn is_egress_connection_use(&self) -> bool;
    fn is_platform_connection_use(&self) -> bool;
    fn is_transit_connection_use(&self) -> bool;
    fn is_transit_connection(&self) -> bool;
    fn is_unnamed_walkway(&self) -> bool;
    fn is_unnamed_cycleway(&self) -> bool;
    fn is_unnamed_mountain_bike_trail(&self) -> bool;
    fn is_highway(&self) -> bool;
    fn is_oneway(&self) -> bool;
    fn is_straightest(&self, turn_degree: u32, straightest_xedge_turn_degree: u32) -> bool;
    fn drive_on_right(&self) -> bool;
    fn has_mode(&self, mode: TravelMode) -> bool;
    fn has_level_changes(&self) -> bool;
    fn traverses_levels(&self) -> bool;
    fn level_refs(&self) -> Vec<String>;
    fn has_turn_lane(&self, direction: u16) -> bool;
    fn has_non_directional_turn_lane(&self) -> bool;
}

impl EdgeExt for Edge {
    #[inline]
    fn is_unnamed(&self) -> bool {
        self.name.is_empty()
    }

    #[inline]
    fn is_road_use(&self) -> bool {
        self.r#use == Use::KRoadUse as i32 || self.r#use == Use::KServiceRoadUse as i32
    }

    #[inline]
    fn is_ramp_use(&self) -> bool {
        self.r#use == Use::KRampUse as i32
    }

    #[inline]
    fn is_turn_channel_use(&self) -> bool {
        self.r#use == Use::KTurnChannelUse as i32
    }

    #[inline]
    fn is_cycleway_use(&self) -> bool {
        self.r#use == Use::KCyclewayUse as i32
    }

    #[inline]
    fn is_mountain_bike_use(&self) -> bool {
        self.r#use == Use::KMountainBikeUse as i32
    }

    #[inline]
    fn is_footway_use(&self) -> bool {
        // Mainline counts crossings as footways.
        self.r#use == Use::KFootwayUse as i32 || self.r#use == Use::KPedestrianCrossingUse as i32
    }

    #[inline]
    fn is_pedestrian_crossing_use(&self) -> bool {
        self.r#use == Use::KPedestrianCrossingUse as i32
    }

    #[inline]
    fn is_elevator_use(&self) -> bool {
        self.r#use == Use::KElevatorUse as i32
    }

    #[inline]
    fn is_steps_use(&self) -> bool {
        self.r#use == Use::KStepsUse as i32
    }

    #[inline]
    fn is_escalator_use(&self) -> bool {
        self.r#use == Use::KEscalatorUse as i32
    }

    #[inline]
    fn is_ferry_use(&self) -> bool {
        self.r#use == Use::KFerryUse as i32
    }

    #[inline]
    fn is_rail_ferry_use(&self) -> bool {
        self.r#use == Use::KRailFerryUse as i32
    }

    #[inline]
    fn is_egress_connection_use(&self) -> bool {
        self.r#use == Use::KEgressConnectionUse as i32
    }

    #[inline]
    fn is_platform_connection_use(&self) -> bool {
        self.r#use == Use::KPlatformConnectionUse as i32
    }

    #[inline]
    fn is_transit_connection_use(&self) -> bool {
        self.r#use == Use::KTransitConnectionUse as i32
    }

    #[inline]
    fn is_transit_connection(&self) -> bool {
        self.is_transit_connection_use()
            || self.is_egress_connection_use()
            || self.is_platform_connection_use()
    }

    #[inline]
    fn is_unnamed_walkway(&self) -> bool {
        self.is_unnamed() && self.is_footway_use()
    }

    #[inline]
    fn is_unnamed_cycleway(&self) -> bool {
        self.is_unnamed() && self.is_cycleway_use()
    }

    #[inline]
    fn is_unnamed_mountain_bike_trail(&self) -> bool {
        self.is_unnamed() && self.is_mountain_bike_use()
    }

    #[inline]
    fn is_highway(&self) -> bool {
        self.road_class == RoadClass::KMotorway as i32
            && !self.is_ramp_use()
            && !self.is_turn_channel_use()
    }

    #[inline]
    fn is_oneway(&self) -> bool {
        self.traversability == Traversability::KForward as i32
            || self.traversability == Traversability::KBackward as i32
    }

    fn is_straightest(&self, turn_degree: u32, straightest_xedge_turn_degree: u32) -> bool {
        if !is_wider_forward(turn_degree) {
            return false;
        }
        let path_straight_delta = straight_delta(turn_degree);
        let xedge_straight_delta = straight_delta(straightest_xedge_turn_degree);
        if turn_degree_delta(path_straight_delta, xedge_straight_delta) > IS_STRAIGHTEST_BUFFER {
            path_straight_delta <= xedge_straight_delta
        } else {
            true
        }
    }

    #[inline]
    fn drive_on_right(&self) -> bool {
        !self.drive_on_left
    }

    #[inline]
    fn has_mode(&self, mode: TravelMode) -> bool {
        self.travel_mode == mode as i32
    }

    #[inline]
    fn has_level_changes(&self) -> bool {
        // Mainline's `has_level_changes` (maneuversbuilder.cc):
        // one level that starts and ends differently, or several levels.
        match self.levels.as_slice() {
            [] => false,
            [level] => level.start != level.end,
            _ => true,
        }
    }

    #[inline]
    fn traverses_levels(&self) -> bool {
        // The same as `has_level_changes`, spelled differently in mainline.
        self.has_level_changes()
    }

    fn level_refs(&self) -> Vec<String> {
        let mut level_refs = Vec::new();
        let mut levels = Vec::new();
        for tagged in &self.tagged_value {
            if tagged.r#type == tagged_value::Type::KLevelRef as i32 {
                level_refs.push(String::from_utf8_lossy(&tagged.value).into_owned());
            } else if tagged.r#type == tagged_value::Type::KLevels as i32
                && let Some((decoded, precision)) = decode_levels(&tagged.value)
                && let [(start, end)] = decoded.as_slice()
                && start == end
            {
                // Mainline formats the level as `std::fixed` with `precision` digits.
                let digits = usize::try_from(precision).unwrap_or(usize::MAX);
                levels.push(format!("Level {:.*}", digits, f64::from(*start)));
            }
        }
        if level_refs.is_empty() {
            levels
        } else {
            level_refs
        }
    }

    fn has_turn_lane(&self, direction: u16) -> bool {
        self.turn_lanes
            .iter()
            .any(|lane| lane.directions_mask & u32::from(direction) != 0)
    }

    fn has_non_directional_turn_lane(&self) -> bool {
        self.turn_lanes.iter().any(|lane| {
            lane.directions_mask == u32::from(lane::EMPTY)
                || lane.directions_mask & u32::from(lane::NONE) != 0
        })
    }
}

/// How far a turn degree is from straight ahead, in `0..=180`.
#[inline]
const fn straight_delta(turn_degree: u32) -> u32 {
    if turn_degree > 180 {
        360_u32.wrapping_sub(turn_degree)
    } else {
        turn_degree
    }
}

/// Decodes a `kLevels` tagged value (`baldr::decode_levels`).
///
/// Returns the level ranges and the precision mainline reports, or `None` if the value is malformed.
/// Mainline would read past the end of a malformed value;
/// we treat it as having no levels instead.
///
/// Mainline has a precedence bug here: `if (int32_t prec_power = parse_varint(ptr) > 0)`
/// binds the *comparison* to `prec_power`, so the precision is 10 whenever the encoded power is
/// positive, and 0 otherwise.
/// Since the precision ends up in instruction text (as the number of decimal places),
/// we reproduce it.
fn decode_levels(encoded: &[u8]) -> Option<(Vec<(f32, f32)>, u32)> {
    /// Mainline's `kLevelRangeSeparator`, compared as a float.
    const LEVEL_RANGE_SEPARATOR: f32 = 1_048_575.0;

    let mut pos = 0;
    let size = parse_varint(encoded, &mut pos)?;
    let end = pos.checked_add(usize::try_from(size).ok()?)?;
    if end > encoded.len() {
        return None;
    }
    let precision: u32 = if parse_varint(encoded, &mut pos)? > 0 {
        10
    } else {
        0
    };

    let mut decoded: Vec<(f32, f32)> = Vec::with_capacity(4);
    let mut in_range = false;
    while pos != end {
        if pos > end {
            return None;
        }
        let value = parse_varint(&encoded[..end], &mut pos)?;
        #[expect(
            clippy::cast_precision_loss,
            reason = "mainline converts to float the same way"
        )]
        let value = value as f32;
        // Mainline compares the converted value exactly.
        let is_separator = value == LEVEL_RANGE_SEPARATOR;
        if is_separator {
            in_range = false;
            continue;
        }
        #[expect(clippy::cast_precision_loss, reason = "mainline divides as float")]
        let level = if precision == 0 {
            value
        } else {
            value / precision as f32
        };
        if in_range {
            if let Some(last) = decoded.last_mut() {
                last.1 = level;
            }
        } else {
            decoded.push((level, level));
            in_range = true;
        }
    }
    Some((decoded, precision))
}

/// Parses one zigzag-encoded 7-bit varint (`parse_varint` in `edgeinfo.cc`).
fn parse_varint(encoded: &[u8], pos: &mut usize) -> Option<i32> {
    let mut result: i32 = 0;
    let mut shift = 0_u32;
    loop {
        let byte = *encoded.get(*pos)?;
        *pos += 1;
        if shift >= 32 {
            return None;
        }
        result |= i32::from(byte & 0x7f).checked_shl(shift)?;
        shift += 7;
        if byte & 0x80 == 0 {
            break;
        }
    }
    Some((if result & 1 != 0 { !result } else { result }) >> 1)
}

/// Predicates on an intersecting edge (`EnhancedTripLeg_IntersectingEdge`).
pub(crate) trait IntersectingEdgeExt {
    fn traversability(&self, mode: i32) -> i32;
    fn is_traversable(&self, mode: i32) -> bool;
    fn is_traversable_outbound(&self, mode: i32) -> bool;
    fn is_highway(&self) -> bool;
}

impl IntersectingEdgeExt for IntersectingEdge {
    #[inline]
    fn traversability(&self, mode: i32) -> i32 {
        if mode == TravelMode::KDrive as i32 {
            self.driveability
        } else if mode == TravelMode::KBicycle as i32 {
            self.cyclability
        } else {
            self.walkability
        }
    }

    #[inline]
    fn is_traversable(&self, mode: i32) -> bool {
        self.traversability(mode) != Traversability::KNone as i32
    }

    #[inline]
    fn is_traversable_outbound(&self, mode: i32) -> bool {
        let traversability = self.traversability(mode);
        traversability == Traversability::KForward as i32
            || traversability == Traversability::KBoth as i32
    }

    #[inline]
    fn is_highway(&self) -> bool {
        self.road_class == RoadClass::KMotorway as i32
            && !(self.r#use == Use::KRampUse as i32 || self.r#use == Use::KTurnChannelUse as i32)
    }
}

/// Predicates on a path node and its intersecting edges (`EnhancedTripLeg_Node`).
///
/// `from_heading` is always the end heading of the edge entering the node,
/// and `mode` a `TravelMode` value.
pub(crate) trait NodeExt {
    fn is_type(&self, node_type: node::Type) -> bool;
    fn path_edge(&self) -> &Edge;
    fn has_intersecting_edges(&self) -> bool;
    fn has_intersecting_edge_name_consistency(&self) -> bool;
    fn has_intersecting_edge_curr_name_consistency(&self) -> bool;
    fn has_non_backward_traversable_same_name_ramp_intersecting_edge(
        &self,
        from_heading: u32,
        mode: i32,
    ) -> bool;
    fn intersecting_edge_counts(&self, from_heading: u32, mode: i32) -> IntersectingEdgeCounts;
    fn has_forward_traversable_intersecting_edge(&self, from_heading: u32, mode: i32) -> bool;
    fn has_road_fork_traversable_intersecting_edge(
        &self,
        from_heading: u32,
        mode: i32,
        allow_service_road: bool,
    ) -> bool;
    fn has_forward_traversable_significant_road_class_xedge(
        &self,
        from_heading: u32,
        mode: i32,
        path_road_class: i32,
    ) -> bool;
    fn has_forward_traversable_use_xedge(&self, from_heading: u32, mode: i32, r#use: Use) -> bool;
    fn has_similar_straight_significant_road_class_xedge(
        &self,
        path_turn_degree: u32,
        from_heading: u32,
        mode: i32,
        path_road_class: i32,
    ) -> bool;
    fn has_similar_straight_non_ramp_or_same_name_ramp_xedge(
        &self,
        path_turn_degree: u32,
        from_heading: u32,
        mode: i32,
    ) -> bool;
    fn has_only_forward_traversable_road_class_xedges(
        &self,
        from_heading: u32,
        mode: i32,
        path_road_class: i32,
    ) -> bool;
    fn has_wider_forward_traversable_intersecting_edge(&self, from_heading: u32, mode: i32)
    -> bool;
    fn has_wider_forward_traversable_highway_xedge(&self, from_heading: u32, mode: i32) -> bool;
    fn has_traversable_intersecting_edge(&self, mode: i32) -> bool;
    fn has_traversable_outbound_intersecting_edge(&self, mode: i32) -> bool;
    fn has_traversable_exclude_use_xedge(&self, mode: i32, exclude_use: Use) -> bool;
    fn has_forward_traversable_exclude_use_xedge(
        &self,
        from_heading: u32,
        mode: i32,
        exclude_use: Use,
    ) -> bool;
    fn has_specified_turn_xedge(&self, turn_type: TurnType, from_heading: u32, mode: i32) -> bool;
    fn has_specified_road_class_xedge(&self, road_class: RoadClass) -> bool;
    fn straightest_intersecting_edge_turn_degree(&self, from_heading: u32) -> u32;
    fn straightest_traversable_intersecting_edge(
        &self,
        from_heading: u32,
        mode: i32,
    ) -> (u32, Option<i32>);
    fn is_straightest_traversable_intersecting_edge_reversed(
        &self,
        from_heading: u32,
        mode: i32,
    ) -> bool;
    fn right_most_turn_degree(&self, turn_degree: u32, from_heading: u32, mode: i32) -> u32;
    fn left_most_turn_degree(&self, turn_degree: u32, from_heading: u32, mode: i32) -> u32;
}

impl NodeExt for Node {
    #[inline]
    fn is_type(&self, node_type: node::Type) -> bool {
        self.r#type == node_type as i32
    }

    #[inline]
    fn path_edge(&self) -> &Edge {
        self.edge.as_ref().unwrap_or(&DEFAULT_EDGE)
    }

    #[inline]
    fn has_intersecting_edges(&self) -> bool {
        !self.intersecting_edge.is_empty()
    }

    fn has_intersecting_edge_name_consistency(&self) -> bool {
        self.intersecting_edge
            .iter()
            .any(|xedge| xedge.curr_name_consistency || xedge.prev_name_consistency)
    }

    fn has_intersecting_edge_curr_name_consistency(&self) -> bool {
        self.intersecting_edge
            .iter()
            .any(|xedge| xedge.curr_name_consistency)
    }

    fn has_non_backward_traversable_same_name_ramp_intersecting_edge(
        &self,
        from_heading: u32,
        mode: i32,
    ) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            if (xedge.prev_name_consistency || xedge.curr_name_consistency)
                && xedge.is_traversable(mode)
                && xedge.r#use == Use::KRampUse as i32
            {
                let degree = turn_degree(from_heading, xedge.begin_heading);
                !(degree > BACKWARD_TURN_DEGREE_LOWER_BOUND
                    && degree < BACKWARD_TURN_DEGREE_UPPER_BOUND)
            } else {
                false
            }
        })
    }

    fn intersecting_edge_counts(&self, from_heading: u32, mode: i32) -> IntersectingEdgeCounts {
        let mut counts = IntersectingEdgeCounts::default();
        if self.intersecting_edge.is_empty() {
            return counts;
        }
        let path_turn_degree = turn_degree(from_heading, self.path_edge().begin_heading);
        for xedge in &self.intersecting_edge {
            let xdegree = turn_degree(from_heading, xedge.begin_heading);
            let outbound = xedge.is_traversable_outbound(mode);
            let (is_right, is_left) = if path_turn_degree > 180 {
                (
                    xdegree > path_turn_degree || xdegree < 180,
                    xdegree < path_turn_degree && xdegree > 180,
                )
            } else {
                (
                    xdegree > path_turn_degree && xdegree < 180,
                    xdegree < path_turn_degree || xdegree > 180,
                )
            };
            if is_right {
                counts.right += 1;
                if is_similar_turn_degree(
                    path_turn_degree,
                    xdegree,
                    true,
                    SIMILAR_TURN_DEGREE_THRESHOLD,
                ) {
                    counts.right_similar += 1;
                    if outbound {
                        counts.right_similar_traversable_outbound += 1;
                    }
                }
                if outbound {
                    counts.right_traversable_outbound += 1;
                }
            } else if is_left {
                counts.left += 1;
                if is_similar_turn_degree(
                    path_turn_degree,
                    xdegree,
                    false,
                    SIMILAR_TURN_DEGREE_THRESHOLD,
                ) {
                    counts.left_similar += 1;
                    if outbound {
                        counts.left_similar_traversable_outbound += 1;
                    }
                }
                if outbound {
                    counts.left_traversable_outbound += 1;
                }
            }
        }
        counts
    }

    fn has_forward_traversable_intersecting_edge(&self, from_heading: u32, mode: i32) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            is_forward(turn_degree(from_heading, xedge.begin_heading))
                && xedge.is_traversable_outbound(mode)
        })
    }

    fn has_road_fork_traversable_intersecting_edge(
        &self,
        from_heading: u32,
        mode: i32,
        allow_service_road: bool,
    ) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            is_fork_forward(turn_degree(from_heading, xedge.begin_heading))
                && xedge.is_traversable_outbound(mode)
                && xedge.prev_name_consistency
                && xedge.r#use != Use::KRampUse as i32
                && xedge.r#use != Use::KTurnChannelUse as i32
                && xedge.r#use != Use::KFerryUse as i32
                && xedge.r#use != Use::KRailFerryUse as i32
                && (allow_service_road || xedge.road_class != RoadClass::KServiceOther as i32)
        })
    }

    fn has_forward_traversable_significant_road_class_xedge(
        &self,
        from_heading: u32,
        mode: i32,
        path_road_class: i32,
    ) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            is_forward(turn_degree(from_heading, xedge.begin_heading))
                && xedge.is_traversable_outbound(mode)
                && xedge.road_class.wrapping_sub(path_road_class)
                    <= SIGNIFICANT_ROAD_CLASS_THRESHOLD
        })
    }

    fn has_forward_traversable_use_xedge(&self, from_heading: u32, mode: i32, r#use: Use) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            is_forward(turn_degree(from_heading, xedge.begin_heading))
                && xedge.is_traversable_outbound(mode)
                && xedge.r#use == r#use as i32
        })
    }

    fn has_similar_straight_significant_road_class_xedge(
        &self,
        path_turn_degree: u32,
        from_heading: u32,
        mode: i32,
        path_road_class: i32,
    ) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            let xdegree = turn_degree(from_heading, xedge.begin_heading);
            is_relative_straight(path_turn_degree)
                && is_relative_straight(xdegree)
                && xedge.is_traversable_outbound(mode)
                && turn_degree_delta(path_turn_degree, xdegree) <= SIMILAR_STRAIGHT_THRESHOLD
                && xedge.road_class.wrapping_sub(path_road_class)
                    <= SIGNIFICANT_ROAD_CLASS_THRESHOLD
        })
    }

    fn has_similar_straight_non_ramp_or_same_name_ramp_xedge(
        &self,
        path_turn_degree: u32,
        from_heading: u32,
        mode: i32,
    ) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            let xdegree = turn_degree(from_heading, xedge.begin_heading);
            let is_ramp = xedge.r#use == Use::KRampUse as i32;
            is_relative_straight(path_turn_degree)
                && is_relative_straight(xdegree)
                && xedge.is_traversable_outbound(mode)
                && turn_degree_delta(path_turn_degree, xdegree) <= SIMILAR_STRAIGHT_THRESHOLD
                && (!is_ramp || xedge.prev_name_consistency)
        })
    }

    fn has_only_forward_traversable_road_class_xedges(
        &self,
        from_heading: u32,
        mode: i32,
        path_road_class: i32,
    ) -> bool {
        if self.intersecting_edge.is_empty() {
            return false;
        }
        self.intersecting_edge.iter().all(|xedge| {
            let excluded_use = xedge.r#use == Use::KRampUse as i32
                || xedge.r#use == Use::KTurnChannelUse as i32
                || xedge.r#use == Use::KFerryUse as i32
                || xedge.r#use == Use::KRailFerryUse as i32;
            !excluded_use
                && path_road_class >= xedge.road_class
                && is_fork_forward(turn_degree(from_heading, xedge.begin_heading))
                && xedge.is_traversable_outbound(mode)
        })
    }

    fn has_wider_forward_traversable_intersecting_edge(
        &self,
        from_heading: u32,
        mode: i32,
    ) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            is_wider_forward(turn_degree(from_heading, xedge.begin_heading))
                && xedge.is_traversable_outbound(mode)
        })
    }

    fn has_wider_forward_traversable_highway_xedge(&self, from_heading: u32, mode: i32) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            is_wider_forward(turn_degree(from_heading, xedge.begin_heading))
                && xedge.is_traversable_outbound(mode)
                && xedge.is_highway()
        })
    }

    fn has_traversable_intersecting_edge(&self, mode: i32) -> bool {
        self.intersecting_edge
            .iter()
            .any(|xedge| xedge.is_traversable(mode))
    }

    fn has_traversable_outbound_intersecting_edge(&self, mode: i32) -> bool {
        self.intersecting_edge
            .iter()
            .any(|xedge| xedge.is_traversable_outbound(mode))
    }

    fn has_traversable_exclude_use_xedge(&self, mode: i32, exclude_use: Use) -> bool {
        self.intersecting_edge
            .iter()
            .any(|xedge| xedge.is_traversable_outbound(mode) && xedge.r#use != exclude_use as i32)
    }

    fn has_forward_traversable_exclude_use_xedge(
        &self,
        from_heading: u32,
        mode: i32,
        exclude_use: Use,
    ) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            is_forward(turn_degree(from_heading, xedge.begin_heading))
                && xedge.is_traversable_outbound(mode)
                && xedge.r#use != exclude_use as i32
        })
    }

    fn has_specified_turn_xedge(&self, turn_type: TurnType, from_heading: u32, mode: i32) -> bool {
        self.intersecting_edge.iter().any(|xedge| {
            xedge.is_traversable_outbound(mode)
                && TurnType::of(turn_degree(from_heading, xedge.begin_heading)) == turn_type
        })
    }

    fn has_specified_road_class_xedge(&self, road_class: RoadClass) -> bool {
        self.intersecting_edge
            .iter()
            .any(|xedge| xedge.road_class == road_class as i32)
    }

    fn straightest_intersecting_edge_turn_degree(&self, from_heading: u32) -> u32 {
        let mut straightest_turn_degree = 180;
        let mut straightest_delta = 180;
        for xedge in &self.intersecting_edge {
            let xdegree = turn_degree(from_heading, xedge.begin_heading);
            let delta = straight_delta(xdegree);
            if delta < straightest_delta {
                straightest_delta = delta;
                straightest_turn_degree = xdegree;
            }
        }
        straightest_turn_degree
    }

    fn straightest_traversable_intersecting_edge(
        &self,
        from_heading: u32,
        mode: i32,
    ) -> (u32, Option<i32>) {
        let mut straightest_turn_degree = 180;
        let mut straightest_delta = 180;
        let mut straightest_use = None;
        for xedge in &self.intersecting_edge {
            let xdegree = turn_degree(from_heading, xedge.begin_heading);
            let delta = straight_delta(xdegree);
            if xedge.is_traversable_outbound(mode) && delta < straightest_delta {
                straightest_delta = delta;
                straightest_turn_degree = xdegree;
                straightest_use = Some(xedge.r#use);
            }
        }
        (straightest_turn_degree, straightest_use)
    }

    fn is_straightest_traversable_intersecting_edge_reversed(
        &self,
        from_heading: u32,
        mode: i32,
    ) -> bool {
        let (degree, _) = self.straightest_traversable_intersecting_edge(from_heading, mode);
        degree > 124 && degree < 236
    }

    fn right_most_turn_degree(&self, turn_degree_in: u32, from_heading: u32, mode: i32) -> u32 {
        const fn right_delta(degree: u32) -> u32 {
            if degree < 90 {
                90 - degree
            } else if degree > 270 {
                (360 - degree) + 90
            } else {
                degree - 90
            }
        }
        let mut right_most = turn_degree_in;
        let mut right_most_delta = right_delta(turn_degree_in);
        for xedge in &self.intersecting_edge {
            if xedge.is_traversable_outbound(mode) {
                let xdegree = turn_degree(from_heading, xedge.begin_heading);
                let delta = right_delta(xdegree);
                if delta < right_most_delta {
                    right_most_delta = delta;
                    right_most = xdegree;
                }
            }
        }
        right_most
    }

    fn left_most_turn_degree(&self, turn_degree_in: u32, from_heading: u32, mode: i32) -> u32 {
        const fn left_delta(degree: u32) -> u32 {
            if degree < 90 {
                90 + degree
            } else {
                degree.abs_diff(270)
            }
        }
        let mut left_most = turn_degree_in;
        let mut left_most_delta = left_delta(turn_degree_in);
        for xedge in &self.intersecting_edge {
            if xedge.is_traversable_outbound(mode) {
                let xdegree = turn_degree(from_heading, xedge.begin_heading);
                let delta = left_delta(xdegree);
                if delta < left_most_delta {
                    left_most_delta = delta;
                    left_most = xdegree;
                }
            }
        }
        left_most
    }
}

/// Turn lane activation on a path edge (`EnhancedTripLeg_Edge::ActivateTurnLanes*`).
pub(crate) mod lanes {
    use super::{EdgeExt, TurnLane, lane, turn_lane};
    use valhalla_proto::directions_leg::maneuver::Type;
    use valhalla_proto::trip_leg::Edge;

    /// The remaining step distance (km) below which only lanes for the next turn are activated.
    const SHORT_REMAINING_DISTANCE_THRESHOLD: f32 = 0.402;

    /// Which end of the lane list to start activating from.
    #[derive(Clone, Copy)]
    enum From {
        Left,
        Right,
    }

    fn activate(
        edge: &mut Edge,
        direction: u16,
        current_type: i32,
        from: From,
        activated_max: u16,
    ) -> u16 {
        // Lanes without a direction make the whole set ambiguous.
        if edge.has_non_directional_turn_lane() {
            return 0;
        }
        let uturn = match from {
            From::Left => Type::KUturnLeft as i32,
            From::Right => Type::KUturnRight as i32,
        };
        let mut activated = 0_u16;
        let mut visit = |turn_lane: &mut TurnLane| {
            if turn_lane.directions_mask & u32::from(direction) != 0 {
                if activated < activated_max {
                    turn_lane.state = turn_lane::State::KActive as i32;
                    activated += 1;
                } else if current_type != uturn {
                    // Other lanes in the maneuver's direction are valid,
                    // except for a U-turn, where only the outermost lane should be used.
                    turn_lane.state = turn_lane::State::KValid as i32;
                }
                turn_lane.active_direction = u32::from(direction);
            }
        };
        match from {
            From::Left => edge.turn_lanes.iter_mut().for_each(&mut visit),
            From::Right => edge.turn_lanes.iter_mut().rev().for_each(&mut visit),
        }
        activated
    }

    /// Activates the turn lanes for a maneuver (`ActivateTurnLanes`), returning how many it activated.
    pub(crate) fn activate_turn_lanes(
        edge: &mut Edge,
        direction: u16,
        remaining_step_distance: f32,
        current_type: i32,
        next_type: i32,
    ) -> u16 {
        let all = u16::MAX;
        let is = |t: Type| next_type == t as i32;
        if current_type == Type::KUturnLeft as i32 && direction != lane::REVERSE {
            return activate(edge, direction, current_type, From::Left, 1);
        }
        if current_type == Type::KUturnRight as i32 && direction != lane::REVERSE {
            return activate(edge, direction, current_type, From::Right, 1);
        }
        let next_is_straight = is(Type::KBecomes)
            || is(Type::KContinue)
            || is(Type::KRampStraight)
            || is(Type::KStayStraight);
        if remaining_step_distance < SHORT_REMAINING_DISTANCE_THRESHOLD && !next_is_straight {
            let left = [
                Type::KSlightLeft,
                Type::KLeft,
                Type::KSharpLeft,
                Type::KUturnLeft,
                Type::KRampLeft,
                Type::KExitLeft,
                Type::KStayLeft,
                Type::KDestinationLeft,
                Type::KMergeLeft,
            ];
            let right = [
                Type::KSlightRight,
                Type::KRight,
                Type::KSharpRight,
                Type::KUturnRight,
                Type::KRampRight,
                Type::KExitRight,
                Type::KStayRight,
                Type::KDestinationRight,
                Type::KMergeRight,
            ];
            let drive_on_right = edge.drive_on_right();
            if left.into_iter().any(is) {
                activate(edge, direction, current_type, From::Left, 1)
            } else if right.into_iter().any(is) {
                activate(edge, direction, current_type, From::Right, 1)
            } else if is(Type::KMerge) {
                let from = if drive_on_right {
                    From::Left
                } else {
                    From::Right
                };
                activate(edge, direction, current_type, from, 1)
            } else if is(Type::KDestination) {
                let from = if drive_on_right {
                    From::Right
                } else {
                    From::Left
                };
                activate(edge, direction, current_type, from, 1)
            } else {
                // Roundabouts, ferries, and everything else activate every matching lane.
                activate(edge, direction, current_type, From::Left, all)
            }
        } else {
            activate(edge, direction, current_type, From::Left, all)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use valhalla_proto::TaggedValue;
    use valhalla_proto::trip_leg::edge::Level;

    fn zigzag(value: i32) -> Vec<u8> {
        let mut encoded = Vec::new();
        #[expect(clippy::cast_sign_loss, reason = "zigzag encoding")]
        let mut v = ((value << 1) ^ (value >> 31)) as u32;
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                encoded.push(byte);
                break;
            }
            encoded.push(byte | 0x80);
        }
        encoded
    }

    fn levels(precision_power: i32, values: &[i32]) -> Vec<u8> {
        let mut body = zigzag(precision_power);
        for value in values {
            body.extend(zigzag(*value));
        }
        let mut encoded = zigzag(i32::try_from(body.len()).unwrap());
        encoded.extend(body);
        encoded
    }

    #[test]
    fn decodes_levels_with_mainline_precision_bug() {
        let (decoded, precision) = decode_levels(&levels(0, &[2])).unwrap();
        assert_eq!(decoded, vec![(2.0, 2.0)]);
        assert_eq!(precision, 0);

        // A precision power of 2 still yields a precision of 10.
        let (decoded, precision) = decode_levels(&levels(2, &[150, 1_048_575, 20, 30])).unwrap();
        assert_eq!(decoded, vec![(15.0, 15.0), (2.0, 3.0)]);
        assert_eq!(precision, 10);

        assert!(decode_levels(&[]).is_none());
        assert!(decode_levels(&[0x10, 0x00]).is_none(), "truncated");
        assert!(
            decode_levels(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_none(),
            "overlong"
        );
    }

    #[test]
    fn level_refs_prefer_explicit_refs() {
        let mut edge = Edge {
            tagged_value: vec![TaggedValue {
                value: levels(0, &[-1]),
                r#type: tagged_value::Type::KLevels as i32,
            }],
            ..Edge::default()
        };
        assert_eq!(edge.level_refs(), vec!["Level -1".to_owned()]);

        edge.tagged_value[0].value = levels(1, &[15]);
        assert_eq!(edge.level_refs(), vec!["Level 1.5000000000".to_owned()]);

        edge.tagged_value.push(TaggedValue {
            value: b"P1".to_vec(),
            r#type: tagged_value::Type::KLevelRef as i32,
        });
        assert_eq!(edge.level_refs(), vec!["P1".to_owned()]);
    }

    #[test]
    fn level_changes() {
        let level = |start, end| Level { start, end };
        let mut edge = Edge::default();
        assert!(!edge.has_level_changes());
        edge.levels = vec![level(1.0, 1.0)];
        assert!(!edge.has_level_changes());
        edge.levels = vec![level(0.0, 1.0)];
        assert!(edge.has_level_changes());
        edge.levels = vec![level(0.0, 0.0), level(1.0, 1.0)];
        assert!(edge.has_level_changes());
    }

    #[test]
    fn edge_indexing_follows_mainline() {
        let leg = TripLeg {
            node: vec![
                Node {
                    edge: Some(Edge {
                        length_km: 1.0,
                        ..Edge::default()
                    }),
                    ..Node::default()
                },
                Node::default(),
                Node::default(),
            ],
            ..TripLeg::default()
        };
        assert_eq!(leg.curr_edge(0).map(|e| e.length_km), Some(1.0));
        // A node without an edge reads as an empty edge, as `mutable_edge()` would make it.
        assert_eq!(leg.curr_edge(1).map(|e| e.length_km), Some(0.0));
        // The last node has no current edge.
        assert!(leg.curr_edge(2).is_none());
        assert!(leg.prev_edge(0, 1).is_none());
        assert_eq!(leg.prev_edge(1, 1).map(|e| e.length_km), Some(1.0));
        assert!(leg.is_last_node_index(2));
        assert!(!leg.is_last_node_index(3));
    }
}
