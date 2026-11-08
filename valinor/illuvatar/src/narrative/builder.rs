//! Deciding which instructions each maneuver gets (`NarrativeBuilder::Build`),
//! and the helpers every instruction shares.

use super::dictionary::{NarrativeDictionary, Subset, dictionary, label};
use crate::error::OdinError;
use crate::maneuver::Maneuver;
use crate::markup_formatter::MarkupFormatter;
use crate::signs::SignFormat;
use crate::street_names::StreetNames;
use crate::verbal_text_formatter::VerbalTextFormatter;
use valhalla_proto::directions_leg::maneuver::{BssManeuverType, Type};
use valhalla_proto::trip_leg::node;
use valhalla_proto::{Options, TravelMode, TripLeg, options};

/// Mainline's `midgard::kMetersPerKm`.
const METERS_PER_KM: f32 = 1000.0;
/// Mainline's `midgard::kFeetPerMile`.
const FEET_PER_MILE: f32 = 5280.0;
/// Ramps longer than this (km) get a verbal post-transition instruction.
pub(super) const VERBAL_POST_MINIMUM_RAMP_LENGTH: f32 = 2.0;
/// A merge gets a verbal alert only after a maneuver longer than this (km).
const VERBAL_ALERT_MERGE_PRIOR_MANEUVER_MINIMUM_LENGTH: f32 = VERBAL_POST_MINIMUM_RAMP_LENGTH;
/// Maneuvers quicker than this (seconds of basic time) are announced together with the next one.
const VERBAL_MULTI_CUE_TIME_THRESHOLD: f64 = 13.0;
/// The same threshold for start maneuvers.
const VERBAL_MULTI_CUE_TIME_START_MANEUVER_THRESHOLD: f64 = VERBAL_MULTI_CUE_TIME_THRESHOLD * 3.0;

/// The tags that phrases contain, replaced with values.
pub(super) mod tag {
    pub(crate) const CARDINAL_DIRECTION: &str = "<CARDINAL_DIRECTION>";
    pub(crate) const RELATIVE_DIRECTION: &str = "<RELATIVE_DIRECTION>";
    pub(crate) const ORDINAL_VALUE: &str = "<ORDINAL_VALUE>";
    pub(crate) const STREET_NAMES: &str = "<STREET_NAMES>";
    pub(crate) const PREVIOUS_STREET_NAMES: &str = "<PREVIOUS_STREET_NAMES>";
    pub(crate) const BEGIN_STREET_NAMES: &str = "<BEGIN_STREET_NAMES>";
    pub(crate) const CROSS_STREET_NAMES: &str = "<CROSS_STREET_NAMES>";
    pub(crate) const ROUNDABOUT_EXIT_STREET_NAMES: &str = "<ROUNDABOUT_EXIT_STREET_NAMES>";
    pub(crate) const ROUNDABOUT_EXIT_BEGIN_STREET_NAMES: &str =
        "<ROUNDABOUT_EXIT_BEGIN_STREET_NAMES>";
    pub(crate) const OBJECT_LABEL: &str = "<OBJECT_LABEL>";
    pub(crate) const LENGTH: &str = "<LENGTH>";
    pub(crate) const DESTINATION: &str = "<DESTINATION>";
    pub(crate) const CURRENT_VERBAL_CUE: &str = "<CURRENT_VERBAL_CUE>";
    pub(crate) const NEXT_VERBAL_CUE: &str = "<NEXT_VERBAL_CUE>";
    pub(crate) const KILOMETERS: &str = "<KILOMETERS>";
    pub(crate) const METERS: &str = "<METERS>";
    pub(crate) const MILES: &str = "<MILES>";
    pub(crate) const TENTHS_OF_MILE: &str = "<TENTHS_OF_MILE>";
    pub(crate) const FEET: &str = "<FEET>";
    pub(crate) const NUMBER_SIGN: &str = "<NUMBER_SIGN>";
    pub(crate) const BRANCH_SIGN: &str = "<BRANCH_SIGN>";
    pub(crate) const TOWARD_SIGN: &str = "<TOWARD_SIGN>";
    pub(crate) const NAME_SIGN: &str = "<NAME_SIGN>";
    pub(crate) const JUNCTION_NAME: &str = "<JUNCTION_NAME>";
    pub(crate) const FERRY_LABEL: &str = "<FERRY_LABEL>";
    pub(crate) const LEVEL: &str = "<LEVEL>";
}

/// Indexes into `empty_street_name_labels`.
pub(super) mod empty_label {
    pub(crate) const WALKWAY: usize = 0;
    pub(crate) const CYCLEWAY: usize = 1;
    pub(crate) const MOUNTAIN_BIKE_TRAIL: usize = 2;
    pub(crate) const PEDESTRIAN_CROSSING: usize = 3;
    pub(crate) const STEPS: usize = 4;
    pub(crate) const BRIDGE: usize = 5;
    pub(crate) const TUNNEL: usize = 6;
}

