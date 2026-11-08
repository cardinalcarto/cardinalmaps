//! Forming each kind of instruction (the `Form*Instruction` methods of `NarrativeBuilder`).
//!
//! Each function picks a phrase id from what the maneuver has
//! (street names, signs, and so on), looks the phrase up,
//! and replaces its tags in the same order as mainline,
//! since a value that happens to contain a later tag would otherwise come out differently.
//! The comments list the English phrases for reference.

use super::builder::{
    Destination, ELEMENT_MAX_COUNT, LIMIT_BY_CONSECUTIVE_COUNT, ListFormat, NarrativeBuilder,
    VERBAL_ALERT_ELEMENT_MAX_COUNT, VERBAL_DELIM, VERBAL_POST_ELEMENT_MAX_COUNT,
    VERBAL_PRE_ELEMENT_MAX_COUNT, tag,
};
use super::dictionary::{Subset, label};
use crate::error::OdinError;
use crate::maneuver::{Maneuver, RelativeDirection};
use crate::signs::SignFormat;
use crate::street_names::StreetNames;
use valhalla_proto::directions_leg::maneuver::Type;
use valhalla_proto::trip_leg::node;
use valhalla_proto::{Location, TravelMode};
use valhalla_response::ErrorCode;

/// Roundabout exit counts in this range are spoken as ordinals (`the 3rd exit`).
const ROUNDABOUT_EXIT_COUNT_BOUNDS: core::ops::RangeInclusive<u32> = 1..=10;

/// Indexes into `pass.object_labels`.
const GATE_INDEX: usize = 0;
const BOLLARD_INDEX: usize = 1;
const STREET_INTERSECTION_INDEX: usize = 2;

/// Replaces every occurrence of a tag (`boost::replace_all`).
fn replace(instruction: &mut String, tag: &str, value: &str) {
    if instruction.contains(tag) {
        *instruction = instruction.replace(tag, value);
    }
}

/// The text sign format with the default limits for textual instructions.
const TEXT_SIGNS: SignFormat<'static> =
    SignFormat::text(ELEMENT_MAX_COUNT, LIMIT_BY_CONSECUTIVE_COUNT);

