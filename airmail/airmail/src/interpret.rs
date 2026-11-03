//! Readings of a query as a subject followed by the places it's in.
//!
//! `chelsea, new york` is Chelsea in New York, not a shop in Wakayama called
//! "CHELSEA New York", and `3122 16th st san francisco ca 94103` is an
//! address on 16th Street in San Francisco, whether or not its document has
//! that postcode. Search tries each reading alongside the query taken
//! literally and scores a result by the one it fits best; see
//! `AirmailIndex::search`.
//!
//! A reading splits the query's words into a subject, which is matched as a
//! query of its own, and a context after it: the names of administrative
//! areas the result must be in, postcodes, which count when they match, and
//! words found nowhere in the index, which are dropped. Which words name
//! areas is decided by the index, which has every Who's On First area, rather
//! than by the parser, whose dictionaries are narrower; the parser says which
//! words are postcodes and whether the query is an address.

use std::collections::{BTreeMap, BTreeSet};

/// A query's words, as the index tokenizes them, and where its comma-separated
/// sections begin.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryWords {
    pub words: Vec<String>,
    /// Whether word `i` begins a section after the first.
    pub section_starts: Vec<bool>,
}

impl QueryWords {
    /// Words from comma-separated sections, each already tokenized.
    #[must_use]
    pub fn from_sections(sections: Vec<Vec<String>>) -> Self {
        let mut words = Vec::new();
        let mut section_starts = Vec::new();
        for section in sections.into_iter().filter(|s| !s.is_empty()) {
            let first = !words.is_empty();
            for (i, word) in section.into_iter().enumerate() {
                section_starts.push(first && i == 0);
                words.push(word);
            }
        }
        Self { words, section_starts }
    }
}

/// What the parser says about a query, as words the index tokenizes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hints {
    /// Words that are (part of) a postcode.
    pub postcodes: BTreeSet<String>,
    /// Whether the query is an address: a house number and a street.
    pub address: bool,
    /// How likely each word is to be (part of) an administrative area's
    /// name, from 0 to 1.
    pub admin: BTreeMap<String, f64>,
}

/// How confident the parser must be that a query is an address.
const ADDRESS_CONFIDENCE: f64 = 0.6;

impl Hints {
    /// Parses `text`, tokenizing what the parser finds with `tokenize`.
    #[must_use]
    pub fn parse(text: &str, tokenize: impl Fn(&str) -> Vec<String>) -> Self {
        let parse = airmail_parser::parse(text);
        let mut hints = Self::default();
        for token in parse.token_weights() {
            let admin = token.weights.admin();
            for word in tokenize(&token.token.text) {
                let weight = hints.admin.entry(word).or_default();
                *weight = weight.max(admin);
            }
        }
        let Some(best) = parse.best() else {
            return hints;
        };
        let mut house_number = 0.0f64;
        let mut street = 0.0f64;
        for span in &best.spans {
            match span.kind {
                airmail_parser::Kind::Postcode => hints.postcodes.extend(tokenize(&span.text)),
                airmail_parser::Kind::HouseNumber => house_number = house_number.max(span.confidence),
                airmail_parser::Kind::Street => street = street.max(span.confidence),
                _ => {}
            }
        }
        hints.address = house_number.min(street) >= ADDRESS_CONFIDENCE;
        hints
    }
}

/// What the index knows about words, for finding a query's context.
pub trait Lookup {
    /// The importance of the most important administrative area named
    /// exactly `name` (words joined by spaces), abbreviations included, if
    /// there's one.
    fn admin_importance(&self, name: &str) -> Option<f64>;
    /// The ISO 3166-1 alpha-2 code of the country `code` names: an alpha-2
    /// or alpha-3 code, or an alias like `uk`.
    fn country_code(&self, code: &str) -> Option<String>;
    /// Whether any document has this word in any text field.
    fn is_known(&self, word: &str) -> bool;
}

