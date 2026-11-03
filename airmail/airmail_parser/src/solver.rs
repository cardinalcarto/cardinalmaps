//! Solvers (upstream `solver/*.js`): build candidate parses ("solutions")
//! from the classified spans, then filter, rewrite and penalize them.

use crate::classification::{Classification, Kind};
use crate::tokenization::{Span, SpanId, Tokenizer};

/// Marks the "skip this label" placeholder pair upstream adds per label.
const EMPTY: SpanId = SpanId::MAX;

/// A span with one of its classifications (upstream `SolutionPair`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pair {
    pub span: SpanId,
    /// Index into the span's classifications.
    pub class: u16,
}

impl Pair {
    pub fn classification(self, t: &Tokenizer) -> &Classification {
        &t.span(self.span).classifications[self.class as usize]
    }
    pub fn kind(self, t: &Tokenizer) -> Kind {
        self.classification(t).kind
    }
    pub fn span(self, t: &Tokenizer) -> &Span {
        t.span(self.span)
    }
}

/// A candidate parse: non-overlapping classified spans (upstream `Solution`).
#[derive(Clone, Debug, Default)]
pub(crate) struct Solution {
    pub pairs: Vec<Pair>,
    pub score: f64,
    pub penalty: f64,
}

impl Solution {
    /// Upstream `copy`: the same pairs, with score and penalty reset.
    fn copy(&self) -> Solution {
        Solution {
            pairs: self.pairs.clone(),
            ..Default::default()
        }
    }

    fn first(&self, t: &Tokenizer, kind: Kind) -> Option<Pair> {
        self.pairs.iter().copied().find(|p| p.kind(t) == kind)
    }

    fn all(&self, t: &Tokenizer, kind: Kind) -> Vec<Pair> {
        self.pairs
            .iter()
            .copied()
            .filter(|p| p.kind(t) == kind)
            .collect()
    }

    fn without(&mut self, t: &Tokenizer, kind: Kind) {
        self.pairs.retain(|p| p.kind(t) != kind);
    }

    /// Upstream `coversSameClassification`: every pair of `other` lies within
    /// a pair of `self` with the same classification.
    fn covers_same_classification(&self, t: &Tokenizer, other: &Solution) -> bool {
        other.pairs.iter().all(|p| {
            self.pairs
                .iter()
                .any(|pp| pp.kind(t) == p.kind(t) && pp.span(t).covers(p.span(t)))
        })
    }

    /// Upstream `computeScore`: the confidence-weighted share of the input's
    /// characters covered, times `1 - penalty`.
    pub fn compute_score(&mut self, t: &Tokenizer) {
        let (mut coverage, mut confidence) = (0.0f64, 0.0f64);
        for p in &self.pairs {
            let span = p.span(t);
            let range = if span.children.is_empty() {
                span.end - span.start
            } else {
                span.children
                    .iter()
                    .map(|&c| t.span(c).end - t.span(c).start)
                    .sum()
            } as f64;
            coverage += range;
            confidence += p.classification(t).confidence * range;
        }
        self.score =
            (confidence / coverage) * (coverage / t.coverage as f64) * (1.0 - self.penalty);
    }
}

/// A pass over the solutions.
pub(crate) trait Solver: Send + Sync {
    fn solve(&self, t: &mut Tokenizer);
}

/// Upstream `HashMapSolver.MAX_PAIRS_PER_LABEL`.
const MAX_PAIRS_PER_LABEL: usize = 8;

/// Upstream `HashMapSolver.generateHashMap`: per label (in first-seen order)
/// up to eight pairs, phrases before words within each section.
fn generate_hash_map(
    t: &Tokenizer,
    include_private: bool,
    include_empty: bool,
) -> Vec<(Kind, Vec<Pair>)> {
    let mut map: Vec<(Kind, Vec<Pair>)> = Vec::new();
    let add = |map: &mut Vec<(Kind, Vec<Pair>)>, span: SpanId| {
        for (i, c) in t.span(span).classifications.iter().enumerate() {
            if !include_private && !c.kind.is_public() {
                continue;
            }
            let entry = match map.iter().position(|(k, _)| *k == c.kind) {
                Some(i) => &mut map[i].1,
                None => {
                    let pairs = if include_empty {
                        vec![Pair {
                            span: EMPTY,
                            class: 0,
                        }]
                    } else {
                        Vec::new()
                    };
                    map.push((c.kind, pairs));
                    &mut map.last_mut().expect("just pushed").1
                }
            };
            if entry.len() < MAX_PAIRS_PER_LABEL {
                entry.push(Pair {
                    span,
                    class: u16::try_from(i).expect("few classifications"),
                });
            }
        }
    };
    for &section in &t.sections {
        for &phrase in &t.span(section).phrases {
            add(&mut map, phrase);
        }
        for &word in &t.span(section).children {
            add(&mut map, word);
        }
    }
    map
}