impl NarrativeBuilder<'_> {
    /// The verbal list format for a maneuver.
    fn verbal(maneuver: &Maneuver, max_count: u32) -> ListFormat<'static> {
        ListFormat::verbal(maneuver, max_count, VERBAL_DELIM)
    }

    /// The verbal sign format for a maneuver.
    fn verbal_signs<'b>(&'b self, maneuver: &Maneuver, max_count: u32) -> SignFormat<'b> {
        self.sign_format(
            Self::verbal(maneuver, max_count),
            LIMIT_BY_CONSECUTIVE_COUNT,
        )
    }

    /// The verbal format for exit numbers: every number, not limited by consecutive count.
    fn verbal_exit_numbers<'b>(&'b self, maneuver: &Maneuver) -> SignFormat<'b> {
        self.sign_format(Self::verbal(maneuver, 0), false)
    }

    /// `UpdateObviousManeuverStreetNames`: a maneuver that contains an obvious maneuver
    /// names the street it begins on.
    fn update_obvious_maneuver_street_names(
        maneuver: &Maneuver,
        begin_street_names: &mut String,
        street_names: &mut String,
    ) {
        if maneuver.contains_obvious_maneuver && !begin_street_names.is_empty() {
            *street_names = core::mem::take(begin_street_names);
        }
    }

    pub(super) fn form_start_instruction(&self, maneuver: &Maneuver) -> Result<String, OdinError> {
        // "Head/Drive/Walk/Bike <CARDINAL_DIRECTION> [on <STREET_NAMES>]", with begin street names
        // adding "on <BEGIN_STREET_NAMES>. Continue on <STREET_NAMES>."
        let subset = &self.instructions().start;
        let cardinal_direction = label(&subset.cardinal_directions, direction_index(maneuver))?;
        let mut street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            ListFormat::TEXT,
        )?;
        let mut begin_street_names = self.form_street_names(
            maneuver,
            &maneuver.begin_street_names,
            None,
            false,
            ListFormat::TEXT,
        )?;
        Self::update_obvious_maneuver_street_names(
            maneuver,
            &mut begin_street_names,
            &mut street_names,
        );

        let mut phrase_id =
            u8::from(!street_names.is_empty()) + u8::from(!begin_street_names.is_empty());
        phrase_id += mode_offset(maneuver, [4, 8, 16]);

        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::CARDINAL_DIRECTION,
            cardinal_direction,
        );
        replace(&mut instruction, tag::STREET_NAMES, &street_names);
        replace(
            &mut instruction,
            tag::BEGIN_STREET_NAMES,
            &begin_street_names,
        );
        Ok(self.finish(instruction))
    }

    pub(super) fn form_verbal_start_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        // 0-4 "Head", 5-9 "Drive", 10-14 "Walk", 15-19 "Bike":
        // "<CARDINAL_DIRECTION>", "+ for <LENGTH>", "on <STREET_NAMES>", "on <STREET_NAMES> for <LENGTH>",
        // "on <BEGIN_STREET_NAMES>".
        let subset = &self.instructions().start_verbal;
        let format = Self::verbal(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT);
        let cardinal_direction = label(&subset.cardinal_directions, direction_index(maneuver))?;
        let mut street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            format,
        )?;
        let mut begin_street_names = self.form_street_names(
            maneuver,
            &maneuver.begin_street_names,
            Some(&subset.empty_street_name_labels),
            false,
            format,
        )?;
        Self::update_obvious_maneuver_street_names(
            maneuver,
            &mut begin_street_names,
            &mut street_names,
        );

        let mut phrase_id = mode_offset(maneuver, [5, 10, 15]);
        if !street_names.is_empty() {
            phrase_id += 2;
        }
        if !begin_street_names.is_empty() {
            phrase_id += 2;
        } else if maneuver.include_verbal_pre_transition_length {
            phrase_id += 1;
        }

        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::CARDINAL_DIRECTION,
            cardinal_direction,
        );
        replace(&mut instruction, tag::STREET_NAMES, &street_names);
        replace(
            &mut instruction,
            tag::BEGIN_STREET_NAMES,
            &begin_street_names,
        );
        replace(
            &mut instruction,
            tag::LENGTH,
            &self.form_length(maneuver, subset)?,
        );
        Ok(self.finish(instruction))
    }

    pub(super) fn form_verbal_succinct_start_transition_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().start_verbal;
        let cardinal_direction = label(&subset.cardinal_directions, direction_index(maneuver))?;
        let mut phrase_id = mode_offset(maneuver, [5, 10, 15]);
        if maneuver.include_verbal_pre_transition_length {
            phrase_id += 1;
        }
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::CARDINAL_DIRECTION,
            cardinal_direction,
        );
        replace(
            &mut instruction,
            tag::LENGTH,
            &self.form_length(maneuver, subset)?,
        );
        Ok(self.finish(instruction))
    }

    /// `FormDestinationInstruction`, `FormVerbalAlertDestinationInstruction`,
    /// and `FormVerbalDestinationInstruction`:
    /// `You have arrived at your destination.` or `at <DESTINATION>.`,
    /// or `<DESTINATION> is on the <RELATIVE_DIRECTION>.`
    pub(super) fn form_destination_instruction(
        &self,
        maneuver: &Maneuver,
        kind: Destination,
    ) -> Result<String, OdinError> {
        let destination_location = self.destination()?;
        let mut phrase_id = 0;
        let mut destination = String::new();
        if !destination_location.name.is_empty() {
            phrase_id += 1;
            destination.clone_from(&destination_location.name);
        } else if !destination_location.street.is_empty() {
            phrase_id += 1;
            destination = match (kind, maneuver.verbal_formatter) {
                (Destination::VerbalAlert | Destination::Verbal, Some(verbal)) => {
                    verbal.format(&destination_location.street)
                }
                _ => destination_location.street.clone(),
            };
        }

        // The relative directions always come from the textual subset.
        let relative_directions = &self.instructions().destination.relative_directions;
        let mut relative_direction = "";
        if maneuver.is_type(Type::KDestinationLeft) {
            phrase_id += 2;
            relative_direction = label(relative_directions, 0)?;
        } else if maneuver.is_type(Type::KDestinationRight) {
            phrase_id += 2;
            relative_direction = label(relative_directions, 1)?;
        }

        let subset = match kind {
            Destination::Text => &self.instructions().destination,
            Destination::VerbalAlert => &self.instructions().destination_verbal_alert,
            Destination::Verbal => &self.instructions().destination_verbal,
        };
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        if phrase_id > 0 {
            replace(
                &mut instruction,
                tag::RELATIVE_DIRECTION,
                relative_direction,
            );
            replace(&mut instruction, tag::DESTINATION, &destination);
        }
        Ok(self.finish(instruction))
    }

    fn destination(&self) -> Result<&Location, OdinError> {
        match self.leg.location.as_slice() {
            [_, .., destination] => Ok(destination),
            _ => Err(OdinError::Coded(ErrorCode::TripNotEnoughLocations)),
        }
    }

    /// `<PREVIOUS_STREET_NAMES> becomes <STREET_NAMES>.`
    ///
    /// The verbal form formats both names with the previous maneuver's verbal formatter.
    pub(super) fn form_becomes_instruction(
        &self,
        maneuver: &Maneuver,
        prev: &Maneuver,
        verbal: bool,
    ) -> Result<String, OdinError> {
        let (subset, format) = if verbal {
            (
                &self.instructions().becomes_verbal,
                ListFormat {
                    max_count: VERBAL_PRE_ELEMENT_MAX_COUNT,
                    delim: VERBAL_DELIM,
                    verbal: prev.verbal_formatter,
                },
            )
        } else {
            (&self.instructions().becomes, ListFormat::TEXT)
        };
        let street_names =
            self.form_street_names(maneuver, &maneuver.street_names, None, false, format)?;
        let prev_street_names =
            self.form_street_names(prev, &prev.street_names, None, false, format)?;
        let mut instruction = subset.phrase(0)?.to_owned();
        replace(
            &mut instruction,
            tag::PREVIOUS_STREET_NAMES,
            &prev_street_names,
        );
        replace(&mut instruction, tag::STREET_NAMES, &street_names);
        Ok(self.finish(instruction))
    }

    /// `Continue.`, `Continue on <STREET_NAMES>.`, `Continue at <JUNCTION_NAME>.`,
    /// or `Continue toward <TOWARD_SIGN>.`
    pub(super) fn form_continue_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().continue_;
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            ListFormat::TEXT,
        )?;
        let (phrase_id, junction_name, guide_sign) =
            Self::continue_phrase(maneuver, &street_names, TEXT_SIGNS, [1, 2, 3])?;
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(&mut instruction, tag::STREET_NAMES, &street_names);
        replace(&mut instruction, tag::JUNCTION_NAME, &junction_name);
        replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
        Ok(self.finish(instruction))
    }

    /// The continue phrase id for street names, a junction, or a guide sign,
    /// with the junction name and guide sign text.
    fn continue_phrase(
        maneuver: &Maneuver,
        street_names: &str,
        signs: SignFormat<'_>,
        [street_id, junction_id, guide_id]: [u8; 3],
    ) -> Result<(u8, String, String), OdinError> {
        // A guide sign takes priority over a junction name, which takes priority over street names.
        if maneuver.signs.has_guide() {
            Ok((guide_id, String::new(), maneuver.signs.guide_string(signs)?))
        } else if maneuver.signs.has_junction_name() {
            Ok((
                junction_id,
                maneuver.signs.junction_name_string(signs)?,
                String::new(),
            ))
        } else if !street_names.is_empty() {
            Ok((street_id, String::new(), String::new()))
        } else {
            Ok((0, String::new(), String::new()))
        }
    }

    /// The verbal continue instructions; the pre-transition form may add `for <LENGTH>`.
    pub(super) fn form_verbal_continue_instruction(
        &self,
        maneuver: &Maneuver,
        alert: bool,
    ) -> Result<String, OdinError> {
        let (subset, max_count) = if alert {
            (
                &self.instructions().continue_verbal_alert,
                VERBAL_ALERT_ELEMENT_MAX_COUNT,
            )
        } else {
            (
                &self.instructions().continue_verbal,
                VERBAL_PRE_ELEMENT_MAX_COUNT,
            )
        };
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            Self::verbal(maneuver, max_count),
        )?;
        let signs = self.verbal_signs(maneuver, max_count);
        let mut instruction = if alert {
            let (phrase_id, junction_name, guide_sign) =
                Self::continue_phrase(maneuver, &street_names, signs, [1, 2, 3])?;
            let mut instruction = subset.phrase(phrase_id)?.to_owned();
            replace(&mut instruction, tag::STREET_NAMES, &street_names);
            replace(&mut instruction, tag::JUNCTION_NAME, &junction_name);
            replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
            instruction
        } else {
            let (mut phrase_id, junction_name, guide_sign) =
                Self::continue_phrase(maneuver, &street_names, signs, [2, 4, 6])?;
            if maneuver.include_verbal_pre_transition_length {
                phrase_id += 1;
            }
            let mut instruction = subset.phrase(phrase_id)?.to_owned();
            replace(
                &mut instruction,
                tag::LENGTH,
                &self.form_length(maneuver, subset)?,
            );
            replace(&mut instruction, tag::STREET_NAMES, &street_names);
            replace(&mut instruction, tag::JUNCTION_NAME, &junction_name);
            replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
            instruction
        };
        instruction = self.finish(instruction);
        Ok(instruction)
    }

    /// The turn subset (bear, turn, or sharp) for a turn maneuver.
    fn turn_subset(&self, maneuver: &Maneuver, verbal: bool) -> Result<&Subset, OdinError> {
        let instructions = self.instructions();
        match Type::try_from(maneuver.maneuver_type) {
            Ok(Type::KSlightRight | Type::KSlightLeft) => Ok(if verbal {
                &instructions.bear_verbal
            } else {
                &instructions.bear
            }),
            Ok(Type::KRight | Type::KLeft) => Ok(if verbal {
                &instructions.turn_verbal
            } else {
                &instructions.turn
            }),
            Ok(Type::KSharpRight | Type::KSharpLeft) => Ok(if verbal {
                &instructions.sharp_verbal
            } else {
                &instructions.sharp
            }),
            _ => Err(OdinError::Coded(ErrorCode::InvalidManeuverTypeForTurn)),
        }
    }

    /// `Turn/Bear/Make a sharp <RELATIVE_DIRECTION>`, `onto <STREET_NAMES>`,
    /// `onto <BEGIN_STREET_NAMES>. Continue on <STREET_NAMES>.`, `to stay on <STREET_NAMES>`,
    /// `at <JUNCTION_NAME>`, or `toward <TOWARD_SIGN>`.
    pub(super) fn form_turn_instruction(&self, maneuver: &Maneuver) -> Result<String, OdinError> {
        let subset = self.turn_subset(maneuver, false)?;
        let mut street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            ListFormat::TEXT,
        )?;
        let mut begin_street_names = self.form_street_names(
            maneuver,
            &maneuver.begin_street_names,
            None,
            false,
            ListFormat::TEXT,
        )?;
        Self::update_obvious_maneuver_street_names(
            maneuver,
            &mut begin_street_names,
            &mut street_names,
        );

        let mut junction_name = String::new();
        let mut guide_sign = String::new();
        let phrase_id = if maneuver.signs.has_guide() {
            guide_sign = maneuver.signs.guide_string(TEXT_SIGNS)?;
            5
        } else if maneuver.signs.has_junction_name() {
            junction_name = maneuver.signs.junction_name_string(TEXT_SIGNS)?;
            4
        } else if maneuver.to_stay_on {
            3
        } else if !begin_street_names.is_empty() {
            2
        } else {
            u8::from(!street_names.is_empty())
        };

        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::RELATIVE_DIRECTION,
            relative_two_direction(maneuver, &subset.relative_directions)?,
        );
        replace(&mut instruction, tag::STREET_NAMES, &street_names);
        replace(
            &mut instruction,
            tag::BEGIN_STREET_NAMES,
            &begin_street_names,
        );
        replace(&mut instruction, tag::JUNCTION_NAME, &junction_name);
        replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
        Ok(self.finish(instruction))
    }

    /// The verbal turn instruction, for the alert (one element) and pre-transition (two) forms.
    pub(super) fn form_verbal_turn_instruction(
        &self,
        maneuver: &Maneuver,
        max_count: u32,
    ) -> Result<String, OdinError> {
        let subset = self.turn_subset(maneuver, true)?;
        let format = Self::verbal(maneuver, max_count);
        let mut street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            format,
        )?;
        let mut begin_street_names = self.form_street_names(
            maneuver,
            &maneuver.begin_street_names,
            Some(&subset.empty_street_name_labels),
            false,
            format,
        )?;
        Self::update_obvious_maneuver_street_names(
            maneuver,
            &mut begin_street_names,
            &mut street_names,
        );

        let signs = self.verbal_signs(maneuver, max_count);
        let mut junction_name = String::new();
        let mut guide_sign = String::new();
        let phrase_id = if maneuver.signs.has_guide() {
            guide_sign = maneuver.signs.guide_string(signs)?;
            5
        } else if maneuver.signs.has_junction_name() {
            junction_name = maneuver.signs.junction_name_string(signs)?;
            4
        } else if maneuver.to_stay_on {
            3
        } else if !begin_street_names.is_empty() {
            2
        } else {
            u8::from(!street_names.is_empty())
        };

        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::RELATIVE_DIRECTION,
            relative_two_direction(maneuver, &subset.relative_directions)?,
        );
        replace(&mut instruction, tag::STREET_NAMES, &street_names);
        replace(
            &mut instruction,
            tag::BEGIN_STREET_NAMES,
            &begin_street_names,
        );
        replace(&mut instruction, tag::JUNCTION_NAME, &junction_name);
        replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
        Ok(self.finish(instruction))
    }

    pub(super) fn form_verbal_succinct_turn_transition_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = self.turn_subset(maneuver, true)?;
        let signs = self.verbal_signs(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT);
        let mut junction_name = String::new();
        let mut guide_sign = String::new();
        let phrase_id = if maneuver.signs.has_guide() {
            guide_sign = maneuver.signs.guide_string(signs)?;
            5
        } else if maneuver.signs.has_junction_name() {
            junction_name = maneuver.signs.junction_name_string(signs)?;
            4
        } else {
            0
        };
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::RELATIVE_DIRECTION,
            relative_two_direction(maneuver, &subset.relative_directions)?,
        );
        replace(&mut instruction, tag::JUNCTION_NAME, &junction_name);
        replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
        Ok(self.finish(instruction))
    }

    /// `Make a <RELATIVE_DIRECTION> U-turn`, `onto <STREET_NAMES>`, `to stay on <STREET_NAMES>`,
    /// `at <CROSS_STREET_NAMES>` (with the previous two), `at <JUNCTION_NAME>`,
    /// or `toward <TOWARD_SIGN>`.
    pub(super) fn form_uturn_instruction(&self, maneuver: &Maneuver) -> Result<String, OdinError> {
        let subset = &self.instructions().uturn;
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            ListFormat::TEXT,
        )?;
        let cross_street_names = self.form_street_names(
            maneuver,
            &maneuver.cross_street_names,
            None,
            false,
            ListFormat::TEXT,
        )?;
        let (phrase_id, junction_name, guide_sign) = Self::uturn_phrase(
            maneuver,
            &street_names,
            &cross_street_names,
            TEXT_SIGNS,
            true,
        )?;
        self.uturn_instruction(
            subset,
            maneuver,
            phrase_id,
            &street_names,
            &cross_street_names,
            &junction_name,
            &guide_sign,
        )
    }

    /// The U-turn phrase id, with the junction name and guide sign text.
    ///
    /// The pre-transition and textual forms add up the street name and cross street name cases;
    /// the alert form picks one, so it never says `at <CROSS_STREET_NAMES> onto <STREET_NAMES>`.
    fn uturn_phrase(
        maneuver: &Maneuver,
        street_names: &str,
        cross_street_names: &str,
        signs: SignFormat<'_>,
        additive: bool,
    ) -> Result<(u8, String, String), OdinError> {
        if maneuver.signs.has_guide() {
            return Ok((7, String::new(), maneuver.signs.guide_string(signs)?));
        }
        if maneuver.signs.has_junction_name() {
            return Ok((
                6,
                maneuver.signs.junction_name_string(signs)?,
                String::new(),
            ));
        }
        let mut phrase_id = 0;
        if !street_names.is_empty() {
            phrase_id = 1 + u8::from(maneuver.to_stay_on);
        }
        if !cross_street_names.is_empty() {
            phrase_id = if additive { phrase_id + 3 } else { 3 };
        }
        Ok((phrase_id, String::new(), String::new()))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors mainline's FormVerbalUturnInstruction overload"
    )]
    fn uturn_instruction(
        &self,
        subset: &Subset,
        maneuver: &Maneuver,
        phrase_id: u8,
        street_names: &str,
        cross_street_names: &str,
        junction_name: &str,
        guide_sign: &str,
    ) -> Result<String, OdinError> {
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::RELATIVE_DIRECTION,
            relative_two_direction(maneuver, &subset.relative_directions)?,
        );
        replace(&mut instruction, tag::STREET_NAMES, street_names);
        replace(
            &mut instruction,
            tag::CROSS_STREET_NAMES,
            cross_street_names,
        );
        replace(&mut instruction, tag::JUNCTION_NAME, junction_name);
        replace(&mut instruction, tag::TOWARD_SIGN, guide_sign);
        Ok(self.finish(instruction))
    }

    fn verbal_uturn(
        &self,
        maneuver: &Maneuver,
        max_count: u32,
        additive: bool,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().uturn_verbal;
        let format = Self::verbal(maneuver, max_count);
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            format,
        )?;
        let cross_street_names = self.form_street_names(
            maneuver,
            &maneuver.cross_street_names,
            Some(&subset.empty_street_name_labels),
            false,
            format,
        )?;
        let signs = self.verbal_signs(maneuver, max_count);
        let (phrase_id, junction_name, guide_sign) = Self::uturn_phrase(
            maneuver,
            &street_names,
            &cross_street_names,
            signs,
            additive,
        )?;
        self.uturn_instruction(
            subset,
            maneuver,
            phrase_id,
            &street_names,
            &cross_street_names,
            &junction_name,
            &guide_sign,
        )
    }

    pub(super) fn form_verbal_alert_uturn_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        self.verbal_uturn(maneuver, VERBAL_ALERT_ELEMENT_MAX_COUNT, false)
    }

    pub(super) fn form_verbal_uturn_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        self.verbal_uturn(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT, true)
    }

    pub(super) fn form_verbal_succinct_uturn_transition_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().uturn_verbal;
        let signs = self.verbal_signs(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT);
        let mut junction_name = String::new();
        let mut guide_sign = String::new();
        let phrase_id = if maneuver.signs.has_guide() {
            guide_sign = maneuver.signs.guide_string(signs)?;
            7
        } else if maneuver.signs.has_junction_name() {
            junction_name = maneuver.signs.junction_name_string(signs)?;
            6
        } else {
            0
        };
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::RELATIVE_DIRECTION,
            relative_two_direction(maneuver, &subset.relative_directions)?,
        );
        replace(&mut instruction, tag::JUNCTION_NAME, &junction_name);
        replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
        Ok(self.finish(instruction))
    }

    /// The exit branch, toward, and name signs of a ramp, with the phrase id increments
    /// for each (`+1` branch, `+2` toward, `+4` name).
    ///
    /// Additively (textual and pre-transition forms), the name only counts without a branch or
    /// toward sign; otherwise (alert forms) only the first present sign counts.
    fn ramp_signs(
        maneuver: &Maneuver,
        signs: SignFormat<'_>,
        additive: bool,
    ) -> Result<(u8, [String; 3]), OdinError> {
        let s = &maneuver.signs;
        let mut phrase_id = 0;
        let mut texts = [String::new(), String::new(), String::new()];
        if additive {
            if s.has_exit_branch() {
                phrase_id += 1;
                texts[0] = s.exit_branch_string(signs)?;
            }
            if s.has_exit_toward() {
                phrase_id += 2;
                texts[1] = s.exit_toward_string(signs)?;
            }
            if s.has_exit_name() && !s.has_exit_branch() && !s.has_exit_toward() {
                phrase_id += 4;
                texts[2] = s.exit_name_string(signs)?;
            }
        } else if s.has_exit_branch() {
            phrase_id += 1;
            texts[0] = s.exit_branch_string(signs)?;
        } else if s.has_exit_toward() {
            phrase_id += 2;
            texts[1] = s.exit_toward_string(signs)?;
        } else if s.has_exit_name() {
            phrase_id += 4;
            texts[2] = s.exit_name_string(signs)?;
        }
        Ok((phrase_id, texts))
    }

    fn ramp_straight(
        &self,
        subset: &Subset,
        phrase_id: u8,
        [branch, toward, name]: &[String; 3],
    ) -> Result<String, OdinError> {
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(&mut instruction, tag::BRANCH_SIGN, branch);
        replace(&mut instruction, tag::TOWARD_SIGN, toward);
        replace(&mut instruction, tag::NAME_SIGN, name);
        Ok(self.finish(instruction))
    }

    /// `Stay straight to take the ramp`, `the <BRANCH_SIGN> ramp`, `the ramp toward <TOWARD_SIGN>`,
    /// `the <BRANCH_SIGN> ramp toward <TOWARD_SIGN>`, or `the <NAME_SIGN> ramp`.
    pub(super) fn form_ramp_straight_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let (phrase_id, signs) = Self::ramp_signs(maneuver, TEXT_SIGNS, true)?;
        self.ramp_straight(&self.instructions().ramp_straight, phrase_id, &signs)
    }

    pub(super) fn form_verbal_alert_ramp_straight_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let format = self.verbal_signs(maneuver, VERBAL_ALERT_ELEMENT_MAX_COUNT);
        let (phrase_id, signs) = Self::ramp_signs(maneuver, format, false)?;
        self.ramp_straight(&self.instructions().ramp_straight_verbal, phrase_id, &signs)
    }

    pub(super) fn form_verbal_ramp_straight_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let format = self.verbal_signs(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT);
        let (phrase_id, signs) = Self::ramp_signs(maneuver, format, true)?;
        self.ramp_straight(&self.instructions().ramp_straight_verbal, phrase_id, &signs)
    }

    /// The base phrase of a ramp: 5 to turn onto it, 10 to take it on the driving side,
    /// and 0 to take it `on the left/right`.
    fn ramp_base_phrase(maneuver: &Maneuver) -> u8 {
        let direction = maneuver.begin_relative_direction;
        if matches!(
            direction,
            RelativeDirection::Right | RelativeDirection::Left
        ) {
            5
        } else if (direction == RelativeDirection::KeepRight && maneuver.drive_on_right)
            || (direction == RelativeDirection::KeepLeft && !maneuver.drive_on_right)
        {
            10
        } else {
            0
        }
    }

    fn ramp(
        &self,
        subset: &Subset,
        maneuver: &Maneuver,
        phrase_id: u8,
        [branch, toward, name]: &[String; 3],
    ) -> Result<String, OdinError> {
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::RELATIVE_DIRECTION,
            relative_two_direction(maneuver, &subset.relative_directions)?,
        );
        replace(&mut instruction, tag::BRANCH_SIGN, branch);
        replace(&mut instruction, tag::TOWARD_SIGN, toward);
        replace(&mut instruction, tag::NAME_SIGN, name);
        Ok(self.finish(instruction))
    }

    /// `Take the ramp on the <RELATIVE_DIRECTION>`, `Turn <RELATIVE_DIRECTION> to take the ramp`,
    /// or `Take the ramp`, each with the branch, toward, or name sign variants.
    pub(super) fn form_ramp_instruction(&self, maneuver: &Maneuver) -> Result<String, OdinError> {
        let subset = &self.instructions().ramp;
        let (sign_id, signs) = Self::ramp_signs(maneuver, TEXT_SIGNS, true)?;
        self.ramp(
            subset,
            maneuver,
            Self::ramp_base_phrase(maneuver) + sign_id,
            &signs,
        )
    }

    pub(super) fn form_verbal_ramp_instruction(
        &self,
        maneuver: &Maneuver,
        alert: bool,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().ramp_verbal;
        let max_count = if alert {
            VERBAL_ALERT_ELEMENT_MAX_COUNT
        } else {
            VERBAL_PRE_ELEMENT_MAX_COUNT
        };
        let format = self.verbal_signs(maneuver, max_count);
        let (sign_id, signs) = Self::ramp_signs(maneuver, format, !alert)?;
        self.ramp(
            subset,
            maneuver,
            Self::ramp_base_phrase(maneuver) + sign_id,
            &signs,
        )
    }

    /// The exit signs (`+1` number, `+2` branch, `+4` toward, `+8` name without a number),
    /// added up, or only the first present one for the alert form.
    fn exit_signs(
        &self,
        maneuver: &Maneuver,
        signs: Option<SignFormat<'_>>,
        alert: bool,
    ) -> Result<(u8, [String; 4]), OdinError> {
        let s = &maneuver.signs;
        let (number_format, format) = match signs {
            Some(format) => (self.verbal_exit_numbers(maneuver), format),
            None => (SignFormat::DEFAULT, TEXT_SIGNS),
        };
        let mut phrase_id = 0;
        let mut texts = [String::new(), String::new(), String::new(), String::new()];
        if alert {
            if s.has_exit_number() {
                phrase_id += 1;
                texts[0] = s.exit_number_string(number_format)?;
            } else if s.has_exit_branch() {
                phrase_id += 2;
                texts[1] = s.exit_branch_string(format)?;
            } else if s.has_exit_toward() {
                phrase_id += 4;
                texts[2] = s.exit_toward_string(format)?;
            } else if s.has_exit_name() {
                phrase_id += 8;
                texts[3] = s.exit_name_string(format)?;
            }
        } else {
            if s.has_exit_number() {
                phrase_id += 1;
                texts[0] = s.exit_number_string(number_format)?;
            }
            if s.has_exit_branch() {
                phrase_id += 2;
                texts[1] = s.exit_branch_string(format)?;
            }
            if s.has_exit_toward() {
                phrase_id += 4;
                texts[2] = s.exit_toward_string(format)?;
            }
            if s.has_exit_name() && !s.has_exit_number() {
                phrase_id += 8;
                texts[3] = s.exit_name_string(format)?;
            }
        }
        Ok((phrase_id, texts))
    }

    fn exit(
        &self,
        subset: &Subset,
        maneuver: &Maneuver,
        signs: Option<SignFormat<'_>>,
        alert: bool,
    ) -> Result<String, OdinError> {
        // Phrases 15 and up take the exit on the driving side, without naming the side.
        let driving_side = (maneuver.is_type(Type::KExitRight) && maneuver.drive_on_right)
            || (maneuver.is_type(Type::KExitLeft) && !maneuver.drive_on_right);
        let base = if driving_side { 15 } else { 0 };
        let relative_direction = relative_two_direction(maneuver, &subset.relative_directions)?;
        let (sign_id, [number, branch, toward, name]) = self.exit_signs(maneuver, signs, alert)?;
        let mut instruction = subset.phrase(base + sign_id)?.to_owned();
        replace(
            &mut instruction,
            tag::RELATIVE_DIRECTION,
            relative_direction,
        );
        replace(&mut instruction, tag::NUMBER_SIGN, &number);
        replace(&mut instruction, tag::BRANCH_SIGN, &branch);
        replace(&mut instruction, tag::TOWARD_SIGN, &toward);
        replace(&mut instruction, tag::NAME_SIGN, &name);
        Ok(self.finish(instruction))
    }

    /// `Take the exit on the <RELATIVE_DIRECTION>`, or `Take exit <NUMBER_SIGN>`
    /// `onto <BRANCH_SIGN>` `toward <TOWARD_SIGN>`, or `Take the <NAME_SIGN> exit`.
    pub(super) fn form_exit_instruction(&self, maneuver: &Maneuver) -> Result<String, OdinError> {
        self.exit(&self.instructions().exit, maneuver, None, false)
    }

    pub(super) fn form_verbal_exit_instruction(
        &self,
        maneuver: &Maneuver,
        alert: bool,
    ) -> Result<String, OdinError> {
        let max_count = if alert {
            VERBAL_ALERT_ELEMENT_MAX_COUNT
        } else {
            VERBAL_PRE_ELEMENT_MAX_COUNT
        };
        let signs = self.verbal_signs(maneuver, max_count);
        self.exit(
            &self.instructions().exit_verbal,
            maneuver,
            Some(signs),
            alert,
        )
    }

    /// The street names (or branch signs) and toward sign of a keep maneuver.
    fn keep_names(
        &self,
        maneuver: &Maneuver,
        labels: &[String],
        names: ListFormat<'_>,
        signs: SignFormat<'_>,
    ) -> Result<(String, String), OdinError> {
        let s = &maneuver.signs;
        let mut street_names = String::new();
        let mut toward_sign = String::new();
        if s.has_guide() {
            if s.has_guide_branch() {
                street_names = s.guide_branch_string(signs)?;
            }
            if s.has_guide_toward() {
                toward_sign = s.guide_toward_string(signs)?;
            }
        } else {
            // For ramps with branch signs, use the signs, which match what drivers see.
            if maneuver.ramp && s.has_exit_branch() {
                street_names = s.exit_branch_string(signs)?;
            } else {
                street_names = self.form_street_names(
                    maneuver,
                    &maneuver.street_names,
                    Some(labels),
                    true,
                    names,
                )?;
                if street_names.is_empty() && s.has_exit_branch() {
                    street_names = s.exit_branch_string(signs)?;
                }
            }
            if s.has_exit_toward() {
                toward_sign = s.exit_toward_string(signs)?;
            }
        }
        Ok((street_names, toward_sign))
    }

    fn keep(
        &self,
        subset: &Subset,
        maneuver: &Maneuver,
        phrase_id: u8,
        street_names: &str,
        number: &str,
        toward: &str,
    ) -> Result<String, OdinError> {
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::RELATIVE_DIRECTION,
            relative_three_direction(maneuver, &subset.relative_directions)?,
        );
        replace(&mut instruction, tag::NUMBER_SIGN, number);
        replace(&mut instruction, tag::STREET_NAMES, street_names);
        replace(&mut instruction, tag::TOWARD_SIGN, toward);
        Ok(self.finish(instruction))
    }

    /// `Keep <RELATIVE_DIRECTION> at the fork`, with `to take exit <NUMBER_SIGN>`,
    /// `to take <STREET_NAMES>`, and `toward <TOWARD_SIGN>` variants.
    pub(super) fn form_keep_instruction(&self, maneuver: &Maneuver) -> Result<String, OdinError> {
        let subset = &self.instructions().keep;
        let (street_names, toward_sign) = self.keep_names(
            maneuver,
            &subset.empty_street_name_labels,
            ListFormat::text(ELEMENT_MAX_COUNT),
            TEXT_SIGNS,
        )?;
        let mut phrase_id = 0;
        let mut number = String::new();
        if maneuver.signs.has_exit_number() {
            phrase_id += 1;
            number = maneuver.signs.exit_number_string(SignFormat::DEFAULT)?;
        }
        if !street_names.is_empty() {
            phrase_id += 2;
        }
        if !toward_sign.is_empty() {
            phrase_id += 4;
        }
        self.keep(
            subset,
            maneuver,
            phrase_id,
            &street_names,
            &number,
            &toward_sign,
        )
    }

    pub(super) fn form_verbal_keep_instruction(
        &self,
        maneuver: &Maneuver,
        alert: bool,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().keep_verbal;
        let max_count = if alert {
            VERBAL_ALERT_ELEMENT_MAX_COUNT
        } else {
            VERBAL_PRE_ELEMENT_MAX_COUNT
        };
        let (street_names, toward_sign) = self.keep_names(
            maneuver,
            &subset.empty_street_name_labels,
            Self::verbal(maneuver, max_count),
            self.verbal_signs(maneuver, max_count),
        )?;
        let mut phrase_id = 0;
        let mut number = String::new();
        if maneuver.signs.has_exit_number() {
            phrase_id += 1;
            number = maneuver
                .signs
                .exit_number_string(self.verbal_exit_numbers(maneuver))?;
        }
        if alert {
            // The alert only picks one.
            if phrase_id == 0 {
                if !street_names.is_empty() {
                    phrase_id += 2;
                } else if !toward_sign.is_empty() {
                    phrase_id += 4;
                }
            }
        } else {
            if !street_names.is_empty() {
                phrase_id += 2;
            }
            if !toward_sign.is_empty() {
                phrase_id += 4;
            }
        }
        self.keep(
            subset,
            maneuver,
            phrase_id,
            &street_names,
            &number,
            &toward_sign,
        )
    }

    fn keep_to_stay_on(
        &self,
        subset: &Subset,
        maneuver: &Maneuver,
        phrase_id: u8,
        street_names: &str,
        number: &str,
        toward: &str,
    ) -> Result<String, OdinError> {
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::RELATIVE_DIRECTION,
            relative_three_direction(maneuver, &subset.relative_directions)?,
        );
        replace(&mut instruction, tag::STREET_NAMES, street_names);
        replace(&mut instruction, tag::NUMBER_SIGN, number);
        replace(&mut instruction, tag::TOWARD_SIGN, toward);
        Ok(self.finish(instruction))
    }

    /// The toward sign of a keep to stay on maneuver: guide toward signs, else exit toward signs.
    fn keep_to_stay_on_toward(
        maneuver: &Maneuver,
        signs: SignFormat<'_>,
    ) -> Result<String, OdinError> {
        if maneuver.signs.has_guide_toward() {
            maneuver.signs.guide_toward_string(signs)
        } else if maneuver.signs.has_exit_toward() {
            maneuver.signs.exit_toward_string(signs)
        } else {
            Ok(String::new())
        }
    }

    /// `Keep <RELATIVE_DIRECTION> to stay on <STREET_NAMES>`, with exit number and toward variants.
    pub(super) fn form_keep_to_stay_on_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().keep_to_stay_on;
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            ListFormat::text(ELEMENT_MAX_COUNT),
        )?;
        let toward_sign = Self::keep_to_stay_on_toward(maneuver, TEXT_SIGNS)?;
        let mut phrase_id = 0;
        let mut number = String::new();
        if maneuver.signs.has_exit_number() {
            phrase_id += 1;
            number = maneuver.signs.exit_number_string(SignFormat::DEFAULT)?;
        }
        if !toward_sign.is_empty() {
            phrase_id += 2;
        }
        self.keep_to_stay_on(
            subset,
            maneuver,
            phrase_id,
            &street_names,
            &number,
            &toward_sign,
        )
    }

    pub(super) fn form_verbal_alert_keep_to_stay_on_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().keep_to_stay_on_verbal;
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            Self::verbal(maneuver, VERBAL_ALERT_ELEMENT_MAX_COUNT),
        )?;
        self.keep_to_stay_on(subset, maneuver, 0, &street_names, "", "")
    }

    pub(super) fn form_verbal_keep_to_stay_on_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().keep_to_stay_on_verbal;
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            Self::verbal(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT),
        )?;
        let toward_sign = Self::keep_to_stay_on_toward(
            maneuver,
            self.verbal_signs(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT),
        )?;
        let mut phrase_id = 0;
        let mut number = String::new();
        if maneuver.signs.has_exit_number() {
            phrase_id += 1;
            number = maneuver
                .signs
                .exit_number_string(self.verbal_exit_numbers(maneuver))?;
        }
        if !toward_sign.is_empty() {
            phrase_id += 2;
        }
        self.keep_to_stay_on(
            subset,
            maneuver,
            phrase_id,
            &street_names,
            &number,
            &toward_sign,
        )
    }

    fn merge(
        &self,
        subset: &Subset,
        maneuver: &Maneuver,
        street_names: &str,
        signs: SignFormat<'_>,
        use_names: bool,
    ) -> Result<String, OdinError> {
        let mut phrase_id = 0;
        let mut guide_sign = String::new();
        if use_names && !street_names.is_empty() {
            // Street names take priority over the toward phrase.
            phrase_id = 2;
        } else if maneuver.signs.has_guide() {
            phrase_id = 4;
            guide_sign = maneuver.signs.guide_string(signs)?;
        }
        let mut relative_direction = "";
        if maneuver.is_any_type(&[Type::KMergeLeft, Type::KMergeRight]) {
            phrase_id += 1;
            relative_direction = relative_two_direction(maneuver, &subset.relative_directions)?;
        }
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::RELATIVE_DIRECTION,
            relative_direction,
        );
        if use_names {
            replace(&mut instruction, tag::STREET_NAMES, street_names);
        }
        replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
        Ok(self.finish(instruction))
    }

    /// `Merge`, `Merge <RELATIVE_DIRECTION>`, `onto <STREET_NAMES>`, or `toward <TOWARD_SIGN>`.
    pub(super) fn form_merge_instruction(&self, maneuver: &Maneuver) -> Result<String, OdinError> {
        let subset = &self.instructions().merge;
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            ListFormat::TEXT,
        )?;
        self.merge(subset, maneuver, &street_names, TEXT_SIGNS, true)
    }

    pub(super) fn form_verbal_merge_instruction(
        &self,
        maneuver: &Maneuver,
        max_count: u32,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().merge_verbal;
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            Self::verbal(maneuver, max_count),
        )?;
        self.merge(
            subset,
            maneuver,
            &street_names,
            self.verbal_signs(maneuver, max_count),
            true,
        )
    }

    pub(super) fn form_verbal_succinct_merge_transition_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().merge_verbal;
        self.merge(
            subset,
            maneuver,
            "",
            self.verbal_signs(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT),
            false,
        )
    }

    /// The roundabout phrase id and the ordinal and guide sign text.
    ///
    /// 0 for an unnamed roundabout and 8 for a named one;
    /// `+1` with an ordinal exit count, else `+4` if the exit has names or guide signs;
    /// then `+3` for a guide sign, or `+1` for each of exit street names and exit begin street names.
    fn roundabout_phrase(
        maneuver: &Maneuver,
        named: bool,
        ordinal_values: &[String],
        exit_street_names: &str,
        exit_begin_street_names: &str,
        signs: SignFormat<'_>,
    ) -> Result<(u8, String, String), OdinError> {
        let mut phrase_id = if named { 8 } else { 0 };
        let exit_signs = &maneuver.roundabout_exit_signs;
        let mut ordinal_value = String::new();
        if ROUNDABOUT_EXIT_COUNT_BOUNDS.contains(&maneuver.roundabout_exit_count) {
            phrase_id += 1;
            label(
                ordinal_values,
                (maneuver.roundabout_exit_count - 1) as usize,
            )?
            .clone_into(&mut ordinal_value);
        } else if !exit_street_names.is_empty()
            || !exit_begin_street_names.is_empty()
            || exit_signs.has_guide()
        {
            phrase_id += 4;
        }
        let mut guide_sign = String::new();
        if exit_signs.has_guide() {
            phrase_id += 3;
            guide_sign = exit_signs.guide_string(signs)?;
        } else {
            phrase_id += u8::from(!exit_street_names.is_empty())
                + u8::from(!exit_begin_street_names.is_empty());
        }
        Ok((phrase_id, ordinal_value, guide_sign))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the roundabout phrases have many parts"
    )]
    fn roundabout(
        &self,
        subset: &Subset,
        phrase_id: u8,
        ordinal_value: &str,
        street_names: &str,
        guide_sign: &str,
        exit_street_names: &str,
        exit_begin_street_names: &str,
    ) -> Result<String, OdinError> {
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(&mut instruction, tag::ORDINAL_VALUE, ordinal_value);
        replace(&mut instruction, tag::STREET_NAMES, street_names);
        replace(&mut instruction, tag::TOWARD_SIGN, guide_sign);
        replace(
            &mut instruction,
            tag::ROUNDABOUT_EXIT_STREET_NAMES,
            exit_street_names,
        );
        replace(
            &mut instruction,
            tag::ROUNDABOUT_EXIT_BEGIN_STREET_NAMES,
            exit_begin_street_names,
        );
        Ok(self.finish(instruction))
    }

    /// `Enter the roundabout` or `Enter <STREET_NAMES>`, `and take the <ORDINAL_VALUE> exit`,
    /// `onto <ROUNDABOUT_EXIT_STREET_NAMES>`, or `toward <TOWARD_SIGN>`.
    pub(super) fn form_enter_roundabout_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().enter_roundabout;
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            None,
            false,
            ListFormat::TEXT,
        )?;
        // Mainline has a TODO to take this from the options; it is always true.
        // So the exit street names are the begin street names when there are any,
        // and the begin street names tag is always empty.
        let exit_names = if maneuver.roundabout_exit_begin_street_names.is_empty() {
            &maneuver.roundabout_exit_street_names
        } else {
            &maneuver.roundabout_exit_begin_street_names
        };
        let exit_street_names =
            self.form_street_names(maneuver, exit_names, None, false, ListFormat::TEXT)?;
        let (phrase_id, ordinal_value, guide_sign) = Self::roundabout_phrase(
            maneuver,
            !street_names.is_empty(),
            &subset.ordinal_values,
            &exit_street_names,
            "",
            TEXT_SIGNS,
        )?;
        self.roundabout(
            subset,
            phrase_id,
            &ordinal_value,
            &street_names,
            &guide_sign,
            &exit_street_names,
            "",
        )
    }

    pub(super) fn form_verbal_enter_roundabout_instruction(
        &self,
        maneuver: &Maneuver,
        max_count: u32,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().enter_roundabout_verbal;
        let format = Self::verbal(maneuver, max_count);
        let labels = Some(subset.empty_street_name_labels.as_slice());
        let street_names =
            self.form_street_names(maneuver, &maneuver.street_names, labels, false, format)?;
        // Empty names are only enhanced when entry and exit are combined,
        // which mainline's always-true `option_roundabout_exits` rules out.
        let exit_street_names = self.form_street_names(
            maneuver,
            &maneuver.roundabout_exit_street_names,
            labels,
            false,
            format,
        )?;
        let exit_begin_street_names = self.form_street_names(
            maneuver,
            &maneuver.roundabout_exit_begin_street_names,
            labels,
            false,
            format,
        )?;
        let (phrase_id, ordinal_value, guide_sign) = Self::roundabout_phrase(
            maneuver,
            !street_names.is_empty(),
            &subset.ordinal_values,
            &exit_street_names,
            &exit_begin_street_names,
            self.verbal_signs(maneuver, max_count),
        )?;
        self.roundabout(
            subset,
            phrase_id,
            &ordinal_value,
            &street_names,
            &guide_sign,
            &exit_street_names,
            &exit_begin_street_names,
        )
    }

    pub(super) fn form_verbal_succinct_enter_roundabout_transition_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().enter_roundabout_verbal;
        let exit_signs = &maneuver.roundabout_exit_signs;
        let mut phrase_id = 0;
        let mut ordinal_value = String::new();
        if ROUNDABOUT_EXIT_COUNT_BOUNDS.contains(&maneuver.roundabout_exit_count) {
            phrase_id += 1;
            label(
                &subset.ordinal_values,
                (maneuver.roundabout_exit_count - 1) as usize,
            )?
            .clone_into(&mut ordinal_value);
        }
        let mut guide_sign = String::new();
        if exit_signs.has_guide() {
            phrase_id += 3;
            guide_sign = exit_signs
                .guide_string(self.verbal_signs(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT))?;
        }
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(&mut instruction, tag::ORDINAL_VALUE, &ordinal_value);
        replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
        Ok(self.finish(instruction))
    }

    fn exit_roundabout(
        &self,
        subset: &Subset,
        maneuver: &Maneuver,
        format: ListFormat<'_>,
        signs: SignFormat<'_>,
    ) -> Result<String, OdinError> {
        let labels = Some(subset.empty_street_name_labels.as_slice());
        let mut street_names =
            self.form_street_names(maneuver, &maneuver.street_names, labels, true, format)?;
        let mut begin_street_names = self.form_street_names(
            maneuver,
            &maneuver.begin_street_names,
            labels,
            false,
            format,
        )?;
        Self::update_obvious_maneuver_street_names(
            maneuver,
            &mut begin_street_names,
            &mut street_names,
        );
        let mut guide_sign = String::new();
        let phrase_id = if maneuver.signs.has_guide() {
            guide_sign = maneuver.signs.guide_string(signs)?;
            3
        } else {
            u8::from(!street_names.is_empty()) + u8::from(!begin_street_names.is_empty())
        };
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(&mut instruction, tag::STREET_NAMES, &street_names);
        replace(
            &mut instruction,
            tag::BEGIN_STREET_NAMES,
            &begin_street_names,
        );
        replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
        Ok(self.finish(instruction))
    }

    /// `Exit the roundabout`, `onto <STREET_NAMES>`,
    /// `onto <BEGIN_STREET_NAMES>. Continue on <STREET_NAMES>.`, or `toward <TOWARD_SIGN>`.
    pub(super) fn form_exit_roundabout_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        self.exit_roundabout(
            &self.instructions().exit_roundabout,
            maneuver,
            ListFormat::TEXT,
            TEXT_SIGNS,
        )
    }

    pub(super) fn form_verbal_exit_roundabout_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        self.exit_roundabout(
            &self.instructions().exit_roundabout_verbal,
            maneuver,
            Self::verbal(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT),
            self.verbal_signs(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT),
        )
    }

    pub(super) fn form_verbal_succinct_exit_roundabout_transition_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().exit_roundabout_verbal;
        let mut guide_sign = String::new();
        let phrase_id = if maneuver.signs.has_guide() {
            guide_sign = maneuver
                .signs
                .guide_string(self.verbal_signs(maneuver, VERBAL_PRE_ELEMENT_MAX_COUNT))?;
            3
        } else {
            0
        };
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
        Ok(self.finish(instruction))
    }

    fn enter_ferry(
        &self,
        subset: &Subset,
        maneuver: &Maneuver,
        format: ListFormat<'_>,
        signs: SignFormat<'_>,
    ) -> Result<String, OdinError> {
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            format,
        )?;
        let ferry_label = &subset.ferry_label;
        let mut guide_sign = String::new();
        let phrase_id = if maneuver.signs.has_guide() {
            guide_sign = maneuver.signs.guide_string(signs)?;
            3
        } else if !street_names.is_empty() {
            // Add the ferry label unless the name already ends with it.
            if has_label(&street_names, ferry_label) {
                1
            } else {
                2
            }
        } else {
            0
        };
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(&mut instruction, tag::STREET_NAMES, &street_names);
        replace(&mut instruction, tag::FERRY_LABEL, ferry_label);
        replace(&mut instruction, tag::TOWARD_SIGN, &guide_sign);
        Ok(self.finish(instruction))
    }

    /// `Take the Ferry.`, `Take the <STREET_NAMES>.`, `Take the <STREET_NAMES> <FERRY_LABEL>.`,
    /// or `Take the ferry toward <TOWARD_SIGN>.`
    pub(super) fn form_enter_ferry_instruction(
        &self,
        maneuver: &Maneuver,
    ) -> Result<String, OdinError> {
        self.enter_ferry(
            &self.instructions().enter_ferry,
            maneuver,
            ListFormat::TEXT,
            TEXT_SIGNS,
        )
    }

    pub(super) fn form_verbal_enter_ferry_instruction(
        &self,
        maneuver: &Maneuver,
        max_count: u32,
    ) -> Result<String, OdinError> {
        self.enter_ferry(
            &self.instructions().enter_ferry_verbal,
            maneuver,
            Self::verbal(maneuver, max_count),
            self.verbal_signs(maneuver, max_count),
        )
    }

    /// `Continue for <LENGTH>.` or `Continue on <STREET_NAMES> for <LENGTH>.`
    pub(super) fn form_verbal_post_transition_instruction(
        &self,
        maneuver: &Maneuver,
        include_street_names: bool,
    ) -> Result<String, OdinError> {
        let subset = &self.instructions().post_transition_verbal;
        let mut street_names = String::new();
        // A maneuver that contains an obvious maneuver, or whose names are long, is not named.
        if !maneuver.contains_obvious_maneuver && !maneuver.has_long_street_name {
            let list: &StreetNames = if maneuver.has_combined_enter_exit_roundabout {
                &maneuver.roundabout_exit_street_names
            } else {
                &maneuver.street_names
            };
            street_names = self.form_street_names(
                maneuver,
                list,
                Some(&subset.empty_street_name_labels),
                true,
                Self::verbal(maneuver, VERBAL_POST_ELEMENT_MAX_COUNT),
            )?;
        }
        let phrase_id = u8::from(include_street_names && !street_names.is_empty());
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(
            &mut instruction,
            tag::LENGTH,
            &self.form_length(maneuver, subset)?,
        );
        replace(&mut instruction, tag::STREET_NAMES, &street_names);
        Ok(self.finish(instruction))
    }

    /// The elevator, steps, escalator, and level change instructions:
    /// `Take the elevator/stairs/escalator`, `to <LEVEL>`.
    ///
    /// Mainline does not form articulated prepositions for these.
    pub(super) fn form_level_instruction(
        maneuver: &Maneuver,
        subset: &Subset,
        level_phrase: bool,
    ) -> Result<String, OdinError> {
        let phrase_id = u8::from(level_phrase && !maneuver.end_level_ref.is_empty());
        Ok(subset
            .phrase(phrase_id)?
            .replace(tag::LEVEL, &maneuver.end_level_ref))
    }

    /// `Enter/Exit the building`, `and continue on <STREET_NAMES>`.
    ///
    /// Mainline does not form articulated prepositions for these.
    pub(super) fn form_building_instruction(
        &self,
        maneuver: &Maneuver,
        subset: &Subset,
    ) -> Result<String, OdinError> {
        let street_names = self.form_street_names(
            maneuver,
            &maneuver.street_names,
            Some(&subset.empty_street_name_labels),
            true,
            ListFormat::TEXT,
        )?;
        let phrase_id = u8::from(!street_names.is_empty());
        Ok(subset
            .phrase(phrase_id)?
            .replace(tag::STREET_NAMES, &street_names))
    }

    /// `Pass <OBJECT_LABEL>.` or `Pass traffic signals on <OBJECT_LABEL>.` (for blind users).
    pub(super) fn form_pass_instruction(&self, maneuver: &Maneuver) -> Result<String, OdinError> {
        let subset = &self.instructions().pass;
        let mut phrase_id = 0;
        let mut object_label = String::new();
        if let Some(node_type) = maneuver.node_type {
            let mut index = STREET_INTERSECTION_INDEX;
            if node_type == node::Type::KGate as i32 {
                index = GATE_INDEX;
            } else if node_type == node::Type::KBollard as i32 {
                index = BOLLARD_INDEX;
            } else if node_type == node::Type::KStreetIntersection as i32 {
                if maneuver.traffic_signal {
                    phrase_id = 1;
                }
                if maneuver.has_cross_street_names() {
                    object_label = self.form_street_names(
                        maneuver,
                        &maneuver.cross_street_names,
                        None,
                        false,
                        ListFormat::TEXT,
                    )?;
                }
            }
            if object_label.is_empty() {
                label(&subset.object_labels, index)?.clone_into(&mut object_label);
            }
        }
        let mut instruction = subset.phrase(phrase_id)?.to_owned();
        replace(&mut instruction, tag::OBJECT_LABEL, &object_label);
        Ok(self.finish(instruction))
    }
}

