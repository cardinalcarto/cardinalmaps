//! Thor against mainline's, on routes that mainline's Loki correlated.
//!
//! The references come from mainline's own `thor_worker_t`, run by the `thor` helper in
//! `tools/oracle/cpp` on the output of the `loki` helper (see `tools/oracle/README.md`),
//! so both sides start from the same Loki→Thor message.
//! Each case is compared under the "Thor (path search)" tolerances of `docs/routing.md`:
//!
//! - the same errors (code and message);
//! - the same warnings, and the same location times;
//! - the same number of routes (alternates) and legs, with the same algorithms;
//! - for each leg, the same edges with the same costs, or, where mainline's path differs,
//!   a total cost within 0.1% of mainline's and never higher, and a length and time
//!   within 0.5%.
//!
//! The committed fixtures (`fixtures/oracle`) are samples of the corpora;
//! set `VALINOR_THOR_ORACLE` to the prefix of a `<prefix>-loki.tsv` and `<prefix>-thor.tsv`
//! pair, and `VALINOR_THOR_TILES` to their tiles, to check a whole corpus
//! (the `oracle_external` test, which is ignored by default).

#![allow(
    clippy::float_cmp,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "references are compared exactly where mainline's values must be exact"
)]

use core::num::NonZeroUsize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use jiff::Timestamp;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::GraphTile;
use valhalla_graphtile::tile_provider::{
    CachingTileProvider, DirectoryGraphTileProvider, GraphTileProvider, TarballTileProvider,
};
use valhalla_proto::Api;
use valhalla_proto::options::Action;
use valhalla_proto::prost::Message;

use crate::{Paths, Thor, ThorConfig, Workspace};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/oracle")
}

/// One edge of a reference path.
#[derive(Clone, Copy, Debug)]
struct RefEdge {
    id: u64,
    cost: f32,
    secs: f32,
    transition: Option<(f32, f32)>,
}

#[derive(Clone, Debug, Default)]
struct RefLeg {
    algorithms: Vec<String>,
    edges: Vec<RefEdge>,
}

#[derive(Clone, Debug, Default)]
struct Reference {
    error: Option<(u16, String)>,
    warnings: Vec<u64>,
    locations: Vec<String>,
    routes: Vec<Vec<RefLeg>>,
}

fn hex_f32(s: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(s, 16).expect("hex float"))
}

/// Parses the `thor` helper's summary, or the fixtures' compact form of it
/// (`e <id> <cost bits> <secs bits>` edges, without transitions).
fn parse_reference(text: &str) -> Reference {
    let mut r = Reference::default();
    for line in text.lines() {
        let (kind, rest) = line.split_once(' ').unwrap_or((line, ""));
        match kind {
            "ok" | "" => {}
            "error" => {
                let (code, message) = rest.split_once(' ').unwrap_or((rest, ""));
                r.error = Some((code.parse().expect("a code"), message.to_owned()));
            }
            "warning" => r.warnings.push(rest.parse().expect("a warning code")),
            "location" => {
                let (_, value) = rest.split_once(' ').unwrap_or((rest, ""));
                r.locations.push(value.to_owned());
            }
            "route" => r.routes.push(Vec::new()),
            "leg" => r.routes.last_mut().expect("a route").push(RefLeg {
                algorithms: rest
                    .split(',')
                    .filter(|a| !a.is_empty())
                    .map(str::to_owned)
                    .collect(),
                edges: Vec::new(),
            }),
            "edge" => {
                let f: Vec<&str> = rest.split(' ').collect();
                let n = |i: usize| f[i].parse::<f64>().expect("a number") as f32;
                r.routes
                    .last_mut()
                    .and_then(|legs| legs.last_mut())
                    .expect("a leg")
                    .edges
                    .push(RefEdge {
                        id: f[0].parse().expect("an id"),
                        secs: n(1),
                        cost: n(2),
                        transition: Some((n(3), n(4))),
                    });
            }
            "e" => {
                let f: Vec<&str> = rest.split(' ').collect();
                r.routes
                    .last_mut()
                    .and_then(|legs| legs.last_mut())
                    .expect("a leg")
                    .edges
                    .push(RefEdge {
                        id: f[0].parse().expect("an id"),
                        cost: hex_f32(f[1]),
                        secs: hex_f32(f[2]),
                        transition: None,
                    });
            }
            other => panic!("unknown reference line {other:?}"),
        }
    }
    r
}