/// Part of a query's context.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    /// Words naming an administrative area the result must be in: one with
    /// all the words among its admins' names, or the country they're the
    /// code of.
    Area { words: Vec<String>, country: Option<String> },
    /// A postcode word, which counts if it matches the result's address.
    Postcode(String),
    /// A word nothing in the index has, dropped.
    Unknown,
}

/// A reading of a query: its first `subject` words are what's searched for,
/// and the rest are `context`.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub subject: usize,
    pub context: Vec<Part>,
    /// How likely the reading is, relative to the query taken literally,
    /// which weighs 1.
    pub weight: f32,
    /// How likely the context's areas are meant as areas, from 0 to 1: 1
    /// after a comma, else from the parser and how well known the best known
    /// area of that name is. 0 without areas.
    pub area_confidence: f32,
}

impl Reading {
    /// Whether the context names an area.
    #[must_use]
    pub fn has_area(&self) -> bool {
        self.context.iter().any(|p| matches!(p, Part::Area { .. }))
    }
}

/// The most words an area's name in a query may have.
const MAX_AREA_WORDS: usize = 6;
/// The most readings tried.
const MAX_READINGS: usize = 4;
/// A reading's weight when its context starts a comma-separated section
/// (`chelsea, new york`), and when it doesn't (`seattle wa`).
const SECTION_READING_WEIGHT: f32 = 1.0;
const READING_WEIGHT: f32 = 0.9;
/// For a reading whose context includes the word still being typed, which
/// may be the beginning of something else.
const TYPING_CONTEXT_WEIGHT: f32 = 0.9;
/// Nearly every word names some area somewhere: without a comma, words are
/// only taken as an area this likely to be meant as one.
const MIN_AREA_CONFIDENCE: f32 = 0.3;
/// An area this important (a city of a few hundred thousand) is surely meant
/// as one, and one this unimportant could be anything.
const SURE_AREA_IMPORTANCE: f64 = 0.45;
const UNSURE_AREA_IMPORTANCE: f64 = 0.15;
/// Without a comma, how much an area's importance counts towards its
/// confidence when the parser doesn't know it as one: many important areas
/// are named with common words, like Cameroon's West region.
const IMPORTANCE_ONLY_FACTOR: f64 = 0.5;
/// Without a comma, an area that is or ends with the word still being typed
/// is this much less likely meant as one: `moscone west` may be about to be
/// Moscone Center West.
const TYPING_AREA_FACTOR: f32 = 0.5;
/// Country codes that are common words, which end queries like `bar near
/// me`: only after a comma or if the parser says they're places.
const WORD_CODES: &[&str] = &["am", "an", "and", "as", "at", "be", "by", "can", "do", "is", "it", "me", "my", "no", "per", "so", "to"];
/// The query taken literally weighs `1 - LITERAL_DISCOUNT * c`, where `c` is
/// the most confident reading's area confidence, or `1 -
/// SECTION_LITERAL_DISCOUNT` for an area after a comma, which says plainly
/// that what follows is where: an embassy named for its country doesn't beat
/// the capital for `sarajevo, bosnia and herzegovina`. The literal reading
/// still wins for a place whose name is the whole query, like the New York
/// New York hotel, when it matches it far better.
const LITERAL_DISCOUNT: f32 = 0.45;
const SECTION_LITERAL_DISCOUNT: f32 = 0.6;

