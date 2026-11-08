//! The working state of one maneuver (`odin::Maneuver`).
//!
//! The maneuvers builder creates these from the trip's edges and combines them;
//! the narrative builder then fills in their instructions,
//! and the directions builder copies them into the directions message.
//! Fields are plain data, named as in mainline.

use crate::signs::Signs;
use crate::street_names::StreetNames;
use crate::trip::MILES_PER_KM;
use crate::verbal_text_formatter::VerbalTextFormatter;
use valhalla_proto::directions_leg::GuidanceView;
use valhalla_proto::directions_leg::maneuver::{BssManeuverType, CardinalDirection, Type};
use valhalla_proto::{
    BicycleType, BikeShareStationInfo, PedestrianType, RouteLandmark, TransitType, TravelMode,
    VehicleType,
};

/// The direction of a maneuver relative to the incoming edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RelativeDirection {
    #[default]
    None,
    KeepStraight,
    KeepRight,
    Right,
    Reverse,
    Left,
    KeepLeft,
}

impl RelativeDirection {
    /// The relative direction of a turn degree (`ManeuversBuilder::DetermineRelativeDirection`).
    pub(crate) const fn of(turn_degree: u32) -> Self {
        if turn_degree > 329 || turn_degree < 31 {
            Self::KeepStraight
        } else if turn_degree > 30 && turn_degree < 160 {
            Self::Right
        } else if turn_degree > 159 && turn_degree < 201 {
            Self::Reverse
        } else if turn_degree > 200 && turn_degree < 330 {
            Self::Left
        } else {
            Self::None
        }
    }
}

/// The kind of trail a maneuver is on, which keeps unlike trails apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TrailType {
    #[default]
    None,
    NamedCycleway,
    UnnamedCycleway,
    NamedWalkway,
    UnnamedWalkway,
    NamedMtbTrail,
    UnnamedMtbTrail,
}

