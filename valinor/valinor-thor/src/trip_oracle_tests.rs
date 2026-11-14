//! Trip building against mainline's, under the "Thor (trip building)" tolerance of
//! `docs/routing.md`: every `trip` field the same, and floats within a relative 1e-6.
//!
//! Two references:
//!
//! - `valinor-tyr`'s captured cases (the `captured_trips` test): each case's Loki→Thor message
//!   and the trip mainline sent Odin, captured from a server (`tools/oracle/capture.py`).
//! - The `thor` helper's `trip` mode (`tools/oracle/cpp/thor.cc`), which prints mainline's
//!   whole Thor→Odin message for generated corpora (the ignored `trips_external` test).
//!
//! Trips are compared through their `Debug` forms, token by token:
//! a float token (one with a decimal point or an exponent) may differ by the tolerance,
//! and anything else must be the same. Prost's `Debug` writes every field,
//! set or not, so the structure (including which messages exist) is compared too.

#![allow(
    clippy::float_cmp,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    reason = "references are compared exactly where mainline's values must be exact"
)]

use core::num::NonZeroUsize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use jiff::Timestamp;
use valhalla_graphtile::tile_provider::{
    CachingTileProvider, DirectoryGraphTileProvider, GraphTileProvider, TarballTileProvider,
};
use valhalla_proto::options::Action;
use valhalla_proto::prost::Message;
use valhalla_proto::{Api, Trip};

use crate::{Thor, ThorConfig, Workspace};

/// The relative tolerance for floats.
const TOLERANCE: f64 = 1e-6;

/// A token of a `Debug` dump: a run of word characters (names and numbers), a string,
/// or one punctuation character.
pub(crate) fn tokens(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
        } else if b == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(bytes.len());
            out.push(&text[start..i]);
        } else if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'+') {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || matches!(bytes[i], b'_' | b'.' | b'-' | b'+'))
            {
                i += 1;
            }
            out.push(&text[start..i]);
        } else {
            out.push(&text[i..=i]);
            i += 1;
        }
    }
    out
}

/// A float token's value (integers are compared exactly, so they aren't floats).
pub(crate) fn float_of(token: &str) -> Option<f64> {
    let is_float =
        token.contains('.') || token.contains('e') || matches!(token, "NaN" | "inf" | "-inf");
    if !is_float {
        return None;
    }
    token.parse().ok()
}

pub(crate) fn floats_match(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan()) || (a - b).abs() <= TOLERANCE * a.abs().max(b.abs())
}

/// The first difference between two dumps, with the field it's in, or `None`.
pub(crate) fn first_difference(ours: &str, theirs: &str) -> Option<String> {
    let (a, b) = (tokens(ours), tokens(theirs));
    let mut field = "";
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        if a.get(i + 1) == Some(&":") {
            field = x;
        }
        if x == y {
            continue;
        }
        if let (Some(fx), Some(fy)) = (float_of(x), float_of(y))
            && floats_match(fx, fy)
        {
            continue;
        }
        return Some(format!("{field}: ours {x}, mainline's {y}"));
    }
    (a.len() != b.len()).then(|| format!("lengths differ after {field}"))
}

