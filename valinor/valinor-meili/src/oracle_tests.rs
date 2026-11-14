//! Matches traces and compares them with mainline 3.9.1's matcher
//! (the `meili` helper in `tools/oracle/cpp`; see `tools/oracle/README.md`).
//!
//! Each case is a trace request (`{"name", "action", "body"}`), parsed by the front as mainline's
//! `ParseApi` does, then matched as Thor does before it forms paths: the matcher made from the
//! request's options, the measurements of its shape, and `offline_match` with
//! k = `alternates` + 1 for `trace_attributes` and 1 for `trace_route`.
//!
//! The tolerances are those of `docs/routing.md`: every result's edge, and every segment,
//! the same, with positions and distances within a relative 1e-6, and scores within a relative
//! 1e-6; errors the same. A case whose matches differ but whose scores are the same is a tie:
//! equal costs that mainline breaks by hash order (see `candidate_search` and `viterbi`).
//! State IDs aren't compared, since they number candidates in that order.

use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use valhalla_graphtile::tile_provider::{DirectoryGraphTileProvider, GraphTileProvider};
use valhalla_proto::Api;
use valhalla_proto::options::Action;

use crate::{MapMatcherFactory, MatchResults, Measurement, MeiliError, Workspace};

/// The matches and, per result, its state's candidate edge count; or the error code and message.
type RunResult = Result<(Vec<MatchResults>, Vec<Vec<Option<usize>>>), (String, String)>;

/// One case's outcome, as the helper prints it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Outcome {
    /// `error <code> <message>`.
    error: Option<(String, String)>,
    matches: Vec<Match>,
    /// Mainline's candidate order per column, when the helper printed it.
    columns: Vec<Vec<Vec<u64>>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Match {
    score: f32,
    results: Vec<Vec<String>>,
    segments: Vec<Vec<String>>,
}

fn number(value: f64) -> String {
    // Rust's shortest round-trip form; compared numerically.
    format!("{value:?}")
}

/// Our outcome in the helper's terms.
fn outcome(results: RunResult) -> Outcome {
    match results {
        Err(error) => Outcome {
            error: Some(error),
            ..Outcome::default()
        },
        Ok((all, counts)) => Outcome {
            error: None,
            columns: Vec::new(),
            matches: all
                .iter()
                .zip(counts)
                .map(|(m, counts)| Match {
                    score: m.score,
                    results: m
                        .results
                        .iter()
                        .zip(counts)
                        .map(|(r, count)| {
                            vec![
                                number(r.lnglat.0),
                                number(r.lnglat.1),
                                number(r.distance_from),
                                r.edge_id.map_or("-".to_owned(), |e| e.value().to_string()),
                                number(r.distance_along),
                                number(r.epoch_time),
                                r.state_id
                                    .map_or("-".to_owned(), |s| format!("{}/{}", s.time, s.id)),
                                u8::from(r.is_break_point).to_string(),
                                u8::from(r.begins_discontinuity).to_string(),
                                u8::from(r.ends_discontinuity).to_string(),
                                count.map_or("-".to_owned(), |c| c.to_string()),
                            ]
                        })
                        .collect(),
                    segments: m
                        .segments
                        .iter()
                        .map(|s| {
                            let idx =
                                |i: Option<usize>| i.map_or("-1".to_owned(), |i| i.to_string());
                            vec![
                                s.edge_id.value().to_string(),
                                number(s.source),
                                number(s.target),
                                idx(s.first_match_idx),
                                idx(s.last_match_idx),
                                s.restriction_idx.to_string(),
                                u8::from(s.discontinuity).to_string(),
                            ]
                        })
                        .collect(),
                })
                .collect(),
        },
    }
}

/// Parses the helper's output: `case <name>` blocks up to `end`.
fn parse_expected(text: &str) -> Vec<(String, Outcome)> {
    let mut cases = Vec::new();
    let mut current: Option<(String, Outcome)> = None;
    for line in text.lines() {
        let (kind, rest) = line.split_once(' ').unwrap_or((line, ""));
        match kind {
            "case" => current = Some((rest.to_owned(), Outcome::default())),
            "end" => cases.push(current.take().expect("end without case")),
            "error" => {
                let (code, message) = rest.split_once(' ').unwrap_or((rest, ""));
                current.as_mut().expect("error outside a case").1.error =
                    Some((code.to_owned(), message.to_owned()));
            }
            "column" => current
                .as_mut()
                .expect("column outside a case")
                .1
                .columns
                .push(
                    rest.split(' ')
                        .filter(|p| !p.is_empty())
                        .map(|state| {
                            state
                                .split(',')
                                .map(|e| e.parse().expect("an edge ID"))
                                .collect()
                        })
                        .collect(),
                ),
            // Diagnostics of the helper's `columns` mode.
            "layer" | "removed" | "debug" => {}
            "match" => current
                .as_mut()
                .expect("match outside a case")
                .1
                .matches
                .push(Match {
                    score: rest.parse().expect("score"),
                    ..Match::default()
                }),
            "result" | "segment" => {
                let fields = rest.split(' ').map(str::to_owned).collect();
                let m = current
                    .as_mut()
                    .and_then(|c| c.1.matches.last_mut())
                    .expect("a record outside a match");
                if kind == "result" {
                    m.results.push(fields);
                } else {
                    m.segments.push(fields);
                }
            }
            _ => panic!("unexpected line {line:?}"),
        }
    }
    cases
}

