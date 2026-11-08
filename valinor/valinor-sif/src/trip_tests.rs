//! Tests against mainline's chosen paths: the costs along each captured trip.
#![expect(
    clippy::float_cmp,
    reason = "recosted paths must match mainline exactly"
)]
//!
//! Each node of a Thor→Odin trip records the elapsed cost and time at the start of its edge
//! and the transition cost onto it, as mainline's path search (and, for bidirectional
//! searches, its recost of the final path) computed them.
//! These tests recost each leg's edges with [`recost_forward`] and compare.
//!
//! Mainline recosts the final paths of bidirectional searches (routes without a time)
//! with `recost_forward`, so those must match exactly.
//! Time-dependent searches are unidirectional and keep the costs they searched with:
//!
//! - Forward searches (`depart_at`, and trivial segments, whose origin and destination
//!   are on one edge) cost the first edge from the location's correlated percent along
//!   (the trip rounds it), add the snap distances to its cost,
//!   and sum each step as `(cost + transition) + edge` rather than `cost + (transition + edge)`,
//!   so their transitions must match exactly and their running sums within 1e-4 (relative;
//!   the largest seen is 1.7e-5).
//! - Reverse searches (`arrive_by`) cost backwards from the arrival time,
//!   and the trip's departure time is rounded to the minute,
//!   so they are held to Thor's path search tolerance (0.1%).

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::GraphTile;
use valhalla_graphtile::tile_provider::{DirectoryGraphTileProvider, GraphTileProvider};
use valhalla_proto::Api;
use valhalla_proto::costing::Type as CostingType;
use valhalla_proto::options::DateTimeType;
use valhalla_proto::prost::Message;

use crate::{AnyCosting, Cost, RecostedEdge, TimeInfo, recost_forward};

/// How a leg's costs were computed in mainline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Search {
    /// Recosted with `recost_forward` (bidirectional searches).
    Recosted,
    /// Forward unidirectional search (time-dependent, or a trivial segment).
    Forward,
    /// Reverse time-dependent search.
    Reverse,
}

impl Search {
    /// The largest relative difference allowed in running sums, and in transitions.
    fn tolerances(self) -> (f64, f64) {
        match self {
            Self::Recosted => (0.0, 0.0),
            Self::Forward => (1e-4, 0.0),
            Self::Reverse => (1e-3, 1e-3),
        }
    }
}

/// Results for one kind of search.
#[derive(Default, Debug)]
struct SearchReport {
    legs: usize,
    exact_legs: usize,
    nodes: usize,
    exact_nodes: usize,
    worst_elapsed: f64,
    worst_transition: f64,
}

/// Results over a set of captured trips.
#[derive(Default, Debug)]
struct TripReport {
    searches: std::collections::BTreeMap<Search, SearchReport>,
    skipped: Vec<String>,
    failures: Vec<String>,
}

fn relative(a: f64, b: f64) -> f64 {
    if a == b {
        0.0
    } else {
        (a - b).abs() / a.abs().max(b.abs()).max(1e-9)
    }
}

/// The trip frames (Thor→Odin `Api` messages) of every case under a capture directory.
fn trip_frames(captures: &Path) -> Vec<(String, PathBuf)> {
    let mut frames = Vec::new();
    let Ok(cases) = std::fs::read_dir(captures) else {
        return frames;
    };
    let mut cases: Vec<_> = cases.filter_map(Result::ok).map(|e| e.path()).collect();
    cases.sort();
    for case in cases {
        // The same trip is captured once per response format; one is enough.
        for format in ["pbf", "json", "osrm"] {
            // Crate fixtures keep one frame per format (see copy-tyr-fixtures.sh).
            let fixture = case.join(format).join("thor-odin.bin");
            if fixture.is_file() {
                let name = case
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                frames.push((name, fixture));
                break;
            }
            let dir = case.join(format).join("thor-odin");
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut found: Vec<_> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(".1.bin"))
                .collect();
            found.sort();
            if let Some(frame) = found.into_iter().next() {
                let name = case
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                frames.push((name, frame));
                break;
            }
        }
    }
    frames
}

