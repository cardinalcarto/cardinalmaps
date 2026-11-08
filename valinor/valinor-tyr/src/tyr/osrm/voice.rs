//! `voiceInstructions` (`voice_instructions`, `distance_along_geometry`,
//! and `addVoiceInstruction` in mainline's `route_serializer_osrm.cc`),
//! the Mapbox Directions extension Ferrostar speaks.
//!
//! Like banners, a step's announcements lead up to the maneuver that ends it,
//! so they are built while handling that next maneuver.
//! Mainline mixes `float` and `double` here; the types below follow it exactly,
//! since announcement distances are written with one decimal.

use super::json::{Json, Map};
use crate::tyr::trip::{curr_edge, node};
use crate::tyr::unknown;
use valhalla_proto::directions_leg::Maneuver;
use valhalla_proto::options::Units;
use valhalla_proto::{Options, TripLeg};
use valhalla_response::ValhallaError;

const SECONDS_BEFORE_VERBAL_TRANSITION_ALERT_INSTRUCTION: u32 = 35;
const SECONDS_BEFORE_VERBAL_PRE_TRANSITION_INSTRUCTION: u32 = 10;
const MIN_DISTANCE_VERBAL_PRE_TRANSITION_INSTRUCTION: f64 = 40.0;
const APPROXIMATE_VERBAL_POSTRANSITION_LENGTH: f64 = 110.0;
const APPROXIMATE_VERBAL_PRERANSITION_SECONDS: f64 = 3.0;

/// `midgard::kMilePerKm`.
const MILE_PER_KM: f32 = 0.621_371;

/// The context the announcements need beyond the two maneuvers.
#[derive(Debug, Clone, Copy)]
pub(super) struct VoiceContext<'a> {
    /// The request's options (for its language and units).
    pub(super) options: &'a Options,
}

/// The alert distance in the request's units, from kilometers.
///
/// # Divergence from mainline
///
/// Mainline passes kilometers to `FormVerbalAlertApproachInstruction` even for requests in miles,
/// which then announces the kilometer value as miles ("In 1 mile" for 1.6 km).
/// We convert as Odin's maneuver lengths do (`kMilePerKm`, in `float`),
/// so the announcement matches the distance it is played at.
fn alert_distance(options: &Options, kilometers: f32) -> f32 {
    if options.units == Units::Miles as i32 {
        kilometers * MILE_PER_KM
    } else {
        kilometers
    }
}

fn elapsed_seconds(leg: &TripLeg, index: u32) -> Result<f64, ValhallaError> {
    let node =
        node(leg, index as usize).ok_or_else(|| unknown("maneuver path index past the leg"))?;
    Ok(node
        .cost
        .unwrap_or_default()
        .elapsed_cost
        .unwrap_or_default()
        .seconds)
}

/// Narrows to `float`, as mainline's `float` locals and return values do.
#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline stores these in floats"
)]
fn to_f32(value: f64) -> f32 {
    value as f32
}

/// `distance_along_geometry`: how far before the end of the step (in meters)
/// to start an announcement so that it plays `target_seconds` before the maneuver,
/// given the speeds along the step; the whole step if it is shorter than that.
fn distance_along_geometry(
    prev_maneuver: &Maneuver,
    leg: &TripLeg,
    distance: f64,
    target_seconds: u32,
) -> Result<f32, ValhallaError> {
    let target = f64::from(target_seconds);
    let mut node_index = prev_maneuver.end_path_index;
    let end_node_elapsed_seconds = elapsed_seconds(leg, node_index)?;
    let begin_node_elapsed_seconds = elapsed_seconds(leg, prev_maneuver.begin_path_index)?;
    if end_node_elapsed_seconds - begin_node_elapsed_seconds < target {
        return Ok(to_f32(distance));
    }

    let mut accumulated_distance_km = 0.0_f32;
    let mut previous_accumulated_distance_km = 0.0_f32;
    let mut accumulated_seconds = 0.0_f64;
    let mut previous_accumulated_seconds = 0.0_f64;
    while accumulated_seconds < target && node_index >= prev_maneuver.begin_path_index {
        // The step lasts at least `target`, so this stops by the step's first node.
        node_index = node_index
            .checked_sub(1)
            .ok_or_else(|| unknown("announcement distance walked past the leg's start"))?;
        previous_accumulated_seconds = accumulated_seconds;
        accumulated_seconds = end_node_elapsed_seconds - elapsed_seconds(leg, node_index)?;
        previous_accumulated_distance_km = accumulated_distance_km;
        accumulated_distance_km += curr_edge(leg, node_index as usize)
            .ok_or_else(|| unknown("announcement distance without an edge"))?
            .length_km;
    }
    // Take off the part of the last edge that is beyond the target time.
    let surplus_percentage =
        (accumulated_seconds - target) / (accumulated_seconds - previous_accumulated_seconds);
    accumulated_distance_km = to_f32(
        f64::from(accumulated_distance_km)
            - f64::from(accumulated_distance_km - previous_accumulated_distance_km)
                * surplus_percentage,
    );
    let meters = accumulated_distance_km * 1000.0;
    if f64::from(meters) > distance {
        Ok(to_f32(distance))
    } else {
        Ok(meters)
    }
}