/// Where two trips differ (by route, leg, and part of the leg), if they do.
pub(crate) fn compare_trips(ours: &Trip, theirs: &Trip) -> Option<String> {
    if ours.routes.len() != theirs.routes.len() {
        return Some(format!(
            "routes: ours {}, mainline's {}",
            ours.routes.len(),
            theirs.routes.len()
        ));
    }
    for (r, (a, b)) in ours.routes.iter().zip(&theirs.routes).enumerate() {
        if a.legs.len() != b.legs.len() {
            return Some(format!(
                "route {r}: legs: ours {}, mainline's {}",
                a.legs.len(),
                b.legs.len()
            ));
        }
        for (l, (x, y)) in a.legs.iter().zip(&b.legs).enumerate() {
            let at = |part: String| format!("route {r} leg {l} {part}");
            if x.node.len() != y.node.len() {
                return Some(at(format!(
                    "nodes: ours {}, mainline's {}",
                    x.node.len(),
                    y.node.len()
                )));
            }
            for (i, (p, q)) in x.location.iter().zip(&y.location).enumerate() {
                if let Some(d) = first_difference(&format!("{p:?}"), &format!("{q:?}")) {
                    return Some(at(format!("location {i}: {d}")));
                }
            }
            for (n, (p, q)) in x.node.iter().zip(&y.node).enumerate() {
                if let Some(d) = first_difference(&format!("{p:?}"), &format!("{q:?}")) {
                    return Some(at(format!("node {n}: {d}")));
                }
            }
            let rest = |t: &valhalla_proto::TripLeg| {
                let mut t = t.clone();
                t.node.clear();
                t.location.clear();
                format!("{t:?}")
            };
            if let Some(d) = first_difference(&rest(x), &rest(y)) {
                return Some(at(d));
            }
        }
        let rest = |t: &valhalla_proto::TripRoute| {
            let mut t = t.clone();
            t.legs.clear();
            format!("{t:?}")
        };
        if let Some(d) = first_difference(&rest(a), &rest(b)) {
            return Some(format!("route {r}: {d}"));
        }
    }
    None
}

/// What mainline did with a case: the Thor→Odin message, or an error.
enum Reference {
    Ok(Box<Api>),
    Error(u16, String),
}

/// The `thor` helper's `trip` output, by case.
fn read_trip_references(path: &Path) -> BTreeMap<String, Reference> {
    let text = std::fs::read_to_string(path).expect("references");
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut f = line.splitn(3, '\t');
            let (id, kind, rest) = (
                f.next().expect("an id"),
                f.next().expect("a kind"),
                f.next().unwrap_or(""),
            );
            let reference = if kind == "ok" {
                let bytes = STANDARD.decode(rest.trim()).expect("base64");
                Reference::Ok(Box::new(Api::decode(bytes.as_slice()).expect("an Api")))
            } else {
                let (code, message) = rest.split_once(' ').unwrap_or((rest, ""));
                Reference::Error(code.parse().expect("a code"), message.to_owned())
            };
            (id.to_owned(), reference)
        })
        .collect()
}

/// The Loki→Thor messages, by case.
fn read_inputs(path: &Path) -> BTreeMap<String, Api> {
    let text = std::fs::read_to_string(path).expect("inputs");
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            (f.len() >= 3 && f[1] == "ok").then(|| {
                let bytes = STANDARD.decode(f[2].trim()).expect("base64");
                (
                    f[0].to_owned(),
                    Api::decode(bytes.as_slice()).expect("an Api"),
                )
            })
        })
        .collect()
}

/// What a comparison found.
#[derive(Debug, Default)]
struct Report {
    cases: usize,
    trips: usize,
    legs: usize,
    errors_matched: usize,
    /// Cases mainline fails on and Valinor builds, by design (the two via locations at one node).
    divergences: Vec<String>,
    failures: Vec<String>,
    /// The failures by the field where they first differ.
    by_field: BTreeMap<String, usize>,
    /// Cases whose paths differ from mainline's at equal cost (a tie path search may break
    /// either way, within the "Thor (path search)" tolerance), so their trips can't be compared.
    path_ties: Vec<String>,
    /// How many matching trips use each feature of trip building.
    features: BTreeMap<&'static str, usize>,
}

impl Report {
    fn fail(&mut self, id: &str, what: String) {
        let field = what
            .rsplit_once(": ours")
            .map_or(what.as_str(), |(head, _)| head)
            .rsplit(' ')
            .next()
            .unwrap_or("")
            .to_owned();
        *self.by_field.entry(field).or_default() += 1;
        self.failures.push(format!("{id}: {what}"));
    }

    fn print(&self, name: &str) {
        println!(
            "{name}: {} cases, {} trips ({} legs) match, {} errors match, {} failures",
            self.cases,
            self.trips,
            self.legs,
            self.errors_matched,
            self.failures.len()
        );
        if !self.divergences.is_empty() {
            println!("  known divergences: {:?}", self.divergences);
        }
        for tie in &self.path_ties {
            println!("  path tie (not compared): {tie}");
        }
        for (feature, count) in &self.features {
            println!("  {count:6} matching trips with {feature}");
        }
        for (field, count) in &self.by_field {
            println!("  {count:6} first differ at {field}");
        }
        for f in self.failures.iter().take(40) {
            println!("  {f}");
        }
    }
}

