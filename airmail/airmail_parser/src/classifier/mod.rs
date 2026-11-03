//! Classifiers (upstream `classifier/*.js`).
//!
//! Upstream has three base classes, mirrored by the helpers here:
//! **word** classifiers see each word of each section (with its index in the
//! section), **phrase** classifiers see every phrase, and **section**
//! classifiers see whole sections. Classifiers run in a fixed order (see
//! [`crate::parser`]) and later ones read the classifications of earlier
//! ones.

mod composite;
mod wof;

pub(crate) use composite::{CompositeClassifier, schemes};
pub(crate) use wof::WhosOnFirstClassifier;

use std::borrow::Cow;

use regex::Regex;

use crate::classification::{Classification, Kind};
use crate::js;
use crate::resources::{self, Dict, LANGUAGES, LoadOptions};
use crate::tokenization::{SpanId, Tokenizer};

/// A pass over a tokenized query that adds classifications.
pub(crate) trait Classifier: Send + Sync {
    fn classify(&self, t: &mut Tokenizer);
}

/// Calls `each(t, word, position_in_section)` for every word of every
/// section (upstream `WordClassifier`). Words added during the pass are
/// visited too, as upstream iterates the live array.
fn for_each_word(t: &mut Tokenizer, mut each: impl FnMut(&mut Tokenizer, SpanId, usize)) {
    for i in 0..t.sections.len() {
        let section = t.sections[i];
        let mut j = 0;
        while j < t.span(section).children.len() {
            let word = t.span(section).children[j];
            each(t, word, j);
            j += 1;
        }
    }
}

/// Calls `each(t, phrase)` for every phrase of every section (upstream
/// `PhraseClassifier`).
fn for_each_phrase(t: &mut Tokenizer, mut each: impl FnMut(&mut Tokenizer, SpanId)) {
    for i in 0..t.sections.len() {
        let section = t.sections[i];
        for j in 0..t.span(section).phrases.len() {
            let phrase = t.span(section).phrases[j];
            each(t, phrase);
        }
    }
}

/// Loads a libpostal dictionary for some languages.
fn libpostal(langs: &[&str], filename: &str, opts: LoadOptions) -> Dict {
    let mut d = Dict::new();
    resources::load_libpostal(&mut d, langs, filename, opts);
    d
}

fn has_key(d: &Dict, key: &str) -> bool {
    d.contains_key(key)
}

/// `numeric`, `alphanumeric`, `punctuation`, `alpha` and `single_alpha`
/// (upstream `AlphaNumericClassifier`).
pub(crate) struct AlphaNumericClassifier {
    punctuation: Regex,
}

impl AlphaNumericClassifier {
    pub fn new() -> Self {
        AlphaNumericClassifier {
            punctuation: Regex::new(r#"^[@&/\\#,+()$~%.!^'";:*?\[\]<>{}]+$"#).expect("regex"),
        }
    }
}

impl Classifier for AlphaNumericClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            let span = t.span_mut(id);
            if !span.norm.is_empty() && span.norm.bytes().all(|b| b.is_ascii_digit()) {
                span.classify(Classification::new(Kind::Numeric, 1.0));
            } else if span.numerals {
                span.classify(Classification::new(Kind::AlphaNumeric, 1.0));
            } else if self.punctuation.is_match(&span.norm) {
                span.classify(Classification::new(Kind::Punctuation, 1.0));
            } else {
                span.classify(Classification::new(Kind::Alpha, 1.0));
                if js::len(&span.norm) == 1 {
                    span.classify(Classification::new(Kind::SingleAlpha, 1.0));
                }
            }
        });
    }
}

/// `start_token` for the first word(s) of the first section, `end_token`
/// (and `end_token_single_character`) for the last word(s) of the last
/// section (upstream `TokenPositionClassifier`).
pub(crate) struct TokenPositionClassifier;