/// Matches one request as the helper does.
fn run<P: GraphTileProvider>(
    factory: &MapMatcherFactory,
    provider: &P,
    workspace: &mut Workspace,
    action: &str,
    body: &str,
) -> RunResult {
    let action = Action::from_str_name(action).expect("a known action");
    let mut api = Api::default();
    valinor_tyr::front::parse_json_request(body.as_bytes(), action, &mut api)
        .map_err(|e| (e.code().code().to_string(), e.message().into_owned()))?;
    let options = api.options.unwrap_or_default();
    let err = |e: MeiliError| match e {
        MeiliError::NoCandidates => ("443".to_owned(), e.to_string()),
        MeiliError::Measurement(_) => ("424".to_owned(), e.to_string()),
        other => ("exception".to_owned(), other.to_string()),
    };
    let matcher = factory.create(&options).map_err(err)?;
    let measurements = Measurement::from_shape(&options.shape, matcher.config())
        .map_err(|e| err(MeiliError::Measurement(e)))?;
    let k = if options.action == Action::TraceAttributes as i32 {
        match options.has_alternates {
            Some(valhalla_proto::options::HasAlternates::Alternates(a)) => a + 1,
            None => 1,
        }
    } else {
        1
    };
    let all = matcher
        .offline_match(provider, workspace, &measurements, k, &|| false)
        .map_err(err)?;
    let counts = all
        .iter()
        .map(|m| {
            m.results
                .iter()
                .map(|r| {
                    r.state_id
                        .and_then(|s| workspace.candidate(s))
                        .map(|c| c.edges.len())
                })
                .collect()
        })
        .collect();
    Ok((all, counts))
}

fn close(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1e-3),
        _ => false,
    }
}

/// The fields of a result record that are the state ID and its candidate's edge count.
const STATE_FIELD: usize = 6;
const COUNT_FIELD: usize = 10;

/// Whether two records match: numbers within tolerance, state IDs only as present or not,
/// and (unless `counts`) candidate edge counts not at all.
fn same_record(a: &[String], b: &[String], result: bool, counts: bool) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).enumerate().all(|(i, (x, y))| {
            if result && i == STATE_FIELD {
                (x == "-") == (y == "-")
            } else if result && i == COUNT_FIELD && !counts {
                true
            } else {
                close(x, y)
            }
        })
}

fn same_match(a: &Match, b: &Match, counts: bool) -> bool {
    a.results.len() == b.results.len()
        && a.segments.len() == b.segments.len()
        && a.results
            .iter()
            .zip(&b.results)
            .all(|(x, y)| same_record(x, y, true, counts))
        && a.segments
            .iter()
            .zip(&b.segments)
            .all(|(x, y)| same_record(x, y, false, true))
}