/// The default element counts and delimiter for each kind of instruction (`odin/util.h`).
pub(super) const ELEMENT_MAX_COUNT: u32 = 4;
pub(super) const VERBAL_ALERT_ELEMENT_MAX_COUNT: u32 = 1;
pub(super) const VERBAL_PRE_ELEMENT_MAX_COUNT: u32 = 2;
pub(super) const VERBAL_POST_ELEMENT_MAX_COUNT: u32 = 2;
pub(super) const VERBAL_DELIM: &str = ", ";
pub(super) const LIMIT_BY_CONSECUTIVE_COUNT: bool = true;

/// How to render a list of street names or signs.
#[derive(Clone, Copy)]
pub(super) struct ListFormat<'b> {
    /// The maximum number of elements, or zero for all of them.
    pub(super) max_count: u32,
    pub(super) delim: &'b str,
    /// The verbal formatter, for verbal instructions.
    pub(super) verbal: Option<VerbalTextFormatter>,
}

impl ListFormat<'static> {
    /// Mainline's defaults: every element, separated by `/`, unformatted.
    pub(super) const TEXT: Self = Self {
        max_count: 0,
        delim: "/",
        verbal: None,
    };

    /// Textual names limited to some number of elements.
    pub(super) const fn text(max_count: u32) -> Self {
        Self {
            max_count,
            delim: "/",
            verbal: None,
        }
    }
}

impl<'b> ListFormat<'b> {
    /// Verbal names, formatted with a maneuver's verbal formatter.
    pub(super) fn verbal(maneuver: &Maneuver, max_count: u32, delim: &'b str) -> Self {
        Self {
            max_count,
            delim,
            verbal: maneuver.verbal_formatter,
        }
    }
}

/// Language-specific behavior (mainline's `NarrativeBuilder` subclasses).
///
/// The Czech, Hindi, and Russian subclasses only change plural categories,
/// which only transit instructions use, so they behave like the default here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LanguageRules {
    Default,
    /// Italian combines prepositions and articles ("su il" becomes "sul").
    Italian,
}

/// Forms the instructions of a leg's maneuvers.
pub(crate) struct NarrativeBuilder<'a> {
    pub(super) leg: &'a TripLeg,
    pub(super) dictionary: &'static NarrativeDictionary,
    pub(super) markup: &'a MarkupFormatter,
    miles: bool,
    rules: LanguageRules,
}

/// The language a request asks for (`options.language()`, empty if unset).
fn requested_language(options: &Options) -> &str {
    match &options.has_language {
        Some(options::HasLanguage::Language(language)) => language,
        None => "",
    }
}

impl<'a> NarrativeBuilder<'a> {
    /// A builder for the request's language and units (`NarrativeBuilderFactory::Create`).
    ///
    /// # Errors
    ///
    /// [`OdinError::InvalidLanguageTag`] if the language has no dictionary.
    pub(crate) fn new(
        options: &Options,
        leg: &'a TripLeg,
        markup: &'a MarkupFormatter,
    ) -> Result<Self, OdinError> {
        let dictionary = dictionary(requested_language(options))?;
        let rules = if dictionary.language_tag == "it-IT" {
            LanguageRules::Italian
        } else {
            LanguageRules::Default
        };
        Ok(Self {
            leg,
            dictionary,
            markup,
            miles: options.units == options::Units::Miles as i32,
            rules,
        })
    }

    /// Forms every maneuver's instructions, then the verbal multi-cues (`Build`).
    pub(crate) fn build(&self, maneuvers: &mut [Maneuver]) -> Result<(), OdinError> {
        for index in 0..maneuvers.len() {
            let (before, rest) = maneuvers.split_at_mut(index);
            let prev = before.last();
            let maneuver = &mut rest[0];
            self.form_instructions(maneuver, prev)?;
            let bss = bss_maneuver_prefix(maneuver.bss_maneuver_type);
            if !bss.is_empty() {
                maneuver.instruction.insert_str(0, bss);
            }
        }
        self.form_verbal_multi_cues(maneuvers)
    }

