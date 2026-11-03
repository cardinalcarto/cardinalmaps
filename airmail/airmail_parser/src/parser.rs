//! The address parser pipeline (upstream `parser/Parser.js` and
//! `parser/AddressParser.js`).

use std::cmp::Ordering;

use crate::classification::Kind;
use crate::classifier::{
    self, Classifier, CompositeClassifier, DictionaryPhraseClassifier, DictionaryWordClassifier,
};
use crate::solver::{self, OrderFilter, Solution, Solver};
use crate::tokenization::Tokenizer;

/// Options for [`AddressParser`].
#[derive(Clone, Debug)]
pub struct ParserOptions {
    /// Solutions kept after each solver (upstream `max_solutions`, default 10).
    pub max_solutions: usize,
    /// Also run upstream's unit classifiers (`apt 2`, `#3b`, `U12`), which
    /// upstream's `AddressParser` defines but does not enable. Off by
    /// default, matching upstream.
    pub unit_classifiers: bool,
}

impl Default for ParserOptions {
    fn default() -> Self {
        ParserOptions {
            max_solutions: 10,
            unit_classifiers: false,
        }
    }
}

/// Upstream's `AddressParser`: its classifiers and solvers in upstream
/// order. Construction loads all dictionaries (see the crate docs for the
/// cost); share one instance, e.g. via [`crate::parser()`].
pub struct AddressParser {
    classifiers: Vec<Box<dyn Classifier>>,
    solvers: Vec<Box<dyn Solver>>,
    max_solutions: usize,
}

impl Default for AddressParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AddressParser {
    /// A parser configured exactly like upstream's `AddressParser`.
    pub fn new() -> Self {
        Self::with_options(ParserOptions::default())
    }

    /// A parser with non-default options.
    pub fn with_options(options: ParserOptions) -> Self {
        let mut classifiers: Vec<Box<dyn Classifier>> = vec![
            // generic word classifiers
            Box::new(classifier::AlphaNumericClassifier::new()),
            Box::new(classifier::TokenPositionClassifier),
        ];
        if options.unit_classifiers {
            classifiers.push(Box::new(DictionaryWordClassifier::unit_type()));
            classifiers.push(Box::new(classifier::UnitTypeUnitClassifier::new()));
            classifiers.push(Box::new(classifier::UnitClassifier::new()));
        }
        let rest: Vec<Box<dyn Classifier>> = vec![
            // word classifiers
            Box::new(classifier::HouseNumberClassifier::new()),
            Box::new(classifier::PostcodeClassifier::new()),
            Box::new(classifier::StreetPrefixClassifier::new()),
            Box::new(classifier::StreetSuffixClassifier::new()),
            Box::new(classifier::StreetProperNameClassifier),
            Box::new(DictionaryWordClassifier::road_type()),
            Box::new(DictionaryWordClassifier::toponym()),
            Box::new(classifier::CompoundStreetClassifier::new()),
            Box::new(classifier::DirectionalClassifier::new()),
            Box::new(classifier::OrdinalClassifier::new()),
            Box::new(DictionaryWordClassifier::stop_word()),
            // phrase classifiers
            Box::new(classifier::IntersectionClassifier),
            Box::new(DictionaryPhraseClassifier::person()),
            Box::new(DictionaryPhraseClassifier::given_name()),
            Box::new(DictionaryPhraseClassifier::surname()),
            Box::new(classifier::MiddleInitialClassifier),
            Box::new(DictionaryPhraseClassifier::personal_suffix()),
            Box::new(DictionaryPhraseClassifier::personal_title()),
            Box::new(DictionaryPhraseClassifier::chain()),
            Box::new(classifier::PlaceClassifier::new()),
            Box::new(classifier::WhosOnFirstClassifier::new()),
            Box::new(DictionaryPhraseClassifier::subdivision()),
            // composite classifiers
            Box::new(CompositeClassifier::new(classifier::schemes::person())),
            Box::new(CompositeClassifier::new(classifier::schemes::street_name())),
            Box::new(CompositeClassifier::new(classifier::schemes::street())),
            Box::new(CompositeClassifier::new(classifier::schemes::venue())),
            Box::new(CompositeClassifier::new(classifier::schemes::intersection())),
            Box::new(CompositeClassifier::new(classifier::schemes::subdivision())),
            // additional classifiers which act on unclassified tokens
            Box::new(classifier::CentralEuropeanStreetNameClassifier),
        ];
        classifiers.extend(rest);

        use Kind::*;
        let invalid: &[&[Kind]] = &[
            &[HouseNumber, Locality],
            &[HouseNumber, Locality, Region],
            &[HouseNumber, Locality, Country],
            &[HouseNumber, Locality, Region, Country],
            &[HouseNumber, Region],
            &[HouseNumber, Region, Country],
            &[HouseNumber, Country],
            &[HouseNumber, Postcode],
            &[HouseNumber, Postcode, Locality],
            &[HouseNumber, Postcode, Region],
            &[HouseNumber, Postcode, Country],
            &[Venue, HouseNumber],
            &[Venue, Postcode],
        ];
        let solvers: Vec<Box<dyn Solver>> = vec![
            Box::new(solver::ExclusiveCartesianSolver),
            Box::new(solver::LeadingAreaDeclassifier),
            Box::new(solver::MultiStreetSolver),
            Box::new(solver::SubsetFilter),
            Box::new(solver::InvalidSolutionFilter::new(invalid)),
            Box::new(OrderFilter::must_not_follow(Venue, HouseNumber)),
            Box::new(OrderFilter::must_not_follow(Venue, Street)),
            Box::new(OrderFilter::must_not_follow(Venue, Locality)),
            Box::new(OrderFilter::must_not_follow(Venue, Region)),
            Box::new(OrderFilter::must_not_follow(Venue, Country)),
            Box::new(OrderFilter::must_not_follow(Venue, Postcode)),
            Box::new(OrderFilter::must_not_precede(Postcode, HouseNumber)),
            Box::new(OrderFilter::must_not_precede(Postcode, Street)),
            Box::new(OrderFilter::must_not_precede(Locality, HouseNumber)),
            Box::new(OrderFilter::must_not_precede(Locality, Street)),
            Box::new(OrderFilter::must_not_precede(Region, HouseNumber)),
            Box::new(OrderFilter::must_not_precede(Region, Street)),
            Box::new(OrderFilter::must_not_precede(Country, Region)),
            Box::new(OrderFilter::must_not_precede(Country, Locality)),
            Box::new(OrderFilter::must_not_precede(Country, Postcode)),
            Box::new(OrderFilter::must_not_precede(Country, Street)),
            Box::new(OrderFilter::must_not_precede(Country, HouseNumber)),
            Box::new(OrderFilter::must_not_precede(Venue, Unit)),
            Box::new(OrderFilter::must_not_follow(Locality, Region)),
            Box::new(OrderFilter::must_not_follow(Locality, Country)),
            Box::new(solver::HouseNumberPositionPenalty),
            Box::new(solver::PostcodePositionPenalty),
            Box::new(solver::TokenDistanceFilter),
            Box::new(solver::OrphanedUnitTypeDeclassifier),
            Box::new(solver::SubsetFilter),
        ];
        AddressParser {
            classifiers,
            solvers,
            max_solutions: options.max_solutions,
        }
    }

