//! Loki against mainline's, on the requests in `fixtures/oracle`.
//!
//! The references come from mainline's own `loki_worker_t`, run by the `loki` helper in
//! `tools/oracle/cpp` on the same tiles (see `tools/oracle/README.md`).
//! Each request goes through the front (`valinor-tyr`) and then Loki,
//! and the result is compared with mainline's under the Loki tolerances of
//! `docs/routing.md`: the same candidate edges per location, `percent_along` within 1e-6,
//! distances within 0.01 m, and the same errors.
//!
//! Set `VALINOR_LOKI_ORACLE` to a directory with other `requests.tsv` and `results.tsv`,
//! and `VALINOR_LOKI_TILES` to their tile set (a directory or tarball),
//! to check other references (the `oracle_external` test, which is ignored by default).

#![allow(
    clippy::float_cmp,
    clippy::too_many_lines,
    reason = "references are compared exactly where mainline's values must be exact"
)]

use crate::{ConnectivityMap, Loki, LokiConfig, LokiOutput};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use core::num::NonZeroUsize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use valhalla_graphtile::tile_provider::{
    CachingTileProvider, DirectoryGraphTileProvider, GraphTileProvider, TarballTileProvider,
};
use valhalla_proto::options::Action;
use valhalla_proto::prost::Message;
use valhalla_proto::{Api, Location, PathEdge};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// One reference result.
#[derive(Debug)]
enum Expected {
    Ok(Api),
    Locate(Api, String),
    Error(u16, String, Api),
}

fn decode(s: &str) -> Vec<u8> {
    STANDARD.decode(s.trim()).expect("base64")
}

fn read_results(path: &Path) -> BTreeMap<String, Expected> {
    let text = std::fs::read_to_string(path).expect("results");
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            let api = |s: &str| Api::decode(decode(s).as_slice()).expect("an Api");
            let expected = match fields[1] {
                "ok" => Expected::Ok(api(fields[2])),
                "locate" => Expected::Locate(
                    api(fields[2]),
                    String::from_utf8(decode(fields[3])).expect("UTF-8"),
                ),
                "error" => Expected::Error(
                    fields[2].parse().expect("a code"),
                    String::from_utf8(decode(fields[3])).expect("UTF-8"),
                    api(fields[4]),
                ),
                other => panic!("unknown status {other}"),
            };
            (fields[0].to_string(), expected)
        })
        .collect()
}

/// The differences between two path edges beyond the tolerances.
fn edge_differences(what: &str, expected: &PathEdge, actual: &PathEdge, out: &mut Vec<String>) {
    let mut e = expected.clone();
    let mut a = actual.clone();
    if (e.percent_along - a.percent_along).abs() > 1e-6 {
        out.push(format!(
            "{what}: percent_along {} != {}",
            e.percent_along, a.percent_along
        ));
    }
    if (e.distance - a.distance).abs() > 0.01 {
        out.push(format!("{what}: distance {} != {}", e.distance, a.distance));
    }
    let heading_diff = (e.heading - a.heading).abs();
    if heading_diff > 1e-3 && (360.0 - heading_diff) > 1e-3 {
        out.push(format!("{what}: heading {} != {}", e.heading, a.heading));
    }
    let ll = |p: &PathEdge| crate::search::point_of(p.ll.as_ref());
    let (el, al) = (ll(&e), ll(&a));
    if (el.lat - al.lat).abs() > 1e-9 || (el.lng - al.lng).abs() > 1e-9 {
        out.push(format!("{what}: ll {el:?} != {al:?}"));
    }
    for p in [&mut e, &mut a] {
        p.percent_along = 0.0;
        p.distance = 0.0;
        p.heading = 0.0;
        p.ll = None;
    }
    if e != a {
        out.push(format!("{what}: {e:?}\n    != {a:?}"));
    }
}

