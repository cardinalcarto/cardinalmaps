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

use std::collections::HashMap;

use crate::crf::TokenFeatures;
use crate::lexicon::{Lexicon, Phrase};

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
        self.counts.iter().filter(|(_, c)| **c >= MIN_WORD_COUNT).map(|(w, c)| (w.as_str(), *c))
    }

    #[must_use]
    pub fn from_counts(counts: HashMap<String, u32>) -> Self {
        Self { counts }
    }
}

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
        let class = if c.is_ascii_digit() || c.is_numeric() { 'D' } else if c.is_alphabetic() { 'a' } else { 'x' };
        if !shape.ends_with(class) {
            shape.push(class);
        }
    }
    let digits = word.chars().filter(|c| c.is_numeric()).count();
    if digits > 0 { format!("{shape}{}", digits.min(7)) } else { shape }
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
        let sections_after = |t: usize| query.section_starts.iter().skip(t + 1).filter(|s| **s).count();
        (0..n).map(|t| self.token(query, t, areas, phrase_at(t), phrase_at, &sections_after)).collect()
    }

    fn token<'p>(
        &self,
        query: &Query,
        t: usize,
        areas: &[AreaMatch],
        phrase: Option<&'p Phrase>,
        phrase_at: impl Fn(usize) -> Option<&'p Phrase>,
        sections_after: &impl Fn(usize) -> usize,
    ) -> TokenFeatures {
        let n = query.words.len();
        let word = query.words[t].as_str();
        let value = self.word(word);
        let mut state = vec!["bias".to_string(), format!("word|{value}"), format!("shape|{}", shape(word))];
        let mut transition = vec![format!("word|{value}")];

        // Rare words by their letters.
        if self.vocabulary.count(word) <= RARE_WORD_COUNT && word.chars().any(char::is_alphabetic) {
            for n in 3..=6 {
                state.extend(ngrams(word, n).into_iter().map(|g| format!("ngram|{n}|{g}")));
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
            state.push(format!("gaz|{band}"));
            state.push(format!("gaz|{band}|{pos}"));
            state.push(format!("gaz|{band}|{pos}|{quality}"));
            // How far this word's matches can be trusted: "hill" in an area
            // name is usually the area, "center" or "market" often a venue.
            if !value.starts_with("unk") {
                state.push(format!("word+gaz|{value}|{band}"));
            }
            state.push(format!("gaz|{band}|{quality}|{}", importance_bucket(area.importance)));
            if let Some(km) = area.distance_km {
                state.push(format!("gaz|{band}|{quality}|{}", distance_bucket(km)));
            }
            if matches!(pos, "B" | "S") {
                transition.push(format!("gaz|{band}|B"));
            }
        }
        // The areas next to it, which areas follow and subjects precede.
        let next_areas = areas.iter().filter(|a| a.start == t + 1);
        for area in next_areas {
            state.push(format!("next_gaz|{}|{}", area.band, area.quality.as_str()));
        }

        // Where it is in the query.
        if t == 0 {
            state.push(format!("first|{value}"));
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

        // Its neighbours, as libpostal has them.
        let lex_or_word = |i: usize| -> String {
            match phrase_at(i) {
                Some(phrase) => format!("<{}>", phrase.kinds.iter().map(|k| k.as_str()).collect::<Vec<_>>().join("+")),
                None => self.word(&query.words[i]).to_string(),
            }
        };
        if t > 0 {
            let prev = lex_or_word(t - 1);
            state.push(format!("prev|{prev}"));
            state.push(format!("prev+word|{prev}|{value}"));
            transition.push(format!("prev|{prev}"));
        }
        if t + 1 < n {
            let next = lex_or_word(t + 1);
            state.push(format!("next|{next}"));
            state.push(format!("word+next|{value}|{next}"));
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
        Query { words, section_starts, typing: false, focus: None }
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
    fn areas_give_their_words_bands_positions_and_quality() {
        let lexicon = Lexicon::new();
        let vocabulary = Vocabulary::default();
        let extractor = Extractor { lexicon: &lexicon, vocabulary: &vocabulary };
        let q = query("pizza new york");
        let areas = vec![AreaMatch {
            start: 1,
            end: 3,
            band: "locality",
            quality: Quality::Exact,
            importance: 0.6,
            distance_km: Some(4000.0),
        }];
        let features = extractor.features(&q, &areas);
        assert!(features[1].state.contains(&"gaz|locality|B|exact".to_string()));
        assert!(features[2].state.contains(&"gaz|locality|E|exact".to_string()));
        assert!(features[2].state.contains(&"gaz|locality|exact|top".to_string()));
        assert!(features[2].state.contains(&"gaz|locality|exact|far".to_string()));
        assert!(features[1].transition.contains(&"gaz|locality|B".to_string()));
        assert!(features[0].state.contains(&"next_gaz|locality|exact".to_string()));
        // Rare words are unknown, with their n-grams.
        assert!(features[0].state.contains(&"word|unk".to_string()));
        assert!(features[0].state.iter().any(|f| f.starts_with("ngram|3|")));
    }

    #[test]
    fn dictionary_phrases_and_sections() {
        let lexicon = Lexicon::new();
        let vocabulary = Vocabulary::default();
        let extractor = Extractor { lexicon: &lexicon, vocabulary: &vocabulary };
        let mut q = query("123 main st seattle");
        q.section_starts[3] = true;
        q.typing = true;
        let features = extractor.features(&q, &[]);
        assert!(features[2].state.contains(&"lex|street_type|S".to_string()));
        assert!(features[3].state.iter().any(|f| f.starts_with("prev|<") && f.contains("street_type")), "{:?}", features[3].state);
        assert!(features[0].state.contains(&"shape|D3".to_string()));
        assert!(features[0].state.contains(&"sections_after|1".to_string()));
        assert!(features[3].state.contains(&"section_start".to_string()));
        assert!(features[3].state.contains(&"typing".to_string()));
    }
}
