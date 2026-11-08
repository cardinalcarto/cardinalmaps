//! Tests against mainline's own costing: dumps from `tools/oracle/cpp/sif.cc`
//! (see `tools/oracle/README.md`).
//!
//! Each dump line holds the inputs of one costing call and mainline's result;
//! the test makes the same call and compares the result exactly
//! (floats bit for bit).
//! The committed dumps sample the Andorra fixture tiles (`fixtures/oracle/andorra-sample.txt`)
//! and check the timed restrictions of a Seattle tile (`fixtures/oracle/seattle-restrictions.txt`);
//! larger dumps (every node, or tiles with predicted speeds) are checked by the ignored test,
//! which reads them from paths in environment variables.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::{DirectoryGraphTileProvider, GraphTileProvider};
use valhalla_proto::Costing as CostingProto;

use crate::costing_options::{parse_costing, parse_costing_name};
use crate::datetime::{ConditionalTime, is_conditional_active_in};
use crate::json::JsonValue;
use crate::{
    AnyCosting, Costing, DisallowMask, EdgeAccess, EdgeLabel, InternalTurn, LabelState, TimeContext,
};

/// The Andorra fixture tiles.
fn fixture_tiles() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("valhalla-graphtile")
        .join("fixtures")
        .join("andorra-tiles")
}

fn parse_id(text: &str) -> GraphId {
    let mut parts = text
        .split('/')
        .map(|p| p.parse::<u64>().expect("graph ID part"));
    let (level, tile, index) = (
        parts.next().expect("level"),
        parts.next().expect("tile"),
        parts.next().expect("index"),
    );
    GraphId::try_from_components(u8::try_from(level).expect("level"), tile, index)
        .expect("graph ID")
}

/// The label state mainline's helper derives from an edge ID (`label_state` in sif.cc).
fn label_state(id: GraphId, costing: &AnyCosting) -> (LabelState, u8) {
    let n = id.feature_index();
    let state = LabelState {
        predecessor: None,
        mode: costing.base().travel_mode(),
        path_distance: u32::try_from((n * 37) % 7919).expect("small"),
        closure_pruning: false,
        has_measured_speed: n.is_multiple_of(3),
        internal_turn: internal_turn(u8::try_from((n / 3) % 3).expect("small")),
        destonly: false,
    };
    (state, if n.is_multiple_of(2) { 0 } else { 0x1e })
}

fn internal_turn(value: u8) -> InternalTurn {
    match value {
        0 => InternalTurn::NoTurn,
        1 => InternalTurn::LeftTurn,
        _ => InternalTurn::RightTurn,
    }
}

/// A tile handle from the fixture provider.
type Handle = <DirectoryGraphTileProvider as GraphTileProvider>::TileHandle;

/// Tiles by base ID, loaded on demand.
struct Tiles {
    provider: DirectoryGraphTileProvider,
    cache: BTreeMap<u64, Handle>,
}

impl Tiles {
    fn new(dir: &Path) -> Self {
        Self {
            provider: DirectoryGraphTileProvider::new(
                dir.to_path_buf(),
                NonZeroUsize::new(64).expect("nonzero"),
            ),
            cache: BTreeMap::new(),
        }
    }

    fn tile(&mut self, id: GraphId) -> Handle {
        let base = id.tile_base_id().value();
        self.cache
            .entry(base)
            .or_insert_with(|| {
                self.provider
                    .get_handle_for_tile_containing(id)
                    .expect("fixture tile")
            })
            .clone()
    }

    fn edge(&mut self, id: GraphId) -> (Handle, DirectedEdge) {
        let tile = self.tile(id);
        let edge = tile.get_directed_edge(id).expect("edge").clone();
        (tile, edge)
    }

    fn node(&mut self, id: GraphId) -> (Handle, NodeInfo) {
        let tile = self.tile(id);
        let node = tile.get_node(id).expect("node").clone();
        (tile, node)
    }
}

/// Builds a costing from its options JSON, as the helper does with mainline's parser.
fn build_costing(costing_name: &str, json: &str) -> AnyCosting {
    let doc_text = format!("{{\"costing_options\":{{\"{costing_name}\":{json}}}}}");
    let doc = JsonValue::parse(doc_text.as_bytes()).expect("options JSON");
    let costing_type = parse_costing_name(costing_name).expect("costing name");
    let mut costing = CostingProto::default();
    let mut warnings = Vec::new();
    parse_costing(
        &doc,
        &format!("/costing_options/{costing_name}"),
        &mut costing,
        &mut warnings,
        Some(costing_type),
    )
    .expect("parse costing");
    AnyCosting::new(&costing).expect("implemented costing")
}

/// Match counts and the first mismatches, by costing and line kind.
#[derive(Default)]
struct Report {
    checked: BTreeMap<(String, String), usize>,
    mismatches: Vec<String>,
    mismatch_count: usize,
}