    #[expect(clippy::too_many_lines, reason = "mirrors mainline's single switch")]
    fn form_instructions(
        &self,
        maneuver: &mut Maneuver,
        prev: Option<&Maneuver>,
    ) -> Result<(), OdinError> {
        let Ok(maneuver_type) = Type::try_from(maneuver.maneuver_type) else {
            return self.form_continue_or_pass(maneuver);
        };
        match maneuver_type {
            Type::KStartRight
            | Type::KStart
            | Type::KStartLeft
            | Type::KFerryExit
            | Type::KPostTransitConnectionDestination => {
                maneuver.instruction = self.form_start_instruction(maneuver)?;
                maneuver.verbal_succinct_transition_instruction =
                    self.form_verbal_succinct_start_transition_instruction(maneuver)?;
                maneuver.verbal_pre_transition_instruction =
                    self.form_verbal_start_instruction(maneuver)?;
                maneuver.verbal_post_transition_instruction = self
                    .form_verbal_post_transition_instruction(
                        maneuver,
                        maneuver.has_begin_street_names(),
                    )?;
            }
            Type::KDestinationRight | Type::KDestination | Type::KDestinationLeft => {
                maneuver.instruction =
                    self.form_destination_instruction(maneuver, Destination::Text)?;
                maneuver.verbal_transition_alert_instruction =
                    self.form_destination_instruction(maneuver, Destination::VerbalAlert)?;
                maneuver.verbal_pre_transition_instruction =
                    self.form_destination_instruction(maneuver, Destination::Verbal)?;
            }
            Type::KBecomes => {
                if let Some(prev) = prev {
                    maneuver.instruction = self.form_becomes_instruction(maneuver, prev, false)?;
                    maneuver.verbal_pre_transition_instruction =
                        self.form_becomes_instruction(maneuver, prev, true)?;
                }
                maneuver.verbal_post_transition_instruction = self
                    .form_verbal_post_transition_instruction(
                        maneuver,
                        maneuver.has_begin_street_names(),
                    )?;
            }
            Type::KSlightRight
            | Type::KSlightLeft
            | Type::KRight
            | Type::KSharpRight
            | Type::KSharpLeft
            | Type::KLeft => {
                maneuver.instruction = self.form_turn_instruction(maneuver)?;
                maneuver.verbal_succinct_transition_instruction =
                    self.form_verbal_succinct_turn_transition_instruction(maneuver)?;
                maneuver.verbal_transition_alert_instruction =
                    self.form_verbal_turn_instruction(maneuver, VERBAL_ALERT_ELEMENT_MAX_COUNT)?;
                maneuver.verbal_pre_transition_instruction =
                    self.form_verbal_turn_instruction(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT)?;
                maneuver.verbal_post_transition_instruction = self
                    .form_verbal_post_transition_instruction(
                        maneuver,
                        maneuver.has_begin_street_names(),
                    )?;
            }
            Type::KUturnRight | Type::KUturnLeft => {
                maneuver.instruction = self.form_uturn_instruction(maneuver)?;
                maneuver.verbal_succinct_transition_instruction =
                    self.form_verbal_succinct_uturn_transition_instruction(maneuver)?;
                maneuver.verbal_transition_alert_instruction =
                    self.form_verbal_alert_uturn_instruction(maneuver)?;
                maneuver.verbal_pre_transition_instruction =
                    self.form_verbal_uturn_instruction(maneuver)?;
                maneuver.verbal_post_transition_instruction =
                    self.form_verbal_post_transition_instruction(maneuver, false)?;
            }
            Type::KRampStraight => {
                maneuver.instruction = self.form_ramp_straight_instruction(maneuver)?;
                maneuver.verbal_transition_alert_instruction =
                    self.form_verbal_alert_ramp_straight_instruction(maneuver)?;
                maneuver.verbal_pre_transition_instruction =
                    self.form_verbal_ramp_straight_instruction(maneuver)?;
                self.set_ramp_post_transition(maneuver)?;
            }
            Type::KRampRight | Type::KRampLeft => {
                maneuver.instruction = self.form_ramp_instruction(maneuver)?;
                maneuver.verbal_transition_alert_instruction =
                    self.form_verbal_ramp_instruction(maneuver, true)?;
                maneuver.verbal_pre_transition_instruction =
                    self.form_verbal_ramp_instruction(maneuver, false)?;
                self.set_ramp_post_transition(maneuver)?;
            }
            Type::KExitRight | Type::KExitLeft => {
                maneuver.instruction = self.form_exit_instruction(maneuver)?;
                maneuver.verbal_transition_alert_instruction =
                    self.form_verbal_exit_instruction(maneuver, true)?;
                maneuver.verbal_pre_transition_instruction =
                    self.form_verbal_exit_instruction(maneuver, false)?;
                self.set_ramp_post_transition(maneuver)?;
            }
            Type::KStayStraight | Type::KStayRight | Type::KStayLeft => {
                if maneuver.to_stay_on {
                    maneuver.instruction = self.form_keep_to_stay_on_instruction(maneuver)?;
                    maneuver.verbal_transition_alert_instruction =
                        self.form_verbal_alert_keep_to_stay_on_instruction(maneuver)?;
                    maneuver.verbal_pre_transition_instruction =
                        self.form_verbal_keep_to_stay_on_instruction(maneuver)?;
                } else {
                    maneuver.instruction = self.form_keep_instruction(maneuver)?;
                    maneuver.verbal_transition_alert_instruction =
                        self.form_verbal_keep_instruction(maneuver, true)?;
                    maneuver.verbal_pre_transition_instruction =
                        self.form_verbal_keep_instruction(maneuver, false)?;
                }
                // For a ramp, only add a verbal post-transition instruction if the ramp is long.
                if !maneuver.ramp
                    || maneuver.has_collapsed_merge_maneuver
                    || maneuver.length > VERBAL_POST_MINIMUM_RAMP_LENGTH
                {
                    maneuver.verbal_post_transition_instruction =
                        self.form_verbal_post_transition_instruction(maneuver, false)?;
                }
            }
            Type::KMerge | Type::KMergeRight | Type::KMergeLeft => {
                maneuver.instruction = self.form_merge_instruction(maneuver)?;
                maneuver.verbal_succinct_transition_instruction =
                    self.form_verbal_succinct_merge_transition_instruction(maneuver)?;
                // Only alert for a merge after a long previous maneuver.
                if prev.is_some_and(|prev| {
                    prev.length > VERBAL_ALERT_MERGE_PRIOR_MANEUVER_MINIMUM_LENGTH
                }) {
                    maneuver.verbal_transition_alert_instruction = self
                        .form_verbal_merge_instruction(maneuver, VERBAL_ALERT_ELEMENT_MAX_COUNT)?;
                }
                maneuver.verbal_pre_transition_instruction =
                    self.form_verbal_merge_instruction(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT)?;
                maneuver.verbal_post_transition_instruction =
                    self.form_verbal_post_transition_instruction(maneuver, false)?;
            }
            Type::KRoundaboutEnter => {
                maneuver.instruction = self.form_enter_roundabout_instruction(maneuver)?;
                maneuver.verbal_succinct_transition_instruction =
                    self.form_verbal_succinct_enter_roundabout_transition_instruction(maneuver)?;
                maneuver.verbal_transition_alert_instruction = self
                    .form_verbal_enter_roundabout_instruction(
                        maneuver,
                        VERBAL_ALERT_ELEMENT_MAX_COUNT,
                    )?;
                maneuver.verbal_pre_transition_instruction = self
                    .form_verbal_enter_roundabout_instruction(
                        maneuver,
                        VERBAL_PRE_ELEMENT_MAX_COUNT,
                    )?;
                if maneuver.has_combined_enter_exit_roundabout {
                    let include = !maneuver.roundabout_exit_begin_street_names.is_empty();
                    maneuver.verbal_post_transition_instruction =
                        self.form_verbal_post_transition_instruction(maneuver, include)?;
                }
            }
            Type::KRoundaboutExit => {
                maneuver.instruction = self.form_exit_roundabout_instruction(maneuver)?;
                maneuver.verbal_succinct_transition_instruction =
                    self.form_verbal_succinct_exit_roundabout_transition_instruction(maneuver)?;
                maneuver.verbal_pre_transition_instruction =
                    self.form_verbal_exit_roundabout_instruction(maneuver)?;
                maneuver.verbal_post_transition_instruction = self
                    .form_verbal_post_transition_instruction(
                        maneuver,
                        maneuver.has_begin_street_names(),
                    )?;
            }
            Type::KFerryEnter => {
                maneuver.instruction = self.form_enter_ferry_instruction(maneuver)?;
                maneuver.verbal_transition_alert_instruction = self
                    .form_verbal_enter_ferry_instruction(
                        maneuver,
                        VERBAL_ALERT_ELEMENT_MAX_COUNT,
                    )?;
                maneuver.verbal_pre_transition_instruction = self
                    .form_verbal_enter_ferry_instruction(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT)?;
                maneuver.verbal_post_transition_instruction =
                    self.form_verbal_post_transition_instruction(maneuver, false)?;
            }
            Type::KTransitConnectionStart
            | Type::KTransitConnectionTransfer
            | Type::KTransitConnectionDestination
            | Type::KTransit
            | Type::KTransitRemainOn
            | Type::KTransitTransfer => return Err(OdinError::TransitUnsupported),
            Type::KElevatorEnter => {
                let instruction =
                    Self::form_level_instruction(maneuver, &self.instructions().elevator, true)?;
                if maneuver.node_type == Some(node::Type::KElevator as i32) {
                    maneuver
                        .verbal_transition_alert_instruction
                        .clone_from(&instruction);
                    maneuver
                        .verbal_pre_transition_instruction
                        .clone_from(&instruction);
                    maneuver.instruction = instruction;
                    maneuver.verbal_post_transition_instruction =
                        self.form_verbal_post_transition_instruction(maneuver, false)?;
                } else {
                    maneuver.instruction = instruction;
                }
            }
            Type::KStepsEnter => {
                let instruction =
                    Self::form_level_instruction(maneuver, &self.instructions().steps, true)?;
                maneuver
                    .verbal_transition_alert_instruction
                    .clone_from(&instruction);
                maneuver
                    .verbal_pre_transition_instruction
                    .clone_from(&instruction);
                maneuver.instruction = instruction;
                maneuver.verbal_post_transition_instruction =
                    self.form_verbal_post_transition_instruction(maneuver, false)?;
            }
            Type::KEscalatorEnter => {
                maneuver.instruction =
                    Self::form_level_instruction(maneuver, &self.instructions().escalator, true)?;
            }
            Type::KBuildingEnter => {
                maneuver.instruction =
                    self.form_building_instruction(maneuver, &self.instructions().enter_building)?;
            }
            Type::KBuildingExit => {
                maneuver.instruction =
                    self.form_building_instruction(maneuver, &self.instructions().exit_building)?;
            }
            Type::KLevelChange => {
                maneuver.instruction = Self::form_level_instruction(
                    maneuver,
                    &self.instructions().level_change,
                    false,
                )?;
            }
            Type::KParkVehicle => {
                self.instructions()
                    .park_vehicle
                    .phrase(0)?
                    .clone_into(&mut maneuver.instruction);
            }
            Type::KContinue | Type::KNone => return self.form_continue_or_pass(maneuver),
        }
        Ok(())
    }

