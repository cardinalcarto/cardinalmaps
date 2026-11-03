//! Composite classification (upstream `CompositeClassifier` and
//! `classifier/scheme/*.js`): sequences of adjacent phrases with given
//! classifications, e.g. `{alpha} {street_suffix}` → street, are classified
//! as a whole.

use super::Classifier;
use crate::classification::{Classification, Kind};
use crate::tokenization::{SpanId, Tokenizer};

/// One position of a scheme: the phrase there must have one of `is` and none
/// of `not`; optionally the matched phrase is itself classified.
pub(crate) struct Element {
    pub is: &'static [Kind],
    pub not: &'static [Kind],
    pub class: Option<(Kind, f64)>,
}

/// A sequence of elements; matching phrases get `class` with `confidence`.
pub(crate) struct Scheme {
    pub confidence: f64,
    pub class: Kind,
    pub elements: Vec<Element>,
}

/// Runs a list of schemes over each section.
pub(crate) struct CompositeClassifier {
    schemes: Vec<Scheme>,
}

impl CompositeClassifier {
    pub fn new(schemes: Vec<Scheme>) -> Self {
        CompositeClassifier { schemes }
    }
}

/// Upstream `CompositeClassifier.match`.
fn matches(t: &Tokenizer, element: &Element, phrase: SpanId) -> bool {
    let p = t.span(phrase);
    let single_child = (p.children.len() == 1).then(|| t.span(p.children[0]));
    if !p.has_any(element.is) && !single_child.is_some_and(|c| c.has_any(element.is)) {
        return false;
    }
    if p.has_any(element.not) {
        return false;
    }
    !single_child.is_some_and(|c| c.has_any(element.not))
}

/// Whether `next` may follow `curr` in a composite: adjacent words, and not
/// a street starting with a street prefix (`A + Ave B`).
fn may_follow(t: &Tokenizer, curr: SpanId, next: SpanId) -> bool {
    let (c, n) = (t.span(curr), t.span(next));
    let (Some(last), Some(first)) = (c.child_last, n.child_first) else {
        return false;
    };
    if !t.span(last).next.contains(&first) {
        return false;
    }
    // the reverse check upstream also performs; edges are symmetric
    if !t.span(first).prev.contains(&last) {
        return false;
    }
    !(n.has(Kind::Street)
        && n.children
            .first()
            .is_some_and(|&f| t.span(f).has(Kind::StreetPrefix)))
}

/// Upstream's cartesian product of the candidate lists, filtered to
/// adjacent sequences, in the same (lexicographic) order. Generated
/// depth-first with pruning, which yields exactly the filtered product
/// because every filter condition only relates consecutive elements.
fn composites(t: &Tokenizer, candidates: &[Vec<SpanId>]) -> Vec<Vec<SpanId>> {
    fn rec(
        t: &Tokenizer,
        candidates: &[Vec<SpanId>],
        current: &mut Vec<SpanId>,
        out: &mut Vec<Vec<SpanId>>,
    ) {
        let depth = current.len();
        if depth == candidates.len() {
            out.push(current.clone());
            return;
        }
        for &c in &candidates[depth] {
            if let Some(&prev) = current.last()
                && !may_follow(t, prev, c)
            {
                continue;
            }
            current.push(c);
            rec(t, candidates, current, out);
            current.pop();
        }
    }
    let mut out = Vec::new();
    rec(
        t,
        candidates,
        &mut Vec::with_capacity(candidates.len()),
        &mut out,
    );
    out
}

impl Classifier for CompositeClassifier {
    fn classify(&self, t: &mut Tokenizer) {
        for i in 0..t.sections.len() {
            let section = t.sections[i];
            // shorter phrases first; upstream sorts the section's phrase
            // array in place (stably), which later passes observe
            let mut phrases = std::mem::take(&mut t.span_mut(section).phrases);
            phrases.sort_by_key(|&p| crate::js::len(&t.span(p).norm));
            t.span_mut(section).phrases = phrases.clone();

            for scheme in &self.schemes {
                let candidates: Vec<Vec<SpanId>> = scheme
                    .elements
                    .iter()
                    .map(|e| {
                        phrases
                            .iter()
                            .copied()
                            .filter(|&p| matches(t, e, p))
                            .collect()
                    })
                    .collect();
                if candidates.iter().any(Vec::is_empty) {
                    continue;
                }
                let found = composites(t, &candidates);
                if found.is_empty() {
                    continue;
                }

                // phrases spanning exactly the same range as each composite
                let mut super_phrases = Vec::new();
                for c in &found {
                    let start = t.span(c[0]).start;
                    let end = t.span(*c.last().expect("non-empty composite")).end;
                    super_phrases.extend(
                        phrases
                            .iter()
                            .copied()
                            .filter(|&p| t.span(p).start == start && t.span(p).end == end),
                    );
                }
                for p in super_phrases {
                    // spread the children's languages to the parent
                    let mut langs: Vec<String> = Vec::new();
                    for &child in &t.span(p).children {
                        for c in &t.span(child).classifications {
                            if let Some(l) = &c.langs {
                                let key = l.join(",");
                                if !langs.contains(&key) {
                                    langs.push(key);
                                }
                            }
                        }
                    }
                    t.span_mut(p).classify(Classification::with_langs(
                        scheme.class,
                        scheme.confidence,
                        langs,
                    ));
                }
                for c in &found {
                    for (element, &phrase) in scheme.elements.iter().zip(c) {
                        if let Some((kind, confidence)) = element.class {
                            t.span_mut(phrase)
                                .classify(Classification::new(kind, confidence));
                        }
                    }
                }
            }
        }
    }
}

