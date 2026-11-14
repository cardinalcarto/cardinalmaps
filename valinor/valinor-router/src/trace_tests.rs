//! Map matching end to end against mainline's whole server: the Andorra trace captures in
//! `valinor-tyr/fixtures/traces` (`tools/oracle/copy-trace-fixtures.sh`) and the
//! `trace_attributes` serializer fixtures in `valinor-tyr/fixtures/trace-attributes`
//! (`tools/oracle/copy-trace-attributes-fixtures.py`) are sent to the router,
//! and each response must be mainline's.
//!
//! JSON bodies must be equal after parsing, and PBF bodies must decode to the same `Api`
//! (apart from latency statistics). The documented divergences are counted apart:
//! Meili's candidates at a node (OSRM's `alternatives_count`, or a zero-length edge at a node
//! that another of the node's edges stands for), OSRM approach alerts in miles,
//! and costings Sif doesn't have yet (error 107 from Loki, where mainline's Loki gives a
//! `multimodal` trace error 140).

use crate::capture_tests::{send, without_statistics, without_voice};
use crate::{Router, RouterConfig};
use serde_json::Value;
use std::path::{Path, PathBuf};
use valhalla_proto::Api;
use valhalla_proto::prost::Message;

fn fixtures(dir: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valinor-tyr/fixtures")
        .join(dir)
}

/// The router over the fixture tiles, with the config mainline's oracle builds used
/// (`valhalla_build_config`'s defaults, including its `meili` section and trace limits).
fn router() -> Router {
    let config =
        RouterConfig::from_valhalla_json(include_str!("../../valinor-loki/fixtures/valhalla.json"))
            .unwrap();
    Router::open(
        config,
        &[PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../valhalla-graphtile/fixtures/andorra-tiles.tar")],
    )
    .unwrap()
}

/// Removes every member named `key`, anywhere in the document.
fn without(value: &mut Value, key: &str) {
    match value {
        Value::Object(map) => {
            map.remove(key);
            map.values_mut().for_each(|v| without(v, key));
        }
        Value::Array(values) => values.iter_mut().for_each(|v| without(v, key)),
        _ => {}
    }
}

/// The edges (ID and length) of a `trace_attributes` response.
fn edges(body: &Value) -> Vec<(u64, f64)> {
    body["edges"]
        .as_array()
        .map(|edges| {
            edges
                .iter()
                .map(|e| {
                    (
                        e["id"].as_u64().unwrap_or(0),
                        e["length"].as_f64().unwrap_or(-1.0),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Cases with a costing Sif doesn't have yet, which Loki answers with error 107.
const MISSING_COSTINGS: [&str; 1] = ["andorra-error_140_multimodal"];

/// Route shapes mainline's edge walk fails on (443), which Valinor's walks
/// (see "Known divergences" in `docs/routing.md`).
const WALKED: [&str; 1] = ["andorra-0005-edge_walk_exact"];

/// What a run found.
#[derive(Debug, Default)]
struct Tally {
    matched: usize,
    missing_costings: usize,
    walked: usize,
    meili: Vec<String>,
    miles: Vec<String>,
    differences: Vec<String>,
}

/// Sends a case and compares the response with mainline's.
fn check(router: &Router, label: &str, dir: &Path, tally: &mut Tally) {
    let response = send(router, dir);
    if WALKED.contains(&label) {
        assert_eq!(response.status, http::StatusCode::OK, "{label}");
        let api = Api::decode(response.body.as_slice()).unwrap();
        assert!(
            api.trip.is_some_and(|t| t
                .routes
                .iter()
                .all(|r| r.legs.iter().all(|l| !l.node.is_empty()))),
            "{label}"
        );
        tally.walked += 1;
        return;
    }
    if MISSING_COSTINGS.contains(&label) {
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["error_code"], 107, "{label}");
        tally.missing_costings += 1;
        return;
    }
    let bin = ["response.bin", "response.1.bin"]
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.exists());
    if let Some(bin) = bin {
        let expected = Api::decode(std::fs::read(bin).unwrap().as_slice()).unwrap();
        let status = std::fs::read_to_string(dir.join("response.status"))
            .map_or(200, |s| s.trim().parse().unwrap());
        let actual = Api::decode(response.body.as_slice()).map(without_statistics);
        if response.status.as_u16() == status && actual.ok() == Some(without_statistics(expected)) {
            tally.matched += 1;
        } else {
            tally.differences.push(label.to_owned());
        }
        return;
    }
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("response.json")).unwrap()).unwrap();
    // The capture fixtures hold the status and body; the serializer fixtures, the body.
    let (status, mut body) = match expected.get("status") {
        Some(status) if expected.get("body").is_some() => {
            (status.as_u64().unwrap(), expected["body"].clone())
        }
        _ => (200, expected),
    };
    let mut actual: Value = serde_json::from_slice(&response.body).unwrap();
    if u64::from(response.status.as_u16()) != status {
        tally.differences.push(format!(
            "{label}: status {} (mainline's {status})",
            response.status
        ));
        return;
    }
    if actual == body {
        tally.matched += 1;
        return;
    }
    let (mut a, mut b) = (actual.clone(), body.clone());
    without(&mut a, "alternatives_count");
    without(&mut b, "alternatives_count");
    let (ours, theirs) = (edges(&actual), edges(&body));
    let at_nodes = ours.len() == theirs.len()
        && !ours.is_empty()
        && ours
            .iter()
            .zip(&theirs)
            .all(|(x, y)| x.0 == y.0 || (x.1 == 0.0 && y.1 == 0.0));
    if a == b || (at_nodes && ours != theirs) {
        tally.meili.push(label.to_owned());
        return;
    }
    without_voice(&mut actual);
    without_voice(&mut body);
    if actual == body {
        tally.miles.push(label.to_owned());
    } else {
        tally.differences.push(label.to_owned());
    }
}

/// The case directories under a fixture directory, in name order.
fn case_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut cases: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    cases
}

#[test]
fn matches_mainline_on_trace_captures() {
    let router = router();
    let mut tally = Tally::default();
    for case in case_dirs(&fixtures("traces/andorra")) {
        let label = case.file_name().unwrap().to_string_lossy().into_owned();
        check(&router, &label, &case, &mut tally);
    }
    eprintln!("{tally:?}");
    assert!(tally.differences.is_empty(), "{:?}", tally.differences);
    assert_eq!(
        tally.matched
            + tally.meili.len()
            + tally.miles.len()
            + tally.missing_costings
            + tally.walked,
        82
    );
    assert_eq!(tally.walked, WALKED.len());
    assert_eq!(tally.missing_costings, MISSING_COSTINGS.len());
    assert!(tally.matched >= 80, "{tally:?}");
}

#[test]
fn matches_mainline_on_trace_attributes_fixtures() {
    let router = router();
    let mut tally = Tally::default();
    for case in case_dirs(&fixtures("trace-attributes/andorra")) {
        let label = case.file_name().unwrap().to_string_lossy().into_owned();
        check(&router, &label, &case, &mut tally);
    }
    eprintln!("{tally:?}");
    assert!(tally.differences.is_empty(), "{:?}", tally.differences);
    assert_eq!(tally.matched + tally.meili.len() + tally.miles.len(), 68);
}
