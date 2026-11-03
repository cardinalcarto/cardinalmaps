//! Admin areas from Who's On First names (upstream `WhosOnFirstClassifier`).

use std::borrow::Cow;
use std::collections::HashSet;

use super::{Classifier, for_each_phrase};
use crate::classification::{Classification, Kind};
use crate::js;
use crate::resources;
use crate::text::normalize_wof;
use crate::tokenization::Tokenizer;

/// Names that are too ambiguous to be admin areas.
const BLACKLIST: &[&str] = &[
    "north", "south", "east", "west", "street", "city", "king", "at", "rue", "one", "two", "three",
    "four", "five", "six", "seven", "eight", "nine", "ten", "cafe", "small", "town", "grand",
];

/// Locality names ending in these words sound like streets and are dropped.
const STREETLIKE_LOCALITY_ENDINGS: &[&str] = &[
    "avenue", "lane", "terrace", "street", "road", "crescent", "furlong", "broadway",
];

/// A placetype, its source files and the classification it yields.
struct Placetype {
    kind: Kind,
    names: HashSet<Cow<'static, str>>,
}

/// Classifies phrases naming a locality, region or country as `area` plus
/// the placetype.
pub(crate) struct WhosOnFirstClassifier {
    /// From most to least granular: locality, region, country.
    placetypes: Vec<Placetype>,
}

impl WhosOnFirstClassifier {
    pub fn new() -> Self {
        let sources: [(&str, Kind, &[&str]); 3] = [
            ("locality", Kind::Locality, &["name-*_x_preferred.txt"]),
            (
                "region",
                Kind::Region,
                &["abrv-*_x_preferred.txt", "name-*_x_preferred.txt"],
            ),
            (
                "country",
                Kind::Country,
                &[
                    "name-*_x_preferred.txt",
                    "wof-country.txt",
                    "wof-country_alpha3.txt",
                ],
            ),
        ];
        let placetypes = sources
            .into_iter()
            .map(|(placetype, kind, patterns)| {
                let mut names = HashSet::new();
                resources::load_whosonfirst(&mut names, placetype, patterns);
                for word in BLACKLIST {
                    names.remove(*word);
                }
                if kind == Kind::Locality {
                    names.retain(|name| {
                        let last = name.rsplit(js::is_whitespace).next().unwrap_or(name);
                        !STREETLIKE_LOCALITY_ENDINGS.contains(&last)
                    });
                }
                names.shrink_to_fit();
                Placetype { kind, names }
            })
            .collect();
        WhosOnFirstClassifier { placetypes }
    }
}

impl Classifier for WhosOnFirstClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for_each_phrase(t, |t, id| {
            let span = t.span(id);
            let mut confidence = 1.0;
            // nothing right after an intersection marker; half confidence
            // after a stop word
            let first = t.span(span.child_first.unwrap_or(id));
            if let Some(&prev) = first.prev.first() {
                let prev = t.span(prev);
                if prev.has(Kind::Intersection) {
                    return;
                } else if prev.has(Kind::StopWord) {
                    confidence /= 2.0;
                }
            }
            // nothing right before a street suffix or place keyword
            let last = t.span(span.child_last.unwrap_or(id));
            if let Some(&next) = last.next.first()
                && t.span(next).has_any(&[Kind::StreetSuffix, Kind::Place])
            {
                return;
            }
            // stop words (or phrases starting with one) are never admin areas
            let is_stop_word = span.has(Kind::StopWord)
                || span
                    .children
                    .first()
                    .is_some_and(|&c| t.span(c).has(Kind::StopWord));
            if is_stop_word {
                return;
            }
            let normalized = normalize_wof(&span.norm);
            let kinds: Vec<Kind> = self
                .placetypes
                .iter()
                .filter(|p| p.names.contains(normalized.as_ref()))
                .map(|p| p.kind)
                .collect();
            let span = t.span_mut(id);
            for kind in kinds {
                span.classify(Classification::new(Kind::Area, confidence));
                span.classify(Classification::new(kind, confidence));
            }
        });
    }
}
