//! Tests of the front against mainline's captured requests, and of its behavior in general.

#![allow(
    clippy::float_cmp,
    clippy::too_many_lines,
    reason = "the front must reproduce mainline's values exactly, and tests read best whole"
)]

use super::*;
use crate::test_support::{Capture, captures};
use ::http::Method;
use alloc::string::String;
use alloc::vec::Vec;
use serde_json::Value;
use valhalla_proto::costing::options::{HasFlowMask, HasIgnoreClosures, HasUseTolls};
use valhalla_proto::costing::{HasFilterClosures, HasName, HasOptions, Type as CostingType};
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::location::{HasHeading, HasTime, PreferredSide, Type as LocationType};
use valhalla_proto::location::{
    HasHeadingTolerance, HasNodeSnapTolerance, HasRadius, HasSearchCutoff, HasStreetSideCutoff,
    HasStreetSideMaxDistance, HasStreetSideTolerance,
};
use valhalla_proto::options::{
    DateTimeType, Format, HasAlternates, HasDateTime, HasDenoise, HasExpansionAction,
    HasGeneralize, HasGpsAccuracy, HasId, HasLanguage, HasMatrixLocations, HasResampleDistance,
    HasRoundaboutExits, HasShapeFormat, HasVerbose, ReverseTimeTracking, Units,
};
use valhalla_proto::prost::Message;
use valhalla_proto::search_filter::{
    HasExcludeClosures, HasLevel, HasMaxRoadClass, HasMinRoadClass,
};
use valhalla_proto::{
    Correlation, DirectionsType, FilterAction, LatLng, Location, Options, RoadClass, ShapeFormat,
};