/// The index of a maneuver's cardinal direction in `cardinal_directions`.
fn direction_index(maneuver: &Maneuver) -> usize {
    // A negative value cannot index the list; `usize::MAX` reports it as out of range.
    usize::try_from(maneuver.begin_cardinal_direction).unwrap_or(usize::MAX)
}

/// The phrase id offset for the travel mode: `[drive, pedestrian, bicycle]`.
fn mode_offset(maneuver: &Maneuver, [drive, pedestrian, bicycle]: [u8; 3]) -> u8 {
    if maneuver.has_mode(TravelMode::KDrive) {
        drive
    } else if maneuver.has_mode(TravelMode::KPedestrian) {
        pedestrian
    } else if maneuver.has_mode(TravelMode::KBicycle) {
        bicycle
    } else {
        0
    }
}

/// `left` or `right` for a maneuver type (`FormRelativeTwoDirection`).
fn relative_two_direction<'d>(
    maneuver: &Maneuver,
    relative_directions: &'d [String],
) -> Result<&'d str, OdinError> {
    use Type as T;
    match Type::try_from(maneuver.maneuver_type) {
        Ok(
            T::KLeft
            | T::KSharpLeft
            | T::KSlightLeft
            | T::KUturnLeft
            | T::KRampLeft
            | T::KExitLeft
            | T::KMergeLeft
            | T::KDestinationLeft,
        ) => label(relative_directions, 0),
        Ok(
            T::KRight
            | T::KSharpRight
            | T::KSlightRight
            | T::KUturnRight
            | T::KRampRight
            | T::KExitRight
            | T::KMergeRight
            | T::KDestinationRight,
        ) => label(relative_directions, 1),
        _ => Err(OdinError::Coded(
            ErrorCode::InvalidManeuverTypeForRelativeTwo,
        )),
    }
}

