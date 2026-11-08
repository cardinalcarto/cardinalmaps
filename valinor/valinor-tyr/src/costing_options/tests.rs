//! Tests of the costing option parsers.

#![allow(
    clippy::float_cmp,
    clippy::too_many_lines,
    reason = "the front must reproduce mainline's values exactly, and tests read best whole"
)]

use super::*;
use crate::costing_options::base::CostingExt;
use valhalla_proto::costing::HasName;
use valhalla_proto::costing::options::{
    HasCyclingSpeed, HasFlowMask, HasMaxHikingDifficulty, HasTopSpeed, HasTransportType,
    HasUseTolls, HasWalkingSpeed,
};
use valhalla_proto::{FilterAction, HierarchyLimits};

const ALL: [CostingType; 13] = [
    CostingType::None,
    CostingType::Bicycle,
    CostingType::Bus,
    CostingType::MotorScooter,
    CostingType::Multimodal,
    CostingType::Pedestrian,
    CostingType::Transit,
    CostingType::Truck,
    CostingType::Motorcycle,
    CostingType::Taxi,
    CostingType::Auto,
    CostingType::Bikeshare,
    CostingType::AutoPedestrian,
];

fn doc(json: &str) -> JsonValue {
    JsonValue::parse(json.as_bytes()).expect("valid JSON")
}

