//! Combining maneuvers that read as one instruction (`ManeuversBuilder::Combine`).
//!
//! Mainline walks a `std::list` with three iterators, `prev_man`, `curr_man`, and `next_man`.
//! `next_man` always follows `curr_man` directly,
//! and `prev_man` is either the maneuver before `curr_man` or, at the start, `curr_man` itself.
//! So here `curr` is an index, `next` is `curr + 1`, and `prev` is an index that is
//! `curr - 1` or `curr`.
//! Erasing `curr_man` (and continuing with `next_man`) leaves `curr` in place;
//! erasing `next_man` leaves both in place.

use super::builder::ManeuversBuilder;
use crate::error::OdinError;
use crate::maneuver::{Maneuver, RelativeDirection};
use crate::trip::{EdgeExt, LegExt, NodeExt};
use crate::turn::{TurnType, turn_degree};
use valhalla_proto::TravelMode;
use valhalla_proto::directions_leg::maneuver::Type;

/// Mainline's `kMaxInternalLength`, in meters.
const MAX_INTERNAL_LENGTH: f32 = 32.0;
/// Mainline's `kMaxTurnChannelLength`, in meters.
const MAX_TURN_CHANNEL_LENGTH: f32 = 200.0;
/// Mainline's `midgard::kKmPerMeter`.
const KM_PER_METER: f32 = 0.001;
/// Turn channels shorter than this (km) combine whatever their exit angle.
const SHORT_TURN_CHANNEL_THRESHOLD: f32 = 0.036;
/// The bounds of the somewhat-straight cone after a turn channel.
const TURN_CHANNEL_TURN_DEGREE_LOWER_BOUND: u32 = 290;
const TURN_CHANNEL_TURN_DEGREE_UPPER_BOUND: u32 = 70;