fn location_differences(what: &str, expected: &Location, actual: &Location, out: &mut Vec<String>) {
    let mut e = expected.clone();
    let mut a = actual.clone();
    let ec = e.correlation.take().unwrap_or_default();
    let ac = a.correlation.take().unwrap_or_default();
    if e != a {
        out.push(format!("{what}: {e:?}\n    != {a:?}"));
    }
    for (kind, ee, ae) in [
        ("edges", &ec.edges, &ac.edges),
        ("filtered_edges", &ec.filtered_edges, &ac.filtered_edges),
    ] {
        let ids = |v: &[PathEdge]| v.iter().map(|p| p.graph_id).collect::<Vec<_>>();
        if ee.len() != ae.len() {
            out.push(format!(
                "{what}.{kind}: {} != {} edges ({:?} != {:?})",
                ee.len(),
                ae.len(),
                ids(ee),
                ids(ae)
            ));
            continue;
        }
        for (i, (x, y)) in ee.iter().zip(ae).enumerate() {
            edge_differences(&format!("{what}.{kind}[{i}]"), x, y, out);
        }
    }
    let mut ec = ec;
    let mut ac = ac;
    ec.edges.clear();
    ec.filtered_edges.clear();
    ac.edges.clear();
    ac.filtered_edges.clear();
    if ec != ac {
        out.push(format!("{what}.correlation: {ec:?} != {ac:?}"));
    }
}

fn api_differences(expected: &Api, actual: &Api) -> Vec<String> {
    let mut out = Vec::new();
    let mut e = expected.clone();
    let mut a = actual.clone();
    for api in [&mut e, &mut a] {
        if let Some(info) = api.info.as_mut() {
            info.statistics.clear();
        }
    }
    let mut eo = e.options.take().unwrap_or_default();
    let mut ao = a.options.take().unwrap_or_default();
    for (kind, el, al) in [
        ("locations", &eo.locations, &ao.locations),
        ("sources", &eo.sources, &ao.sources),
        ("targets", &eo.targets, &ao.targets),
        (
            "exclude_locations",
            &eo.exclude_locations,
            &ao.exclude_locations,
        ),
    ] {
        if el.len() != al.len() {
            out.push(format!("{kind}: {} != {} locations", el.len(), al.len()));
            continue;
        }
        for (i, (x, y)) in el.iter().zip(al).enumerate() {
            location_differences(&format!("{kind}[{i}]"), x, y, &mut out);
        }
    }
    for o in [&mut eo, &mut ao] {
        o.locations.clear();
        o.sources.clear();
        o.targets.clear();
        o.exclude_locations.clear();
    }
    if eo != ao {
        for (key, ec) in &eo.costings {
            if ao.costings.get(key) != Some(ec) {
                out.push(format!(
                    "costing {key}: {ec:?}\n    != {:?}",
                    ao.costings.get(key)
                ));
            }
        }
        eo.costings.clear();
        ao.costings.clear();
        if eo != ao {
            out.push(format!("options: {eo:?}\n    != {ao:?}"));
        }
    }
    if e != a {
        out.push(format!("api: {e:?}\n    != {a:?}"));
    }
    out
}

/// Compares JSON with a relative float tolerance.
fn json_differences(path: &str, expected: &Value, actual: &Value, out: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            if (x - y).abs() > 1e-6 * x.abs().max(1.0) {
                out.push(format!("{path}: {x} != {y}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push(format!("{path}: {} != {} elements", x.len(), y.len()));
                return;
            }
            for (i, (x, y)) in x.iter().zip(y).enumerate() {
                json_differences(&format!("{path}[{i}]"), x, y, out);
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                match y.get(k) {
                    Some(w) => json_differences(&format!("{path}.{k}"), v, w, out),
                    None => out.push(format!("{path}.{k}: missing")),
                }
            }
            for k in y.keys() {
                if !x.contains_key(k) {
                    out.push(format!("{path}.{k}: unexpected"));
                }
            }
        }
        (x, y) if x != y => out.push(format!("{path}: {x} != {y}")),
        _ => {}
    }
}

/// Mainline's side of the street for a trace end given as a location is read past the end of
/// the shape in some cases (see `worker::reads_past_end`); we take ours for those.
fn ignore_past_end_sides<P: GraphTileProvider>(
    loki: &Loki<P>,
    expected: &Api,
    actual: &Api,
) -> Api {
    let mut expected = expected.clone();
    let (Some(eo), Some(ao)) = (expected.options.as_mut(), actual.options.as_ref()) else {
        return expected;
    };
    let trace = [
        i32::from(Action::TraceRoute),
        i32::from(Action::TraceAttributes),
    ];
    if !trace.contains(&eo.action) {
        return expected;
    }
    for (el, al) in eo.locations.iter_mut().zip(&ao.locations) {
        let ll = crate::search::point_of(el.ll.as_ref());
        let (Some(ec), Some(ac)) = (el.correlation.as_mut(), al.correlation.as_ref()) else {
            continue;
        };
        for (ee, ae) in ec.edges.iter_mut().zip(&ac.edges) {
            if ee.graph_id == ae.graph_id
                && crate::worker::reads_past_end(loki.provider(), ee.graph_id, ll)
            {
                ee.side_of_street = ae.side_of_street;
            }
        }
    }
    expected
}

