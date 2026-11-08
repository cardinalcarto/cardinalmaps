use super::*;
use crate::test_support::{captures, json_differences};
use crate::tyr::polyline::{self, LngLat};
use crate::tyr::serialize_directions;
use serde_json::{Value, json};
use valhalla_proto::trip_leg::{Admin, Cost, Edge, PathCost};
use valhalla_proto::{
    BoundingBox, CodedDescription, Correlation, Costing, Directions, DirectionsRoute, Info,
    LevelChange, Location, StreetName, Summary, TransitPlatformInfo, TransitRouteInfo, Trip,
    TripRoute, TripSign, costing, options,
};
use valhalla_response::ErrorCode;

fn parse(response: &Response) -> Value {
    assert_eq!(response.content_type, crate::tyr::JSON_CONTENT_TYPE);
    serde_json::from_slice(&response.body).expect("valid JSON")
}

fn text(api: &Api) -> String {
    String::from_utf8(serialize(api).expect("serializable").body).expect("UTF-8")
}

/// Every captured Valhalla JSON route response, from what Odin handed Tyr.
#[test]
fn route_responses_match_mainline() {
    let mut checked = 0;
    for capture in captures("json") {
        if capture.request().0 != "route" {
            continue;
        }
        let (status, expected) = capture.json_response();
        let Some(mut api) = capture.odin_output() else {
            assert!(
                status != 200 || !capture.has_directions(),
                "{}",
                capture.name()
            );
            continue;
        };
        assert_eq!(status, 200, "{}", capture.name());
        let response = serialize_directions(&mut api).expect("serializable");
        let differences = json_differences(&expected, &parse(&response), 20);
        assert!(
            differences.is_empty(),
            "{}:\n{}",
            capture.name(),
            differences.join("\n")
        );
        checked += 1;
    }
    assert!(checked > 20, "only {checked} JSON captures");
}

#[expect(clippy::unnecessary_wraps, reason = "fills optional fields")]
fn ll(lat: f64, lng: f64) -> Option<LatLng> {
    Some(LatLng {
        has_lat: Some(lat_lng::HasLat::Lat(lat)),
        has_lng: Some(lat_lng::HasLng::Lng(lng)),
    })
}

#[expect(clippy::unnecessary_wraps, reason = "fills optional fields")]
fn cost(seconds: f64, cost: f64) -> Option<PathCost> {
    Some(PathCost {
        elapsed_cost: Some(Cost { seconds, cost }),
        transition_cost: None,
    })
}

fn recost(seconds: Option<f64>) -> PathCost {
    PathCost {
        elapsed_cost: seconds.map(|seconds| Cost { seconds, cost: 0.0 }),
        transition_cost: None,
    }
}

fn location(lat: f64, lng: f64, original_index: u32) -> Location {
    Location {
        ll: ll(lat, lng),
        correlation: Some(Correlation {
            original_index,
            ..Correlation::default()
        }),
        ..Location::default()
    }
}

