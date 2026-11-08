//! `bannerInstructions` (`banner_instructions` and its helpers in mainline's
//! `route_serializer_osrm.cc`), the Mapbox Directions extension Ferrostar shows on screen.
//!
//! A step's banners describe the *next* maneuver,
//! so the serializer builds them while handling that next maneuver
//! and attaches them to the previous step.

use super::intersections::{drive_on_right, has_usable_turn_lanes};
use super::json::{Json, Map};
use super::turns::{lane_indications, turn_lane_direction};
use crate::tyr::trip::{curr_edge, prev_edge};
use crate::tyr::unknown;
use valhalla_proto::TripLeg;
use valhalla_proto::directions_leg::Maneuver;
use valhalla_proto::directions_leg::maneuver::Type as ManeuverType;
use valhalla_proto::turn_lane::State as LaneState;
use valhalla_response::ValhallaError;

/// What the banners of a step say about the maneuver that ends it.
#[derive(Debug)]
pub(super) struct BannerInput<'a> {
    /// The upcoming maneuver's street names (`; `-separated non-route-numbers).
    pub(super) name: &'a str,
    /// Its destinations from signs.
    pub(super) destinations: &'a str,
    /// Its route numbers.
    pub(super) refs: &'a str,
    /// The step's maneuver (the banner is shown along it).
    pub(super) prev_maneuver: &'a Maneuver,
    /// The upcoming maneuver.
    pub(super) maneuver: &'a Maneuver,
    /// Whether the upcoming maneuver is the arrival.
    pub(super) arrive_maneuver: bool,
    /// The upcoming maneuver's OSRM type.
    pub(super) maneuver_type: &'a str,
    /// Its OSRM modifier.
    pub(super) modifier: &'a str,
    /// Its exit numbers.
    pub(super) exit: &'a str,
    /// The step's length in meters.
    pub(super) distance: f64,
    /// `right` or `left`.
    pub(super) drive_side: &'a str,
}

fn component(kind: &str, text: &str) -> Map {
    let mut component = Map::new();
    component.emplace("type", kind);
    component.emplace("text", text);
    component
}

/// `primary_banner_instruction`.
#[expect(clippy::too_many_arguments, reason = "mirrors mainline's helper")]
fn primary(
    text: &str,
    refs: &str,
    exit: &str,
    arrive_maneuver: bool,
    maneuver_type: &str,
    modifier: &str,
    roundabout: bool,
    roundabout_turn_degrees: u32,
    drive_side: &str,
) -> Map {
    let mut instruction = Map::new();
    let mut components = Vec::new();
    if !exit.is_empty() && !arrive_maneuver {
        components.push(component("exit", "Exit").into());
        components.push(component("exit-number", exit).into());
    }
    components.push(component("text", text).into());
    if !refs.is_empty() && !arrive_maneuver {
        components.push(component("delimiter", "/").into());
        components.push(component("text", refs).into());
    }
    instruction.emplace("components", components);
    instruction.emplace("text", text);
    if !maneuver_type.is_empty() {
        instruction.emplace("type", maneuver_type);
    }
    if !modifier.is_empty() {
        instruction.emplace("modifier", modifier);
    }
    if roundabout {
        instruction.emplace("degrees", roundabout_turn_degrees);
        instruction.emplace("driving_side", drive_side);
    }
    instruction
}

/// `secondary_banner_instruction`.
fn secondary(text: &str) -> Map {
    let mut instruction = Map::new();
    instruction.emplace("components", vec![Json::from(component("text", text))]);
    instruction.emplace("text", text);
    instruction
}

/// `sub_banner_instruction`: the lanes at the end of the step, if any are worth showing.
fn sub(prev_maneuver: &Maneuver, leg: &TripLeg) -> Option<Map> {
    let edge = prev_edge(leg, prev_maneuver.end_path_index as usize)?;
    if !has_usable_turn_lanes(edge) {
        return None;
    }
    let lanes: Vec<Json> = edge
        .turn_lanes
        .iter()
        .map(|turn_lane| {
            let mut lane = component("lane", "");
            lane.emplace("active", turn_lane.state == LaneState::KActive as i32);
            if turn_lane.state != LaneState::KInvalid as i32 {
                lane.emplace(
                    "active_direction",
                    turn_lane_direction(turn_lane.active_direction),
                );
            }
            let directions: Vec<Json> =
                lane_indications(drive_on_right(edge), turn_lane.directions_mask)
                    .into_iter()
                    .map(Json::from)
                    .collect();
            lane.emplace("directions", directions);
            lane.into()
        })
        .collect();
    let mut instruction = Map::new();
    instruction.emplace("components", lanes);
    instruction.emplace("text", "");
    Some(instruction)
}

