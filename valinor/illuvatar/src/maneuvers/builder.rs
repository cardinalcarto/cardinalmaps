//! Producing the initial maneuvers from a leg's nodes
//! (`ManeuversBuilder::Produce` and the per-node helpers it calls).

use crate::error::OdinError;
use crate::maneuver::{Maneuver, RelativeDirection, TrailType};
use crate::signs::Sign;
use crate::street_names::{NameKind, StreetNames};
use crate::trip::{EdgeExt, IntersectingEdgeExt, LegExt, NodeExt};
use crate::turn::{TurnType, is_fork_forward, is_forward, is_wider_forward, turn_degree};
use crate::verbal_text_formatter::VerbalTextFormatter;
use valhalla_proto::directions_leg::maneuver::{BssManeuverType, Type};
use valhalla_proto::location::SideOfStreet;
use valhalla_proto::trip_leg::{Edge, Node, Use, node};
use valhalla_proto::{Location, Options, PedestrianType, RoadClass, TravelMode, TripLeg};
use valhalla_response::ErrorCode;

/// Mainline's `midgard::kHourPerSec`.
const HOUR_PER_SEC: f32 = 1.0 / 3600.0;

/// Builds the maneuvers of one trip leg.
///
/// The builder borrows the leg mutably because some passes update it in place,
/// as mainline does: they mark the turn lanes each maneuver uses
/// and copy turn lanes onto internal edges.
pub(crate) struct ManeuversBuilder<'a> {
    pub(super) options: &'a Options,
    pub(super) leg: &'a mut TripLeg,
}

impl<'a> ManeuversBuilder<'a> {
    pub(crate) fn new(options: &'a Options, leg: &'a mut TripLeg) -> Self {
        Self { options, leg }
    }

    /// Builds the maneuvers (`ManeuversBuilder::Build`).
    ///
    /// # Errors
    ///
    /// Fails, with mainline's error, when the leg has too few nodes or locations,
    /// when a heading is out of range, or when an edge the builder needs is missing.
    pub(crate) fn build(&mut self) -> Result<Vec<Maneuver>, OdinError> {
        let mut maneuvers = self.produce()?;
        self.combine(&mut maneuvers)?;
        Self::count_and_sort_signs(&mut maneuvers);
        self.confirm_maneuver_type_assignment(&mut maneuvers)?;
        self.set_traversable_outbound_intersecting_edge_flags(&mut maneuvers)?;
        self.process_roundabouts(&mut maneuvers);
        Self::set_to_stay_on_attribute(&mut maneuvers);
        Self::enhance_signless_interchanges(&mut maneuvers);
        self.process_guidance_views(&mut maneuvers);
        self.update_maneuver_placement_for_internal_intersection_turns(&mut maneuvers)?;
        // This must follow the placement update for internal edges.
        Self::collapse_small_end_ramp_fork(&mut maneuvers);
        Self::collapse_merge_maneuvers(&mut maneuvers);
        // This must follow the placement update, so that the right lanes are activated.
        self.process_turn_lanes(&maneuvers);
        self.add_landmarks_from_trip_leg_to_maneuvers(&mut maneuvers);
        Self::process_verbal_succinct_transition_instruction(&mut maneuvers);
        Ok(maneuvers)
    }

    // --- Accessors that turn mainline's null dereferences into errors. ---

    pub(super) fn node(&self, index: usize) -> Result<&Node, OdinError> {
        self.leg
            .node_at(index)
            .ok_or(OdinError::MissingNode { node_index: index })
    }

    pub(super) fn prev(&self, node_index: usize) -> Result<&Edge, OdinError> {
        self.leg
            .prev_edge(node_index, 1)
            .ok_or(OdinError::MissingEdge {
                node_index: node_index.wrapping_sub(1),
            })
    }

    pub(super) fn curr(&self, node_index: usize) -> Result<&Edge, OdinError> {
        self.leg
            .curr_edge(node_index)
            .ok_or(OdinError::MissingEdge { node_index })
    }

    pub(super) fn name_kind(&self, node_index: usize) -> NameKind {
        NameKind::for_country(self.leg.country_code(node_index))
    }

    /// The trip leg's origin and destination.
    fn origin_and_destination(&self) -> Result<(&Location, &Location), OdinError> {
        match self.leg.location.as_slice() {
            [origin, .., destination] => Ok((origin, destination)),
            _ => Err(OdinError::Coded(ErrorCode::TripNotEnoughLocations)),
        }
    }

    /// Creates the maneuvers, one per stretch of path that reads as one instruction
    /// (`ManeuversBuilder::Produce`).
    fn produce(&self) -> Result<Vec<Maneuver>, OdinError> {
        match self.leg.node.len() {
            0 => return Err(OdinError::Coded(ErrorCode::TripPathNoNodes)),
            // Mainline does not handle an origin and destination at the same node.
            1 => return Err(OdinError::Coded(ErrorCode::TripPathOneNode)),
            _ => {}
        }
        if self.leg.location.len() < 2 {
            return Err(OdinError::Coded(ErrorCode::TripNotEnoughLocations));
        }

        let last = self.leg.last_node_index();
        let mut destination = Maneuver::default();
        self.create_destination_maneuver(&mut destination)?;

        // Mainline pushes each new maneuver to the front of a list.
        // We push finished ones onto `finished` (so it is in reverse) and keep the current one apart.
        let mut finished = vec![destination];
        let mut current = Maneuver::default();
        self.initialize_maneuver(&mut current, last)?;

        // Step through the nodes in reverse, excluding the first and last.
        for i in (1..last).rev() {
            let node = self.node(i)?;
            if self.curr(i)?.pedestrian_type == PedestrianType::KBlind as i32 {
                Self::note_node_for_blind_users(&mut current, node);
            }
            if self.can_maneuver_include_prev_edge(&mut current, i)? {
                self.update_maneuver(&mut current, i)?;
            } else {
                self.finalize_maneuver(&mut current, i)?;
                finished.push(core::mem::take(&mut current));
                self.initialize_maneuver(&mut current, i)?;
            }
        }

        self.create_start_maneuver(&mut current)?;
        finished.push(current);
        finished.reverse();
        Ok(finished)
    }

