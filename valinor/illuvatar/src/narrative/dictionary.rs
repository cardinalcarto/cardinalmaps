//! The narrative dictionaries, one per locale (`odin::NarrativeDictionary`).
//!
//! Mainline embeds the JSON files from its `locales` directory into the library;
//! we embed the same 3.9.1 files (see `locales/README.md`).
//! A dictionary is parsed the first time a request uses its locale.

use crate::error::OdinError;
use crate::narrative::number::NumberFormat;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The phrases and labels for one kind of instruction (mainline's `*Subset` structs).
///
/// Mainline has a struct per kind with just the lists that kind uses;
/// one struct with every list keeps this simpler,
/// and a test checks that each locale has the lists mainline requires.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Subset {
    phrases: HashMap<String, String>,
    #[serde(default)]
    pub(crate) cardinal_directions: Vec<String>,
    #[serde(default)]
    pub(crate) relative_directions: Vec<String>,
    #[serde(default)]
    pub(crate) empty_street_name_labels: Vec<String>,
    #[serde(default)]
    pub(crate) ordinal_values: Vec<String>,
    #[serde(default)]
    pub(crate) metric_lengths: Vec<String>,
    #[serde(default)]
    pub(crate) us_customary_lengths: Vec<String>,
    #[serde(default)]
    pub(crate) object_labels: Vec<String>,
    #[serde(default)]
    pub(crate) ferry_label: String,
}

impl Subset {
    /// The phrase with the given id (`phrases.at(std::to_string(id))`).
    pub(crate) fn phrase(&self, id: u8) -> Result<&str, OdinError> {
        self.phrases
            .get(id.to_string().as_str())
            .map(String::as_str)
            .ok_or(OdinError::MissingPhrase)
    }
}

/// A label from a list (`std::vector::at`).
pub(crate) fn label(labels: &[String], index: usize) -> Result<&str, OdinError> {
    labels
        .get(index)
        .map(String::as_str)
        .ok_or(OdinError::LabelOutOfRange {
            index,
            len: labels.len(),
        })
}

/// The instruction subsets of a locale that the narrative builder uses.
#[derive(Debug, Deserialize)]
pub(crate) struct Instructions {
    pub(crate) start: Subset,
    pub(crate) start_verbal: Subset,
    pub(crate) destination: Subset,
    pub(crate) destination_verbal_alert: Subset,
    pub(crate) destination_verbal: Subset,
    pub(crate) becomes: Subset,
    pub(crate) becomes_verbal: Subset,
    #[serde(rename = "continue")]
    pub(crate) continue_: Subset,
    pub(crate) continue_verbal_alert: Subset,
    pub(crate) continue_verbal: Subset,
    pub(crate) bear: Subset,
    pub(crate) bear_verbal: Subset,
    pub(crate) turn: Subset,
    pub(crate) turn_verbal: Subset,
    pub(crate) sharp: Subset,
    pub(crate) sharp_verbal: Subset,
    pub(crate) uturn: Subset,
    pub(crate) uturn_verbal: Subset,
    pub(crate) ramp_straight: Subset,
    pub(crate) ramp_straight_verbal: Subset,
    pub(crate) ramp: Subset,
    pub(crate) ramp_verbal: Subset,
    pub(crate) exit: Subset,
    pub(crate) exit_verbal: Subset,
    pub(crate) keep: Subset,
    pub(crate) keep_verbal: Subset,
    pub(crate) keep_to_stay_on: Subset,
    pub(crate) keep_to_stay_on_verbal: Subset,
    pub(crate) merge: Subset,
    pub(crate) merge_verbal: Subset,
    pub(crate) enter_roundabout: Subset,
    pub(crate) enter_roundabout_verbal: Subset,
    pub(crate) exit_roundabout: Subset,
    pub(crate) exit_roundabout_verbal: Subset,
    pub(crate) enter_ferry: Subset,
    pub(crate) enter_ferry_verbal: Subset,
    pub(crate) post_transition_verbal: Subset,
    pub(crate) verbal_multi_cue: Subset,
    pub(crate) approach_verbal_alert: Subset,
    pub(crate) pass: Subset,
    pub(crate) elevator: Subset,
    pub(crate) steps: Subset,
    pub(crate) escalator: Subset,
    pub(crate) level_change: Subset,
    pub(crate) enter_building: Subset,
    pub(crate) exit_building: Subset,
    pub(crate) park_vehicle: Subset,
}

/// The parts of a locale file we read.
#[derive(Debug, Deserialize)]
struct LocaleFile {
    instructions: Instructions,
    /// Mainline defaults to `en_US.UTF-8` when a file has none.
    #[serde(default = "default_posix_locale")]
    posix_locale: String,
}