/// One maneuver under construction.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_field_names,
    clippy::struct_excessive_bools,
    reason = "mirrors mainline's Maneuver, whose many flags each feed a different decision"
)]
pub(crate) struct Maneuver {
    /// A `DirectionsLeg.Maneuver.Type` value.
    pub(crate) maneuver_type: i32,
    /// The node type to announce (for blind users), if any; a `TripLeg.Node.Type` value.
    pub(crate) node_type: Option<i32>,
    pub(crate) traffic_signal: bool,
    pub(crate) is_steps: bool,
    pub(crate) is_bridge: bool,
    pub(crate) is_tunnel: bool,
    pub(crate) street_names: StreetNames,
    pub(crate) begin_street_names: StreetNames,
    pub(crate) cross_street_names: StreetNames,
    pub(crate) instruction: String,
    /// Kilometers.
    pub(crate) length: f32,
    /// Seconds.
    pub(crate) time: f64,
    /// Length over speed on each edge, with no stop impact, in seconds.
    pub(crate) basic_time: f64,
    pub(crate) turn_degree: u32,
    pub(crate) begin_relative_direction: RelativeDirection,
    /// A `DirectionsLeg.Maneuver.CardinalDirection` value.
    pub(crate) begin_cardinal_direction: i32,
    pub(crate) begin_heading: u32,
    pub(crate) end_heading: u32,
    pub(crate) begin_node_index: u32,
    pub(crate) end_node_index: u32,
    pub(crate) begin_shape_index: u32,
    pub(crate) end_shape_index: u32,
    pub(crate) ramp: bool,
    pub(crate) turn_channel: bool,
    pub(crate) ferry: bool,
    pub(crate) rail_ferry: bool,
    pub(crate) roundabout: bool,
    pub(crate) portions_toll: bool,
    pub(crate) portions_unpaved: bool,
    pub(crate) portions_highway: bool,
    pub(crate) internal_intersection: bool,
    pub(crate) signs: Signs,
    pub(crate) internal_right_turn_count: u32,
    pub(crate) internal_left_turn_count: u32,
    pub(crate) fork: bool,
    pub(crate) begin_intersecting_edge_name_consistency: bool,
    pub(crate) intersecting_forward_edge: bool,
    pub(crate) verbal_succinct_transition_instruction: String,
    pub(crate) verbal_transition_alert_instruction: String,
    pub(crate) verbal_pre_transition_instruction: String,
    pub(crate) verbal_post_transition_instruction: String,
    pub(crate) tee: bool,
    pub(crate) trail_type: TrailType,
    pub(crate) imminent_verbal_multi_cue: bool,
    pub(crate) distant_verbal_multi_cue: bool,
    pub(crate) to_stay_on: bool,
    pub(crate) pedestrian_crossing: bool,
    pub(crate) merge_to_relative_direction: RelativeDirection,
    pub(crate) drive_on_right: bool,
    pub(crate) has_time_restrictions: bool,
    pub(crate) has_right_traversable_outbound_intersecting_edge: bool,
    pub(crate) has_left_traversable_outbound_intersecting_edge: bool,
    pub(crate) include_verbal_pre_transition_length: bool,
    pub(crate) contains_obvious_maneuver: bool,
    pub(crate) roundabout_exit_count: u32,
    pub(crate) has_combined_enter_exit_roundabout: bool,
    /// Kilometers.
    pub(crate) roundabout_length: f32,
    /// Kilometers.
    pub(crate) roundabout_exit_length: f32,
    pub(crate) roundabout_exit_street_names: StreetNames,
    pub(crate) roundabout_exit_begin_street_names: StreetNames,
    pub(crate) roundabout_exit_signs: Signs,
    pub(crate) roundabout_exit_begin_heading: u32,
    pub(crate) roundabout_exit_turn_degree: u32,
    pub(crate) roundabout_exit_shape_index: u32,
    pub(crate) has_collapsed_small_end_ramp_fork: bool,
    pub(crate) has_collapsed_merge_maneuver: bool,
    pub(crate) has_long_street_name: bool,
    pub(crate) bss_info: BikeShareStationInfo,
    pub(crate) elevator: bool,
    pub(crate) indoor_steps: bool,
    pub(crate) escalator: bool,
    pub(crate) building_enter: bool,
    pub(crate) building_exit: bool,
    pub(crate) has_level_changes: bool,
    pub(crate) end_level_ref: String,
    pub(crate) landmarks: Vec<RouteLandmark>,
    pub(crate) transit_connection: bool,
    /// A `TravelMode` value.
    pub(crate) travel_mode: i32,
    /// A `VehicleType` value.
    pub(crate) vehicle_type: i32,
    /// A `PedestrianType` value.
    pub(crate) pedestrian_type: i32,
    /// A `BicycleType` value.
    pub(crate) bicycle_type: i32,
    /// A `TransitType` value.
    pub(crate) transit_type: i32,
    /// A `DirectionsLeg.Maneuver.BssManeuverType` value.
    pub(crate) bss_maneuver_type: i32,
    pub(crate) verbal_formatter: Option<VerbalTextFormatter>,
    pub(crate) guidance_views: Vec<GuidanceView>,
}