    /// Continue maneuvers, and every type without its own case (`default` in mainline's switch).
    fn form_continue_or_pass(&self, maneuver: &mut Maneuver) -> Result<(), OdinError> {
        if maneuver.node_type.is_some() {
            let instruction = self.form_pass_instruction(maneuver)?;
            maneuver
                .verbal_pre_transition_instruction
                .clone_from(&instruction);
            maneuver.instruction = instruction;
        } else {
            maneuver.instruction = self.form_continue_instruction(maneuver)?;
            maneuver.verbal_transition_alert_instruction =
                self.form_verbal_continue_instruction(maneuver, true)?;
            maneuver.verbal_pre_transition_instruction =
                self.form_verbal_continue_instruction(maneuver, false)?;
            maneuver.verbal_post_transition_instruction =
                self.form_verbal_post_transition_instruction(maneuver, false)?;
        }
        Ok(())
    }

    /// Ramps and exits only get a verbal post-transition instruction when long,
    /// when they contain an obvious maneuver, or when they absorbed a merge.
    fn set_ramp_post_transition(&self, maneuver: &mut Maneuver) -> Result<(), OdinError> {
        if maneuver.length > VERBAL_POST_MINIMUM_RAMP_LENGTH
            || maneuver.contains_obvious_maneuver
            || maneuver.has_collapsed_merge_maneuver
        {
            maneuver.verbal_post_transition_instruction =
                self.form_verbal_post_transition_instruction(maneuver, false)?;
        }
        Ok(())
    }