impl Report {
    fn record(&mut self, costing: &str, kind: &str, line: &str, expected: &str, actual: &str) {
        *self
            .checked
            .entry((costing.to_owned(), kind.to_owned()))
            .or_default() += 1;
        if expected != actual {
            self.mismatch_count += 1;
            if self.mismatches.len() < 40 {
                self.mismatches
                    .push(format!("{costing}: {line}\n    valinor: {actual}"));
            }
        }
    }

    fn summary(&self) -> String {
        let mut by_costing: BTreeMap<&str, usize> = BTreeMap::new();
        for ((costing, _), count) in &self.checked {
            *by_costing.entry(costing.as_str()).or_default() += count;
        }
        let mut out = String::new();
        for (costing, count) in by_costing {
            let _ = writeln!(out, "{costing}: {count}");
        }
        out
    }

    fn assert_clean(&self) {
        assert!(
            self.mismatch_count == 0,
            "{} mismatches with mainline:\n{}",
            self.mismatch_count,
            self.mismatches.join("\n")
        );
    }
}

fn hex(value: f32) -> String {
    format!("{:08x}", value.to_bits())
}

/// Checks every line of a costing dump.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per line kind reads best together"
)]
fn check_costing_dump(dump: &str, tile_dir: &Path) -> Report {
    let mut tiles = Tiles::new(tile_dir);
    let mut report = Report::default();
    let mut costing: Option<(String, AnyCosting)> = None;

    for line in dump.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f[0] == "O" {
            let mut built = build_costing(f[2], f[3]);
            // Variants Thor configures after building a costing (see sif.cc).
            if f[1].contains("+bidir") {
                built.base_mut().set_allow_destination_only(false);
            }
            if f[1].contains("+transit") {
                built.base_mut().set_allow_transit_connections(true);
            }
            costing = Some((f[1].to_owned(), built));
            continue;
        }
        let Some((name, cost)) = costing.as_mut() else {
            panic!("a line before any costing: {line}");
        };
        let name = name.as_str();
        match f[0] {
            "X" => {
                let b = cost.base();
                let mut actual = format!(
                    "{}\t{}\t{}\t{}\t{}",
                    hex(cost.astar_cost_factor()),
                    u8::from(cost.allow_multi_pass()),
                    cost.travel_type(),
                    u8::from(b.default_hierarchy_limits()),
                    u8::from(b.use_hierarchy_limits()),
                );
                for h in b.hierarchy_limits() {
                    let _ = write!(
                        actual,
                        "\t{},{}",
                        h.max_up_transitions,
                        hex(h.expand_within_dist)
                    );
                }
                report.record(name, "X", line, &f[1..].join("\t"), &actual);
            }
            "X1" => {
                cost.base_mut().set_pass(1);
                let actual = hex(cost.astar_cost_factor());
                cost.base_mut().set_pass(0);
                report.record(name, "X", line, f[1], &actual);
            }
            "E" => {
                let id = parse_id(f[1]);
                let (tile, edge) = tiles.edge(id);
                let sow = (f[2] != "-").then(|| f[2].parse::<u32>().expect("second of week"));
                let actual = match cost.edge_cost(&edge, id, &*tile, sow) {
                    Ok(r) => format!(
                        "{}\t{}\t{}",
                        hex(r.cost.cost),
                        hex(r.cost.secs),
                        r.flow_sources.as_repr()
                    ),
                    Err(e) => format!("error: {e}"),
                };
                report.record(name, "E", line, &f[3..].join("\t"), &actual);
            }
            "F" => {
                let id = parse_id(f[1]);
                let (tile, edge) = tiles.edge(id);
                let mask = DisallowMask::from_bits(f[2].parse().expect("mask"));
                let actual = u8::from(cost.edge_filter(&edge, &*tile, mask)).to_string();
                report.record(name, "F", line, f[3], &actual);
            }
            "G" => {
                let id = parse_id(f[1]);
                let (tile, edge) = tiles.edge(id);
                let time = TimeContext::new(f[2].parse().expect("time"), f[3].parse().expect("tz"));
                let (mask, destonly) = cost
                    .base()
                    .exempted_access_restrictions(&edge, &*tile, id, time);
                let actual = format!("{mask}\t{}", u8::from(destonly));
                report.record(name, "G", line, &f[4..].join("\t"), &actual);
            }
            "N" => {
                let (_, node) = tiles.node(parse_id(f[1]));
                let actual = u8::from(cost.node_allowed(&node)).to_string();
                report.record(name, "N", line, f[2], &actual);
            }
            "A" | "AR" => {
                let pred_id = parse_id(f[1]);
                let edge_id = parse_id(f[2]);
                let (_, pred_edge) = tiles.edge(pred_id);
                let (state, mask) = label_state(pred_id, cost);
                let pred = EdgeLabel::new(pred_id, &pred_edge, state);
                let (edge_tile, edge) = tiles.edge(edge_id);
                let reverse = f[0] == "AR";
                let base = if reverse { 4 } else { 3 };
                let time = TimeContext::new(
                    f[base].parse().expect("time"),
                    f[base + 1].parse().expect("time zone"),
                );
                let mut access = EdgeAccess::new(mask);
                let ok = if reverse {
                    let opp_id = parse_id(f[3]);
                    let (opp_tile, opp_edge) = tiles.edge(opp_id);
                    cost.allowed_reverse(
                        &edge,
                        &pred,
                        &opp_edge,
                        opp_id,
                        &*opp_tile,
                        time,
                        &mut access,
                    )
                } else {
                    cost.allowed(&edge, edge_id, &*edge_tile, &pred, time, &mut access)
                };
                let actual = format!(
                    "{}\t{}\t{}\t{}",
                    u8::from(ok),
                    access.restriction_idx,
                    access.destonly_access_restr_mask,
                    u8::from(access.edge_destonly)
                );
                report.record(name, f[0], line, &f[base + 2..].join("\t"), &actual);
            }
            "T" => {
                let pred_id = parse_id(f[1]);
                let edge_id = parse_id(f[2]);
                let (_, pred_edge) = tiles.edge(pred_id);
                let (state, _) = label_state(pred_id, cost);
                let pred = EdgeLabel::new(pred_id, &pred_edge, state);
                let (edge_tile, edge) = tiles.edge(edge_id);
                let (_, node) = tiles.node(parse_id(f[3]));
                let actual = match cost.transition_cost(
                    &edge,
                    &*edge_tile,
                    &node,
                    &pred,
                    &tiles.provider,
                    f[4] == "1",
                ) {
                    Ok(c) => format!("{}\t{}", hex(c.cost), hex(c.secs)),
                    Err(e) => format!("error: {e}"),
                };
                report.record(name, "T", line, &f[5..].join("\t"), &actual);
            }
            "TR" => {
                let pred_id = parse_id(f[1]);
                let (_, pred_edge) = tiles.edge(pred_id);
                let (state, _) = label_state(pred_id, cost);
                let pred = EdgeLabel::new(pred_id, &pred_edge, state);
                let (_, edge) = tiles.edge(parse_id(f[2]));
                let (_, opp_edge) = tiles.edge(parse_id(f[3]));
                let (_, opp_pred_edge) = tiles.edge(parse_id(f[4]));
                let (node_tile, node) = tiles.node(parse_id(f[5]));
                let actual = match cost.transition_cost_reverse(
                    edge.local_edge_index(),
                    &node,
                    &opp_edge,
                    &opp_pred_edge,
                    &pred,
                    &*node_tile,
                    &tiles.provider,
                    f[6] == "1",
                    internal_turn(f[7].parse().expect("internal turn")),
                    f[8] == "1",
                ) {
                    Ok(c) => format!("{}\t{}", hex(c.cost), hex(c.secs)),
                    Err(e) => format!("error: {e}"),
                };
                report.record(name, "TR", line, &f[9..].join("\t"), &actual);
            }
            "U" => {
                let pred_id = parse_id(f[1]);
                let (_, pred_edge) = tiles.edge(pred_id);
                let (_, edge) = tiles.edge(parse_id(f[2]));
                let (_, node) = tiles.node(parse_id(f[3]));
                // The opposing edges, as the helper picked them.
                let opp_pred_idx = pred_edge.opposing_local_index();
                let forward = cost.base().turn_type(opp_pred_idx, &node, &edge, None);
                // Reverse: the opposing edge of `edge`, with the opposing predecessor.
                let opp_edge_id = opposing_edge_id(&mut tiles, parse_id(f[2]));
                let (_, opp_edge) = tiles.edge(opp_edge_id);
                let opp_pred_id = opposing_edge_id(&mut tiles, pred_id);
                let (_, opp_pred) = tiles.edge(opp_pred_id);
                let reverse = cost.base().turn_type(
                    edge.local_edge_index(),
                    &node,
                    &opp_edge,
                    Some(&opp_pred),
                );
                let actual = format!("{}\t{}", forward as u8, reverse as u8);
                report.record(name, "U", line, &f[4..].join("\t"), &actual);
            }
            "R" => {
                let edge_id = parse_id(f[1]);
                let (edge_tile, edge) = tiles.edge(edge_id);
                let forward = f[2] == "1";
                let time = TimeContext::new(f[3].parse().expect("time"), f[4].parse().expect("tz"));
                let labels: Vec<EdgeLabel> = f[5]
                    .split(',')
                    .enumerate()
                    .map(|(k, id)| {
                        let id = parse_id(id);
                        let (_, chain_edge) = tiles.edge(id);
                        let (mut state, _) = label_state(id, cost);
                        state.predecessor =
                            k.checked_sub(1).map(|p| u32::try_from(p).expect("small"));
                        EdgeLabel::new(id, &chain_edge, state)
                    })
                    .collect();
                let pred = labels.last().expect("a chain");
                let actual = match cost.base().restricted(
                    &edge,
                    pred,
                    &labels,
                    &*edge_tile,
                    edge_id,
                    forward,
                    time,
                    |_| {},
                ) {
                    Ok(r) => u8::from(r).to_string(),
                    Err(e) => format!("error: {e}"),
                };
                report.record(name, "R", line, f[6], &actual);
            }
            other => panic!("unknown line kind {other}"),
        }
    }
    report
}