    /// Records the intersection, gate, or bollard at a node, which blind users are told about.
    fn note_node_for_blind_users(maneuver: &mut Maneuver, node: &Node) {
        if node.is_type(node::Type::KStreetIntersection) {
            let mut names: Vec<(String, bool)> = Vec::new();
            for xedge in &node.intersecting_edge {
                for name in &xedge.name {
                    if !names.iter().any(|(value, route)| {
                        *value == name.value && *route == name.is_route_number
                    }) {
                        names.push((name.value.clone(), name.is_route_number));
                    }
                }
            }
            if !names.is_empty() {
                // `set_cross_street_names` with a vector always makes US street names.
                maneuver.cross_street_names = StreetNames::from_pairs(NameKind::Us, names);
                maneuver.node_type = Some(node.r#type);
                if node.traffic_signal {
                    maneuver.traffic_signal = true;
                }
            }
        } else if node.is_type(node::Type::KGate) || node.is_type(node::Type::KBollard) {
            maneuver.node_type = Some(node.r#type);
        }
    }

    fn create_destination_maneuver(&self, maneuver: &mut Maneuver) -> Result<(), OdinError> {
        let node_index = self.leg.last_node_index();
        let (_, destination) = self.origin_and_destination()?;
        maneuver.set_type(
            if destination.side_of_street == SideOfStreet::KLeft as i32 {
                Type::KDestinationLeft
            } else if destination.side_of_street == SideOfStreet::KRight as i32 {
                Type::KDestinationRight
            } else {
                Type::KDestination
            },
        );

        let node_index_u32 = index_u32(node_index);
        maneuver.begin_node_index = node_index_u32;
        maneuver.end_node_index = node_index_u32;

        let prev_edge = self.prev(node_index)?;
        maneuver.begin_shape_index = prev_edge.end_shape_index;
        maneuver.end_shape_index = prev_edge.end_shape_index;
        set_travel_types(maneuver, prev_edge);

        maneuver.verbal_formatter = Some(VerbalTextFormatter::for_region(
            self.leg.country_code(node_index),
            self.leg.state_code(node_index),
        ));
        Ok(())
    }

    fn create_start_maneuver(&self, maneuver: &mut Maneuver) -> Result<(), OdinError> {
        let (origin, _) = self.origin_and_destination()?;
        maneuver.set_type(if origin.side_of_street == SideOfStreet::KLeft as i32 {
            Type::KStartLeft
        } else if origin.side_of_street == SideOfStreet::KRight as i32 {
            Type::KStartRight
        } else {
            Type::KStart
        });

        // Start maneuvers are not helpful for routes starting on stairs or escalators.
        let curr_edge = self.curr(0)?;
        if curr_edge.is_steps_use() || curr_edge.is_escalator_use() || curr_edge.has_level_changes()
        {
            maneuver.set_type(Type::KNone);
        }

        self.finalize_maneuver(maneuver, 0)
    }

    /// Starts a maneuver that ends at `node_index` (`InitializeManeuver`).
    fn initialize_maneuver(
        &self,
        maneuver: &mut Maneuver,
        node_index: usize,
    ) -> Result<(), OdinError> {
        let prev_edge = self.prev(node_index)?;
        let curr_edge = self.leg.curr_edge(node_index);

        maneuver.end_heading = prev_edge.end_heading;
        maneuver.end_node_index = index_u32(node_index);
        maneuver.end_shape_index = prev_edge.end_shape_index;

        if let Some(curr_edge) = curr_edge {
            set_end_level_ref(maneuver, curr_edge);
        }

        if prev_edge.is_elevator_use() {
            maneuver.elevator = true;
        }
        if prev_edge.is_steps_use() && prev_edge.indoor {
            maneuver.indoor_steps = true;
        }
        if prev_edge.is_escalator_use() {
            maneuver.escalator = true;
        }
        if prev_edge.has_level_changes() {
            maneuver.has_level_changes = true;
        }
        if prev_edge.is_ramp_use() {
            maneuver.ramp = true;
        }
        if prev_edge.is_turn_channel_use() {
            maneuver.turn_channel = true;
        }
        if prev_edge.is_ferry_use() {
            maneuver.ferry = true;
        }
        if prev_edge.is_rail_ferry_use() {
            maneuver.rail_ferry = true;
        }
        if are_roundabouts_processable(prev_edge.travel_mode) && prev_edge.roundabout {
            maneuver.roundabout = true;
            maneuver.roundabout_exit_count = 1;
        }

        // Internal intersections, excluding the first and last edges.
        if prev_edge.internal_intersection
            && !self.leg.is_last_node_index(node_index)
            && node_index != 1
        {
            maneuver.internal_intersection = true;
        }

        maneuver.travel_mode = prev_edge.travel_mode;
        maneuver.drive_on_right = prev_edge.drive_on_right();
        set_travel_types(maneuver, prev_edge);

        maneuver.trail_type = if prev_edge.is_footway_use() {
            if prev_edge.is_unnamed() {
                TrailType::UnnamedWalkway
            } else {
                TrailType::NamedWalkway
            }
        } else if prev_edge.is_mountain_bike_use() {
            if prev_edge.is_unnamed() {
                TrailType::UnnamedMtbTrail
            } else {
                TrailType::NamedMtbTrail
            }
        } else if prev_edge.is_cycleway_use() {
            if prev_edge.is_unnamed() {
                TrailType::UnnamedCycleway
            } else {
                TrailType::NamedCycleway
            }
        } else {
            TrailType::None
        };

        // Only steps that change level or are long enough to matter count as steps.
        maneuver.is_steps = prev_edge.r#use == Use::KStepsUse as i32
            && (prev_edge.traverses_levels() || f64::from(prev_edge.length_km) >= 0.003);

        if maneuver.is_blind() {
            if prev_edge.r#use == Use::KStepsUse as i32 {
                maneuver.is_steps = true;
            }
            if prev_edge.bridge {
                maneuver.is_bridge = true;
            }
            if prev_edge.tunnel {
                maneuver.is_tunnel = true;
            }
        }

        self.update_maneuver(maneuver, node_index)
    }

    /// Extends a maneuver back over the edge entering `node_index` (`UpdateManeuver`).
    fn update_maneuver(&self, maneuver: &mut Maneuver, node_index: usize) -> Result<(), OdinError> {
        let prev_edge = self.prev(node_index)?;

        // Set the street names if there are none (and this is not an internal intersection),
        // or if a usable internal intersection name exists.
        if (maneuver.street_names.is_empty() && !maneuver.internal_intersection)
            || self.usable_internal_intersection_name(maneuver, node_index)?
        {
            maneuver.street_names =
                StreetNames::from_proto(self.name_kind(node_index), &prev_edge.name);
        }

        self.update_internal_turn_count(maneuver, node_index)?;

        maneuver.length += prev_edge.length_km;
        maneuver.basic_time += f64::from(get_time(
            prev_edge.length_km,
            get_speed(maneuver.travel_mode, prev_edge.default_speed),
        ));

        if prev_edge.toll {
            maneuver.portions_toll = true;
        }
        if prev_edge.has_time_restrictions {
            maneuver.has_time_restrictions = true;
        }
        if prev_edge.unpaved {
            maneuver.portions_unpaved = true;
        }
        if prev_edge.is_highway() {
            maneuver.portions_highway = true;
        }

        if are_roundabouts_processable(prev_edge.travel_mode) && prev_edge.roundabout {
            // A bicycle on a roundabout road counts exits as a car would.
            let mode = if prev_edge.travel_mode == TravelMode::KBicycle as i32
                && prev_edge.is_road_use()
            {
                TravelMode::KDrive as i32
            } else {
                prev_edge.travel_mode
            };
            let counts = self
                .node(node_index)?
                .intersecting_edge_counts(prev_edge.end_heading, mode);
            maneuver.roundabout_exit_count += if prev_edge.drive_on_right() {
                counts.right_traversable_outbound
            } else {
                counts.left_traversable_outbound
            };
        }

        if let Some(sign) = &prev_edge.sign {
            let signs = &mut maneuver.signs;
            signs
                .exit_number_list
                .extend(sign.exit_numbers.iter().map(Sign::from_proto));
            signs
                .exit_branch_list
                .extend(sign.exit_onto_streets.iter().map(Sign::from_proto));
            signs
                .exit_toward_list
                .extend(sign.exit_toward_locations.iter().map(Sign::from_proto));
            signs
                .exit_name_list
                .extend(sign.exit_names.iter().map(Sign::from_proto));
        }
        Ok(())
    }

    /// Completes a maneuver that begins at `node_index` (`FinalizeManeuver`).
    pub(super) fn finalize_maneuver(
        &self,
        maneuver: &mut Maneuver,
        node_index: usize,
    ) -> Result<(), OdinError> {
        let prev_edge = self.leg.prev_edge(node_index, 1);
        let curr_edge = self.curr(node_index)?;
        let node = self.node(node_index)?;

        maneuver.begin_cardinal_direction = determine_cardinal_direction(curr_edge.begin_heading);
        maneuver.begin_heading = curr_edge.begin_heading;
        maneuver.begin_node_index = index_u32(node_index);
        maneuver.begin_shape_index = curr_edge.begin_shape_index;
        self.update_time(maneuver);

        if node.is_type(node::Type::KElevator) {
            maneuver.elevator = true;
            maneuver.node_type = Some(node.r#type);
            set_end_level_ref(maneuver, curr_edge);
        }

        if node.is_type(node::Type::KBuildingEntrance) {
            if curr_edge.indoor && prev_edge.is_some_and(|prev| !prev.indoor) {
                maneuver.building_enter = true;
            } else if !curr_edge.indoor && prev_edge.is_some_and(|prev| prev.indoor) {
                maneuver.building_exit = true;
            }
        }

        if let Some(prev_edge) = prev_edge {
            maneuver.turn_degree = turn_degree(prev_edge.end_heading, curr_edge.begin_heading);
            self.determine_relative_direction(maneuver)?;
        }

        maneuver.begin_intersecting_edge_name_consistency =
            node.has_intersecting_edge_name_consistency();

        if !curr_edge.is_highway() && !curr_edge.internal_intersection && curr_edge.name.len() > 1 {
            let curr_edge_names =
                StreetNames::from_proto(self.name_kind(node_index), &curr_edge.name);
            let common_base_names = curr_edge_names.common_base_names(&maneuver.street_names);
            if curr_edge_names.len() > common_base_names.len() {
                maneuver.begin_street_names = curr_edge_names;
            }
        }

        if node.is_type(node::Type::KBikeShare)
            && let Some(prev_edge) = prev_edge
        {
            let bss_type = if prev_edge.has_mode(TravelMode::KBicycle)
                && maneuver.has_mode(TravelMode::KPedestrian)
            {
                Some(BssManeuverType::KReturnBikeAtBikeShare)
            } else if prev_edge.has_mode(TravelMode::KPedestrian)
                && maneuver.has_mode(TravelMode::KBicycle)
            {
                Some(BssManeuverType::KRentBikeAtBikeShare)
            } else {
                None
            };
            if let Some(bss_type) = bss_type {
                maneuver.bss_maneuver_type = bss_type as i32;
                if let Some(bss_info) = &node.bss_info {
                    maneuver.bss_info = bss_info.clone();
                }
            }
        }

        maneuver.verbal_formatter = Some(VerbalTextFormatter::for_region(
            self.leg.country_code(node_index),
            self.leg.state_code(node_index),
        ));

        if let Some(sign) = &curr_edge.sign {
            let signs = &mut maneuver.signs;
            signs
                .guide_branch_list
                .extend(sign.guide_onto_streets.iter().map(Sign::from_proto));
            signs
                .guide_toward_list
                .extend(sign.guide_toward_locations.iter().map(Sign::from_proto));
            signs
                .junction_name_list
                .extend(sign.junction_names.iter().map(Sign::from_proto));
        }

        if curr_edge.is_pedestrian_crossing_use() && prev_edge.is_some_and(EdgeExt::is_footway_use)
        {
            maneuver.pedestrian_crossing = true;
        }

        self.set_maneuver_type(maneuver, true)
    }

    /// The elapsed time between a maneuver's begin and end nodes.
    pub(super) fn update_time(&self, maneuver: &mut Maneuver) {
        maneuver.time = self.leg.elapsed_seconds(maneuver.end_node_index as usize)
            - self.leg.elapsed_seconds(maneuver.begin_node_index as usize);
    }

    /// Whether the edge entering `node_index` continues the maneuver (`CanManeuverIncludePrevEdge`).
    ///
    /// Along the way this marks forks, tees, U-turns, and intersecting forward edges on the maneuver,
    /// and narrows its street names to those it shares with the previous edge.
    #[expect(
        clippy::too_many_lines,
        reason = "mirrors mainline's single decision chain"
    )]
    fn can_maneuver_include_prev_edge(
        &self,
        maneuver: &mut Maneuver,
        node_index: usize,
    ) -> Result<bool, OdinError> {
        let prev_edge = self.prev(node_index)?;
        let curr_edge = self.curr(node_index)?;
        let node = self.node(node_index)?;
        let degree = turn_degree(prev_edge.end_heading, curr_edge.begin_heading);

        if curr_edge.pedestrian_type == PedestrianType::KBlind as i32
            && maneuver.node_type.is_some()
        {
            return Ok(false);
        }
        if node.is_type(node::Type::KBikeShare) || node.is_type(node::Type::KParking) {
            return Ok(false);
        }

        // Transit and transit connections are rejected before maneuvers are built
        // (see `directions::reject_transit`), so mainline's transit checks reduce to this.
        if maneuver.has_mode(TravelMode::KTransit) != prev_edge.has_mode(TravelMode::KTransit) {
            return Ok(false);
        }
        if maneuver.transit_connection || prev_edge.is_transit_connection() {
            return Ok(false);
        }

        if maneuver.drive_on_right != prev_edge.drive_on_right() {
            return Ok(false);
        }

        // Elevators.
        if maneuver.elevator != prev_edge.is_elevator_use() {
            return Ok(false);
        }
        if maneuver.elevator && prev_edge.is_elevator_use() {
            return Ok(true);
        }
        if node.is_type(node::Type::KElevator) {
            return Ok(false);
        }

        // Indoor steps.
        let prev_indoor_steps = prev_edge.is_steps_use() && prev_edge.indoor;
        if maneuver.indoor_steps != prev_indoor_steps {
            return Ok(false);
        }
        if maneuver.indoor_steps && prev_indoor_steps {
            return Ok(true);
        }

        // Steps.
        if maneuver.is_steps != prev_edge.is_steps_use() {
            return Ok(false);
        }
        if maneuver.is_steps && prev_edge.is_steps_use() {
            return Ok(true);
        }

        // Escalators.
        if maneuver.escalator != prev_edge.is_escalator_use() {
            return Ok(false);
        }
        if maneuver.escalator && prev_edge.is_escalator_use() {
            return Ok(true);
        }

        if node.is_type(node::Type::KBuildingEntrance) {
            return Ok(false);
        }
        if maneuver.has_level_changes != prev_edge.has_level_changes() {
            return Ok(false);
        }

        // Travel mode and the unnamed trail types.
        if maneuver.travel_mode != prev_edge.travel_mode
            || maneuver.unnamed_walkway() != prev_edge.is_unnamed_walkway()
            || maneuver.unnamed_cycleway() != prev_edge.is_unnamed_cycleway()
            || maneuver.unnamed_mountain_bike_trail() != prev_edge.is_unnamed_mountain_bike_trail()
        {
            return Ok(false);
        }

        // Roundabouts.
        if are_roundabouts_processable(prev_edge.travel_mode) {
            if maneuver.roundabout != prev_edge.roundabout {
                return Ok(false);
            }
            if maneuver.roundabout && prev_edge.roundabout {
                return Ok(true);
            }
        }

        // Forks.
        if self.is_fork(node_index, prev_edge, curr_edge)?
            || self.is_pedestrian_fork(node_index, prev_edge, curr_edge)?
        {
            maneuver.fork = true;
            return Ok(false);
        }

        // Internal intersections, which cannot be the first edge in the trip.
        if prev_edge.internal_intersection != maneuver.internal_intersection {
            return Ok(false);
        }
        if prev_edge.internal_intersection && node_index != 1 && maneuver.internal_intersection {
            return Ok(true);
        }

        // Simple turn channels.
        if prev_edge.is_turn_channel_use() != maneuver.turn_channel {
            return Ok(false);
        }
        if prev_edge.is_turn_channel_use() && maneuver.turn_channel {
            return Ok(true);
        }

        if maneuver.signs.has_exit() {
            return Ok(false);
        }

        // Ramps; a ramp to a ramp that is not forward starts a new maneuver.
        if maneuver.ramp != prev_edge.is_ramp_use() {
            return Ok(false);
        }
        if maneuver.ramp && prev_edge.is_ramp_use() {
            return Ok(is_forward(degree));
        }

        // Ferries.
        if maneuver.ferry != prev_edge.is_ferry_use() {
            return Ok(false);
        }
        if maneuver.ferry && prev_edge.is_ferry_use() {
            return Ok(true);
        }

        // Rail ferries.
        if maneuver.rail_ferry != prev_edge.is_rail_ferry_use() {
            return Ok(false);
        }
        if maneuver.rail_ferry && prev_edge.is_rail_ferry_use() {
            return Ok(true);
        }

        // Simple U-turns.
        if degree == 180 {
            maneuver.set_type(if prev_edge.drive_on_right() {
                Type::KUturnLeft
            } else {
                Type::KUturnRight
            });
            return Ok(false);
        }

        // Pencil point U-turns.
        if self.is_left_pencil_point_uturn(node_index, prev_edge, curr_edge)? {
            maneuver.set_type(Type::KUturnLeft);
            return Ok(false);
        }
        if self.is_right_pencil_point_uturn(node_index, prev_edge, curr_edge)? {
            maneuver.set_type(Type::KUturnRight);
            return Ok(false);
        }

        if Self::is_intersecting_forward_edge(node, prev_edge, curr_edge) {
            maneuver.intersecting_forward_edge = true;
            return Ok(false);
        }

        let prev_edge_names = StreetNames::from_proto(self.name_kind(node_index), &prev_edge.name);
        let common_base_names = prev_edge_names.common_base_names(&maneuver.street_names);

        if Self::is_tee(node, prev_edge, curr_edge, !common_base_names.is_empty()) {
            maneuver.tee = true;
            return Ok(false);
        }

        // A non-forward transition with an intersecting traversable edge.
        let (straightest, _) = node.straightest_traversable_intersecting_edge(
            prev_edge.end_heading,
            prev_edge.travel_mode,
        );
        if !curr_edge.is_straightest(degree, straightest)
            && !node.has_forward_traversable_intersecting_edge(
                prev_edge.end_heading,
                prev_edge.travel_mode,
            )
            && node.has_traversable_exclude_use_xedge(prev_edge.travel_mode, Use::KTrackUse)
        {
            return Ok(false);
        }

        if !common_base_names.is_empty() {
            maneuver.street_names = common_base_names;
            return Ok(true);
        }

        // An unnamed edge.
        if !maneuver.has_street_names()
            && prev_edge.is_unnamed()
            && Self::include_unnamed_prev_edge(node, prev_edge, curr_edge)
        {
            return Ok(true);
        }

        Ok(false)
    }