/// The schemes of upstream `classifier/scheme/*.js`.
pub(crate) mod schemes {
    use super::{Element, Scheme};
    use crate::classification::Kind::{self, *};

    const fn el(is: &'static [Kind], not: &'static [Kind]) -> Element {
        Element {
            is,
            not,
            class: None,
        }
    }

    const fn el_class(
        is: &'static [Kind],
        not: &'static [Kind],
        class: Kind,
        confidence: f64,
    ) -> Element {
        Element {
            is,
            not,
            class: Some((class, confidence)),
        }
    }

    fn scheme(confidence: f64, class: Kind, elements: Vec<Element>) -> Scheme {
        Scheme {
            confidence,
            class,
            elements,
        }
    }

    const ANO: &[Kind] = &[Alpha, Numeric, Ordinal];

    /// `scheme/person.js`
    pub fn person() -> Vec<Scheme> {
        let given = || el(&[GivenName], &[Street, Intersection]);
        vec![
            // Anne Marie
            scheme(
                0.25,
                GivenName,
                vec![given(), el(&[GivenName], &[Street, StreetPrefix, StopWord])],
            ),
            // Georges Bizet
            scheme(
                0.5,
                Person,
                vec![given(), el(&[Surname], &[Street, StreetPrefix, StopWord])],
            ),
            // Rose de Lima
            scheme(
                0.5,
                Person,
                vec![
                    given(),
                    el(&[StopWord], &[Street, Intersection]),
                    el(&[Surname], &[Street, StreetPrefix, StopWord]),
                ],
            ),
            // Raul Leite Magalhães, Donald W. Reynolds
            scheme(
                0.5,
                Person,
                vec![
                    given(),
                    el(
                        &[GivenName, Surname, MiddleInitial],
                        &[Street, Intersection],
                    ),
                    el(&[Surname], &[Street, StreetPrefix, StopWord]),
                ],
            ),
            // unknown surname
            scheme(
                0.1,
                Person,
                vec![given(), el(&[Alpha], &[Street, StreetPrefix, StopWord])],
            ),
            scheme(
                0.1,
                Person,
                vec![
                    given(),
                    el(&[StopWord], &[Street, Intersection]),
                    el(&[Alpha], &[Street, StreetPrefix, StopWord]),
                ],
            ),
        ]
    }

    /// `scheme/street_name.js`
    pub fn street_name() -> Vec<Scheme> {
        vec![
            // dos Fiéis, a Santa
            scheme(
                0.5,
                StreetName,
                vec![
                    el(&[StopWord, SingleAlpha], &[Directional, Intersection]),
                    el(&[Alpha, Person], &[Street, Intersection, StreetSuffix]),
                ],
            ),
            // Academia das Ciências, Sol a Santa
            scheme(
                0.5,
                StreetName,
                vec![
                    el(
                        &[Alpha],
                        &[Street, Intersection, StopWord, SingleAlpha, StreetPrefix],
                    ),
                    el(&[StopWord, SingleAlpha], &[Directional]),
                    el(&[Alpha, Person], &[Street, Intersection, StreetSuffix]),
                ],
            ),
            // du 4 septembre
            scheme(
                0.5,
                StreetName,
                vec![
                    el(&[StopWord], &[Intersection]),
                    el(&[Numeric], &[Postcode]),
                    el(&[Alpha], &[Street, Intersection, Locality]),
                ],
            ),
            // dos Fiéis de Deus
            scheme(
                0.5,
                StreetName,
                vec![
                    el(&[StreetName], &[Street, Intersection]),
                    el(&[StreetName], &[Street, Intersection]),
                ],
            ),
        ]
    }

