//! Compares the stage against mainline 3.9.1, case by case, on the captured fixtures.
//!
//! Each fixture case (see `fixtures/README.md`) holds the `Api` that Thor sent to mainline's Odin
//! and the PBF response mainline produced from it.
//! We run our stage on the former and compare its directions with the latter's,
//! per the match tolerances in `docs/routing.md`:
//! everything must match exactly except lengths and times,
//! which may differ by a relative 1e-6.

use crate::narrate;
use std::fs;
use std::path::{Path, PathBuf};
use valhalla_proto::directions_leg::Maneuver;
use valhalla_proto::prost::Message;
use valhalla_proto::{Api, Directions, DirectionsLeg, Summary};

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");
const RELATIVE_TOLERANCE: f64 = 1e-6;

/// The fixture cases, as `<region>/<case>` directories.
pub(crate) fn cases() -> Vec<PathBuf> {
    let mut cases = Vec::new();
    for region in fs::read_dir(FIXTURES_DIR).expect("fixtures directory") {
        let region = region.expect("directory entry").path();
        if !region.is_dir() {
            continue;
        }
        for case in fs::read_dir(&region).expect("region directory") {
            let case = case.expect("directory entry").path();
            if case.join("thor-odin.bin").is_file() {
                cases.push(case);
            }
        }
    }
    cases.sort();
    cases
}

pub(crate) fn decode(path: &Path) -> Api {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Api::decode(bytes.as_slice()).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn close(expected: f64, actual: f64) -> bool {
    expected == actual
        || (expected - actual).abs() <= RELATIVE_TOLERANCE * expected.abs().max(actual.abs())
}

/// Compares a maneuver, returning a description of each mismatch.
fn compare_maneuver(expected: &Maneuver, actual: &Maneuver, at: &str, out: &mut Vec<String>) {
    if !close(f64::from(expected.length), f64::from(actual.length)) {
        out.push(format!(
            "{at}: length {} != {}",
            actual.length, expected.length
        ));
    }
    if !close(expected.time, actual.time) {
        out.push(format!("{at}: time {} != {}", actual.time, expected.time));
    }
    let strip = |m: &Maneuver| Maneuver {
        length: 0.0,
        time: 0.0,
        ..m.clone()
    };
    let (expected, actual) = (strip(expected), strip(actual));
    if expected != actual {
        // Name the fields that differ by comparing their debug forms.
        let expected_fields = debug_fields(&expected);
        let actual_fields = debug_fields(&actual);
        for (name, value) in &expected_fields {
            let other = actual_fields
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str());
            if other != Some(value.as_str()) {
                out.push(format!(
                    "{at}: {name}:\n    ours:     {}\n    mainline: {value}",
                    other.unwrap_or("<missing>")
                ));
            }
        }
    }
}

/// The top-level fields of a message's pretty debug output, as `(name, value)` pairs.
fn debug_fields(message: &impl core::fmt::Debug) -> Vec<(String, String)> {
    let text = format!("{message:#?}");
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in text.lines().skip(1) {
        if let Some(rest) = line.strip_prefix("    ")
            && !rest.starts_with(' ')
            && let Some((name, value)) = rest.split_once(": ")
        {
            fields.push((name.to_owned(), value.to_owned()));
        } else if let Some((_, value)) = fields.last_mut() {
            value.push_str(line.trim());
        }
    }
    fields
}

fn compare_summary(
    expected: Option<&Summary>,
    actual: Option<&Summary>,
    at: &str,
    out: &mut Vec<String>,
) {
    let (Some(expected), Some(actual)) = (expected, actual) else {
        if expected.is_some() != actual.is_some() {
            out.push(format!("{at}: summary presence differs"));
        }
        return;
    };
    if !close(f64::from(expected.length), f64::from(actual.length)) {
        out.push(format!(
            "{at}: summary length {} != {}",
            actual.length, expected.length
        ));
    }
    if !close(expected.time, actual.time) {
        out.push(format!(
            "{at}: summary time {} != {}",
            actual.time, expected.time
        ));
    }
    let strip = |s: &Summary| Summary {
        length: 0.0,
        time: 0.0,
        ..*s
    };
    if strip(expected) != strip(actual) {
        out.push(format!(
            "{at}: summary:\n    ours:     {:?}\n    mainline: {:?}",
            strip(actual),
            strip(expected)
        ));
    }
}