/// The bearing into the maneuver starting at `begin_index`:
/// the end of the previous edge, or the start of the first edge at the leg's start.
fn bearing_before(leg: &TripLeg, begin_index: u32) -> Result<u32, ValhallaError> {
    let index = begin_index as usize;
    let edge = if begin_index > 0 {
        prev_edge(leg, index).map(|edge| edge.end_heading)
    } else {
        curr_edge(leg, index).map(|edge| edge.begin_heading)
    };
    edge.ok_or_else(|| unknown("roundabout maneuver without an edge"))
}

/// `calc_roundabout_turn_degrees`: how far around the roundabout the maneuver goes
/// (180 straight through), from the headings before and after it.
fn roundabout_turn_degrees(
    prev_maneuver: &Maneuver,
    maneuver: &Maneuver,
    leg: &TripLeg,
) -> Result<u32, ValhallaError> {
    let (mut before, mut after) = (0_u32, 0_u32);
    if maneuver.r#type == ManeuverType::KRoundaboutEnter as i32 {
        before = bearing_before(leg, maneuver.begin_path_index)?;
        if let Some(edge) = curr_edge(leg, maneuver.end_path_index as usize) {
            after = edge.begin_heading;
        }
    }
    if maneuver.r#type == ManeuverType::KRoundaboutExit as i32 {
        before = bearing_before(leg, prev_maneuver.begin_path_index)?;
        after = maneuver.begin_heading;
    }
    // Unsigned arithmetic, as in mainline.
    let mut degrees = before
        .wrapping_sub(after)
        .wrapping_sub(180)
        .wrapping_add(720)
        % 360;
    let edge = curr_edge(leg, maneuver.begin_path_index as usize)
        .ok_or_else(|| unknown("roundabout maneuver without an outgoing edge"))?;
    if !drive_on_right(edge) {
        degrees = 360 - degrees;
    }
    Ok(degrees)
}

/// `banner_instructions`: the step's banners, plus a second one with lanes
/// 400 meters before the maneuver when the step is longer than that.
pub(super) fn banner_instructions(
    input: &BannerInput<'_>,
    leg: &TripLeg,
) -> Result<Vec<Json>, ValhallaError> {
    let mut primary_text = input.name.to_string();
    let mut secondary_text = input.destinations.to_string();
    let mut refs = input.refs.to_string();
    if primary_text.is_empty() && !secondary_text.is_empty() {
        primary_text = core::mem::take(&mut secondary_text);
    }
    if primary_text.is_empty() && !refs.is_empty() {
        primary_text = core::mem::take(&mut refs);
    }
    if input.arrive_maneuver || primary_text.is_empty() {
        primary_text.clone_from(&input.maneuver.text_instruction);
    }

    let roundabout = input.maneuver.r#type == ManeuverType::KRoundaboutEnter as i32
        || input.maneuver.r#type == ManeuverType::KRoundaboutExit as i32;
    let degrees = if roundabout {
        roundabout_turn_degrees(input.prev_maneuver, input.maneuver, leg)?
    } else {
        0
    };
    let make_primary = || {
        primary(
            &primary_text,
            &refs,
            input.exit,
            input.arrive_maneuver,
            input.maneuver_type,
            input.modifier,
            roundabout,
            degrees,
            input.drive_side,
        )
    };

    let mut main = Map::new();
    main.emplace("distanceAlongGeometry", Json::Fixed(input.distance, 3));
    main.emplace("primary", make_primary());
    if !secondary_text.is_empty() {
        main.emplace("secondary", secondary(&secondary_text));
    }

    let mut with_sub = None;
    if let Some(sub) = sub(input.prev_maneuver, leg) {
        if input.distance > 400.0 {
            let mut banner = Map::new();
            banner.emplace("sub", sub);
            if !secondary_text.is_empty() {
                banner.emplace("secondary", secondary(&secondary_text));
            }
            banner.emplace("primary", make_primary());
            banner.emplace("distanceAlongGeometry", Json::Fixed(400.0, 3));
            with_sub = Some(banner);
        } else {
            main.emplace("sub", sub);
        }
    }

    let mut banners = vec![Json::from(main)];
    banners.extend(with_sub.map(Json::from));
    Ok(banners)
}