/// A one-leg route over three nodes, with two maneuvers, built by hand.
fn route_api() -> Api {
    let shape = [
        LngLat {
            lng: 1.0,
            lat: 42.0,
        },
        LngLat {
            lng: 1.0,
            lat: 42.001,
        },
        LngLat {
            lng: 1.001,
            lat: 42.001,
        },
    ];
    let edge = |begin, end, end_heading| Edge {
        begin_shape_index: begin,
        end_shape_index: end,
        end_heading,
        traversability: 3,
        length_km: 0.111,
        ..Edge::default()
    };
    let trip_leg = TripLeg {
        shape: polyline::encode(&shape, 1e6),
        node: vec![
            Node {
                edge: Some(edge(0, 1, 359)),
                cost: cost(0.0, 0.0),
                recosts: vec![recost(Some(1.0)), recost(Some(2.0))],
                ..Node::default()
            },
            Node {
                edge: Some(edge(1, 2, 90)),
                cost: cost(10.5, 12.25),
                recosts: vec![recost(Some(11.0)), recost(None)],
                ..Node::default()
            },
            Node {
                cost: cost(20.123_456, 30.987_654),
                recosts: vec![recost(Some(25.0)), recost(None)],
                ..Node::default()
            },
        ],
        ..TripLeg::default()
    };
    let maneuver = |r#type, begin, end| Maneuver {
        r#type,
        text_instruction: format!("maneuver {type}"),
        begin_path_index: begin,
        end_path_index: end,
        begin_shape_index: begin,
        end_shape_index: end,
        begin_heading: 7,
        time: 10.123_456,
        length: 0.111_111,
        ..Maneuver::default()
    };
    let directions_leg = DirectionsLeg {
        location: vec![location(42.0, 1.0, 0), location(42.001, 1.001, 1)],
        summary: Some(Summary {
            length: 0.222_222,
            time: 20.123_456,
            bbox: Some(BoundingBox {
                min_ll: ll(42.000_000_4, 1.000_000_7),
                max_ll: ll(42.001, 1.001),
            }),
            ..Summary::default()
        }),
        maneuver: vec![maneuver(1, 0, 1), maneuver(10, 1, 2), maneuver(4, 2, 2)],
        shape: polyline::encode(&shape, 1e6),
        ..DirectionsLeg::default()
    };
    let named = |name: &str| Costing {
        has_name: Some(costing::HasName::Name(name.to_string())),
        ..Costing::default()
    };
    Api {
        options: Some(Options {
            action: options::Action::Route.into(),
            has_language: Some(options::HasLanguage::Language("en-US".to_string())),
            recostings: vec![named("a"), named("b")],
            ..Options::default()
        }),
        trip: Some(Trip {
            routes: vec![TripRoute {
                legs: vec![trip_leg],
                ..TripRoute::default()
            }],
        }),
        directions: Some(Directions {
            routes: vec![DirectionsRoute {
                legs: vec![directions_leg],
            }],
        }),
        ..Api::default()
    }
}

fn options_mut(api: &mut Api) -> &mut Options {
    api.options.as_mut().expect("options")
}

fn first_maneuver_mut(api: &mut Api) -> &mut Maneuver {
    &mut api.directions.as_mut().expect("directions").routes[0].legs[0].maneuver[0]
}

fn trip_leg_mut(api: &mut Api) -> &mut TripLeg {
    &mut api.trip.as_mut().expect("trip").routes[0].legs[0]
}

/// The whole response, byte for byte, as mainline's writer calls produce it:
/// coordinates at six decimals, times and costs truncated (not rounded) to three,
/// lengths to three (kilometers), and recosted times.
#[test]
fn hand_built_route() {
    insta::assert_snapshot!(text(&route_api()));
}

#[test]
fn recosted_times_quirks() {
    let body: Value = serde_json::from_str(&text(&route_api())).expect("JSON");
    let maneuvers = &body["trip"]["legs"][0]["maneuvers"];
    // Mainline never advances the begin node's recost iterator,
    // so recosting "b" is measured from recosting "a"'s start (1.0), not its own (2.0).
    assert_eq!(maneuvers[0]["time_a"], json!(10.0));
    assert_eq!(maneuvers[0]["time_b"], Value::Null);
    assert_eq!(maneuvers[1]["time_a"], json!(14.0));
    // The leg and route summaries: missing elapsed costs become null.
    assert_eq!(body["trip"]["legs"][0]["summary"]["time_a"], json!(25.0));
    assert_eq!(body["trip"]["legs"][0]["summary"]["time_b"], Value::Null);
    assert_eq!(body["trip"]["summary"]["time_a"], json!(25.0));
    assert_eq!(body["trip"]["summary"]["time_b"], Value::Null);
}

#[test]
fn miles_have_four_decimals() {
    let mut api = route_api();
    options_mut(&mut api).units = Units::Miles.into();
    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    assert_eq!(body["trip"]["units"], "miles");
    assert_eq!(body["trip"]["summary"]["length"], json!(0.2222));
    assert_eq!(
        body["trip"]["legs"][0]["maneuvers"][0]["length"],
        json!(0.1111)
    );
}

