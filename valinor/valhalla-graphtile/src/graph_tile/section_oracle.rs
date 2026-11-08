//! Tests the variable-length section decoders against mainline Valhalla 3.9.1.
//!
//! `tools/oracle/cpp/sections.cc` links mainline's `libvalhalla`
//! and prints a text dump of every edge info, sign, turn lane, access restriction,
//! complex restriction, lane connectivity record, and admin in the fixture tiles,
//! decoded by mainline's own readers (`EdgeInfo`, `GraphTile::GetSigns`, and so on).
//! These tests format the same dump with Valinor's decoders and compare.
//!
//! The full dump is several megabytes,
//! so `fixtures/oracle/sections-3.9.1.txt` keeps a line count and digest per tile and kind of line,
//! plus a sample of the lines (see `tools/oracle/sections-fixture.py`).
//! When a digest differs, the test writes Valinor's dump to a temporary file;
//! regenerate mainline's dump as described in `tools/oracle/README.md` and diff the two.
//!
//! `fixtures/oracle/sections-synthetic-3.9.1.txt` covers what the fixture tiles lack
//! (landmarks, conditional speed limits, OSM node IDs, fractional levels,
//! timed complex restrictions, lane connectivity, and more):
//! records built with mainline's encoders, followed by mainline's decoding of them.

use super::complex_restriction::ComplexRestriction;
use super::edge_info::EdgeInfo;
use super::handles::OwnedGraphTileHandle;
use super::lane_connectivity::LaneConnectivity;
use super::tagged_value::{TaggedValue, TaggedValueKind};
use super::{AccessRestriction, GraphId, GraphTile, TileSections, TimeDomain};
use core::fmt::Write;
use enumset::EnumSet;
use md5::{Digest, Md5};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use zerocopy::FromBytes;

/// The level probes for `includes_level` (the same as in `sections.cc`).
const LEVEL_PROBES: [f32; 11] = [-2.0, -1.0, 0.0, 0.5, 1.0, 2.5, 3.0, 13.0, 14.0, 17.5, 25.0];

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// Escapes bytes the way `sections.cc` does:
/// printable ASCII (except the backslash) as is, and everything else as `\xHH`.
fn esc(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if (0x20..0x7f).contains(&b) && b != b'\\' {
            out.push(char::from(b));
        } else {
            write!(out, "\\x{b:02x}").unwrap();
        }
    }
    out
}

/// Formats a number like C's `%.<precision>g`.
fn fmt_g(value: f64, precision: usize) -> String {
    fn strip(s: &str) -> &str {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.')
        } else {
            s
        }
    }
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    let scientific = format!("{:.*e}", precision - 1, value);
    let (mantissa, exponent) = scientific.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    let precision = i32::try_from(precision).unwrap();
    if exponent < -4 || exponent >= precision {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa), exponent.abs())
    } else {
        let decimals = usize::try_from(precision - 1 - exponent).unwrap();
        strip(&format!("{value:.decimals$}")).to_string()
    }
}

/// Formats a raw graph ID the way `sections.cc` does.
fn gid(raw: u64) -> String {
    if raw == (1 << 46) - 1 {
        return "invalid".to_string();
    }
    format!(
        "{}/{}/{}",
        raw & 0x7,
        (raw >> 3) & 0x3f_ffff,
        (raw >> 25) & 0x1f_ffff
    )
}

fn dump_elevation(out: &mut String, ei: &EdgeInfo<'_>, edge: usize, length: u32) {
    let elevation = ei.encoded_elevation(length).unwrap();
    write!(
        out,
        " elevation edge={edge} length={length} interval={}:",
        fmt_g(elevation.interval(), 17)
    )
    .unwrap();
    for delta in elevation.deltas() {
        write!(out, " {delta}").unwrap();
    }
    out.push('\n');
}