    fn include_unnamed_prev_edge(node: &Node, prev_edge: &Edge, curr_edge: &Edge) -> bool {
        !node.has_intersecting_edges()
            || curr_edge.is_straightest(
                turn_degree(prev_edge.end_heading, curr_edge.begin_heading),
                node.straightest_intersecting_edge_turn_degree(prev_edge.end_heading),
            )
    }

    pub(super) fn determine_merge_to_relative_direction(
        node: &Node,
        prev_edge: &Edge,
    ) -> RelativeDirection {
        let counts = node.intersecting_edge_counts(prev_edge.end_heading, prev_edge.travel_mode);
        if counts.left > 0 && counts.left_similar == 0 && counts.right == 0 {
            RelativeDirection::KeepLeft
        } else if counts.right > 0 && counts.right_similar == 0 && counts.left == 0 {
            RelativeDirection::KeepRight
        } else {
            RelativeDirection::None
        }
    }

    fn is_fork(
        &self,
        node_index: usize,
        prev_edge: &Edge,
        curr_edge: &Edge,
    ) -> Result<bool, OdinError> {
        let node = self.node(node_index)?;
        let xedge_count = node.intersecting_edge.len();
        if !(1..=2).contains(&xedge_count) {
            return Ok(false);
        }
        let degree = turn_degree(prev_edge.end_heading, curr_edge.begin_heading);
        let from = prev_edge.end_heading;
        let mode = prev_edge.travel_mode;

        if node.fork
            && is_wider_forward(degree)
            && node.has_wider_forward_traversable_intersecting_edge(from, curr_edge.travel_mode)
        {
            // A motorway junction where only an intersecting edge is a service road is not a fork.
            if node.is_type(node::Type::KMotorwayJunction)
                && curr_edge.road_class != RoadClass::KServiceOther as i32
                && node.has_specified_road_class_xedge(RoadClass::KServiceOther)
            {
                return Ok(false);
            }
            let counts = node.intersecting_edge_counts(from, mode);
            if counts.left_similar_traversable_outbound > 0
                || counts.right_similar_traversable_outbound > 0
                || ((counts.left_traversable_outbound > 0 || counts.right_traversable_outbound > 0)
                    && curr_edge.is_ramp_use()
                    && !node.is_straightest_traversable_intersecting_edge_reversed(from, mode))
                || (is_fork_forward(degree)
                    && node.has_only_forward_traversable_road_class_xedges(
                        from,
                        mode,
                        prev_edge.road_class,
                    ))
            {
                return Ok(true);
            }
            // Mainline does not fall through to the other checks here.
            return Ok(false);
        }

        let not_special = |edge: &Edge| {
            !edge.is_ramp_use()
                && !edge.is_turn_channel_use()
                && !edge.is_ferry_use()
                && !edge.is_rail_ferry_use()
        };

        if prev_edge.is_highway()
            && curr_edge.is_highway()
            && is_wider_forward(degree)
            && node.has_wider_forward_traversable_highway_xedge(from, mode)
        {
            return Ok(true);
        }
        if prev_edge.road_class >= curr_edge.road_class
            && not_special(prev_edge)
            && not_special(curr_edge)
            && is_fork_forward(degree)
            && node.has_only_forward_traversable_road_class_xedges(from, mode, prev_edge.road_class)
        {
            return Ok(true);
        }
        let service = RoadClass::KServiceOther as i32;
        if is_fork_forward(degree)
            && not_special(prev_edge)
            && not_special(curr_edge)
            && node.has_road_fork_traversable_intersecting_edge(
                from,
                mode,
                prev_edge.road_class == service || curr_edge.road_class == service,
            )
        {
            return Ok(true);
        }
        if let [xedge] = node.intersecting_edge.as_slice()
            && prev_edge.is_highway()
            && ((curr_edge.is_highway() && xedge.r#use == Use::KRampUse as i32)
                || (xedge.is_highway() && curr_edge.is_ramp_use()))
            && self.has_lane_bifurcation(
                node_index,
                prev_edge,
                curr_edge,
                xedge.lane_count,
                xedge.r#use,
            )
            && is_fork_forward(degree)
            && is_fork_forward(turn_degree(from, xedge.begin_heading))
        {
            return Ok(true);
        }
        Ok(false)
    }

    /// Whether a highway splits its lanes between the path and the intersecting edge,
    /// rather than dropping a deceleration lane onto an exit.
    ///
    /// Patterns like 2 lanes into 1 and 1, 3 into 2 and 2, or 5 into 3 and 3.
    fn has_lane_bifurcation(
        &self,
        node_index: usize,
        prev_edge: &Edge,
        curr_edge: &Edge,
        xedge_lane_count: u32,
        xedge_use: i32,
    ) -> bool {
        let prev_lane_count = prev_edge.lane_count;
        let curr_lane_count = curr_edge.lane_count;

        // Going from N+1 lanes to N lanes: is this a bifurcation, or a deceleration lane for an exit?
        if curr_lane_count > 1
            && prev_lane_count == curr_lane_count.wrapping_add(1)
            && xedge_use == Use::KRampUse as i32
        {
            let mut delta = 1;
            let Some(first) = self.leg.prev_edge(node_index, delta) else {
                return false;
            };
            let orig_lane_count = first.lane_count;
            let (min_length, max_length) = deceleration_lane_length(first.default_speed);
            let mut aggregate_length = first.length_km;

            // Walk back until the extra lane goes away or the lane is too long to be a deceleration lane.
            loop {
                delta += 1;
                let Some(prev_at_delta) = self.leg.prev_edge(node_index, delta) else {
                    // The start of the route: do not consider this a deceleration lane.
                    aggregate_length = 0.0;
                    break;
                };
                if prev_at_delta.lane_count < orig_lane_count {
                    break;
                }
                aggregate_length += prev_at_delta.length_km;
                if aggregate_length > max_length {
                    break;
                }
            }
            if aggregate_length < max_length && aggregate_length > min_length {
                return false;
            }
        }

        let post_split_min_count = prev_lane_count.wrapping_add(1) / 2;
        (prev_lane_count == 2 && curr_lane_count == 1 && xedge_lane_count == 1)
            || (prev_lane_count > 2
                && curr_lane_count == post_split_min_count
                && xedge_lane_count == post_split_min_count)
    }

    fn is_pedestrian_fork(
        &self,
        node_index: usize,
        prev_edge: &Edge,
        curr_edge: &Edge,
    ) -> Result<bool, OdinError> {
        let is_relative_straight = |degree: u32| !(45..=315).contains(&degree);
        let node = self.node(node_index)?;
        let path_turn_degree = turn_degree(prev_edge.end_heading, curr_edge.begin_heading);
        let pedestrian = prev_edge.has_mode(TravelMode::KPedestrian)
            && curr_edge.has_mode(TravelMode::KPedestrian);

        if pedestrian && is_relative_straight(path_turn_degree) && node.intersecting_edge.len() < 3
        {
            let counts =
                node.intersecting_edge_counts(prev_edge.end_heading, prev_edge.travel_mode);
            let (straightest, xedge_use) = node.straightest_traversable_intersecting_edge(
                prev_edge.end_heading,
                prev_edge.travel_mode,
            );
            // Footways and crosswalks count as the same use here.
            let similar_use = xedge_use.is_some_and(|xedge_use| {
                curr_edge.r#use == xedge_use
                    || (curr_edge.is_footway_use()
                        && (xedge_use == Use::KPedestrianCrossingUse as i32
                            || xedge_use == Use::KFootwayUse as i32))
            });
            if ((counts.left_similar_traversable_outbound > 0
                || counts.right_similar_traversable_outbound > 0
                || is_relative_straight(straightest))
                && similar_use)
                || (prev_edge.roundabout && !curr_edge.roundabout)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn is_tee(
        node: &Node,
        prev_edge: &Edge,
        curr_edge: &Edge,
        prev_edge_has_common_base_name: bool,
    ) -> bool {
        let [xedge] = node.intersecting_edge.as_slice() else {
            return false;
        };
        let path_turn = TurnType::of(turn_degree(prev_edge.end_heading, curr_edge.begin_heading));
        let xedge_turn = TurnType::of(turn_degree(prev_edge.end_heading, xedge.begin_heading));
        if !xedge.is_traversable(prev_edge.travel_mode) {
            return false;
        }
        if prev_edge_has_common_base_name
            && !node.has_traversable_exclude_use_xedge(prev_edge.travel_mode, Use::KTrackUse)
        {
            return false;
        }
        matches!(
            (path_turn, xedge_turn),
            (TurnType::Right, TurnType::Left) | (TurnType::Left, TurnType::Right)
        )
    }

    fn is_left_pencil_point_uturn(
        &self,
        node_index: usize,
        prev_edge: &Edge,
        curr_edge: &Edge,
    ) -> Result<bool, OdinError> {
        let degree = turn_degree(prev_edge.end_heading, curr_edge.begin_heading);
        // Driving on the right, a sharp left (179 < turn < 226), between one-way edges.
        if curr_edge.drive_on_right()
            && degree > 179
            && degree < 226
            && prev_edge.is_oneway()
            && curr_edge.is_oneway()
        {
            let counts = self
                .node(node_index)?
                .intersecting_edge_counts(prev_edge.end_heading, prev_edge.travel_mode);
            return Ok(counts.left_traversable_outbound == 0
                && self.have_common_base_names(node_index, prev_edge, curr_edge));
        }
        Ok(false)
    }

    fn is_right_pencil_point_uturn(
        &self,
        node_index: usize,
        prev_edge: &Edge,
        curr_edge: &Edge,
    ) -> Result<bool, OdinError> {
        let degree = turn_degree(prev_edge.end_heading, curr_edge.begin_heading);
        // Mainline's comment says "drive on left", but it checks for driving on the right,
        // so right pencil point U-turns only happen there. We keep that.
        if curr_edge.drive_on_right()
            && degree > 134
            && degree < 181
            && prev_edge.is_oneway()
            && curr_edge.is_oneway()
        {
            let counts = self
                .node(node_index)?
                .intersecting_edge_counts(prev_edge.end_heading, prev_edge.travel_mode);
            return Ok(counts.right_traversable_outbound == 0
                && self.have_common_base_names(node_index, prev_edge, curr_edge));
        }
        Ok(false)
    }

    fn have_common_base_names(
        &self,
        node_index: usize,
        prev_edge: &Edge,
        curr_edge: &Edge,
    ) -> bool {
        let kind = self.name_kind(node_index);
        let prev_edge_names = StreetNames::from_proto(kind, &prev_edge.name);
        let curr_edge_names = StreetNames::from_proto(kind, &curr_edge.name);
        !prev_edge_names
            .common_base_names(&curr_edge_names)
            .is_empty()
    }

    /// Whether a non-forward path has a forward intersecting edge, or a forward path a straighter
    /// significant one (`IsIntersectingForwardEdge`).
    fn is_intersecting_forward_edge(node: &Node, prev_edge: &Edge, curr_edge: &Edge) -> bool {
        let degree = turn_degree(prev_edge.end_heading, curr_edge.begin_heading);
        let from = prev_edge.end_heading;
        let mode = prev_edge.travel_mode;
        let highway_to_highway = curr_edge.is_highway() && prev_edge.is_highway();
        if node.has_intersecting_edges()
            && !node.is_type(node::Type::KMotorwayJunction)
            && !node.fork
            && !highway_to_highway
        {
            let forward = is_forward(degree);
            if !forward
                && node.has_forward_traversable_exclude_use_xedge(from, mode, Use::KTrackUse)
            {
                return true;
            }
            if forward
                && node.has_forward_traversable_significant_road_class_xedge(
                    from,
                    mode,
                    prev_edge.road_class,
                )
            {
                let (straightest, _) = node.straightest_traversable_intersecting_edge(from, mode);
                if !curr_edge.is_straightest(degree, straightest) {
                    return true;
                }
            }
        }
        false
    }

    /// Sets the maneuver's relative direction, refining "keep straight" from the intersecting
    /// edges (`DetermineRelativeDirection`).
    fn determine_relative_direction(&self, maneuver: &mut Maneuver) -> Result<(), OdinError> {
        let begin = maneuver.begin_node_index as usize;
        let prev_edge = self.prev(begin)?;
        let curr_edge = self.curr(begin)?;
        let node = self.node(begin)?;
        let from = prev_edge.end_heading;
        let mode = prev_edge.travel_mode;
        let counts = node.intersecting_edge_counts(from, mode);

        let relative_direction = RelativeDirection::of(maneuver.turn_degree);
        maneuver.begin_relative_direction = relative_direction;
        let turn_type = TurnType::of(maneuver.turn_degree);

        if relative_direction == RelativeDirection::KeepStraight {
            let not_straightest = || {
                let (straightest, _) = node.straightest_traversable_intersecting_edge(from, mode);
                !curr_edge.is_straightest(maneuver.turn_degree, straightest)
            };
            if counts.right_similar_traversable_outbound == 0
                && counts.left_similar_traversable_outbound > 0
            {
                maneuver.begin_relative_direction = RelativeDirection::KeepRight;
            } else if counts.right_similar_traversable_outbound > 0
                && counts.left_similar_traversable_outbound == 0
            {
                maneuver.begin_relative_direction = RelativeDirection::KeepLeft;
            } else if counts.left_similar_traversable_outbound == 0
                && counts.left_traversable_outbound > 0
                && counts.right_traversable_outbound == 0
            {
                if not_straightest()
                    || (maneuver.turn_channel && turn_type != TurnType::Straight)
                    || maneuver.fork
                {
                    maneuver.begin_relative_direction = RelativeDirection::KeepRight;
                }
            } else if counts.right_similar_traversable_outbound == 0
                && counts.right_traversable_outbound > 0
                && counts.left_traversable_outbound == 0
                && (not_straightest()
                    || (maneuver.turn_channel && turn_type != TurnType::Straight)
                    || maneuver.fork)
            {
                maneuver.begin_relative_direction = RelativeDirection::KeepLeft;
            }
        } else if relative_direction == RelativeDirection::Left
            && turn_type == TurnType::SlightLeft
            && node.has_specified_turn_xedge(TurnType::Left, from, maneuver.travel_mode)
        {
            maneuver.begin_relative_direction = RelativeDirection::KeepLeft;
        } else if relative_direction == RelativeDirection::Right
            && turn_type == TurnType::SlightRight
            && node.has_specified_turn_xedge(TurnType::Right, from, maneuver.travel_mode)
        {
            maneuver.begin_relative_direction = RelativeDirection::KeepRight;
        }
        Ok(())
    }

    /// The relative direction of the turn from the edge two back onto the previous edge.
    fn prev_prev_relative_direction(
        &self,
        node_index: usize,
    ) -> Result<(RelativeDirection, &Edge), OdinError> {
        let prev_edge = self.prev(node_index)?;
        let degree = self.leg.prev_edge(node_index, 2).map_or(0, |prev_prev| {
            turn_degree(prev_prev.end_heading, prev_edge.begin_heading)
        });
        Ok((RelativeDirection::of(degree), prev_edge))
    }

    fn usable_internal_intersection_name(
        &self,
        maneuver: &Maneuver,
        node_index: usize,
    ) -> Result<bool, OdinError> {
        let (relative_direction, prev_edge) = self.prev_prev_relative_direction(node_index)?;
        // A left turn when driving on the right, or a right turn when driving on the left.
        Ok(maneuver.internal_intersection
            && ((prev_edge.drive_on_right() && relative_direction == RelativeDirection::Left)
                || (!prev_edge.drive_on_right() && relative_direction == RelativeDirection::Right)))
    }

    fn update_internal_turn_count(
        &self,
        maneuver: &mut Maneuver,
        node_index: usize,
    ) -> Result<(), OdinError> {
        let (relative_direction, _) = self.prev_prev_relative_direction(node_index)?;
        match relative_direction {
            RelativeDirection::Right => maneuver.internal_right_turn_count += 1,
            RelativeDirection::Left => maneuver.internal_left_turn_count += 1,
            _ => {}
        }
        Ok(())
    }
}

/// Copies the travel mode's type (vehicle, pedestrian, bicycle, or transit) from an edge.
fn set_travel_types(maneuver: &mut Maneuver, edge: &Edge) {
    maneuver.travel_mode = edge.travel_mode;
    if edge.has_mode(TravelMode::KDrive) {
        maneuver.vehicle_type = edge.vehicle_type;
    }
    if edge.has_mode(TravelMode::KPedestrian) {
        maneuver.pedestrian_type = edge.pedestrian_type;
    }
    if edge.has_mode(TravelMode::KBicycle) {
        maneuver.bicycle_type = edge.bicycle_type;
    }
    if edge.has_mode(TravelMode::KTransit) {
        maneuver.transit_type = edge.transit_type;
    }
}

/// Sets the maneuver's end level from an edge's level refs, if it has exactly one.
fn set_end_level_ref(maneuver: &mut Maneuver, edge: &Edge) {
    let level_refs = edge.level_refs();
    match level_refs.as_slice() {
        [] => {}
        [level_ref] => maneuver.end_level_ref.clone_from(level_ref),
        _ => maneuver.end_level_ref.clear(),
    }
}

/// Whether maneuvers in a travel mode treat roundabouts specially (`AreRoundaboutsProcessable`).
#[inline]
pub(super) fn are_roundabouts_processable(mode: i32) -> bool {
    mode == TravelMode::KDrive as i32 || mode == TravelMode::KBicycle as i32
}

/// The speed used for basic times (`GetSpeed`): fixed for walking and cycling.
#[inline]
pub(super) fn get_speed(mode: i32, edge_speed: f32) -> f32 {
    if mode == TravelMode::KPedestrian as i32 {
        5.1
    } else if mode == TravelMode::KBicycle as i32 {
        20.0
    } else {
        edge_speed
    }
}

/// The time to travel a length at a speed, rounded to whole seconds (`midgard::GetTime`).
#[inline]
pub(super) fn get_time(length_km: f32, speed_kph: f32) -> i32 {
    if speed_kph > 0.0 {
        // Mainline computes this in single precision and truncates with `static_cast<int>`.
        #[expect(clippy::cast_possible_truncation, reason = "mainline truncates to int")]
        let seconds = (length_km / (speed_kph * HOUR_PER_SEC) + 0.5) as i32;
        seconds
    } else {
        0
    }
}

/// The deceleration lane length range (km) for a road speed (`get_deceleration_lane_length`).
///
/// Mainline derived this empirically; see the comment there.
fn deceleration_lane_length(speed_kph: f32) -> (f32, f32) {
    // Mainline's coefficients are double literals, so the line is evaluated in double precision
    // and only then stored in a float.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mainline stores the result in a float"
    )]
    let length: f32 = if speed_kph < 80.0 {
        0.1
    } else {
        (0.001_419_94 * f64::from(speed_kph) + 0.035_093_88) as f32
    };
    let tolerance = length * 0.35;
    (length - tolerance, length + tolerance)
}

/// The cardinal direction of a heading (`DetermineCardinalDirection`).
///
/// Mainline's ranges cover every `u32`: headings of 360 and above count as north
/// (Thor sends 360 for some edges), so its error 220 cannot happen.
pub(super) fn determine_cardinal_direction(heading: u32) -> i32 {
    use valhalla_proto::directions_leg::maneuver::CardinalDirection as Cardinal;
    let direction = match heading {
        24..=66 => Cardinal::KNorthEast,
        67..=113 => Cardinal::KEast,
        114..=156 => Cardinal::KSouthEast,
        157..=203 => Cardinal::KSouth,
        204..=246 => Cardinal::KSouthWest,
        247..=293 => Cardinal::KWest,
        294..=336 => Cardinal::KNorthWest,
        _ => Cardinal::KNorth,
    };
    direction as i32
}

/// Converts a node index to the `u32` that maneuvers store.
///
/// Trip legs come from protobuf repeated fields, whose length fits in `i32`.
#[inline]
pub(super) fn index_u32(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cardinal_directions_match_mainline() {
        use valhalla_proto::directions_leg::maneuver::CardinalDirection as Cardinal;
        let expected = [
            (0, Cardinal::KNorth),
            (23, Cardinal::KNorth),
            (24, Cardinal::KNorthEast),
            (66, Cardinal::KNorthEast),
            (67, Cardinal::KEast),
            (113, Cardinal::KEast),
            (114, Cardinal::KSouthEast),
            (156, Cardinal::KSouthEast),
            (157, Cardinal::KSouth),
            (203, Cardinal::KSouth),
            (204, Cardinal::KSouthWest),
            (246, Cardinal::KSouthWest),
            (247, Cardinal::KWest),
            (293, Cardinal::KWest),
            (294, Cardinal::KNorthWest),
            (336, Cardinal::KNorthWest),
            (337, Cardinal::KNorth),
            (359, Cardinal::KNorth),
        ];
        for (heading, direction) in expected {
            assert_eq!(
                determine_cardinal_direction(heading),
                direction as i32,
                "heading {heading}"
            );
        }
        assert_eq!(determine_cardinal_direction(360), Cardinal::KNorth as i32);
        assert_eq!(
            determine_cardinal_direction(u32::MAX),
            Cardinal::KNorth as i32
        );
    }

    #[test]
    fn times_round_like_mainline() {
        assert_eq!(get_time(1.0, 60.0), 60);
        assert_eq!(get_time(0.0125, 90.0), 1);
        assert_eq!(get_time(1.0, 0.0), 0);
        assert_eq!(get_speed(TravelMode::KPedestrian as i32, 50.0), 5.1);
        assert_eq!(get_speed(TravelMode::KDrive as i32, 50.0), 50.0);
    }
}
