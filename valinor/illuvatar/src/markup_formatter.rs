//! Phoneme markup for street names and signs (`odin::MarkupFormatter`).
//!
//! When enabled, verbal instructions render a name that has a pronunciation
//! through a configurable template,
//! for example `<TEXTUAL_STRING> (<span class=<QUOTES>phoneme<QUOTES>>/<VERBAL_STRING>/</span>)`.
//! Mainline reads the template from `odin.markup_formatter` in its configuration,
//! and markup is disabled by default.

use crate::error::OdinError;
use valhalla_proto::{Pronunciation, pronunciation};

const QUOTES_TAG: &str = "<QUOTES>";
const PHONETIC_ALPHABET_TAG: &str = "<PHONETIC_ALPHABET>";
const TEXTUAL_STRING_TAG: &str = "<TEXTUAL_STRING>";
const VERBAL_STRING_TAG: &str = "<VERBAL_STRING>";

/// Mainline's default phoneme template (`valhalla_build_config`).
pub const DEFAULT_PHONEME_FORMAT: &str =
    "<TEXTUAL_STRING> (<span class=<QUOTES>phoneme<QUOTES>>/<VERBAL_STRING>/</span>)";

/// Phoneme markup configuration (`odin.markup_formatter`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkupFormatter {
    markup_enabled: bool,
    phoneme_format: String,
}

impl Default for MarkupFormatter {
    /// Markup disabled, which is mainline's default.
    ///
    /// With no `phoneme_format` configured, mainline's template is empty,
    /// so enabling markup without a template changes nothing.
    fn default() -> Self {
        Self {
            markup_enabled: false,
            phoneme_format: String::new(),
        }
    }
}

impl MarkupFormatter {
    /// A formatter with markup enabled or not, using the given phoneme template.
    pub fn new(markup_enabled: bool, phoneme_format: impl Into<String>) -> Self {
        Self {
            markup_enabled,
            phoneme_format: phoneme_format.into(),
        }
    }

    /// Whether markup is enabled.
    #[inline]
    pub fn markup_enabled(&self) -> bool {
        self.markup_enabled
    }

    /// The phoneme template.
    #[inline]
    pub fn phoneme_format(&self) -> &str {
        &self.phoneme_format
    }

    /// The marked-up form of a name with a pronunciation (`FormatPhonemeElement`),
    /// or `None` when markup is disabled, the name has no pronunciation,
    /// or the template renders empty.
    ///
    /// # Errors
    ///
    /// Fails, as mainline does, when the pronunciation's alphabet has no name
    /// (that is, `kNone` or an unknown value).
    pub(crate) fn format_phoneme(
        &self,
        text: &str,
        pronunciation: Option<&Pronunciation>,
    ) -> Result<Option<String>, OdinError> {
        let Some(pronunciation) = pronunciation.filter(|_| self.markup_enabled) else {
            return Ok(None);
        };
        let alphabet = pronunciation::Alphabet::try_from(pronunciation.alphabet).ok();
        let (alphabet_name, quotes) = match alphabet {
            Some(pronunciation::Alphabet::KIpa) => ("ipa", "\""),
            Some(pronunciation::Alphabet::KKatakana) => ("katakana", "\""),
            Some(pronunciation::Alphabet::KJeita) => ("jeita", "\""),
            Some(pronunciation::Alphabet::KNtSampa) => ("nt-sampa", "'"),
            Some(pronunciation::Alphabet::KNone) | None => {
                // Mainline replaces the quotes before it looks the alphabet name up,
                // but the lookup throws either way.
                return Err(OdinError::UnknownPronunciationAlphabet);
            }
        };
        let markup = self
            .phoneme_format
            .replace(QUOTES_TAG, quotes)
            .replace(PHONETIC_ALPHABET_TAG, alphabet_name)
            .replace(TEXTUAL_STRING_TAG, text)
            .replace(VERBAL_STRING_TAG, &pronunciation.value);
        Ok((!markup.is_empty()).then_some(markup))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipa(value: &str) -> Pronunciation {
        Pronunciation {
            alphabet: pronunciation::Alphabet::KIpa as i32,
            value: value.to_owned(),
        }
    }

    #[test]
    fn disabled_markup_does_nothing() {
        let formatter = MarkupFormatter::new(false, DEFAULT_PHONEME_FORMAT);
        assert_eq!(
            formatter
                .format_phoneme("Main", Some(&ipa("meɪn")))
                .unwrap(),
            None
        );
    }

    #[test]
    fn markup_uses_the_template() {
        let formatter = MarkupFormatter::new(true, DEFAULT_PHONEME_FORMAT);
        assert_eq!(
            formatter
                .format_phoneme("Main", Some(&ipa("meɪn")))
                .unwrap()
                .as_deref(),
            Some("Main (<span class=\"phoneme\">/meɪn/</span>)")
        );
        let sampa = Pronunciation {
            alphabet: pronunciation::Alphabet::KNtSampa as i32,
            value: "m\"eIn".to_owned(),
        };
        assert_eq!(
            formatter
                .format_phoneme("Main", Some(&sampa))
                .unwrap()
                .as_deref(),
            Some("Main (<span class='phoneme'>/m\"eIn/</span>)")
        );
        assert_eq!(formatter.format_phoneme("Main", None).unwrap(), None);
    }

    #[test]
    fn empty_templates_and_unknown_alphabets() {
        let formatter = MarkupFormatter::new(true, "");
        assert_eq!(
            formatter
                .format_phoneme("Main", Some(&ipa("meɪn")))
                .unwrap(),
            None
        );
        let none = Pronunciation {
            alphabet: 0,
            value: String::new(),
        };
        assert_eq!(
            formatter.format_phoneme("Main", Some(&none)),
            Err(OdinError::UnknownPronunciationAlphabet)
        );
    }
}