/// `addVoiceInstruction`.
fn add(voice_instructions: &mut Vec<Json>, instruction: &str, distance_along_geometry: f64) {
    let mut voice = Map::new();
    voice.emplace(
        "distanceAlongGeometry",
        Json::Fixed(distance_along_geometry, 1),
    );
    voice.emplace("announcement", instruction);
    voice.emplace("ssmlAnnouncement", format!("<speak>{instruction}</speak>"));
    voice_instructions.push(voice.into());
}

/// `voice_instructions`: the announcements along the step of `prev_maneuver`
/// (`distance` meters long) leading up to `maneuver` (the `maneuver_index`th of the leg).
pub(super) fn voice_instructions(
    prev_maneuver: &Maneuver,
    maneuver: &Maneuver,
    distance: f64,
    maneuver_index: usize,
    leg: &TripLeg,
    context: VoiceContext<'_>,
) -> Result<Vec<Json>, ValhallaError> {
    // NarrativeBuilderFactory::Create throws for unknown languages, before anything else.
    let language = match &context.options.has_language {
        Some(valhalla_proto::options::HasLanguage::Language(language)) => language.as_str(),
        None => "",
    };
    if !illuvatar::is_supported_language(language) {
        return Err(unknown("Invalid language tag."));
    }
    let mut out = Vec::new();
    let mut before_alert = distance_along_geometry(
        prev_maneuver,
        leg,
        distance,
        SECONDS_BEFORE_VERBAL_TRANSITION_ALERT_INSTRUCTION,
    )?;
    let mut before_pre = distance_along_geometry(
        prev_maneuver,
        leg,
        distance,
        SECONDS_BEFORE_VERBAL_PRE_TRANSITION_INSTRUCTION,
    )?;

    // Even when driving slowly, give the pre-transition instruction a few meters ahead.
    if f64::from(before_pre) < MIN_DISTANCE_VERBAL_PRE_TRANSITION_INSTRUCTION
        && distance > MIN_DISTANCE_VERBAL_PRE_TRANSITION_INSTRUCTION
    {
        before_pre = to_f32(MIN_DISTANCE_VERBAL_PRE_TRANSITION_INSTRUCTION);
    }
    // And skip the alert if it would come too close to the maneuver (-1 means "skip").
    if f64::from(before_alert) < MIN_DISTANCE_VERBAL_PRE_TRANSITION_INSTRUCTION {
        before_alert = -1.0;
    }

    if maneuver_index == 1 && !prev_maneuver.verbal_pre_transition_instruction.is_empty() {
        // The depart instruction always plays at the start.
        add(
            &mut out,
            &prev_maneuver.verbal_pre_transition_instruction,
            distance,
        );
    } else if before_alert >= 0.0
        && distance > f64::from(before_alert) + APPROXIMATE_VERBAL_POSTRANSITION_LENGTH
        && !prev_maneuver.verbal_post_transition_instruction.is_empty()
    {
        // Otherwise the post-transition instruction plays 10 meters in, if there is time.
        add(
            &mut out,
            &prev_maneuver.verbal_post_transition_instruction,
            distance - 10.0,
        );
    }

    if !maneuver.verbal_transition_alert_instruction.is_empty()
        && before_alert >= 0.0
        && prev_maneuver.time
            > f64::from(SECONDS_BEFORE_VERBAL_TRANSITION_ALERT_INSTRUCTION)
                + APPROXIMATE_VERBAL_PRERANSITION_SECONDS
    {
        #[expect(clippy::float_cmp, reason = "mainline compares exactly")]
        let whole_step = f64::from(before_alert) == distance;
        if maneuver_index == 1 && whole_step {
            // On a short depart step, play the alert halfway along it.
            before_alert = to_f32(distance / 2.0);
        }
        let distance_km = before_alert / 1000.0;
        let instruction = illuvatar::verbal_alert_approach_instruction(
            context.options,
            alert_distance(context.options, distance_km),
            &maneuver.verbal_transition_alert_instruction,
        )
        .map_err(|e| unknown(e.to_string()))?;
        add(&mut out, &instruction, f64::from(before_alert));
    }

    if !maneuver.verbal_pre_transition_instruction.is_empty() {
        if maneuver_index == 1 && f64::from(before_pre) >= distance / 2.0 {
            before_pre = to_f32(distance / 4.0);
        }
        add(
            &mut out,
            &maneuver.verbal_pre_transition_instruction,
            f64::from(before_pre),
        );
    }
    Ok(out)
}