fn default_posix_locale() -> String {
    "en_US.UTF-8".to_owned()
}

/// A locale's narrative phrases and its number formatting.
#[derive(Debug)]
pub(crate) struct NarrativeDictionary {
    pub(crate) language_tag: &'static str,
    pub(crate) instructions: Instructions,
    pub(crate) number_format: NumberFormat,
}

/// A locale shipped with mainline 3.9.1: its language tag, aliases, and JSON.
struct Locale {
    tag: &'static str,
    aliases: &'static [&'static str],
    json: &'static str,
}

macro_rules! locale {
    ($tag:literal, [$($alias:literal),*]) => {
        Locale {
            tag: $tag,
            aliases: &[$($alias),*],
            json: include_str!(concat!("../../locales/", $tag, ".json")),
        }
    };
}

/// Mainline's locales, with the aliases from their `aliases` keys
/// (a test checks these against the files).
static LOCALES: [Locale; 35] = [
    locale!("ar-SA", ["ar"]),
    locale!("bg-BG", ["bg"]),
    locale!("ca-ES", ["ca"]),
    locale!("cs-CZ", ["cs"]),
    locale!("da-DK", ["da"]),
    locale!("de-DE", ["de"]),
    locale!("el-GR", ["el"]),
    locale!("en-AU", ["au"]),
    locale!("en-GB", []),
    locale!("en-US-x-pirate", ["en-x-pirate"]),
    locale!("en-US", ["en"]),
    locale!("es-ES", ["es"]),
    locale!("et-EE", ["et"]),
    locale!("fi-FI", ["fi"]),
    locale!("fr-FR", ["fr"]),
    locale!("he-IL", ["he"]),
    locale!("hi-IN", ["hi"]),
    locale!("hu-HU", ["hu"]),
    locale!("it-IT", ["it"]),
    locale!("ja-JP", ["ja"]),
    locale!("ko-KR", ["ko"]),
    locale!("mn-MN", ["mn"]),
    locale!("nb-NO", ["nb"]),
    locale!("nl-NL", ["nl"]),
    locale!("pl-PL", ["pl"]),
    locale!("pt-BR", []),
    locale!("pt-PT", ["pt"]),
    locale!("ro-RO", ["ro"]),
    locale!("ru-RU", ["ru"]),
    locale!("sk-SK", ["sk"]),
    locale!("sl-SI", ["sl"]),
    locale!("sv-SE", ["sv"]),
    locale!("tr-TR", ["tr"]),
    locale!("uk-UA", ["uk"]),
    locale!("vi-VN", ["vi"]),
];

/// The parsed dictionaries, filled in lazily.
static DICTIONARIES: [OnceLock<Option<NarrativeDictionary>>; 35] = [const { OnceLock::new() }; 35];

/// Whether a language tag (or alias) names one of mainline's locales.
///
/// The lookup is exact, as in mainline: the front end normalizes the request's language first.
#[inline]
pub fn is_supported_language(language: &str) -> bool {
    locale_index(language).is_some()
}

fn locale_index(language: &str) -> Option<usize> {
    LOCALES
        .iter()
        .position(|locale| locale.tag == language || locale.aliases.contains(&language))
}