    // --- Verbal multi-cues ---

    /// Announces quick maneuvers together with the next one (`FormVerbalMultiCue`).
    fn form_verbal_multi_cues(&self, maneuvers: &mut [Maneuver]) -> Result<(), OdinError> {
        let mut prev: Option<usize> = None;
        for index in 0..maneuvers.len() {
            if maneuvers[index].is_blind() {
                continue;
            }
            if let Some(prev_index) = prev {
                let (head, tail) = maneuvers.split_at_mut(index);
                let (prev_man, maneuver) = (&mut head[prev_index], &tail[0]);
                if self.is_verbal_multi_cue_possible(prev_man, maneuver) {
                    let distant = if maneuver.is_any_type(&[
                        Type::KSlightRight,
                        Type::KRight,
                        Type::KSharpRight,
                        Type::KUturnRight,
                        Type::KRampRight,
                        Type::KExitRight,
                        Type::KStayRight,
                    ]) {
                        prev_man.has_right_traversable_outbound_intersecting_edge
                    } else if maneuver.is_any_type(&[
                        Type::KSlightLeft,
                        Type::KLeft,
                        Type::KSharpLeft,
                        Type::KUturnLeft,
                        Type::KRampLeft,
                        Type::KExitLeft,
                        Type::KStayLeft,
                    ]) {
                        prev_man.has_left_traversable_outbound_intersecting_edge
                    } else if maneuver.is_destination_type() {
                        prev_man.has_left_traversable_outbound_intersecting_edge
                            || prev_man.has_right_traversable_outbound_intersecting_edge
                    } else {
                        false
                    };
                    if distant {
                        prev_man.distant_verbal_multi_cue = true;
                    } else {
                        prev_man.imminent_verbal_multi_cue = true;
                    }

                    let next_cue = if maneuver.verbal_transition_alert_instruction.is_empty() {
                        &maneuver.verbal_pre_transition_instruction
                    } else {
                        &maneuver.verbal_transition_alert_instruction
                    };
                    if !prev_man.verbal_succinct_transition_instruction.is_empty() {
                        prev_man.verbal_succinct_transition_instruction = self
                            .form_verbal_multi_cue(
                                prev_man,
                                &prev_man.verbal_succinct_transition_instruction,
                                next_cue,
                            )?;
                    }
                    prev_man.verbal_pre_transition_instruction = self.form_verbal_multi_cue(
                        prev_man,
                        &prev_man.verbal_pre_transition_instruction,
                        next_cue,
                    )?;
                }
            }
            prev = Some(index);
        }
        Ok(())
    }