/// The readings of `query` with a context, most context first. `prefix` says
/// the last word may be unfinished.
pub fn readings(query: &QueryWords, hints: &Hints, lookup: &impl Lookup, prefix: bool) -> Vec<Reading> {
    let words = &query.words;
    let n = words.len();
    if n < 2 {
        return Vec::new();
    }
    // `tails[s]`: how words `s..` divide into context parts, if they do,
    // with the confidence of their areas.
    let mut tails: Vec<Option<(Vec<Part>, f32)>> = vec![None; n + 1];
    tails[n] = Some((Vec::new(), 1.0));
    for s in (1..n).rev() {
        let mut found = None;
        // The longest area name first: `new york` before `york`.
        for e in (s + 1..=n.min(s + MAX_AREA_WORDS)).rev() {
            let Some((rest, rest_confidence)) = &tails[e] else { continue };
            let name = words[s..e].join(" ");
            // Codes only at the end, since many are words: `in`, `and`.
            let last = rest.iter().all(|p| !matches!(p, Part::Area { .. }));
            let country = if e == s + 1 && last { lookup.country_code(&name) } else { None };
            let importance = lookup.admin_importance(&name);
            if country.is_none() && importance.is_none() {
                continue;
            }
            let after_comma = (1..=s).any(|i| query.section_starts[i]);
            // Not a name split in two: `york` in `new york` isn't York,
            // unless there's a comma.
            if !query.section_starts[s]
                && lookup.admin_importance(&words[s - 1..e].join(" ")) >= importance
                && importance.is_some()
            {
                continue;
            }
            let confidence = if after_comma {
                1.0
            } else {
                let parser = words[s..e].iter().map(|w| hints.admin.get(w).copied().unwrap_or(0.0)).fold(1.0, f64::min);
                let known = ((importance.unwrap_or(0.0) - UNSURE_AREA_IMPORTANCE)
                    / (SURE_AREA_IMPORTANCE - UNSURE_AREA_IMPORTANCE))
                    .clamp(0.0, 1.0);
                // Nor while it's being typed: `daly ci` is Daly City, not
                // Daly in Côte d'Ivoire.
                let typing = prefix && e == n;
                let code = if country.is_none() || typing || WORD_CODES.contains(&name.as_str()) { 0.0 } else { 1.0 };
                let mut confidence = parser.max(IMPORTANCE_ONLY_FACTOR * known).max(code) as f32;
                if prefix && e == n {
                    confidence *= TYPING_AREA_FACTOR;
                }
                confidence
            };
            if confidence < MIN_AREA_CONFIDENCE {
                continue;
            }
            let mut parts = vec![Part::Area { words: words[s..e].to_vec(), country }];
            parts.extend(rest.iter().cloned());
            found = Some((parts, confidence.min(*rest_confidence)));
            break;
        }
        if found.is_none()
            && let Some((rest, rest_confidence)) = &tails[s + 1]
        {
            let word = &words[s];
            let typing = prefix && s + 1 == n;
            let part = if hints.postcodes.contains(word) || looks_like_postcode(word) {
                Some(Part::Postcode(word.clone()))
            } else if !typing && !lookup.is_known(word) {
                Some(Part::Unknown)
            } else {
                None
            };
            found = part.map(|part| {
                let mut parts = vec![part];
                parts.extend(rest.iter().cloned());
                (parts, *rest_confidence)
            });
        }
        tails[s] = found;
    }

    let mut readings: Vec<Reading> = (1..n)
        .filter_map(|s| {
            let (context, confidence) = tails[s].clone()?;
            let mut weight = if query.section_starts[s] { SECTION_READING_WEIGHT } else { READING_WEIGHT };
            if prefix && matches!(context.last(), Some(Part::Area { .. })) {
                weight *= TYPING_CONTEXT_WEIGHT;
            }
            let has_area = context.iter().any(|p| matches!(p, Part::Area { .. }));
            // Unknown words are only dropped around an area or postcode:
            // `seattle cof` isn't Seattle.
            if context.iter().all(|p| matches!(p, Part::Unknown)) {
                return None;
            }
            Some(Reading { subject: s, context, weight, area_confidence: if has_area { confidence } else { 0.0 } })
        })
        .collect();
    // More context first: a reading with less is a fallback for results not
    // in all of it.
    readings.truncate(MAX_READINGS);
    readings
}

/// How much the query taken literally counts, given its other readings.
#[must_use]
pub fn literal_weight(query: &QueryWords, readings: &[Reading]) -> f32 {
    readings
        .iter()
        .filter(|r| r.has_area())
        .map(|r| {
            if query.section_starts[r.subject..].iter().any(|s| *s) && r.area_confidence >= 1.0 {
                1.0 - SECTION_LITERAL_DISCOUNT
            } else {
                1.0 - LITERAL_DISCOUNT * r.area_confidence
            }
        })
        .fold(1.0, f32::min)
}