impl Classifier for TokenPositionClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        let (Some(&first), Some(&last)) = (t.sections.first(), t.sections.last()) else {
            return;
        };
        for c in t.span(first).children.clone() {
            if t.span(c).prev.is_empty() {
                t.span_mut(c)
                    .classify(Classification::new(Kind::StartToken, 1.0));
            }
        }
        for c in t.span(last).children.clone() {
            if t.span(c).next.is_empty() {
                let span = t.span_mut(c);
                span.classify(Classification::new(Kind::EndToken, 1.0));
                if js::len(&span.norm) == 1 {
                    span.classify(Classification::new(Kind::EndTokenSingleCharacter, 1.0));
                }
            }
        }
    }
}

/// House numbers such as `10`, `10a`, `10-19`, `1/135`, `6N23` or
/// `W350N5337` (upstream `HouseNumberClassifier`, patterns from Mapbox
/// carmen).
pub(crate) struct HouseNumberClassifier {
    patterns: Regex,
}

impl HouseNumberClassifier {
    pub fn new() -> Self {
        let letter = r"[a-zA-Z\x{0400}-\x{04FF}]";
        let patterns = [
            format!(r"^[0-9]{{1,5}}{letter}?$"),
            format!(r"^([0-9]{{1,5}})-([0-9]{{1,5}}){letter}?$"),
            format!(r"^([0-9]{{1,5}}){letter}?/([0-9]{{1,5}})$"),
            r"^([0-9]{1,5})([nsewNSEW])([0-9]{1,5})[a-zA-Z]?$".to_string(),
            r"^([nsewNSEW])([0-9]{1,5})([nsewNSEW][0-9]{1,5})?$".to_string(),
        ];
        HouseNumberClassifier {
            patterns: Regex::new(&patterns.join("|")).expect("regex"),
        }
    }
}

impl Classifier for HouseNumberClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            let span = t.span(id);
            if !span.numerals || !self.patterns.is_match(&span.body) {
                return;
            }
            // must not be preceded by a unit type
            if let Some(&prev) = span.prev.first()
                && t.span(prev).has(Kind::UnitType)
            {
                return;
            }
            // five-digit house numbers exist but are uncommon
            let leading_digits = span.norm.bytes().take_while(u8::is_ascii_digit).count();
            let confidence = match leading_digits {
                5.. => 0.2,
                4 => 0.9,
                _ => 1.0,
            };
            t.span_mut(id)
                .classify(Classification::new(Kind::HouseNumber, confidence));
        });
    }
}

/// Postcodes matching the chromium-i18n patterns of 15 countries (upstream
/// `PostcodeClassifier`).
pub(crate) struct PostcodeClassifier {
    patterns: Vec<PostcodePattern>,
}

/// A country's postcode pattern. Rust's `regex` has no lookahead, so the one
/// pattern that uses it (NL, `(?!SA|SD|SS)`) carries the excluded letter
/// pairs separately.
struct PostcodePattern {
    regex: Regex,
    /// For a pattern ending in a negative lookahead before `[A-Z]{2}`: the
    /// letter pairs it excludes (compared case-insensitively).
    excluded_suffixes: Vec<String>,
}

impl PostcodePattern {
    fn new(zip: &str) -> Self {
        let mut zip = zip.replace(r"\d", "[0-9]");
        let mut excluded_suffixes = Vec::new();
        if let Some(start) = zip.find("(?!") {
            let end = start + zip[start..].find(')').expect("closed lookahead");
            excluded_suffixes = zip[start + 3..end]
                .split('|')
                .map(str::to_lowercase)
                .collect();
            zip.replace_range(start..=end, "");
        }
        PostcodePattern {
            regex: Regex::new(&format!("(?i)^({zip})$")).expect("postcode regex"),
            excluded_suffixes,
        }
    }

    fn is_match(&self, s: &str) -> bool {
        self.regex.is_match(s)
            && !self.excluded_suffixes.iter().any(|x| {
                let lower = s.to_lowercase();
                lower.ends_with(x.as_str())
            })
    }
}

impl PostcodeClassifier {
    pub fn new() -> Self {
        const COUNTRIES: &[&str] = &[
            "US", "GB", "FR", "DE", "ES", "PT", "AU", "NZ", "KR", "JP", "IN", "RU", "BR", "NL",
            "PL",
        ];
        let patterns = COUNTRIES
            .iter()
            .filter_map(|cc| resources::chromium_zip_pattern(cc))
            .map(|zip| PostcodePattern::new(&zip))
            // drop countries with 3-digit postcodes
            .filter(|p| !p.is_match("100"))
            .collect();
        PostcodeClassifier { patterns }
    }
}