#[expect(clippy::too_many_lines, reason = "one pass over the trip structure")]
fn check_trips(captures: &Path, tiles: &Path) -> TripReport {
    let provider = DirectoryGraphTileProvider::new(
        tiles.to_path_buf(),
        NonZeroUsize::new(64).expect("nonzero"),
    );
    let mut report = TripReport::default();
    for (case, frame) in trip_frames(captures) {
        let bytes = std::fs::read(&frame).expect("frame");
        let mut api = Api::decode(bytes.as_slice()).expect("Api");
        // Crate fixtures keep the trip in its own file (see copy-tyr-fixtures.sh).
        if api.trip.is_none()
            && let Some(case_dir) = frame.parent().and_then(Path::parent)
            && let Ok(trip_bytes) = std::fs::read(case_dir.join("trip.bin"))
        {
            api.trip = Api::decode(trip_bytes.as_slice()).expect("trip Api").trip;
        }
        let (Some(options), Some(trip)) = (&api.options, &api.trip) else {
            report.skipped.push(format!("{case}: no trip"));
            continue;
        };
        let costing_type = CostingType::try_from(options.costing_type).unwrap_or(CostingType::None);
        let Some(costing_proto) = options.costings.get(&i32::from(costing_type)) else {
            report.skipped.push(format!("{case}: no costing options"));
            continue;
        };
        let costing = match AnyCosting::new(costing_proto) {
            Ok(c) => c,
            Err(e) => {
                report.skipped.push(format!("{case}: {e}"));
                continue;
            }
        };
        let date_time_type = DateTimeType::try_from(options.date_time_type).ok();
        let invariant = date_time_type == Some(DateTimeType::Invariant);
        let timed = options.has_date_time.is_some()
            || options.locations.iter().any(|l| !l.date_time.is_empty());
        let search = match date_time_type {
            _ if !timed || invariant => Search::Recosted,
            Some(DateTimeType::ArriveBy) => Search::Reverse,
            _ => Search::Forward,
        };

        for (r, route) in trip.routes.iter().enumerate() {
            for (l, leg) in route.legs.iter().enumerate() {
                let edges: Vec<_> = leg.node.iter().filter_map(|n| n.edge.as_ref()).collect();
                if edges.is_empty() {
                    continue;
                }
                let ids: Vec<GraphId> = edges
                    .iter()
                    .map(|e| GraphId::try_from_id(e.id).expect("edge id"))
                    .collect();
                // The time at the leg's origin, in the time zone at the end of its first edge.
                let date_time = leg
                    .location
                    .first()
                    .map_or("", |loc| loc.date_time.as_str());
                let tz = provider
                    .get_handle_for_tile_containing(ids[0])
                    .ok()
                    .and_then(|t| {
                        let end = t.get_directed_edge(ids[0]).ok()?.end_node_id();
                        let node_tile = provider.get_handle_for_tile_containing(end).ok()?;
                        node_tile
                            .get_node(end)
                            .ok()?
                            .time_zone_index()
                            .map(|z| u32::from(z.get()))
                    })
                    .unwrap_or(0);
                // `current` times depend on the capture's clock; the trips here don't use them.
                let time_info = if timed {
                    TimeInfo::make(date_time, tz, Timestamp::now())
                } else {
                    TimeInfo::INVALID
                };
                // Through locations split a leg into segments, which mainline recosts
                // separately and joins: the split edge appears twice in a row.
                let mut segments: Vec<std::ops::Range<usize>> = Vec::new();
                let mut start = 0;
                for i in 1..ids.len() {
                    if ids[i] == ids[i - 1] {
                        segments.push(start..i);
                        start = i;
                    }
                }
                segments.push(start..ids.len());
                // A segment on a single edge is a trivial route, which mainline searches
                // with unidirectional A* (and doesn't recost).
                let search = if search == Search::Recosted && segments.iter().any(|s| s.len() == 1)
                {
                    Search::Forward
                } else {
                    search
                };
                let mut recosted: Vec<RecostedEdge> = Vec::new();
                let leg_name = format!("{case} route {r} leg {l}");
                let mut failed = None;
                for segment in segments {
                    let offset = recosted.last().map_or(Cost::ZERO, |e| e.cost);
                    let seg_time = time_info.forward(offset.secs, tz);
                    let result = recost_forward(
                        &provider,
                        &costing,
                        ids[segment.clone()].iter().copied(),
                        edges[segment.start].source_along_edge,
                        edges[segment.end - 1].target_along_edge,
                        if segment.start == 0 {
                            time_info
                        } else {
                            seg_time
                        },
                        invariant,
                        true,
                        |e| {
                            let mut e = *e;
                            e.cost = offset + e.cost;
                            recosted.push(e);
                        },
                    );
                    if let Err(e) = result {
                        failed = Some(e);
                        break;
                    }
                }
                if let Some(e) = failed {
                    report.failures.push(format!("{leg_name}: {e}"));
                    continue;
                }
                let stats = report.searches.entry(search).or_default();
                stats.legs += 1;
                let (elapsed_tolerance, transition_tolerance) = search.tolerances();
                // Node i records the elapsed cost before edge i and the transition onto it;
                // the last node records the total.
                let mut leg_exact = true;
                for (i, node) in leg.node.iter().enumerate() {
                    let Some(cost) = &node.cost else { continue };
                    let elapsed = cost.elapsed_cost.as_ref();
                    let transition = cost.transition_cost.as_ref();
                    let mine_elapsed = i
                        .checked_sub(1)
                        .and_then(|prev| recosted.get(prev))
                        .map_or(Cost::ZERO, |e| e.cost);
                    let mine_transition = recosted.get(i).map_or(Cost::ZERO, |e| e.transition_cost);
                    let expected = [
                        elapsed.map_or(0.0, |c| c.cost),
                        elapsed.map_or(0.0, |c| c.seconds),
                        transition.map_or(0.0, |c| c.cost),
                        transition.map_or(0.0, |c| c.seconds),
                    ];
                    let actual = [
                        f64::from(mine_elapsed.cost),
                        f64::from(mine_elapsed.secs),
                        f64::from(mine_transition.cost),
                        f64::from(mine_transition.secs),
                    ];
                    stats.nodes += 1;
                    // The trip stores each float as a double, so equal floats compare equal.
                    if expected == actual {
                        stats.exact_nodes += 1;
                        continue;
                    }
                    leg_exact = false;
                    let elapsed_error =
                        relative(expected[0], actual[0]).max(relative(expected[1], actual[1]));
                    let transition_error =
                        relative(expected[2], actual[2]).max(relative(expected[3], actual[3]));
                    stats.worst_elapsed = stats.worst_elapsed.max(elapsed_error);
                    stats.worst_transition = stats.worst_transition.max(transition_error);
                    if (elapsed_error > elapsed_tolerance
                        || transition_error > transition_tolerance)
                        && report.failures.len() < 40
                    {
                        report.failures.push(format!(
                            "{leg_name} ({search:?}) node {i}: mainline {expected:?}, valinor {actual:?}"
                        ));
                    }
                }
                if leg_exact {
                    stats.exact_legs += 1;
                }
            }
        }
    }
    report
}