impl ManeuversBuilder<'_> {
    /// Combines maneuvers until no more can be combined (`Combine`).
    pub(super) fn combine(&self, maneuvers: &mut Vec<Maneuver>) -> Result<(), OdinError> {
        let mut combined = true;
        while combined {
            combined = false;
            let mut prev = 0;
            let mut curr = 0;
            while curr + 1 < maneuvers.len() {
                let next = curr + 1;
                let is_first = curr == 0;
                if self.combine_step(maneuvers, &mut prev, &mut curr, next, is_first)? {
                    combined = true;
                }
            }
        }
        Ok(())
    }

    /// One step of the combine walk, returning whether it combined anything.
    #[expect(
        clippy::too_many_lines,
        reason = "mirrors mainline's single decision chain"
    )]
    fn combine_step(
        &self,
        maneuvers: &mut Vec<Maneuver>,
        prev: &mut usize,
        curr: &mut usize,
        next: usize,
        is_first: bool,
    ) -> Result<bool, OdinError> {
        let (c, n) = (&maneuvers[*curr], &maneuvers[next]);
        let p = &maneuvers[*prev];
        let common_base_names = c.street_names.common_base_names(&n.street_names);
        let next_begin_edge = self.leg.curr_edge(n.begin_node_index as usize);

        // Moves on without combining.
        let advance = |prev: &mut usize, curr: &mut usize| {
            *prev = *curr;
            *curr = next;
        };

        // Do not combine elevators, indoor steps, escalators, level changes, or building entrances
        // with anything; nor transit connections, changes of driving side or travel mode,
        // or anything with the destination.
        if c.elevator
            || n.elevator
            || c.indoor_steps
            || n.indoor_steps
            || c.escalator
            || n.escalator
            || c.has_level_changes != n.has_level_changes
            || c.building_enter
            || n.building_enter
            || c.building_exit
            || n.building_exit
            || c.transit_connection
            || n.transit_connection
            || c.drive_on_right != n.drive_on_right
            || c.travel_mode != n.travel_mode
            || n.is_type(Type::KDestination)
        {
            advance(prev, curr);
            return Ok(false);
        }

        // Double left or right turns through a short unspecified internal intersection
        // become a U-turn.
        let possible_internal = Self::possible_unspecified_internal_maneuver(p, c, n);
        let reverse = RelativeDirection::of(turn_degree(p.end_heading, n.begin_heading))
            == RelativeDirection::Reverse;
        for (direction, uturn) in [
            (RelativeDirection::Left, Type::KUturnLeft),
            (RelativeDirection::Right, Type::KUturnRight),
        ] {
            if possible_internal
                && p.has_similar_names(n, true)
                && RelativeDirection::of(c.turn_degree) == direction
                && RelativeDirection::of(n.turn_degree) == direction
                && reverse
            {
                Self::combine_unspecified_internal_maneuver(maneuvers, *prev, *curr, uturn);
                return Ok(true);
            }
        }

        if n.fork || n.tee || c.ferry || n.ferry {
            advance(prev, curr);
            return Ok(false);
        }

        if c.internal_intersection && !n.is_destination_type() {
            self.combine_internal_maneuver(maneuvers, *prev, *curr, is_first)?;
            if is_first {
                *prev = *curr;
            }
            return Ok(true);
        }

        if self.is_turn_channel_maneuver_combinable(p, c, n, is_first) {
            self.combine_turn_channel_maneuver(maneuvers, *prev, *curr, is_first)?;
            if is_first {
                *prev = *curr;
            }
            return Ok(true);
        }

        if n.intersecting_forward_edge {
            advance(prev, curr);
            return Ok(false);
        }

        // Blind users are told about nodes, steps, bridges, and tunnels.
        if (c.is_blind() && n.is_blind())
            && (c.node_type.is_some()
                || n.node_type.is_some()
                || c.is_steps
                || n.is_steps
                || c.is_bridge
                || n.is_bridge
                || c.is_tunnel
                || n.is_tunnel)
        {
            advance(prev, curr);
            return Ok(false);
        }

        if c.trail_type != n.trail_type {
            advance(prev, curr);
            return Ok(false);
        }

        let next_begin_is_turn_channel = next_begin_edge.is_none_or(EdgeExt::is_turn_channel_use);
        let straight_and_plain = n.begin_relative_direction == RelativeDirection::KeepStraight
            && !next_begin_is_turn_channel
            && !n.internal_intersection
            && !c.ramp
            && !n.ramp
            && !c.roundabout
            && !n.roundabout;

        // Combine a "same name, straight" next maneuver into the current one.
        if straight_and_plain && !common_base_names.is_empty() {
            let curr_man = &mut maneuvers[*curr];
            if !curr_man.has_begin_street_names()
                && !curr_man.portions_highway
                && curr_man.street_names.len() > common_base_names.len()
            {
                curr_man.begin_street_names = curr_man.street_names.duplicate();
            }
            curr_man.street_names = common_base_names;
            Self::combine_maneuvers(maneuvers, *curr, next);
            return Ok(true);
        }

        // Combine unnamed straight maneuvers.
        if straight_and_plain
            && !c.has_street_names()
            && !n.has_street_names()
            && !c.is_transit()
            && !n.is_transit()
        {
            Self::combine_maneuvers(maneuvers, *curr, next);
            return Ok(true);
        }

        if self.are_ramp_maneuvers_combinable(c, n) {
            Self::combine_maneuvers(maneuvers, *curr, next);
            return Ok(true);
        }

        if self.is_next_maneuver_obvious(maneuvers, *curr, next) {
            let next_names =
                (!c.has_street_names() && n.has_street_names()).then(|| n.street_names.duplicate());
            let curr_man = &mut maneuvers[*curr];
            if let Some(names) = next_names {
                curr_man.street_names = names;
            }
            curr_man.contains_obvious_maneuver = true;
            curr_man.turn_channel = false;
            Self::combine_maneuvers(maneuvers, *curr, next);
            return Ok(true);
        }

        // Combine a short non-internal left or right turn with a following straight ramp.
        // (OSM data should already mark these as internal, so this should not happen for OSM.)
        let curr_direction = RelativeDirection::of(c.turn_degree);
        if possible_internal
            && matches!(
                curr_direction,
                RelativeDirection::Left | RelativeDirection::Right
            )
            && n.is_type(Type::KRampStraight)
        {
            Self::combine_unspecified_internal_maneuver(maneuvers, *prev, *curr, Type::KNone);
            return Ok(true);
        }

        advance(prev, curr);
        Ok(false)
    }

    /// Whether the current maneuver may be an internal intersection that the data does not mark
    /// (`PossibleUnspecifiedInternalManeuver`).
    fn possible_unspecified_internal_maneuver(
        prev: &Maneuver,
        curr: &Maneuver,
        next: &Maneuver,
    ) -> bool {
        !curr.internal_intersection
            && curr.has_mode(TravelMode::KDrive)
            && !prev.roundabout
            && !curr.roundabout
            && !next.roundabout
            && curr.length <= MAX_INTERNAL_LENGTH * KM_PER_METER
            && !curr.is_start_type()
            && !next.is_destination_type()
    }

    /// Folds the current (unspecified internal) maneuver into the next one,
    /// giving it the specified type (`CombineUnspecifiedInternalManeuver`).
    fn combine_unspecified_internal_maneuver(
        maneuvers: &mut Vec<Maneuver>,
        prev: usize,
        curr: usize,
        maneuver_type: Type,
    ) {
        let prev_end_heading = maneuvers[prev].end_heading;
        let curr_man = maneuvers.remove(curr);
        let next_man = &mut maneuvers[curr];
        next_man.turn_degree = turn_degree(prev_end_heading, next_man.begin_heading);
        if curr_man.has_street_names() {
            next_man.cross_street_names = curr_man.street_names.duplicate();
        }
        next_man.begin_relative_direction = RelativeDirection::of(next_man.turn_degree);
        absorb_preceding(next_man, &curr_man);
        next_man.set_type(maneuver_type);
    }

    /// Folds the current internal intersection maneuver into the next one (`CombineInternalManeuver`).
    fn combine_internal_maneuver(
        &self,
        maneuvers: &mut Vec<Maneuver>,
        prev: usize,
        curr: usize,
        is_first: bool,
    ) -> Result<(), OdinError> {
        let prev_end_heading = maneuvers[prev].end_heading;
        let curr_man = maneuvers.remove(curr);
        let next_man = &mut maneuvers[curr];

        let from_heading = if is_first {
            curr_man.end_heading
        } else {
            prev_end_heading
        };
        next_man.turn_degree = turn_degree(from_heading, next_man.begin_heading);

        if curr_man.has_usable_internal_intersection_name() {
            next_man.cross_street_names = curr_man.street_names.duplicate();
        }

        next_man.internal_right_turn_count = curr_man.internal_right_turn_count;
        next_man.internal_left_turn_count = curr_man.internal_left_turn_count;

        next_man.begin_relative_direction = RelativeDirection::of(next_man.turn_degree);
        // A straight path through an intersection with both left and right internal turns
        // takes its direction from the internal edges instead.
        if next_man.begin_relative_direction == RelativeDirection::KeepStraight
            && curr_man.internal_left_turn_count > 0
            && curr_man.internal_right_turn_count > 0
        {
            next_man.begin_relative_direction =
                RelativeDirection::of(turn_degree(prev_end_heading, curr_man.end_heading));
        }

        absorb_preceding(next_man, &curr_man);
        // Signs are deliberately not copied from the internal maneuver; mainline says that
        // would produce invalid results.
        self.retype_combined(next_man, is_first)
    }

    /// Folds the current turn channel maneuver into the next one (`CombineTurnChannelManeuver`).
    fn combine_turn_channel_maneuver(
        &self,
        maneuvers: &mut Vec<Maneuver>,
        prev: usize,
        curr: usize,
        is_first: bool,
    ) -> Result<(), OdinError> {
        let prev_end_heading = maneuvers[prev].end_heading;
        let curr_man = maneuvers.remove(curr);
        let next_man = &mut maneuvers[curr];

        let from_heading = if is_first {
            curr_man.end_heading
        } else {
            prev_end_heading
        };
        next_man.turn_degree = turn_degree(from_heading, next_man.begin_heading);
        next_man.begin_relative_direction = curr_man.begin_relative_direction;
        absorb_preceding(next_man, &curr_man);
        if curr_man.signs.has_any() && !next_man.signs.has_any() {
            next_man.signs = curr_man.signs;
        }
        self.retype_combined(next_man, is_first)
    }

    /// Gives a maneuver that absorbed its predecessor its type again.
    fn retype_combined(&self, maneuver: &mut Maneuver, is_first: bool) -> Result<(), OdinError> {
        if is_first {
            maneuver.set_type(Type::KStart);
            Ok(())
        } else {
            maneuver.set_type(Type::KNone);
            self.set_maneuver_type(maneuver, true)
        }
    }

    /// Appends the next maneuver to the current one and removes it (`CombineManeuvers`).
    pub(super) fn combine_maneuvers(maneuvers: &mut Vec<Maneuver>, curr: usize, next: usize) {
        let next_man = maneuvers.remove(next);
        let curr_man = &mut maneuvers[curr];
        curr_man.length += next_man.length;
        curr_man.time += next_man.time;
        curr_man.basic_time += next_man.basic_time;
        curr_man.end_heading = next_man.end_heading;
        curr_man.end_node_index = next_man.end_node_index;
        curr_man.end_shape_index = next_man.end_shape_index;
        curr_man.end_level_ref = next_man.end_level_ref;
        curr_man.elevator |= next_man.elevator;
        curr_man.indoor_steps |= next_man.indoor_steps;
        curr_man.is_steps |= next_man.is_steps;
        curr_man.escalator |= next_man.escalator;
        curr_man.has_level_changes |= next_man.has_level_changes;
        curr_man.ramp |= next_man.ramp;
        curr_man.ferry |= next_man.ferry;
        curr_man.rail_ferry |= next_man.rail_ferry;
        curr_man.roundabout |= next_man.roundabout;
        curr_man.portions_toll |= next_man.portions_toll;
        curr_man.has_time_restrictions |= next_man.has_time_restrictions;
        curr_man.portions_unpaved |= next_man.portions_unpaved;
        curr_man.portions_highway |= next_man.portions_highway;
        curr_man.contains_obvious_maneuver |= next_man.contains_obvious_maneuver;
    }

    /// Whether the current turn channel maneuver can be folded into the next one
    /// (`IsTurnChannelManeuverCombinable`).
    fn is_turn_channel_maneuver_combinable(
        &self,
        prev: &Maneuver,
        curr: &Maneuver,
        next: &Maneuver,
        is_first: bool,
    ) -> bool {
        if !curr.turn_channel || next.is_destination_type() {
            return false;
        }
        let from_heading = if is_first {
            curr.end_heading
        } else {
            prev.end_heading
        };
        let new_turn_type = TurnType::of(turn_degree(from_heading, next.begin_heading));

        let end = curr.end_node_index as usize;
        let (Some(node), Some(prev_edge), Some(curr_edge)) = (
            self.leg.node_at(end),
            self.leg.prev_edge(end, 1),
            self.leg.curr_edge(end),
        ) else {
            return false;
        };
        // Computed here because collapsing internal intersections may change the next turn degree.
        let post_turn_channel_turn_degree =
            turn_degree(prev_edge.end_heading, curr_edge.begin_heading);
        let within_turn_channel_range = post_turn_channel_turn_degree
            >= TURN_CHANNEL_TURN_DEGREE_LOWER_BOUND
            || post_turn_channel_turn_degree <= TURN_CHANNEL_TURN_DEGREE_UPPER_BOUND;

        // At most 200 m, no forward traversable intersecting edge,
        // and either fairly straight afterwards or short.
        if !(curr.length <= MAX_TURN_CHANNEL_LENGTH * KM_PER_METER
            && !node.has_forward_traversable_intersecting_edge(curr.end_heading, curr.travel_mode)
            && (within_turn_channel_range || curr.length < SHORT_TURN_CHANNEL_THRESHOLD))
        {
            return false;
        }

        let direction = curr.begin_relative_direction;
        let straight_or_reverse = matches!(new_turn_type, TurnType::Reverse | TurnType::Straight);
        // A right turn channel that does not end up turning left.
        if matches!(
            direction,
            RelativeDirection::KeepRight | RelativeDirection::Right
        ) && next.begin_relative_direction != RelativeDirection::Left
            && (straight_or_reverse
                || matches!(
                    new_turn_type,
                    TurnType::SlightRight | TurnType::Right | TurnType::SharpRight
                ))
        {
            return true;
        }
        // A left turn channel that does not end up turning right.
        if matches!(
            direction,
            RelativeDirection::KeepLeft | RelativeDirection::Left
        ) && next.begin_relative_direction != RelativeDirection::Right
            && (straight_or_reverse
                || matches!(
                    new_turn_type,
                    TurnType::SlightLeft | TurnType::Left | TurnType::SharpLeft
                ))
        {
            return true;
        }
        // A straight "turn channel".
        direction == RelativeDirection::KeepStraight && new_turn_type == TurnType::Straight
    }

    /// Whether consecutive ramp maneuvers read as one (`AreRampManeuversCombinable`).
    fn are_ramp_maneuvers_combinable(&self, curr: &Maneuver, next: &Maneuver) -> bool {
        if curr.ramp
            && next.ramp
            && !next.fork
            && !curr.internal_intersection
            && !next.internal_intersection
        {
            let Some(node) = self.leg.node_at(next.begin_node_index as usize) else {
                return false;
            };
            return !node.has_traversable_outbound_intersecting_edge(next.travel_mode)
                || node.is_straightest_traversable_intersecting_edge_reversed(
                    curr.end_heading,
                    next.travel_mode,
                )
                || next.is_type(Type::KRampStraight);
        }
        false
    }

    /// Whether the next maneuver is an obvious continuation of the current one (`IsNextManeuverObvious`).
    fn is_next_maneuver_obvious(&self, maneuvers: &[Maneuver], curr: usize, next: usize) -> bool {
        let (curr_man, next_man) = (&maneuvers[curr], &maneuvers[next]);
        if !next_man.is_type(Type::KContinue) {
            return false;
        }
        let node = self.leg.node_at(next_man.begin_node_index as usize);

        // Obvious if there are no traversable intersecting edges.
        if node.is_some_and(|node| !node.has_traversable_intersecting_edge(next_man.travel_mode)) {
            return true;
        }
        if next_man.signs.has_exit_number() {
            return false;
        }

        // Ramp forks.
        if curr_man.ramp && curr_man.fork && !curr_man.contains_obvious_maneuver {
            if curr_man.is_type(Type::KStayStraight) {
                return true;
            }
            if let Some(node) = node {
                let counts =
                    node.intersecting_edge_counts(curr_man.end_heading, curr_man.travel_mode);
                // Obvious if forking left with no edges on the left, or right with none on the right.
                if (curr_man.is_type(Type::KStayLeft) && counts.left == 0)
                    || (curr_man.is_type(Type::KStayRight) && counts.right == 0)
                {
                    return true;
                }
            }
            return false;
        }

        // Obvious if a short continue is followed by something other than a continue.
        if next_man.length < super::maneuver_type::SHORT_CONTINUE_THRESHOLD
            && maneuvers
                .get(next + 1)
                .is_some_and(|next_next| !next_next.is_type(Type::KContinue))
        {
            return true;
        }

        let Some(node) = node else {
            return false;
        };
        if node.is_type(valhalla_proto::trip_leg::node::Type::KMotorwayJunction) {
            return false;
        }
        !node.has_non_backward_traversable_same_name_ramp_intersecting_edge(
            curr_man.end_heading,
            next_man.travel_mode,
        )
    }
}

/// Adds a preceding maneuver's length and times to a maneuver and takes over its beginning.
fn absorb_preceding(maneuver: &mut Maneuver, preceding: &Maneuver) {
    maneuver.length += preceding.length;
    maneuver.time += preceding.time;
    maneuver.basic_time += preceding.basic_time;
    maneuver.begin_node_index = preceding.begin_node_index;
    maneuver.begin_shape_index = preceding.begin_shape_index;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combining_appends_the_next_maneuver() {
        let mut maneuvers = vec![
            Maneuver {
                length: 1.0,
                time: 10.0,
                end_node_index: 2,
                ..Maneuver::default()
            },
            Maneuver {
                length: 2.0,
                time: 20.0,
                end_node_index: 5,
                ramp: true,
                ..Maneuver::default()
            },
            Maneuver::default(),
        ];
        ManeuversBuilder::combine_maneuvers(&mut maneuvers, 0, 1);
        assert_eq!(maneuvers.len(), 2);
        assert_eq!(maneuvers[0].length, 3.0);
        assert_eq!(maneuvers[0].time, 30.0);
        assert_eq!(maneuvers[0].end_node_index, 5);
        assert!(maneuvers[0].ramp);
    }
}
