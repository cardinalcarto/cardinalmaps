//! Map matching (`trace_route` and `trace_attributes`) against mainline 3.9.1's Thor,
//! under the "Thor (map matching)" tolerance of `docs/routing.md`.
//!
//! Each case is a captured trace request (`tools/oracle/trace-corpus.py`, captured with
//! `capture.py`): Thor's input is the Loki→Thor message, and its reference is the Thor→Odin
//! message (`trace_route`: the trip, and the options with the shape's correlations), or the
//! response Thor sent (`trace_attributes`, and errors).
//! Trips and options are compared through their `Debug` forms (see `trip_oracle_tests`),
//! and JSON responses after parsing, with floats within a relative 1e-6.
//!
//! The documented Meili divergence (which of a node's edges stands for a candidate at the
//! node, and the candidates' order; see "Known divergences" in `docs/routing.md`) is counted
//! apart: an OSRM response whose only differences are the counts of empty candidate edges
//! (OSRM's `alternatives_count`), or a `trace_attributes` response with alternates whose best
//! match is mainline's.

#![allow(
    clippy::too_many_lines,
    clippy::float_cmp,
    reason = "test reports; references are compared exactly where mainline's values must be exact"
)]

use core::num::NonZeroUsize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde_json::Value;
use valhalla_graphtile::tile_provider::{
    CachingTileProvider, DirectoryGraphTileProvider, GraphTileProvider, TarballTileProvider,
};
use valhalla_proto::options::{Action, Format};
use valhalla_proto::prost::Message;
use valhalla_proto::{Api, Options};
use valhalla_response::ValhallaError;
use valinor_meili::Workspace as MeiliWorkspace;

use crate::trip_oracle_tests::{compare_trips, first_difference, floats_match};
use crate::{Thor, ThorConfig};

/// A fixed time: the corpora don't use `current`.
fn now() -> Timestamp {
    Timestamp::from_second(1_794_116_280).expect("a time")
}

/// What mainline did with a case.
enum Reference {
    /// `trace_route`: the Thor→Odin message.
    Trip(Box<Api>),
    /// `trace_attributes` or an error: the HTTP status and body (JSON, or a PBF `Api`).
    Response(u16, Body),
}

enum Body {
    Json(Value),
    Pbf(Box<Api>),
}

/// One captured case: Thor's input and mainline's output.
pub(crate) struct Case {
    name: String,
    input: Api,
    reference: Reference,
}

/// The single frame-1 message in a tap directory, or the fixture's file.
pub(crate) fn message(dir: &Path, tap: &str) -> Option<Api> {
    let bytes = std::fs::read(dir.join(format!("{tap}.bin")))
        .ok()
        .or_else(|| {
            let tap_dir = dir.join(tap);
            let frame = std::fs::read_dir(tap_dir)
                .ok()?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .find(|p| p.to_string_lossy().ends_with(".1.bin"))?;
            std::fs::read(frame).ok()
        })?;
    Some(Api::decode(bytes.as_slice()).expect("an Api"))
}

/// Reads a case from a fixture directory (`loki-thor.bin`, ...) or a capture's format directory
/// (`loki-thor/<frame>.1.bin`, ...).
fn read_case(name: String, dir: &Path) -> Option<Case> {
    let input = message(dir, "loki-thor")?;
    let reference = if let Some(trip) = message(dir, "thor-odin") {
        Reference::Trip(Box::new(trip))
    } else if let Ok(text) = std::fs::read_to_string(dir.join("response.json")) {
        let response: Value = serde_json::from_str(&text).expect("a response");
        let status = u16::try_from(response["status"].as_u64().expect("a status")).expect("u16");
        Reference::Response(status, Body::Json(response["body"].clone()))
    } else {
        let bin = std::fs::read(dir.join("response.bin"))
            .or_else(|_| std::fs::read(dir.join("response.1.bin")))
            .ok()?;
        let status = std::fs::read_to_string(dir.join("response.status"))
            .ok()?
            .trim()
            .parse()
            .expect("a status");
        Reference::Response(
            status,
            Body::Pbf(Box::new(Api::decode(bin.as_slice()).expect("an Api"))),
        )
    };
    Some(Case {
        name,
        input,
        reference,
    })
}