    /// `scheme/street.js`
    pub fn street() -> Vec<Scheme> {
        const SI: &[Kind] = &[Street, Intersection];
        const SSP: &[Kind] = &[Street, StreetPrefix];
        let prefix = || el(&[StreetPrefix], SI);
        vec![
            // Main Street
            scheme(
                0.82,
                Street,
                vec![el(&[Alpha], SI), el(&[StreetSuffix], SI)],
            ),
            // Rue Montmartre, Boulevard Charles De Gaulle
            scheme(
                0.88,
                Street,
                vec![prefix(), el(&[Alpha, Person, StreetName], SI)],
            ),
            // 26th Street
            scheme(
                0.87,
                Street,
                vec![el(&[Ordinal], SI), el(&[StreetSuffix], SI)],
            ),
            // 26 Street
            scheme(
                0.86,
                Street,
                vec![
                    el(&[Numeric], SI),
                    el(&[StreetSuffix], &[Street, Intersection, RoadType]),
                ],
            ),
            // The Stables
            scheme(0.82, Street, vec![el(&[StopWord], SI), el(&[Place], SI)]),
            // SW 26th
            scheme(
                0.77,
                Street,
                vec![el(&[Directional], SI), el(&[Ordinal], SI)],
            ),
            // St Kilda Road
            scheme(
                0.85,
                Street,
                vec![el(&[PersonalSuffix], SI), el(&[StreetSuffix], SI)],
            ),
            // N Main
            scheme(
                0.98,
                Street,
                vec![
                    el(&[Directional], &[Intersection, Street]),
                    el(&[Street], &[Intersection, Directional]),
                ],
            ),
            // Martin Luther King Blvd.
            scheme(
                0.82,
                Street,
                vec![el(&[Person], SI), el(&[StreetSuffix], SI)],
            ),
            // Martin Luther King Jr. Blvd.
            scheme(
                0.81,
                Street,
                vec![
                    el(&[Person], SI),
                    el(&[PersonalSuffix], SI),
                    el(&[StreetSuffix], SI),
                ],
            ),
            // Rue De Paris
            scheme(
                0.8,
                Street,
                vec![prefix(), el(&[StopWord], SI), el(&[Alpha, Person], SSP)],
            ),
            // Boulevard De La Paix
            scheme(
                0.79,
                Street,
                vec![
                    prefix(),
                    el(&[StopWord], SI),
                    el(&[StopWord], SI),
                    el(&[Alpha], SSP),
                ],
            ),
            // Rue Saint Anne
            scheme(
                0.91,
                Street,
                vec![
                    prefix(),
                    el(&[PersonalTitle], SI),
                    el(&[Alpha, GivenName, Person], SSP),
                ],
            ),
            // Aleja Wojska Polskiego
            scheme(
                0.91,
                Street,
                vec![
                    prefix(),
                    el(&[Place], SI),
                    el(&[Alpha, GivenName, Person], SSP),
                ],
            ),
            // Aleja 11 Listopada
            scheme(
                0.84,
                Street,
                vec![
                    prefix(),
                    el(&[Numeric], SI),
                    el(&[Alpha, GivenName, Person], SSP),
                ],
            ),
            // Boulevard du Général Charles De Gaulle
            scheme(
                0.81,
                Street,
                vec![
                    prefix(),
                    el(&[StopWord], SI),
                    el(&[PersonalTitle], SI),
                    el(&[Alpha, GivenName, Person], SSP),
                ],
            ),
            // Avenue Aristide Briand, Allée Victor Hugo
            scheme(
                0.92,
                Street,
                vec![prefix(), el(&[GivenName, Alpha], SI), el(&[Surname], SSP)],
            ),
            // Broadway Market
            scheme(
                0.80,
                Street,
                vec![el(&[StreetProperName], SI), el(&[StreetSuffix], SI)],
            ),
            // Broadway
            scheme(0.82, Street, vec![el(&[StreetProperName], SI)]),
            // +++ Main Street
            scheme(
                0.84,
                Street,
                vec![
                    el(&[Alpha], &[Street, Intersection, StopWord]),
                    el(&[Street], &[Directional]),
                ],
            ),
            // Highway 27
            scheme(
                0.84,
                Street,
                vec![el(&[RoadType, Toponym], &[]), el(&[Numeric], &[])],
            ),
            // +++ +++ Main Street (identical to "+++ Main Street" upstream)
            scheme(
                0.84,
                Street,
                vec![
                    el(&[Alpha], &[Street, Intersection, StopWord]),
                    el(&[Street], &[Directional]),
                ],
            ),
            // Main Street West
            scheme(
                0.88,
                Street,
                vec![
                    el(&[Street], &[Directional]),
                    el(
                        &[Directional],
                        &[Street, Intersection, EndTokenSingleCharacter],
                    ),
                ],
            ),
            // West Main Street
            scheme(
                0.88,
                Street,
                vec![
                    el(
                        &[Directional],
                        &[Street, Intersection, EndTokenSingleCharacter],
                    ),
                    el(&[Street], &[Directional]),
                ],
            ),
        ]
    }