    fn form_verbal_multi_cue(
        &self,
        maneuver: &Maneuver,
        first_verbal_cue: &str,
        second_verbal_cue: &str,
    ) -> Result<String, OdinError> {
        let phrase_id = u8::from(maneuver.distant_verbal_multi_cue);
        let lengths = &self.instructions().post_transition_verbal;
        let length = self.form_length(maneuver, lengths)?;
        let instruction = self
            .instructions()
            .verbal_multi_cue
            .phrase(phrase_id)?
            .replace(tag::CURRENT_VERBAL_CUE, first_verbal_cue)
            .replace(tag::NEXT_VERBAL_CUE, second_verbal_cue)
            .replace(tag::LENGTH, &length);
        Ok(self.finish(instruction))
    }

    /// Whether a maneuver can be announced together with the next one (`IsVerbalMultiCuePossible`).
    fn is_verbal_multi_cue_possible(&self, maneuver: &Maneuver, next: &Maneuver) -> bool {
        let _ = self;
        !maneuver.verbal_pre_transition_instruction.is_empty()
            && (!next.verbal_transition_alert_instruction.is_empty()
                || !next.verbal_pre_transition_instruction.is_empty())
            && is_within_verbal_multi_cue_bounds(maneuver)
            && !next.is_merge_type()
            && (!maneuver.roundabout || maneuver.has_combined_enter_exit_roundabout)
            && !(maneuver.is_type(Type::KRoundaboutExit) && next.roundabout)
            && !maneuver.is_transit()
            && !next.is_transit()
            && !maneuver.transit_connection
            && !next.transit_connection
    }

    // --- Shared helpers ---