/// Whether two JSON values match, floats within the tolerance; the first difference if not.
fn json_difference(ours: &Value, theirs: &Value, at: &str) -> Option<String> {
    match (ours, theirs) {
        (Value::Number(a), Value::Number(b)) => {
            if a == b {
                return None;
            }
            match (a.as_f64(), b.as_f64()) {
                (Some(x), Some(y)) if (a.is_f64() || b.is_f64()) && floats_match(x, y) => None,
                _ => Some(format!("{at}: ours {a}, mainline's {b}")),
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!(
                    "{at}: ours {} items, mainline's {}",
                    a.len(),
                    b.len()
                ));
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(i, (x, y))| json_difference(x, y, &format!("{at}[{i}]")))
        }
        (Value::Object(a), Value::Object(b)) => {
            let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            keys.into_iter().find_map(|k| match (a.get(k), b.get(k)) {
                (Some(x), Some(y)) => json_difference(x, y, &format!("{at}.{k}")),
                (x, _) => Some(format!(
                    "{at}.{k}: {}",
                    if x.is_some() {
                        "only ours"
                    } else {
                        "only mainline's"
                    }
                )),
            })
        }
        _ => (ours != theirs).then(|| format!("{at}: ours {ours}, mainline's {theirs}")),
    }
}

/// The options with each shape point's empty candidate edges dropped
/// (OSRM's `alternatives_count`, which the Meili divergence can change).
fn without_empty_edges(options: &Options) -> Options {
    let mut options = options.clone();
    for location in &mut options.shape {
        if let Some(c) = location.correlation.as_mut() {
            c.edges
                .retain(|e| *e != valhalla_proto::PathEdge::default());
        }
    }
    options
}

/// Whether two lists of edges (ID and length) differ only at edges of no length:
/// a match at a node, which Meili may stand for with another of the node's edges
/// (the documented Meili divergence).
fn differ_only_at_nodes(ours: &[(u64, f64)], theirs: &[(u64, f64)]) -> bool {
    ours.len() == theirs.len()
        && ours != theirs
        && ours
            .iter()
            .zip(theirs)
            .all(|(a, b)| a.0 == b.0 || (a.1 == 0.0 && b.1 == 0.0))
}

/// The edges of a `trace_attributes` JSON response (the best match's).
fn json_edges(body: &Value) -> Vec<(u64, f64)> {
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

/// The edges of a trip, across its routes and legs.
fn trip_edges(api: &Api) -> Vec<(u64, f64)> {
    api.trip
        .iter()
        .flat_map(|t| &t.routes)
        .flat_map(|r| &r.legs)
        .flat_map(|l| &l.node)
        .filter_map(|n| n.edge.as_ref())
        .map(|e| (e.id, f64::from(e.length_km)))
        .collect()
}

/// Where two `Api`s differ in what Thor sets: the trip, the options, and the warnings.
fn api_difference(ours: &Api, theirs: &Api) -> Option<String> {
    let empty = valhalla_proto::Trip::default();
    if let Some(d) = compare_trips(
        ours.trip.as_ref().unwrap_or(&empty),
        theirs.trip.as_ref().unwrap_or(&empty),
    ) {
        return Some(format!("trip: {d}"));
    }
    if let Some(d) = first_difference(
        &format!("{:?}", ours.options),
        &format!("{:?}", theirs.options),
    ) {
        return Some(format!("options: {d}"));
    }
    // Mainline's info always exists after Thor, for its statistics (which Valinor doesn't record).
    let warnings = |api: &Api| {
        api.info
            .as_ref()
            .map(|i| i.warnings.clone())
            .unwrap_or_default()
    };
    (warnings(ours) != warnings(theirs)).then(|| {
        format!(
            "warnings: ours {:?}, mainline's {:?}",
            warnings(ours),
            warnings(theirs)
        )
    })
}

/// Cases that tie in Meili without mainline's candidate order: `valinor-meili`'s oracle test
/// shows each is exactly mainline's match when mainline's candidates are replayed
/// (a path through either of two candidates at one node, at equal cost).
const MEILI_TIES: [&str; 1] = ["seattle-0194-uturn/json"];

/// What a run found.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) cases: usize,
    /// Trips, responses, and errors that match, by action.
    pub(crate) trace_route: usize,
    pub(crate) trace_attributes: usize,
    pub(crate) errors: usize,
    /// Cases that differ only as the documented Meili divergence allows.
    pub(crate) meili_divergence: Vec<String>,
    /// Cases whose matches tie in Meili (see [`MEILI_TIES`]).
    pub(crate) meili_ties: Vec<String>,
    pub(crate) failures: Vec<String>,
}