/// Whether a leg has two intermediate locations in a row at the same shape index
/// (the trips mainline fails to build).
fn has_shared_shape_index(api: &Api) -> bool {
    api.trip
        .iter()
        .flat_map(|t| &t.routes)
        .flat_map(|r| &r.legs)
        .any(|leg| {
            let intermediates = leg
                .location
                .get(1..leg.location.len().saturating_sub(1))
                .unwrap_or_default();
            intermediates.windows(2).any(|w| {
                let index = |l: &valhalla_proto::Location| {
                    l.correlation.as_ref().map(|c| c.leg_shape_index)
                };
                index(&w[0]) == index(&w[1])
            })
        })
}

/// The edges of each leg of each route.
fn paths_of(trip: &Trip) -> Vec<Vec<u64>> {
    trip.routes
        .iter()
        .flat_map(|r| &r.legs)
        .map(|l| {
            l.node
                .iter()
                .filter_map(|n| n.edge.as_ref().map(|e| e.id))
                .collect()
        })
        .collect()
}

/// The elapsed cost at the end of each leg of each route.
fn costs_of(trip: &Trip) -> Vec<f64> {
    trip.routes
        .iter()
        .flat_map(|r| &r.legs)
        .filter_map(|l| l.node.last()?.cost?.elapsed_cost.map(|c| c.cost))
        .collect()
}

/// Counts the features of trip building a trip uses, to show what the corpora cover.
fn count_features(trip: &Trip, features: &mut BTreeMap<&'static str, usize>) {
    let legs = || trip.routes.iter().flat_map(|r| &r.legs);
    let edges = || legs().flat_map(|l| &l.node).filter_map(|n| n.edge.as_ref());
    let mut count = |name, used: bool| {
        if used {
            *features.entry(name).or_default() += 1;
        }
    };
    count("alternates", trip.routes.len() > 1);
    count("several legs", trip.routes.iter().any(|r| r.legs.len() > 1));
    count(
        "intermediate locations",
        legs().any(|l| l.location.len() > 2),
    );
    count(
        "shape attributes",
        legs().any(|l| l.shape_attributes.is_some()),
    );
    count("elevation", edges().any(|e| !e.elevation.is_empty()));
    count("level changes", legs().any(|l| !l.level_changes.is_empty()));
    count(
        "recostings",
        legs().flat_map(|l| &l.node).any(|n| !n.recosts.is_empty()),
    );
    count("no admins", legs().any(|l| l.admin.is_empty()));
    count("no shape", legs().any(|l| l.shape.is_empty()));
    count("signs", edges().any(|e| e.sign.is_some()));
    count("turn lanes", edges().any(|e| !e.turn_lanes.is_empty()));
    count(
        "tunnel or bridge names",
        edges().any(|e| !e.tunnel_name.is_empty()),
    );
    count(
        "pronunciations",
        edges()
            .flat_map(|e| &e.name)
            .any(|n| n.pronunciation.is_some()),
    );
    count(
        "time restrictions",
        edges().any(|e| e.has_time_restrictions),
    );
    count(
        "bicycle walking",
        edges().any(|e| {
            e.travel_mode == 1
                && e.bicycle_type == 0
                && legs()
                    .flat_map(|l| &l.node)
                    .filter_map(|n| n.edge.as_ref())
                    .any(|x| x.travel_mode == 2)
        }),
    );
    count(
        "edge trimming",
        edges().any(|e| e.source_along_edge > 0.0 && e.source_along_edge < 1.0)
            && legs().any(|l| l.location.len() > 2),
    );
}

/// Routes a Loki→Thor message and builds its trip, as `valinor-thor-service` does.
fn route_and_build<P: GraphTileProvider>(
    thor: &Thor<P>,
    workspace: &mut Workspace,
    input: &Api,
    now: Timestamp,
) -> Result<Api, (u16, String)> {
    let mut api = input.clone();
    let paths = thor
        .route_at(workspace, &mut api, now)
        .map_err(|e| (e.code().code(), e.message().into_owned()))?;
    thor.build_trip_interruptible(&mut api, &paths, now, &|| false)
        .map_err(|e| (e.code().code(), e.message().into_owned()))?;
    Ok(api)
}