fn dump_edge_info(out: &mut String, ei: &EdgeInfo<'_>, edge: usize, offset: u32) {
    writeln!(
        out,
        " edgeinfo edge={edge} offset={offset} way={} mean_elevation={} speed_limit={} bike={} \
         has_elevation={} names={} types={}",
        ei.way_id(),
        // Mainline reports the "no data" elevation as -500.
        fmt_g(f64::from(ei.mean_elevation().unwrap_or(-500.0)), 9),
        ei.speed_limit(),
        ei.bicycle_network().as_repr(),
        u8::from(ei.has_elevation()),
        ei.name_infos().len(),
        ei.types(),
    )
    .unwrap();
    for name in ei.names() {
        let name = name.unwrap();
        writeln!(
            out,
            "  name rn={} {}",
            u8::from(name.is_route_number()),
            esc(name.text())
        )
        .unwrap();
    }

    // Mainline's GetTags: a multimap sorted by tag, without the linguistic values.
    let mut tags = ei
        .tagged_values()
        .map(Result::unwrap)
        .filter(|v| v.kind().is_some_and(|k| k != TaggedValueKind::Linguistic))
        .collect::<Vec<_>>();
    tags.sort_by_key(TaggedValue::tag);
    for value in &tags {
        writeln!(out, "  tag {} {}", value.tag(), esc(value.value())).unwrap();
    }

    for record in ei.linguistic_records() {
        let record = record.unwrap();
        let mut bytes = record.raw_header().to_vec();
        bytes.extend(record.pronunciation());
        writeln!(out, "  linguistic {}", esc(&bytes)).unwrap();
    }
    let map = ei.linguistic_map().unwrap();
    for record in map.iter() {
        writeln!(
            out,
            "  linguistic_map {} lang={} alphabet={} {}",
            record.name_index(),
            record.raw_language(),
            record.raw_phonetic_alphabet(),
            esc(record.pronunciation())
        )
        .unwrap();
    }

    let layer = ei.layer().unwrap();
    if layer != 0 {
        writeln!(out, "  layer {layer}").unwrap();
    }
    dump_levels(out, ei);
    dump_other_tagged_values(out, ei);
}

fn dump_levels(out: &mut String, ei: &EdgeInfo<'_>) {
    if let Some(levels) = ei.levels().unwrap() {
        let ranges = levels.ranges().collect::<Result<Vec<_>, _>>().unwrap();
        if !ranges.is_empty() {
            write!(out, "  levels precision={}:", levels.precision()).unwrap();
            for (lo, hi) in ranges {
                write!(
                    out,
                    " {}-{}",
                    fmt_g(f64::from(lo), 9),
                    fmt_g(f64::from(hi), 9)
                )
                .unwrap();
            }
            out.push('\n');
            out.push_str("  includes_level:");
            for probe in LEVEL_PROBES {
                write!(
                    out,
                    " {}={}",
                    fmt_g(f64::from(probe), 9),
                    u8::from(ei.includes_level(probe).unwrap())
                )
                .unwrap();
            }
            out.push('\n');
        }
    }
}

fn dump_other_tagged_values(out: &mut String, ei: &EdgeInfo<'_>) {
    for level_ref in ei.level_refs() {
        writeln!(out, "  level_ref {}", esc(level_ref.unwrap())).unwrap();
    }
    if let Some(ids) = ei.osm_node_ids().unwrap() {
        let ids = ids.collect::<Result<Vec<_>, _>>().unwrap();
        if !ids.is_empty() {
            out.push_str("  osm_node_ids:");
            for id in ids {
                write!(out, " {id}").unwrap();
            }
            out.push('\n');
        }
    }
    for limit in ei.conditional_speed_limits() {
        let limit = limit.unwrap();
        writeln!(
            out,
            "  conditional_speed td={} speed={} \"{}\"",
            limit.time_domain().raw(),
            limit.speed(),
            limit.time_domain()
        )
        .unwrap();
    }
    for landmark in ei.landmarks() {
        let landmark = landmark.unwrap();
        writeln!(
            out,
            "  landmark type={} lng={:.7} lat={:.7} {}",
            landmark.landmark_type() as u8,
            landmark.lng(),
            landmark.lat(),
            esc(landmark.name())
        )
        .unwrap();
    }
}

fn dump_restriction(out: &mut String, label: &str, r: &ComplexRestriction<'_>) {
    write!(
        out,
        " {label}_restriction from={} to={} type={} modes={} vias={}",
        gid(r.raw_from_graph_id()),
        gid(r.raw_to_graph_id()),
        r.raw_restriction_type(),
        r.modes().as_repr(),
        r.vias().len()
    )
    .unwrap();
    for via in r.vias() {
        write!(out, " {}", gid(via.value())).unwrap();
    }
    write!(
        out,
        " probability={} has_dt={}",
        r.probability(),
        u8::from(r.has_date_time())
    )
    .unwrap();
    if let Some(dt) = r.date_time() {
        write!(
            out,
            " dt_type={} dow={} begin={}/{}/{} {}:{} end={}/{}/{} {}:{}",
            u8::from(dt.nth_dow),
            dt.dow,
            dt.begin_month,
            dt.begin_week,
            dt.begin_day_dow,
            dt.begin_hrs,
            dt.begin_mins,
            dt.end_month,
            dt.end_week,
            dt.end_day_dow,
            dt.end_hrs,
            dt.end_mins
        )
        .unwrap();
    }
    out.push('\n');
}