impl Report {
    fn print(&self, name: &str) {
        println!(
            "{name}: {} cases; {} trace_route and {} trace_attributes responses and {} errors \
             match; {} differ by the Meili divergence and {} are Meili ties; {} failures",
            self.cases,
            self.trace_route,
            self.trace_attributes,
            self.errors,
            self.meili_divergence.len(),
            self.meili_ties.len(),
            self.failures.len()
        );
        for t in &self.meili_ties {
            println!("  meili tie: {t}");
        }
        for d in &self.meili_divergence {
            println!("  meili divergence: {d}");
        }
        for f in self.failures.iter().take(60) {
            println!("  {f}");
        }
    }
}

/// Runs a case through Thor as `valinor-thor-service` does: `trace_route` gives the
/// Thor→Odin message, and `trace_attributes` and errors give the response.
fn run<P: GraphTileProvider>(
    thor: &Thor<P>,
    workspace: &mut MeiliWorkspace,
    input: &Api,
) -> (Api, Result<Option<Vec<u8>>, ValhallaError>) {
    let mut api = input.clone();
    let action = api.options.as_ref().map_or(0, |o| o.action);
    let result = if action == Action::TraceRoute as i32 {
        thor.trace_route_interruptible(workspace, &mut api, now(), &|| false)
            .map(|()| None)
    } else {
        thor.trace_attributes_interruptible(workspace, &mut api, now(), &|| false)
            .map(|r| Some(r.body))
    };
    (api, result)
}

/// Checks one case.
fn check<P: GraphTileProvider>(
    thor: &Thor<P>,
    workspace: &mut MeiliWorkspace,
    case: &Case,
    report: &mut Report,
) {
    let failures = report.failures.len();
    check_case(thor, workspace, case, report);
    if report.failures.len() > failures && MEILI_TIES.contains(&case.name.as_str()) {
        let tie = report.failures.pop().unwrap_or_default();
        report.meili_ties.push(tie);
    }
}

