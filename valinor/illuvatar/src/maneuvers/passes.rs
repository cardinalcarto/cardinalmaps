//! The passes over the combined maneuver list (`ManeuversBuilder::Build` after `Combine`).
//!
//! Several of mainline's walks start with `prev_man` at the first maneuver,
//! `curr_man` at the second, and `next_man` at the third,
//! and advance all three together while `next_man` is not the end.
//! Here those walks are loops over `curr` in `1..len - 1`, with `prev = curr - 1`.

use super::builder::{ManeuversBuilder, get_speed, get_time};
use crate::error::OdinError;
use crate::maneuver::{Maneuver, RelativeDirection};
use crate::signs::{Sign, Signs};
use crate::trip::{EdgeExt, LegExt, NodeExt, edge_mut, lane, lanes};
use crate::turn::turn_degree;
use valhalla_proto::TravelMode;
use valhalla_proto::directions_leg::GuidanceView;
use valhalla_proto::directions_leg::guidance_view;
use valhalla_proto::directions_leg::maneuver::Type;
use valhalla_proto::trip_leg::Edge;

/// Forks shorter than this (km) do not get turn lanes.
const SHORT_FORK_THRESHOLD: f32 = 0.05;
/// How far ahead (km) lanes for an upcoming turn are activated.
const UPCOMING_LANES_THRESHOLD: f32 = 3.0;
/// End-of-ramp forks no longer than this (km) are collapsed.
const SMALL_END_RAMP_FORK_THRESHOLD: f32 = 0.125;
/// The maximum number of edges to search for a matching guidance view overlay.
const OVERLAY_SIGN_BOARD_EDGE_MAX: u32 = 5;
/// Street names with more words than this are too long for succinct phrases.
const MAX_WORD_COUNT: usize = 5;
/// Street names with more characters than this are too long for succinct phrases.
const MAX_STREET_NAME_LENGTH: usize = 25;
/// The number of street names verbal pre-transition instructions use.
const VERBAL_PRE_ELEMENT_MAX_COUNT: usize = 2;
/// Mainline's `midgard::kMetersPerKm`.
const METERS_PER_KM: f32 = 1000.0;
/// The bounds of the relatively straight cone for internal edges.
const RELATIVE_STRAIGHT_TURN_DEGREE_LOWER_BOUND: u32 = 330;
const RELATIVE_STRAIGHT_TURN_DEGREE_UPPER_BOUND: u32 = 30;

