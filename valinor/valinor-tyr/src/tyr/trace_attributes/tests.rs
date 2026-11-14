use super::*;
use crate::test_support::json_differences;
use crate::tyr::serialize_directions;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use valhalla_graphtile::GraphId;
use valhalla_proto::prost::Message;
use valhalla_proto::trip_leg::{Admin, Cost, Node, PathCost};
use valhalla_proto::{FilterAction, Info, StreetName, Trip, TripRoute, options};
use valinor_meili::StateId;

/// The `trace_attributes` oracle fixtures (see `tools/oracle/README.md`).
const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/trace-attributes/andorra"
);

/// A case of the oracle helper's output: what mainline's serializer got, and its response.
struct Case {
    name: String,
    api: Api,
    /// `None` for `trace_route` cases, whose `api` is Odin's output.
    matches: Option<Vec<MapMatchResult>>,
    response: Vec<u8>,
}

/// The helper writes numbers as strings of their round-tripping digits,
/// since `serde_json`'s default parsing can be a unit in the last place off.
fn number(value: &Value) -> f64 {
    value
        .as_str()
        .and_then(|digits| digits.parse().ok())
        .expect("a number as a string")
}

/// The helper's JSON dump of the match results.
fn parse_matches(json: &[u8]) -> Vec<MapMatchResult> {
    let value: Value = serde_json::from_slice(json).expect("valid match results");
    value
        .as_array()
        .expect("a list of matches")
        .iter()
        .map(|entry| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the scores are floats printed with 9 digits, which round-trip"
            )]
            let score = |key: &str| number(&entry[key]) as f32;
            let results = entry["results"]
                .as_array()
                .expect("results")
                .iter()
                .map(|result| MatchResult {
                    lnglat: (number(&result["lnglat"][0]), number(&result["lnglat"][1])),
                    distance_from: number(&result["distance_from"]),
                    edge_id: result["edge_id"]
                        .as_u64()
                        .map(|id| GraphId::try_from_id(id).expect("a valid edge ID")),
                    distance_along: number(&result["distance_along"]),
                    epoch_time: number(&result["epoch_time"]),
                    state_id: result["state_id"].as_array().map(|state| StateId {
                        time: u32::try_from(state[0].as_u64().expect("time")).expect("u32"),
                        id: u32::try_from(state[1].as_u64().expect("id")).expect("u32"),
                    }),
                    is_break_point: result["is_break_point"].as_bool().expect("bool"),
                    begins_discontinuity: result["begins_discontinuity"].as_bool().expect("bool"),
                    ends_discontinuity: result["ends_discontinuity"].as_bool().expect("bool"),
                    edge_index: result["edge_index"]
                        .as_u64()
                        .map(|index| usize::try_from(index).expect("usize")),
                })
                .collect();
            (score("confidence_score"), score("raw_score"), results)
        })
        .collect()
}

fn read_case(dir: &Path) -> Case {
    let read = |name: &str| std::fs::read(dir.join(name)).ok();
    let api = Api::decode(read("api.bin").expect("api.bin").as_slice()).expect("a valid Api");
    let response = read("response.json")
        .or_else(|| read("response.bin"))
        .expect("a response");
    Case {
        name: dir
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        api,
        matches: read("matches.json").map(|json| parse_matches(&json)),
        response,
    }
}

fn cases(dir: &str) -> Vec<Case> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("the fixture directory")
        .map(|entry| entry.expect("a directory entry").path())
        .collect();
    dirs.sort();
    dirs.iter().map(|dir| read_case(dir)).collect()
}

/// Serializes a case as mainline did, and returns the differences from mainline's response.
fn check(case: &mut Case) -> Vec<String> {
    let format = case.api.options.as_ref().map_or(0, |o| o.format);
    // The documented divergence for OSRM approach alerts in miles (see `docs/routing.md`):
    // mainline announces a kilometer figure as miles.
    let osrm_miles = case
        .api
        .options
        .as_ref()
        .is_some_and(|o| o.format == Format::Osrm as i32 && o.units == Units::Miles as i32);
    let response = match &case.matches {
        Some(matches) => {
            let options = case.api.options.clone().unwrap_or_default();
            let controller = AttributesController::new(&options, true);
            serialize_trace_attributes(&mut case.api, &controller, matches)
        }
        None => serialize_directions(&mut case.api),
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => return vec![format!("failed to serialize: {error}")],
    };
    if format == Format::Pbf as i32 {
        let actual = Api::decode(response.body.as_slice()).expect("a valid Api");
        let expected = Api::decode(case.response.as_slice()).expect("a valid Api");
        if actual == expected {
            Vec::new()
        } else {
            vec!["the PBF responses differ".to_owned()]
        }
    } else {
        let expected: Value = serde_json::from_slice(&case.response).expect("valid JSON");
        let actual: Value = serde_json::from_slice(&response.body).expect("valid JSON");
        let mut differences = json_differences(&expected, &actual, usize::MAX);
        if osrm_miles {
            differences.retain(|difference| !difference.contains("/voiceInstructions/"));
        }
        differences.truncate(20);
        differences
    }
}