fn check_case<P: GraphTileProvider>(
    thor: &Thor<P>,
    workspace: &mut MeiliWorkspace,
    case: &Case,
    report: &mut Report,
) {
    report.cases += 1;
    let name = &case.name;
    let (mut api, result) = run(thor, workspace, &case.input);
    let action = case.input.options.as_ref().map_or(0, |o| o.action);
    let format = case.input.options.as_ref().map_or(0, |o| o.format);
    match (&case.reference, result) {
        (Reference::Trip(theirs), Ok(None)) => match api_difference(&api, theirs) {
            None => report.trace_route += 1,
            Some(d) => {
                // OSRM's candidate counts are the Meili divergence.
                let mut ours = api.clone();
                let mut mainline = (**theirs).clone();
                ours.options = ours.options.as_ref().map(without_empty_edges);
                mainline.options = mainline.options.as_ref().map(without_empty_edges);
                if (format == Format::Osrm as i32 && api_difference(&ours, &mainline).is_none())
                    || differ_only_at_nodes(&trip_edges(&api), &trip_edges(theirs))
                {
                    report.meili_divergence.push(format!("{name}: {d}"));
                } else {
                    report.failures.push(format!("{name}: {d}"));
                }
            }
        },
        (Reference::Response(status, body), Ok(Some(bytes))) => {
            if *status != 200 {
                report
                    .failures
                    .push(format!("{name}: ours 200, mainline's {status}"));
                return;
            }
            if let Ok(dump) = std::env::var("VALINOR_THOR_DUMP") {
                let file = Path::new(&dump).join(name.replace('/', "_"));
                std::fs::write(file, &bytes).expect("a dump");
            }
            let (difference, at_nodes) = match body {
                Body::Json(theirs) => {
                    let ours: Value = serde_json::from_slice(&bytes).expect("JSON");
                    (
                        json_difference(&ours, theirs, ""),
                        differ_only_at_nodes(&json_edges(&ours), &json_edges(theirs)),
                    )
                }
                Body::Pbf(theirs) => {
                    let ours = Api::decode(bytes.as_slice()).expect("an Api");
                    (
                        api_difference(&ours, theirs),
                        differ_only_at_nodes(&trip_edges(&ours), &trip_edges(theirs)),
                    )
                }
            };
            match difference {
                None => report.trace_attributes += 1,
                // With alternates, which candidates are removed as redundant can differ.
                Some(d)
                    if case
                        .input
                        .options
                        .as_ref()
                        .is_some_and(|o| o.has_alternates.is_some())
                        && d.starts_with(".alternate_paths") =>
                {
                    report.meili_divergence.push(format!("{name}: {d}"));
                }
                Some(d) if at_nodes => report.meili_divergence.push(format!("{name}: {d}")),
                Some(d) => report.failures.push(format!("{name}: {d}")),
            }
        }
        (Reference::Response(status, body), Err(error)) => {
            let ours_status = error.http_status().as_u16();
            let ours = error.serialize(&mut api);
            let difference = match body {
                Body::Json(theirs) => {
                    let ours: Value = serde_json::from_slice(&ours).expect("JSON");
                    json_difference(&ours, theirs, "")
                }
                Body::Pbf(theirs) => {
                    let ours = Api::decode(ours.as_slice()).expect("an Api");
                    let errors = |api: &Api| api.info.as_ref().map(|i| i.errors.clone());
                    (errors(&ours) != errors(theirs)).then(|| {
                        format!(
                            "errors: ours {:?}, mainline's {:?}",
                            errors(&ours),
                            errors(theirs)
                        )
                    })
                }
            };
            match difference {
                None if ours_status == *status => report.errors += 1,
                d => report.failures.push(format!(
                    "{name}: error {} ({ours_status}, mainline's {status}): {d:?}",
                    error.message()
                )),
            }
        }
        (Reference::Trip(_), Err(error)) => report.failures.push(format!(
            "{name}: ours {} {}, mainline's a trip",
            error.code().code(),
            error.message()
        )),
        (Reference::Trip(_), Ok(Some(_))) | (Reference::Response(..), Ok(None)) => {
            report.failures.push(format!(
                "{name}: action {action} gave the other kind of output"
            ));
        }
    }
}

/// The Andorra fixture tiles.
pub(crate) fn andorra_tiles() -> DirectoryGraphTileProvider {
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles");
    DirectoryGraphTileProvider::new(tiles, NonZeroUsize::new(64).expect("64"))
}

/// The oracle builds' config (`valhalla_build_config`'s defaults, as the captures used).
pub(crate) fn oracle_config() -> ThorConfig {
    ThorConfig::from_valhalla_json(include_str!("../../valinor-loki/fixtures/valhalla.json"))
        .expect("the config")
}