fn dump_signs(out: &mut String, sections: &TileSections<'_>) {
    for (i, sign) in sections.signs().iter().enumerate() {
        writeln!(
            out,
            " sign {i} index={} type={} rn={} tagged={} offset={}",
            sign.edge_or_node_index(),
            sign.raw_sign_type(),
            u8::from(sign.is_route_num_type()),
            u8::from(sign.is_text_tagged()),
            sign.text_offset.get()
        )
        .unwrap();
    }
    let indexes = sections
        .signs()
        .iter()
        .map(super::Sign::edge_or_node_index)
        .collect::<BTreeSet<_>>();
    for index in indexes {
        for on_node in [false, true] {
            let place = if on_node { "node" } else { "edge" };
            let signs = sections.get_signs(index, on_node).unwrap();
            for s in &signs {
                writeln!(
                    out,
                    " get_signs {place}={index} type={} rn={} tagged={} {}",
                    s.sign().raw_sign_type(),
                    u8::from(s.is_route_number()),
                    u8::from(s.is_tagged()),
                    esc(s.text())
                )
                .unwrap();
            }
            let (signs, map) = sections.get_signs_with_linguistics(index, on_node).unwrap();
            for s in &signs {
                writeln!(
                    out,
                    " get_signs_linguistic {place}={index} type={} rn={} tagged={} {}",
                    s.sign().raw_sign_type(),
                    u8::from(s.is_route_number()),
                    u8::from(s.is_tagged()),
                    esc(s.text())
                )
                .unwrap();
            }
            for record in map.iter() {
                writeln!(
                    out,
                    " sign_linguistic_map {place}={index} {} lang={} alphabet={} {}",
                    record.name_index(),
                    record.raw_language(),
                    record.raw_phonetic_alphabet(),
                    esc(record.pronunciation())
                )
                .unwrap();
            }
        }
    }
}

/// Formats a tile the way `sections.cc` does.
fn dump_tile(out: &mut String, tile: &OwnedGraphTileHandle) {
    let id = tile.graph_id();
    writeln!(out, "tile {}/{}/0", id.level(), id.tile_id()).unwrap();

    // Edge info, once per distinct offset, in edge order, with the elevation per directed edge.
    let mut seen = BTreeSet::new();
    for (i, edge) in tile.directed_edges().iter().enumerate() {
        let ei = tile.get_edge_info(edge).unwrap();
        if ei.has_elevation() {
            dump_elevation(out, &ei, i, edge.length());
        }
        if seen.insert(edge.edge_info_offset()) {
            dump_edge_info(out, &ei, i, edge.edge_info_offset());
        }
    }

    let sections = tile.sections();
    dump_signs(out, &sections);

    for tl in sections.turn_lanes() {
        let edge = tl.directed_edge_index();
        let masks = tile
            .get_turn_lanes(edge)
            .unwrap()
            .map(|lanes| lanes.to_mainline_string().unwrap())
            .unwrap_or_default();
        writeln!(
            out,
            " turn_lanes edge={edge} offset={} text={} masks={}",
            tl.text_offset.get(),
            esc(sections.text(tl.text_offset.get()).unwrap()),
            esc(masks.as_bytes())
        )
        .unwrap();
    }

    // Through the lookup, which (like the raw list mainline prints) is in edge order.
    for i in 0..tile.directed_edges().len() {
        for r in tile.get_access_restrictions(u32::try_from(i).unwrap(), EnumSet::all()) {
            dump_access_restriction(out, r);
        }
    }

    for (label, restrictions) in [
        ("forward", sections.forward_complex_restrictions()),
        ("reverse", sections.reverse_complex_restrictions()),
    ] {
        let mut count = 0;
        for r in restrictions {
            dump_restriction(out, label, &r.unwrap());
            count += 1;
        }
        writeln!(out, " {label}_restrictions count={count}").unwrap();
    }
    for i in 0..tile.directed_edges().len() {
        let edge_id = GraphId::try_from_components(id.level(), id.tile_id(), i as u64).unwrap();
        for forward in [true, false] {
            for r in tile.get_complex_restrictions(forward, edge_id, EnumSet::all()) {
                let r = r.unwrap();
                writeln!(
                    out,
                    " get_restrictions edge={i} {} from={} to={}",
                    if forward { "forward" } else { "reverse" },
                    gid(r.raw_from_graph_id()),
                    gid(r.raw_to_graph_id())
                )
                .unwrap();
            }
        }
    }

    for lc in sections.lane_connectivity() {
        dump_lane_connectivity(out, " lane_connectivity ", lc);
    }
    for i in 0..tile.directed_edges().len() {
        let found = tile.get_lane_connectivity(u32::try_from(i).unwrap());
        if !found.is_empty() {
            writeln!(out, " get_lane_connectivity edge={i} count={}", found.len()).unwrap();
        }
    }

    for (i, admin) in tile.admins().iter().enumerate() {
        // Resolve through the public lookup too, so that it is covered.
        let info = tile.get_admin_info(i).unwrap();
        writeln!(
            out,
            " admin {i} country={} state={} country_iso={} state_iso={}",
            esc(sections.text(admin.country_name_offset.get()).unwrap()),
            esc(sections
                .text(admin.principal_subdivision_offset.get())
                .unwrap()),
            info.country_iso,
            info.state_iso
        )
        .unwrap();
    }
}