/// Compares one case.
fn check<P: GraphTileProvider>(
    thor: &Thor<P>,
    workspace: &mut Workspace,
    id: &str,
    input: &Api,
    reference: &Reference,
    now: Timestamp,
    report: &mut Report,
) {
    report.cases += 1;
    match (route_and_build(thor, workspace, input, now), reference) {
        (Ok(ours), Reference::Ok(theirs)) => {
            let empty = Trip::default();
            let theirs_trip = theirs.trip.as_ref().unwrap_or(&empty);
            let ours_trip = ours.trip.as_ref().unwrap_or(&empty);
            match compare_trips(ours_trip, theirs_trip) {
                // A different path is path search's to explain (see `oracle_tests`):
                // equal costs are a tie that path search may break either way,
                // and anything else is labeled with the costs.
                Some(d)
                    if paths_of(ours_trip) != paths_of(theirs_trip)
                        && costs_of(ours_trip) == costs_of(theirs_trip) =>
                {
                    report.path_ties.push(format!("{id}: {d}"));
                }
                Some(d) if paths_of(ours_trip) != paths_of(theirs_trip) => report.fail(
                    id,
                    format!(
                        "path: ours costs {:?}, mainline's {:?} (first difference {d})",
                        costs_of(ours_trip),
                        costs_of(theirs_trip)
                    ),
                ),
                Some(d) => report.fail(id, d),
                None => {
                    report.trips += 1;
                    report.legs += theirs_trip
                        .routes
                        .iter()
                        .map(|r| r.legs.len())
                        .sum::<usize>();
                    count_features(theirs_trip, &mut report.features);
                }
            }
        }
        (Err((code, message)), Reference::Error(c, m)) => {
            // Mainline's message is the detail after its code's own text.
            if code == *c && message.ends_with(m.as_str()) {
                report.errors_matched += 1;
            } else {
                report.fail(
                    id,
                    format!("error: ours {code} {message}, mainline's {c} {m}"),
                );
            }
        }
        // The known divergence: two intermediate locations at one node, which mainline
        // fails on and Valinor builds (see "Known divergences" in `docs/routing.md`).
        (Ok(ours), Reference::Error(499, m))
            if m.starts_with("leg_shape_index not set") && has_shared_shape_index(&ours) =>
        {
            report.divergences.push(id.to_owned());
        }
        (Ok(_), Reference::Error(c, m)) => {
            report.fail(id, format!("error: ours none, mainline's {c} {m}"));
        }
        (Err((code, message)), Reference::Ok(_)) => {
            report.fail(id, format!("error: ours {code} {message}, mainline's none"));
        }
    }
}

/// A fixed time: the corpora and captures don't use `current`.
fn now() -> Timestamp {
    Timestamp::from_second(1_794_116_280).expect("a time")
}

/// Trip building on the Loki→Thor messages captured from mainline in `valinor-tyr`'s fixtures
/// (Ferrostar's requests among them, with their shape attributes, typed waypoints,
/// and via locations), against the trips mainline sent to Odin.
#[test]
fn captured_trips() {
    let captures =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../valinor-tyr/fixtures/captures/andorra");
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles");
    let provider = DirectoryGraphTileProvider::new(tiles, NonZeroUsize::new(64).expect("64"));
    let thor = Thor::new(ThorConfig::default(), provider);
    let mut workspace = Workspace::default();
    let mut report = Report::default();
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&captures)
        .expect("captures")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    cases.sort();
    for case in cases {
        let name = case
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // `current` times depend on when mainline ran.
        if name.contains("current") {
            continue;
        }
        let Ok(trip) = std::fs::read(case.join("trip.bin")) else {
            continue;
        };
        let reference = Api::decode(trip.as_slice()).expect("an Api");
        for format in ["pbf", "json", "osrm"] {
            let Ok(input) = std::fs::read(case.join(format).join("loki-thor.bin")) else {
                continue;
            };
            let input = Api::decode(input.as_slice()).expect("an Api");
            if input.options.as_ref().is_none_or(|o| {
                o.action != Action::Route as i32
                    || !matches!(
                        valhalla_proto::costing::Type::try_from(o.costing_type),
                        Ok(valhalla_proto::costing::Type::Auto
                            | valhalla_proto::costing::Type::Pedestrian
                            | valhalla_proto::costing::Type::Bicycle)
                    )
            }) {
                break;
            }
            check(
                &thor,
                &mut workspace,
                &format!("{name}/{format}"),
                &input,
                &Reference::Ok(Box::new(reference.clone())),
                now(),
                &mut report,
            );
            // The trip is the same for every format (see `copy-tyr-fixtures.sh`).
            break;
        }
    }
    report.print("captured trips");
    assert!(report.legs > 20, "{report:?}");
    assert!(
        report.failures.is_empty(),
        "{} failures",
        report.failures.len()
    );
}