impl Classifier for PostcodeClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            let span = t.span(id);
            if !span.numerals {
                return;
            }
            // no postcode first, unless it's alone in its section
            if span.has(Kind::StartToken) && (!span.prev.is_empty() || !span.next.is_empty()) {
                return;
            }
            if self.patterns.iter().any(|p| p.is_match(&span.norm)) {
                t.span_mut(id)
                    .classify(Classification::new(Kind::Postcode, 1.0));
            }
        });
    }
}

/// Languages that put the street type before the name.
const PREFIX_LANGUAGES: &[&str] = &["fr", "ca", "es", "pt", "ro", "pl"];

/// Classifies words found in `index` (or found after dropping a final
/// period) with `kind`; shared by the street prefix and suffix classifiers.
fn classify_street_affix(t: &mut Tokenizer, id: SpanId, index: &Dict, kind: Kind) {
    let span = t.span(id);
    if has_key(index, &span.norm) {
        let confidence = if js::len(&span.norm) < 2 { 0.2 } else { 1.0 };
        t.span_mut(id)
            .classify(Classification::new(kind, confidence));
        return;
    }
    if span.final_period
        && span
            .norm
            .strip_suffix('.')
            .is_some_and(|s| has_key(index, s))
    {
        let confidence = if js::len(&span.norm) < 3 { 0.2 } else { 1.0 };
        t.span_mut(id)
            .classify(Classification::new(kind, confidence));
    }
}

/// Street types of languages that put them first, e.g. `rue`, `avenida`
/// (upstream `StreetPrefixClassifier`).
pub(crate) struct StreetPrefixClassifier {
    index: Dict,
}

impl StreetPrefixClassifier {
    pub fn new() -> Self {
        StreetPrefixClassifier {
            index: libpostal(PREFIX_LANGUAGES, "street_types.txt", LoadOptions::default()),
        }
    }
}

impl Classifier for StreetPrefixClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            if !t.span(id).numerals {
                classify_street_affix(t, id, &self.index, Kind::StreetPrefix);
            }
        });
    }
}

/// Street types of the other languages, e.g. `st`, `avenue`, `straße`, never
/// in first position (upstream `StreetSuffixClassifier`).
pub(crate) struct StreetSuffixClassifier {
    index: Dict,
}

impl StreetSuffixClassifier {
    pub fn new() -> Self {
        let langs: Vec<&str> = LANGUAGES
            .iter()
            .copied()
            .filter(|l| !PREFIX_LANGUAGES.contains(l))
            .collect();
        let mut index = libpostal(&langs, "street_types.txt", LoadOptions::default());
        index.retain(|token, _| js::len(token) >= 2);
        StreetSuffixClassifier { index }
    }
}

impl Classifier for StreetSuffixClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, position| {
            if !t.span(id).numerals && position > 0 {
                classify_street_affix(t, id, &self.index, Kind::StreetSuffix);
            }
        });
    }
}

/// Streets without a type such as `broadway` (upstream
/// `StreetProperNameClassifier`).
pub(crate) struct StreetProperNameClassifier;

impl Classifier for StreetProperNameClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            let span = t.span_mut(id);
            if !span.numerals && matches!(span.norm.as_str(), "broadway" | "esplanade") {
                span.classify(Classification::new(Kind::StreetProperName, 0.7));
            }
        });
    }
}

/// A word classifier that looks the word's normalized text up in a
/// dictionary, skipping words with digits; one-character words get
/// `short_confidence`.
pub(crate) struct DictionaryWordClassifier {
    kind: Kind,
    index: Dict,
    confidence: f64,
    short_confidence: f64,
    /// Attach the dictionary entry's languages as metadata (toponyms).
    with_langs: bool,
}

