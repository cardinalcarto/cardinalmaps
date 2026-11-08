//! The whole chain against mainline's whole server: every Andorra capture in
//! `valinor-tyr/fixtures/captures` (made with `tools/oracle/capture.py` against mainline 3.9.1
//! on the fixture tiles) is sent to the router as capture.py sent it,
//! and the response must be mainline's.
//!
//! JSON bodies must be equal after parsing. PBF bodies must decode to the same `Api`,
//! apart from `info.statistics` (latencies, which Valinor's Thor doesn't record).
//! The documented divergences in `docs/routing.md` are counted apart:
//! costings Sif doesn't have yet are error 107, and OSRM-format voice instructions in miles
//! use the request's units for approach alerts (only `voiceInstructions` may differ).

use crate::{HttpRequest, HttpResponse, Method, Router, RouterConfig, StatusCode};
use serde_json::Value;
use std::path::{Path, PathBuf};
use valhalla_proto::Api;
use valhalla_proto::prost::Message;

fn captures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../valinor-tyr/fixtures/captures/andorra")
}

/// Costings Sif doesn't have yet (error 107 instead of a route).
const MISSING_COSTINGS: [&str; 6] = [
    "bus_and_name",
    "motorcycle_defaults",
    "motor_scooter_defaults",
    "taxi_defaults",
    "truck_defaults",
    "truck_options",
];

/// OSRM-format responses in miles, whose approach alerts differ.
const MILES: [&str; 3] = [
    "auto_catalan_miles/osrm",
    "auto_german_miles/osrm",
    "ferrostar_pedestrian_german_miles/osrm",
];

/// Requests with a `current` time, whose responses depend on when they are made.
const NOW: [&str; 1] = ["auto_current_and_invariant"];

/// Sends a captured request as `capture.py` sent it.
fn send(router: &Router, dir: &Path) -> HttpResponse {
    let recorded: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("request.json")).unwrap()).unwrap();
    let Some(http) = recorded.get("http") else {
        let target = format!("/{}", recorded["action"].as_str().unwrap());
        let body = serde_json::to_vec(&recorded["request"]).unwrap();
        return router.handle(&HttpRequest {
            method: &Method::POST,
            target: target.as_bytes(),
            content_type: None,
            body: &body,
        });
    };
    let method = Method::from_bytes(http["method"].as_str().unwrap().as_bytes()).unwrap();
    let body = match (http.get("body_file"), http.get("body")) {
        (Some(file), _) => std::fs::read(dir.join(file.as_str().unwrap())).unwrap(),
        (None, Some(Value::String(body))) => body.clone().into_bytes(),
        _ => Vec::new(),
    };
    let content_type = http["content_type"].as_str();
    router.handle(&HttpRequest {
        method: &method,
        target: http["target"].as_str().unwrap().as_bytes(),
        content_type: content_type.map(str::as_bytes),
        body: &body,
    })
}

fn without_statistics(mut api: Api) -> Api {
    if let Some(info) = api.info.as_mut() {
        info.statistics.clear();
    }
    api
}

/// Removes every `voiceInstructions` member, anywhere in the document.
fn without_voice(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("voiceInstructions");
            map.values_mut().for_each(without_voice);
        }
        Value::Array(values) => values.iter_mut().for_each(without_voice),
        _ => {}
    }
}

#[test]
fn matches_mainline_on_every_capture() {
    let router = Router::open(
        RouterConfig::default(),
        &[PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../valhalla-graphtile/fixtures/andorra-tiles.tar")],
    )
    .unwrap();
    let mut cases: Vec<PathBuf> = std::fs::read_dir(captures())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    cases.sort();

    let (mut matched, mut missing_costings, mut miles, mut skipped) = (0, 0, 0, 0);
    let mut differences = Vec::new();
    for case in &cases {
        let name = case.file_name().unwrap().to_str().unwrap();
        if NOW.contains(&name) {
            skipped += 1;
            continue;
        }
        let mut formats: Vec<PathBuf> = std::fs::read_dir(case)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_dir())
            .collect();
        formats.sort();
        for dir in formats {
            let label = format!("{name}/{}", dir.file_name().unwrap().to_str().unwrap());
            let response = send(&router, &dir);
            if MISSING_COSTINGS.contains(&name) {
                // The JSON error is 107; the PBF error carries it in `info.errors`.
                let code = if dir.join("response.bin").exists() {
                    Api::decode(response.body.as_slice())
                        .ok()
                        .and_then(|api| api.info)
                        .and_then(|info| info.errors.first().map(|e| e.code))
                } else {
                    serde_json::from_slice::<Value>(&response.body).unwrap()["error_code"].as_u64()
                };
                assert_eq!(response.status, StatusCode::NOT_IMPLEMENTED, "{label}");
                // OSRM-format errors have no Valhalla code.
                if !label.ends_with("/osrm") {
                    assert_eq!(code, Some(107), "{label}");
                }
                missing_costings += 1;
                continue;
            }
            if dir.join("response.bin").exists() {
                let status: u16 = std::fs::read_to_string(dir.join("response.status"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                let expected =
                    Api::decode(std::fs::read(dir.join("response.bin")).unwrap().as_slice())
                        .unwrap();
                let actual = Api::decode(response.body.as_slice());
                if response.status.as_u16() != status
                    || actual.map(without_statistics).ok() != Some(without_statistics(expected))
                {
                    differences.push(label);
                } else {
                    matched += 1;
                }
                continue;
            }
            let expected: Value =
                serde_json::from_str(&std::fs::read_to_string(dir.join("response.json")).unwrap())
                    .unwrap();
            let mut actual: Value = serde_json::from_slice(&response.body).unwrap();
            let mut body = expected["body"].clone();
            let same_status = expected["status"].as_u64() == Some(response.status.as_u16().into());
            if MILES.contains(&label.as_str()) {
                assert_ne!(actual, body, "{label} matches; is the divergence gone?");
                without_voice(&mut actual);
                without_voice(&mut body);
                assert!(same_status && actual == body, "{label}");
                miles += 1;
            } else if same_status && actual == body {
                matched += 1;
            } else {
                differences.push(label);
            }
        }
    }
    assert!(differences.is_empty(), "differences: {differences:?}");
    // Each of them in JSON, OSRM format, and PBF.
    assert_eq!(missing_costings, 3 * MISSING_COSTINGS.len());
    assert_eq!(miles, MILES.len());
    assert_eq!(skipped, NOW.len());
    eprintln!(
        "{matched} responses match; {missing_costings} are error 107, {miles} differ in miles"
    );
    assert_eq!(matched, 168);
}