/// Checks a whole corpus against the `thor` helper's `trip` output:
/// `VALINOR_THOR_ORACLE=<prefix>` (of `<prefix>-loki.tsv` and `<prefix>-trip.tsv`),
/// `VALINOR_THOR_TILES=<tile directory or tarball>`, and `VALINOR_THOR_CONFIG`
/// (a directory with the `valhalla.json` used). `VALINOR_THOR_ONLY` picks cases by ID.
#[test]
#[ignore = "needs a corpus outside the repository"]
fn trips_external() {
    let (Ok(prefix), Ok(tiles)) = (
        std::env::var("VALINOR_THOR_ORACLE"),
        std::env::var("VALINOR_THOR_TILES"),
    ) else {
        println!("set VALINOR_THOR_ORACLE and VALINOR_THOR_TILES");
        return;
    };
    let config = std::env::var("VALINOR_THOR_CONFIG")
        .ok()
        .and_then(|d| std::fs::read_to_string(PathBuf::from(d).join("valhalla.json")).ok())
        .and_then(|json| ThorConfig::from_valhalla_json(&json).ok())
        .unwrap_or_default();
    let inputs = read_inputs(Path::new(&format!("{prefix}-loki.tsv")));
    let references = read_trip_references(Path::new(&format!("{prefix}-trip.tsv")));
    let only = std::env::var("VALINOR_THOR_ONLY").ok();
    let tiles = PathBuf::from(tiles);
    let cache = NonZeroUsize::new(4096).expect("4096");
    let started = std::time::Instant::now();
    let only = only.as_deref();
    let report = if tiles.is_dir() {
        let provider = DirectoryGraphTileProvider::new(tiles, cache);
        run_corpus(
            &Thor::new(config, CachingTileProvider::new(provider, cache)),
            &inputs,
            &references,
            only,
        )
    } else {
        let provider =
            TarballTileProvider::<false, false>::new_readonly(&tiles).expect("a tile tarball");
        run_corpus(
            &Thor::new(config, CachingTileProvider::new(provider, cache)),
            &inputs,
            &references,
            only,
        )
    };
    report.print(&prefix);
    println!("{:?}", started.elapsed());
    assert!(
        report.failures.is_empty(),
        "{} failures",
        report.failures.len()
    );
}

/// Checks every `route` case of a corpus (or those named in `only`, comma-separated).
fn run_corpus<P: GraphTileProvider>(
    thor: &Thor<P>,
    inputs: &BTreeMap<String, Api>,
    references: &BTreeMap<String, Reference>,
    only: Option<&str>,
) -> Report {
    let mut workspace = Workspace::default();
    let mut report = Report::default();
    for (id, reference) in references {
        if only.is_some_and(|o| !o.split(',').any(|x| x == id)) {
            continue;
        }
        let Some(input) = inputs.get(id) else {
            continue;
        };
        // `optimized_route` isn't implemented.
        if input
            .options
            .as_ref()
            .is_none_or(|o| o.action != Action::Route as i32)
        {
            continue;
        }
        check(
            thor,
            &mut workspace,
            id,
            input,
            reference,
            now(),
            &mut report,
        );
    }
    report
}

