//! Assigning maneuver types (`SetManeuverType` and `SetSimpleDirectionalManeuverType`).

use super::builder::{ManeuversBuilder, are_roundabouts_processable};
use crate::error::OdinError;
use crate::maneuver::{Maneuver, RelativeDirection};
use crate::trip::{EdgeExt, LegExt, NodeExt};
use crate::turn::{TurnType, is_forward};
use valhalla_proto::directions_leg::maneuver::Type;
use valhalla_proto::trip_leg::{Edge, Use};
use valhalla_proto::{RoadClass, TravelMode};

/// A highway continue shorter than this (km) stays a continue.
///
/// Picked because the next rounded announcement happens at a quarter mile or 400 meters.
pub(super) const SHORT_CONTINUE_THRESHOLD: f32 = 0.6;

impl ManeuversBuilder<'_> {
    /// Assigns a maneuver's type, unless it already has one (`SetManeuverType`).
    ///
    /// With `none_type_allowed`, internal intersections and turn channels stay untyped
    /// so that they can be combined later.
    pub(super) fn set_maneuver_type(
        &self,
        maneuver: &mut Maneuver,
        none_type_allowed: bool,
    ) -> Result<(), OdinError> {
        if !maneuver.is_type(Type::KNone) {
            return Ok(());
        }
        let begin = maneuver.begin_node_index as usize;
        let prev_edge = self.leg.prev_edge(begin, 1);
        let curr_edge = self.leg.curr_edge(begin);

        // Transit and transit connections are rejected before maneuvers are built,
        // so mainline's transit branches never apply.
        if maneuver.roundabout {
            maneuver.set_type(Type::KRoundaboutEnter);
        } else if prev_edge
            .is_some_and(|prev| are_roundabouts_processable(prev.travel_mode) && prev.roundabout)
        {
            maneuver.set_type(Type::KRoundaboutExit);
        } else if maneuver.fork {
            maneuver.set_type(match maneuver.begin_relative_direction {
                RelativeDirection::KeepRight | RelativeDirection::Right => Type::KStayRight,
                RelativeDirection::KeepLeft | RelativeDirection::Left => Type::KStayLeft,
                _ => Type::KStayStraight,
            });
        } else if none_type_allowed && (maneuver.internal_intersection || maneuver.turn_channel) {
            // Internal intersections and turn channels stay untyped until they are combined.
            maneuver.set_type(Type::KNone);
        } else if let Some(prev) = prev_edge
            && maneuver.ramp
            && (prev.is_highway()
                || maneuver.signs.has_exit_number()
                || (!prev.is_ramp_use()
                    && !self.ramp_leads_to_highway(maneuver)
                    && matches!(
                        maneuver.begin_relative_direction,
                        RelativeDirection::KeepRight | RelativeDirection::KeepLeft
                    )))
        {
            // An exit: from a highway, with an exit number,
            // or a keep left or right onto a ramp that does not lead to a highway.
            maneuver.set_type(match maneuver.begin_relative_direction {
                RelativeDirection::KeepRight | RelativeDirection::Right => Type::KExitRight,
                RelativeDirection::KeepLeft | RelativeDirection::Left => Type::KExitLeft,
                // Mainline has a TODO to decide how to handle other directions.
                _ if maneuver.drive_on_right => Type::KExitRight,
                _ => Type::KExitLeft,
            });
        } else if let Some(prev) = prev_edge
            && maneuver.ramp
            && !prev.is_highway()
        {
            maneuver.set_type(match maneuver.begin_relative_direction {
                // Mainline has a TODO to decide how to handle no direction; it picks right.
                RelativeDirection::KeepRight
                | RelativeDirection::Right
                | RelativeDirection::None => Type::KRampRight,
                RelativeDirection::KeepLeft | RelativeDirection::Left => Type::KRampLeft,
                RelativeDirection::KeepStraight => Type::KRampStraight,
                RelativeDirection::Reverse => {
                    if maneuver.drive_on_right {
                        if maneuver.turn_degree < 180 {
                            Type::KRampRight
                        } else {
                            Type::KRampLeft
                        }
                    } else if maneuver.turn_degree > 180 {
                        Type::KRampLeft
                    } else {
                        Type::KRampRight
                    }
                }
            });
        } else if self.is_merge_maneuver_type(maneuver, prev_edge, curr_edge)? {
            maneuver.set_type(match maneuver.merge_to_relative_direction {
                RelativeDirection::KeepRight => Type::KMergeRight,
                RelativeDirection::KeepLeft => Type::KMergeLeft,
                _ => Type::KMerge,
            });
        } else if maneuver.ferry || maneuver.rail_ferry {
            maneuver.set_type(Type::KFerryEnter);
        } else if prev_edge.is_some_and(|prev| prev.is_ferry_use() || prev.is_rail_ferry_use()) {
            maneuver.set_type(Type::KFerryExit);
        } else if maneuver.elevator {
            maneuver.set_type(Type::KElevatorEnter);
        } else if maneuver.indoor_steps || maneuver.is_steps {
            maneuver.set_type(Type::KStepsEnter);
        } else if maneuver.escalator {
            maneuver.set_type(Type::KEscalatorEnter);
        } else if maneuver.building_enter {
            maneuver.set_type(Type::KBuildingEnter);
        } else if maneuver.building_exit {
            maneuver.set_type(Type::KBuildingExit);
        } else if maneuver.has_level_changes && !maneuver.end_level_ref.is_empty() {
            maneuver.set_type(Type::KLevelChange);
        } else if curr_edge
            .ok_or(OdinError::MissingEdge { node_index: begin })?
            .has_mode(TravelMode::KPedestrian)
            && prev_edge.is_some_and(|prev| prev.has_mode(TravelMode::KDrive))
        {
            maneuver.set_type(Type::KParkVehicle);
        } else {
            self.set_simple_directional_maneuver_type(maneuver, prev_edge, curr_edge)?;
        }
        Ok(())
    }

    /// Assigns a type from the turn degree alone, refined by the intersecting edges
    /// (`SetSimpleDirectionalManeuverType`).
    #[expect(clippy::too_many_lines, reason = "mirrors mainline's single switch")]
    fn set_simple_directional_maneuver_type(
        &self,
        maneuver: &mut Maneuver,
        prev_edge: Option<&Edge>,
        curr_edge: Option<&Edge>,
    ) -> Result<(), OdinError> {
        let begin = maneuver.begin_node_index as usize;
        let node = self.leg.node_at(begin);
        let edges = prev_edge.zip(curr_edge).zip(node);
        match TurnType::of(maneuver.turn_degree) {
            TurnType::Straight => {
                maneuver.set_type(Type::KContinue);
                let Some(((prev_edge, curr_edge), node)) = edges else {
                    return Ok(());
                };
                let from = prev_edge.end_heading;
                let mode = prev_edge.travel_mode;
                let direction = maneuver.begin_relative_direction;
                // The maneuver's begin edge is the current edge.
                if curr_edge.is_turn_channel_use() && direction != RelativeDirection::KeepStraight {
                    // A turn channel that is not straight is a slight turn.
                    if direction == RelativeDirection::KeepRight {
                        maneuver.set_type(Type::KSlightRight);
                    } else if direction == RelativeDirection::KeepLeft {
                        maneuver.set_type(Type::KSlightLeft);
                    }
                } else if curr_edge.internal_intersection {
                    // A straight turn type at an internal intersection, but a turning direction.
                    if matches!(
                        direction,
                        RelativeDirection::Left | RelativeDirection::KeepLeft
                    ) {
                        maneuver.set_type(Type::KSlightLeft);
                    } else if matches!(
                        direction,
                        RelativeDirection::Right | RelativeDirection::KeepRight
                    ) {
                        maneuver.set_type(Type::KSlightRight);
                    }
                } else if curr_edge.is_highway() && maneuver.length < SHORT_CONTINUE_THRESHOLD {
                    // Keep a short highway continue as is.
                } else if matches!(
                    direction,
                    RelativeDirection::KeepRight | RelativeDirection::KeepLeft
                ) && node.has_similar_straight_significant_road_class_xedge(
                    maneuver.turn_degree,
                    from,
                    mode,
                    prev_edge.road_class,
                ) {
                    let (stay, slight) = if direction == RelativeDirection::KeepRight {
                        (Type::KStayRight, Type::KSlightRight)
                    } else {
                        (Type::KStayLeft, Type::KSlightLeft)
                    };
                    if curr_edge.is_highway()
                        || node.has_forward_traversable_use_xedge(from, mode, Use::KRampUse)
                    {
                        // Only certain highways change from a continue.
                        if node.has_similar_straight_non_ramp_or_same_name_ramp_xedge(
                            maneuver.turn_degree,
                            from,
                            mode,
                        ) {
                            maneuver.set_type(stay);
                        }
                    } else {
                        maneuver.set_type(slight);
                    }
                }
            }
            turn_type @ (TurnType::SlightRight | TurnType::SlightLeft) => {
                let right = turn_type == TurnType::SlightRight;
                maneuver.set_type(if right {
                    Type::KSlightRight
                } else {
                    Type::KSlightLeft
                });
                let Some(((prev_edge, _), node)) = edges else {
                    return Ok(());
                };
                let counts =
                    node.intersecting_edge_counts(prev_edge.end_heading, prev_edge.travel_mode);
                let same_side = if right { counts.right } else { counts.left };
                // Even with a slight turn along the path, a continue reads better here.
                if maneuver.begin_relative_direction == RelativeDirection::KeepStraight
                    && !node.has_forward_traversable_significant_road_class_xedge(
                        prev_edge.end_heading,
                        prev_edge.travel_mode,
                        prev_edge.road_class,
                    )
                    && (same_side > 0 || (counts.right == 0 && counts.left == 0))
                {
                    maneuver.set_type(Type::KContinue);
                }
            }
            TurnType::Right => {
                maneuver.set_type(Type::KRight);
                let Some(((prev_edge, _), node)) = edges else {
                    return Ok(());
                };
                let mode = maneuver.travel_mode;
                if node.has_traversable_outbound_intersecting_edge(mode) {
                    let degree = maneuver.turn_degree;
                    let right_most =
                        node.right_most_turn_degree(degree, prev_edge.end_heading, mode);
                    if degree == right_most {
                        maneuver.set_type(Type::KRight);
                    } else if degree < right_most
                        && !node.has_specified_turn_xedge(
                            TurnType::SlightRight,
                            prev_edge.end_heading,
                            mode,
                        )
                    {
                        maneuver.set_type(Type::KSlightRight);
                    } else if degree > right_most
                        && !node.has_specified_turn_xedge(
                            TurnType::SharpRight,
                            prev_edge.end_heading,
                            mode,
                        )
                    {
                        maneuver.set_type(Type::KSharpRight);
                    }
                }
            }
            TurnType::SharpRight => maneuver.set_type(Type::KSharpRight),
            TurnType::Reverse => {
                let uturn = if maneuver.internal_left_turn_count
                    > maneuver.internal_right_turn_count
                {
                    Type::KUturnLeft
                } else if maneuver.internal_right_turn_count > maneuver.internal_left_turn_count {
                    Type::KUturnRight
                } else if maneuver.begin_relative_direction == RelativeDirection::KeepLeft {
                    Type::KUturnLeft
                } else if maneuver.begin_relative_direction == RelativeDirection::KeepRight {
                    Type::KUturnRight
                } else if self
                    .leg
                    .curr_edge(begin)
                    .ok_or(OdinError::MissingEdge { node_index: begin })?
                    .drive_on_right()
                {
                    if maneuver.turn_degree < 180 {
                        Type::KUturnRight
                    } else {
                        Type::KUturnLeft
                    }
                } else if maneuver.turn_degree > 180 {
                    Type::KUturnLeft
                } else {
                    Type::KUturnRight
                };
                maneuver.set_type(uturn);
            }
            TurnType::SharpLeft => maneuver.set_type(Type::KSharpLeft),
            TurnType::Left => {
                maneuver.set_type(Type::KLeft);
                let Some(((prev_edge, _), node)) = edges else {
                    return Ok(());
                };
                let mode = maneuver.travel_mode;
                if node.has_traversable_outbound_intersecting_edge(mode) {
                    let degree = maneuver.turn_degree;
                    let left_most = node.left_most_turn_degree(degree, prev_edge.end_heading, mode);
                    if degree == left_most {
                        maneuver.set_type(Type::KLeft);
                    } else if degree > left_most
                        && !node.has_specified_turn_xedge(
                            TurnType::SlightLeft,
                            prev_edge.end_heading,
                            mode,
                        )
                    {
                        maneuver.set_type(Type::KSlightLeft);
                    } else if degree < left_most
                        && !node.has_specified_turn_xedge(
                            TurnType::SharpLeft,
                            prev_edge.end_heading,
                            mode,
                        )
                    {
                        maneuver.set_type(Type::KSharpLeft);
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether a maneuver merges from a ramp onto a highway or a similar road (`IsMergeManeuverType`).
    ///
    /// When it does, this also sets the maneuver's merge direction.
    fn is_merge_maneuver_type(
        &self,
        maneuver: &mut Maneuver,
        prev_edge: Option<&Edge>,
        curr_edge: Option<&Edge>,
    ) -> Result<bool, OdinError> {
        let Some(prev_edge) = prev_edge.filter(|prev| prev.is_ramp_use()) else {
            return Ok(false);
        };
        let begin = maneuver.begin_node_index as usize;
        let curr_edge = curr_edge.ok_or(OdinError::MissingEdge { node_index: begin })?;
        let node = self.node(begin)?;
        // The current edge is a highway, or a trunk or primary one-way road continuing forward
        // with a name it shares with an intersecting edge.
        if !curr_edge.is_ramp_use()
            && (curr_edge.is_highway()
                || ((curr_edge.road_class == RoadClass::KTrunk as i32
                    || curr_edge.road_class == RoadClass::KPrimary as i32)
                    && curr_edge.is_oneway()
                    && is_forward(maneuver.turn_degree)
                    && node.has_intersecting_edge_curr_name_consistency()))
        {
            maneuver.merge_to_relative_direction =
                Self::determine_merge_to_relative_direction(node, prev_edge);
            return Ok(true);
        }
        Ok(false)
    }

    /// Whether a ramp maneuver leads onto a highway, past any ramps, turn channels,
    /// and internal edges (`RampLeadsToHighway`).
    pub(super) fn ramp_leads_to_highway(&self, maneuver: &Maneuver) -> bool {
        if !maneuver.ramp {
            return false;
        }
        for node_index in (maneuver.end_node_index as usize)..self.leg.last_node_index() {
            match self.leg.curr_edge(node_index) {
                Some(edge)
                    if edge.is_ramp_use()
                        || edge.is_turn_channel_use()
                        || edge.internal_intersection => {}
                Some(edge) if edge.is_highway() => return true,
                _ => return false,
            }
        }
        false
    }
}
