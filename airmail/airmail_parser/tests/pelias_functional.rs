//! Upstream's functional test suite (`test/*.test.js`): query → expected
//! parse, extracted as data into `fixtures/pelias_functional.json`.
//!
//! Upstream checks the first solution (or, for some cases, all solutions)
//! as a list of `{label: body}` objects. Cases this port fails are listed in
//! `KNOWN_FAILURES` with the reason; the test fails if any other case fails
//! (a regression) or if a known failure starts passing (so the list stays
//! accurate). Run with `--nocapture` to see the counts.

use serde_json::Value;

/// Inputs this port parses differently from upstream's expectations, as
/// `(input, reason)`. Currently none: all 367 cases pass.
const KNOWN_FAILURES: &[(&str, &str)] = &[];

type Object = Vec<(String, String)>;

fn object(v: &Value) -> Object {
    let mut pairs: Object = v
        .as_array()
        .expect("object as [key, value] list")
        .iter()
        .map(|kv| {
            (
                kv[0].as_str().expect("key").to_string(),
                kv[1].as_str().expect("value").to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

fn solution(v: &Value) -> Vec<Object> {
    v.as_array()
        .expect("solution as list")
        .iter()
        .map(object)
        .collect()
}

fn actual_solution(pairs: &[(String, String)]) -> Vec<Object> {
    pairs.iter().map(|p| vec![p.clone()]).collect()
}

#[test]
fn pelias_functional_suite() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/pelias_functional.json"))
        .expect("valid fixture");
    let parser = airmail_parser::parser();
    let (mut passed, mut failed) = (0, Vec::new());
    for case in fixture["cases"].as_array().expect("cases") {
        let input = case["input"].as_str().expect("input");
        let actual = parser.upstream_solutions(input);
        let ok = if case["first_only"].as_bool().expect("first_only") {
            let expected = solution(&case["expected"]);
            actual.first().map(|s| actual_solution(s)) == Some(expected)
        } else {
            let expected: Vec<Vec<Object>> = case["expected"]
                .as_array()
                .expect("solutions")
                .iter()
                .map(solution)
                .collect();
            actual
                .iter()
                .map(|s| actual_solution(s))
                .collect::<Vec<_>>()
                == expected
        };
        if ok {
            passed += 1;
        } else {
            failed.push((
                case["file"].as_str().unwrap_or_default().to_string(),
                input.to_string(),
                case["expected"].clone(),
                actual,
            ));
        }
    }
    println!(
        "pelias functional suite: {passed} passed, {} failed",
        failed.len()
    );
    for (file, input, expected, actual) in &failed {
        let known = KNOWN_FAILURES.iter().any(|(i, _)| i == input);
        println!(
            "{} {file}: {input:?}\n    expected {expected}\n    actual   {:?}",
            if known { "KNOWN" } else { "FAIL " },
            actual.first()
        );
    }
    let unexpected: Vec<&str> = failed
        .iter()
        .map(|f| f.1.as_str())
        .filter(|i| !KNOWN_FAILURES.iter().any(|(k, _)| k == i))
        .collect();
    let fixed: Vec<&str> = KNOWN_FAILURES
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| !failed.iter().any(|f| f.1 == *k))
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected failures: {unexpected:#?}"
    );
    assert!(
        fixed.is_empty(),
        "known failures now pass, update KNOWN_FAILURES: {fixed:#?}"
    );
}