fn check_all(mut cases: Vec<Case>) -> (usize, usize) {
    let mut failures = Vec::new();
    let (mut trace_attributes, mut trace_route) = (0, 0);
    for case in &mut cases {
        if case.matches.is_some() {
            trace_attributes += 1;
        } else {
            trace_route += 1;
        }
        let differences = check(case);
        if !differences.is_empty() {
            failures.push(format!("{}:\n{}", case.name, differences.join("\n")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    (trace_attributes, trace_route)
}

/// Every oracle case: `trace_attributes` from what mainline's Thor handed its serializer,
/// and `trace_route` (Valhalla JSON and OSRM) from what Odin handed Tyr.
#[test]
fn responses_match_mainline() {
    let (trace_attributes, trace_route) = check_all(cases(FIXTURES));
    assert!(
        trace_attributes >= 40,
        "only {trace_attributes} trace_attributes cases"
    );
    assert!(trace_route >= 20, "only {trace_route} trace_route cases");
}

/// The whole output of the oracle helper (`results.tsv`), outside the repository:
///
/// ```shell
/// VALINOR_TYR_TRACE_ORACLE=/mnt/maps/cardinal/valhalla-oracle/trace-attributes/results.tsv \
///   cargo test -p valinor-tyr trace_oracle_external -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs the oracle helper's output (VALINOR_TYR_TRACE_ORACLE)"]
fn trace_oracle_external() {
    let path = std::env::var("VALINOR_TYR_TRACE_ORACLE").expect("VALINOR_TYR_TRACE_ORACLE");
    let text = std::fs::read_to_string(path).expect("the results");
    let decode = |field: &str| base64_decode(field);
    let mut cases = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.get(1) != Some(&"ok") {
            continue;
        }
        let matches = decode(fields[3]);
        cases.push(Case {
            name: fields[0].to_owned(),
            api: Api::decode(decode(fields[2]).as_slice()).expect("a valid Api"),
            matches: (!matches.is_empty()).then(|| parse_matches(&matches)),
            response: decode(fields[4]),
        });
    }
    let (trace_attributes, trace_route) = check_all(cases);
    println!("{trace_attributes} trace_attributes and {trace_route} trace_route responses match");
}

/// Standard base64, as the helper writes it (no dependency for one ignored test).
fn base64_decode(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => u32::from(c - b'A'),
        b'a'..=b'z' => u32::from(c - b'a') + 26,
        b'0'..=b'9' => u32::from(c - b'0') + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("not base64: {c}"),
    };
    let bytes: Vec<u8> = text.bytes().filter(|&c| c != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut accumulator = 0_u32;
        for (i, &c) in chunk.iter().enumerate() {
            accumulator |= value(c) << (18 - 6 * i);
        }
        let [_, a, b, c] = accumulator.to_be_bytes();
        out.extend_from_slice(&[a, b, c][..chunk.len() - 1]);
    }
    out
}

// Hand-built cases for what the oracle cases don't reach.

fn edge_id(index: u64) -> GraphId {
    GraphId::try_from_components(0, 3015, index).expect("a valid ID")
}

fn result(edge: Option<u64>, state: bool, edge_index: Option<usize>) -> MatchResult {
    MatchResult {
        lnglat: (1.523_456_789, 42.507_654_321),
        distance_from: 7.654_321_987,
        edge_id: edge.map(edge_id),
        distance_along: 0.123_456_789,
        epoch_time: -1.0,
        state_id: state.then_some(StateId { time: 0, id: 0 }),
        is_break_point: false,
        begins_discontinuity: false,
        ends_discontinuity: false,
        edge_index,
    }
}

fn leg() -> TripLeg {
    let cost = |seconds, cost| {
        Some(PathCost {
            elapsed_cost: Some(Cost { seconds, cost }),
            transition_cost: Some(Cost {
                seconds: 0.5,
                cost: 1.0,
            }),
        })
    };
    TripLeg {
        osm_changeset: 7,
        shape: "abc".to_owned(),
        admin: vec![Admin {
            country_code: "AD".to_owned(),
            ..Admin::default()
        }],
        node: vec![
            Node {
                edge: Some(Edge {
                    name: vec![StreetName {
                        value: "Carrer".to_owned(),
                        ..StreetName::default()
                    }],
                    length_km: 0.123_45,
                    speed: 50.0,
                    speed_limit: 255,
                    mean_elevation: 1000,
                    travel_mode: TravelMode::KPedestrian as i32,
                    pedestrian_type: PedestrianType::KBlind as i32,
                    r#use: 99,
                    source_along_edge: 0.25,
                    target_along_edge: 1.0,
                    levels: vec![trip_leg::edge::Level {
                        start: 1.5,
                        end: 2.25,
                    }],
                    level_precision: 0,
                    ..Edge::default()
                }),
                cost: cost(0.0, 0.0),
                ..Node::default()
            },
            Node {
                cost: cost(12.345_67, 23.456_78),
                r#type: 1,
                ..Node::default()
            },
        ],
        ..TripLeg::default()
    }
}

fn api(routes: usize, format: options::Format) -> Api {
    Api {
        options: Some(Options {
            format: format as i32,
            action: options::Action::TraceAttributes as i32,
            units: Units::Miles as i32,
            ..Options::default()
        }),
        trip: Some(Trip {
            routes: (0..routes)
                .map(|_| TripRoute {
                    legs: vec![leg()],
                    ..TripRoute::default()
                })
                .collect(),
        }),
        info: Some(Info::default()),
        ..Api::default()
    }
}

fn serialize_json(
    api: &mut Api,
    controller: &AttributesController,
    results: &[MapMatchResult],
) -> Value {
    let response = serialize_trace_attributes(api, controller, results).expect("serializable");
    assert_eq!(response.content_type, crate::tyr::JSON_CONTENT_TYPE);
    serde_json::from_slice(&response.body).expect("valid JSON")
}

#[test]
fn matched_points_and_their_types() {
    let results = vec![(
        1.0,
        12.5,
        vec![
            result(Some(1), true, Some(0)),
            result(Some(1), false, Some(0)),
            // An edge whose index was never set reads as mainline's `kInvalidEdgeIndex`.
            result(Some(2), true, None),
            result(None, false, None),
        ],
    )];
    let mut api = api(1, options::Format::Json);
    let json = serialize_json(&mut api, &AttributesController::default(), &results);
    assert_eq!(
        json["matched_points"],
        json!([
            {"lon": 1.523_456, "lat": 42.507_654, "type": "matched", "edge_index": 0,
             "distance_along_edge": 0.123_456, "distance_from_trace_point": 7.654_321},
            {"lon": 1.523_456, "lat": 42.507_654, "type": "interpolated", "edge_index": 0,
             "distance_along_edge": 0.123_456, "distance_from_trace_point": 7.654_321},
            {"lon": 1.523_456, "lat": 42.507_654, "type": "matched", "edge_index": u64::MAX,
             "distance_along_edge": 0.123_456, "distance_from_trace_point": 7.654_321},
            {"lon": 1.523_456, "lat": 42.507_654, "type": "unmatched"},
        ])
    );
    assert_eq!(json["units"], "miles");
    assert_eq!(json["alternate_paths"], json!([]));
    assert_eq!(json["confidence_score"], 1.0);
    assert_eq!(json["raw_score"], 12.5);
    // The trip carries them too, for PBF.
    let points = &api.trip.as_ref().expect("trip").routes[0].matched_points;
    assert_eq!(points.len(), 4);
    assert_eq!(
        points[2].has_edge_index,
        Some(matched_point::HasEdgeIndex::EdgeIndex(u32::MAX))
    );
    assert_eq!(points[3].has_distance_along_edge, None);
    assert_eq!(
        points[3].r#type,
        matched_point::MatchType::KUnmatched as i32
    );
}

#[test]
fn edges_as_mainline_writes_them() {
    let results = vec![(1.0, 0.0, Vec::new())];
    let mut api = api(1, options::Format::Json);
    let json = serialize_json(&mut api, &AttributesController::default(), &results);
    let edge = &json["edges"][0];
    // An unlimited speed limit, miles, and feet.
    assert_eq!(edge["speed_limit"], "unlimited");
    assert_eq!(edge["speed"], 31);
    assert_eq!(edge["mean_elevation"], 3280);
    assert_eq!(edge["length"], 0.076);
    assert_eq!(edge["source_percent_along"], 0.25);
    assert!(edge.get("target_percent_along").is_none());
    // Blind pedestrians and unknown uses have no names in mainline's tables.
    assert_eq!(edge["pedestrian_type"], "null");
    assert_eq!(edge["use"], "null");
    assert!(edge.get("vehicle_type").is_none());
    // A level precision of 0 keeps one decimal, as rapidjson does.
    assert_eq!(edge["levels"], json!([[1.5, 2.2]]));
    assert_eq!(edge["names"], json!(["Carrer"]));
    assert_eq!(edge["speeds_non_faded"], json!({"no_flow": 0}));
    assert_eq!(
        edge["end_node"],
        json!({"elapsed_time": 12.345, "elapsed_cost": 23.456, "admin_index": 0,
               "type": "gate", "traffic_signal": false, "fork": false, "transition_time": 0.5})
    );
    assert_eq!(json["admins"], json!([{"country_code": "AD"}]));
    assert_eq!(json["osm_changeset"], 7);
    assert_eq!(json["shape"], "abc");
}

#[test]
fn strict_filters_select_exactly() {
    let options = Options {
        filter_action: FilterAction::Include as i32,
        filter_attributes: vec!["edge.names".to_owned(), "matched.type".to_owned()],
        ..Options::default()
    };
    let controller = AttributesController::new(&options, true);
    let results = vec![
        (1.0, 10.0, vec![result(Some(1), true, Some(0))]),
        (0.5, 20.0, vec![result(None, false, None)]),
    ];
    let mut api = api(2, options::Format::Json);
    if let Some(o) = api.options.as_mut() {
        o.has_id = Some(HasId::Id("x".to_owned()));
    }
    let json = serialize_json(&mut api, &controller, &results);
    assert_eq!(
        json,
        json!({
            "id": "x",
            "units": "miles",
            "admins": [{"country_code": "AD"}],
            // Names, lane connectivity, and signs are written whatever the filter says.
            "edges": [{"names": ["Carrer"]}],
            "matched_points": [{"type": "matched"}],
            "alternate_paths": [{
                "admins": [{"country_code": "AD"}],
                "edges": [{"names": ["Carrer"]}],
                "matched_points": [{"type": "unmatched"}],
            }],
        })
    );
    // Nothing that was filtered out is in the trip either.
    let routes = &api.trip.as_ref().expect("trip").routes;
    assert_eq!(routes[1].confidence_score.to_bits(), 0);
    assert_eq!(
        routes[0].matched_points[0],
        MatchedPoint {
            r#type: matched_point::MatchType::KMatched as i32,
            ..MatchedPoint::default()
        }
    );
}

#[test]
fn pbf_carries_the_trip_with_matches() {
    let results = vec![(1.0, 10.0, vec![result(Some(1), true, Some(3))])];
    let mut api = api(1, options::Format::Pbf);
    let response = serialize_trace_attributes(&mut api, &AttributesController::default(), &results)
        .expect("serializable");
    assert_eq!(response.content_type, crate::tyr::PBF_CONTENT_TYPE);
    let decoded = Api::decode(response.body.as_slice()).expect("a valid Api");
    assert!(decoded.options.is_none());
    let route = &decoded.trip.expect("the trip").routes[0];
    assert_eq!(route.raw_score.to_bits(), 10.0_f32.to_bits());
    assert_eq!(
        route.matched_points[0].has_edge_index,
        Some(matched_point::HasEdgeIndex::EdgeIndex(3))
    );
}

#[test]
fn inconsistent_inputs_are_errors() {
    let controller = AttributesController::default();
    let one = vec![(1.0, 0.0, Vec::new())];
    // No matches at all: mainline ends an array it never started.
    let error = serialize_trace_attributes(&mut api(1, options::Format::Json), &controller, &[])
        .expect_err("no matches");
    assert_eq!(error.code(), ErrorCode::ThorUnknown);
    // More matches than routes, and a route without legs.
    let two = vec![one[0].clone(), one[0].clone()];
    for format in [options::Format::Json, options::Format::Pbf] {
        let error = serialize_trace_attributes(&mut api(1, format), &controller, &two)
            .expect_err("too many matches");
        assert_eq!(error.code(), ErrorCode::ThorUnknown);
    }
    let mut no_legs = api(1, options::Format::Json);
    no_legs.trip.as_mut().expect("trip").routes[0].legs.clear();
    let error = serialize_trace_attributes(&mut no_legs, &controller, &one).expect_err("no legs");
    assert_eq!(error.code(), ErrorCode::ThorUnknown);
}
