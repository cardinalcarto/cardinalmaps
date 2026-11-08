//! How a Valhalla maneuver maps to an OSRM step's maneuver
//! (`turn_modifier`, `ramp_type`, `maneuver_type`, `get_mode`, `names_and_refs`,
//! `get_pronunciations`, and `osrm_maneuver` in mainline's `route_serializer_osrm.cc`).

use super::json::{Json, Map};
use super::turns::{
    MODIFIER_LEFT, MODIFIER_RIGHT, MODIFIER_SHARP_LEFT, MODIFIER_SHARP_RIGHT, MODIFIER_SLIGHT_LEFT,
    MODIFIER_SLIGHT_RIGHT, MODIFIER_STRAIGHT, MODIFIER_UTURN, TurnType, turn_degree, turn_type,
};
use crate::tyr::polyline::LngLat;
use crate::tyr::trip::{curr_edge, node, prev_edge};
use crate::tyr::unknown;
use valhalla_proto::directions_leg::Maneuver;
use valhalla_proto::directions_leg::maneuver::Type as ManeuverType;
use valhalla_proto::{StreetName, TravelMode, TripLeg, trip_leg};
use valhalla_response::ValhallaError;

fn is(maneuver: &Maneuver, kind: ManeuverType) -> bool {
    maneuver.r#type == kind as i32
}

/// `turn_modifier(maneuver, …)`: the OSRM modifier for a maneuver.
pub(super) fn turn_modifier(
    maneuver: &Maneuver,
    in_bearing: u32,
    out_bearing: u32,
    arrive_maneuver: bool,
) -> &'static str {
    let Ok(kind) = ManeuverType::try_from(maneuver.r#type) else {
        return MODIFIER_STRAIGHT;
    };
    match kind {
        ManeuverType::KStart | ManeuverType::KDestination => "",
        ManeuverType::KSlightRight
        | ManeuverType::KStayRight
        | ManeuverType::KExitRight
        | ManeuverType::KMergeRight => MODIFIER_SLIGHT_RIGHT,
        ManeuverType::KRight | ManeuverType::KStartRight | ManeuverType::KDestinationRight => {
            MODIFIER_RIGHT
        }
        ManeuverType::KSharpRight => MODIFIER_SHARP_RIGHT,
        // TODO (from mainline, #1789): a route ending in a U-turn shouldn't say uturn.
        ManeuverType::KUturnRight | ManeuverType::KUturnLeft => {
            if arrive_maneuver {
                ""
            } else {
                MODIFIER_UTURN
            }
        }
        ManeuverType::KSharpLeft => MODIFIER_SHARP_LEFT,
        ManeuverType::KLeft | ManeuverType::KStartLeft | ManeuverType::KDestinationLeft => {
            MODIFIER_LEFT
        }
        ManeuverType::KSlightLeft
        | ManeuverType::KStayLeft
        | ManeuverType::KExitLeft
        | ManeuverType::KMergeLeft => MODIFIER_SLIGHT_LEFT,
        ManeuverType::KRampRight => {
            if turn_type(in_bearing, out_bearing) == TurnType::Right {
                MODIFIER_RIGHT
            } else {
                MODIFIER_SLIGHT_RIGHT
            }
        }
        ManeuverType::KRampLeft => {
            if turn_type(in_bearing, out_bearing) == TurnType::Left {
                MODIFIER_LEFT
            } else {
                MODIFIER_SLIGHT_LEFT
            }
        }
        ManeuverType::KRoundaboutEnter
        | ManeuverType::KRoundaboutExit
        | ManeuverType::KFerryEnter
        | ManeuverType::KFerryExit => turn_type(in_bearing, out_bearing).modifier(),
        _ => MODIFIER_STRAIGHT,
    }
}

/// `ramp_type`: off ramps leave a motorway, on ramps (nearly straight) join one.
fn ramp_type(maneuver: &Maneuver) -> &'static str {
    if is(maneuver, ManeuverType::KExitRight) || is(maneuver, ManeuverType::KExitLeft) {
        "off ramp"
    } else if is(maneuver, ManeuverType::KRampStraight)
        || is(maneuver, ManeuverType::KRampRight)
        || is(maneuver, ManeuverType::KRampLeft)
    {
        if maneuver.turn_degree > 329 || maneuver.turn_degree < 31 {
            "on ramp"
        } else {
            "turn"
        }
    } else {
        ""
    }
}

/// The state `maneuver_type` needs from the previous step.
#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mainline's maneuver_type arguments"
)]
pub(super) struct ManeuverContext<'a> {
    pub(super) depart_maneuver: bool,
    pub(super) arrive_maneuver: bool,
    pub(super) modifier: &'a str,
    pub(super) prev_intersection_count: u32,
    pub(super) mode: &'a str,
    pub(super) prev_mode: &'a str,
    pub(super) rotary: bool,
    pub(super) prev_rotary: bool,
}

