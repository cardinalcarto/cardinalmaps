//! Shared helpers for tests against the captured mainline cases in `fixtures/captures`.
//!
//! See `tools/oracle/copy-tyr-fixtures.sh` for the layout.

#![allow(
    dead_code,
    reason = "the helpers are shared by test modules that each use a subset"
)]

use illuvatar::Odin;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use valhalla_proto::Api;
use valhalla_proto::prost::Message;

/// The root of the captured cases.
pub(crate) const CAPTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/captures");

/// One captured case in one response format, for example `andorra/auto_town/osrm`.
#[derive(Debug, Clone)]
pub(crate) struct Capture {
    /// `<region>/<case>`.
    pub(crate) case: String,
    /// `json`, `osrm`, or `pbf`.
    pub(crate) format: String,
}

impl Capture {
    fn case_dir(&self) -> PathBuf {
        Path::new(CAPTURES_DIR).join(&self.case)
    }

    fn dir(&self) -> PathBuf {
        self.case_dir().join(&self.format)
    }

    /// A short name for assertion messages.
    pub(crate) fn name(&self) -> String {
        format!("{}/{}", self.case, self.format)
    }

    /// The exact HTTP request, for cases that describe one
    /// (GET requests, extra query parameters, raw or PBF bodies, other methods;
    /// see `tools/oracle/capture.py`).
    ///
    /// `None` for ordinary cases, which are a JSON body sent with `POST /<action>`
    /// (see [`Capture::request`]).
    pub(crate) fn http(&self) -> Option<HttpCapture> {
        let request = read_json(&self.dir().join("request.json"));
        let http = request.get("http")?;
        let field = |name: &str| http[name].as_str().map(str::to_string);
        let body = match (field("body"), field("body_file")) {
            (Some(body), _) => body.into_bytes(),
            (None, Some(file)) => fs::read(self.dir().join(file)).expect("request body file"),
            (None, None) => Vec::new(),
        };
        Some(HttpCapture {
            method: field("method").expect("http request has a method"),
            target: field("target").expect("http request has a target"),
            content_type: field("content_type"),
            body,
        })
    }

    /// The captured HTTP request: its action and its JSON body.
    ///
    /// For cases with an exact [`Capture::http`] request, the body is the JSON the case
    /// was built from (`null` for raw and PBF bodies), not necessarily what was sent.
    pub(crate) fn request(&self) -> (String, Value) {
        let mut request = read_json(&self.dir().join("request.json"));
        let action = request["action"]
            .as_str()
            .expect("request has an action")
            .to_string();
        (action, request["request"].take())
    }

    /// The Loki→Thor message, if mainline got that far.
    pub(crate) fn loki_thor(&self) -> Option<Api> {
        read_api(&self.dir().join("loki-thor.bin"))
    }

    /// What Odin handed Tyr: the Thor→Odin message plus Odin's directions,
    /// if mainline got that far.
    ///
    /// Odin's directions only reach us in a PBF response that selects them,
    /// so exact-request cases (see [`Capture::http`]) captured in another format,
    /// or with a selector that leaves the directions out, have none.
    /// For those this returns `None` (JSON and OSRM formats),
    /// or the request without directions (PBF format, whose output then leaves them out too).
    /// Every other case that reached Odin must have directions.
    pub(crate) fn odin_output(&self) -> Option<Api> {
        let mut api = read_api(&self.dir().join("thor-odin.bin"))?;
        let trip = read_api(&self.case_dir().join("trip.bin")).expect("case has a trip");
        api.trip = trip.trip;
        // Mainline's Odin also updates the trip in place
        // (it marks the turn lanes maneuvers use and fills in short edges' headings),
        // and Tyr reads those updates.
        // The captures only hold the trip before Odin, so Ilúvatar (our Odin) applies them;
        // its directions are thrown away in favor of mainline's.
        let mut narrated = api.clone();
        Odin::default()
            .narrate(&mut narrated)
            .unwrap_or_else(|e| panic!("{}: Ilúvatar fails: {e}", self.name()));
        api.trip = narrated.trip;
        if let Some(directions) = read_api(&self.case_dir().join("directions.bin")) {
            api.directions = directions.directions;
        } else {
            assert!(
                self.http().is_some(),
                "{}: only exact-request cases may lack directions",
                self.name()
            );
            if self.format != "pbf" {
                return None;
            }
        }
        Some(api)
    }

    /// Whether Odin's directions were captured for this case (see [`Capture::odin_output`]).
    pub(crate) fn has_directions(&self) -> bool {
        self.case_dir().join("directions.bin").is_file()
    }

    /// The captured JSON response: its HTTP status and parsed body.
    pub(crate) fn json_response(&self) -> (u16, Value) {
        let mut response = read_json(&self.dir().join("response.json"));
        let status = response["status"]
            .as_u64()
            .and_then(|status| u16::try_from(status).ok())
            .expect("numeric status");
        (status, response["body"].take())
    }

