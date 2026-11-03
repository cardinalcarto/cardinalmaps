//! Parse results and soft, per-token class weights.

use std::collections::HashMap;
use std::ops::Range;

use serde::Serialize;

use crate::classification::Kind;
use crate::parser::AddressParser;
use crate::tokenization::{SpanId, Tokenizer};

/// A range of the input, in bytes (for slicing the `&str`) and in chars.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct TextRange {
    /// Byte offset of the start.
    pub start: usize,
    /// Byte offset of the end (exclusive).
    pub end: usize,
    /// Char (Unicode scalar) offset of the start.
    pub char_start: usize,
    /// Char offset of the end (exclusive).
    pub char_end: usize,
}

impl TextRange {
    /// The byte range, for slicing the input.
    pub fn bytes(&self) -> Range<usize> {
        self.start..self.end
    }
}

/// A span of the input with a public classification, as part of a solution.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClassifiedSpan {
    /// What the span is: one of the public kinds (house number, street,
    /// postcode, venue, locality, region, country, dependency, unit,
    /// unit type).
    pub kind: Kind,
    /// The input text of the span.
    pub text: String,
    /// Where the span is in the input.
    pub range: TextRange,
    /// The classifier's confidence in this classification, in `(0, 1]`.
    pub confidence: f64,
}

/// One interpretation of the query: non-overlapping classified spans.
/// Words not in any span are unclassified (for example the subject of a
/// categorical query such as "coffee").
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Solution {
    /// Upstream's score in `[0, 1]`: the confidence-weighted fraction of the
    /// query's characters covered, minus penalties. Not a probability.
    pub score: f64,
    /// The spans, in input order.
    pub spans: Vec<ClassifiedSpan>,
}

/// A whitespace-separated word of the query (sections split at commas,
/// tabs, newlines and quotes; words at whitespace).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Token {
    /// The word.
    pub text: String,
    /// Where it is in the input.
    pub range: TextRange,
}

/// How likely a piece of the query is to be each kind, aggregated over
/// solutions. See [`Parse::token_weights`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Weights {
    /// Kinds with non-zero weight, highest first.
    pub kinds: Vec<(Kind, f64)>,
    /// Remaining weight: no solution classifies this text (or only with low
    /// confidence). `1.0` when nothing does.
    pub unclassified: f64,
}

impl Weights {
    /// The weight of one kind (0 if absent).
    pub fn get(&self, kind: Kind) -> f64 {
        self.kinds
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(0.0, |(_, w)| *w)
    }

    /// Total weight of the admin kinds (locality, region, country,
    /// dependency).
    pub fn admin(&self) -> f64 {
        [
            Kind::Locality,
            Kind::Region,
            Kind::Country,
            Kind::Dependency,
        ]
        .iter()
        .map(|k| self.get(*k))
        .sum()
    }

    /// Total weight of the address kinds (house number, street, postcode,
    /// unit, unit type).
    pub fn address(&self) -> f64 {
        [
            Kind::HouseNumber,
            Kind::Street,
            Kind::Postcode,
            Kind::Unit,
            Kind::UnitType,
        ]
        .iter()
        .map(|k| self.get(*k))
        .sum()
    }
}

/// [`Weights`] for one token.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TokenWeights {
    /// The token.
    pub token: Token,
    /// Its weights.
    pub weights: Weights,
}

/// A raw classifier output: a word or phrase and one classification it
/// received, public or private, whether or not any solution used it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Feature {
    /// The classification, e.g. [`Kind::Locality`] for a phrase found in
    /// Who's On First, [`Kind::Place`] for keywords such as "park".
    pub kind: Kind,
    /// The input text.
    pub text: String,
    /// Where it is in the input.
    pub range: TextRange,
    /// The classifier's confidence.
    pub confidence: f64,
}

/// The result of parsing a query: ranked solutions plus helpers to use them
/// as soft signals.
#[derive(Clone, Debug, Serialize)]
pub struct Parse {
    input: String,
    solutions: Vec<Solution>,
    tokens: Vec<Token>,
    features: Vec<Feature>,
}

/// Maps the tokenizer's UTF-16 offsets to byte and char offsets.
struct OffsetMap {
    /// UTF-16 offset of each char boundary, plus the end.
    utf16: Vec<usize>,
    /// Byte offset of each char boundary, plus the end.
    bytes: Vec<usize>,
}

impl OffsetMap {
    fn new(input: &str) -> Self {
        let (mut utf16, mut bytes) = (vec![0], vec![0]);
        let mut u = 0;
        for (b, c) in input.char_indices() {
            u += c.len_utf16();
            utf16.push(u);
            bytes.push(b + c.len_utf8());
        }
        OffsetMap { utf16, bytes }
    }

    /// Char index of the char boundary at or after UTF-16 offset `u`.
    fn char_index(&self, u: usize) -> usize {
        self.utf16
            .partition_point(|&x| x < u)
            .min(self.utf16.len() - 1)
    }

    fn range(&self, start: usize, end: usize) -> TextRange {
        let (char_start, char_end) = (self.char_index(start), self.char_index(end));
        TextRange {
            start: self.bytes[char_start],
            end: self.bytes[char_end],
            char_start,
            char_end,
        }
    }
}

