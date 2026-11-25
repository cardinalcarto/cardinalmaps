//! The features of a query's words, for the CRF.
//!
//! libpostal's templates (`address_parser.c`, see its `address_parser_features`)
//! are the base: the word or `unk`, its shape, character n-grams of rare
//! words, the previous and next words, dictionary phrases, joined street-type
//! suffixes, and `word` and `prev word` as state-transition features. Added
//! here:
//!
//! - **Gazetteer areas**, from the index ([`Gazetteer`]): each match of
//!   words to an area's name gives each word its band, its position in the
//!   match (begin, inside, end, or single), how well it matched (exactly, as
//!   the beginning of the word still being typed, or with typos), how well
//!   known the best-known area of that name is, and how far the nearest is
//!   from the focus. libpostal's gazetteer is learned from its training
//!   labels and matches exactly; this one knows every area, and as typed.
//! - **Position in the query**: first and last word, the word being typed,
//!   and comma-separated sections, counted from the end, since areas come
//!   last.
//! - **Dictionary phrase positions**, begin, inside, end or single, where
//!   libpostal gives every word of a phrase the same features.
//!
//! Also as libpostal has them: an exact match of an area's name is one unit,
//! its words having the whole name as their value (`phrase|new york`) in
//! place of their own word and n-gram features, with neighbours outside it;
//! a name in more than one band has the band it's `commonly` (its best-known
//! area's, where libpostal counts its training labels); and an unknown first
//! word has what's to its right: whether a number and a street type follow,
//! and in which order.

use std::collections::HashMap;

use crate::crf::TokenFeatures;
use crate::lexicon::{Kind, Lexicon, Phrase};

/// How words matched an area's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quality {
    Exact,
    /// The last word is the beginning of the name's last word, still being
    /// typed.
    Prefix,
    /// With one edit.
    Typo,
    /// With two edits.
    Typos,
}

impl Quality {
    fn as_str(self) -> &'static str {
        match self {
            Quality::Exact => "exact",
            Quality::Prefix => "prefix",
            Quality::Typo => "typo",
            Quality::Typos => "typos",
        }
    }
}

/// Words `start..end` match the name of an administrative area.
#[derive(Debug, Clone, PartialEq)]
pub struct AreaMatch {
    pub start: usize,
    pub end: usize,
    /// `neighbourhood`, `locality`, `county`, `region` or `country`.
    pub band: &'static str,
    pub quality: Quality,
    /// Of the best-known area of that name in the band, from 0 to 1.
    pub importance: f32,
    /// To the nearest such area from the focus, if there's a focus.
    pub distance_km: Option<f32>,
    /// The areas named (exactly) around it that one of its areas is in, by
    /// their words' span and band: `chelsea` in `chelsea new york` is in
    /// the locality at words 1..3.
    pub inside: Vec<(usize, usize, &'static str)>,
}

/// Finds a query's words in a gazetteer of administrative areas.
pub trait Gazetteer {
    /// Every span of words matching an area's name, in each band, the best
    /// match of each span and band only.
    fn areas(&self, query: &Query) -> Vec<AreaMatch>;
}

/// No gazetteer: for tests, and for training without one.
pub struct NoGazetteer;

impl Gazetteer for NoGazetteer {
    fn areas(&self, _: &Query) -> Vec<AreaMatch> {
        Vec::new()
    }
}

/// A query's words, folded (see `text`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub words: Vec<String>,
    /// Whether word `i` begins a comma-separated section after the first.
    pub section_starts: Vec<bool>,
    /// Whether the last word may be unfinished.
    pub typing: bool,
    /// `[lon, lat]` the query was made looking at.
    pub focus: Option<[f64; 2]>,
}

/// How often words were seen in training, for telling rare words from
/// common ones.
#[derive(Debug, Clone, Default)]
pub struct Vocabulary {
    counts: HashMap<String, u32>,
}