/// Sends a request to the front the way mainline's server would hand it to Loki.
fn request(
    method: &Method,
    target: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> (Api, Result<(), ValhallaError>) {
    let mut api = Api::default();
    let result = parse_http_request(
        &HttpRequest {
            method,
            target: target.as_bytes(),
            content_type: content_type.map(str::as_bytes),
            body,
        },
        &mut api,
    );
    (api, result)
}

fn post(action: &str, body: &str) -> (Api, Result<(), ValhallaError>) {
    request(&Method::POST, &format!("/{action}"), None, body.as_bytes())
}

fn parse_options(action: &str, body: &str) -> Options {
    let (api, result) = post(action, body);
    result.unwrap_or_else(|e| panic!("{body}: {e}"));
    api.options.expect("options")
}

fn error(action: &str, body: &str) -> ValhallaError {
    post(action, body).1.expect_err(body)
}

fn warnings(action: &str, body: &str) -> Vec<(u64, String)> {
    let (api, result) = post(action, body);
    result.unwrap_or_else(|e| panic!("{body}: {e}"));
    api.info
        .expect("info")
        .warnings
        .into_iter()
        .map(|w| (w.code, w.description))
        .collect()
}

/// Percent-encodes everything but unreserved characters, as a client would for `?json=`.
fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Removes from a Loki→Thor location what Loki (not the front) sets,
/// given the location the front produced.
///
/// Loki's `parse_location` (`src/loki/worker.cc`) fills in these defaults when they are unset:
/// `radius`, `heading_tolerance`, `node_snap_tolerance`, `search_cutoff`,
/// `street_side_tolerance`, `street_side_cutoff`, and `street_side_max_distance`.
/// It always sets `minimum_inbound_reachability` and `minimum_outbound_reachability`
/// (from `minimum_reachability` or its configured default,
/// and the route action zeroes the origin's inbound and the destination's outbound),
/// and its search fills in the `correlation` (everything but `original_index`, which the front sets).
/// The search filter defaults it would fill in are already set by the front, so they stay.
fn strip_loki_location(reference: &mut Location, front: &Location) {
    if front.has_radius.is_none() {
        reference.has_radius = None::<HasRadius>;
    }
    if front.has_heading_tolerance.is_none() {
        reference.has_heading_tolerance = None::<HasHeadingTolerance>;
    }
    if front.has_node_snap_tolerance.is_none() {
        reference.has_node_snap_tolerance = None::<HasNodeSnapTolerance>;
    }
    if front.has_search_cutoff.is_none() {
        reference.has_search_cutoff = None::<HasSearchCutoff>;
    }
    if front.has_street_side_tolerance.is_none() {
        reference.has_street_side_tolerance = None::<HasStreetSideTolerance>;
    }
    if front.has_street_side_cutoff.is_none() {
        reference.has_street_side_cutoff = None::<HasStreetSideCutoff>;
    }
    if front.has_street_side_max_distance.is_none() {
        reference.has_street_side_max_distance = None::<HasStreetSideMaxDistance>;
    }
    reference.has_minimum_inbound_reachability = None;
    reference.has_minimum_outbound_reachability = None;
    if let Some(correlation) = reference.correlation.as_mut() {
        *correlation = Correlation {
            original_index: correlation.original_index,
            ..Correlation::default()
        };
    }
}

/// Removes from Loki→Thor options what Loki (not the front) sets, given the front's options.
///
/// Besides the per-location fields ([`strip_loki_location`]),
/// Loki caps `alternates` at its configured `max_alternates`,
/// adds `exclude_edges` to the costing for `exclude_locations` and `exclude_polygons`,
/// and may turn off `disable_hierarchy_pruning` (warning 205).
/// No capture turns off hierarchy pruning, so that is not allowed for here.
/// The reference may have fewer alternates than the front asked for, never more,
/// and Loki's exclude edges come after any the front parsed (from a PBF request).
fn strip_loki_options(reference: &mut Options, front: &Options) {
    for (key, costing) in &mut reference.costings {
        let fronts = match front.costings.get(key).and_then(|c| c.has_options.as_ref()) {
            Some(HasOptions::Options(options)) => options.exclude_edges.as_slice(),
            None => &[],
        };
        if let Some(HasOptions::Options(options)) = costing.has_options.as_mut() {
            assert!(
                options.exclude_edges.starts_with(fronts),
                "Loki keeps the front's exclude edges"
            );
            options.exclude_edges.truncate(fronts.len());
        }
    }
    for (references, fronts) in [
        (&mut reference.locations, &front.locations),
        (&mut reference.sources, &front.sources),
        (&mut reference.targets, &front.targets),
        (&mut reference.exclude_locations, &front.exclude_locations),
    ] {
        assert_eq!(references.len(), fronts.len(), "location count");
        for (reference, front) in references.iter_mut().zip(fronts) {
            strip_loki_location(reference, front);
        }
    }
    if let (
        Some(valhalla_proto::options::HasAlternates::Alternates(reference)),
        Some(valhalla_proto::options::HasAlternates::Alternates(front)),
    ) = (reference.has_alternates.as_mut(), front.has_alternates)
        && *reference < front
    {
        *reference = front;
    }
}

fn front_options(capture: &Capture) -> (Api, Result<(), ValhallaError>) {
    if let Some(http) = capture.http() {
        let method = Method::from_bytes(http.method.as_bytes()).expect("valid method");
        return request(
            &method,
            &http.target,
            http.content_type.as_deref(),
            &http.body,
        );
    }
    let (action, body) = capture.request();
    let body = serde_json::to_string(&body).expect("serializable");
    post(&action, &body)
}

/// Every captured request that reached Thor parses into the options mainline sent it,
/// apart from what Loki adds.
#[test]
fn options_match_mainline() {
    let mut checked = 0;
    for format in ["json", "osrm", "pbf"] {
        for capture in captures(format) {
            let Some(mut reference) = capture.loki_thor() else {
                continue;
            };
            let (api, result) = front_options(&capture);
            result.unwrap_or_else(|e| panic!("{}: {e}", capture.name()));
            let front = api.options.expect("options");
            let mut reference_options = reference.options.take().expect("reference options");
            strip_loki_options(&mut reference_options, &front);
            assert_eq!(front, reference_options, "{}", capture.name());

            // Loki adds its statistics (and could add warnings of its own, but none here).
            let reference_info = reference.info.take().unwrap_or_default();
            let info = api.info.expect("info");
            assert_eq!(info.warnings, reference_info.warnings, "{}", capture.name());
            assert!(info.errors.is_empty() && info.statistics.is_empty());
            assert!(!info.is_service, "mainline clears the service flag again");
            checked += 1;
        }
    }
    assert!(checked >= 50, "only {checked} captures checked");
}

/// A GET request with `?json=` gives the same options as the POST body.
#[test]
fn get_requests_match_post() {
    // Cases with an exact HTTP request are not a JSON body to resend.
    for capture in captures("json").into_iter().filter(|c| c.http().is_none()) {
        let (action, body) = capture.request();
        let body = serde_json::to_string(&body).expect("serializable");
        let (post_api, post_result) = post(&action, &body);
        let target = format!("/{action}?json={}", percent_encode(&body));
        let (get_api, get_result) = request(&Method::GET, &target, None, b"");
        assert_eq!(post_result, get_result, "{}", capture.name());
        assert_eq!(post_api, get_api, "{}", capture.name());
    }
}

/// The captured error cases fail the way mainline did, or (for Loki's errors) parse fine.
#[test]
fn captured_errors() {
    for format in ["json", "osrm", "pbf"] {
        for capture in captures(format) {
            let name = capture.name();
            let (api, result) = front_options(&capture);
            if capture.loki_thor().is_some() {
                assert_eq!(result, Ok(()), "{name}");
                continue;
            }
            // Mainline failed before Thor; its Valhalla JSON error says with what.
            // Exact-request cases capture only one format, which is JSON for every error case.
            let json = Capture {
                case: capture.case.clone(),
                format: "json".to_string(),
            };
            let (_, body) = json.json_response();
            // Loki answers `/locate` itself, so that succeeds without reaching Thor.
            let Some(code) = body.get("error_code").and_then(Value::as_u64) else {
                assert_eq!(result, Ok(()), "{name}");
                continue;
            };
            let message = body["error"].as_str().expect("an error message");
            match result {
                Err(error) => {
                    assert_eq!(u64::from(error.code().code()), code, "{name}");
                    assert_eq!(error.message(), message, "{name}");
                    if capture.http().is_none() {
                        // The format is already parsed, so the error is serialized in it.
                        let parsed_format = api.options.as_ref().map(|o| o.format);
                        let expected = match format {
                            "osrm" => Format::Osrm,
                            "pbf" => Format::Pbf,
                            _ => Format::Json,
                        };
                        assert_eq!(parsed_format, Some(expected as i32), "{name}");
                    }
                }
                // These are Loki's errors: an action it does not offer (106), too few
                // locations (120), no costing (124), too many locations (150),
                // and no data near a location (171).
                Ok(()) => assert!(
                    [106, 120, 124, 150, 171].contains(&code),
                    "{name}: the front accepted a request mainline refused with {code}"
                ),
            }
        }
    }
}

/// Parsing a PBF request made from the front's own output changes nothing:
/// the PBF path validates and keeps what is already there.
///
/// The one exception is `ignore_closures`: the front always sets it in the costing options,
/// and a PBF request that sets it as well as a location's `exclude_closures` is refused (143),
/// as it is in mainline. So we leave it out of the request, as a client would.
#[test]
fn pbf_requests_round_trip() {
    for capture in captures("json") {
        let (api, result) = front_options(&capture);
        if result.is_err() {
            continue;
        }
        let path = format!("/{}", capture.request().0);
        let mut options = api.options.clone().expect("options");
        let mut had_ignore_closures = false;
        let mut ignores_closures = false;
        for costing in options.costings.values_mut() {
            if let Some(HasOptions::Options(costing_options)) = costing.has_options.as_mut() {
                let ignore = costing_options.has_ignore_closures.take();
                had_ignore_closures |= ignore.is_some();
                ignores_closures |= ignore == Some(HasIgnoreClosures::IgnoreClosures(true));
            }
        }
        // A request that ignores closures cannot round-trip:
        // without `ignore_closures` its locations would exclude closures again,
        // and with it, the locations' `exclude_closures` conflict (143).
        if ignores_closures {
            continue;
        }
        let body = Api {
            options: Some(options.clone()),
            ..Api::default()
        }
        .encode_to_vec();
        let (pbf_api, pbf_result) = request(&Method::POST, &path, Some(PBF_CONTENT_TYPE), &body);
        assert_eq!(pbf_result, Ok(()), "{}", capture.name());
        assert_eq!(pbf_api.options, api.options, "{}", capture.name());

        // With both set, mainline refuses the request.
        // (Without a costing, as in a request mainline refuses with 124, there is nothing to set.)
        if !had_ignore_closures {
            continue;
        }
        let body = Api {
            options: api.options.clone(),
            ..Api::default()
        }
        .encode_to_vec();
        let (_, pbf_result) = request(&Method::POST, &path, Some(PBF_CONTENT_TYPE), &body);
        assert_eq!(
            pbf_result,
            Err(ErrorCode::ClosuresConflict.into()),
            "{}",
            capture.name()
        );
    }
}

const ROUTE: &str = r#""locations": [{"lat": 42.5, "lon": 1.5}, {"lat": 42.6, "lon": 1.6}]"#;

fn route(extra: &str) -> String {
    if extra.is_empty() {
        format!("{{{ROUTE}, \"costing\": \"auto\"}}")
    } else {
        format!("{{{ROUTE}, \"costing\": \"auto\", {extra}}}")
    }
}

#[test]
fn http_errors() {
    for method in [Method::PUT, Method::DELETE, Method::HEAD, Method::OPTIONS] {
        let (_, result) = request(&method, "/route", None, route("").as_bytes());
        assert_eq!(result, Err(ErrorCode::WrongHttpMethod.into()), "{method}");
    }
    assert_eq!(error("route", "{"), ErrorCode::JsonParseFailed.into());
    assert_eq!(error("route", "  "), ErrorCode::JsonParseFailed.into());
    let (_, result) = request(&Method::GET, "/route?json=%7B", None, b"");
    assert_eq!(result, Err(ErrorCode::JsonParseFailed.into()));
    let (_, result) = request(&Method::POST, "/route", Some(PBF_CONTENT_TYPE), b"\xff\xff");
    assert_eq!(result, Err(ErrorCode::PbfParseFailed.into()));
    // A content type other than exactly the PBF one means JSON.
    let (_, result) = request(
        &Method::POST,
        "/route",
        Some("application/x-protobuf; charset=utf-8"),
        b"\xff",
    );
    assert_eq!(result, Err(ErrorCode::JsonParseFailed.into()));
}

#[test]
fn http_actions_and_query_parameters() {
    // The action comes from the path; an unknown one leaves no action (Loki then rejects it).
    let (api, _) = request(&Method::POST, "/locate", None, b"{}");
    assert_eq!(api.options.map(|o| o.action), Some(Action::Locate as i32));
    let (api, _) = request(&Method::POST, "/nope", None, b"{}");
    assert_eq!(api.options.map(|o| o.action), Some(Action::NoAction as i32));

    // Query parameters are added as strings, after (so losing to) the JSON's members.
    let json = percent_encode(&route(r#""units": "km""#));
    let (api, result) = request(
        &Method::GET,
        &format!("/route?json={json}&costing=bicycle&units=miles&alternates=2&verbose=true"),
        None,
        b"",
    );
    assert_eq!(result, Ok(()));
    let options = api.options.expect("options");
    assert_eq!(options.costing_type, CostingType::Auto as i32);
    assert_eq!(options.units, Units::Kilometers as i32);
    assert_eq!(options.has_alternates, Some(HasAlternates::Alternates(2)));
    // `"true"` is not a boolean to boost's lexical_cast, so this keeps the default.
    assert_eq!(options.has_verbose, Some(HasVerbose::Verbose(false)));

    // A query parameter can stand in for a JSON member, and `"1"` is true.
    let (api, _) = request(
        &Method::POST,
        "/route?costing=pedestrian&verbose=1",
        None,
        format!("{{{ROUTE}}}").as_bytes(),
    );
    let options = api.options.expect("options");
    assert_eq!(options.costing_type, CostingType::Pedestrian as i32);
    assert_eq!(options.has_verbose, Some(HasVerbose::Verbose(true)));

    // `?json=` wins over the body, and the target is decoded before it is split,
    // so an encoded `&` splits the JSON.
    let (api, result) = request(
        &Method::POST,
        &format!("/route?json={}", percent_encode(&route(""))),
        None,
        b"not json",
    );
    assert_eq!(result, Ok(()));
    assert_eq!(api.options.map(|o| o.locations.len()), Some(2));
    let (_, result) = request(
        &Method::GET,
        &format!("/route?json={}", percent_encode(r#"{"id": "a&b"}"#)),
        None,
        b"",
    );
    assert_eq!(result, Err(ErrorCode::JsonParseFailed.into()));

    // Repeated parameters become arrays (here an array of strings, which a `costing` is not).
    let (api, _) = request(
        &Method::POST,
        "/route?costing=auto&costing=bicycle",
        None,
        b"",
    );
    assert_eq!(
        api.options.map(|o| o.costing_type),
        Some(CostingType::None as i32)
    );
}

#[test]
fn non_object_documents() {
    // Adding query parameters to a document that is not an object fails rapidjson's assertion.
    let (_, result) = request(&Method::POST, "/route?costing=auto", None, b"[1]");
    assert_eq!(
        result,
        Err(ValhallaError::with_detail(
            ErrorCode::LokiUnknown,
            "IsObject()"
        ))
    );
    // Without them, mainline gets as far as looking for `avoid_locations`.
    assert_eq!(
        error("route", "5"),
        ValhallaError::with_detail(ErrorCode::LokiUnknown, "IsObject()")
    );
    // A tile request returns before that.
    assert_eq!(
        error("tile", "[]"),
        ErrorCode::InvalidTileCoordinates.into()
    );
}

#[test]
fn defaults() {
    let options = parse_options("route", &route(""));
    assert_eq!(
        options.has_language,
        Some(HasLanguage::Language("en-US".into()))
    );
    assert_eq!(options.format, Format::Json as i32);
    assert_eq!(options.units, Units::Kilometers as i32);
    assert_eq!(options.has_verbose, Some(HasVerbose::Verbose(false)));
    assert_eq!(options.has_alternates, Some(HasAlternates::Alternates(0)));
    assert_eq!(options.has_denoise, Some(HasDenoise::Denoise(1.0)));
    assert_eq!(
        options.has_roundabout_exits,
        Some(HasRoundaboutExits::RoundaboutExits(true))
    );
    assert_eq!(
        options.has_matrix_locations,
        Some(HasMatrixLocations::MatrixLocations(u32::MAX))
    );
    assert_eq!(options.has_shape_format, None);
    // Without a time, the predicted and live speeds are off.
    let costing = &options.costings[&(CostingType::Auto as i32)];
    let Some(HasOptions::Options(costing_options)) = &costing.has_options else {
        panic!("costing options");
    };
    assert_eq!(
        costing_options.has_flow_mask,
        Some(HasFlowMask::FlowMask(3))
    );
    assert_eq!(
        costing.has_filter_closures,
        Some(HasFilterClosures::FilterClosures(true))
    );

    // Matrix responses are verbose and shapeless by default.
    let options = parse_options(
        "sources_to_targets",
        r#"{"sources": [{"lat": 1, "lon": 1}], "targets": [{"lat": 2, "lon": 2}], "costing": "auto"}"#,
    );
    assert_eq!(options.has_verbose, Some(HasVerbose::Verbose(true)));
    assert_eq!(
        options.has_shape_format,
        Some(HasShapeFormat::ShapeFormat(ShapeFormat::NoShape as i32))
    );

    // /locate needs no costing.
    let options = parse_options("locate", r#"{"locations": [{"lat": 1, "lon": 1}]}"#);
    assert_eq!(options.costing_type, CostingType::None as i32);
    assert_eq!(
        options.costings[&(CostingType::None as i32)].has_name,
        Some(HasName::Name("none".into()))
    );
}

#[test]
fn locations() {
    let options = parse_options(
        "route",
        r#"{"costing": "auto", "locations": [
            {"lat": 1, "lon": 541, "type": "via", "heading": -10, "name": "a", "waiting": 5},
            {"lat": "2", "lon": -200.5, "type": "through", "waiting": 30, "radius": 7,
             "node_snap_tolerance": 2.9, "rank_candidates": false, "preferred_side": "same",
             "display_lat": 3, "display_lon": 4, "search_filter": {"min_road_class": "bogus",
             "max_road_class": "primary", "exclude_ramp": true, "level": 2}},
            {"lat": 3, "lon": 3, "type": "spaceship", "street_side_cutoff": "residential",
             "display_lat": 100, "display_lon": 4}
        ]}"#,
    );
    let [first, second, third] = options.locations.as_slice() else {
        panic!("three locations");
    };
    // The first and last are always breaks; an unknown type is a break too.
    assert_eq!(first.r#type, LocationType::KBreak as i32);
    assert_eq!(second.r#type, LocationType::KThrough as i32);
    assert_eq!(third.r#type, LocationType::KBreak as i32);
    // Longitudes wrap around.
    assert_eq!(
        first.ll,
        Some(LatLng {
            has_lat: Some(HasLat::Lat(1.0)),
            has_lng: Some(HasLng::Lng(-179.0)),
        })
    );
    assert_eq!(
        second.ll,
        Some(LatLng {
            has_lat: Some(HasLat::Lat(2.0)),
            has_lng: Some(HasLng::Lng(159.5)),
        })
    );
    // A negative heading wraps around in the unsigned field.
    assert_eq!(first.has_heading, Some(HasHeading::Heading(4_294_967_286)));
    assert_eq!(first.has_time, Some(HasTime::Time(-1.0)));
    // Origins never wait; through locations warn about waiting instead.
    assert_eq!(first.waiting_secs, 0.0);
    assert_eq!(second.waiting_secs, 0.0);
    assert_eq!(
        second.has_node_snap_tolerance,
        Some(HasNodeSnapTolerance::NodeSnapTolerance(2))
    );
    assert!(second.skip_ranking_candidates);
    assert_eq!(second.preferred_side, PreferredSide::Same as i32);
    let filter = second.search_filter.expect("filter");
    assert_eq!(
        filter.has_min_road_class,
        Some(HasMinRoadClass::MinRoadClass(
            RoadClass::KServiceOther as i32
        ))
    );
    assert_eq!(
        filter.has_max_road_class,
        Some(HasMaxRoadClass::MaxRoadClass(RoadClass::KPrimary as i32))
    );
    assert!(filter.exclude_ramp);
    assert_eq!(filter.has_level, Some(HasLevel::Level(2.0)));
    assert_eq!(
        filter.has_exclude_closures,
        Some(HasExcludeClosures::ExcludeClosures(true))
    );
    assert!(second.display_ll.is_some());
    // An invalid display location is dropped.
    assert_eq!(third.display_ll, None);
    assert_eq!(
        third.has_street_side_cutoff,
        Some(HasStreetSideCutoff::StreetSideCutoff(
            RoadClass::KResidential as i32
        ))
    );
    assert_eq!(
        options
            .locations
            .iter()
            .map(|l| l.correlation.as_ref().map(|c| c.original_index))
            .collect::<Vec<_>>(),
        [Some(0), Some(1), Some(2)]
    );
    // Three locations mean no alternates.
    assert_eq!(options.has_alternates, Some(HasAlternates::Alternates(0)));
}

#[test]
fn waiting_and_closures() {
    let body = r#"{"costing": "auto", "locations": [
        {"lat": 1, "lon": 1, "waiting": 5},
        {"lat": 2, "lon": 2, "type": "break", "waiting": 30},
        {"lat": 3, "lon": 3, "type": "via", "waiting": 30, "search_filter": {"exclude_closures": false}},
        {"lat": 4, "lon": 4, "waiting": 5}
    ]}"#;
    let options = parse_options("route", body);
    let waits: Vec<f32> = options.locations.iter().map(|l| l.waiting_secs).collect();
    assert_eq!(waits, [0.0, 30.0, 0.0, 0.0]);
    // One location lets closures through, so the costing must too.
    assert_eq!(
        options.costings[&(CostingType::Auto as i32)].has_filter_closures,
        Some(HasFilterClosures::FilterClosures(false))
    );
    assert_eq!(
        warnings("route", body),
        [(
            203,
            r#""waiting_time" is set on a location of type "via" or "through", ignoring waiting_time"#
                .into()
        )]
    );

    // `ignore_closures` decides for every location, but not together with `exclude_closures`.
    let options = parse_options(
        "route",
        &route(r#""costing_options": {"auto": {"ignore_closures": true}}"#),
    );
    assert!(
        options
            .locations
            .iter()
            .all(|l| l.search_filter.and_then(|f| f.has_exclude_closures)
                == Some(HasExcludeClosures::ExcludeClosures(false)))
    );
    assert_eq!(
        error(
            "route",
            r#"{"costing": "auto", "costing_options": {"auto": {"ignore_closures": false}},
                "locations": [{"lat": 1, "lon": 1, "search_filter": {"exclude_closures": true}},
                              {"lat": 2, "lon": 2}]}"#
        ),
        ErrorCode::ClosuresConflict.into()
    );
}

#[test]
fn location_errors() {
    for (body, expected) in [
        (
            r#"{"locations": [{"lon": 1}]}"#,
            ErrorCode::LocationParseFailed,
        ),
        (
            r#"{"locations": [{"lat": 91, "lon": 1}]}"#,
            ErrorCode::LocationParseFailed,
        ),
        (
            r#"{"locations": [{"lat": "x", "lon": 1}]}"#,
            ErrorCode::LocationParseFailed,
        ),
        (r#"{"locations": [5]}"#, ErrorCode::LocationParseFailed),
        (r#"{"sources": [{"lat": 1}]}"#, ErrorCode::SourceParseFailed),
        (r#"{"targets": [{"lat": 1}]}"#, ErrorCode::TargetParseFailed),
        (
            r#"{"exclude_locations": [{"lat": 1}]}"#,
            ErrorCode::AvoidParseFailed,
        ),
        (
            r#"{"avoid_locations": [{"lat": 1}]}"#,
            ErrorCode::AvoidParseFailed,
        ),
        (r#"{"shape": [{"lat": 1}]}"#, ErrorCode::ShapeParseFailed),
        (r#"{"trace": [{"lat": 1}]}"#, ErrorCode::TraceParseFailed),
    ] {
        assert_eq!(error("route", body), expected.into(), "{body}");
    }
    // Not an array: ignored.
    assert_eq!(
        parse_options("route", r#"{"locations": {"lat": 1}}"#)
            .locations
            .len(),
        0
    );
}

#[test]
fn costing_errors_and_aliases() {
    assert_eq!(
        error("route", &format!("{{{ROUTE}, \"costing\": \"spaceship\"}}")),
        ValhallaError::with_detail(ErrorCode::UnknownCosting, "'spaceship'")
    );
    // A costing that is not a string means none.
    assert_eq!(
        parse_options("route", &format!("{{{ROUTE}, \"costing\": 5}}")).costing_type,
        CostingType::None as i32
    );

    let body = format!(
        "{{{ROUTE}, \"costing\": \"auto_shorter\", \"costing_options\": {{\"auto_shorter\": {{\"use_tolls\": 0.25}}}}}}"
    );
    let options = parse_options("route", &body);
    assert_eq!(options.costing_type, CostingType::Auto as i32);
    let auto = auto_options(&options);
    assert!(auto.shortest);
    assert_eq!(auto.has_use_tolls, Some(HasUseTolls::UseTolls(0.25)));
    assert_eq!(warnings("route", &body)[0].0, 100);

    let body = format!("{{{ROUTE}, \"costing\": \"hov\"}}");
    assert!(auto_options(&parse_options("route", &body)).include_hov2);
    assert_eq!(warnings("route", &body)[0].0, 101);

    let body = format!("{{{ROUTE}, \"costing\": \"auto_data_fix\"}}");
    let options = parse_options("route", &body);
    let auto = auto_options(&options);
    assert!(auto.ignore_restrictions && auto.ignore_oneways && auto.ignore_access);
    assert_eq!(
        auto.has_ignore_closures,
        Some(HasIgnoreClosures::IgnoreClosures(true))
    );
    // The rewritten document's `ignore_closures` applies to the locations too.
    assert!(
        options
            .locations
            .iter()
            .all(|l| l.search_filter.and_then(|f| f.has_exclude_closures)
                == Some(HasExcludeClosures::ExcludeClosures(false)))
    );
    assert_eq!(warnings("route", &body)[0].0, 102);
}

fn auto_options(options: &Options) -> valhalla_proto::costing::Options {
    match &options.costings[&(CostingType::Auto as i32)].has_options {
        Some(HasOptions::Options(options)) => options.clone(),
        None => panic!("auto options"),
    }
}

#[test]
fn deprecated_and_top_level_options() {
    // `directions_options` members move up, unless the top level has them already.
    let options = parse_options(
        "route",
        &route(
            r#""units": "km", "directions_options": {"units": "miles", "language": "de-DE", "narrative": false, "format": "osrm"}"#,
        ),
    );
    assert_eq!(options.units, Units::Kilometers as i32);
    assert_eq!(
        options.has_language,
        Some(HasLanguage::Language("de-DE".into()))
    );
    assert_eq!(options.directions_type, DirectionsType::None as i32);
    assert_eq!(options.format, Format::Osrm as i32);

    assert_eq!(
        parse_options("route", &route(r#""units": "mi""#)).units,
        Units::Miles as i32
    );
    for (language, expected) in [
        ("en-x-pirate", "en-x-pirate"),
        ("pt-BR", "pt-BR"),
        ("xx", "en-US"),
        ("EN-us", "en-US"),
    ] {
        assert_eq!(
            parse_options("route", &route(&format!("\"language\": \"{language}\""))).has_language,
            Some(HasLanguage::Language(expected.into())),
            "{language}"
        );
    }
    let options = parse_options(
        "route",
        &route(
            r#""directions_type": "maneuvers", "id": "x", "banner_instructions": true, "voice_instructions": 1, "alternates": 3, "roundabout_exits": false, "linear_references": true, "admin_crossings": true, "turn_lanes": true, "elevation_interval": 5000, "denoise": -1, "generalize": 0, "filters": {"action": "include", "attributes": ["edge.tagged_names"]}, "reverse_time_tracking": "disabled", "prioritize_bidirectional": true"#,
        ),
    );
    assert_eq!(options.directions_type, DirectionsType::Maneuvers as i32);
    assert_eq!(options.has_id, Some(HasId::Id("x".into())));
    assert!(options.banner_instructions && options.voice_instructions && options.turn_lanes);
    assert!(
        options.linear_references && options.admin_crossings && options.prioritize_bidirectional
    );
    assert_eq!(options.has_alternates, Some(HasAlternates::Alternates(3)));
    assert_eq!(
        options.has_roundabout_exits,
        Some(HasRoundaboutExits::RoundaboutExits(false))
    );
    assert_eq!(options.elevation_interval, 1000.0);
    assert_eq!(options.has_denoise, Some(HasDenoise::Denoise(0.0)));
    assert_eq!(options.has_generalize, Some(HasGeneralize::Generalize(0.0)));
    assert_eq!(options.filter_action, FilterAction::Include as i32);
    assert_eq!(options.filter_attributes, ["edge.tagged_values"]);
    assert_eq!(
        options.reverse_time_tracking,
        ReverseTimeTracking::RttDisabled as i32
    );

    // Deprecated and unsupported things warn.
    assert_eq!(
        warnings("route", &route(r#""jsonp": "f", "best_paths": 2"#)),
        [
            (104, "jsonp has been deprecated, use CORS instead".into()),
            (
                103,
                r#"best_paths has been deprecated, use "alternates" instead"#.into()
            ),
        ]
    );
    assert_eq!(
        parse_options("route", &route(r#""best_paths": 2"#)).has_alternates,
        Some(HasAlternates::Alternates(1))
    );
    let (api, result) = post(
        "locate",
        r#"{"locations": [{"lat": 1, "lon": 1}], "format": "pbf"}"#,
    );
    assert_eq!(result, Ok(()));
    assert_eq!(api.options.map(|o| o.format), Some(Format::Json as i32));
    assert_eq!(api.info.expect("info").warnings[0].code, 211);
    assert_eq!(
        error("route", &route(r#""filters": {"attributes": [1]}"#)),
        ValhallaError::with_detail(ErrorCode::LokiUnknown, "IsString()")
    );
}

#[test]
fn date_times() {
    let with = |date_time: &str| route(&format!("\"date_time\": {date_time}"));
    let options = parse_options("route", &with(r#"{"type": 0}"#));
    assert_eq!(options.date_time_type, DateTimeType::Current as i32);
    assert_eq!(
        options.has_date_time,
        Some(HasDateTime::DateTime("current".into()))
    );
    assert_eq!(options.locations[0].date_time, "current");
    assert_eq!(options.locations[1].date_time, "");
    // With a time, predicted and live speeds stay on.
    assert_eq!(
        auto_options(&options).has_flow_mask,
        Some(HasFlowMask::FlowMask(15))
    );

    let options = parse_options(
        "route",
        &with(r#"{"type": 2, "value": "2026-11-09T18:00"}"#),
    );
    assert_eq!(options.date_time_type, DateTimeType::ArriveBy as i32);
    assert_eq!(
        options
            .locations
            .iter()
            .map(|l| l.date_time.as_str())
            .collect::<Vec<_>>(),
        ["", "2026-11-09T18:00"]
    );
    let options = parse_options(
        "route",
        &with(r#"{"type": 3, "value": "2026-11-09T18:00"}"#),
    );
    assert!(
        options
            .locations
            .iter()
            .all(|l| l.date_time == "2026-11-09T18:00")
    );
    // A location's own date and time wins.
    let options = parse_options(
        "route",
        r#"{"costing": "auto", "date_time": {"type": 1, "value": "2026-11-09T08:00"},
            "locations": [{"lat": 1, "lon": 1}, {"lat": 2, "lon": 2, "date_time": "x"}]}"#,
    );
    assert_eq!(
        options
            .locations
            .iter()
            .map(|l| l.date_time.as_str())
            .collect::<Vec<_>>(),
        ["", "x"]
    );
    // An unknown type is ignored.
    assert_eq!(
        parse_options("route", &with(r#"{"type": 7}"#)).date_time_type,
        DateTimeType::NoTime as i32
    );

    for (date_time, expected) in [
        (r#"{"type": 1}"#, ErrorCode::MissingDepartAtDate),
        (r#"{"type": 2}"#, ErrorCode::MissingArriveByDate),
        (
            r#"{"type": 3, "value": ""}"#,
            ErrorCode::MissingInvariantDate,
        ),
        (
            r#"{"type": 1, "value": "2026-11-09 08:00"}"#,
            ErrorCode::DateParseFailed,
        ),
        (
            r#"{"type": 1, "value": "1900-11-09T08:00"}"#,
            ErrorCode::DateParseFailed,
        ),
    ] {
        assert_eq!(
            error("route", &with(date_time)),
            expected.into(),
            "{date_time}"
        );
    }
    assert_eq!(
        error(
            "route",
            &format!(
                "{{{ROUTE}, \"costing\": \"multimodal\", \"date_time\": {{\"type\": 2, \"value\": \"2026-11-09T08:00\"}}}}"
            )
        ),
        ErrorCode::MultimodalArriveByNotImplemented.into()
    );
    assert_eq!(
        error(
            "isochrone",
            &with(r#"{"type": 3, "value": "2026-11-09T08:00"}"#)
        ),
        ErrorCode::IsochroneArriveByNotImplemented.into()
    );
    // Transit without a time leaves now.
    let options = parse_options("route", &format!("{{{ROUTE}, \"costing\": \"transit\"}}"));
    assert_eq!(options.date_time_type, DateTimeType::Current as i32);

    // Only a PBF request can carry an out-of-range type.
    let body = Api {
        options: Some(Options {
            date_time_type: 7,
            ..Options::default()
        }),
        ..Api::default()
    }
    .encode_to_vec();
    let (_, result) = request(&Method::POST, "/route", Some(PBF_CONTENT_TYPE), &body);
    assert_eq!(result, Err(ErrorCode::InvalidDateType.into()));
}

#[test]
fn shapes() {
    assert_eq!(
        error("route", &route(r#""shape_format": "wkt""#)),
        ErrorCode::InvalidShapeFormat.into()
    );
    assert_eq!(
        error("height", r#"{"shape_format": "no_shape"}"#),
        ErrorCode::InvalidShapeFormat.into()
    );
    assert_eq!(
        parse_options("route", &route(r#""shape_format": "polyline5""#)).has_shape_format,
        Some(HasShapeFormat::ShapeFormat(ShapeFormat::Polyline5 as i32))
    );

    // An encoded polyline becomes the shape: vias between two breaks.
    let options = parse_options(
        "trace_route",
        r#"{"costing": "auto", "encoded_polyline": "_c`|@_c`|@_c`|@_c`|@_c`|@_c`|@"}"#,
    );
    let types: Vec<i32> = options.shape.iter().map(|s| s.r#type).collect();
    assert_eq!(
        types,
        [
            LocationType::KBreak as i32,
            LocationType::KVia as i32,
            LocationType::KBreak as i32
        ]
    );
    assert_eq!(
        options.shape[1].ll,
        Some(LatLng {
            has_lat: Some(HasLat::Lat(2.0)),
            has_lng: Some(HasLng::Lng(2.0)),
        })
    );
    assert_eq!(
        error("trace_route", r#"{"encoded_polyline": "_c"}"#),
        ValhallaError::with_detail(ErrorCode::LokiUnknown, "Bad encoded polyline")
    );

    // Trace routes default to vias; durations set the times.
    let options = parse_options(
        "trace_route",
        r#"{"costing": "auto", "begin_time": 100, "durations": [5, 2.5],
            "shape": [{"lat": 1, "lon": 1}, {"lat": 2, "lon": 2}, {"lat": 3, "lon": 3}]}"#,
    );
    let times: Vec<_> = options.shape.iter().map(|s| s.has_time).collect();
    assert_eq!(
        times,
        [
            Some(HasTime::Time(100.0)),
            Some(HasTime::Time(105.0)),
            Some(HasTime::Time(107.5))
        ]
    );
    assert_eq!(options.shape[1].r#type, LocationType::KVia as i32);
    assert_eq!(
        error(
            "trace_route",
            r#"{"durations": [5], "shape": [{"lat": 1, "lon": 1}, {"lat": 2, "lon": 2}, {"lat": 3, "lon": 3}]}"#
        ),
        ErrorCode::TraceDurationMismatch.into()
    );
    assert_eq!(
        error("trace_route", r#"{"use_timestamps": true}"#),
        ErrorCode::MissingTimestamps.into()
    );
    // `trace` is read only without a `shape`.
    let options = parse_options(
        "trace_route",
        r#"{"trace": [{"lat": 1, "lon": 1}, {"lat": 2, "lon": 2}]}"#,
    );
    assert_eq!(options.trace.len(), 2);
    assert_eq!(
        error("trace_route", r#"{"shape_match": "teleport"}"#),
        ErrorCode::InvalidShapeMatch.into()
    );
}

#[test]
fn polygons_and_linear_features() {
    let options = parse_options(
        "route",
        &route(
            r#""exclude_polygons": [[[1, 1], [2, 2], [361, 3]], [], {"type": "Feature", "geometry": {"type": "Polygon", "coordinates": [[[1, 1], [2, 2]]]}, "properties": {"levels": [1, 2.0]}}]"#,
        ),
    );
    assert_eq!(options.exclude_polygons.len(), 2);
    assert_eq!(
        options.exclude_polygons[0].coords[2].has_lng,
        Some(HasLng::Lng(1.0))
    );
    // Only doubles count as levels (rapidjson's `IsFloat`).
    assert_eq!(
        options
            .exclude_levels
            .iter()
            .map(|l| l.levels.clone())
            .collect::<Vec<_>>(),
        [vec![], vec![2.0]]
    );
    let geojson = parse_options(
        "route",
        &route(
            r#""exclude_polygons": {"type": "FeatureCollection", "features": [[[1, 1], [2, 2]]]}"#,
        ),
    );
    assert_eq!(geojson.exclude_polygons.len(), 1);
    assert_eq!(
        warnings("route", &route(r#""exclude_polygons": 5"#))[0].0,
        204
    );
    for (polygons, detail) in [
        (
            r"[[[1]]]",
            "Polygon coordinates must consist of [Lon, Lat] arrays.",
        ),
        (
            r"[[[1, 91]]]",
            "Latitude must be in the range [-90, 90] degrees",
        ),
        (r#"[[["a", 1]]]"#, "IsNumber()"),
        (r"[[5]]", "IsArray()"),
        (
            r#"[{"geometry": {"type": "LineString"}}]"#,
            "Expected object to have coordinates member",
        ),
        (r#"{"type": "Feature"}"#, "IsArray()"),
    ] {
        assert_eq!(
            error(
                "route",
                &route(&format!("\"exclude_polygons\": {polygons}"))
            ),
            ValhallaError::with_detail(ErrorCode::PolygonParseFailed, detail),
            "{polygons}"
        );
    }

    let options = parse_options(
        "route",
        &route(
            r#""linear_cost_factors": [{"shape": "_c`|@_c`|@", "factor": 0.5}, {"type": "Feature", "geometry": {"coordinates": [[1.5, 2.5]]}, "properties": {"ignore_access_restrictions": true}}]"#,
        ),
    );
    let [encoded, geojson] = options.cost_factor_lines.as_slice() else {
        panic!("two lines");
    };
    assert_eq!(encoded.cost_factor, 0.5);
    assert_eq!(encoded.shape.len(), 1);
    assert_eq!(geojson.cost_factor, 1.0);
    assert!(geojson.ignore_access_restrictions);
    assert_eq!(
        warnings("route", &route(r#""linear_cost_factors": {}"#))[0].0,
        212
    );
    for (features, detail) in [
        ("[5]", "IsObject()"),
        (r#"[{"factor": 1}]"#, "false"),
        (
            r#"[{"shape": ""}]"#,
            "Failed to parse line feature: feature coordinates are empty",
        ),
        (r#"[{"shape": "_c"}]"#, "Bad encoded polyline"),
        (
            r#"[{"type": "Feature", "geometry": {"coordinates": [[1]]}}]"#,
            "index < data_.a.size",
        ),
        (
            r#"[{"type": "Feature", "geometry": {"coordinates": [[1, 1]]}}]"#,
            "false",
        ),
    ] {
        assert_eq!(
            error(
                "route",
                &route(&format!("\"linear_cost_factors\": {features}"))
            ),
            ValhallaError::with_detail(ErrorCode::LineFeatureParseFailed, detail),
            "{features}"
        );
    }
}

#[test]
fn other_actions() {
    // Tiles.
    let options = parse_options(
        "tile",
        r#"{"tile": {"x": 1, "y": 1, "z": 1}, "tile_options": {"exclude_layers": ["edges", "roads", 5]}}"#,
    );
    assert_eq!(options.format, Format::Mvt as i32);
    assert_eq!(
        options.tile_options.map(|t| t.exclude_layers),
        Some(vec!["edges".into()])
    );
    assert_eq!(
        warnings(
            "tile",
            r#"{"tile": {"x": 0, "y": 0, "z": 0}, "tile_options": {"exclude_layers": ["roads", 5]}}"#
        ),
        [
            (
                212,
                "Invalid layer name in exclude_layers array: roads".into()
            ),
            (
                212,
                "Invalid layer name in exclude_layers array: not a string".into()
            ),
        ]
    );
    for tile in [
        r#"{"x": 2, "y": 0, "z": 1}"#,
        r#"{"x": 0, "y": 0, "z": 31}"#,
        r#"{"x": 0, "z": 1}"#,
        "{}",
    ] {
        assert_eq!(
            error("tile", &format!("{{\"tile\": {tile}}}")),
            ErrorCode::InvalidTileCoordinates.into(),
            "{tile}"
        );
    }

    // Expansion.
    let options = parse_options(
        "expansion",
        &route(
            r#""action": "route", "expansion_properties": ["cost", "edge_id"], "skip_opposites": true, "dedupe": true"#,
        ),
    );
    assert_eq!(
        options.has_expansion_action,
        Some(HasExpansionAction::ExpansionAction(Action::Route as i32))
    );
    assert_eq!(options.expansion_properties, [0, 4]);
    assert!(options.skip_opposites && options.dedupe);
    assert_eq!(
        error("expansion", &route(r#""action": "teleport""#)),
        ValhallaError::with_detail(ErrorCode::ExpansionNotSupported, "teleport")
    );
    assert_eq!(
        error("expansion", &route("")),
        ValhallaError::with_detail(ErrorCode::ActionRequired, "action")
    );
    assert_eq!(
        error(
            "expansion",
            &route(r#""action": "route", "expansion_properties": ["color"]"#)
        ),
        ValhallaError::with_detail(ErrorCode::InvalidExpansionProperty, "color")
    );

    // Isochrones.
    let options = parse_options(
        "isochrone",
        &route(
            r#""contours": [{"time": 10, "color": "ff0000"}, {"distance": 2}], "polygons": true, "show_locations": true, "reverse": true"#,
        ),
    );
    assert_eq!(options.contours.len(), 2);
    assert!(options.polygons && options.show_locations && options.reverse);
    assert_eq!(
        error("isochrone", &route(r#""contours": [{"color": "ff0000"}]"#)),
        ErrorCode::TimeRequired.into()
    );

    // Matrices.
    let options = parse_options(
        "sources_to_targets",
        r#"{"costing": "auto", "sources": [{"lat": 1, "lon": 1}], "targets": [{"lat": 2, "lon": 2}, {"lat": 3, "lon": 3}], "matrix_locations": 1}"#,
    );
    assert_eq!(
        options.has_matrix_locations,
        Some(HasMatrixLocations::MatrixLocations(1))
    );

    // Map matching.
    let options = parse_options(
        "trace_attributes",
        &route(
            r#""trace_options": {"gps_accuracy": 5, "search_radius": 50, "turn_penalty_factor": 1, "breakage_distance": 2000, "interpolation_distance": 10}, "resample_distance": 5, "height_precision": 3"#,
        ),
    );
    assert_eq!(
        options.has_gps_accuracy,
        Some(HasGpsAccuracy::GpsAccuracy(5.0))
    );
    assert_eq!(
        options.has_resample_distance,
        Some(HasResampleDistance::ResampleDistance(5.0))
    );
    // Trace attributes makes every location a via (then the ends breaks).
    assert_eq!(options.locations[0].r#type, LocationType::KBreak as i32);
    // Height precision is at most 2.
    assert_eq!(options.has_height_precision, None);
}

#[test]
fn recostings() {
    let options = parse_options(
        "route",
        &route(
            r#""recostings": [{"costing": "bicycle", "name": "bike"}, {"costing": "pedestrian"}]"#,
        ),
    );
    assert_eq!(
        options
            .recostings
            .iter()
            .map(|r| r.has_name.clone())
            .collect::<Vec<_>>(),
        [
            Some(HasName::Name("bike".into())),
            Some(HasName::Name("pedestrian".into()))
        ]
    );
    for (recostings, expected) in [
        (
            r#"[{"name": "x"}]"#,
            ValhallaError::new(ErrorCode::RecostingCostingRequired),
        ),
        (
            r"[5]",
            ValhallaError::new(ErrorCode::RecostingCostingRequired),
        ),
        (
            r#"[{"costing": "rocket"}]"#,
            ValhallaError::with_detail(ErrorCode::UnknownCosting, "'rocket'"),
        ),
        (
            r#"[{"costing": "auto"}, {"costing": "auto"}]"#,
            ValhallaError::new(ErrorCode::DuplicateRecostingNames),
        ),
        // Multimodal has no options, so no name.
        (
            r#"[{"costing": "multimodal"}]"#,
            ValhallaError::new(ErrorCode::RecostingCostingRequired),
        ),
    ] {
        assert_eq!(
            error("route", &route(&format!("\"recostings\": {recostings}"))),
            expected,
            "{recostings}"
        );
    }
}

#[test]
fn json_requests() {
    // An empty request is an empty object.
    let mut api = Api::default();
    assert_eq!(parse_json_request(b"", Action::Status, &mut api), Ok(()));
    assert_eq!(
        api.options.as_ref().map(|o| o.action),
        Some(Action::Status as i32)
    );
    // With options already there, it validates them (as a PBF request).
    api.options.as_mut().expect("options").costing_type = CostingType::Pedestrian.into();
    assert_eq!(parse_json_request(b"", Action::Status, &mut api), Ok(()));
    assert!(
        api.options
            .expect("options")
            .costings
            .contains_key(&(CostingType::Pedestrian as i32))
    );
    assert_eq!(
        parse_json_request(b"{", Action::Route, &mut Api::default()),
        Err(ErrorCode::JsonParseFailed.into())
    );
}