impl Parse {
    pub(crate) fn new(input: &str, t: &Tokenizer) -> Self {
        let map = OffsetMap::new(input);
        let range_of = |id: SpanId| {
            let s = t.span(id);
            map.range(s.start, s.end)
        };
        let text_of = |r: &TextRange| input[r.bytes()].to_string();

        let solutions = t
            .solutions
            .iter()
            .map(|s| Solution {
                score: s.score,
                spans: s
                    .pairs
                    .iter()
                    .map(|p| {
                        let range = range_of(p.span);
                        let c = p.classification(t);
                        ClassifiedSpan {
                            kind: c.kind,
                            text: text_of(&range),
                            range,
                            confidence: c.confidence,
                        }
                    })
                    .collect(),
            })
            .collect();

        let tokens = t
            .words
            .iter()
            .map(|&w| {
                let range = range_of(w);
                Token {
                    text: text_of(&range),
                    range,
                }
            })
            .collect();

        // every classification of every word and phrase, deduplicated by
        // range and kind (keeping the highest confidence)
        let mut best: HashMap<(usize, usize, Kind), f64> = HashMap::new();
        let mut order: Vec<(usize, usize, Kind)> = Vec::new();
        for &section in &t.sections {
            let section = t.span(section);
            for &id in section.children.iter().chain(&section.phrases) {
                let s = t.span(id);
                for c in &s.classifications {
                    let key = (s.start, s.end, c.kind);
                    let entry = best.entry(key).or_insert_with(|| {
                        order.push(key);
                        c.confidence
                    });
                    *entry = entry.max(c.confidence);
                }
            }
        }
        order.sort_by_key(|&(start, end, kind)| (start, std::cmp::Reverse(end), kind));
        let features = order
            .into_iter()
            .map(|key @ (start, end, kind)| {
                let range = map.range(start, end);
                Feature {
                    kind,
                    text: text_of(&range),
                    range,
                    confidence: best[&key],
                }
            })
            .collect();

        Parse {
            input: input.to_string(),
            solutions,
            tokens,
            features,
        }
    }

    /// The parsed query.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Solutions, best first (at most `max_solutions`, 10 by default). Empty
    /// when nothing in the query was classified.
    pub fn solutions(&self) -> &[Solution] {
        &self.solutions
    }

    /// The best solution, if any.
    pub fn best(&self) -> Option<&Solution> {
        self.solutions.first()
    }

    /// The query's words.
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    /// Raw classifier output for every word and phrase, ordered by position
    /// (longer spans first). Includes private intermediate kinds such as
    /// [`Kind::Place`], [`Kind::StreetSuffix`] or [`Kind::StopWord`], and
    /// admin matches no solution picked.
    pub fn features(&self) -> &[Feature] {
        &self.features
    }

    /// [`Weights`] for any byte range of the input, e.g. a token from a
    /// different tokenizer.
    ///
    /// Each solution contributes its share of the total score
    /// (`score / Σ scores`); within a solution, each span overlapping the
    /// range contributes that share × the fraction of the range it covers ×
    /// its confidence. Whatever is left is [`Weights::unclassified`]. This is
    /// a heuristic, not a calibrated probability.
    pub fn weights(&self, range: Range<usize>) -> Weights {
        let total: f64 = self
            .solutions
            .iter()
            .map(|s| s.score)
            .filter(|s| s.is_finite() && *s > 0.0)
            .sum();
        let mut kinds: Vec<(Kind, f64)> = Vec::new();
        let width = self
            .input
            .get(range.clone())
            .map_or(0, |s| s.chars().count())
            .max(1) as f64;
        if total > 0.0 {
            for s in &self.solutions {
                if !(s.score.is_finite() && s.score > 0.0) {
                    continue;
                }
                let share = s.score / total;
                for span in &s.spans {
                    let (start, end) = (
                        span.range.start.max(range.start),
                        span.range.end.min(range.end),
                    );
                    if start >= end {
                        continue;
                    }
                    let overlap =
                        self.input.get(start..end).map_or(0, |x| x.chars().count()) as f64;
                    let w = share * (overlap / width).min(1.0) * span.confidence;
                    match kinds.iter_mut().find(|(k, _)| *k == span.kind) {
                        Some((_, v)) => *v += w,
                        None => kinds.push((span.kind, w)),
                    }
                }
            }
        }
        kinds.retain(|(_, w)| *w > 0.0);
        kinds.sort_by(|a, b| b.1.total_cmp(&a.1));
        let classified: f64 = kinds.iter().map(|(_, w)| w).sum();
        Weights {
            kinds,
            unclassified: (1.0 - classified).max(0.0),
        }
    }

    /// [`Weights`] for every token, see [`Parse::weights`].
    pub fn token_weights(&self) -> Vec<TokenWeights> {
        self.tokens
            .iter()
            .map(|t| TokenWeights {
                token: t.clone(),
                weights: self.weights(t.range.bytes()),
            })
            .collect()
    }
}

impl AddressParser {
    /// Parses a query.
    pub fn parse(&self, input: &str) -> Parse {
        Parse::new(input, &self.run(input))
    }

    /// Solutions in upstream's test format: per solution, `(label, body)`
    /// pairs where `body` is upstream's span body (phrase words joined by
    /// single spaces). For the ported upstream test suite.
    #[doc(hidden)]
    pub fn upstream_solutions(&self, input: &str) -> Vec<Vec<(String, String)>> {
        let t = self.run(input);
        t.solutions
            .iter()
            .map(|s| {
                s.pairs
                    .iter()
                    .map(|p| (p.kind(&t).label().to_string(), p.span(&t).body.clone()))
                    .collect()
            })
            .collect()
    }
}