    /// Tokenizes, classifies and solves `input`.
    pub(crate) fn run(&self, input: &str) -> Tokenizer {
        let mut t = Tokenizer::new(input);
        for c in &self.classifiers {
            c.classify(&mut t);
        }
        for s in &self.solvers {
            self.score_and_sort(&mut t);
            s.solve(&mut t);
        }
        self.score_and_sort(&mut t);
        t
    }

    /// Upstream `scoreAndSort`: rescore, order each solution's pairs by
    /// position, order solutions best first and keep `max_solutions`.
    fn score_and_sort(&self, t: &mut Tokenizer) {
        let mut solutions = std::mem::take(&mut t.solutions);
        for s in &mut solutions {
            s.compute_score(t);
        }
        for s in &mut solutions {
            s.pairs.sort_by_key(|p| p.span(t).start);
        }
        // stable, like V8's `Array.prototype.sort`
        solutions.sort_by(|a, b| compare(t, a, b));
        solutions.truncate(self.max_solutions);
        t.solutions = solutions;
    }
}

/// Upstream's solution comparator: score descending; on equal scores,
/// prefer the solution whose first admin-area span is a locality, then a
/// region, then a country.
///
/// Deviation: upstream returns "a first" whenever both first admin spans
/// are localities (likewise regions, countries). That is not a consistent
/// ordering, and V8's sort then swaps such ties on each of the ~30 sorts per
/// parse, so the winner depends on the parity of the number of solvers. Here
/// such ties are broken by the next admin span with the same rule, and fully
/// equal solutions keep their order. With this, `Indianapolis, IN`, `jasper
/// ga` and `… Oakland 94610-2737 CA` parse with a region, as upstream's tests
/// expect.
fn compare(t: &Tokenizer, a: &Solution, b: &Solution) -> Ordering {
    let by_score = b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal);
    if by_score != Ordering::Equal {
        return by_score;
    }
    let rank = |k: Option<Kind>| match k {
        Some(Kind::Locality) => 0,
        Some(Kind::Region) => 1,
        Some(Kind::Country) => 2,
        _ => 3,
    };
    let areas = |s: &Solution| -> Vec<Kind> {
        s.pairs
            .iter()
            .filter(|p| p.span(t).has(Kind::Area))
            .map(|p| p.kind(t))
            .collect()
    };
    let (aa, ab) = (areas(a), areas(b));
    for i in 0..aa.len().max(ab.len()) {
        let (ra, rb) = (rank(aa.get(i).copied()), rank(ab.get(i).copied()));
        if ra != rb || ra == 3 {
            return ra.cmp(&rb);
        }
    }
    Ordering::Equal
}