impl DictionaryWordClassifier {
    /// Upstream `RoadTypeClassifier`: `highway`, `county road`, … in any language.
    pub fn road_type() -> Self {
        DictionaryWordClassifier {
            kind: Kind::RoadType,
            index: libpostal(LANGUAGES, "road_types.txt", LoadOptions::default()),
            confidence: 1.0,
            short_confidence: 0.2,
            with_langs: false,
        }
    }

    /// Upstream `ToponymClassifier`: English toponyms (`county`, `island`, …).
    pub fn toponym() -> Self {
        DictionaryWordClassifier {
            kind: Kind::Toponym,
            index: libpostal(&["en"], "toponyms.txt", LoadOptions::default()),
            confidence: 1.0,
            short_confidence: 1.0,
            with_langs: true,
        }
    }

    /// Upstream `StopWordClassifier`: French, German, English and Portuguese
    /// stop words, with confidence 0.75.
    pub fn stop_word() -> Self {
        DictionaryWordClassifier {
            kind: Kind::StopWord,
            index: libpostal(
                &["fr", "de", "en", "pt"],
                "stopwords.txt",
                LoadOptions::default(),
            ),
            confidence: 0.75,
            short_confidence: 0.2,
            with_langs: false,
        }
    }

    /// Upstream `UnitTypeClassifier` (not in upstream's `AddressParser`):
    /// English unit types such as `apt`, `suite`.
    pub fn unit_type() -> Self {
        DictionaryWordClassifier {
            kind: Kind::UnitType,
            index: unit_types(),
            confidence: 1.0,
            short_confidence: 1.0,
            with_langs: false,
        }
    }
}

fn unit_types() -> Dict {
    libpostal(&["en"], "unit_types_numbered.txt", LoadOptions::default())
}

impl Classifier for DictionaryWordClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            let span = t.span(id);
            if span.numerals {
                return;
            }
            let Some(langs) = self.index.get(span.norm.as_str()) else {
                return;
            };
            let confidence = if js::len(&span.norm) < 2 {
                self.short_confidence
            } else {
                self.confidence
            };
            let classification = if self.with_langs {
                Classification::with_langs(
                    self.kind,
                    confidence,
                    langs.codes().into_iter().map(String::from).collect(),
                )
            } else {
                Classification::new(self.kind, confidence)
            };
            t.span_mut(id).classify(classification);
        });
    }
}

/// Germanic compound street names such as `Grolmanstraße` (upstream
/// `CompoundStreetClassifier`).
pub(crate) struct CompoundStreetClassifier {
    /// Suffixes as UTF-16, for upstream's code-unit offset arithmetic.
    suffixes: Vec<Vec<u16>>,
}

impl CompoundStreetClassifier {
    pub fn new() -> Self {
        let opts = LoadOptions {
            minlength: 3,
            ..Default::default()
        };
        let mut d = Dict::new();
        resources::load_libpostal(
            &mut d,
            &["de", "nl", "sv", "nb"],
            "concatenated_suffixes_separable.txt",
            opts,
        );
        resources::load_libpostal(
            &mut d,
            &["de", "nl", "nb"],
            "concatenated_suffixes_inseparable.txt",
            opts,
        );
        let mut suffixes: Vec<Vec<u16>> = d.keys().map(|k| k.encode_utf16().collect()).collect();
        suffixes.sort();
        CompoundStreetClassifier { suffixes }
    }
}

impl Classifier for CompoundStreetClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            let span = t.span(id);
            if span.numerals {
                return;
            }
            let body_len = js::len(&span.body);
            let norm: Vec<u16> = span.norm.encode_utf16().collect();
            let matched = self.suffixes.iter().any(|suffix| {
                // the word must be longer than the suffix (offset >= 1)
                body_len > suffix.len()
                    && norm.get(body_len - suffix.len()..) == Some(suffix.as_slice())
            });
            if matched {
                t.span_mut(id)
                    .classify(Classification::new(Kind::Street, 1.0));
            }
        });
    }
}

/// Compass directions such as `n`, `west`, `nord` (upstream
/// `DirectionalClassifier`).
pub(crate) struct DirectionalClassifier {
    index: Dict,
}