/// The trips captured for valinor-tyr's tests, over the Andorra fixture tiles
/// (which route the same as the oracle build they were captured on).
#[test]
fn tyr_fixture_trips_recost_like_mainline() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let report = check_trips(
        &root.join("valinor-tyr/fixtures/captures/andorra"),
        &root.join("valhalla-graphtile/fixtures/andorra-tiles"),
    );
    for (search, s) in &report.searches {
        println!(
            "{search:?}: {} legs ({} exact), {} nodes ({} exact)",
            s.legs, s.exact_legs, s.nodes, s.exact_nodes
        );
    }
    assert!(report.failures.is_empty(), "{}", report.failures.join("\n"));
    let recosted = report
        .searches
        .get(&Search::Recosted)
        .expect("recosted legs");
    assert!(recosted.legs >= 30, "only {} recosted legs", recosted.legs);
    assert_eq!(recosted.exact_legs, recosted.legs);
}

/// Recosts the trips in `VALINOR_SIF_CAPTURES` (a directory of capture cases)
/// over the tiles in `VALINOR_SIF_TILES` (the tiles mainline routed on).
#[test]
#[ignore = "needs captures and their tiles (see tools/oracle/README.md)"]
fn captured_trips_recost_like_mainline() {
    let captures =
        PathBuf::from(std::env::var("VALINOR_SIF_CAPTURES").expect("VALINOR_SIF_CAPTURES"));
    let tiles = PathBuf::from(std::env::var("VALINOR_SIF_TILES").expect("VALINOR_SIF_TILES"));
    let report = check_trips(&captures, &tiles);
    for (search, s) in &report.searches {
        println!(
            "{search:?}: {} legs ({} exact), {} nodes ({} exact); worst elapsed {:e}, transition {:e}",
            s.legs, s.exact_legs, s.nodes, s.exact_nodes, s.worst_elapsed, s.worst_transition
        );
    }
    for skipped in &report.skipped {
        println!("skipped {skipped}");
    }
    assert!(report.failures.is_empty(), "{}", report.failures.join("\n"));
}