/// Builds every combination of at most one pair per public label whose
/// spans don't overlap (upstream `ExclusiveCartesianSolver`).
pub(crate) struct ExclusiveCartesianSolver;

/// Upstream limits: recursion depth (labels) and solutions generated.
const MAX_RECURSION: usize = 10;
const MAX_SOLUTIONS: usize = 50_000;

impl ExclusiveCartesianSolver {
    /// Upstream `exclusiveCartesian`, which enumerates the full product of
    /// the label lists (counting overlapping combinations against
    /// `MAX_SOLUTIONS`), reverses it and drops overlapping combinations.
    /// Here overlapping branches are pruned early but still counted, so the
    /// result is identical.
    fn exclusive_cartesian(t: &Tokenizer, args: &[Vec<Pair>]) -> Vec<Vec<Pair>> {
        let Some(max) = args.len().checked_sub(1) else {
            return Vec::new();
        };
        // upstream only reaches the last list if it is within the recursion limit
        if max > MAX_RECURSION {
            return Vec::new();
        }
        // leaves below depth i: product of the remaining list lengths
        let mut leaves_below = vec![1usize; args.len() + 1];
        for i in (0..args.len()).rev() {
            leaves_below[i] = leaves_below[i + 1].saturating_mul(args[i].len());
        }

        struct State<'a> {
            t: &'a Tokenizer,
            args: &'a [Vec<Pair>],
            leaves_below: Vec<usize>,
            max: usize,
            generated: usize,
            out: Vec<Vec<Pair>>,
        }

        fn helper(s: &mut State, solution: &mut Vec<Pair>, i: usize) {
            for j in 0..s.args[i].len() {
                if s.generated >= MAX_SOLUTIONS {
                    return;
                }
                let pair = s.args[i][j];
                let is_empty = pair.span == EMPTY;
                let overlaps = !is_empty
                    && solution
                        .iter()
                        .any(|p| p.span(s.t).intersects(pair.span(s.t)));
                if !is_empty {
                    solution.push(pair);
                }
                if i == s.max {
                    if !solution.is_empty() {
                        s.generated += 1;
                        if !overlaps {
                            s.out.push(solution.clone());
                        }
                    }
                } else if overlaps {
                    // every leaf below contains this overlap
                    s.generated = s
                        .generated
                        .saturating_add(s.leaves_below[i + 1])
                        .min(MAX_SOLUTIONS);
                } else {
                    helper(s, solution, i + 1);
                }
                if !is_empty {
                    solution.pop();
                }
            }
        }

        let mut state = State {
            t,
            args,
            leaves_below,
            max,
            generated: 0,
            out: Vec::new(),
        };
        helper(&mut state, &mut Vec::new(), 0);
        let mut out = state.out;
        out.reverse();
        out
    }
}

impl Solver for ExclusiveCartesianSolver {
    fn solve(&self, t: &mut Tokenizer) {
        let mut lists: Vec<Vec<Pair>> = generate_hash_map(t, false, true)
            .into_iter()
            .map(|(_, pairs)| pairs)
            .collect();
        lists.reverse();
        let solutions = Self::exclusive_cartesian(t, &lists);
        t.solutions
            .extend(solutions.into_iter().map(|pairs| Solution {
                pairs,
                ..Default::default()
            }));
    }
}

const ADMIN: &[Kind] = &[Kind::Locality, Kind::Region, Kind::Country];

/// Drops admin areas that come before the last non-admin, non-postcode
/// component (upstream `LeadingAreaDeclassifier`).
pub(crate) struct LeadingAreaDeclassifier;

impl Solver for LeadingAreaDeclassifier {
    fn solve(&self, t: &mut Tokenizer) {
        let mut solutions = std::mem::take(&mut t.solutions);
        for s in &mut solutions {
            let mut last_non_admin_end = 0;
            for p in &s.pairs {
                let kind = p.kind(t);
                if !ADMIN.contains(&kind) && kind != Kind::Postcode {
                    last_non_admin_end = p.span(t).end;
                }
            }
            s.pairs
                .retain(|p| !(ADMIN.contains(&p.kind(t)) && p.span(t).end < last_non_admin_end));
        }
        solutions.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for s in &mut solutions {
            s.pairs.sort_by_key(|p| p.span(t).start);
        }
        t.solutions = solutions;
    }
}

/// Classifications more granular than a street, excluded from intersections.
const MORE_GRANULAR_THAN_STREET: &[Kind] =
    &[Kind::HouseNumber, Kind::Unit, Kind::UnitType, Kind::Venue];

/// Adds intersection solutions with two streets, which the exclusive
/// cartesian product can't produce (upstream `MultiStreetSolver`).
pub(crate) struct MultiStreetSolver;