/// `maneuver_type`: the OSRM type of a step's maneuver (and of its banner).
pub(super) fn maneuver_type(
    maneuver: &Maneuver,
    leg: &TripLeg,
    context: &ManeuverContext<'_>,
) -> Result<&'static str, ValhallaError> {
    if context.depart_maneuver {
        return Ok("depart");
    }
    if context.arrive_maneuver {
        return Ok("arrive");
    }
    if context.mode != context.prev_mode {
        return Ok("notification");
    }
    if is(maneuver, ManeuverType::KRoundaboutEnter) {
        return Ok(if context.rotary {
            "rotary"
        } else {
            "roundabout"
        });
    }
    if is(maneuver, ManeuverType::KRoundaboutExit) {
        return Ok(if context.prev_rotary {
            "exit rotary"
        } else {
            "exit roundabout"
        });
    }

    let index = maneuver.begin_path_index as usize;
    let new_name_type =
        is(maneuver, ManeuverType::KContinue) || is(maneuver, ManeuverType::KBecomes);
    let ramp = [
        ManeuverType::KRampStraight,
        ManeuverType::KRampRight,
        ManeuverType::KRampLeft,
        ManeuverType::KExitRight,
        ManeuverType::KExitLeft,
    ]
    .into_iter()
    .any(|kind| is(maneuver, kind));
    let fork = [
        ManeuverType::KStayStraight,
        ManeuverType::KStayRight,
        ManeuverType::KStayLeft,
    ]
    .into_iter()
    .any(|kind| is(maneuver, kind));
    let mut kind = if is(maneuver, ManeuverType::KMerge)
        || is(maneuver, ManeuverType::KMergeLeft)
        || is(maneuver, ManeuverType::KMergeRight)
    {
        "merge"
    } else if fork {
        "fork"
    } else if ramp {
        ramp_type(maneuver)
    } else if new_name_type {
        "new name"
    } else {
        ""
    };
    if !kind.is_empty() {
        return Ok(kind);
    }

    let node = node(leg, index).ok_or_else(|| unknown("maneuver path index past the leg"))?;
    let prev = prev_edge(leg, index).ok_or_else(|| unknown("turn maneuver at the leg's start"))?;
    let curr = curr_edge(leg, index).ok_or_else(|| unknown("turn maneuver at the leg's end"))?;
    let false_node = node.intersecting_edge.is_empty();

    // The road ends at a T: a new name after passing more than one intersection,
    // with a single intersecting edge in the opposite turn direction.
    let mut road_ends = context.prev_intersection_count > 1
        && prev.r#use != trip_leg::Use::KRampUse as i32
        && node.intersecting_edge.len() == 1;
    if road_ends && let Some(intersecting) = node.intersecting_edge.first() {
        if intersecting.prev_name_consistency {
            road_ends = false;
        } else {
            let turn1 = TurnType::from_degree(turn_degree(prev.end_heading, curr.begin_heading));
            let turn2 =
                TurnType::from_degree(turn_degree(prev.end_heading, intersecting.begin_heading));
            let opposing = (turn1 == TurnType::Right && turn2 == TurnType::Left)
                || (turn2 == TurnType::Right && turn1 == TurnType::Left);
            if !opposing {
                road_ends = false;
            }
        }
    }

    let new_name = !prev.name.is_empty()
        && prev.name.len() == curr.name.len()
        && prev.name[0].value != curr.name[0].value;

    kind = if context.prev_intersection_count > 1 && road_ends {
        "end of road"
    } else if false_node && new_name {
        "new name"
    } else if context.modifier != MODIFIER_UTURN && !maneuver.to_stay_on {
        "turn"
    } else {
        "continue"
    };
    Ok(kind)
}

/// `get_mode`: `ferry` on ferries, else the travel mode.
pub(super) fn mode(
    maneuver: &Maneuver,
    arrive_maneuver: bool,
    leg: &TripLeg,
) -> Result<&'static str, ValhallaError> {
    if !arrive_maneuver {
        let edge = curr_edge(leg, maneuver.begin_path_index as usize)
            .ok_or_else(|| unknown("maneuver without an outgoing edge"))?;
        if edge.r#use == trip_leg::Use::KFerryUse as i32 {
            return Ok("ferry");
        }
    }
    Ok(match TravelMode::try_from(maneuver.travel_mode) {
        Ok(TravelMode::KDrive) => "driving",
        Ok(TravelMode::KPedestrian) => "walking",
        Ok(TravelMode::KBicycle) => "cycling",
        Ok(TravelMode::KTransit) => "transit",
        Err(_) => "",
    })
}

/// `get_maneuver_street_names`: roundabout entries use the exit's names,
/// others their begin street names if they have any.
fn street_names(maneuver: &Maneuver) -> &[StreetName] {
    if is(maneuver, ManeuverType::KRoundaboutEnter) {
        &maneuver.roundabout_exit_street_names
    } else if !maneuver.begin_street_name.is_empty() {
        &maneuver.begin_street_name
    } else {
        &maneuver.street_name
    }
}