fn dump_access_restriction(out: &mut String, r: &AccessRestriction) {
    writeln!(
        out,
        " access_restriction edge={} type={} modes={} except_destination={} value={}",
        r.edge_index(),
        r.raw_restriction_type(),
        r.affected_access_modes().as_repr(),
        u8::from(r.except_destination()),
        r.value()
    )
    .unwrap();
}

fn dump_lane_connectivity(out: &mut String, prefix: &str, lc: &LaneConnectivity) {
    writeln!(
        out,
        "{prefix}to={} from={} to_lanes={} from_lanes={}",
        lc.to(),
        lc.from(),
        lc.to_lanes(),
        lc.from_lanes()
    )
    .unwrap();
}

/// Groups dump lines as `sections-fixture.py` does: by tile and kind,
/// with two-space indented lines belonging to the group of the line before them.
fn group_lines(dump: &str) -> BTreeMap<(String, String), Vec<&str>> {
    let mut groups: BTreeMap<(String, String), Vec<&str>> = BTreeMap::new();
    let mut tile = String::new();
    let mut kind = String::new();
    for line in dump.lines() {
        if let Some(t) = line.strip_prefix("tile ") {
            tile = t.to_string();
            continue;
        }
        if !line.starts_with("  ") {
            kind = line.split(' ').nth(1).unwrap().to_string();
        }
        groups
            .entry((tile.clone(), kind.clone()))
            .or_default()
            .push(line);
    }
    groups
}

/// Loads every fixture tile, sorted by level and tile ID (as `sections.cc` visits them).
pub(super) fn load_fixture_tiles() -> Vec<OwnedGraphTileHandle> {
    let mut tiles = walkdir::WalkDir::new(fixture_path("andorra-tiles"))
        .into_iter()
        .map(Result::unwrap)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "gph"))
        .map(|e| OwnedGraphTileHandle::try_from(std::fs::read(e.path()).unwrap()).unwrap())
        .collect::<Vec<_>>();
    tiles.sort_by_key(|t| (t.graph_id().level(), t.graph_id().tile_id()));
    tiles
}