    /// The captured PBF response: its HTTP status and body.
    pub(crate) fn pbf_response(&self) -> (u16, Vec<u8>) {
        let status = fs::read_to_string(self.dir().join("response.status"))
            .expect("response status")
            .trim()
            .parse()
            .expect("numeric status");
        let body = fs::read(self.dir().join("response.bin")).expect("response body");
        (status, body)
    }
}

/// An exact HTTP request a captured case sent (see [`Capture::http`]).
#[derive(Debug, Clone)]
pub(crate) struct HttpCapture {
    /// The method, for example `GET`.
    pub(crate) method: String,
    /// The path and query, as sent.
    pub(crate) target: String,
    /// The `Content-Type` header, if one was set.
    pub(crate) content_type: Option<String>,
    /// The body.
    pub(crate) body: Vec<u8>,
}

/// Every captured case in the given format, sorted by name.
pub(crate) fn captures(format: &str) -> Vec<Capture> {
    let mut out = Vec::new();
    for region in sorted_dirs(Path::new(CAPTURES_DIR)) {
        for case in sorted_dirs(&region) {
            if case.join(format).is_dir() {
                let relative = case
                    .strip_prefix(CAPTURES_DIR)
                    .expect("case is under the captures dir");
                out.push(Capture {
                    case: relative.to_string_lossy().into_owned(),
                    format: format.to_string(),
                });
            }
        }
    }
    assert!(!out.is_empty(), "no captures for {format}");
    out
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .expect("readable fixture directory")
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn read_api(path: &Path) -> Option<Api> {
    let bytes = fs::read(path).ok()?;
    Some(Api::decode(bytes.as_slice()).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

/// Compares two JSON documents under Tyr's match tolerances (see `docs/routing.md`),
/// returning a description of each difference (at most `limit` of them).
///
/// Key order is irrelevant, and floating point numbers may differ by a relative 1e-6.
/// Integers must match exactly, and a number must have the same kind (integer or not)
/// on both sides, since that changes how it is written.
/// Strings, polylines included, must match exactly:
/// a polyline encodes integer coordinates, so decoding to the same coordinates
/// is the same as having the same encoding.
pub(crate) fn json_differences(expected: &Value, actual: &Value, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    compare(expected, actual, &mut String::new(), &mut out, limit);
    out
}

fn compare(
    expected: &Value,
    actual: &Value,
    path: &mut String,
    out: &mut Vec<String>,
    limit: usize,
) {
    if out.len() >= limit {
        return;
    }
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (key, e_value) in e {
                let len = path.len();
                path.push('/');
                path.push_str(key);
                match a.get(key) {
                    Some(a_value) => compare(e_value, a_value, path, out, limit),
                    None => out.push(format!("{path}: missing (expected {e_value})")),
                }
                path.truncate(len);
            }
            for key in a.keys() {
                if !e.contains_key(key) && out.len() < limit {
                    out.push(format!("{path}/{key}: unexpected ({})", a[key]));
                }
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            if e.len() != a.len() {
                out.push(format!(
                    "{path}: length {} != expected {}",
                    a.len(),
                    e.len()
                ));
            }
            for (i, (e_value, a_value)) in e.iter().zip(a).enumerate() {
                let len = path.len();
                path.push('/');
                path.push_str(&i.to_string());
                compare(e_value, a_value, path, out, limit);
                path.truncate(len);
            }
        }
        (Value::Number(e), Value::Number(a)) => {
            let same = if e.is_f64() || a.is_f64() {
                e.is_f64() == a.is_f64()
                    && match (e.as_f64(), a.as_f64()) {
                        (Some(e), Some(a)) => (e - a).abs() <= 1e-6 * e.abs().max(a.abs()),
                        _ => false,
                    }
            } else {
                e == a
            };
            if !same {
                out.push(format!("{path}: {a} != expected {e}"));
            }
        }
        _ => {
            if expected != actual {
                out.push(format!("{path}: {actual} != expected {expected}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tolerances() {
        let expected = json!({"a": 1.0, "b": [1, 2], "c": "x", "d": 100.0});
        assert!(
            json_differences(
                &expected,
                &json!({"d": 100.00001, "c": "x", "b": [1, 2], "a": 1.0}),
                10
            )
            .is_empty()
        );
        assert_eq!(
            json_differences(
                &expected,
                &json!({"a": 1, "b": [1, 2], "c": "x", "d": 100.0}),
                10
            )
            .len(),
            1
        );
        assert_eq!(
            json_differences(
                &expected,
                &json!({"a": 1.0, "b": [1], "c": "y", "d": 100.1, "e": null}),
                10
            )
            .len(),
            4
        );
    }
}
