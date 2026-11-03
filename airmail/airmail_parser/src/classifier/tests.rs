//! Classifier unit tests, ported from upstream's `classifier/*.test.js`.
//!
//! Upstream calls `classifier.each(span)` on a detached span; here the token
//! is tokenized on its own (or after a filler word where upstream passes a
//! non-zero position) and the classifier runs over the tokenizer.

use super::*;

fn run(c: &dyn Classifier, input: &str) -> Tokenizer {
    let mut t = Tokenizer::new(input);
    c.classify(&mut t);
    t
}

fn kinds_of(t: &Tokenizer, ids: impl Iterator<Item = SpanId>, body: &str) -> Vec<(Kind, f64)> {
    let ids: Vec<SpanId> = ids.collect();
    let id = ids
        .into_iter()
        .find(|&id| t.span(id).body == body)
        .unwrap_or_else(|| panic!("no span {body:?}"));
    t.span(id)
        .classifications
        .iter()
        .map(|c| (c.kind, c.confidence))
        .collect()
}

/// Classifications of the word `body` after running `c` on `input`.
fn word(c: &dyn Classifier, input: &str, body: &str) -> Vec<(Kind, f64)> {
    let t = run(c, input);
    let words = t
        .sections
        .iter()
        .flat_map(|&s| t.span(s).children.clone())
        .collect::<Vec<_>>();
    kinds_of(&t, words.into_iter(), body)
}

/// Classifications of the phrase `body` after running `c` on `input`.
fn phrase(c: &dyn Classifier, input: &str, body: &str) -> Vec<(Kind, f64)> {
    let t = run(c, input);
    let phrases = t
        .sections
        .iter()
        .flat_map(|&s| t.span(s).phrases.clone())
        .collect::<Vec<_>>();
    kinds_of(&t, phrases.into_iter(), body)
}

fn only(kind: Kind, confidence: f64) -> Vec<(Kind, f64)> {
    vec![(kind, confidence)]
}

#[test]
fn alpha_numeric() {
    let c = AlphaNumericClassifier::new();
    assert_eq!(word(&c, "1", "1"), only(Kind::Numeric, 1.0));
    assert_eq!(word(&c, "1a", "1a"), only(Kind::AlphaNumeric, 1.0));
    assert_eq!(
        word(&c, "a", "a"),
        vec![(Kind::Alpha, 1.0), (Kind::SingleAlpha, 1.0)]
    );
    assert_eq!(word(&c, "abc", "abc"), only(Kind::Alpha, 1.0));
    assert_eq!(word(&c, "&", "&"), only(Kind::Punctuation, 1.0));
    assert_eq!(word(&c, "#", "#"), only(Kind::Punctuation, 1.0));
}

#[test]
fn token_position() {
    let c = TokenPositionClassifier;
    let t = run(&c, "a b, c d");
    let words: Vec<_> = t
        .sections
        .iter()
        .flat_map(|&s| t.span(s).children.clone())
        .collect();
    let k = |b: &str| kinds_of(&t, words.iter().copied(), b);
    assert_eq!(k("a"), only(Kind::StartToken, 1.0));
    assert!(k("b").is_empty());
    assert!(k("c").is_empty());
    assert_eq!(
        k("d"),
        vec![(Kind::EndToken, 1.0), (Kind::EndTokenSingleCharacter, 1.0)]
    );
}

#[test]
fn house_numbers() {
    let c = HouseNumberClassifier::new();
    for (token, confidence) in [
        ("1", 1.0),
        ("12", 1.0),
        ("123", 1.0),
        ("1234", 0.9),
        ("12345", 0.2),
        ("1A", 1.0),
        ("12b", 1.0),
        ("123C", 1.0),
        ("1234d", 0.9),
        ("12345E", 0.2),
        ("1-1", 1.0),
        ("12-12b", 1.0),
        ("1/135", 1.0),
        ("1b/135", 1.0),
        ("6N23", 1.0),
        ("W350N5337", 1.0),
        ("N453", 1.0),
        ("10Д", 1.0),
    ] {
        assert_eq!(
            word(&c, token, token),
            only(Kind::HouseNumber, confidence),
            "{token}"
        );
    }
    for token in ["123456", "1AA", "abc", "A1", "12345-123456"] {
        assert!(word(&c, token, token).is_empty(), "{token}");
    }
}

#[test]
fn postcodes() {
    let c = PostcodeClassifier::new();
    for token in [
        "10010",
        "99577-0727",
        "10117",
        "6012",
        "2000",
        "75000",
        "E81DN",
        "100-0000",
        "101000",
        "18180-000",
        "1012 AB",
    ] {
        let body = token.split(' ').next().expect("word");
        if token.contains(' ') {
            // the Dutch pattern allows a space, but words never contain one
            continue;
        }
        assert_eq!(word(&c, token, body), only(Kind::Postcode, 1.0), "{token}");
    }
    for token in ["100", "1", "abc", "1012SA"] {
        assert!(word(&c, token, token).is_empty(), "{token}");
    }
    assert_eq!(word(&c, "1012AB", "1012AB"), only(Kind::Postcode, 1.0));
}