/// Words seen this often or less get character n-gram features, as in
/// libpostal.
pub const RARE_WORD_COUNT: u32 = 50;

/// Words seen less often than this are `unk`.
pub const MIN_WORD_COUNT: u32 = 5;

impl Vocabulary {
    pub fn add(&mut self, word: &str) {
        *self.counts.entry(word.to_string()).or_default() += 1;
    }

    #[must_use]
    pub fn count(&self, word: &str) -> u32 {
        self.counts.get(word).copied().unwrap_or(0)
    }

    /// The words seen at least `MIN_WORD_COUNT` times, and their counts.
    pub fn known(&self) -> impl Iterator<Item = (&str, u32)> {
        self.counts
            .iter()
            .filter(|(_, c)| **c >= MIN_WORD_COUNT)
            .map(|(w, c)| (w.as_str(), *c))
    }

    /// Adds another's counts to its own.
    pub fn merge(&mut self, other: Vocabulary) {
        for (word, count) in other.counts {
            *self.counts.entry(word).or_default() += count;
        }
    }

    #[must_use]
    pub fn from_counts(counts: HashMap<String, u32>) -> Self {
        Self { counts }
    }
}

/// What every word's features are computed from.
struct Context<'q> {
    query: &'q Query,
    areas: &'q [AreaMatch],
    /// The place name each word is in, if any (see `units`).
    units: &'q [Option<(usize, usize)>],
    /// The band each matching span of words is most commonly, where it's in
    /// more than one (see `commonly`).
    commonly: &'q [((usize, usize), &'static str)],
}

/// The place name each of `n` words is in: the longest exact match of an
/// area's name covering it, the first of the longest. Its words are one unit
/// to the features, as libpostal has its known place names.
fn units(areas: &[AreaMatch], n: usize) -> Vec<Option<(usize, usize)>> {
    let mut units: Vec<Option<(usize, usize)>> = vec![None; n];
    for area in areas
        .iter()
        .filter(|a| a.quality == Quality::Exact && a.band != NAME_BAND && a.band != POSTCODE_BAND)
    {
        for unit in &mut units[area.start..area.end] {
            let longer = unit.is_none_or(|(start, end)| {
                area.end - area.start > end - start
                    || (area.end - area.start == end - start && area.start < start)
            });
            if longer {
                *unit = Some((area.start, area.end));
            }
        }
    }
    units
}

/// For each span of words matching areas' names in more than one band, the
/// band of its best-known match.
fn commonly(areas: &[AreaMatch]) -> Vec<((usize, usize), &'static str)> {
    let mut best: HashMap<(usize, usize), (f32, &'static str, usize)> = HashMap::new();
    for area in areas
        .iter()
        .filter(|a| a.band != NAME_BAND && a.band != POSTCODE_BAND)
    {
        let entry = best
            .entry((area.start, area.end))
            .or_insert((area.importance, area.band, 0));
        entry.2 += 1;
        if area.importance > entry.0 {
            (entry.0, entry.1) = (area.importance, area.band);
        }
    }
    let mut commonly: Vec<_> = best
        .into_iter()
        .filter(|(_, (_, _, bands))| *bands > 1)
        .map(|(span, (_, band, _))| (span, band))
        .collect();
    commonly.sort();
    commonly
}

/// The band of an index's exact-name matches (see the gazetteer), which says
/// how many places of any kind have the name rather than naming an area.
pub const NAME_BAND: &str = "name";

/// The band of postcode matches, their importance one of the three below.
pub const POSTCODE_BAND: &str = "postcode";
/// An area named next to the postcode has a postcode of that code.
pub const POSTCODE_IN_AREA_BESIDE: f32 = 1.0;
/// No area is named next to it.
pub const POSTCODE_ALONE: f32 = 0.5;
/// Areas are named next to it, none with a postcode of that code: likely a
/// house number.
pub const POSTCODE_NOT_IN_AREA_BESIDE: f32 = 0.0;

/// What features are computed from.
pub struct Extractor<'a> {
    pub lexicon: &'a Lexicon,
    pub vocabulary: &'a Vocabulary,
}