/// Runs every request against the references; returns (checked, failures).
fn run<P: GraphTileProvider>(
    loki: &Loki<P>,
    requests: &Path,
    results: &Path,
) -> (usize, Vec<String>) {
    let expected = read_results(results);
    let text = std::fs::read_to_string(requests).expect("requests");
    let mut failures = Vec::new();
    let mut checked = 0;
    for line in text.lines().filter(|l| !l.is_empty()) {
        let mut fields = line.splitn(3, '\t');
        let (Some(id), Some(action), Some(json)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Some(reference) = expected.get(id) else {
            failures.push(format!("{id}: no reference"));
            continue;
        };
        let action = Action::from_str_name(action).expect("an action");
        let mut api = Api::default();
        let result = valinor_tyr::front::parse_json_request(json.as_bytes(), action, &mut api)
            .and_then(|()| loki.handle(&mut api));
        checked += 1;
        let differences = match (reference, &result) {
            (Expected::Ok(e), Ok(LokiOutput::Forward)) => {
                let e = &ignore_past_end_sides(loki, e, &api);
                api_differences(e, &api)
            }
            (Expected::Locate(e, body), Ok(LokiOutput::Respond(ours))) => {
                let mut out = api_differences(e, &api);
                let x: Value = serde_json::from_str(body).expect("JSON");
                let y: Value = serde_json::from_str(ours).expect("our JSON");
                json_differences("locate", &x, &y, &mut out);
                out
            }
            (Expected::Error(code, message, e), Err(error)) => {
                let mut out = Vec::new();
                if error.code().code() != *code || error.message() != message.as_str() {
                    out.push(format!(
                        "error {code} {message:?} != {} {:?}",
                        error.code().code(),
                        error.message()
                    ));
                }
                // The warnings added before the failure.
                let warnings = |a: &Api| {
                    a.info
                        .as_ref()
                        .map(|i| i.warnings.clone())
                        .unwrap_or_default()
                };
                if warnings(e) != warnings(&api) {
                    out.push(format!(
                        "warnings {:?} != {:?}",
                        warnings(e),
                        warnings(&api)
                    ));
                }
                out
            }
            (reference, result) => vec![format!(
                "outcome {} != {result:?}",
                match reference {
                    Expected::Ok(_) => "ok".to_string(),
                    Expected::Locate(..) => "locate".to_string(),
                    Expected::Error(code, message, _) => format!("error {code} {message}"),
                }
            )],
        };
        if !differences.is_empty() {
            failures.push(format!("{id} {json}\n  {}", differences.join("\n  ")));
        }
    }
    (checked, failures)
}

fn loki_over_tarball(tiles: &Path) -> Loki<CachingTileProvider<TarballTileProvider<false, false>>> {
    let tarball = TarballTileProvider::<false, false>::new_readonly(tiles).expect("tiles");
    let connectivity = ConnectivityMap::new(tarball.tile_ids().copied());
    let config =
        LokiConfig::from_valhalla_json(include_str!("../fixtures/valhalla.json")).expect("config");
    Loki::new(
        config,
        CachingTileProvider::new(tarball, NonZeroUsize::new(64).expect("nonzero")),
        Some(connectivity),
    )
}

fn report(checked: usize, failures: &[String]) {
    for failure in failures.iter().take(15) {
        eprintln!("{failure}\n");
    }
    assert!(
        failures.is_empty(),
        "{} of {checked} requests differ from mainline",
        failures.len()
    );
}

#[test]
fn oracle_andorra() {
    let loki =
        loki_over_tarball(&fixtures().join("../../valhalla-graphtile/fixtures/andorra-tiles.tar"));
    let dir = fixtures().join("oracle/andorra");
    let (checked, failures) = run(&loki, &dir.join("requests.tsv"), &dir.join("results.tsv"));
    assert!(checked > 100, "only {checked} requests");
    report(checked, &failures);
}

/// The worker's entry point gives the same results from raw HTTP requests.
#[test]
fn oracle_andorra_over_http() {
    let loki =
        loki_over_tarball(&fixtures().join("../../valhalla-graphtile/fixtures/andorra-tiles.tar"));
    let dir = fixtures().join("oracle/andorra");
    let expected = read_results(&dir.join("results.tsv"));
    let text = std::fs::read_to_string(dir.join("requests.tsv")).expect("requests");
    let mut seen = [0; 3];
    for line in text.lines().take(200) {
        let mut fields = line.splitn(3, '\t');
        let (Some(id), Some(action), Some(json)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let raw = format!(
            "POST /{action} HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{json}",
            json.len()
        );
        let outcome = crate::handle_http_request(&loki, raw.as_bytes());
        match (&expected[id], outcome) {
            (Expected::Ok(e), crate::HttpOutcome::Forward(api)) => {
                let e = ignore_past_end_sides(&loki, e, &api);
                assert!(api_differences(&e, &api).is_empty(), "{id}");
                seen[0] += 1;
            }
            (
                Expected::Locate(_, body),
                crate::HttpOutcome::Respond {
                    body: ours,
                    content_type,
                    ..
                },
            ) => {
                assert_eq!(content_type, valinor_tyr::tyr::JSON_CONTENT_TYPE);
                let x: Value = serde_json::from_str(body).expect("JSON");
                let y: Value = serde_json::from_slice(&ours).expect("our JSON");
                let mut out = Vec::new();
                json_differences("locate", &x, &y, &mut out);
                assert!(out.is_empty(), "{id}: {out:?}");
                seen[1] += 1;
            }
            (Expected::Error(code, message, _), crate::HttpOutcome::Error { error, .. }) => {
                assert_eq!(
                    (error.code().code(), error.message().as_ref()),
                    (*code, message.as_str()),
                    "{id}"
                );
                seen[2] += 1;
            }
            (e, outcome) => panic!("{id}: {e:?} != {outcome:?}"),
        }
    }
    assert!(seen.iter().all(|&n| n > 10), "{seen:?}");
}

/// The same check for references elsewhere (see the module docs).
#[test]
#[ignore = "needs VALINOR_LOKI_ORACLE and VALINOR_LOKI_TILES"]
fn oracle_external() {
    let dir = PathBuf::from(std::env::var("VALINOR_LOKI_ORACLE").expect("VALINOR_LOKI_ORACLE"));
    let tiles = PathBuf::from(std::env::var("VALINOR_LOKI_TILES").expect("VALINOR_LOKI_TILES"));
    let (checked, failures) = if tiles.is_dir() {
        let provider = DirectoryGraphTileProvider::new(
            tiles.clone(),
            NonZeroUsize::new(256).expect("nonzero"),
        );
        let ids = walk_tiles(&tiles);
        let config = LokiConfig::from_valhalla_json(include_str!("../fixtures/valhalla.json"))
            .expect("config");
        let loki = Loki::new(
            config,
            CachingTileProvider::new(provider, NonZeroUsize::new(256).expect("nonzero")),
            Some(ConnectivityMap::new(ids)),
        );
        run(&loki, &dir.join("requests.tsv"), &dir.join("results.tsv"))
    } else {
        let loki = loki_over_tarball(&tiles);
        run(&loki, &dir.join("requests.tsv"), &dir.join("results.tsv"))
    };
    eprintln!("checked {checked}");
    report(checked, &failures);
}

/// The tile IDs of a tile directory (`<level>/<digits>/.../<digits>.gph`).
fn walk_tiles(dir: &Path) -> Vec<valhalla_graphtile::GraphId> {
    let mut ids = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("readable").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(relative) = path
                .with_extension("")
                .strip_prefix(dir)
                .map(Path::to_path_buf)
            else {
                continue;
            };
            let parts: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            if let Some((level, rest)) = parts.split_first()
                && let (Ok(level), Ok(tile)) = (level.parse::<u8>(), rest.concat().parse::<u64>())
                && let Ok(id) = valhalla_graphtile::GraphId::try_from_components(level, tile, 0)
            {
                ids.push(id);
            }
        }
    }
    ids
}