impl Default for Maneuver {
    /// A new maneuver, initialized as mainline's constructor does.
    fn default() -> Self {
        Self {
            maneuver_type: Type::KNone as i32,
            node_type: None,
            traffic_signal: false,
            is_steps: false,
            is_bridge: false,
            is_tunnel: false,
            // Mainline's constructor creates plain (not US) street name lists.
            street_names: StreetNames::default(),
            begin_street_names: StreetNames::default(),
            cross_street_names: StreetNames::default(),
            instruction: String::new(),
            length: 0.0,
            time: 0.0,
            basic_time: 0.0,
            turn_degree: 0,
            begin_relative_direction: RelativeDirection::None,
            begin_cardinal_direction: CardinalDirection::KNorth as i32,
            begin_heading: 0,
            end_heading: 0,
            begin_node_index: 0,
            end_node_index: 0,
            begin_shape_index: 0,
            end_shape_index: 0,
            ramp: false,
            turn_channel: false,
            ferry: false,
            rail_ferry: false,
            roundabout: false,
            portions_toll: false,
            portions_unpaved: false,
            portions_highway: false,
            internal_intersection: false,
            signs: Signs::default(),
            internal_right_turn_count: 0,
            internal_left_turn_count: 0,
            fork: false,
            begin_intersecting_edge_name_consistency: false,
            intersecting_forward_edge: false,
            verbal_succinct_transition_instruction: String::new(),
            verbal_transition_alert_instruction: String::new(),
            verbal_pre_transition_instruction: String::new(),
            verbal_post_transition_instruction: String::new(),
            tee: false,
            trail_type: TrailType::None,
            imminent_verbal_multi_cue: false,
            distant_verbal_multi_cue: false,
            to_stay_on: false,
            pedestrian_crossing: false,
            merge_to_relative_direction: RelativeDirection::None,
            drive_on_right: true,
            has_time_restrictions: false,
            has_right_traversable_outbound_intersecting_edge: false,
            has_left_traversable_outbound_intersecting_edge: false,
            include_verbal_pre_transition_length: false,
            contains_obvious_maneuver: false,
            roundabout_exit_count: 0,
            has_combined_enter_exit_roundabout: false,
            roundabout_length: 0.0,
            roundabout_exit_length: 0.0,
            roundabout_exit_street_names: StreetNames::default(),
            roundabout_exit_begin_street_names: StreetNames::default(),
            roundabout_exit_signs: Signs::default(),
            roundabout_exit_begin_heading: 0,
            roundabout_exit_turn_degree: 0,
            roundabout_exit_shape_index: 0,
            has_collapsed_small_end_ramp_fork: false,
            has_collapsed_merge_maneuver: false,
            has_long_street_name: false,
            bss_info: BikeShareStationInfo::default(),
            elevator: false,
            indoor_steps: false,
            escalator: false,
            building_enter: false,
            building_exit: false,
            has_level_changes: false,
            end_level_ref: String::new(),
            landmarks: Vec::new(),
            transit_connection: false,
            travel_mode: TravelMode::KDrive as i32,
            vehicle_type: VehicleType::KCar as i32,
            pedestrian_type: PedestrianType::KFoot as i32,
            bicycle_type: BicycleType::KRoad as i32,
            transit_type: TransitType::KRail as i32,
            bss_maneuver_type: BssManeuverType::KNoneAction as i32,
            verbal_formatter: None,
            guidance_views: Vec::new(),
        }
    }
}

impl Maneuver {
    #[inline]
    pub(crate) fn is_type(&self, maneuver_type: Type) -> bool {
        self.maneuver_type == maneuver_type as i32
    }

    #[inline]
    pub(crate) fn set_type(&mut self, maneuver_type: Type) {
        self.maneuver_type = maneuver_type as i32;
    }

    #[inline]
    pub(crate) fn is_any_type(&self, types: &[Type]) -> bool {
        types.iter().any(|t| self.is_type(*t))
    }

    #[inline]
    pub(crate) fn is_start_type(&self) -> bool {
        self.is_any_type(&[Type::KStart, Type::KStartLeft, Type::KStartRight])
    }

    #[inline]
    pub(crate) fn is_destination_type(&self) -> bool {
        self.is_any_type(&[
            Type::KDestination,
            Type::KDestinationLeft,
            Type::KDestinationRight,
        ])
    }

    #[inline]
    pub(crate) fn is_merge_type(&self) -> bool {
        self.is_any_type(&[Type::KMerge, Type::KMergeLeft, Type::KMergeRight])
    }

    pub(crate) fn is_left_type(&self) -> bool {
        self.is_any_type(&[
            Type::KSlightLeft,
            Type::KLeft,
            Type::KSharpLeft,
            Type::KUturnLeft,
            Type::KRampLeft,
            Type::KExitLeft,
            Type::KStayLeft,
            Type::KDestinationLeft,
            Type::KMergeLeft,
        ])
    }

    pub(crate) fn is_right_type(&self) -> bool {
        self.is_any_type(&[
            Type::KSlightRight,
            Type::KRight,
            Type::KSharpRight,
            Type::KUturnRight,
            Type::KRampRight,
            Type::KExitRight,
            Type::KStayRight,
            Type::KDestinationRight,
            Type::KMergeRight,
        ])
    }

    #[inline]
    pub(crate) fn is_transit(&self) -> bool {
        self.is_any_type(&[
            Type::KTransit,
            Type::KTransitTransfer,
            Type::KTransitRemainOn,
        ])
    }