impl DirectionalClassifier {
    pub fn new() -> Self {
        DirectionalClassifier {
            index: libpostal(
                &["en", "es", "de", "fr", "nl", "nb"],
                "directionals.txt",
                LoadOptions::default(),
            ),
        }
    }
}

impl Classifier for DirectionalClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            let span = t.span(id);
            if span.numerals {
                return;
            }
            let matched = has_key(&self.index, &span.norm)
                || span
                    .norm
                    .strip_suffix('.')
                    .is_some_and(|s| has_key(&self.index, s));
            if matched {
                t.span_mut(id)
                    .classify(Classification::new(Kind::Directional, 1.0));
            }
        });
    }
}

/// English ordinals such as `1st`, `22nd`, `250th` (upstream
/// `OrdinalClassifier`).
///
/// The pattern reproduces upstream's regex exactly, including its operator
/// precedence: `^` binds only to the first alternative and `$` only to the
/// last, so e.g. any number containing `1` followed by a digit matches.
pub(crate) struct OrdinalClassifier {
    regex: Regex,
}

impl OrdinalClassifier {
    pub fn new() -> Self {
        let ord = concat!(
            r"((1)st?|(2)nd?|(3)rd?|([4-9])th?)",
            r"|",
            r"(0*([0-9]*)(1[0-9])th?)",
            r"|",
            r"(0*([0-9]*[02-9])((1)st?|(2)nd?|(3)rd?|([04-9])th?))"
        );
        OrdinalClassifier {
            regex: Regex::new(&format!("(?i)^{ord}$")).expect("regex"),
        }
    }
}

impl Classifier for OrdinalClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            let span = t.span(id);
            if span.numerals && self.regex.is_match(&span.norm) {
                t.span_mut(id)
                    .classify(Classification::new(Kind::Ordinal, 1.0));
            }
        });
    }
}

/// Intersection markers such as `&`, `and`, `@`, `an der ecke von`, which
/// need words on both sides (upstream `IntersectionClassifier`).
pub(crate) struct IntersectionClassifier;

impl Classifier for IntersectionClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_phrase(t, |t, id| {
            let span = t.span(id);
            if span.numerals {
                return;
            }
            // upstream checks both neighbours of the *first* word
            let first = t.span(span.child_first.unwrap_or(id));
            if first.prev.is_empty() || first.next.is_empty() {
                return;
            }
            if matches!(
                span.norm.as_str(),
                "&" | "and" | "und" | "@" | "at" | "con" | "an der ecke von"
            ) {
                let children = span.children.clone();
                t.span_mut(id)
                    .classify(Classification::new(Kind::Intersection, 1.0));
                for c in children {
                    t.span_mut(c)
                        .classify(Classification::new(Kind::Intersection, 1.0));
                }
            }
        });
    }
}

/// A phrase classifier that looks the phrase's normalized text up in a
/// dictionary.
pub(crate) struct DictionaryPhraseClassifier {
    kind: Kind,
    index: Dict,
    /// Skip phrases containing digits.
    skip_numerals: bool,
    /// Look up the text without a trailing period.
    strip_final_period: bool,
}

impl DictionaryPhraseClassifier {
    fn new(kind: Kind, index: Dict) -> Self {
        DictionaryPhraseClassifier {
            kind,
            index,
            skip_numerals: true,
            strip_final_period: false,
        }
    }

    /// Upstream `PersonClassifier`: well-known people (`martin luther king`).
    pub fn person() -> Self {
        let opts = LoadOptions {
            lowercase: true,
            ..Default::default()
        };
        Self::new(Kind::Person, libpostal(&["all", "fr"], "people.txt", opts))
    }

    /// Upstream `GivenNameClassifier`.
    pub fn given_name() -> Self {
        let opts = LoadOptions {
            lowercase: true,
            minlength: 3,
            ..Default::default()
        };
        Self::new(
            Kind::GivenName,
            libpostal(&["all"], "given_names.txt", opts),
        )
    }

    /// Upstream `SurnameClassifier`.
    pub fn surname() -> Self {
        let opts = LoadOptions {
            lowercase: true,
            minlength: 3,
            ..Default::default()
        };
        Self::new(Kind::Surname, libpostal(&["all"], "surnames.txt", opts))
    }