#[test]
fn ordinals() {
    let c = OrdinalClassifier::new();
    for token in ["1st", "2nd", "3rd", "4th", "11th", "22nd", "250th", "101st"] {
        assert_eq!(word(&c, token, token), only(Kind::Ordinal, 1.0), "{token}");
    }
    for token in ["1rd", "250nd", "2st"] {
        assert!(word(&c, token, token).is_empty(), "{token}");
    }
}

#[test]
fn directionals() {
    let c = DirectionalClassifier::new();
    for token in [
        "north",
        "n",
        "n.",
        "south",
        "s.",
        "east",
        "west",
        "w.",
        "northeast",
        "ne.",
        "sw",
        "lower",
        "lwr",
        "upper",
        "norte",
        "oeste",
        "noroeste",
        "nord",
        "ost",
        "süd",
    ] {
        assert_eq!(
            word(&c, token, token),
            only(Kind::Directional, 1.0),
            "{token}"
        );
    }
    for token in ["northsouth", "ns", "ns.", "westeast", "we", "we."] {
        assert!(word(&c, token, token).is_empty(), "{token}");
    }
}

#[test]
fn street_suffixes() {
    let c = StreetSuffixClassifier::new();
    for token in [
        "street",
        "st",
        "st.",
        "road",
        "rd",
        "rd.",
        "boulevard",
        "blvd",
        "blvd.",
        "straße",
        "strasse",
        "str",
        "str.",
        "platz",
        "allee",
        "weg",
    ] {
        let input = format!("main {token}");
        assert_eq!(
            word(&c, &input, token),
            only(Kind::StreetSuffix, 1.0),
            "{token}"
        );
    }
    // never the first word
    assert!(word(&c, "street", "street").is_empty());
}

#[test]
fn street_prefixes() {
    let c = StreetPrefixClassifier::new();
    for token in [
        "rue",
        "allée",
        "allee",
        "avenue",
        "av",
        "boulevard",
        "blvd",
        "blvd.",
    ] {
        assert_eq!(
            word(&c, token, token),
            only(Kind::StreetPrefix, 1.0),
            "{token}"
        );
    }
}

#[test]
fn compound_streets() {
    let c = CompoundStreetClassifier::new();
    for token in [
        "teststraße",
        "teststrasse",
        "teststr.",
        "teststr",
        "grolmanstr",
        "testallee",
        "testweg",
        "testplatz",
        "testpl.",
        "testvägen",
    ] {
        assert_eq!(word(&c, token, token), only(Kind::Street, 1.0), "{token}");
    }
    for token in ["testal", "testw", "testw.", "straße"] {
        assert!(word(&c, token, token).is_empty(), "{token}");
    }
}

#[test]
fn road_types() {
    let c = DictionaryWordClassifier::road_type();
    for token in ["highway", "road", "hi", "route", "hway"] {
        assert_eq!(word(&c, token, token), only(Kind::RoadType, 1.0), "{token}");
    }
    assert_eq!(word(&c, "r", "r"), only(Kind::RoadType, 0.2));
}

#[test]
fn toponyms_carry_languages() {
    let c = DictionaryWordClassifier::toponym();
    let t = run(&c, "md");
    let w = t.span(t.span(t.sections[0]).children[0]);
    assert_eq!(
        w.classifications,
        vec![Classification::with_langs(
            Kind::Toponym,
            1.0,
            vec!["en".into()]
        )]
    );
    for token in ["maryland", "ca", "california", "ia", "nj"] {
        assert_eq!(word(&c, token, token)[0].0, Kind::Toponym, "{token}");
    }
}

#[test]
fn stop_words() {
    let c = DictionaryWordClassifier::stop_word();
    for token in ["de", "la", "l'", "du", "sur", "the", "of"] {
        assert_eq!(
            word(&c, token, token),
            only(Kind::StopWord, 0.75),
            "{token}"
        );
    }
    assert_eq!(word(&c, "à", "à"), only(Kind::StopWord, 0.2));
}

#[test]
fn personal_titles() {
    let c = DictionaryPhraseClassifier::personal_title();
    for token in [
        "Général",
        "General",
        "gal",
        "Saint",
        "st",
        "cdt",
        "l'Amiral",
        "Burgemeester",
    ] {
        assert_eq!(
            phrase(&c, token, token),
            only(Kind::PersonalTitle, 1.0),
            "{token}"
        );
    }
}