/// `left`, `straight`, or `right` for a keep maneuver (`FormRelativeThreeDirection`).
fn relative_three_direction<'d>(
    maneuver: &Maneuver,
    relative_directions: &'d [String],
) -> Result<&'d str, OdinError> {
    match Type::try_from(maneuver.maneuver_type) {
        Ok(Type::KStayLeft) => label(relative_directions, 0),
        Ok(Type::KStayStraight) => label(relative_directions, 1),
        Ok(Type::KStayRight) => label(relative_directions, 2),
        _ => Err(OdinError::Coded(
            ErrorCode::InvalidManeuverTypeForRelativeThree,
        )),
    }
}

/// Whether a name ends with a label, ignoring ASCII case (`boost::iends_with` in the `C` locale).
fn has_label(name: &str, label: &str) -> bool {
    let (name, label) = (name.as_bytes(), label.as_bytes());
    name.len() >= label.len() && name[name.len() - label.len()..].eq_ignore_ascii_case(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_case_insensitively() {
        assert!(has_label("Bainbridge Island Ferry", "Ferry"));
        assert!(has_label("Bainbridge Island ferry", "Ferry"));
        assert!(!has_label("Ferry to Bainbridge", "Ferry"));
        assert!(has_label("Ferry", "Ferry"));
        assert!(!has_label("rry", "Ferry"));
        assert!(has_label("Transbordador", ""));
    }
}