/// The compact form of a reference, for fixtures.
fn compact(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("edge ") {
            let f: Vec<&str> = rest.split(' ').collect();
            let bits = |s: &str| (s.parse::<f64>().expect("a number") as f32).to_bits();
            let _ = writeln!(out, "e {} {:08x} {:08x}", f[0], bits(f[2]), bits(f[1]));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn decode(s: &str) -> Vec<u8> {
    STANDARD.decode(s.trim()).expect("base64")
}

/// The Loki→Thor messages, by case.
fn read_inputs(path: &Path) -> BTreeMap<String, Api> {
    let text = std::fs::read_to_string(path).expect("inputs");
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            (f.len() >= 3 && f[1] == "ok").then(|| {
                (
                    f[0].to_owned(),
                    Api::decode(decode(f[2]).as_slice()).expect("an Api"),
                )
            })
        })
        .collect()
}

/// The references, by case: the helper's output (base64 summaries)
/// or a fixture (`case <id>` blocks of compact summaries).
fn read_references(path: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(path).expect("references");
    let mut out = BTreeMap::new();
    if text.starts_with("case ") {
        let mut current: Option<(String, String)> = None;
        for line in text.lines() {
            if let Some(id) = line.strip_prefix("case ") {
                if let Some((id, body)) = current.take() {
                    out.insert(id, body);
                }
                current = Some((id.to_owned(), String::new()));
            } else if let Some((_, body)) = current.as_mut() {
                body.push_str(line);
                body.push('\n');
            }
        }
        if let Some((id, body)) = current {
            out.insert(id, body);
        }
    } else {
        for line in text.lines().filter(|l| !l.is_empty()) {
            let (id, b64) = line.split_once('\t').expect("id and summary");
            out.insert(
                id.to_owned(),
                String::from_utf8(decode(b64)).expect("UTF-8"),
            );
        }
    }
    out
}

/// What a comparison found.
#[derive(Debug, Default)]
struct Report {
    cases: usize,
    errors_matched: usize,
    legs: usize,
    identical_legs: usize,
    identical_costs: usize,
    equivalent_legs: usize,
    routes_with_alternates: usize,
    /// Cases with actions other than `route`.
    skipped_actions: usize,
    /// Mainline errors in trip building, which comes after path search.
    trip_building_errors: Vec<String>,
    worst_cost_diff: f64,
    failures: Vec<String>,
}

fn relative(a: f64, b: f64) -> f64 {
    if a == b {
        0.0
    } else {
        (a - b).abs() / a.abs().max(b.abs()).max(1e-9)
    }
}

/// A leg's length in meters: its edges, less the parts before the start and after the end.
fn leg_length<P: GraphTileProvider>(
    provider: &P,
    edges: &[u64],
    begin: Option<f64>,
    end: Option<f64>,
) -> f64 {
    let length = |id: u64| {
        let id = GraphId::try_from_id(id).expect("an id");
        let tile = provider.get_handle_for_tile_containing(id).expect("a tile");
        f64::from(tile.get_directed_edge(id).expect("an edge").length())
    };
    let mut total: f64 = edges.iter().map(|&e| length(e)).sum();
    if let Some(&first) = edges.first() {
        total -= length(first) * begin.unwrap_or(0.0);
    }
    if let Some(&last) = edges.last() {
        total -= length(last) * (1.0 - end.unwrap_or(1.0));
    }
    total
}

/// The percent along of an edge among a location's candidates (filtered or not).
fn percent_of(api: &Api, location: usize, edge: u64) -> Option<f64> {
    let loc = api.options.as_ref()?.locations.get(location)?;
    let c = loc.correlation.as_ref()?;
    c.edges
        .iter()
        .chain(&c.filtered_edges)
        .find(|e| e.graph_id == edge)
        .map(|e| e.percent_along)
}