    pub(super) fn instructions(&self) -> &'static super::dictionary::Instructions {
        &self.dictionary.instructions
    }

    /// Applies the language's final touches (`FormArticulatedPrepositions`).
    pub(super) fn finish(&self, instruction: String) -> String {
        match self.rules {
            LanguageRules::Default => instruction,
            LanguageRules::Italian => instruction
                .replace(" su il ", " sul ")
                .replace(" su la ", " sulla "),
        }
    }

    /// The text of a list of street names, with labels for unnamed walkways and the like
    /// (`FormStreetNames`).
    pub(super) fn form_street_names(
        &self,
        maneuver: &Maneuver,
        street_names: &StreetNames,
        empty_labels: Option<&[String]>,
        enhance_empty_street_names: bool,
        format: ListFormat<'_>,
    ) -> Result<String, OdinError> {
        let mut text = if street_names.is_empty() {
            String::new()
        } else {
            self.street_names_text(street_names, format)?
        };
        if enhance_empty_street_names
            && text.is_empty()
            && let Some(labels) = empty_labels
        {
            let index = if maneuver.is_blind() {
                if maneuver.is_steps {
                    Some(empty_label::STEPS)
                } else if maneuver.is_bridge {
                    Some(empty_label::BRIDGE)
                } else if maneuver.is_tunnel {
                    Some(empty_label::TUNNEL)
                } else {
                    None
                }
            } else if maneuver.has_mode(TravelMode::KPedestrian) && maneuver.unnamed_walkway() {
                Some(if maneuver.pedestrian_crossing {
                    empty_label::PEDESTRIAN_CROSSING
                } else {
                    empty_label::WALKWAY
                })
            } else if maneuver.has_mode(TravelMode::KBicycle) && maneuver.unnamed_cycleway() {
                Some(empty_label::CYCLEWAY)
            } else if maneuver.has_mode(TravelMode::KBicycle)
                && maneuver.unnamed_mountain_bike_trail()
            {
                Some(empty_label::MOUNTAIN_BIKE_TRAIL)
            } else {
                None
            };
            if let Some(index) = index {
                label(labels, index)?.clone_into(&mut text);
            }
        }
        Ok(text)
    }

    /// The street names joined by the delimiter, verbally formatted if asked.
    pub(super) fn street_names_text(
        &self,
        street_names: &StreetNames,
        format: ListFormat<'_>,
    ) -> Result<String, OdinError> {
        let mut text = String::new();
        for (count, street_name) in street_names.iter().enumerate() {
            if format.max_count > 0 && count == format.max_count as usize {
                break;
            }
            if !text.is_empty() {
                text.push_str(format.delim);
            }
            match format.verbal {
                Some(verbal) => {
                    text.push_str(&verbal.format_street_name(street_name, self.markup)?);
                }
                None => text.push_str(street_name.value()),
            }
        }
        Ok(text)
    }

    /// The sign format for a list format.
    pub(super) fn sign_format<'b>(
        &'b self,
        format: ListFormat<'b>,
        limit_by_consecutive_count: bool,
    ) -> SignFormat<'b> {
        SignFormat {
            max_count: format.max_count,
            limit_by_consecutive_count,
            delim: format.delim,
            verbal: format.verbal.map(|verbal| (verbal, self.markup)),
        }
    }

    /// A maneuver's length in the requested units (`FormLength`).
    ///
    /// A combined roundabout entry and exit uses the length after the roundabout.
    pub(super) fn form_length(
        &self,
        maneuver: &Maneuver,
        lengths: &Subset,
    ) -> Result<String, OdinError> {
        let distance = if maneuver.has_combined_enter_exit_roundabout {
            maneuver.roundabout_exit_length_in(self.miles)
        } else {
            maneuver.length_in(self.miles)
        };
        self.form_distance(distance, lengths)
    }

    /// A distance in the requested units, kilometers or miles.
    pub(super) fn form_distance(
        &self,
        distance: f32,
        lengths: &Subset,
    ) -> Result<String, OdinError> {
        if self.miles {
            self.form_us_customary_length(distance, &lengths.us_customary_lengths)
        } else {
            self.form_metric_length(distance, &lengths.metric_lengths)
        }
    }

    /// `FormMetricLength`:
    /// "<KILOMETERS> kilometers", "1 kilometer", "<METERS> meters", or "less than 10 meters".
    fn form_metric_length(
        &self,
        kilometers: f32,
        metric_lengths: &[String],
    ) -> Result<String, OdinError> {
        const KILOMETERS: usize = 0;
        const ONE_KILOMETER: usize = 1;
        const METERS: usize = 2;
        const SMALL_METERS: usize = 3;

        let numbers = &self.dictionary.number_format;
        let meters = (kilometers * METERS_PER_KM).round();
        let (template, distance) = if meters > 949.0 {
            // Whole kilometers above 3 km, half kilometers below.
            let rounded = if kilometers > 3.0 {
                kilometers.round()
            } else {
                (kilometers * 2.0).round() / 2.0
            };
            if rounded == 1.0 {
                (label(metric_lengths, ONE_KILOMETER)?, String::new())
            } else {
                // One decimal for a fraction, none for whole numbers.
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "mainline compares with an int cast"
                )]
                let precision = usize::from(f64::from(rounded) != f64::from(rounded as i32));
                (
                    label(metric_lengths, KILOMETERS)?,
                    numbers.fixed(rounded, precision),
                )
            }
        } else if meters > 94.0 {
            (
                label(metric_lengths, METERS)?,
                numbers.general((meters / 100.0).round() * 100.0),
            )
        } else if meters > 9.0 {
            (
                label(metric_lengths, METERS)?,
                numbers.general((meters / 10.0).round() * 10.0),
            )
        } else {
            (label(metric_lengths, SMALL_METERS)?, String::new())
        };
        Ok(template
            .replace(tag::KILOMETERS, &distance)
            .replace(tag::METERS, &distance))
    }

    /// `FormUsCustomaryLength`: "<MILES> miles", "1 mile", "a half mile", "a quarter mile",
    /// "<FEET> feet", or "less than 10 feet".
    fn form_us_customary_length(
        &self,
        miles: f32,
        us_customary_lengths: &[String],
    ) -> Result<String, OdinError> {
        const MILES: usize = 0;
        const ONE_MILE: usize = 1;
        const HALF_MILE: usize = 2;
        const QUARTER_MILE: usize = 3;
        const FEET: usize = 4;
        const SMALL_FEET: usize = 5;

        let numbers = &self.dictionary.number_format;
        let feet = (miles * FEET_PER_MILE).round();
        let (template, distance) = if feet > 1000.0 {
            let rounded = if miles > 2.0 {
                miles.round()
            } else if f64::from(miles) >= 0.625 {
                (miles * 2.0).round() / 2.0
            } else {
                (miles * 4.0).round() / 4.0
            };
            if rounded == 0.25 {
                (label(us_customary_lengths, QUARTER_MILE)?, String::new())
            } else if rounded == 0.5 {
                (label(us_customary_lengths, HALF_MILE)?, String::new())
            } else if rounded == 1.0 {
                (label(us_customary_lengths, ONE_MILE)?, String::new())
            } else {
                // Mainline only shows a decimal for exactly 1.5 miles.
                let precision = usize::from(rounded == 1.5);
                (
                    label(us_customary_lengths, MILES)?,
                    numbers.fixed(rounded, precision),
                )
            }
        } else if feet > 94.0 {
            (
                label(us_customary_lengths, FEET)?,
                numbers.general((feet / 100.0).round() * 100.0),
            )
        } else if feet > 9.0 {
            (
                label(us_customary_lengths, FEET)?,
                numbers.general((feet / 10.0).round() * 10.0),
            )
        } else {
            (label(us_customary_lengths, SMALL_FEET)?, String::new())
        };
        Ok(template
            .replace(tag::MILES, &distance)
            .replace(tag::TENTHS_OF_MILE, &distance)
            .replace(tag::FEET, &distance))
    }
}