    /// Upstream `PersonalSuffixClassifier`: `jr`, `sr`, … in any language.
    pub fn personal_suffix() -> Self {
        let opts = LoadOptions {
            strip_final_period: true,
            ..Default::default()
        };
        DictionaryPhraseClassifier {
            strip_final_period: true,
            ..Self::new(
                Kind::PersonalSuffix,
                libpostal(LANGUAGES, "personal_suffixes.txt", opts),
            )
        }
    }

    /// Upstream `PersonalTitleClassifier`: `saint`, `general`, `dr`, … in
    /// any language.
    pub fn personal_title() -> Self {
        let opts = LoadOptions {
            strip_final_period: true,
            minlength: 2,
            ..Default::default()
        };
        DictionaryPhraseClassifier {
            strip_final_period: true,
            ..Self::new(
                Kind::PersonalTitle,
                libpostal(LANGUAGES, "personal_titles.txt", opts),
            )
        }
    }

    /// Upstream `ChainClassifier`: chain stores and restaurants. Its
    /// classification is private and unused by any scheme upstream.
    pub fn chain() -> Self {
        DictionaryPhraseClassifier {
            skip_numerals: false,
            ..Self::new(
                Kind::Chain,
                libpostal(&["all"], "chains.txt", LoadOptions::default()),
            )
        }
    }

    /// Upstream `SubdivisionClassifier`: French subdivisions such as `bis`.
    pub fn subdivision() -> Self {
        let opts = LoadOptions {
            lowercase: true,
            minlength: 3,
            ..Default::default()
        };
        Self::new(
            Kind::Subdivision,
            libpostal(&["fr"], "subdivisions.txt", opts),
        )
    }
}

impl Classifier for DictionaryPhraseClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_phrase(t, |t, id| {
            let span = t.span(id);
            if self.skip_numerals && span.numerals {
                return;
            }
            let key = if self.strip_final_period {
                span.norm.strip_suffix('.').unwrap_or(&span.norm)
            } else {
                &span.norm
            };
            if has_key(&self.index, key) {
                t.span_mut(id).classify(Classification::new(self.kind, 1.0));
            }
        });
    }
}

/// Single letters, optionally followed by a period (upstream
/// `MiddleInitialClassifier`).
pub(crate) struct MiddleInitialClassifier;

impl Classifier for MiddleInitialClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_phrase(t, |t, id| {
            let body = t.span(id).body.as_bytes();
            let is_initial = match body {
                [c] | [c, b'.'] => c.is_ascii_alphabetic(),
                _ => false,
            };
            if is_initial {
                t.span_mut(id)
                    .classify(Classification::new(Kind::MiddleInitial, 1.0));
            }
        });
    }
}

/// Place-type keywords such as `park`, `hospital`, `university` and their
/// English plurals (upstream `PlaceClassifier`).
pub(crate) struct PlaceClassifier {
    index: Dict,
}

impl PlaceClassifier {
    pub fn new() -> Self {
        let mut index = libpostal(
            &["fr", "de", "en", "pl"],
            "place_names.txt",
            LoadOptions::default(),
        );
        resources::generate_plurals(&mut index);
        PlaceClassifier { index }
    }
}

impl Classifier for PlaceClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            let span = t.span(id);
            if span.numerals {
                return;
            }
            let first = t.span(span.child_first.unwrap_or(id));
            if first
                .prev
                .first()
                .is_some_and(|&p| t.span(p).has(Kind::Intersection))
            {
                return;
            }
            if has_key(&self.index, &span.norm) {
                t.span_mut(id)
                    .classify(Classification::new(Kind::Place, 1.0));
            }
        });
    }
}

/// Guesses that the first word of a two-word section ending in a house
/// number is a street name, as in `Grolmanstraße 23` without a recognised
/// suffix (upstream `CentralEuropeanStreetNameClassifier`).
pub(crate) struct CentralEuropeanStreetNameClassifier;

impl Classifier for CentralEuropeanStreetNameClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for i in 0..t.sections.len() {
            let section = t.span(t.sections[i]);
            if section.children.len() < 2 {
                continue;
            }
            let first_id = section.children[0];
            let first = t.span(first_id);
            let Some(&next) = first.next.first() else {
                continue;
            };
            let next = t.span(next);
            if !next.next.is_empty() || !next.has(Kind::HouseNumber) {
                continue;
            }
            if first.classifications.iter().any(|c| c.kind.is_public()) {
                continue;
            }
            t.span_mut(first_id)
                .classify(Classification::new(Kind::Street, 0.5));
        }
    }
}

/// Unit numbers after a unit type (`apt 2b`) or starting with `#` (upstream
/// `UnitClassifier`; not in upstream's `AddressParser`).
pub(crate) struct UnitClassifier {
    regex: Regex,
}

impl UnitClassifier {
    pub fn new() -> Self {
        let parts = [
            r"^#?[0-9]+$",
            r"^#?[A-Za-z]$",
            r"^#?[0-9]+-?[A-Za-z]$",
            r"^#?[A-Za-z]-?[0-9]+$",
        ];
        UnitClassifier {
            regex: Regex::new(&format!("(?:{})", parts.join(")|(?:"))).expect("regex"),
        }
    }
}

impl Classifier for UnitClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_word(t, |t, id, _| {
            let span = t.span(id);
            let prev = span.prev.first().copied();
            let after_unit_type = prev.is_some_and(|p| t.span(p).has(Kind::UnitType));
            let matches = self.regex.is_match(&span.body);
            if (after_unit_type && matches)
                || (span.body.starts_with('#') && prev.is_some() && matches)
            {
                t.span_mut(id)
                    .classify(Classification::new(Kind::Unit, 1.0));
            }
        });
    }
}

/// Splits words such as `U12` into a unit type and a unit (upstream
/// `UnitTypeUnitClassifier`; not in upstream's `AddressParser`).
pub(crate) struct UnitTypeUnitClassifier {
    /// Unit types in sorted order. Upstream takes the first match in
    /// dictionary insertion order, which a hash map doesn't keep; this only
    /// matters when two unit types prefix the same word.
    types: Vec<Cow<'static, str>>,
}

impl UnitTypeUnitClassifier {
    pub fn new() -> Self {
        let mut types: Vec<_> = unit_types().into_keys().collect();
        types.sort();
        UnitTypeUnitClassifier { types }
    }
}

impl Classifier for UnitTypeUnitClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for i in 0..t.sections.len() {
            let section = t.sections[i];
            let mut j = 0;
            while j < t.span(section).children.len() {
                let id = t.span(section).children[j];
                j += 1;
                let span = t.span(id);
                if !span.numerals {
                    continue;
                }
                let found = self.types.iter().find(|ty| {
                    span.norm.len() >= ty.len()
                        && span.norm.starts_with(ty.as_ref())
                        && span.norm.len() > ty.len()
                        && span.norm[ty.len()..].bytes().all(|b| b.is_ascii_digit())
                });
                let Some(ty) = found else { continue };
                let split = ty.len().min(span.body.len());
                if !span.body.is_char_boundary(split) {
                    continue;
                }
                let (type_body, unit_body) = (
                    span.body[..split].to_string(),
                    span.body[split..].to_string(),
                );
                let (start, prev, next) = (span.start, span.prev.clone(), span.next.clone());
                let mut unit_type = crate::tokenization::Span::new(&type_body, start);
                unit_type.classify(Classification::new(Kind::UnitType, 1.0));
                unit_type.prev = prev;
                let unit_type_end = unit_type.end;
                let unit_type_id = t.push(unit_type);
                let mut unit = crate::tokenization::Span::new(&unit_body, unit_type_end);
                unit.classify(Classification::new(Kind::Unit, 1.0));
                unit.prev = vec![unit_type_id];
                unit.next = next;
                let unit_id = t.push(unit);
                t.span_mut(unit_type_id).next = vec![unit_id];
                let children = &mut t.span_mut(section).children;
                children.push(unit_type_id);
                children.push(unit_id);
            }
        }
    }
}

#[cfg(test)]
mod tests;