#[test]
fn alternates_and_id() {
    let mut api = route_api();
    let mut alternate = api.directions.as_ref().expect("directions").routes[0].clone();
    alternate.legs[0].shape = "alternate".to_string();
    api.directions
        .as_mut()
        .expect("directions")
        .routes
        .push(alternate);
    let trip_route = api.trip.as_ref().expect("trip").routes[0].clone();
    api.trip.as_mut().expect("trip").routes.push(trip_route);
    options_mut(&mut api).has_id = Some(options::HasId::Id("my id".to_string()));
    api.info = Some(Info {
        warnings: vec![CodedDescription {
            description: "a warning".to_string(),
            code: 200,
        }],
        ..Info::default()
    });

    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    assert_eq!(body["id"], "my id");
    assert_eq!(body["alternates"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        body["alternates"][0]["trip"]["legs"][0]["shape"],
        "alternate"
    );
    // Warnings go into every route.
    let warnings = json!([{"code": 200, "text": "a warning"}]);
    assert_eq!(body["trip"]["warnings"], warnings);
    assert_eq!(body["alternates"][0]["trip"]["warnings"], warnings);
    // The first route's object is the response: its trip, then the alternates, then the id.
    let response = text(&api);
    assert!(
        response.starts_with(r#"{"trip":{"locations":"#),
        "{response}"
    );
    let alternates = response
        .find(r#","alternates":[{"trip":"#)
        .expect("alternates");
    let id = response.find(r#","id":"my id"}"#).expect("id");
    assert!(alternates < id && response.ends_with(r#""id":"my id"}"#));
}

#[test]
fn locations() {
    let mut api = route_api();
    let legs = &mut api.directions.as_mut().expect("directions").routes[0].legs;
    let mut second = legs[0].clone();
    second.location = vec![location(42.001, 1.001, 1), location(42.002, 1.002, 2)];
    let first = &mut legs[0].location[0];
    first.r#type = location::Type::KVia.into();
    first.name = "home".to_string();
    first.street = "a street".to_string();
    first.has_heading = Some(location::HasHeading::Heading(90));
    first.date_time = "2026-11-09T08:00".to_string();
    first.time_zone_offset = "+01:00".to_string();
    first.time_zone_name = "Europe/Andorra".to_string();
    first.waiting_secs = 30.9;
    first.side_of_street = location::SideOfStreet::KLeft.into();
    legs.push(second);
    let trip_leg = api.trip.as_ref().expect("trip").routes[0].legs[0].clone();
    api.trip.as_mut().expect("trip").routes[0]
        .legs
        .push(trip_leg);

    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    assert_eq!(
        body["trip"]["locations"],
        json!([
            {
                "type": "via", "lat": 42.0, "lon": 1.0, "name": "home", "street": "a street",
                "heading": 90, "date_time": "2026-11-09T08:00", "time_zone_offset": "+01:00",
                "time_zone_name": "Europe/Andorra", "waiting": 30, "side_of_street": "left",
                "original_index": 0
            },
            {"type": "break", "lat": 42.001, "lon": 1.001, "original_index": 1},
            {"type": "break", "lat": 42.002, "lon": 1.002, "original_index": 2},
        ])
    );
}

#[test]
fn maneuver_extras() {
    let mut api = route_api();
    let maneuver = first_maneuver_mut(&mut api);
    maneuver.verbal_transition_alert_instruction = "alert".to_string();
    maneuver.street_name = vec![StreetName {
        value: "Main".to_string(),
        ..StreetName::default()
    }];
    maneuver.begin_street_name = vec![StreetName {
        value: "Begin".to_string(),
        ..StreetName::default()
    }];
    maneuver.portions_toll = true;
    maneuver.portions_highway = true;
    maneuver.portions_ferry = true;
    maneuver.portions_unpaved = true;
    maneuver.has_time_restrictions = true;
    maneuver.roundabout_exit_count = 2;
    maneuver.depart_instruction = "depart".to_string();
    maneuver.verbal_multi_cue = true;
    maneuver.travel_mode = TravelMode::KBicycle.into();
    maneuver.bicycle_type = BicycleType::KMountain.into();
    let element = |text: &str, consecutive_count| TripSignElement {
        text: text.to_string(),
        consecutive_count,
        ..TripSignElement::default()
    };
    maneuver.sign = Some(TripSign {
        exit_numbers: vec![element("12A", 0)],
        exit_onto_streets: vec![element("I 5", 1)],
        exit_toward_locations: vec![element("Seattle", 0), element("Tacoma", 2)],
        exit_names: vec![element("Main", 0)],
        ..TripSign::default()
    });

    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    let maneuver = &body["trip"]["legs"][0]["maneuvers"][0];
    assert_eq!(maneuver["verbal_transition_alert_instruction"], "alert");
    assert_eq!(maneuver["street_names"], json!(["Main"]));
    assert_eq!(maneuver["begin_street_names"], json!(["Begin"]));
    for key in [
        "toll",
        "highway",
        "ferry",
        "rough",
        "has_time_restrictions",
        "verbal_multi_cue",
    ] {
        assert_eq!(maneuver[key], json!(true), "{key}");
    }
    assert_eq!(maneuver["roundabout_exit_count"], json!(2));
    assert_eq!(maneuver["depart_instruction"], "depart");
    assert_eq!(maneuver["travel_mode"], "bicycle");
    assert_eq!(maneuver["travel_type"], "mountain");
    assert_eq!(
        maneuver["sign"],
        json!({
            "exit_number_elements": [{"text": "12A"}],
            "exit_branch_elements": [{"text": "I 5", "consecutive_count": 1}],
            "exit_toward_elements": [{"text": "Seattle"}, {"text": "Tacoma", "consecutive_count": 2}],
            "exit_name_elements": [{"text": "Main"}],
        })
    );
    // The portions mark the leg summary too, but not the route summary.
    let summary = &body["trip"]["legs"][0]["summary"];
    for key in [
        "has_toll",
        "has_highway",
        "has_ferry",
        "has_time_restrictions",
    ] {
        assert_eq!(summary[key], json!(true), "{key}");
        assert_eq!(body["trip"]["summary"][key], json!(false), "{key}");
    }
}

#[test]
fn travel_mode_names() {
    let cases = [
        (TravelMode::KDrive, 0, ("drive", "car")),
        (
            TravelMode::KDrive,
            VehicleType::KMotorScooter as i32,
            ("drive", "motor_scooter"),
        ),
        (TravelMode::KDrive, 99, ("drive", "car")),
        (
            TravelMode::KPedestrian,
            PedestrianType::KWheelchair as i32,
            ("pedestrian", "wheelchair"),
        ),
        (
            TravelMode::KPedestrian,
            PedestrianType::KBlind as i32,
            ("pedestrian", "foot"),
        ),
        (
            TravelMode::KBicycle,
            BicycleType::KHybrid as i32,
            ("bicycle", "hybrid"),
        ),
        (
            TravelMode::KTransit,
            TransitType::KCableCar as i32,
            ("transit", "cable_car"),
        ),
        (TravelMode::KTransit, 99, ("transit", "rail")),
    ];
    for (mode, kind, expected) in cases {
        let maneuver = Maneuver {
            travel_mode: mode.into(),
            vehicle_type: kind,
            pedestrian_type: kind,
            bicycle_type: kind,
            transit_type: kind,
            ..Maneuver::default()
        };
        assert_eq!(travel_mode_type(&maneuver), Ok(expected), "{mode:?} {kind}");
    }
    let unknown_mode = Maneuver {
        travel_mode: 7,
        ..Maneuver::default()
    };
    let error = travel_mode_type(&unknown_mode).expect_err("unknown mode");
    assert_eq!(error.message(), "Unknown: Unknown travel mode");
}

#[test]
fn turn_lanes_only_when_asked() {
    let mut api = route_api();
    let lanes = vec![
        TurnLane {
            directions_mask: 1,
            state: turn_lane::State::KActive.into(),
            active_direction: 1,
        },
        TurnLane {
            directions_mask: 6,
            state: turn_lane::State::KValid.into(),
            active_direction: 4,
        },
        TurnLane {
            directions_mask: 8,
            state: turn_lane::State::KInvalid.into(),
            active_direction: 0,
        },
    ];
    trip_leg_mut(&mut api).node[0]
        .edge
        .as_mut()
        .expect("edge")
        .turn_lanes = lanes;
    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    assert!(
        body["trip"]["legs"][0]["maneuvers"][1]
            .get("lanes")
            .is_none()
    );

    options_mut(&mut api).turn_lanes = true;
    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    let maneuvers = &body["trip"]["legs"][0]["maneuvers"];
    // The lanes are on the edge before the maneuver, so the depart maneuver has none.
    assert!(maneuvers[0].get("lanes").is_none());
    assert_eq!(
        maneuvers[1]["lanes"],
        json!([{"directions": 1, "active": 1}, {"directions": 6, "valid": 4}, {"directions": 8}])
    );
}

#[test]
fn admins_and_crossings() {
    let mut api = route_api();
    let admin = |code: &str| Admin {
        country_code: code.to_string(),
        country_text: format!("{code} country"),
        state_code: String::new(),
        state_text: String::new(),
    };
    let leg = trip_leg_mut(&mut api);
    leg.admin = vec![admin("AD"), admin("ES")];
    leg.node[1].admin_index = 1;
    leg.node[2].admin_index = 1;

    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    assert!(body["trip"]["legs"][0]["summary"].get("admins").is_none());

    options_mut(&mut api).admin_crossings = true;
    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    let summary = &body["trip"]["legs"][0]["summary"];
    assert_eq!(summary["admins"][1]["country_text"], "ES country");
    assert_eq!(
        summary["admin_crossings"],
        json!([{"from_admin_index": 0, "to_admin_index": 1, "begin_shape_index": 0, "end_shape_index": 1}])
    );

    // A single admin has no crossings array.
    trip_leg_mut(&mut api).admin.truncate(1);
    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    assert!(
        body["trip"]["legs"][0]["summary"]
            .get("admin_crossings")
            .is_none()
    );
}

#[test]
fn level_changes_use_their_precision() {
    let mut api = route_api();
    api.directions.as_mut().expect("directions").routes[0].legs[0].level_changes = vec![
        LevelChange {
            shape_index: 0,
            level: 1.0,
            precision: 0,
        },
        LevelChange {
            shape_index: 2,
            level: -1.25,
            precision: 2,
        },
    ];
    let response = text(&api);
    assert!(
        response.contains(r#""level_changes":[[0,1.0],[2,-1.25]]"#),
        "{response}"
    );
}

#[test]
fn transit_stop_precision_leaks() {
    let mut api = route_api();
    first_maneuver_mut(&mut api).transit_info = Some(TransitRouteInfo {
        onestop_id: "r-1".to_string(),
        color: 255,
        text_color: 0,
        transit_stops: vec![
            TransitPlatformInfo {
                r#type: transit_platform_info::Type::KStation.into(),
                station_name: "Central".to_string(),
                assumed_schedule: true,
                ll: ll(42.123_456_789, 1.987_654_321),
                ..TransitPlatformInfo::default()
            },
            TransitPlatformInfo::default(),
        ],
        ..TransitRouteInfo::default()
    });
    let response = text(&api);
    assert!(
        response.contains(concat!(
            r#""transit_info":{"onestop_id":"r-1","color":255,"text_color":0,"#,
            r#""transit_stops":[{"type":"station","name":"Central","assumed_schedule":true,"#,
            r#""lat":42.123456,"lon":1.987654},{"type":"stop","assumed_schedule":false}]}"#
        )),
        "{response}"
    );
    // Mainline never resets the precision after a stop's coordinates,
    // but every double written after them comes after another `set_precision`,
    // so the leak never shows: the next maneuver's time is still truncated to three decimals.
    let body: Value = serde_json::from_str(&response).expect("JSON");
    assert_eq!(
        body["trip"]["legs"][0]["maneuvers"][1]["time"],
        json!(10.123)
    );
}

#[test]
fn elevation_in_meters_and_feet() {
    let mut api = route_api();
    {
        let leg = trip_leg_mut(&mut api);
        for (node, heights) in leg.node.iter_mut().zip([[100.0, 110.0], [110.0, 90.0]]) {
            let edge = node.edge.as_mut().expect("edge");
            edge.elevation = heights.to_vec();
            edge.elevation_sampling_interval = 30.0;
        }
    }
    options_mut(&mut api).elevation_interval = 20.0;
    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    let leg = &body["trip"]["legs"][0];
    assert_eq!(leg["elevation_interval"], json!(20.0));
    // Postings every 20 m over two 30 m edges, interpolated and truncated to one decimal.
    assert_eq!(leg["elevation"], json!([100.0, 106.6, 103.3, 90.0]));

    options_mut(&mut api).units = Units::Miles.into();
    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    let leg = &body["trip"]["legs"][0];
    assert_eq!(leg["elevation_interval"], json!(65.6));
    assert_eq!(leg["elevation"][0], json!(328.0));
}

#[test]
fn elevation_needs_samples() {
    let mut api = route_api();
    options_mut(&mut api).elevation_interval = 20.0;
    let error = serialize(&api).expect_err("no elevation");
    assert_eq!(error.code(), ErrorCode::OdinUnknown);
}

#[test]
fn bridges_are_interpolated() {
    let edge = |heights: &[f32], bridge| Node {
        edge: Some(Edge {
            elevation: heights.to_vec(),
            elevation_sampling_interval: 10.0,
            bridge,
            ..Edge::default()
        }),
        ..Node::default()
    };
    let leg = TripLeg {
        node: vec![
            edge(&[0.0, 10.0], false),
            edge(&[10.0, 50.0], true),
            edge(&[50.0, 50.0], true),
            edge(&[50.0, 40.0], false),
            Node::default(),
        ],
        ..TripLeg::default()
    };
    // Traced by hand through mainline's `get_elevation`:
    // with the sampling interval equal to the edges', each posting lags one edge behind
    // (it interpolates at the start of the new sample), giving 0, 10, 50, 50, 40.
    // The bridge run is recorded as postings 1 to 2, and `update_bridge_elevations`
    // draws a line from posting 0 (0) to posting 3 (50) over them, overshooting to 100.
    // That is mainline's behavior, and we keep it so elevations match.
    assert_eq!(
        elevation(&leg, 10.0).expect("elevation"),
        [0.0, 50.0, 100.0, 50.0, 40.0]
    );
}

#[test]
fn linear_references_for_routes_only() {
    let mut api = route_api();
    options_mut(&mut api).linear_references = true;
    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    let references = body["trip"]["linear_references"].as_array().expect("array");
    assert_eq!(references.len(), 2);
    let trip_leg = &api.trip.as_ref().expect("trip").routes[0].legs[0];
    assert_eq!(
        references,
        &openlr_edges(trip_leg)
            .expect("references")
            .into_iter()
            .map(Value::String)
            .collect::<Vec<_>>()
    );

    options_mut(&mut api).action = options::Action::OptimizedRoute.into();
    let body: Value = serde_json::from_str(&text(&api)).expect("JSON");
    assert!(body["trip"].get("linear_references").is_none());
}

#[test]
fn inconsistent_requests_fail() {
    // No routes: mainline's writer asserts.
    let mut api = route_api();
    api.directions = Some(Directions::default());
    let error = serialize(&api).expect_err("no routes");
    assert_eq!(
        error.message(),
        "Unknown: level_stack_.GetSize() >= sizeof(Level)"
    );
    options_mut(&mut api).has_id = Some(options::HasId::Id("x".to_string()));
    assert_eq!(
        serialize(&api).expect_err("no routes").message(),
        "Unknown: !hasRoot_"
    );

    // Directions without a trip.
    let mut api = route_api();
    api.trip = None;
    assert_eq!(
        serialize(&api).expect_err("no trip").code(),
        ErrorCode::OdinUnknown
    );

    // A maneuver past the trip's nodes.
    let mut api = route_api();
    first_maneuver_mut(&mut api).end_path_index = 9;
    assert!(serialize(&api).is_err());

    // More recosts than recostings.
    let mut api = route_api();
    options_mut(&mut api).recostings.truncate(1);
    assert!(serialize(&api).is_err());
}
