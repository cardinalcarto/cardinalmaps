//! A trained model with what it needs to read queries.

use std::io::Read;

use crate::features::{AreaMatch, Extractor, Query, Vocabulary};
use crate::labels::Tag;
use crate::lexicon::Lexicon;
use crate::model::Model;
use crate::text::{fold, words};

/// Reads queries: which words are what.
pub struct Tagger {
    model: Model,
    vocabulary: Vocabulary,
    lexicon: Lexicon,
}

/// A reading of a query: a tag for each word, and how likely it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Interpretation {
    pub tags: Vec<Tag>,
    pub probability: f64,
}

impl Tagger {
    /// A model written by `Model::write` with its vocabulary.
    ///
    /// # Errors
    /// If it isn't one.
    pub fn read(input: &mut impl Read) -> std::io::Result<Self> {
        let (model, known) = Model::read(input)?;
        Ok(Self { model, vocabulary: Vocabulary::from_counts(known.into_iter().collect()), lexicon: Lexicon::new() })
    }

    /// The most likely readings, most likely first, until together they're
    /// at least `coverage` likely or there are `max` of them.
    #[must_use]
    pub fn interpret(&self, query: &Query, areas: &[AreaMatch], max: usize, coverage: f64) -> Vec<Interpretation> {
        if query.words.is_empty() {
            return Vec::new();
        }
        let features = Extractor { lexicon: &self.lexicon, vocabulary: &self.vocabulary }.features(query, areas);
        let mut total = 0.0;
        let mut readings = Vec::new();
        for (path, probability) in self.model.best(&features, max) {
            if total >= coverage {
                break;
            }
            total += probability;
            readings.push(Interpretation { tags: path.into_iter().map(Tag::of_label).collect(), probability });
        }
        readings
    }
}

/// A query's words, folded, from its text: comma-separated sections, the
/// last word maybe still being typed, seen from `focus`.
#[must_use]
pub fn query(text: &str, typing: bool, focus: Option<[f64; 2]>) -> Query {
    let mut query = Query { typing, focus, ..Query::default() };
    for (i, section) in text.split(',').enumerate() {
        let folded = fold(section);
        for (j, (_, word)) in words(&folded).into_iter().enumerate() {
            query.section_starts.push(i > 0 && j == 0);
            query.words.push(word.to_string());
        }
    }
    query
}