/// Parses one costing's options from `{"o": <json>}`.
fn parse(costing_type: CostingType, json: &str) -> (Costing, Vec<CodedDescription>) {
    let doc = doc(&format!(r#"{{"o": {json}}}"#));
    let mut costing = Costing::default();
    let mut warnings = Vec::new();
    parse_costing(&doc, "/o", &mut costing, &mut warnings, Some(costing_type)).expect("parses");
    (costing, warnings)
}

fn costing_options(costing: &mut Costing) -> valhalla_proto::costing::Options {
    costing.has_options_mut().clone()
}

#[test]
fn names_round_trip() {
    for costing in ALL {
        assert_eq!(parse_costing_name(costing_name(costing)), Some(costing));
    }
    assert_eq!(parse_costing_name(""), Some(CostingType::None));
    for deprecated in [
        "auto_shorter",
        "hov",
        "auto_data_fix",
        "none_",
        "auto_",
        "Auto",
    ] {
        assert_eq!(parse_costing_name(deprecated), None, "{deprecated}");
    }
}

/// The defaults of every costing, as mainline 3.9.1 fills them in for an empty request.
///
/// The auto, bicycle, and pedestrian defaults are also checked against mainline's own output
/// by the front's capture tests; the snapshots cover the rest.
#[test]
fn defaults() {
    for costing_type in ALL {
        let (costing, warnings) = parse(costing_type, "{}");
        assert!(warnings.is_empty());
        insta::assert_debug_snapshot!(costing_name(costing_type), costing);
    }
}

#[test]
fn names_and_types() {
    // Auto-derived costings take auto's options under their own name.
    let (costing, _) = parse(CostingType::Bus, r#"{"name": "mine", "use_tolls": 0.1}"#);
    assert_eq!(costing.r#type, CostingType::Bus as i32);
    assert_eq!(costing.has_name, Some(HasName::Name("bus".into())));
    let (mut costing, _) = parse(CostingType::Auto, r#"{"name": "mine"}"#);
    assert_eq!(costing.has_name, Some(HasName::Name("mine".into())));
    assert_eq!(
        costing_options(&mut costing).has_top_speed,
        Some(HasTopSpeed::TopSpeed(140.0))
    );
    // The combined costings only get a type.
    let (costing, _) = parse(CostingType::Multimodal, r#"{"name": "x"}"#);
    assert_eq!(
        costing,
        Costing {
            r#type: CostingType::Multimodal as i32,
            ..Costing::default()
        }
    );
}

#[test]
fn out_of_range_values_are_clamped_to_the_default() {
    let (mut costing, warnings) = parse(
        CostingType::Auto,
        r#"{"use_tolls": 5, "top_speed": 2000, "maneuver_penalty": -1, "fixed_speed": 300}"#,
    );
    let co = costing_options(&mut costing);
    assert_eq!(co.has_use_tolls, Some(HasUseTolls::UseTolls(0.5)));
    assert_eq!(co.has_top_speed, Some(HasTopSpeed::TopSpeed(140.0)));
    assert_eq!(co.fixed_speed, 0);
    let warnings: Vec<_> = warnings
        .iter()
        .map(|w| (w.code, w.description.as_str()))
        .collect();
    assert_eq!(
        warnings,
        [
            (500, "'/maneuver_penalty' has been clamped to 5.000000"),
            (500, "'/fixed_speed' has been clamped to 0"),
            (500, "'/use_tolls' has been clamped to 0.500000"),
            (500, "'/top_speed' has been clamped to 140"),
        ]
    );
    // Values are converted as mainline converts them: an unsigned top speed truncates.
    let (mut costing, warnings) = parse(CostingType::Auto, r#"{"top_speed": "99.7"}"#);
    // `"99.7"` is not an unsigned integer to boost's lexical_cast, so this is the default.
    assert_eq!(
        costing_options(&mut costing).has_top_speed,
        Some(HasTopSpeed::TopSpeed(140.0))
    );
    assert!(warnings.is_empty());
    let (mut costing, _) = parse(CostingType::Auto, "{\"top_speed\": 99.7}");
    assert_eq!(
        costing_options(&mut costing).has_top_speed,
        Some(HasTopSpeed::TopSpeed(99.0))
    );
    // A truck's top speed is a float.
    let (mut costing, _) = parse(CostingType::Truck, "{\"top_speed\": 99.7}");
    assert_eq!(
        costing_options(&mut costing).has_top_speed,
        Some(HasTopSpeed::TopSpeed(99.7))
    );
}

#[test]
fn type_specific_defaults() {
    let (mut costing, _) = parse(CostingType::Bicycle, r#"{"bicycle_type": "ROAD"}"#);
    let co = costing_options(&mut costing);
    assert_eq!(
        co.has_transport_type,
        Some(HasTransportType::TransportType("road".into()))
    );
    assert_eq!(
        co.has_cycling_speed,
        Some(HasCyclingSpeed::CyclingSpeed(25.0))
    );
    let (mut costing, _) = parse(CostingType::Bicycle, r#"{"bicycle_type": "unicycle"}"#);
    let co = costing_options(&mut costing);
    // Unknown types keep their name but get hybrid defaults.
    assert_eq!(
        co.has_transport_type,
        Some(HasTransportType::TransportType("unicycle".into()))
    );
    assert_eq!(
        co.has_cycling_speed,
        Some(HasCyclingSpeed::CyclingSpeed(18.0))
    );

    let (mut costing, warnings) = parse(
        CostingType::Pedestrian,
        r#"{"type": "wheelchair", "walking_speed": 30, "max_hiking_difficulty": 6}"#,
    );
    let co = costing_options(&mut costing);
    assert_eq!(
        co.has_walking_speed,
        Some(HasWalkingSpeed::WalkingSpeed(4.0))
    );
    assert_eq!(
        co.has_max_hiking_difficulty,
        Some(HasMaxHikingDifficulty::MaxHikingDifficulty(6))
    );
    assert_eq!(
        warnings[0].description,
        "'/walking_speed' has been clamped to 4.000000"
    );
    let (mut costing, warnings) = parse(CostingType::Pedestrian, r#"{"max_hiking_difficulty": 7}"#);
    assert_eq!(
        costing_options(&mut costing).has_max_hiking_difficulty,
        Some(HasMaxHikingDifficulty::MaxHikingDifficulty(1))
    );
    assert_eq!(warnings.len(), 1);
}

#[test]
fn speed_types_and_hierarchy_limits() {
    for (speed_types, mask) in [
        (r#"["freeflow", "current", 5, "bogus"]"#, 1 | 8),
        ("[]", 0),
        (r#""freeflow""#, DEFAULT_FLOW_MASK),
    ] {
        let (mut costing, _) = parse(
            CostingType::Auto,
            &format!("{{\"speed_types\": {speed_types}}}"),
        );
        assert_eq!(
            costing_options(&mut costing).has_flow_mask,
            Some(HasFlowMask::FlowMask(mask)),
            "{speed_types}"
        );
    }

    let (mut costing, _) = parse(
        CostingType::Auto,
        r#"{"hierarchy_limits": {"1": {"max_up_transitions": 7}, "2": {}, "3": {"max_up_transitions": 1}}}"#,
    );
    let co = costing_options(&mut costing);
    // Only the levels the request set, and only the routing levels (0, 1, and 2).
    assert_eq!(co.hierarchy_limits.len(), 1);
    assert_eq!(
        co.hierarchy_limits.get(&1),
        Some(&HierarchyLimits {
            up_transition_count: 0,
            max_up_transitions: 7,
            expand_within_dist: f32::MAX,
        })
    );
}

#[test]
fn pbf_values_are_kept_unless_the_json_overrides_them() {
    let mut costing = Costing::default();
    let co = costing.has_options_mut();
    co.has_use_tolls = Some(HasUseTolls::UseTolls(0.2));
    co.has_flow_mask = Some(HasFlowMask::FlowMask(1));
    co.exclude_tolls = true;
    let mut warnings = Vec::new();
    parse_costing(
        &doc("{}"),
        "/o",
        &mut costing,
        &mut warnings,
        Some(CostingType::Auto),
    )
    .expect("parses");
    let co = costing_options(&mut costing);
    assert_eq!(co.has_use_tolls, Some(HasUseTolls::UseTolls(0.2)));
    assert_eq!(co.has_flow_mask, Some(HasFlowMask::FlowMask(1)));
    assert!(co.exclude_tolls);

    // A bogus flow mask is dropped (and defaulted).
    let mut costing = Costing::default();
    costing.has_options_mut().has_flow_mask = Some(HasFlowMask::FlowMask(99));
    parse_costing(
        &doc("{}"),
        "/o",
        &mut costing,
        &mut warnings,
        Some(CostingType::Auto),
    )
    .expect("parses");
    assert_eq!(
        costing_options(&mut costing).has_flow_mask,
        Some(HasFlowMask::FlowMask(DEFAULT_FLOW_MASK))
    );

    let doc = doc(r#"{"o": {"use_tolls": 0.9, "exclude_tolls": false}}"#);
    parse_costing(
        &doc,
        "/o",
        &mut costing,
        &mut warnings,
        Some(CostingType::Auto),
    )
    .expect("parses");
    assert_eq!(
        costing_options(&mut costing).has_use_tolls,
        Some(HasUseTolls::UseTolls(0.9))
    );
}

#[test]
fn costings_without_a_type_name_it() {
    let doc =
        doc(r#"{"a": {"costing": "pedestrian"}, "b": {"costing": 5}, "c": {"costing": "x"}}"#);
    let mut warnings = Vec::new();
    let mut costing = Costing::default();
    parse_costing(&doc, "/a", &mut costing, &mut warnings, None).expect("parses");
    assert_eq!(costing.r#type, CostingType::Pedestrian as i32);
    for (key, expected) in [
        (
            "/b",
            ValhallaError::new(ErrorCode::RecostingCostingRequired),
        ),
        (
            "/missing",
            ValhallaError::new(ErrorCode::RecostingCostingRequired),
        ),
        (
            "/c",
            ValhallaError::with_detail(ErrorCode::UnknownCosting, "'x'"),
        ),
    ] {
        assert_eq!(
            parse_costing(&doc, key, &mut Costing::default(), &mut warnings, None),
            Err(expected),
            "{key}"
        );
    }
}

#[test]
fn combined_costings_parse_their_parts() {
    let doc = doc(r#"{"costing_options": {"pedestrian": {"walking_speed": 4}}}"#);
    let mut options = Options {
        costing_type: CostingType::Bikeshare.into(),
        ..Options::default()
    };
    let mut warnings = Vec::new();
    parse_costings(&doc, "/costing_options", &mut options, &mut warnings).expect("parses");
    let mut keys: Vec<i32> = options.costings.keys().copied().collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            CostingType::Bicycle as i32,
            CostingType::Pedestrian as i32,
            CostingType::Bikeshare as i32
        ]
    );
    let pedestrian = options
        .costings
        .get_mut(&(CostingType::Pedestrian as i32))
        .expect("pedestrian");
    assert_eq!(
        costing_options(pedestrian).has_walking_speed,
        Some(HasWalkingSpeed::WalkingSpeed(4.0))
    );
}

#[test]
fn transit_filters() {
    let (mut costing, _) = parse(
        CostingType::Transit,
        r#"{"filters": {"stops": {"action": "exclude", "ids": ["a", "b"]}, "routes": {"action": "nope", "ids": ["c"]}}}"#,
    );
    let co = costing_options(&mut costing);
    assert_eq!(co.filter_stop_action, FilterAction::Exclude as i32);
    assert_eq!(co.filter_stop_ids, ["a", "b"]);
    assert_eq!(co.filter_route_action, FilterAction::NoAction as i32);
    assert!(co.filter_route_ids.is_empty());

    let doc = doc(r#"{"o": {"filters": {"operators": {"action": "include", "ids": [1]}}}}"#);
    assert_eq!(
        parse_costing(
            &doc,
            "/o",
            &mut Costing::default(),
            &mut Vec::new(),
            Some(CostingType::Transit)
        ),
        Err(ValhallaError::with_detail(
            ErrorCode::LokiUnknown,
            "IsString()"
        ))
    );
}