/// Whether a word looks like a postcode or part of one: four to six digits
/// (`10010`, `75008`), or letters and digits mixed as in British and
/// Canadian postcodes (`w1k`, `6ah`, `t2g`), but not a house number with a
/// letter (`14a`).
#[must_use]
pub fn looks_like_postcode(word: &str) -> bool {
    let digits = word.chars().filter(char::is_ascii_digit).count();
    let letters = word.chars().filter(char::is_ascii_alphabetic).count();
    if digits + letters != word.chars().count() || digits == 0 {
        return false;
    }
    if letters == 0 {
        return (4..=6).contains(&digits);
    }
    // `14a`, and ordinals: `16th`, `3rd`.
    let number_end = word.trim_start_matches(|c: char| c.is_ascii_digit());
    let house_number = number_end.len() < word.len()
        && (number_end.len() == 1 || matches!(number_end, "st" | "nd" | "rd" | "th"));
    (3..=4).contains(&word.len()) && !house_number
}

#[cfg(test)]
mod test {
    use super::*;

    struct FakeLookup {
        admins: &'static [(&'static str, f64)],
        countries: &'static [&'static str],
        unknown: &'static [&'static str],
    }

    impl Lookup for FakeLookup {
        fn admin_importance(&self, name: &str) -> Option<f64> {
            self.admins.iter().find(|(a, _)| *a == name).map(|(_, i)| *i)
        }
        fn country_code(&self, code: &str) -> Option<String> {
            self.countries.contains(&code).then(|| code.to_uppercase())
        }
        fn is_known(&self, word: &str) -> bool {
            !self.unknown.contains(&word)
        }
    }

    const LOOKUP: FakeLookup = FakeLookup {
        admins: &[
            ("new york", 0.6),
            ("york", 0.4),
            ("chelsea", 0.3),
            ("san francisco", 0.55),
            ("ca", 0.55),
            ("seattle", 0.5),
            ("market", 0.1),
            ("in", 0.1),
        ],
        countries: &["ca", "us", "in"],
        unknown: &["zzyzx"],
    };

    fn query(sections: &[&str]) -> QueryWords {
        QueryWords::from_sections(
            sections.iter().map(|s| s.split_whitespace().map(str::to_string).collect()).collect(),
        )
    }

    fn area(words: &str) -> Part {
        Part::Area { words: words.split_whitespace().map(str::to_string).collect(), country: None }
    }

    #[test]
    fn an_area_after_a_comma_is_context() {
        let q = query(&["chelsea", "new york"]);
        let readings = readings(&q, &Hints::default(), &LOOKUP, false);
        assert_eq!(
            readings[0],
            Reading { subject: 1, context: vec![area("new york")], weight: SECTION_READING_WEIGHT, area_confidence: 1.0 }
        );
        assert!((literal_weight(&q, &readings) - (1.0 - SECTION_LITERAL_DISCOUNT)).abs() < 1e-6);
    }

    #[test]
    fn the_longest_area_name_is_taken() {
        let q = query(&["chelsea new york"]);
        let readings = readings(&q, &Hints::default(), &LOOKUP, false);
        assert_eq!(readings.len(), 1, "{readings:?}");
        assert_eq!(readings[0].context, vec![area("new york")]);
        assert_eq!(readings[0].weight, READING_WEIGHT);
        assert_eq!(readings[0].area_confidence, IMPORTANCE_ONLY_FACTOR as f32);
    }

    #[test]
    fn obscure_areas_need_a_comma_or_the_parser() {
        assert!(readings(&query(&["pike place market"]), &Hints::default(), &LOOKUP, false).is_empty());
        assert_eq!(readings(&query(&["pike place", "market"]), &Hints::default(), &LOOKUP, false).len(), 1);
        let hints = Hints { admin: [("market".to_string(), 0.8)].into(), ..Hints::default() };
        assert_eq!(readings(&query(&["pike place market"]), &hints, &LOOKUP, false)[0].area_confidence, 0.8);
    }

    #[test]
    fn country_codes_postcodes_and_unknown_words_are_context() {
        let q = query(&["3122 16th st san francisco", "ca 94103 zzyzx"]);
        let readings = readings(&q, &Hints::default(), &LOOKUP, false);
        let ca = Part::Area { words: vec!["ca".into()], country: Some("CA".into()) };
        assert_eq!(readings[0].subject, 3);
        assert_eq!(readings[0].context, vec![area("san francisco"), ca.clone(), Part::Postcode("94103".into()), Part::Unknown]);
        assert_eq!(readings[1].subject, 5);
        assert_eq!(readings[1].context[0], ca);
        let postcode_only = readings.iter().find(|r| r.subject == 6).unwrap();
        assert_eq!(postcode_only.area_confidence, 0.0);
    }

    #[test]
    fn country_codes_only_end_a_query() {
        let readings = readings(&query(&["pizza in seattle"]), &Hints::default(), &LOOKUP, false);
        assert!(readings.iter().all(|r| r.context.iter().all(|p| !matches!(p, Part::Area { country: Some(_), .. }))), "{readings:?}");
    }

    #[test]
    fn no_reading_without_context() {
        assert!(readings(&query(&["seattle"]), &Hints::default(), &LOOKUP, false).is_empty());
        assert!(readings(&query(&["new york"]), &Hints::default(), &LOOKUP, false).iter().all(|r| r.subject > 0));
        assert_eq!(literal_weight(&query(&["seattle"]), &[]), 1.0);
    }

    #[test]
    fn an_area_being_typed_is_less_likely() {
        let hints = Hints { admin: [("west".to_string(), 0.4)].into(), ..Hints::default() };
        const WEST: FakeLookup = FakeLookup { admins: &[("west", 0.6)], countries: &[], unknown: &[] };
        assert!(readings(&query(&["moscone west"]), &hints, &WEST, true).is_empty());
        assert_eq!(readings(&query(&["moscone west"]), &hints, &WEST, false)[0].area_confidence, 0.5);
        assert_eq!(readings(&query(&["moscone", "west"]), &hints, &WEST, true)[0].area_confidence, 1.0);
    }

    #[test]
    fn country_codes_being_typed_need_a_comma() {
        let lookup = FakeLookup { admins: &[], countries: &["ci"], unknown: &[] };
        assert!(readings(&query(&["daly ci"]), &Hints::default(), &lookup, true).is_empty());
        assert_eq!(readings(&query(&["daly ci"]), &Hints::default(), &lookup, false)[0].area_confidence, 1.0);
        assert_eq!(readings(&query(&["daly", "ci"]), &Hints::default(), &lookup, true)[0].area_confidence, 1.0);
    }

    #[test]
    fn country_codes_that_are_words_need_a_comma() {
        assert!(readings(&query(&["bar near me"]), &Hints::default(), &FakeLookup { admins: &[], countries: &["me"], unknown: &[] }, false).is_empty());
        assert_eq!(readings(&query(&["seattle us"]), &Hints::default(), &LOOKUP, false)[0].area_confidence, 1.0);
    }

    #[test]
    fn unknown_words_are_only_dropped_around_other_context() {
        assert!(readings(&query(&["starbucks zzyzx"]), &Hints::default(), &LOOKUP, false).is_empty());
        assert_eq!(readings(&query(&["starbucks zzyzx seattle"]), &Hints::default(), &LOOKUP, false)[0].context, vec![Part::Unknown, area("seattle")]);
        // Nor the word being typed.
        assert!(readings(&query(&["starbucks seattle zzyzx"]), &Hints::default(), &LOOKUP, true).iter().all(|r| r.context.last() != Some(&Part::Unknown)));
    }

    #[test]
    fn postcodes() {
        for word in ["10010", "75008", "04709", "w1k", "6ah", "t2g", "4t8", "en5"] {
            assert!(looks_like_postcode(word), "{word}");
        }
        for word in ["14a", "110a", "1", "12", "1234567", "street", "16th", "3rd", "s"] {
            assert!(!looks_like_postcode(word), "{word}");
        }
    }
}