/// The opposing edge of an edge.
fn opposing_edge_id(tiles: &mut Tiles, id: GraphId) -> GraphId {
    let tile = tiles.tile(id);
    tiles
        .provider
        .get_opposing_edge_id(id, &*tile)
        .expect("opposing edge")
}

#[test]
fn matches_mainline_on_the_andorra_sample() {
    let dump = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/oracle/andorra-sample.txt"),
    )
    .expect("sample dump");
    let report = check_costing_dump(&dump, &fixture_tiles());
    println!("{}", report.summary());
    report.assert_clean();
    // Every costing in the options list was checked.
    let costings: std::collections::BTreeSet<_> =
        report.checked.keys().map(|(c, _)| c.clone()).collect();
    assert_eq!(costings.len(), 31);
}

/// Time-dependent edge costs, on two Andorra tiles with generated predicted speeds
/// (see `tools/oracle/predicted-profiles.py`).
#[test]
fn matches_mainline_with_predicted_speeds() {
    let dump = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/oracle/andorra-predicted-sample.txt"),
    )
    .expect("predicted dump");
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/andorra-predicted-tiles");
    let report = check_costing_dump(&dump, &tiles);
    println!("{}", report.summary());
    report.assert_clean();
    // Flow source 4 is the predicted speed.
    let predicted = dump
        .lines()
        .filter(|l| l.starts_with("E\t") && l.ends_with("\t4"))
        .count();
    assert!(predicted > 5000, "only {predicted} predicted edge costs");
}