impl ManeuversBuilder<'_> {
    /// Counts the signs that consecutive maneuvers share, then sorts them (`CountAndSortSigns`).
    pub(super) fn count_and_sort_signs(maneuvers: &mut [Maneuver]) {
        // Walk backwards: `curr` is the later maneuver and `prev` the one before it.
        for curr in (1..maneuvers.len()).rev() {
            let (head, tail) = maneuvers.split_at_mut(curr);
            let (prev_man, curr_man) = (&mut head[curr - 1], &mut tail[0]);

            if prev_man.signs.has_exit_branch()
                && !curr_man.signs.has_exit()
                && curr_man.has_street_names()
            {
                // An exit branch sign that names the street the next maneuver follows.
                count_matching_street_names(&mut prev_man.signs.exit_branch_list, curr_man);
                Signs::sort(&mut prev_man.signs.exit_branch_list);
            } else if prev_man.signs.has_guide_branch()
                && !curr_man.signs.has_guide()
                && curr_man.has_street_names()
            {
                count_matching_street_names(&mut prev_man.signs.guide_branch_list, curr_man);
                Signs::sort(&mut prev_man.signs.guide_branch_list);
            } else if prev_man.signs.has_any() && curr_man.signs.has_any() {
                let (prev_signs, curr_signs) = (&mut prev_man.signs, &mut curr_man.signs);
                Signs::count_and_sort(
                    &mut prev_signs.exit_number_list,
                    &mut curr_signs.exit_number_list,
                );
                Signs::count_and_sort(
                    &mut prev_signs.exit_branch_list,
                    &mut curr_signs.exit_branch_list,
                );
                Signs::count_and_sort(
                    &mut prev_signs.exit_toward_list,
                    &mut curr_signs.exit_toward_list,
                );
                Signs::count_and_sort(
                    &mut prev_signs.exit_name_list,
                    &mut curr_signs.exit_name_list,
                );
                Signs::count_and_sort(
                    &mut prev_signs.guide_branch_list,
                    &mut curr_signs.guide_branch_list,
                );
                Signs::count_and_sort(
                    &mut prev_signs.guide_toward_list,
                    &mut curr_signs.guide_toward_list,
                );
                Signs::count_and_sort(
                    &mut prev_signs.junction_name_list,
                    &mut curr_signs.junction_name_list,
                );
            }
        }
    }

    /// Assigns a type to every maneuver still without one (`ConfirmManeuverTypeAssignment`).
    pub(super) fn confirm_maneuver_type_assignment(
        &self,
        maneuvers: &mut [Maneuver],
    ) -> Result<(), OdinError> {
        for maneuver in maneuvers {
            self.set_maneuver_type(maneuver, false)?;
        }
        Ok(())
    }

    /// Marks maneuvers that pass traversable outbound intersecting edges on either side,
    /// skipping their initial internal and turn channel edges
    /// (`SetTraversableOutboundIntersectingEdgeFlags`).
    pub(super) fn set_traversable_outbound_intersecting_edge_flags(
        &self,
        maneuvers: &mut [Maneuver],
    ) -> Result<(), OdinError> {
        for maneuver in maneuvers {
            let mut found_first_edge = false;
            for node_index in
                (maneuver.begin_node_index as usize)..(maneuver.end_node_index as usize)
            {
                if !found_first_edge {
                    let curr_edge = self.curr(node_index)?;
                    if curr_edge.internal_intersection || curr_edge.is_turn_channel_use() {
                        continue;
                    }
                    found_first_edge = true;
                    continue;
                }
                if let (Some(node), Some(prev_edge)) = (
                    self.leg.node_at(node_index),
                    self.leg.prev_edge(node_index, 1),
                ) {
                    let counts =
                        node.intersecting_edge_counts(prev_edge.end_heading, prev_edge.travel_mode);
                    if counts.right_traversable_outbound > 0 {
                        maneuver.has_right_traversable_outbound_intersecting_edge = true;
                    }
                    if counts.left_traversable_outbound > 0 {
                        maneuver.has_left_traversable_outbound_intersecting_edge = true;
                    }
                    if maneuver.has_right_traversable_outbound_intersecting_edge
                        && maneuver.has_left_traversable_outbound_intersecting_edge
                    {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// Names roundabouts and attaches their exits (`ProcessRoundabouts`).
    ///
    /// Unless the request asks for roundabout exits, the exit maneuver is folded into the entry.
    pub(super) fn process_roundabouts(&self, maneuvers: &mut Vec<Maneuver>) {
        let roundabout_exits = matches!(
            self.options.has_roundabout_exits,
            Some(valhalla_proto::options::HasRoundaboutExits::RoundaboutExits(true))
        );
        let mut curr = 1;
        while curr + 1 < maneuvers.len() {
            let next = curr + 1;
            if maneuvers[curr].roundabout {
                let (head, tail) = maneuvers.split_at_mut(curr);
                let prev_man = &head[curr - 1];
                let (curr_man, rest) = tail.split_at_mut(1);
                let (curr_man, next_man) = (&mut curr_man[0], &rest[0]);

                let non_route_numbers = curr_man.street_names.non_route_numbers();
                curr_man.street_names.clear();
                curr_man.begin_street_names.clear();
                // Use the roundabout's name only if it is neither the incoming nor the outgoing name.
                if !non_route_numbers.is_empty()
                    && non_route_numbers
                        .common_base_names(&prev_man.street_names)
                        .is_empty()
                    && non_route_numbers
                        .common_base_names(&next_man.street_names)
                        .is_empty()
                {
                    curr_man.street_names = non_route_numbers;
                }

                if next_man.is_type(Type::KRoundaboutExit) {
                    if next_man.has_begin_street_names() {
                        if next_man.contains_obvious_maneuver {
                            // Use the begin street names as the street names.
                            curr_man.roundabout_exit_street_names =
                                next_man.begin_street_names.duplicate();
                        } else {
                            curr_man.roundabout_exit_begin_street_names =
                                next_man.begin_street_names.duplicate();
                            curr_man.roundabout_exit_street_names =
                                next_man.street_names.duplicate();
                        }
                    } else {
                        curr_man.roundabout_exit_street_names = next_man.street_names.duplicate();
                    }
                    if next_man.signs.has_any() {
                        curr_man.roundabout_exit_signs = next_man.signs.clone();
                    }

                    if !roundabout_exits {
                        curr_man.has_combined_enter_exit_roundabout = true;
                        curr_man.roundabout_length = curr_man.length;
                        curr_man.roundabout_exit_length = next_man.length;
                        // Kept to compute turn angles when the exit maneuver is suppressed.
                        curr_man.roundabout_exit_begin_heading = next_man.begin_heading;
                        curr_man.roundabout_exit_turn_degree = next_man.turn_degree;
                        curr_man.roundabout_exit_shape_index = curr_man.end_shape_index;
                        curr_man.has_left_traversable_outbound_intersecting_edge =
                            next_man.has_left_traversable_outbound_intersecting_edge;
                        curr_man.has_right_traversable_outbound_intersecting_edge =
                            next_man.has_right_traversable_outbound_intersecting_edge;
                        Self::combine_maneuvers(maneuvers, curr, next);
                    }
                }
            }
            curr += 1;
        }
    }

    /// Marks turns and forks that keep to the same road (`SetToStayOnAttribute`).
    pub(super) fn set_to_stay_on_attribute(maneuvers: &mut [Maneuver]) {
        if maneuvers.len() < 3 {
            return;
        }
        for curr in 1..maneuvers.len() - 1 {
            let (head, tail) = maneuvers.split_at_mut(curr);
            let prev_man = &head[curr - 1];
            let (curr_man, rest) = tail.split_at_mut(1);
            let (curr_man, next_man) = (&mut curr_man[0], &rest[0]);
            let to_stay_on = match Type::try_from(curr_man.maneuver_type) {
                Ok(
                    Type::KSlightRight
                    | Type::KSlightLeft
                    | Type::KRight
                    | Type::KSharpRight
                    | Type::KSharpLeft
                    | Type::KLeft,
                ) => {
                    !curr_man.has_begin_street_names() && curr_man.has_similar_names(prev_man, true)
                }
                Ok(Type::KStayStraight | Type::KStayRight | Type::KStayLeft) => {
                    curr_man.has_similar_names(prev_man, true)
                        && (!curr_man.ramp || curr_man.has_similar_names(next_man, true))
                }
                Ok(Type::KUturnRight | Type::KUturnLeft) => curr_man.has_same_names(prev_man, true),
                _ => false,
            };
            if to_stay_on {
                curr_man.to_stay_on = true;
            }
        }
    }

    /// Gives signless ramps (and unnamed forks) before a merge the merge's street name as a branch
    /// sign (`EnhanceSignlessInterchnages`).
    pub(super) fn enhance_signless_interchanges(maneuvers: &mut [Maneuver]) {
        // `prev` starts at the first maneuver, like `curr`.
        let mut prev = 0;
        for curr in 0..maneuvers.len().saturating_sub(1) {
            let next = curr + 1;
            let (prev_ramp_or_fork, next_merge_name) = {
                let (p, n) = (&maneuvers[prev], &maneuvers[next]);
                (
                    p.ramp || p.fork,
                    n.is_merge_type()
                        .then(|| n.street_names.first().cloned())
                        .flatten(),
                )
            };
            let curr_man = &mut maneuvers[curr];
            if (curr_man.ramp || (curr_man.fork && !curr_man.has_street_names()))
                && !curr_man.signs.has_exit()
                && !prev_ramp_or_fork
                && let Some(name) = next_merge_name
            {
                curr_man.signs.exit_branch_list.push(Sign::new(
                    name.value().to_owned(),
                    name.is_route_number(),
                    name.pronunciation().cloned(),
                ));
            }
            prev = curr;
        }
    }

    /// Attaches junction and signboard guidance views (`ProcessGuidanceViews`).
    pub(super) fn process_guidance_views(&self, maneuvers: &mut [Maneuver]) {
        for maneuver in maneuvers {
            if !maneuver.has_mode(TravelMode::KDrive) {
                continue;
            }
            if let Some(sign) = self
                .leg
                .prev_edge(maneuver.begin_node_index as usize, 1)
                .and_then(|edge| edge.sign.as_ref())
            {
                for base in &sign.guidance_view_junctions {
                    if let Some((prefix, suffix)) = split_pair(&base.text)
                        && base.is_route_number
                    {
                        self.match_guidance_view_junctions(maneuver, prefix, suffix);
                    }
                }
            }
            // Signboards only have base images, so they come from the current edges.
            self.process_guidance_view_signboards(maneuver);
        }
    }

    fn match_guidance_view_junctions(
        &self,
        maneuver: &mut Maneuver,
        base_prefix: &str,
        base_suffix: &str,
    ) {
        let range = (maneuver.begin_node_index..maneuver.end_node_index)
            .take(OVERLAY_SIGN_BOARD_EDGE_MAX as usize);
        for node_index in range {
            let Some(sign) = self
                .leg
                .curr_edge(node_index as usize)
                .and_then(|edge| edge.sign.as_ref())
            else {
                continue;
            };
            for overlay in &sign.guidance_view_junctions {
                if let Some((prefix, suffix)) = split_pair(&overlay.text)
                    && !overlay.is_route_number
                    && prefix == base_prefix
                {
                    maneuver.guidance_views.push(GuidanceView {
                        data_id: self.leg.osm_changeset.to_string(),
                        r#type: guidance_view::Type::KJunction as i32,
                        base_id: format!("{base_prefix}{base_suffix}"),
                        overlay_ids: vec![format!("{prefix}{suffix}")],
                    });
                    return;
                }
            }
        }
    }

    fn process_guidance_view_signboards(&self, maneuver: &mut Maneuver) {
        let range = (maneuver.begin_node_index..maneuver.end_node_index)
            .take(OVERLAY_SIGN_BOARD_EDGE_MAX as usize);
        for node_index in range {
            let Some(sign) = self
                .leg
                .curr_edge(node_index as usize)
                .and_then(|edge| edge.sign.as_ref())
            else {
                continue;
            };
            for base in &sign.guidance_view_signboards {
                if let Some((prefix, suffix)) = split_pair(&base.text)
                    && base.is_route_number
                {
                    maneuver.guidance_views.push(GuidanceView {
                        data_id: self.leg.osm_changeset.to_string(),
                        r#type: guidance_view::Type::KSignboard as i32,
                        base_id: format!("{prefix}{suffix}"),
                        overlay_ids: Vec::new(),
                    });
                }
            }
        }
    }

    /// Moves straight internal edges at the start of a turn into the previous maneuver,
    /// so the turn is announced where it happens
    /// (`UpdateManeuverPlacementForInternalIntersectionTurns`).
    pub(super) fn update_maneuver_placement_for_internal_intersection_turns(
        &mut self,
        maneuvers: &mut [Maneuver],
    ) -> Result<(), OdinError> {
        let is_turn = |m: &Maneuver| {
            m.is_any_type(&[
                Type::KSlightRight,
                Type::KRight,
                Type::KSharpRight,
                Type::KUturnRight,
                Type::KUturnLeft,
                Type::KSharpLeft,
                Type::KLeft,
                Type::KSlightLeft,
                Type::KRampRight,
                Type::KRampLeft,
                Type::KStayRight,
                Type::KStayLeft,
            ])
        };
        let is_relative_straight = |degree: u32| {
            degree >= RELATIVE_STRAIGHT_TURN_DEGREE_LOWER_BOUND
                || degree <= RELATIVE_STRAIGHT_TURN_DEGREE_UPPER_BOUND
        };

        for curr in 1..maneuvers.len() {
            let (head, tail) = maneuvers.split_at_mut(curr);
            let (prev_man, maneuver) = (&mut head[curr - 1], &mut tail[0]);
            if maneuver.is_destination_type() {
                break;
            }
            if !is_turn(maneuver) {
                continue;
            }
            let original_end = maneuver.end_node_index;
            for node_index in maneuver.begin_node_index..original_end {
                let new_node_index = node_index + 1;
                let index = node_index as usize;
                let prev_edge = self.prev(index)?;
                let edge = self.curr(index)?;
                if new_node_index < original_end
                    && prev_man.travel_mode == maneuver.travel_mode
                    && edge.internal_intersection
                    && is_relative_straight(turn_degree(prev_edge.end_heading, edge.begin_heading))
                {
                    self.move_internal_edge_to_previous_maneuver(
                        prev_man,
                        maneuver,
                        new_node_index,
                        index,
                    );
                } else {
                    break;
                }
            }
        }
        Ok(())
    }

    fn move_internal_edge_to_previous_maneuver(
        &mut self,
        prev_man: &mut Maneuver,
        maneuver: &mut Maneuver,
        new_node_index: u32,
        node_index: usize,
    ) {
        let Some(edge) = self.leg.curr_edge(node_index) else {
            return;
        };
        let length_km = edge.length_km;
        let default_speed = edge.default_speed;
        let end_shape_index = edge.end_shape_index;

        prev_man.length += length_km;
        prev_man.basic_time += f64::from(get_time(
            length_km,
            get_speed(prev_man.travel_mode, default_speed),
        ));
        prev_man.end_node_index = new_node_index;
        prev_man.end_shape_index = end_shape_index;
        self.update_time(prev_man);

        maneuver.length -= length_km;
        maneuver.basic_time -= f64::from(get_time(
            length_km,
            get_speed(maneuver.travel_mode, default_speed),
        ));
        maneuver.begin_node_index = new_node_index;
        maneuver.begin_shape_index = end_shape_index;
        self.update_time(maneuver);

        // An internal edge without turn lanes takes the previous edge's.
        if edge.turn_lanes.is_empty() {
            let prev_lanes = self
                .leg
                .prev_edge(node_index, 1)
                .map(|prev| prev.turn_lanes.clone())
                .unwrap_or_default();
            if let Some(edge) = edge_mut(self.leg, node_index) {
                edge.turn_lanes = prev_lanes;
            }
        }
    }

    /// Folds a short ramp fork at the end of a ramp into the ramp, when the following turn
    /// goes the same way (`CollapseSmallEndRampFork`).
    pub(super) fn collapse_small_end_ramp_fork(maneuvers: &mut Vec<Maneuver>) {
        let fork_then_turn_same_direction = |curr: &Maneuver, next: &Maneuver| {
            (curr.is_type(Type::KStayRight)
                && next.is_any_type(&[Type::KSlightRight, Type::KRight, Type::KSharpRight]))
                || (curr.is_type(Type::KStayLeft)
                    && next.is_any_type(&[Type::KSlightLeft, Type::KLeft, Type::KSharpLeft]))
        };
        // `prev` trails `curr`; after a collapse, `curr` takes the next maneuver's place
        // and `prev` stays.
        let mut prev = 0;
        let mut curr = 1;
        while curr + 1 < maneuvers.len() {
            let next = curr + 1;
            let (p, c, n) = (&maneuvers[prev], &maneuvers[curr], &maneuvers[next]);
            if prev != curr
                && !p.has_collapsed_small_end_ramp_fork
                && p.ramp
                && c.ramp
                && !n.ramp
                && c.length <= SMALL_END_RAMP_FORK_THRESHOLD
                && fork_then_turn_same_direction(c, n)
            {
                Self::combine_maneuvers(maneuvers, prev, curr);
                maneuvers[prev].has_collapsed_small_end_ramp_fork = true;
                // The maneuver after the removed one is now at `curr`.
            } else {
                prev = curr;
                curr += 1;
            }
        }
    }

    /// Folds merges into the ramps before them (`CollapseMergeManeuvers`).
    pub(super) fn collapse_merge_maneuvers(maneuvers: &mut Vec<Maneuver>) {
        let mut curr = 0;
        while curr + 1 < maneuvers.len() {
            let next = curr + 1;
            if maneuvers[curr].ramp
                && maneuvers[next].is_merge_type()
                && !maneuvers[curr].has_collapsed_merge_maneuver
            {
                let (head, tail) = maneuvers.split_at_mut(next);
                let (curr_man, next_man) = (&mut head[curr], &tail[0]);
                // Only stay on if the merge keeps the same names.
                if curr_man.to_stay_on && !next_man.has_same_names(curr_man, true) {
                    curr_man.to_stay_on = false;
                }
                if next_man.has_street_names() {
                    curr_man.street_names = next_man.street_names.duplicate();
                }
                if !curr_man.signs.has_any() {
                    if next_man.signs.has_guide_branch() {
                        curr_man
                            .signs
                            .guide_branch_list
                            .clone_from(&next_man.signs.guide_branch_list);
                    }
                    if next_man.signs.has_guide_toward() {
                        curr_man
                            .signs
                            .guide_toward_list
                            .clone_from(&next_man.signs.guide_toward_list);
                    }
                }
                Self::combine_maneuvers(maneuvers, curr, next);
                maneuvers[curr].has_collapsed_merge_maneuver = true;
            }
            // Mainline advances after a collapse too, past the maneuver that followed the merge.
            curr += 1;
        }
    }

    /// The turn lane direction a maneuver uses on an edge (`GetExpectedTurnLaneDirection`).
    fn expected_turn_lane_direction(edge: &Edge, maneuver: &Maneuver) -> u16 {
        let has = |direction| edge.has_turn_lane(direction);
        let first_of = |directions: &[u16]| {
            directions
                .iter()
                .copied()
                .find(|d| has(*d))
                .unwrap_or(lane::NONE)
        };
        let Ok(maneuver_type) = Type::try_from(maneuver.maneuver_type) else {
            return lane::NONE;
        };
        match maneuver_type {
            Type::KUturnLeft => first_of(&[lane::REVERSE, lane::LEFT]),
            Type::KSharpLeft => first_of(&[lane::SHARP_LEFT, lane::LEFT]),
            Type::KLeft => {
                if has(lane::LEFT) {
                    lane::LEFT
                } else if has(lane::SLIGHT_LEFT) && maneuver.turn_degree > 270 {
                    lane::SLIGHT_LEFT
                } else if has(lane::SHARP_LEFT) && maneuver.turn_degree < 270 {
                    lane::SHARP_LEFT
                } else {
                    lane::NONE
                }
            }
            Type::KSlightLeft | Type::KExitLeft => first_of(&[lane::SLIGHT_LEFT, lane::LEFT]),
            Type::KRampLeft => {
                if maneuver.begin_relative_direction == RelativeDirection::Left && has(lane::LEFT) {
                    lane::LEFT
                } else {
                    first_of(&[lane::SLIGHT_LEFT, lane::LEFT])
                }
            }
            Type::KStayLeft => {
                if has(lane::SLIGHT_LEFT) {
                    lane::SLIGHT_LEFT
                } else if has(lane::LEFT) {
                    lane::LEFT
                } else if has(lane::THROUGH) && (has(lane::RIGHT) || has(lane::SLIGHT_RIGHT)) {
                    lane::THROUGH
                } else {
                    lane::NONE
                }
            }
            Type::KBecomes | Type::KContinue | Type::KRampStraight | Type::KStayStraight => {
                lane::THROUGH
            }
            Type::KStayRight => {
                if has(lane::SLIGHT_RIGHT) {
                    lane::SLIGHT_RIGHT
                } else if has(lane::RIGHT) {
                    lane::RIGHT
                } else if has(lane::THROUGH) && (has(lane::LEFT) || has(lane::SLIGHT_LEFT)) {
                    lane::THROUGH
                } else {
                    lane::NONE
                }
            }
            Type::KSlightRight | Type::KExitRight => first_of(&[lane::SLIGHT_RIGHT, lane::RIGHT]),
            Type::KRampRight => {
                if maneuver.begin_relative_direction == RelativeDirection::Right && has(lane::RIGHT)
                {
                    lane::RIGHT
                } else {
                    first_of(&[lane::SLIGHT_RIGHT, lane::RIGHT])
                }
            }
            Type::KRight => {
                if has(lane::RIGHT) {
                    lane::RIGHT
                } else if has(lane::SLIGHT_RIGHT) && maneuver.turn_degree < 90 {
                    lane::SLIGHT_RIGHT
                } else if has(lane::SHARP_RIGHT) && maneuver.turn_degree > 90 {
                    lane::SHARP_RIGHT
                } else {
                    lane::NONE
                }
            }
            Type::KSharpRight => first_of(&[lane::SHARP_RIGHT, lane::RIGHT]),
            Type::KUturnRight => first_of(&[lane::REVERSE, lane::RIGHT]),
            _ => lane::NONE,
        }
    }

    /// Marks the turn lanes each driving maneuver uses (`ProcessTurnLanes`).
    ///
    /// This updates the trip leg's edges.
    ///
    /// Mainline reads `next_man->type()` even when `next_man` is the list's end
    /// (for the destination maneuver), which is undefined behavior.
    /// In practice it reads the list sentinel; we use `kNone`,
    /// which activates the lanes the same way unless the garbage happens to name a turn.
    pub(super) fn process_turn_lanes(&mut self, maneuvers: &[Maneuver]) {
        let no_type = Type::KNone as i32;
        for curr in 1..maneuvers.len() {
            let (prev_man, curr_man) = (&maneuvers[curr - 1], &maneuvers[curr]);
            let next_type = maneuvers.get(curr + 1).map_or(no_type, |m| m.maneuver_type);
            if !curr_man.has_mode(TravelMode::KDrive) {
                continue;
            }

            // The edge entering the maneuver has the turn lanes at the transition point.
            let begin = curr_man.begin_node_index as usize;
            let entering = self.leg.prev_edge(begin, 1).map(|edge| {
                (
                    !edge.turn_lanes.is_empty(),
                    Self::expected_turn_lane_direction(edge, curr_man),
                    edge.length_km,
                )
            });
            let mut remaining_step_distance = 0.0_f32;
            if let Some((has_lanes, direction, length_km)) = entering {
                // Short forks do not get turn lanes.
                let short_fork = curr_man.length < SHORT_FORK_THRESHOLD
                    && curr_man.is_any_type(&[
                        Type::KStayLeft,
                        Type::KStayRight,
                        Type::KStayStraight,
                    ]);
                if has_lanes
                    && !short_fork
                    && let Some(edge) = edge_mut(self.leg, begin - 1)
                {
                    lanes::activate_turn_lanes(
                        edge,
                        direction,
                        curr_man.length,
                        curr_man.maneuver_type,
                        next_type,
                    );
                }
                // Account for the entering edge, which the walk below skips.
                remaining_step_distance += length_km;
            }

            // Walk backwards over the inner nodes of the *previous* maneuver,
            // which is what mainline's loop indexes (`prev_man`, not `curr_man`).
            let mut has_directional_intersecting_edge = false;
            for i in
                ((prev_man.begin_node_index as usize + 1)..(prev_man.end_node_index as usize)).rev()
            {
                let (Some(node), Some(prev_edge)) = (self.leg.node_at(i), self.leg.prev_edge(i, 1))
                else {
                    continue;
                };
                if !has_directional_intersecting_edge {
                    let counts =
                        node.intersecting_edge_counts(prev_edge.end_heading, prev_edge.travel_mode);
                    if (counts.right_traversable_outbound > 0 && curr_man.is_right_type())
                        || (counts.left_traversable_outbound > 0 && curr_man.is_left_type())
                    {
                        has_directional_intersecting_edge = true;
                    }
                }
                let length_km = prev_edge.length_km;
                let lanes = (!prev_edge.turn_lanes.is_empty())
                    .then(|| Self::expected_turn_lane_direction(prev_edge, curr_man));
                if let Some(direction) = lanes
                    && let Some(edge) = edge_mut(self.leg, i - 1)
                {
                    if remaining_step_distance < UPCOMING_LANES_THRESHOLD
                        && !has_directional_intersecting_edge
                        && direction != lane::NONE
                    {
                        // Activate the lanes for the upcoming turn.
                        lanes::activate_turn_lanes(
                            edge,
                            direction,
                            curr_man.length,
                            curr_man.maneuver_type,
                            next_type,
                        );
                    } else {
                        // Activate one or all of the through lanes.
                        lanes::activate_turn_lanes(
                            edge,
                            lane::THROUGH,
                            remaining_step_distance,
                            prev_man.maneuver_type,
                            curr_man.maneuver_type,
                        );
                    }
                }
                remaining_step_distance += length_km;
            }
        }
    }

    /// Gives each maneuver the landmarks along the previous maneuver's edges,
    /// with distances measured back from the maneuver point (`AddLandmarksFromTripLegToManeuvers`).
    pub(super) fn add_landmarks_from_trip_leg_to_maneuvers(&self, maneuvers: &mut [Maneuver]) {
        let mut landmarks = Vec::new();
        for maneuver in maneuvers {
            if !landmarks.is_empty() {
                maneuver.landmarks = core::mem::take(&mut landmarks);
            }
            landmarks.clear();

            let total_distance = f64::from(maneuver.length * METERS_PER_KM);
            let mut distance_to_curr_edge = 0.0_f64;
            for node_index in maneuver.begin_node_index..maneuver.end_node_index {
                // Mainline compares two fresh `unique_ptr`s here to skip repeated edges
                // (from through locations), which never compare equal, so every edge counts.
                let Some(edge) = self.leg.curr_edge(node_index as usize) else {
                    continue;
                };
                for landmark in &edge.landmarks {
                    let mut landmark = landmark.clone();
                    landmark.distance = total_distance - landmark.distance - distance_to_curr_edge;
                    landmarks.push(landmark);
                }
                distance_to_curr_edge += f64::from(edge.length_km * METERS_PER_KM);
            }
        }
    }

    /// Marks maneuvers with street names too long for succinct phrases
    /// (`ProcessVerbalSuccinctTransitionInstruction`).
    pub(super) fn process_verbal_succinct_transition_instruction(maneuvers: &mut [Maneuver]) {
        let too_long = |value: &str| {
            word_count(value) > MAX_WORD_COUNT || utf8_length(value) > MAX_STREET_NAME_LENGTH
        };
        for maneuver in maneuvers {
            if maneuver
                .street_names
                .iter()
                .take(VERBAL_PRE_ELEMENT_MAX_COUNT)
                .any(|name| too_long(name.value()))
            {
                maneuver.has_long_street_name = true;
            }
            if maneuver.is_type(Type::KRoundaboutEnter)
                && !maneuver.has_long_street_name
                && maneuver
                    .roundabout_exit_street_names
                    .iter()
                    .take(VERBAL_PRE_ELEMENT_MAX_COUNT)
                    .any(|name| too_long(name.value()))
            {
                maneuver.has_long_street_name = true;
            }
        }
    }
}

/// Raises the consecutive count of signs that name one of the maneuver's streets.
fn count_matching_street_names(signs: &mut [Sign], maneuver: &Maneuver) {
    for sign in signs {
        for street_name in &maneuver.street_names {
            if sign.text == street_name.value() {
                sign.consecutive_count += 1;
            }
        }
    }
}

/// Splits `prefix;suffix` (exactly two parts), as mainline's `split` and `is_pair` do.
///
/// `std::getline` drops a trailing empty token, so `a;` is one token, not a pair,
/// while `;b` is the pair `("", "b")`.
fn split_pair(text: &str) -> Option<(&str, &str)> {
    let mut tokens: Vec<&str> = text.split(';').collect();
    if tokens.last() == Some(&"") {
        tokens.pop();
    }
    match tokens.as_slice() {
        [prefix, suffix] => Some((prefix, suffix)),
        _ => None,
    }
}

/// The number of words in a name (`get_word_count`): runs of characters that are neither
/// whitespace nor ASCII punctuation, as `std::isspace` and `std::ispunct` see them in the "C" locale.
pub(crate) fn word_count(name: &str) -> usize {
    let is_separator =
        |byte: &u8| byte.is_ascii_whitespace() || *byte == 0x0b || byte.is_ascii_punctuation();
    name.as_bytes()
        .split(is_separator)
        .filter(|word| !word.is_empty())
        .count()
}

/// The number of code points in UTF-8 text, counting bytes that are not continuation bytes
/// (`strlen_utf8`).
pub(crate) fn utf8_length(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_pair_follows_getline() {
        assert_eq!(split_pair("a;b"), Some(("a", "b")));
        assert_eq!(split_pair(";b"), Some(("", "b")));
        assert_eq!(split_pair("a;"), None);
        assert_eq!(split_pair("a;b;"), Some(("a", "b")));
        assert_eq!(split_pair("a;b;c"), None);
        assert_eq!(split_pair("a"), None);
        assert_eq!(split_pair(""), None);
    }

    #[test]
    fn word_counts() {
        assert_eq!(word_count("Main Street"), 2);
        assert_eq!(word_count("  Avinguda   de   Meritxell  "), 3);
        assert_eq!(word_count("Carrer de l'Alzinaret"), 4);
        assert_eq!(word_count("Straße-Nord"), 2);
        assert_eq!(word_count(""), 0);
        assert_eq!(utf8_length("Straße"), 6);
    }
}