fn same_score(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

#[derive(Debug, Default)]
struct Tally {
    cases: usize,
    /// Every field the same (but state IDs).
    exact: usize,
    /// The same, but for the edge count of a candidate at a node: mainline picks which of the
    /// node's edges stands for it by hash order.
    node_counts: usize,
    /// Different matches with the same scores.
    ties: usize,
    /// Costings Valinor doesn't have (multimodal, which Loki rejects for traces anyway).
    unsupported: usize,
    /// Alternates (k > 1) whose best match is the same but later ones differ:
    /// which candidates are removed as redundant after each path depends on the order of
    /// the candidates, which is mainline's hash order (the replayed comparison checks them).
    alternates_order: usize,
    errors: usize,
    failures: Vec<String>,
}

fn describe(name: &str, ours: &Outcome, theirs: &Outcome) -> String {
    let mut s = format!("{name}:\n");
    let show = |s: &mut String, label: &str, o: &Outcome| {
        let _ = writeln!(s, "  {label}: error {:?}", o.error);
        for m in &o.matches {
            let _ = writeln!(s, "  {label}: match {}", m.score);
            for r in &m.results {
                let _ = writeln!(s, "    result {}", r.join(" "));
            }
            for g in &m.segments {
                let _ = writeln!(s, "    segment {}", g.join(" "));
            }
        }
    };
    show(&mut s, "ours", ours);
    show(&mut s, "mainline", theirs);
    s
}

/// Compares every case of a corpus with the helper's output for it.
///
/// With `replay`, each column's candidates are mainline's (from the helper's `columns` lines),
/// in mainline's order, so that every field must match, including which edge a candidate at a
/// node has; otherwise they are Valinor's own, and the differences that order makes are
/// counted apart.
fn compare<P: GraphTileProvider>(
    corpus: &Path,
    expected: &Path,
    provider: &P,
    config: &Path,
    replay: bool,
) -> Tally {
    let factory =
        MapMatcherFactory::from_valhalla_json(&std::fs::read_to_string(config).expect("config"))
            .expect("the meili config");
    let expected = parse_expected(&std::fs::read_to_string(expected).expect("expected"));
    let corpus = std::fs::read_to_string(corpus).expect("corpus");
    let mut workspace = Workspace::new();
    let mut tally = Tally::default();
    let mut expected = expected.into_iter();
    for line in corpus.lines().filter(|l| !l.trim().is_empty()) {
        let case: serde_json::Value = serde_json::from_str(line).expect("a JSON case");
        if case["body"].is_null() {
            continue;
        }
        let name = case["name"].as_str().expect("name");
        let (expected_name, theirs) = expected.next().expect("a reference for every case");
        assert_eq!(expected_name, name, "the reference is for another corpus");
        workspace.candidate_order = replay.then(|| theirs.columns.clone());
        let ours = outcome(run(
            &factory,
            provider,
            &mut workspace,
            case["action"].as_str().expect("action"),
            &case["body"].to_string(),
        ));
        tally.cases += 1;
        if let Some((code, _)) = &theirs.error {
            tally.errors += 1;
            if ours.error.as_ref().map(|e| &e.0) != Some(code) {
                tally.failures.push(describe(name, &ours, &theirs));
            }
            continue;
        }
        if ours
            .error
            .as_ref()
            .is_some_and(|e| e.1.contains("not implemented"))
        {
            tally.unsupported += 1;
            continue;
        }
        if ours.error.is_some() || ours.matches.len() != theirs.matches.len() {
            tally.failures.push(describe(name, &ours, &theirs));
            continue;
        }
        let pairs = || ours.matches.iter().zip(&theirs.matches);
        let all_same = pairs().all(|(a, b)| same_match(a, b, true));
        let all_but_counts = pairs().all(|(a, b)| same_match(a, b, false));
        let scores_same = ours
            .matches
            .iter()
            .zip(&theirs.matches)
            .all(|(a, b)| same_score(a.score, b.score));
        if all_same && scores_same {
            tally.exact += 1;
        } else if all_but_counts && scores_same && !replay {
            tally.node_counts += 1;
        } else if !replay
            && ours.matches.len() > 1
            && same_match(&ours.matches[0], &theirs.matches[0], false)
            && same_score(ours.matches[0].score, theirs.matches[0].score)
        {
            tally.alternates_order += 1;
        } else if scores_same {
            tally.ties += 1;
        } else {
            tally.failures.push(describe(name, &ours, &theirs));
        }
    }
    assert!(expected.next().is_none(), "references left over");
    tally
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/oracle")
}

fn andorra_tiles() -> DirectoryGraphTileProvider {
    let tiles =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../valhalla-graphtile/fixtures/andorra-tiles");
    DirectoryGraphTileProvider::new(tiles, NonZeroUsize::new(64).expect("64"))
}

fn report(tally: &Tally) {
    println!(
        "{} cases: {} exact, {} the same but for a node candidate's edge count, {} ties, \
         {} alternates that differ by candidate order, {} errors (as mainline), \
         {} unsupported costings, {} failures",
        tally.cases,
        tally.exact,
        tally.node_counts,
        tally.ties,
        tally.alternates_order,
        tally.errors,
        tally.unsupported,
        tally.failures.len()
    );
    for failure in tally.failures.iter().take(10) {
        println!("{failure}");
    }
}

fn check(tally: &Tally) {
    report(tally);
    assert!(
        tally.failures.is_empty(),
        "{} cases differ",
        tally.failures.len()
    );
    assert!(tally.cases > 0);
}

#[test]
fn oracle_andorra() {
    let dir = fixtures();
    for replay in [false, true] {
        let tally = compare(
            &dir.join("andorra.jsonl"),
            &dir.join("andorra-3.9.1.txt"),
            &andorra_tiles(),
            &dir.join("valhalla.json"),
            replay,
        );
        check(&tally);
        if replay {
            assert_eq!(tally.node_counts + tally.alternates_order, 0);
        }
    }
}

/// The shared trace corpus (or any other):
/// `VALINOR_MEILI_CORPUS` (JSON lines), `VALINOR_MEILI_EXPECTED` (the helper's output,
/// with `columns`), `VALINOR_MEILI_CONFIG` (a `valhalla.json`), and `VALINOR_MEILI_TILES`
/// (a tile directory, the Andorra fixture tiles by default).
#[test]
#[ignore = "needs a corpus and mainline's output for it"]
fn oracle_external() {
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
    let tiles = std::env::var("VALINOR_MEILI_TILES")
        .ok()
        .filter(|d| !d.is_empty())
        .map_or_else(andorra_tiles, |dir| {
            DirectoryGraphTileProvider::new(dir.into(), NonZeroUsize::new(256).expect("256"))
        });
    for replay in [false, true] {
        let tally = compare(
            Path::new(&var("VALINOR_MEILI_CORPUS")),
            Path::new(&var("VALINOR_MEILI_EXPECTED")),
            &tiles,
            Path::new(&var("VALINOR_MEILI_CONFIG")),
            replay,
        );
        check(&tally);
        if replay {
            assert_eq!(tally.node_counts + tally.alternates_order, 0);
        }
    }
}