/// Destination instruction variants, which differ only in their subset and formatting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Destination {
    Text,
    VerbalAlert,
    Verbal,
}

fn is_within_verbal_multi_cue_bounds(maneuver: &Maneuver) -> bool {
    if maneuver.is_start_type() {
        maneuver.basic_time < VERBAL_MULTI_CUE_TIME_START_MANEUVER_THRESHOLD
    } else {
        maneuver.basic_time < VERBAL_MULTI_CUE_TIME_THRESHOLD
    }
}

/// The untranslated English prefix mainline puts on bike share maneuvers (`FormBssManeuverType`).
fn bss_maneuver_prefix(bss_maneuver_type: i32) -> &'static str {
    if bss_maneuver_type == BssManeuverType::KRentBikeAtBikeShare as i32 {
        "Then rent a bike at BSS. "
    } else if bss_maneuver_type == BssManeuverType::KReturnBikeAtBikeShare as i32 {
        "Then return the bike to BSS. "
    } else {
        ""
    }
}

/// The verbal alert Tyr's OSRM serializer plays before a maneuver: "In <LENGTH>, <cue>"
/// (`NarrativeBuilder::FormVerbalAlertApproachInstruction`).
///
/// Mainline's OSRM serializer builds these from each maneuver's verbal transition alert,
/// using the request's language and units, and no phoneme markup.
///
/// `distance` is in the request's units: kilometers, or miles.
/// Note that mainline's OSRM serializer always passes kilometers here,
/// so for requests in miles it announces the kilometer value as miles
/// (a mainline bug; whether to reproduce it is the caller's choice).
///
/// # Errors
///
/// [`OdinError::InvalidLanguageTag`] if the request's language has no dictionary,
/// or a phrase or label error if its dictionary lacks what the alert needs.
pub fn verbal_alert_approach_instruction(
    options: &Options,
    distance: f32,
    verbal_cue: &str,
) -> Result<String, OdinError> {
    let leg = TripLeg::default();
    let markup = MarkupFormatter::default();
    let builder = NarrativeBuilder::new(options, &leg, &markup)?;
    let subset = &builder.instructions().approach_verbal_alert;
    let length = builder.form_distance(distance, subset)?;
    let instruction = subset
        .phrase(0)?
        .replace(tag::LENGTH, &length)
        .replace(tag::CURRENT_VERBAL_CUE, verbal_cue);
    Ok(builder.finish(instruction))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(language: &str, units: options::Units) -> Options {
        Options {
            units: units as i32,
            has_language: Some(options::HasLanguage::Language(language.to_owned())),
            ..Options::default()
        }
    }

    /// Announcements from mainline's OSRM responses in the Andorra captures
    /// (`auto_town` and `auto_catalan_miles`), with distances (in the request's units) that
    /// round to them.
    #[test]
    fn approach_alerts_match_mainline_announcements() {
        let km = options("en-US", options::Units::Kilometers);
        assert_eq!(
            verbal_alert_approach_instruction(&km, 0.5, "Turn left onto Travessera del Jardí.")
                .unwrap(),
            "In 500 meters, Turn left onto Travessera del Jardí."
        );
        assert_eq!(
            verbal_alert_approach_instruction(&km, 0.052, "You will arrive at your destination.")
                .unwrap(),
            "In 50 meters, You will arrive at your destination."
        );
        let ca_miles = options("ca-ES", options::Units::Miles);
        assert_eq!(
            verbal_alert_approach_instruction(
                &ca_miles,
                0.5,
                "Gira a l'esquerra cap a Travessera del Jardí."
            )
            .unwrap(),
            "En mitja milla, Gira a l'esquerra cap a Travessera del Jardí."
        );
        assert_eq!(
            verbal_alert_approach_instruction(&ca_miles, 0.057, "Arribaràs a la teva destinació.")
                .unwrap(),
            "En 300 peus, Arribaràs a la teva destinació."
        );
        assert_eq!(
            verbal_alert_approach_instruction(&options("xx", options::Units::Miles), 1.0, ""),
            Err(OdinError::InvalidLanguageTag)
        );
    }

    #[test]
    fn bike_share_prefixes() {
        assert_eq!(bss_maneuver_prefix(BssManeuverType::KNoneAction as i32), "");
        assert_eq!(
            bss_maneuver_prefix(BssManeuverType::KRentBikeAtBikeShare as i32),
            "Then rent a bike at BSS. "
        );
        assert_eq!(bss_maneuver_prefix(99), "");
    }
}