/// Every case in a directory of fixture cases, or of captures (`<case>/<format>/`).
fn cases_in(dir: &Path) -> Vec<Case> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("the cases")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let mut cases = Vec::new();
    for case in dirs {
        let name = case
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if case.join("request.json").exists() {
            cases.extend(read_case(name, &case));
            continue;
        }
        let mut formats: Vec<PathBuf> = std::fs::read_dir(&case)
            .expect("a case")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        formats.sort();
        for format in formats {
            let label = format!(
                "{name}/{}",
                format
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            );
            cases.extend(read_case(label, &format));
        }
    }
    cases
}

/// Checks cases, with one Meili workspace (as a worker keeps one).
pub(crate) fn check_cases<P: GraphTileProvider>(thor: &Thor<P>, cases: &[Case]) -> Report {
    let mut workspace = MeiliWorkspace::new();
    let mut report = Report::default();
    let only = std::env::var("VALINOR_THOR_ONLY").ok();
    for case in cases {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|x| case.name.starts_with(x)))
        {
            continue;
        }
        check(thor, &mut workspace, case, &mut report);
    }
    report
}

/// The Andorra trace fixtures (`tools/oracle/copy-trace-fixtures.sh`).
#[test]
fn traces_andorra() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/traces/andorra");
    let cases = cases_in(&fixtures);
    let thor = Thor::new(oracle_config(), andorra_tiles());
    let report = check_cases(&thor, &cases);
    report.print("andorra traces");
    assert!(
        report.failures.is_empty(),
        "{} failures",
        report.failures.len()
    );
    assert_eq!(report.cases, cases.len());
    assert!(report.trace_route > 15 && report.trace_attributes > 15 && report.errors > 5);
}

/// The whole captures of a region: `VALINOR_THOR_TRACES=<captures dir>` (such as
/// `/mnt/maps/cardinal/valhalla-oracle/captures/traces-seattle`),
/// `VALINOR_THOR_TILES=<tile directory or tarball>` (the fixture tiles by default),
/// and `VALINOR_THOR_CONFIG=<valhalla.json>` (the oracle config by default).
#[test]
#[ignore = "needs captures outside the repository"]
fn traces_external() {
    let Ok(dir) = std::env::var("VALINOR_THOR_TRACES") else {
        println!("set VALINOR_THOR_TRACES");
        return;
    };
    let config = std::env::var("VALINOR_THOR_CONFIG")
        .ok()
        .map_or_else(oracle_config, |path| {
            ThorConfig::from_valhalla_json(&std::fs::read_to_string(path).expect("the config"))
                .expect("a config")
        });
    let cases = cases_in(Path::new(&dir));
    let cache = NonZeroUsize::new(4096).expect("4096");
    let report = match std::env::var("VALINOR_THOR_TILES") {
        Ok(tiles) if Path::new(&tiles).is_file() => {
            let provider = TarballTileProvider::<false, false>::new_readonly(Path::new(&tiles))
                .expect("a tile tarball");
            check_cases(
                &Thor::new(config, CachingTileProvider::new(provider, cache)),
                &cases,
            )
        }
        Ok(tiles) => {
            let provider = DirectoryGraphTileProvider::new(PathBuf::from(tiles), cache);
            check_cases(
                &Thor::new(config, CachingTileProvider::new(provider, cache)),
                &cases,
            )
        }
        Err(_) => check_cases(&Thor::new(config, andorra_tiles()), &cases),
    };
    report.print(&dir);
    let by_kind: BTreeMap<&str, usize> =
        report.failures.iter().fold(BTreeMap::new(), |mut m, f| {
            let kind = f
                .split(':')
                .next()
                .unwrap_or("")
                .rsplit('-')
                .next()
                .unwrap_or("");
            *m.entry(kind).or_default() += 1;
            m
        });
    println!("failures by kind: {by_kind:?}");
    assert!(
        report.failures.is_empty(),
        "{} failures",
        report.failures.len()
    );
}