#[test]
fn compares_debug_dumps_with_a_float_tolerance() {
    assert_eq!(tokens("Foo { a: 1.5, b: \"x y\", c: [] }").len(), 15);
    assert_eq!(first_difference("a: 1.0000001", "a: 1.0"), None);
    assert!(first_difference("a: 1.0001", "a: 1.0").is_some());
    // Integers are exact.
    assert!(first_difference("id: 1000000001", "id: 1000000000").is_some());
    assert_eq!(first_difference("x: NaN", "x: NaN"), None);
    assert_eq!(
        first_difference("a: 1, b: \"q\"", "a: 1, b: \"r\"").as_deref(),
        Some("b: ours \"q\", mainline's \"r\"")
    );
}

/// The second node's edge, which the mutations below change.
fn edge(leg: &mut valhalla_proto::TripLeg) -> &mut valhalla_proto::trip_leg::Edge {
    leg.node[1].edge.as_mut().expect("an edge")
}

/// The comparison catches small changes anywhere in a trip:
/// a float past the tolerance, an integer, a message that exists on one side only,
/// and a location field trip building sets.
#[test]
fn comparison_catches_changes() {
    let case = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valinor-tyr/fixtures/captures/andorra/auto_town");
    let trip = std::fs::read(case.join("trip.bin")).expect("a trip");
    let trip = Api::decode(trip.as_slice())
        .expect("an Api")
        .trip
        .expect("a trip");
    assert_eq!(compare_trips(&trip, &trip), None);
    let mutate = |f: &dyn Fn(&mut valhalla_proto::TripLeg)| {
        let mut changed = trip.clone();
        f(&mut changed.routes[0].legs[0]);
        compare_trips(&changed, &trip)
    };
    assert!(mutate(&|leg| edge(leg).speed *= 1.000_01).is_some_and(|d| d.contains("speed")));
    assert_eq!(mutate(&|leg| edge(leg).speed *= 1.000_000_1), None);
    assert!(
        mutate(&|leg| edge(leg).begin_heading += 1)
            .is_some_and(|d| d.contains("node 1") && d.contains("begin_heading"))
    );
    assert!(mutate(&|leg| edge(leg).sign = Some(valhalla_proto::TripSign::default())).is_some());
    assert!(
        mutate(&|leg| {
            let c = leg.location[1].correlation.as_mut().expect("a correlation");
            c.leg_shape_index += 1;
        })
        .is_some_and(|d| d.contains("location 1"))
    );
    assert!(mutate(&|leg| leg.shape.push('?')).is_some_and(|d| d.contains("shape")));
}

/// Two via locations at the same node (Andorra's `andorra-01959`, from the Thor oracle fixture):
/// mainline fails with "`leg_shape_index` not set for intermediate location";
/// Valinor builds the trip, with both locations at the node's shape index and distance.
#[test]
fn builds_trips_with_two_vias_at_one_node() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/oracle/andorra-loki.tsv");
    let inputs = read_inputs(&fixture);
    let input = inputs.get("andorra-01959").expect("the case");
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles");
    let provider = DirectoryGraphTileProvider::new(tiles, NonZeroUsize::new(64).expect("64"));
    let thor = Thor::new(ThorConfig::default(), provider);
    let mut workspace = Workspace::default();
    let api = route_and_build(&thor, &mut workspace, input, now()).expect("a trip");
    assert!(has_shared_shape_index(&api));
    let trip = api.trip.as_ref().expect("a trip");
    let leg = &trip.routes[0].legs[0];
    assert_eq!(leg.location.len(), 4);
    let (a, b) = (
        leg.location[1].correlation.as_ref().expect("a correlation"),
        leg.location[2].correlation.as_ref().expect("a correlation"),
    );
    assert_eq!(a.leg_shape_index, b.leg_shape_index);
    assert_eq!(a.distance_from_leg_origin, b.distance_from_leg_origin);
    assert!(a.leg_shape_index > 0);
    // Both are at the end of the edge the path reaches them on.
    let edge = leg.node.iter().find(|n| {
        n.edge
            .as_ref()
            .is_some_and(|e| e.end_shape_index == a.leg_shape_index)
    });
    assert!(edge.is_some());
}