    #[inline]
    pub(crate) fn has_mode(&self, mode: TravelMode) -> bool {
        self.travel_mode == mode as i32
    }

    #[inline]
    pub(crate) fn is_blind(&self) -> bool {
        self.pedestrian_type == PedestrianType::KBlind as i32
    }

    /// The length in kilometers, or in miles.
    #[inline]
    pub(crate) fn length_in(&self, miles: bool) -> f32 {
        if miles {
            self.length * MILES_PER_KM
        } else {
            self.length
        }
    }

    /// The roundabout exit length in kilometers, or in miles.
    #[inline]
    pub(crate) fn roundabout_exit_length_in(&self, miles: bool) -> f32 {
        if miles {
            self.roundabout_exit_length * MILES_PER_KM
        } else {
            self.roundabout_exit_length
        }
    }

    #[inline]
    pub(crate) fn has_street_names(&self) -> bool {
        !self.street_names.is_empty()
    }

    #[inline]
    pub(crate) fn has_begin_street_names(&self) -> bool {
        !self.begin_street_names.is_empty()
    }

    #[inline]
    pub(crate) fn has_cross_street_names(&self) -> bool {
        !self.cross_street_names.is_empty()
    }

    /// Whether this maneuver's names all appear in the other's (`HasSameNames`).
    pub(crate) fn has_same_names(&self, other: &Self, allow_begin_name_consistency: bool) -> bool {
        if (allow_begin_name_consistency || !self.begin_intersecting_edge_name_consistency)
            && self.has_street_names()
        {
            let same = other.street_names.common_street_names(&self.street_names);
            return !same.is_empty() && self.street_names.len() == same.len();
        }
        false
    }

    /// Whether this maneuver's base names all appear in the other's (`HasSimilarNames`).
    pub(crate) fn has_similar_names(
        &self,
        other: &Self,
        allow_begin_name_consistency: bool,
    ) -> bool {
        if (allow_begin_name_consistency || !self.begin_intersecting_edge_name_consistency)
            && self.has_street_names()
        {
            let similar = other.street_names.common_base_names(&self.street_names);
            return !similar.is_empty() && self.street_names.len() == similar.len();
        }
        false
    }

    /// Whether an internal intersection's name can serve as a cross street name
    /// (`HasUsableInternalIntersectionName`).
    pub(crate) fn has_usable_internal_intersection_name(&self) -> bool {
        let link_count = self.end_node_index.wrapping_sub(self.begin_node_index);
        self.internal_intersection
            && self.has_street_names()
            && (link_count == 1 || link_count == 3)
    }

    #[inline]
    pub(crate) fn has_verbal_multi_cue(&self) -> bool {
        self.imminent_verbal_multi_cue || self.distant_verbal_multi_cue
    }

    #[inline]
    pub(crate) fn unnamed_walkway(&self) -> bool {
        self.trail_type == TrailType::UnnamedWalkway
    }

    #[inline]
    pub(crate) fn unnamed_cycleway(&self) -> bool {
        self.trail_type == TrailType::UnnamedCycleway
    }

    #[inline]
    pub(crate) fn unnamed_mountain_bike_trail(&self) -> bool {
        self.trail_type == TrailType::UnnamedMtbTrail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_directions() {
        assert_eq!(RelativeDirection::of(0), RelativeDirection::KeepStraight);
        assert_eq!(RelativeDirection::of(30), RelativeDirection::KeepStraight);
        assert_eq!(RelativeDirection::of(31), RelativeDirection::Right);
        assert_eq!(RelativeDirection::of(159), RelativeDirection::Right);
        assert_eq!(RelativeDirection::of(160), RelativeDirection::Reverse);
        assert_eq!(RelativeDirection::of(200), RelativeDirection::Reverse);
        assert_eq!(RelativeDirection::of(201), RelativeDirection::Left);
        assert_eq!(RelativeDirection::of(329), RelativeDirection::Left);
        assert_eq!(RelativeDirection::of(330), RelativeDirection::KeepStraight);
        assert_eq!(RelativeDirection::of(400), RelativeDirection::KeepStraight);
    }
}