/// The dictionary for a language tag or alias (`get_locales().find(...)`).
///
/// # Errors
///
/// [`OdinError::InvalidLanguageTag`] if the language is unknown,
/// as `NarrativeBuilderFactory::Create` throws in mainline.
pub(crate) fn dictionary(language: &str) -> Result<&'static NarrativeDictionary, OdinError> {
    let index = locale_index(language).ok_or(OdinError::InvalidLanguageTag)?;
    let locale = &LOCALES[index];
    DICTIONARIES[index]
        .get_or_init(|| {
            // The files are embedded and every one is parsed by the tests,
            // so this only fails if the build embedded a broken file.
            let file: LocaleFile = serde_json::from_str(locale.json).ok()?;
            Some(NarrativeDictionary {
                language_tag: locale.tag,
                instructions: file.instructions,
                number_format: NumberFormat::for_posix_locale(&file.posix_locale),
            })
        })
        .as_ref()
        .ok_or(OdinError::CorruptLocale {
            language_tag: locale.tag,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The lists each subset must have, from mainline's `NarrativeDictionary::Load` overloads.
    const REQUIRED_LISTS: &[(&str, &[&str])] = &[
        (
            "start",
            &["cardinal_directions", "empty_street_name_labels"],
        ),
        (
            "start_verbal",
            &[
                "cardinal_directions",
                "empty_street_name_labels",
                "metric_lengths",
                "us_customary_lengths",
            ],
        ),
        ("destination", &["relative_directions"]),
        ("destination_verbal_alert", &["relative_directions"]),
        ("destination_verbal", &["relative_directions"]),
        ("continue", &["empty_street_name_labels"]),
        ("continue_verbal_alert", &["empty_street_name_labels"]),
        (
            "continue_verbal",
            &[
                "empty_street_name_labels",
                "metric_lengths",
                "us_customary_lengths",
            ],
        ),
        ("bear", &["relative_directions", "empty_street_name_labels"]),
        ("turn", &["relative_directions", "empty_street_name_labels"]),
        (
            "sharp",
            &["relative_directions", "empty_street_name_labels"],
        ),
        (
            "uturn",
            &["relative_directions", "empty_street_name_labels"],
        ),
        ("ramp", &["relative_directions"]),
        ("exit", &["relative_directions"]),
        ("keep", &["relative_directions", "empty_street_name_labels"]),
        (
            "keep_to_stay_on",
            &["relative_directions", "empty_street_name_labels"],
        ),
        (
            "merge",
            &["relative_directions", "empty_street_name_labels"],
        ),
        (
            "enter_roundabout",
            &["ordinal_values", "empty_street_name_labels"],
        ),
        ("exit_roundabout", &["empty_street_name_labels"]),
        ("enter_ferry", &["empty_street_name_labels", "ferry_label"]),
        (
            "post_transition_verbal",
            &[
                "metric_lengths",
                "us_customary_lengths",
                "empty_street_name_labels",
            ],
        ),
        (
            "verbal_multi_cue",
            &["metric_lengths", "us_customary_lengths"],
        ),
        (
            "approach_verbal_alert",
            &["metric_lengths", "us_customary_lengths"],
        ),
        ("pass", &["object_labels"]),
        ("enter_building", &["empty_street_name_labels"]),
        ("exit_building", &["empty_street_name_labels"]),
    ];

    #[test]
    fn every_locale_parses_with_the_lists_mainline_requires() {
        for (index, locale) in LOCALES.iter().enumerate() {
            let dictionary =
                dictionary(locale.tag).unwrap_or_else(|e| panic!("{}: {e}", locale.tag));
            assert_eq!(dictionary.language_tag, locale.tag);
            assert!(core::ptr::eq(
                dictionary,
                DICTIONARIES[index].get().unwrap().as_ref().unwrap()
            ));

            let json: Value = serde_json::from_str(locale.json).unwrap();
            for (subset, lists) in REQUIRED_LISTS {
                for list in *lists {
                    let names = [subset.to_string(), format!("{subset}_verbal")];
                    for name in names
                        .iter()
                        .filter(|name| json["instructions"].get(name.as_str()).is_some())
                    {
                        assert!(
                            json["instructions"][name.as_str()].get(*list).is_some(),
                            "{} lacks instructions.{name}.{list}",
                            locale.tag
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn aliases_match_the_locale_files() {
        for locale in &LOCALES {
            let json: Value = serde_json::from_str(locale.json).unwrap();
            let aliases: Vec<&str> = json["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect();
            assert_eq!(aliases, locale.aliases, "{}", locale.tag);
        }
    }

    #[test]
    fn locale_files_are_the_whole_directory() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/locales");
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter_map(|name| name.strip_suffix(".json").map(str::to_owned))
            .collect();
        files.sort();
        let mut tags: Vec<&str> = LOCALES.iter().map(|locale| locale.tag).collect();
        tags.sort_unstable();
        assert_eq!(files, tags);
    }

    #[test]
    fn lookups_are_exact() {
        assert!(is_supported_language("en-US"));
        assert!(is_supported_language("en"));
        assert!(is_supported_language("en-x-pirate"));
        assert!(!is_supported_language("en_US"));
        assert!(!is_supported_language("EN-us"));
        assert!(!is_supported_language(""));
        assert_eq!(
            dictionary("xx").map(|d| d.language_tag),
            Err(OdinError::InvalidLanguageTag)
        );
        assert_eq!(dictionary("ca").map(|d| d.language_tag), Ok("ca-ES"));
    }

    #[test]
    fn missing_phrases_and_labels() {
        let subset = &dictionary("en-US").unwrap().instructions.start;
        assert_eq!(subset.phrase(0), Ok("Head <CARDINAL_DIRECTION>."));
        assert_eq!(subset.phrase(3), Err(OdinError::MissingPhrase));
        assert_eq!(label(&subset.cardinal_directions, 2), Ok("east"));
        assert_eq!(
            label(&subset.cardinal_directions, 8),
            Err(OdinError::LabelOutOfRange { index: 8, len: 8 })
        );
    }
}