fn append(out: &mut String, value: &str) {
    if !out.is_empty() {
        out.push_str("; ");
    }
    out.push_str(value);
}

/// `names_and_refs`: the street names and the route numbers, each `; `-separated.
pub(super) fn names_and_refs(maneuver: &Maneuver) -> (String, String) {
    let (mut names, mut refs) = (String::new(), String::new());
    for name in street_names(maneuver) {
        if name.is_route_number {
            append(&mut refs, &name.value);
        } else {
            append(&mut names, &name.value);
        }
    }
    (names, refs)
}

/// `get_pronunciations`: the non-route-number names' pronunciations.
pub(super) fn pronunciations(maneuver: &Maneuver) -> String {
    let mut out = String::new();
    for name in street_names(maneuver) {
        if let Some(pronunciation) = &name.pronunciation
            && !name.is_route_number
        {
            append(&mut out, &pronunciation.value);
        }
    }
    out
}

/// `osrm_maneuver`: the step's `maneuver` object.
pub(super) fn osrm_maneuver(
    maneuver: &Maneuver,
    maneuver_type: &str,
    modifier: &str,
    in_bearing: u32,
    out_bearing: u32,
    location: LngLat,
    with_instruction: bool,
) -> Map {
    let mut out = Map::new();
    out.emplace(
        "location",
        vec![Json::Fixed(location.lng, 6), Json::Fixed(location.lat, 6)],
    );
    out.emplace("bearing_before", in_bearing);
    out.emplace("bearing_after", out_bearing);
    out.emplace("type", maneuver_type);
    if with_instruction {
        out.emplace("instruction", maneuver.text_instruction.as_str());
    }
    if !modifier.is_empty() {
        out.emplace("modifier", modifier);
    }
    if is(maneuver, ManeuverType::KRoundaboutEnter) && maneuver.roundabout_exit_count > 0 {
        out.emplace("exit", maneuver.roundabout_exit_count);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use valhalla_proto::Pronunciation;

    fn maneuver(kind: ManeuverType) -> Maneuver {
        Maneuver {
            r#type: kind as i32,
            ..Maneuver::default()
        }
    }

    fn name(value: &str, is_route_number: bool) -> StreetName {
        StreetName {
            value: value.to_string(),
            is_route_number,
            ..StreetName::default()
        }
    }

    #[test]
    fn modifiers() {
        assert_eq!(
            turn_modifier(&maneuver(ManeuverType::KStart), 0, 0, false),
            ""
        );
        assert_eq!(
            turn_modifier(&maneuver(ManeuverType::KUturnLeft), 0, 0, false),
            "uturn"
        );
        assert_eq!(
            turn_modifier(&maneuver(ManeuverType::KUturnLeft), 0, 0, true),
            ""
        );
        assert_eq!(
            turn_modifier(&maneuver(ManeuverType::KRampRight), 0, 90, false),
            "right"
        );
        assert_eq!(
            turn_modifier(&maneuver(ManeuverType::KRampRight), 0, 20, false),
            "slight right"
        );
        assert_eq!(
            turn_modifier(&maneuver(ManeuverType::KRoundaboutExit), 0, 270, false),
            "left"
        );
        assert_eq!(
            turn_modifier(&maneuver(ManeuverType::KContinue), 0, 270, false),
            "straight"
        );
    }

    #[test]
    fn ramps() {
        let mut ramp = maneuver(ManeuverType::KRampLeft);
        ramp.turn_degree = 340;
        assert_eq!(ramp_type(&ramp), "on ramp");
        ramp.turn_degree = 300;
        assert_eq!(ramp_type(&ramp), "turn");
        assert_eq!(ramp_type(&maneuver(ManeuverType::KExitRight)), "off ramp");
        assert_eq!(ramp_type(&maneuver(ManeuverType::KLeft)), "");
    }

    #[test]
    fn names() {
        let mut m = maneuver(ManeuverType::KRight);
        m.street_name = vec![
            name("Main Street", false),
            name("US 1", true),
            name("High Street", false),
        ];
        assert_eq!(
            names_and_refs(&m),
            ("Main Street; High Street".to_string(), "US 1".to_string())
        );
        m.begin_street_name = vec![name("Begin Street", false)];
        assert_eq!(names_and_refs(&m).0, "Begin Street");

        let mut roundabout = maneuver(ManeuverType::KRoundaboutEnter);
        roundabout.street_name = vec![name("Circle", false)];
        roundabout.roundabout_exit_street_names = vec![name("Exit Road", false)];
        assert_eq!(names_and_refs(&roundabout).0, "Exit Road");

        let mut spoken = maneuver(ManeuverType::KRight);
        spoken.street_name = vec![StreetName {
            pronunciation: Some(Pronunciation {
                value: "meyn".to_string(),
                ..Pronunciation::default()
            }),
            ..name("Main", false)
        }];
        assert_eq!(pronunciations(&spoken), "meyn");
    }
}