#[test]
fn test_fixture_tiles_match_mainline() {
    let mut dump = String::new();
    for tile in load_fixture_tiles() {
        dump_tile(&mut dump, &tile);
    }
    let ours = group_lines(&dump);

    let fixture = std::fs::read_to_string(fixture_path("oracle/sections-3.9.1.txt")).unwrap();
    let mut expected_groups = BTreeSet::new();
    let mut mismatches = Vec::new();
    for line in fixture.lines().filter(|l| !l.starts_with('#')) {
        let fields = line.splitn(5, '\t').collect::<Vec<_>>();
        let key = (fields[1].to_string(), fields[2].to_string());
        let group = ours.get(&key).map(Vec::as_slice).unwrap_or_default();
        match fields[0] {
            "group" => {
                expected_groups.insert(key.clone());
                let count: usize = fields[3].parse().unwrap();
                let mut hasher = Md5::new();
                for l in group {
                    hasher.update(l.as_bytes());
                    hasher.update(b"\n");
                }
                let digest: [u8; 16] = hasher.finalize().into();
                let digest = digest.iter().fold(String::new(), |mut s, b| {
                    write!(s, "{b:02x}").unwrap();
                    s
                });
                if group.len() != count || digest != fields[4] {
                    mismatches.push(format!(
                        "{key:?}: {} lines with digest {digest}, but mainline has {count} with {}",
                        group.len(),
                        fields[4]
                    ));
                }
            }
            "sample" => {
                let index: usize = fields[3].parse().unwrap();
                let actual = group.get(index).copied().unwrap_or("<missing>");
                if actual != fields[4] {
                    mismatches.push(format!(
                        "{key:?} line {index}:\n  mainline: {}\n  valinor:  {actual}",
                        fields[4]
                    ));
                }
            }
            other => panic!("unexpected fixture line kind {other}"),
        }
    }
    for key in ours.keys().filter(|k| !expected_groups.contains(*k)) {
        mismatches.push(format!("{key:?}: not in mainline's dump"));
    }

    if !mismatches.is_empty() {
        let path = std::env::temp_dir().join("valinor-sections-dump.txt");
        std::fs::write(&path, &dump).unwrap();
        panic!(
            "the section dump differs from mainline's ({} problems; Valinor's dump is in {}):\n{}",
            mismatches.len(),
            path.display(),
            mismatches
                .iter()
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn test_synthetic_records_match_mainline() {
    let fixture =
        std::fs::read_to_string(fixture_path("oracle/sections-synthetic-3.9.1.txt")).unwrap();
    let mut lines = fixture.lines().peekable();
    let mut text = Vec::new();
    let mut edge_infos = 0;
    let mut time_domains = 0;
    let mut checked = 0;

    // Collects the expected (indented) lines that follow a record.
    let expected_block = |lines: &mut core::iter::Peekable<core::str::Lines<'_>>| {
        let mut block = String::new();
        while let Some(line) = lines.next_if(|l| l.starts_with(' ')) {
            block.push_str(line);
            block.push('\n');
        }
        block
    };

    while let Some(line) = lines.next() {
        let (kind, rest) = line.split_once(' ').unwrap();
        match kind {
            "td" => {
                let (raw, expected) = rest.split_once(' ').unwrap();
                let td = TimeDomain::from_raw(raw.parse().unwrap());
                assert_eq!(td.to_string(), expected, "time domain {raw}");
                time_domains += 1;
            }
            "text" => text = unhex(rest),
            "edgeinfo_bytes" => {
                let (length, bytes) = rest.split_once(' ').unwrap();
                let bytes = unhex(bytes);
                let ei = EdgeInfo::try_from((bytes.as_slice(), text.as_slice())).unwrap();
                let mut ours = String::new();
                dump_elevation(&mut ours, &ei, edge_infos, length.parse().unwrap());
                dump_edge_info(&mut ours, &ei, edge_infos, 0);
                assert_eq!(ours, expected_block(&mut lines), "edge info {edge_infos}");
                edge_infos += 1;
            }
            "restriction_bytes" => {
                let bytes = unhex(rest);
                let mut restrictions =
                    super::complex_restriction::ComplexRestrictions::new(&bytes, "test");
                let mut ours = String::new();
                dump_restriction(
                    &mut ours,
                    "synthetic",
                    &restrictions.next().unwrap().unwrap(),
                );
                assert!(restrictions.next().is_none());
                assert_eq!(ours, expected_block(&mut lines));
                checked += 1;
            }
            "lane_connectivity_bytes" => {
                let (bytes, expected) = rest.split_once(' ').unwrap();
                let bytes = unhex(bytes);
                let lc = LaneConnectivity::ref_from_bytes(&bytes).unwrap();
                let mut ours = String::new();
                dump_lane_connectivity(&mut ours, "", lc);
                assert_eq!(ours.trim_end(), expected);
                checked += 1;
            }
            "access_restriction_bytes" => {
                let (bytes, expected) = rest.split_once(' ').unwrap();
                let bytes = unhex(bytes);
                let r = AccessRestriction::read_from_bytes(&bytes).unwrap();
                let mut ours = String::new();
                dump_access_restriction(&mut ours, &r);
                assert_eq!(
                    ours.trim_end()
                        .strip_prefix(" access_restriction ")
                        .unwrap(),
                    expected
                );
                checked += 1;
            }
            other => panic!("unexpected synthetic line kind {other}"),
        }
    }
    assert_eq!(time_domains, 300);
    assert_eq!(edge_infos, 4);
    assert_eq!(checked, 6);
}

#[test]
fn test_fmt_g() {
    assert_eq!(fmt_g(25.0, 17), "25");
    assert_eq!(fmt_g(28.571_428_571_428_573, 17), "28.571428571428573");
    assert_eq!(fmt_g(-1.0, 9), "-1");
    assert_eq!(fmt_g(0.5, 9), "0.5");
    assert_eq!(fmt_g(0.0, 9), "0");
    assert_eq!(fmt_g(1e20, 9), "1e+20");
    assert_eq!(fmt_g(1.5e-5, 9), "1.5e-05");
    assert_eq!(fmt_g(1130.0, 9), "1130");
}