impl Solver for MultiStreetSolver {
    fn solve(&self, t: &mut Tokenizer) {
        let map = generate_hash_map(t, true, false);
        let get = |kind: Kind| {
            map.iter()
                .find(|(k, _)| *k == kind)
                .map(|(_, pairs)| pairs.as_slice())
                .unwrap_or(&[])
        };
        let (multis, street_pairs) = (get(Kind::MultiStreet), get(Kind::Street));
        if multis.is_empty() || street_pairs.len() < 2 {
            return;
        }
        let multi = multis[0].span(t);
        let streets: Vec<Pair> = street_pairs
            .iter()
            .copied()
            .filter(|s| s.span(t).intersects(multi))
            .collect();
        if streets.len() < 2 {
            return;
        }

        // solutions with a street inside the multistreet, truncated to the
        // pairs from that street on, minus more granular components
        let mut seen = std::collections::HashSet::new();
        let mut candidates: Vec<Solution> = Vec::new();
        for solution in &t.solutions {
            if !solution
                .pairs
                .iter()
                .any(|p| p.kind(t) == Kind::Street && p.span(t).intersects(multi))
            {
                continue;
            }
            let street = solution
                .first(t, Kind::Street)
                .expect("has a street")
                .span(t)
                .start;
            let mut truncated = solution.copy();
            truncated.pairs.retain(|p| {
                p.span(t).start >= street && !MORE_GRANULAR_THAN_STREET.contains(&p.kind(t))
            });
            let key: Vec<String> = truncated
                .pairs
                .iter()
                .map(|p| format!("{}:{}", p.kind(t).label(), p.span(t).norm))
                .collect();
            if seen.insert(key.join("_")) {
                candidates.push(truncated);
            }
        }

        for truncated in candidates {
            for &street in &streets {
                if truncated
                    .pairs
                    .iter()
                    .all(|p| !p.span(t).intersects(street.span(t)))
                {
                    let mut intersection = truncated.copy();
                    intersection.pairs.push(street);
                    t.solutions.push(intersection);
                }
            }
        }
    }
}

/// Removes solutions covered by a better or earlier solution with the same
/// classifications (upstream `SubsetFilter`).
pub(crate) struct SubsetFilter;

impl Solver for SubsetFilter {
    fn solve(&self, t: &mut Tokenizer) {
        let mut solutions = std::mem::take(&mut t.solutions);
        let mut i = 0;
        while i < solutions.len() {
            let keeper = solutions[i].clone();
            let mut j = 0;
            solutions.retain(|s| {
                let index = j;
                j += 1;
                index <= i || (keeper.score >= s.score && !keeper.covers_same_classification(t, s))
            });
            i += 1;
        }
        t.solutions = solutions;
    }
}

/// Removes solutions consisting of exactly one of the given combinations of
/// classifications (upstream `InvalidSolutionFilter`).
pub(crate) struct InvalidSolutionFilter {
    patterns: Vec<Vec<&'static str>>,
}

impl InvalidSolutionFilter {
    pub fn new(patterns: &[&[Kind]]) -> Self {
        let patterns = patterns
            .iter()
            .map(|p| {
                let mut names: Vec<&'static str> = p.iter().map(|k| k.class_name()).collect();
                names.sort_unstable();
                names
            })
            .collect();
        InvalidSolutionFilter { patterns }
    }
}

impl Solver for InvalidSolutionFilter {
    fn solve(&self, t: &mut Tokenizer) {
        let mut solutions = std::mem::take(&mut t.solutions);
        solutions.retain(|s| {
            let mut names: Vec<&str> = s.pairs.iter().map(|p| p.kind(t).class_name()).collect();
            names.sort_unstable();
            !self.patterns.contains(&names)
        });
        t.solutions = solutions;
    }
}

/// Where an ordering filter looks for the subject relative to the object.
#[derive(Clone, Copy)]
enum Order {
    /// Upstream `MustNotFollowFilter`: drop `object` if any `subject` starts
    /// before the first `object` ends.
    MustNotFollow,
    /// Upstream `MustNotPreceedFilter`: drop `object` if any `subject` starts
    /// after the first `object` ends.
    MustNotPrecede,
}

/// Removes the `object` classification from solutions where it is in the
/// wrong order relative to `subject`, dropping solutions left empty.
pub(crate) struct OrderFilter {
    order: Order,
    object: Kind,
    subject: Kind,
}

impl OrderFilter {
    pub fn must_not_follow(object: Kind, subject: Kind) -> Self {
        OrderFilter {
            order: Order::MustNotFollow,
            object,
            subject,
        }
    }
    pub fn must_not_precede(object: Kind, subject: Kind) -> Self {
        OrderFilter {
            order: Order::MustNotPrecede,
            object,
            subject,
        }
    }
}