/// The begin/inside/end/single position of word `t` in `start..end`.
fn position(t: usize, start: usize, end: usize) -> &'static str {
    match (t == start, t + 1 == end) {
        (true, true) => "S",
        (true, false) => "B",
        (false, true) => "E",
        (false, false) => "I",
    }
}

fn importance_bucket(importance: f32) -> &'static str {
    match importance {
        i if i < 0.1 => "lo",
        i if i < 0.25 => "mid",
        i if i < 0.45 => "hi",
        _ => "top",
    }
}

fn distance_bucket(km: f32) -> &'static str {
    match km {
        d if d < 30.0 => "near",
        d if d < 300.0 => "regional",
        _ => "far",
    }
}

/// A word's shape: runs of digits as `D` and of letters as `a`, so `10010`
/// and `98101` are both `D`, `14a` is `Da` and `m2m` is `aDa`, with how many
/// digits it has.
fn shape(word: &str) -> String {
    let mut shape = String::new();
    for c in word.chars() {
        let class = if c.is_ascii_digit() || c.is_numeric() {
            'D'
        } else if c.is_alphabetic() {
            'a'
        } else {
            'x'
        };
        if !shape.ends_with(class) {
            shape.push(class);
        }
    }
    let digits = word.chars().filter(|c| c.is_numeric()).count();
    if digits > 0 {
        format!("{shape}{}", digits.min(7))
    } else {
        shape
    }
}

/// Character n-grams of a word, as libpostal makes them: `_` marks a gram
/// that doesn't begin or end the word.
fn ngrams(word: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    if chars.len() <= n {
        return Vec::new();
    }
    let count = chars.len() - n + 1;
    (0..count)
        .map(|i| {
            let gram: String = chars[i..i + n].iter().collect();
            let before = if i > 0 { "_" } else { "" };
            let after = if i + 1 < count { "_" } else { "" };
            format!("{before}{gram}{after}")
        })
        .collect()
}

