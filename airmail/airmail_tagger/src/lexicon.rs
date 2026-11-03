//! Words and phrases libpostal's dictionaries know, as kinds: street types,
//! directionals, unit and level types, words like "near", stopwords. Every
//! language's are merged, as libpostal's parser does, so `strasse` and
//! `street` are both street types whatever the query's language.

use std::collections::HashMap;

use crate::dictionaries;
use crate::text::{fold, words};

/// What a dictionary phrase is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    StreetType,
    Directional,
    Stopword,
    Near,
    Unit,
    Level,
    Building,
    PostOffice,
    Entrance,
    Staircase,
    Number,
}

impl Kind {
    pub const ALL: [Kind; 11] = [
        Kind::StreetType,
        Kind::Directional,
        Kind::Stopword,
        Kind::Near,
        Kind::Unit,
        Kind::Level,
        Kind::Building,
        Kind::PostOffice,
        Kind::Entrance,
        Kind::Staircase,
        Kind::Number,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::StreetType => "street_type",
            Kind::Directional => "directional",
            Kind::Stopword => "stopword",
            Kind::Near => "near",
            Kind::Unit => "unit",
            Kind::Level => "level",
            Kind::Building => "building",
            Kind::PostOffice => "post_office",
            Kind::Entrance => "entrance",
            Kind::Staircase => "staircase",
            Kind::Number => "number",
        }
    }

    /// The dictionary types of each kind.
    fn dictionaries(self) -> &'static [&'static str] {
        match self {
            Kind::StreetType => &["street_types"],
            Kind::Directional => &["directionals"],
            Kind::Stopword => &["stopwords"],
            Kind::Near => &["near"],
            Kind::Unit => &["unit_types_numbered", "unit_types_standalone"],
            Kind::Level => &[
                "level_types_numbered",
                "level_types_standalone",
                "level_types_basement",
                "level_types_mezzanine",
                "level_types_sub_basement",
            ],
            Kind::Building => &["building_types"],
            Kind::PostOffice => &["post_office"],
            Kind::Entrance => &["entrances"],
            Kind::Staircase => &["staircases"],
            Kind::Number => &["number"],
        }
    }

    fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// The kinds a phrase is, as a set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Kinds(u16);

impl Kinds {
    #[must_use]
    pub fn contains(self, kind: Kind) -> bool {
        self.0 & kind.bit() != 0
    }

    pub fn iter(self) -> impl Iterator<Item = Kind> {
        Kind::ALL.into_iter().filter(move |k| self.contains(*k))
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The kind, if the phrase is only one.
    #[must_use]
    pub fn single(self) -> Option<Kind> {
        (self.0.count_ones() == 1).then(|| self.iter().next()).flatten()
    }
}

/// A dictionary phrase in a query: words `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phrase {
    pub start: usize,
    pub end: usize,
    pub kinds: Kinds,
}

/// The dictionaries, as phrases and street-type suffixes.
#[derive(Debug, Default)]
pub struct Lexicon {
    /// Phrases (words joined by spaces) and their kinds.
    phrases: HashMap<String, Kinds>,
    /// Street types written joined to a name (German `straße`, Dutch
    /// `weg`), longest first.
    suffixes: Vec<String>,
    longest: usize,
}

/// The shortest name a street-type suffix is recognized after.
const MIN_STEM: usize = 3;

impl Lexicon {
    /// Every language's dictionaries.
    #[must_use]
    pub fn new() -> Self {
        let mut lexicon = Lexicon::default();
        for kind in Kind::ALL {
            for dictionary in kind.dictionaries() {
                for (_, contents) in dictionaries::of_type(dictionary) {
                    for line in contents.lines() {
                        for phrase in line.split('|') {
                            let key = key(phrase);
                            if key.is_empty() {
                                continue;
                            }
                            lexicon.longest = lexicon.longest.max(key.split(' ').count());
                            let kinds = lexicon.phrases.entry(key).or_default();
                            kinds.0 |= kind.bit();
                        }
                    }
                }
            }
        }
        for dictionary in ["concatenated_suffixes_separable", "concatenated_suffixes_inseparable"] {
            for (_, contents) in dictionaries::of_type(dictionary) {
                for line in contents.lines() {
                    lexicon.suffixes.extend(line.split('|').map(key).filter(|s| s.len() >= 2 && !s.contains(' ')));
                }
            }
        }
        lexicon.suffixes.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        lexicon.suffixes.dedup();
        lexicon
    }

    /// The kinds of a phrase of folded words.
    #[must_use]
    pub fn kinds(&self, words: &[String]) -> Kinds {
        self.phrases.get(&words.join(" ")).copied().unwrap_or_default()
    }

    /// The query's dictionary phrases: the longest at each word, left to
    /// right, not overlapping.
    #[must_use]
    pub fn phrases(&self, words: &[String]) -> Vec<Phrase> {
        let mut phrases = Vec::new();
        let mut start = 0;
        while start < words.len() {
            let found = (start + 1..=words.len().min(start + self.longest))
                .rev()
                .find_map(|end| Some((end, self.kinds(&words[start..end]))).filter(|(_, k)| !k.is_empty()));
            match found {
                Some((end, kinds)) => {
                    phrases.push(Phrase { start, end, kinds });
                    start = end;
                }
                None => start += 1,
            }
        }
        phrases
    }

    /// The street-type suffix a word ends with, after a name: `straße` in
    /// `grolmanstraße`.
    #[must_use]
    pub fn street_suffix<'a>(&'a self, word: &str) -> Option<&'a str> {
        self.suffixes
            .iter()
            .find(|suffix| word.len() >= suffix.len() + MIN_STEM && word.ends_with(suffix.as_str()))
            .map(String::as_str)
    }
}

/// A dictionary phrase as query words are: folded and split, joined by
/// spaces.
fn key(phrase: &str) -> String {
    let folded = fold(phrase.trim());
    words(&folded).into_iter().map(|(_, w)| w).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod test {
    use super::*;

    fn words_of(text: &str) -> Vec<String> {
        words(&fold(text)).into_iter().map(|(_, w)| w.to_string()).collect()
    }

    #[test]
    fn dictionaries_know_address_words() {
        let lexicon = Lexicon::new();
        assert!(lexicon.kinds(&words_of("street")).contains(Kind::StreetType));
        assert!(lexicon.kinds(&words_of("st")).contains(Kind::StreetType));
        assert!(lexicon.kinds(&words_of("Straße")).contains(Kind::StreetType));
        assert!(lexicon.kinds(&words_of("rue")).contains(Kind::StreetType));
        assert!(lexicon.kinds(&words_of("near")).contains(Kind::Near));
        assert!(lexicon.kinds(&words_of("apt")).contains(Kind::Unit));
        assert!(lexicon.kinds(&words_of("north")).contains(Kind::Directional));
        assert!(lexicon.kinds(&words_of("seattle")).is_empty());
    }

    #[test]
    fn phrases_are_the_longest_at_each_word() {
        let lexicon = Lexicon::new();
        let words = words_of("coffee near me on 5th ave");
        let phrases = lexicon.phrases(&words);
        let near = phrases.iter().find(|p| p.kinds.contains(Kind::Near)).unwrap();
        assert_eq!(&words[near.start..near.end], ["near", "me"]);
        assert!(phrases.iter().any(|p| words[p.start] == "ave" && p.kinds.contains(Kind::StreetType)));
    }

    #[test]
    fn joined_street_types_are_suffixes() {
        let lexicon = Lexicon::new();
        assert!(lexicon.street_suffix("grolmanstrasse").is_some());
        assert!(lexicon.street_suffix("strasse").is_none());
    }
}