impl Solver for OrderFilter {
    fn solve(&self, t: &mut Tokenizer) {
        let mut solutions = std::mem::take(&mut t.solutions);
        solutions.retain_mut(|s| {
            let (Some(object), subjects) = (s.first(t, self.object), s.all(t, self.subject)) else {
                return true;
            };
            let object_end = object.span(t).end;
            let wrong = subjects.iter().any(|p| match self.order {
                Order::MustNotFollow => p.span(t).start < object_end,
                Order::MustNotPrecede => p.span(t).start > object_end,
            });
            if wrong {
                s.without(t, self.object);
                return !s.pairs.is_empty();
            }
            true
        });
        t.solutions = solutions;
    }
}

/// Penalizes the house number being on the unusual side of the street for
/// the street's language (upstream `HouseNumberPositionPenalty`).
pub(crate) struct HouseNumberPositionPenalty;

const BASE_HOUSENUMBER_PENALTY: f64 = 0.05;

impl HouseNumberPositionPenalty {
    fn number_last(lang: &str) -> Option<f64> {
        match lang {
            "de" | "sl" | "pl" | "bs" | "hr" | "nl" | "cs" | "da" | "fi" | "el" | "is" | "it"
            | "nb" | "pt" | "sv" | "sk" | "tr" | "ro" | "hu" => Some(BASE_HOUSENUMBER_PENALTY),
            // Guatemala & Honduras do not flip their house numbers
            "es" => Some(BASE_HOUSENUMBER_PENALTY / 2.0),
            _ => None,
        }
    }

    fn number_first(lang: &str) -> Option<f64> {
        match lang {
            "en" => Some(BASE_HOUSENUMBER_PENALTY),
            // Switzerland and Andorra have some French streets
            "fr" => Some(BASE_HOUSENUMBER_PENALTY / 2.0),
            _ => None,
        }
    }
}

impl Solver for HouseNumberPositionPenalty {
    fn solve(&self, t: &mut Tokenizer) {
        let mut solutions = std::mem::take(&mut t.solutions);
        for s in &mut solutions {
            let (Some(housenumber), Some(street)) =
                (s.first(t, Kind::HouseNumber), s.first(t, Kind::Street))
            else {
                continue;
            };
            let Some(langs) = &street.classification(t).langs else {
                continue;
            };
            if langs.len() != 1 || langs[0] == "all" {
                continue;
            }
            let (hn, st) = (housenumber.span(t).start, street.span(t).start);
            if let Some(penalty) = Self::number_last(&langs[0]).filter(|_| hn < st) {
                s.penalty += penalty;
            } else if let Some(penalty) = Self::number_first(&langs[0]).filter(|_| st < hn) {
                s.penalty += penalty;
            }
        }
        t.solutions = solutions;
    }
}

/// Penalizes a postcode with a single street but no house number, e.g.
/// `rua godinho de faria 1200` (upstream `PostcodePositionPenalty`).
pub(crate) struct PostcodePositionPenalty;

impl Solver for PostcodePositionPenalty {
    fn solve(&self, t: &mut Tokenizer) {
        let mut solutions = std::mem::take(&mut t.solutions);
        for s in &mut solutions {
            if s.first(t, Kind::Postcode).is_none() || s.first(t, Kind::HouseNumber).is_some() {
                continue;
            }
            if s.all(t, Kind::Street).len() == 1 {
                s.penalty += 0.1;
            }
        }
        t.solutions = solutions;
    }
}

/// Requires a street for a house number (else drops the house number) and
/// at most two characters between them (upstream `TokenDistanceFilter`).
pub(crate) struct TokenDistanceFilter;

impl Solver for TokenDistanceFilter {
    fn solve(&self, t: &mut Tokenizer) {
        let mut solutions = std::mem::take(&mut t.solutions);
        solutions.retain_mut(|s| {
            let housenumber = s.first(t, Kind::HouseNumber);
            let street = s.first(t, Kind::Street);
            match (housenumber, street) {
                (Some(_), None) => {
                    s.without(t, Kind::HouseNumber);
                    !s.pairs.is_empty()
                }
                (Some(hn), Some(st)) => st.span(t).distance(hn.span(t)) <= 2,
                _ => true,
            }
        });
        t.solutions = solutions;
    }
}

/// Drops unit types without a unit (upstream `OrphanedUnitTypeDeclassifier`).
pub(crate) struct OrphanedUnitTypeDeclassifier;

impl Solver for OrphanedUnitTypeDeclassifier {
    fn solve(&self, t: &mut Tokenizer) {
        let mut solutions = std::mem::take(&mut t.solutions);
        solutions.retain_mut(|s| {
            if s.first(t, Kind::UnitType).is_none() || s.first(t, Kind::Unit).is_some() {
                return true;
            }
            s.without(t, Kind::UnitType);
            !s.pairs.is_empty()
        });
        t.solutions = solutions;
    }
}