fn compare_case<P: GraphTileProvider>(
    provider: &P,
    id: &str,
    input: &Api,
    reference: &Reference,
    ours: &Result<Paths, valhalla_response::ValhallaError>,
    after: &Api,
    report: &mut Report,
) {
    report.cases += 1;
    let fail = |report: &mut Report, why: String| report.failures.push(format!("{id}: {why}"));
    match (&reference.error, ours) {
        (Some((code, message)), Err(e)) => {
            if e.code().code() == *code && e.message() == *message {
                report.errors_matched += 1;
            } else {
                fail(
                    report,
                    format!(
                        "error {} {:?}, mainline {code} {message:?}",
                        e.code().code(),
                        e.message()
                    ),
                );
            }
            return;
        }
        (Some((499, message)), Ok(_)) if message.starts_with("leg_shape_index") => {
            report.trip_building_errors.push(id.to_owned());
            return;
        }
        (Some((code, message)), Ok(_)) => {
            fail(
                report,
                format!("found a path; mainline: error {code} {message}"),
            );
            return;
        }
        (None, Err(e)) => {
            fail(report, format!("error {e}; mainline found a path"));
            return;
        }
        (None, Ok(_)) => {}
    }
    let Ok(paths) = ours else { return };

    let warnings: Vec<u64> = after
        .info
        .as_ref()
        .map(|i| i.warnings.iter().map(|w| w.code).collect())
        .unwrap_or_default();
    if warnings != reference.warnings {
        fail(
            report,
            format!("warnings {warnings:?}, mainline {:?}", reference.warnings),
        );
    }
    let locations: Vec<String> = after
        .options
        .as_ref()
        .map(|o| {
            o.locations
                .iter()
                .map(|l| {
                    format!(
                        "{}|{}|{}",
                        l.date_time, l.time_zone_offset, l.time_zone_name
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if locations != reference.locations {
        fail(
            report,
            format!(
                "locations {locations:?}, mainline {:?}",
                reference.locations
            ),
        );
    }
    if paths.routes.len() != reference.routes.len() {
        fail(
            report,
            format!(
                "{} routes, mainline {}",
                paths.routes.len(),
                reference.routes.len()
            ),
        );
        return;
    }
    if paths.routes.len() > 1 {
        report.routes_with_alternates += 1;
    }
    for (r, (route, ref_route)) in paths.routes.iter().zip(&reference.routes).enumerate() {
        if route.legs.len() != ref_route.len() {
            fail(
                report,
                format!(
                    "route {r}: {} legs, mainline {}",
                    route.legs.len(),
                    ref_route.len()
                ),
            );
            continue;
        }
        for (l, (leg, ref_leg)) in route.legs.iter().zip(ref_route).enumerate() {
            report.legs += 1;
            let algorithms: Vec<String> = leg.algorithms.iter().map(|a| (*a).to_owned()).collect();
            if algorithms != ref_leg.algorithms {
                fail(
                    report,
                    format!(
                        "route {r} leg {l}: algorithms {algorithms:?}, mainline {:?}",
                        ref_leg.algorithms
                    ),
                );
            }
            let ours_ids: Vec<u64> = leg.edges.iter().map(|e| e.edge_id.value()).collect();
            let ref_ids: Vec<u64> = ref_leg.edges.iter().map(|e| e.id).collect();
            if ours_ids == ref_ids {
                report.identical_legs += 1;
                let mut exact = true;
                for (e, re) in leg.edges.iter().zip(&ref_leg.edges) {
                    let diff = relative(f64::from(e.elapsed_cost.cost), f64::from(re.cost))
                        .max(relative(f64::from(e.elapsed_cost.secs), f64::from(re.secs)));
                    let tdiff = re.transition.map_or(0.0, |(ts, tc)| {
                        relative(f64::from(e.transition_cost.secs), f64::from(ts))
                            .max(relative(f64::from(e.transition_cost.cost), f64::from(tc)))
                    });
                    report.worst_cost_diff = report.worst_cost_diff.max(diff.max(tdiff));
                    exact = exact && diff == 0.0 && tdiff == 0.0;
                }
                if exact {
                    report.identical_costs += 1;
                } else if report.worst_cost_diff > 1e-6 {
                    fail(
                        report,
                        format!("route {r} leg {l}: same edges, costs differ"),
                    );
                }
                continue;
            }
            // A different path: it must be as good, within the tolerances.
            let (Some(ours_last), Some(ref_last)) = (leg.edges.last(), ref_leg.edges.last()) else {
                fail(report, format!("route {r} leg {l}: an empty path"));
                continue;
            };
            let (oc, rc) = (
                f64::from(ours_last.elapsed_cost.cost),
                f64::from(ref_last.cost),
            );
            let (os, rs) = (
                f64::from(ours_last.elapsed_cost.secs),
                f64::from(ref_last.secs),
            );
            let ref_begin = ref_ids
                .first()
                .and_then(|&e| percent_of(input, leg.origin, e));
            let ref_end = ref_ids
                .last()
                .and_then(|&e| percent_of(input, leg.destination, e));
            let ol = leg_length(provider, &ours_ids, leg.begin_percent, leg.end_percent);
            let rl = leg_length(provider, &ref_ids, ref_begin, ref_end);
            let ok = oc <= rc * (1.0 + 1e-6)
                && relative(oc, rc) <= 1e-3
                && relative(os, rs) <= 5e-3
                && relative(ol, rl) <= 5e-3;
            if ok {
                report.equivalent_legs += 1;
            } else {
                fail(
                    report,
                    format!(
                        "route {r} leg {l}: {} edges, cost {oc} time {os} length {ol}; \
                         mainline {} edges, cost {rc} time {rs} length {rl}",
                        ours_ids.len(),
                        ref_ids.len()
                    ),
                );
            }
        }
    }
}

/// Runs every case with a reference through Thor and compares.
fn run_oracle<P: GraphTileProvider>(
    provider: P,
    config: ThorConfig,
    inputs: &BTreeMap<String, Api>,
    references: &BTreeMap<String, String>,
) -> Report {
    let thor = Thor::new(config, provider);
    let mut workspace = Workspace::default();
    let mut report = Report::default();
    // Corpora don't use `current`, so the time doesn't matter.
    let now = Timestamp::from_second(1_794_116_280).expect("a time");
    for (id, text) in references {
        let Some(input) = inputs.get(id) else {
            continue;
        };
        // `optimized_route` isn't implemented (see the crate docs).
        if input
            .options
            .as_ref()
            .is_none_or(|o| o.action != Action::Route as i32)
        {
            report.skipped_actions += 1;
            continue;
        }
        let reference = parse_reference(text);
        let mut api = input.clone();
        let ours = thor.route_at(&mut workspace, &mut api, now);
        compare_case(
            thor.provider(),
            id,
            input,
            &reference,
            &ours,
            &api,
            &mut report,
        );
    }
    report
}

fn print_report(name: &str, report: &Report) {
    println!(
        "{name}: {} cases ({} other actions skipped), {} errors matched, {} legs: {} with mainline's edges ({} with its exact costs), \
         {} equivalent; {} with alternates; worst cost difference on the same edges {:e}; \
         {} trip building errors (after path search): {:?}",
        report.cases,
        report.skipped_actions,
        report.errors_matched,
        report.legs,
        report.identical_legs,
        report.identical_costs,
        report.equivalent_legs,
        report.routes_with_alternates,
        report.worst_cost_diff,
        report.trip_building_errors.len(),
        report.trip_building_errors,
    );
    for f in report.failures.iter().take(40) {
        println!("  {f}");
    }
    if report.failures.len() > 40 {
        println!("  ... and {} more", report.failures.len() - 40);
    }
}

fn mainline_config(dir: &str) -> ThorConfig {
    let path = PathBuf::from(dir).join("valhalla.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|json| ThorConfig::from_valhalla_json(&json).ok())
        .unwrap_or_default()
}

#[test]
fn oracle_andorra() {
    let dir = fixtures();
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles");
    let provider = DirectoryGraphTileProvider::new(tiles, NonZeroUsize::new(64).expect("64"));
    let report = run_oracle(
        provider,
        ThorConfig::default(),
        &read_inputs(&dir.join("andorra-loki.tsv")),
        &read_references(&dir.join("andorra-thor.txt")),
    );
    print_report("andorra", &report);
    assert!(report.cases > 0);
    assert!(
        report.failures.is_empty(),
        "{} failures",
        report.failures.len()
    );
}

/// Checks a whole corpus: `VALINOR_THOR_ORACLE=<prefix>` (of `<prefix>-loki.tsv` and
/// `<prefix>-thor.tsv`) and `VALINOR_THOR_TILES=<tile directory or tarball>`;
/// `VALINOR_THOR_CONFIG` names a directory with the `valhalla.json` used.
/// `VALINOR_THOR_FIXTURE=<path>` also writes a sample of the cases as a fixture
/// (every error and alternate, and every `VALINOR_THOR_STRIDE`th other case, default 6).
#[test]
#[ignore = "needs a corpus outside the repository"]
fn oracle_external() {
    let (Ok(prefix), Ok(tiles)) = (
        std::env::var("VALINOR_THOR_ORACLE"),
        std::env::var("VALINOR_THOR_TILES"),
    ) else {
        println!("set VALINOR_THOR_ORACLE and VALINOR_THOR_TILES");
        return;
    };
    let config = std::env::var("VALINOR_THOR_CONFIG")
        .map(|d| mainline_config(&d))
        .unwrap_or_default();
    let inputs = read_inputs(Path::new(&format!("{prefix}-loki.tsv")));
    let references = read_references(Path::new(&format!("{prefix}-thor.tsv")));
    let only = std::env::var("VALINOR_THOR_ONLY").ok();
    let references: BTreeMap<String, String> = references
        .into_iter()
        .filter(|(id, _)| only.as_ref().is_none_or(|o| o.split(',').any(|x| x == id)))
        .collect();
    let started = std::time::Instant::now();
    let tiles = PathBuf::from(tiles);
    let cache = NonZeroUsize::new(4096).expect("4096");
    let report = if tiles.is_dir() {
        run_oracle(
            DirectoryGraphTileProvider::new(tiles, cache),
            config,
            &inputs,
            &references,
        )
    } else {
        let provider =
            TarballTileProvider::<false, false>::new_readonly(&tiles).expect("a tile tarball");
        run_oracle(
            CachingTileProvider::new(provider, cache),
            config,
            &inputs,
            &references,
        )
    };
    print_report(&prefix, &report);
    println!("{:?}", started.elapsed());

    if let Ok(fixture) = std::env::var("VALINOR_THOR_FIXTURE") {
        let stride: usize = std::env::var("VALINOR_THOR_STRIDE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6);
        let failed: Vec<&str> = report
            .failures
            .iter()
            .filter_map(|f| f.split(':').next())
            .collect();
        let mut refs = String::new();
        let mut loki = String::new();
        let loki_lines = std::fs::read_to_string(format!("{prefix}-loki.tsv")).expect("inputs");
        let loki_by_id: BTreeMap<&str, &str> = loki_lines
            .lines()
            .filter_map(|l| l.split_once('\t').map(|(id, _)| (id, l)))
            .collect();
        for (i, (id, text)) in references.iter().enumerate() {
            // Every error and alternate; a sample of the rest.
            let reference = parse_reference(text);
            let interesting = reference.error.is_some() || reference.routes.len() > 1;
            if (!interesting && i % stride != 0) || failed.contains(&id.as_str()) {
                continue;
            }
            let Some(line) = loki_by_id.get(id.as_str()) else {
                continue;
            };
            let _ = writeln!(refs, "case {id}");
            refs.push_str(&compact(text));
            loki.push_str(line);
            loki.push('\n');
        }
        std::fs::write(format!("{fixture}-thor.txt"), refs).expect("write the fixture");
        std::fs::write(format!("{fixture}-loki.tsv"), loki).expect("write the fixture");
    }
    assert!(
        report.failures.is_empty(),
        "{} failures",
        report.failures.len()
    );
}

/// Searches stop when interrupted, after mainline's 5000 iterations,
/// and Thor can be used again afterwards.
#[test]
fn interrupts_long_searches() {
    let dir = fixtures();
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles");
    let provider = DirectoryGraphTileProvider::new(tiles, NonZeroUsize::new(64).expect("64"));
    let thor = Thor::new(ThorConfig::default(), provider);
    let inputs = read_inputs(&dir.join("andorra-loki.tsv"));
    let references = read_references(&dir.join("andorra-thor.txt"));
    let now = Timestamp::from_second(1_794_116_280).expect("a time");
    let mut workspace = Workspace::default();
    let mut interrupted = 0;
    let mut finished = 0;
    for (id, text) in &references {
        let reference = parse_reference(text);
        let Some(input) = inputs.get(id) else {
            continue;
        };
        if reference.error.is_some()
            || input
                .options
                .as_ref()
                .is_none_or(|o| o.action != Action::Route as i32)
        {
            continue;
        }
        let mut api = input.clone();
        match thor.route_interruptible(&mut workspace, &mut api, now, &|| true) {
            Err(e) => {
                assert_eq!(e.code().code(), 499, "{id}");
                assert!(e.message().contains("interrupted"), "{id}: {}", e.message());
                interrupted += 1;
            }
            // Short searches finish before the first check.
            Ok(_) => finished += 1,
        }
        // The same request runs to the end afterwards.
        let mut api = input.clone();
        let mut report = Report::default();
        let ours = thor.route_at(&mut workspace, &mut api, now);
        compare_case(
            thor.provider(),
            id,
            input,
            &reference,
            &ours,
            &api,
            &mut report,
        );
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        if interrupted >= 5 && finished >= 5 {
            break;
        }
    }
    assert!(
        interrupted >= 5 && finished >= 5,
        "{interrupted} {finished}"
    );
}
