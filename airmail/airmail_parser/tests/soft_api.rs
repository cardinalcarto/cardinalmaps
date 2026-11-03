//! Tests of the public, soft-signal API.

use airmail_parser::Kind;

#[test]
fn address_with_admins() {
    let parse = airmail_parser::parse("123 main st springfield il");
    let best = parse.best().expect("a solution");
    let kinds: Vec<(Kind, &str)> = best
        .spans
        .iter()
        .map(|s| (s.kind, s.text.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (Kind::HouseNumber, "123"),
            (Kind::Street, "main st"),
            (Kind::Locality, "springfield"),
            (Kind::Region, "il"),
        ]
    );
    let weights = parse.token_weights();
    assert_eq!(weights.len(), 5);
    let springfield = &weights[3];
    assert_eq!(springfield.token.text, "springfield");
    assert!(springfield.weights.admin() > 0.9);
    assert!(weights[1].weights.get(Kind::Street) > 0.5);
}

#[test]
fn offsets_are_bytes_and_chars() {
    let input = "Grolmanstraße 23, Zürich";
    let parse = airmail_parser::parse(input);
    for solution in parse.solutions() {
        for span in &solution.spans {
            assert_eq!(&input[span.range.bytes()], span.text);
            let chars: String = input
                .chars()
                .skip(span.range.char_start)
                .take(span.range.char_end - span.range.char_start)
                .collect();
            assert_eq!(chars, span.text);
        }
    }
    let zurich = parse.tokens().last().expect("tokens");
    assert_eq!(zurich.text, "Zürich");
    assert_eq!((zurich.range.char_start, zurich.range.char_end), (18, 24));
    assert_eq!((zurich.range.start, zurich.range.end), (19, 26));
}

#[test]
fn categorical_words_stay_unclassified() {
    let parse = airmail_parser::parse("vegan food");
    for tw in parse.token_weights() {
        assert!(tw.weights.unclassified > 0.99, "{tw:?}");
    }
    let parse = airmail_parser::parse("starbucks seattle");
    let w = parse.token_weights();
    assert!(w[0].weights.unclassified > 0.99);
    assert!(w[1].weights.get(Kind::Locality) > 0.99);
}

#[test]
fn weights_are_bounded() {
    for q in [
        "coffee near capitol hill",
        "30 w 26 st nyc 10010",
        "Main St & Second Ave",
        "paris texas usa",
    ] {
        let parse = airmail_parser::parse(q);
        for tw in parse.token_weights() {
            let total: f64 =
                tw.weights.kinds.iter().map(|(_, w)| w).sum::<f64>() + tw.weights.unclassified;
            assert!(
                (total - 1.0).abs() < 1e-9 || tw.weights.unclassified == 0.0,
                "{q}: {tw:?}"
            );
            assert!(
                tw.weights
                    .kinds
                    .iter()
                    .all(|(_, w)| *w > 0.0 && *w <= 1.0 + 1e-9)
            );
        }
    }
}

#[test]
fn features_expose_dictionary_hits() {
    // (upstream ignores admin names right before a place keyword, so not
    // "springfield park")
    let parse = airmail_parser::parse("park springfield");
    assert!(
        parse
            .features()
            .iter()
            .any(|f| f.kind == Kind::Locality && f.text == "springfield")
    );
    assert!(
        parse
            .features()
            .iter()
            .any(|f| f.kind == Kind::Place && f.text == "park")
    );
}

#[test]
fn empty_and_odd_inputs() {
    for q in ["", " ", ",,,", "\"", "😀 main st", "&", "-"] {
        let parse = airmail_parser::parse(q);
        for s in parse.solutions() {
            for span in &s.spans {
                assert_eq!(&q[span.range.bytes()], span.text);
            }
        }
        let _ = parse.token_weights();
    }
    // long input is truncated to 140 UTF-16 units, as upstream
    let long = "a ".repeat(200);
    let parse = airmail_parser::parse(&long);
    assert!(parse.tokens().len() <= 70);
}

#[test]
fn weights_for_arbitrary_ranges() {
    let q = "123 main st springfield il";
    let parse = airmail_parser::parse(q);
    let start = q.find("springfield").expect("present");
    let w = parse.weights(start..start + "spring".len());
    assert!(w.get(Kind::Locality) > 0.9);
}
