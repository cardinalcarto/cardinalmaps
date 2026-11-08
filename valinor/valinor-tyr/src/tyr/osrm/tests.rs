//! The OSRM serializer against mainline's captured responses.

use crate::test_support::{captures, json_differences};
use crate::tyr::{JSON_CONTENT_TYPE, serialize_directions};
use serde_json::Value;

/// Every captured OSRM-format route (as Ferrostar requests it, with banner and voice
/// instructions) serializes to mainline's response, given what mainline's Odin produced.
///
/// Only cases without captured directions (exact GET and raw requests,
/// which have no PBF variant) lack Tyr input;
/// any other 200 response without it fails the test.
#[test]
fn osrm_responses_match_mainline() {
    let mut checked = 0;
    let mut failures = Vec::new();
    for capture in captures("osrm") {
        let (status, mut expected) = capture.json_response();
        let name = capture.name();
        let Some(mut api) = capture.odin_output() else {
            assert!(
                status != 200 || !capture.has_directions(),
                "{name}: no Tyr input"
            );
            continue;
        };
        assert_eq!(status, 200, "{name}");
        correct_imperial_alerts(&name, &mut expected);
        let response = serialize_directions(&mut api).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(response.content_type, JSON_CONTENT_TYPE);
        let actual: Value = serde_json::from_slice(&response.body)
            .unwrap_or_else(|e| panic!("{name}: invalid JSON: {e}"));
        let differences = json_differences(&expected, &actual, 10);
        if !differences.is_empty() {
            failures.push(format!("{name}:\n  {}", differences.join("\n  ")));
        }
        checked += 1;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(checked >= 40, "only {checked} captures checked");
}

/// Approach alerts in miles requests where we deliberately differ from mainline:
/// `(case, step, voice instruction, mainline's announcement, ours)`.
///
/// Mainline passes the alert distance in kilometers even for requests in miles,
/// so it announces the kilometer value as miles
/// ("half a mile" for an alert played a quarter mile ahead).
/// We announce the distance in the request's units (see `voice::alert_distance`).
/// Kilometer requests match mainline exactly.
const IMPERIAL_ALERT_CORRECTIONS: &[(&str, usize, usize, &str, &str)] = &[
    (
        "andorra/auto_catalan_miles/osrm",
        17,
        1,
        "En mitja milla, Entra a la rotonda i agafa la 2a sortida de Carretera d'Engolasters.",
        "En un quart de milla, Entra a la rotonda i agafa la 2a sortida de Carretera d'Engolasters.",
    ),
    (
        "andorra/auto_catalan_miles/osrm",
        31,
        1,
        "En mitja milla, Gira a l'esquerra cap a Travessera del Jardí.",
        "En un quart de milla, Gira a l'esquerra cap a Travessera del Jardí.",
    ),
    (
        "andorra/auto_catalan_miles/osrm",
        34,
        0,
        "En 300 peus, Arribaràs a la teva destinació.",
        "En 200 peus, Arribaràs a la teva destinació.",
    ),
    (
        "andorra/auto_german_miles/osrm",
        4,
        0,
        "In eine viertel Meile, Rechts auf Avinguda de Santa Coloma abbiegen.",
        "In 900 Fuß, Rechts auf Avinguda de Santa Coloma abbiegen.",
    ),
];

/// Replaces mainline's miscomputed miles announcements with ours,
/// checking first that mainline's are the ones listed.
fn correct_imperial_alerts(name: &str, expected: &mut Value) {
    for &(case, step, index, mainline, ours) in IMPERIAL_ALERT_CORRECTIONS {
        if case != name {
            continue;
        }
        let voice =
            &mut expected["routes"][0]["legs"][0]["steps"][step]["voiceInstructions"][index];
        assert_eq!(voice["announcement"], mainline, "{name} step {step}");
        voice["announcement"] = Value::from(ours);
        voice["ssmlAnnouncement"] = Value::from(format!("<speak>{ours}</speak>"));
    }
}