/// Timed and destination restrictions, which the Andorra tiles lack for these costings,
/// on a level 0 tile from the Seattle oracle build (`America/Los_Angeles`).
#[test]
fn matches_mainline_on_seattle_restrictions() {
    let dump = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/oracle/seattle-restrictions.txt"),
    )
    .expect("restrictions dump");
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/seattle-tiles");
    let report = check_costing_dump(&dump, &tiles);
    println!("{}", report.summary());
    report.assert_clean();
    let timed = dump
        .lines()
        .filter(|l| {
            l.starts_with("A\t") && !l.ends_with("\t255\t0\t0") && !l.contains("\t255\t30\t")
        })
        .count();
    assert!(timed > 100, "only {timed} timed access checks");
}

/// Checks a full dump, from `VALINOR_SIF_DUMP` over the tiles in `VALINOR_SIF_TILES`
/// (the Andorra fixture tiles by default).
/// Run it in release mode; full dumps have tens of millions of lines.
#[test]
#[ignore = "needs a dump from tools/oracle/sif-oracle.sh"]
fn matches_mainline_on_a_full_dump() {
    let dump_path = std::env::var("VALINOR_SIF_DUMP").expect("VALINOR_SIF_DUMP");
    let tiles = std::env::var("VALINOR_SIF_TILES").map_or_else(|_| fixture_tiles(), PathBuf::from);
    let dump = std::fs::read_to_string(dump_path).expect("dump");
    let report = check_costing_dump(&dump, &tiles);
    println!("{}", report.summary());
    report.assert_clean();
}

/// Checks mainline's `is_conditional_active` results (`sif conditional`).
#[test]
fn conditional_time_domains_match_mainline() {
    let dump = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/oracle/conditional.txt"),
    )
    .expect("conditional dump");
    let mut checked = 0;
    let mut mismatches = Vec::new();
    for line in dump.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        assert_eq!(f[0], "C");
        let td = valhalla_graphtile::graph_tile::TimeDomain::from_raw(f[1].parse().expect("td"));
        let time: u64 = f[2].parse().expect("time");
        let tz: u32 = f[3].parse().expect("tz");
        let actual = is_conditional_active_in(ConditionalTime::from(td), time, tz);
        checked += 1;
        if actual != (f[4] == "1") && mismatches.len() < 30 {
            mismatches.push(format!("{line} (valinor: {actual}, {td})"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    assert!(checked > 10_000);
}