fn compare_leg(expected: &DirectionsLeg, actual: &DirectionsLeg, at: &str, out: &mut Vec<String>) {
    compare_summary(expected.summary.as_ref(), actual.summary.as_ref(), at, out);
    if expected.location != actual.location {
        out.push(format!("{at}: locations differ"));
    }
    if (expected.trip_id, expected.leg_id, expected.leg_count)
        != (actual.trip_id, actual.leg_id, actual.leg_count)
    {
        out.push(format!("{at}: ids differ"));
    }
    if expected.shape != actual.shape {
        out.push(format!("{at}: shape differs"));
    }
    if expected.level_changes != actual.level_changes {
        out.push(format!("{at}: level changes differ"));
    }
    if expected.maneuver.len() != actual.maneuver.len() {
        let types = |leg: &DirectionsLeg| leg.maneuver.iter().map(|m| m.r#type).collect::<Vec<_>>();
        out.push(format!(
            "{at}: {} maneuvers, mainline has {}\n    ours:     {:?}\n    mainline: {:?}",
            actual.maneuver.len(),
            expected.maneuver.len(),
            types(actual),
            types(expected)
        ));
        return;
    }
    for (i, (e, a)) in expected.maneuver.iter().zip(&actual.maneuver).enumerate() {
        compare_maneuver(e, a, &format!("{at} maneuver {i}"), out);
    }
}

/// Compares two sets of directions, returning every mismatch.
pub(crate) fn compare_directions(expected: &Directions, actual: &Directions) -> Vec<String> {
    let mut out = Vec::new();
    if expected.routes.len() != actual.routes.len() {
        out.push(format!(
            "{} routes, mainline has {}",
            actual.routes.len(),
            expected.routes.len()
        ));
        return out;
    }
    for (r, (expected_route, actual_route)) in
        expected.routes.iter().zip(&actual.routes).enumerate()
    {
        if expected_route.legs.len() != actual_route.legs.len() {
            out.push(format!(
                "route {r}: {} legs, mainline has {}",
                actual_route.legs.len(),
                expected_route.legs.len()
            ));
            continue;
        }
        for (l, (e, a)) in expected_route
            .legs
            .iter()
            .zip(&actual_route.legs)
            .enumerate()
        {
            compare_leg(e, a, &format!("route {r} leg {l}"), &mut out);
        }
    }
    out
}

#[test]
fn directions_match_mainline() {
    let cases = cases();
    assert!(
        cases.len() >= 27,
        "expected at least 27 fixture cases, found {}",
        cases.len()
    );
    let mut failures = Vec::new();
    for case in &cases {
        let name = case
            .strip_prefix(FIXTURES_DIR)
            .unwrap_or(case)
            .display()
            .to_string();
        let mut request = decode(&case.join("thor-odin.bin"));
        let response = decode(&case.join("response.bin"));
        let expected = response.directions.expect("mainline directions");
        if let Err(error) = narrate(&mut request) {
            failures.push(format!("{name}: {error}"));
            continue;
        }
        let actual = request.directions.unwrap_or_default();
        let mismatches = compare_directions(&expected, &actual);
        if !mismatches.is_empty() {
            failures.push(format!("{name}:\n  {}", mismatches.join("\n  ")));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ from mainline:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

fn maneuver(directions: &mut Directions) -> &mut Maneuver {
    &mut directions.routes[0].legs[0].maneuver[1]
}

/// The comparison catches differences, and tolerates only tiny length and time differences.
#[test]
fn comparison_detects_differences() {
    let case = Path::new(FIXTURES_DIR).join("andorra/auto_catalan_miles");
    let expected = decode(&case.join("response.bin"))
        .directions
        .expect("directions");
    assert!(compare_directions(&expected, &expected).is_empty());

    let mut changed = expected.clone();
    maneuver(&mut changed)
        .verbal_pre_transition_instruction
        .push('!');
    let mismatches = compare_directions(&expected, &changed);
    assert_eq!(mismatches.len(), 1, "{mismatches:?}");
    assert!(
        mismatches[0].contains("verbal_pre_transition_instruction"),
        "{mismatches:?}"
    );

    let mut changed = expected.clone();
    maneuver(&mut changed).length *= 1.0 + 1e-7;
    assert!(compare_directions(&expected, &changed).is_empty());
    maneuver(&mut changed).length *= 1.0 + 1e-5;
    assert_eq!(compare_directions(&expected, &changed).len(), 1);

    let mut changed = expected.clone();
    changed.routes[0].legs[0].maneuver.pop();
    assert_eq!(compare_directions(&expected, &changed).len(), 1);
}