    /// `scheme/venue.js`
    pub fn venue() -> Vec<Scheme> {
        const PV: &[Kind] = &[Place, Venue];
        vec![
            // University Hospital
            scheme(0.9, Venue, vec![el(PV, &[Street]), el(PV, &[Street])]),
            // +++ Park
            scheme(
                0.7,
                Venue,
                vec![
                    el(&[Alpha], &[Street, Intersection, StopWord]),
                    el(PV, &[Street]),
                ],
            ),
            // Mt +++ Park
            scheme(0.8, Venue, vec![el(PV, &[Street]), el(PV, &[])]),
            // Air & Space Museum
            scheme(
                0.8,
                Venue,
                vec![
                    el(&[Alpha], &[Street, Intersection, StopWord]),
                    el(&[StopWord], &[Street]),
                    el(PV, &[]),
                ],
            ),
            // National Air & Space Museum
            scheme(
                0.8,
                Venue,
                vec![el(&[Alpha], &[Street, Intersection, StopWord]), el(PV, &[])],
            ),
            // Stop 10792
            scheme(
                0.8,
                Venue,
                vec![
                    el(PV, &[Street, Intersection, StopWord]),
                    el(&[Numeric], &[]),
                ],
            ),
            // University of Somewhere
            scheme(
                0.8,
                Venue,
                vec![
                    el(PV, &[Street]),
                    el(&[StopWord], &[Street]),
                    el(&[Area], &[Street]),
                ],
            ),
            // Ecole Jules Vernes
            scheme(
                0.8,
                Venue,
                vec![el(PV, &[Street]), el(&[Person], &[Street])],
            ),
            // Donald W Reynolds Stadium
            scheme(
                0.82,
                Venue,
                vec![el(&[Person], &[Street]), el(PV, &[Street])],
            ),
            // ZAC du Pré
            scheme(
                0.8,
                Venue,
                vec![el(PV, &[Street]), el(&[StreetName], &[Street])],
            ),
            // ZAC de la Tuilerie
            scheme(
                0.8,
                Venue,
                vec![
                    el(PV, &[Street]),
                    el(&[StopWord], &[Street]),
                    el(&[StreetName], &[Street]),
                ],
            ),
            // ZA Entraigues
            scheme(
                0.7,
                Venue,
                vec![el(PV, &[Street, StreetSuffix]), el(&[Alpha], &[Street])],
            ),
        ]
    }

    /// `scheme/intersection.js`
    pub fn intersection() -> Vec<Scheme> {
        const INTERSECTION: &[Kind] = &[Intersection];
        let marker = || el(INTERSECTION, &[Street, StreetSuffix]);
        vec![
            // SW 6th & Pine
            scheme(
                1.0,
                MultiStreet,
                vec![
                    el(&[Directional], &[Intersection, StreetSuffix]),
                    el(ANO, &[Intersection, StreetSuffix]),
                    marker(),
                    el(ANO, INTERSECTION),
                ],
            ),
            // Foo St and Bar St
            scheme(
                1.0,
                MultiStreet,
                vec![
                    el_class(ANO, &[Intersection, StreetSuffix], Street, 0.81),
                    marker(),
                    el_class(ANO, INTERSECTION, Street, 0.82),
                ],
            ),
            // Foo and Bar St
            scheme(
                1.0,
                MultiStreet,
                vec![
                    el_class(
                        &[Alpha],
                        &[Intersection, Street, StreetSuffix],
                        Street,
                        0.53,
                    ),
                    marker(),
                    el(ANO, INTERSECTION),
                ],
            ),
            // Foo St and Bar
            scheme(
                1.0,
                MultiStreet,
                vec![
                    el(ANO, INTERSECTION),
                    marker(),
                    el_class(ANO, &[Intersection, Street], Street, 0.56),
                ],
            ),
            // Foo and Bar
            scheme(
                1.0,
                MultiStreet,
                vec![
                    el_class(
                        &[Alpha],
                        &[Intersection, Street, StreetSuffix],
                        Street,
                        0.57,
                    ),
                    marker(),
                    el_class(
                        &[Alpha],
                        &[Intersection, Street, StreetSuffix],
                        Street,
                        0.58,
                    ),
                ],
            ),
        ]
    }

    /// `scheme/subdivision.js`
    pub fn subdivision() -> Vec<Scheme> {
        vec![
            // 10 bis / 10 ter
            scheme(
                0.99,
                HouseNumber,
                vec![
                    el(&[HouseNumber], &[Intersection]),
                    el(&[Subdivision], &[Intersection, Punctuation]),
                ],
            ),
        ]
    }
}