#[test]
fn places_include_plurals() {
    let c = PlaceClassifier::new();
    for token in [
        "stables",
        "cafe",
        "bar",
        "park",
        "parks",
        "hospital",
        "hospitals",
        "university",
        "universities",
    ] {
        assert_eq!(word(&c, token, token), only(Kind::Place, 1.0), "{token}");
    }
    // lodash path quirk: the plural of "ind.est" adds "ind"
    assert_eq!(word(&c, "ind", "ind"), only(Kind::Place, 1.0));
}

#[test]
fn intersections_need_both_sides() {
    let c = IntersectionClassifier;
    assert_eq!(
        phrase(&c, "main & second", "&"),
        only(Kind::Intersection, 1.0)
    );
    assert_eq!(
        word(&c, "main & second", "&"),
        only(Kind::Intersection, 1.0)
    );
    assert_eq!(
        phrase(&c, "a an der ecke von b", "an der ecke von"),
        only(Kind::Intersection, 1.0)
    );
    assert!(phrase(&c, "& second", "&").is_empty());
    assert!(phrase(&c, "main and", "and").is_empty());
}

#[test]
fn whosonfirst() {
    let c = WhosOnFirstClassifier::new();
    for token in [
        "new york",
        "london",
        "paris",
        "berlin",
        "bern",
        "tokyo",
        "nyc",
        "sf",
        "zürich",
        "Saint-Étienne",
    ] {
        let k = phrase(&c, token, token);
        assert!(
            k.contains(&(Kind::Locality, 1.0)) && k.contains(&(Kind::Area, 1.0)),
            "{token}: {k:?}"
        );
    }
    for token in ["texas", "california", "italy"] {
        assert!(
            !phrase(&c, token, token)
                .iter()
                .any(|(k, _)| *k == Kind::Locality),
            "{token}"
        );
    }
    assert!(phrase(&c, "texas", "texas").contains(&(Kind::Region, 1.0)));
    assert!(phrase(&c, "il", "il").contains(&(Kind::Region, 1.0)));
    assert!(phrase(&c, "italy", "italy").contains(&(Kind::Country, 1.0)));
    assert!(phrase(&c, "usa", "usa").contains(&(Kind::Country, 1.0)));
    // blacklisted and street-like names
    assert!(phrase(&c, "city", "city").is_empty());
    assert!(phrase(&c, "north", "north").is_empty());
}

#[test]
fn central_european_street_names() {
    let c = CentralEuropeanStreetNameClassifier;
    let mut t = Tokenizer::new("Mühlenweg 12");
    HouseNumberClassifier::new().classify(&mut t);
    c.classify(&mut t);
    let first = t.span(t.span(t.sections[0]).children[0]);
    assert!(
        first
            .classifications
            .contains(&Classification::new(Kind::Street, 0.5))
    );

    // not when the house number is not last
    let mut t = Tokenizer::new("Mühlenweg 12 foo");
    HouseNumberClassifier::new().classify(&mut t);
    c.classify(&mut t);
    assert!(!t.span(t.span(t.sections[0]).children[0]).has(Kind::Street));
}

#[test]
fn composite_street_scheme() {
    let mut t = Tokenizer::new("main street");
    AlphaNumericClassifier::new().classify(&mut t);
    StreetSuffixClassifier::new().classify(&mut t);
    CompositeClassifier::new(schemes::street()).classify(&mut t);
    let phrases = t.span(t.sections[0]).phrases.clone();
    let k = kinds_of(&t, phrases.into_iter(), "main street");
    assert_eq!(k, vec![(Kind::Street, 0.82)]);
}

#[test]
fn units() {
    let mut t = Tokenizer::new("10 main st apt 2b #3");
    DictionaryWordClassifier::unit_type().classify(&mut t);
    UnitClassifier::new().classify(&mut t);
    let words = t.span(t.sections[0]).children.clone();
    assert_eq!(
        kinds_of(&t, words.iter().copied(), "apt"),
        only(Kind::UnitType, 1.0)
    );
    assert_eq!(
        kinds_of(&t, words.iter().copied(), "2b"),
        only(Kind::Unit, 1.0)
    );
    assert_eq!(
        kinds_of(&t, words.iter().copied(), "#3"),
        only(Kind::Unit, 1.0)
    );

    let mut t = Tokenizer::new("U12 10 main st");
    UnitTypeUnitClassifier::new().classify(&mut t);
    let words = t.span(t.sections[0]).children.clone();
    assert_eq!(
        kinds_of(&t, words.iter().copied(), "U"),
        only(Kind::UnitType, 1.0)
    );
    assert_eq!(
        kinds_of(&t, words.iter().copied(), "12"),
        only(Kind::Unit, 1.0)
    );
}