impl Extractor<'_> {
    /// The word as a feature value: itself if known, else `unk` or
    /// `unk_num`.
    fn word<'w>(&self, word: &'w str) -> &'w str {
        if self.vocabulary.count(word) >= MIN_WORD_COUNT {
            word
        } else if word.chars().any(char::is_numeric) {
            "unk_num"
        } else {
            "unk"
        }
    }

    /// Every token's features.
    #[must_use]
    pub fn features(&self, query: &Query, areas: &[AreaMatch]) -> Vec<TokenFeatures> {
        let n = query.words.len();
        let phrases = self.lexicon.phrases(&query.words);
        let phrase_at = |t: usize| phrases.iter().find(|p| p.start <= t && t < p.end);
        let sections_after = |t: usize| {
            query
                .section_starts
                .iter()
                .skip(t + 1)
                .filter(|s| **s)
                .count()
        };
        let units = units(areas, n);
        let commonly = commonly(areas);
        let context = Context {
            query,
            areas,
            units: &units,
            commonly: &commonly,
        };
        (0..n)
            .map(|t| self.token(&context, t, phrase_at(t), phrase_at, &sections_after))
            .collect()
    }

    /// A word's value as a neighbour: its place name if it's in one, else
    /// its dictionary phrase's kinds, else itself or `unk`.
    fn neighbour<'p>(
        &self,
        context: &Context,
        i: usize,
        phrase_at: &impl Fn(usize) -> Option<&'p Phrase>,
    ) -> String {
        if let Some((start, end)) = context.units[i] {
            return context.query.words[start..end].join(" ");
        }
        match phrase_at(i) {
            Some(phrase) => format!(
                "<{}>",
                phrase
                    .kinds
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join("+")
            ),
            None => self.word(&context.query.words[i]).to_string(),
        }
    }

    fn token<'p>(
        &self,
        context: &Context,
        t: usize,
        phrase: Option<&'p Phrase>,
        phrase_at: impl Fn(usize) -> Option<&'p Phrase>,
        sections_after: &impl Fn(usize) -> usize,
    ) -> TokenFeatures {
        let (query, areas) = (context.query, context.areas);
        let n = query.words.len();
        let word = query.words[t].as_str();
        // In a place's name, as libpostal has its known phrases: the name is
        // the word's value, in place of the word's own features.
        let unit = context.units[t];
        let value = match unit {
            Some((start, end)) => query.words[start..end].join(" "),
            None => self.word(word).to_string(),
        };
        let mut state = vec!["bias".to_string(), format!("shape|{}", shape(word))];
        state.push(if unit.is_some() {
            format!("phrase|{value}")
        } else {
            format!("word|{value}")
        });
        let mut transition = vec![format!("word|{value}")];

        // Rare words by their letters, outside place names.
        if unit.is_none()
            && self.vocabulary.count(word) <= RARE_WORD_COUNT
            && word.chars().any(char::is_alphabetic)
        {
            for n in 3..=6 {
                state.extend(
                    ngrams(word, n)
                        .into_iter()
                        .map(|g| format!("ngram|{n}|{g}")),
                );
            }
        }
        if let Some(suffix) = self.lexicon.street_suffix(word) {
            state.push(format!("street_suffix|{suffix}"));
            state.push("street_suffix".to_string());
        }

        // Dictionary phrases.
        if let Some(phrase) = phrase {
            let pos = position(t, phrase.start, phrase.end);
            let single = phrase.kinds.single();
            for kind in phrase.kinds.iter() {
                let name = kind.as_str();
                state.push(format!("lex|{name}"));
                state.push(format!("lex|{name}|{pos}"));
                if single == Some(kind) {
                    state.push(format!("lex_only|{name}"));
                }
                transition.push(format!("lex|{name}|{pos}"));
            }
        }

        // Gazetteer areas.
        for area in areas.iter().filter(|a| a.start <= t && t < a.end) {
            let (band, quality) = (area.band, area.quality.as_str());
            let pos = position(t, area.start, area.end);
            if band == POSTCODE_BAND {
                // libpostal's "postcode have context" and "no context", the latter
                // split by whether any area is named beside it.
                #[allow(clippy::float_cmp)]
                let context = if area.importance == POSTCODE_IN_AREA_BESIDE {
                    "context"
                } else if area.importance == POSTCODE_ALONE {
                    "alone"
                } else {
                    "unsupported"
                };
                state.push(format!("postcode|{context}"));
                state.push(format!("postcode|{context}|{pos}"));
                continue;
            }
            state.push(format!("gaz|{band}"));
            state.push(format!("gaz|{band}|{pos}"));
            state.push(format!("gaz|{band}|{pos}|{quality}"));
            // How far this word's matches can be trusted: "hill" in an area
            // name is usually the area, "center" or "market" often a venue.
            if !value.starts_with("unk") {
                state.push(format!("word+gaz|{value}|{band}"));
            }
            state.push(format!(
                "gaz|{band}|{quality}|{}",
                importance_bucket(area.importance)
            ));
            if let Some(km) = area.distance_km {
                state.push(format!("gaz|{band}|{quality}|{}", distance_bucket(km)));
            }
            if matches!(pos, "B" | "S") {
                transition.push(format!("gaz|{band}|B"));
            }
        }
        // Whether its area is in the area named after it, or the area
        // before it is in its: which readings of the pair are real places.
        for area in areas.iter().filter(|a| a.start <= t && t < a.end) {
            for (_, _, parent) in &area.inside {
                state.push(format!("inside|{}|{parent}", area.band));
            }
        }
        for child in areas {
            for (_, _, parent) in child
                .inside
                .iter()
                .filter(|(start, end, _)| *start <= t && t < *end)
            {
                state.push(format!("contains|{parent}|{}", child.band));
            }
        }
        // The band a name is most commonly, of the bands it's in: its
        // best-known area's, where libpostal counts its training labels.
        for ((start, end), band) in context.commonly {
            if *start <= t && t < *end {
                state.push(format!("commonly|{band}"));
                state.push(format!("commonly|{band}|{}", position(t, *start, *end)));
            }
        }
        // The areas next to it, which areas follow and subjects precede.
        let next_areas = areas.iter().filter(|a| a.start == t + 1);
        for area in next_areas {
            state.push(format!("next_gaz|{}|{}", area.band, area.quality.as_str()));
        }

        // Where it is in the query.
        if t == 0 {
            if unit.is_none() {
                state.push(format!("first|{value}"));
            }
            state.push("first".to_string());
        }
        if t + 1 == n {
            state.push("last".to_string());
            if query.typing {
                state.push("typing".to_string());
                state.push(format!("typing|{}", shape(word)));
            }
        }
        let after = sections_after(t).min(3);
        if query.section_starts.iter().any(|s| *s) {
            state.push(format!("sections_after|{after}"));
        }
        if query.section_starts[t] {
            state.push("section_start".to_string());
            transition.push("section_start".to_string());
        }

        // Its neighbours, as libpostal has them: outside its place name, and
        // a neighbouring place name as a whole.
        let before = unit.map_or(t, |(start, _)| start);
        let after = unit.map_or(t + 1, |(_, end)| end);
        if before > 0 {
            let prev = self.neighbour(context, before - 1, &phrase_at);
            state.push(format!("prev|{prev}"));
            state.push(format!("prev+word|{prev}|{value}"));
            if before == t {
                transition.push(format!("prev|{prev}"));
            }
        }
        if after < n {
            let next = self.neighbour(context, after, &phrase_at);
            state.push(format!("next|{next}"));
            state.push(format!("word+next|{value}|{next}"));
        }

        // An unknown first word, by what's to its right, as libpostal has it:
        // a venue's name before a number then a street type ("barboncino 781
        // franklin ave"), a street's before its number ("calle x 12"), or a
        // street's with its type after.
        if t == 0 && unit.is_none() && value.starts_with("unk") {
            let number = (1..n).find(|&i| query.words[i].chars().any(char::is_numeric));
            let street = (1..n).find(|&i| {
                phrase_at(i).is_some_and(|p| p.kinds.contains(Kind::StreetType))
                    || self.lexicon.street_suffix(&query.words[i]).is_some()
            });
            let building =
                (1..n).find(|&i| phrase_at(i).is_some_and(|p| p.kinds.contains(Kind::Building)));
            match (number, street) {
                (Some(number), Some(street)) => {
                    let order = if street < number {
                        "street_before_number"
                    } else {
                        "street_after_number"
                    };
                    state.push(format!("first_unk|{order}"));
                    state.push(format!(
                        "first_unk|{order}|{}",
                        self.neighbour(context, street, &phrase_at)
                    ));
                }
                (Some(_), None) => state.push("first_unk|number_right".to_string()),
                (None, Some(street)) => {
                    state.push("first_unk|street_right".to_string());
                    state.push(format!(
                        "first_unk|street_right|{}",
                        self.neighbour(context, street, &phrase_at)
                    ));
                }
                (None, None) => {}
            }
            if building.is_some() {
                state.push("first_unk|building_right".to_string());
            }
        }
        TokenFeatures { state, transition }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn query(text: &str) -> Query {
        let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        let section_starts = vec![false; words.len()];
        Query {
            words,
            section_starts,
            typing: false,
            focus: None,
        }
    }

    #[test]
    fn shapes_and_ngrams() {
        assert_eq!(shape("10010"), "D5");
        assert_eq!(shape("14a"), "Da2");
        assert_eq!(shape("m2m"), "aDa1");
        assert_eq!(shape("seattle"), "a");
        assert_eq!(ngrams("brooklyn", 3)[..2], ["bro_", "_roo_"]);
        assert_eq!(ngrams("brooklyn", 3).last().unwrap(), "_lyn");
        assert!(ngrams("bro", 3).is_empty());
    }

    #[test]
    fn place_names_are_units_with_the_band_they_commonly_are() {
        let lexicon = Lexicon::new();
        let vocabulary = Vocabulary::default();
        let extractor = Extractor {
            lexicon: &lexicon,
            vocabulary: &vocabulary,
        };
        let q = query("chelsea new york");
        let area = |start, end, band, importance| AreaMatch {
            start,
            end,
            band,
            quality: Quality::Exact,
            importance,
            distance_km: None,
            inside: Vec::new(),
        };
        let areas = vec![
            area(0, 1, "neighbourhood", 0.2),
            area(1, 3, "locality", 0.6),
            area(1, 3, "region", 0.55),
            area(2, 3, "locality", 0.1),
        ];
        let features = extractor.features(&q, &areas);
        // Both words of the longest name are it, not themselves.
        for t in [1, 2] {
            assert!(
                features[t].state.contains(&"phrase|new york".to_string()),
                "{:?}",
                features[t].state
            );
            assert!(
                !features[t]
                    .state
                    .iter()
                    .any(|f| f.starts_with("word|") || f.starts_with("ngram|"))
            );
            assert!(
                features[t]
                    .transition
                    .contains(&"word|new york".to_string())
            );
            assert!(features[t].state.contains(&"prev|chelsea".to_string()));
            assert!(features[t].state.contains(&"commonly|locality".to_string()));
        }
        assert!(
            features[1]
                .state
                .contains(&"commonly|locality|B".to_string())
        );
        // The neighbour is the whole name; one band is no commonly.
        assert!(features[0].state.contains(&"next|new york".to_string()));
        assert!(!features[0].state.iter().any(|f| f.starts_with("commonly|")));
    }

    #[test]
    fn postcodes_say_whether_an_area_next_to_them_has_them() {
        let lexicon = Lexicon::new();
        let vocabulary = Vocabulary::default();
        let extractor = Extractor {
            lexicon: &lexicon,
            vocabulary: &vocabulary,
        };
        let area = |start, end, band, importance| AreaMatch {
            start,
            end,
            band,
            quality: Quality::Exact,
            importance,
            distance_km: None,
            inside: Vec::new(),
        };
        let q = query("new york 10001");
        let features = extractor.features(
            &q,
            &[area(0, 2, "locality", 0.6), area(2, 3, POSTCODE_BAND, 1.0)],
        );
        assert!(
            features[2]
                .state
                .contains(&"postcode|context|S".to_string()),
            "{:?}",
            features[2].state
        );
        assert!(
            !features[2]
                .state
                .iter()
                .any(|f| f.starts_with("gaz|postcode"))
        );
        let q = query("10001 main st");
        let features = extractor.features(
            &q,
            &[area(0, 1, POSTCODE_BAND, POSTCODE_NOT_IN_AREA_BESIDE)],
        );
        assert!(
            features[0]
                .state
                .contains(&"postcode|unsupported".to_string())
        );
        let features = extractor.features(&q, &[area(0, 1, POSTCODE_BAND, POSTCODE_ALONE)]);
        assert!(features[0].state.contains(&"postcode|alone".to_string()));
        // A postcode isn't a place name.
        assert!(features[0].state.iter().any(|f| f.starts_with("word|")));
    }

    #[test]
    fn areas_in_the_areas_named_after_them_say_so() {
        let lexicon = Lexicon::new();
        let vocabulary = Vocabulary::default();
        let extractor = Extractor {
            lexicon: &lexicon,
            vocabulary: &vocabulary,
        };
        let area = |start, end, band, inside| AreaMatch {
            start,
            end,
            band,
            quality: Quality::Exact,
            importance: 0.5,
            distance_km: None,
            inside,
        };
        let q = query("toronto ca");
        let areas = vec![
            area(0, 1, "locality", vec![(1, 2, "country")]),
            area(1, 2, "region", vec![]),
            area(1, 2, "country", vec![]),
        ];
        let features = extractor.features(&q, &areas);
        assert!(
            features[0]
                .state
                .contains(&"inside|locality|country".to_string()),
            "{:?}",
            features[0].state
        );
        assert!(
            features[1]
                .state
                .contains(&"contains|country|locality".to_string())
        );
        assert!(
            !features[1]
                .state
                .contains(&"contains|region|locality".to_string())
        );
    }

    #[test]
    fn an_unknown_first_word_knows_whats_to_its_right() {
        let lexicon = Lexicon::new();
        let vocabulary = Vocabulary::default();
        let extractor = Extractor {
            lexicon: &lexicon,
            vocabulary: &vocabulary,
        };
        let venue = extractor.features(&query("barboncino 781 franklin ave"), &[]);
        assert!(
            venue[0]
                .state
                .contains(&"first_unk|street_after_number".to_string()),
            "{:?}",
            venue[0].state
        );
        let street = extractor.features(&query("calle mayor 12"), &[]);
        assert!(
            street[0]
                .state
                .contains(&"first_unk|street_before_number".to_string())
                || street[0].state.iter().any(|f| f.starts_with("lex|")),
            "{:?}",
            street[0].state
        );
        let bare = extractor.features(&query("barboncino 781"), &[]);
        assert!(
            bare[0]
                .state
                .contains(&"first_unk|number_right".to_string())
        );
    }

    #[test]
    fn areas_give_their_words_bands_positions_and_quality() {
        let lexicon = Lexicon::new();
        let vocabulary = Vocabulary::default();
        let extractor = Extractor {
            lexicon: &lexicon,
            vocabulary: &vocabulary,
        };
        let q = query("pizza new york");
        let areas = vec![AreaMatch {
            start: 1,
            end: 3,
            band: "locality",
            quality: Quality::Exact,
            importance: 0.6,
            distance_km: Some(4000.0),
            inside: Vec::new(),
        }];
        let features = extractor.features(&q, &areas);
        assert!(
            features[1]
                .state
                .contains(&"gaz|locality|B|exact".to_string())
        );
        assert!(
            features[2]
                .state
                .contains(&"gaz|locality|E|exact".to_string())
        );
        assert!(
            features[2]
                .state
                .contains(&"gaz|locality|exact|top".to_string())
        );
        assert!(
            features[2]
                .state
                .contains(&"gaz|locality|exact|far".to_string())
        );
        assert!(
            features[1]
                .transition
                .contains(&"gaz|locality|B".to_string())
        );
        assert!(
            features[0]
                .state
                .contains(&"next_gaz|locality|exact".to_string())
        );
        // Rare words are unknown, with their n-grams.
        assert!(features[0].state.contains(&"word|unk".to_string()));
        assert!(features[0].state.iter().any(|f| f.starts_with("ngram|3|")));
    }

    #[test]
    fn dictionary_phrases_and_sections() {
        let lexicon = Lexicon::new();
        let vocabulary = Vocabulary::default();
        let extractor = Extractor {
            lexicon: &lexicon,
            vocabulary: &vocabulary,
        };
        let mut q = query("123 main st seattle");
        q.section_starts[3] = true;
        q.typing = true;
        let features = extractor.features(&q, &[]);
        assert!(features[2].state.contains(&"lex|street_type|S".to_string()));
        assert!(
            features[3]
                .state
                .iter()
                .any(|f| f.starts_with("prev|<") && f.contains("street_type")),
            "{:?}",
            features[3].state
        );
        assert!(features[0].state.contains(&"shape|D3".to_string()));
        assert!(features[0].state.contains(&"sections_after|1".to_string()));
        assert!(features[3].state.contains(&"section_start".to_string()));
        assert!(features[3].state.contains(&"typing".to_string()));
    }
}
