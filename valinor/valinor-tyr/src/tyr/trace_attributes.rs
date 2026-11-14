//! The `trace_attributes` response, as mainline's `tyr::serializeTraceAttributes`
//! (`src/tyr/trace_serializer.cc`) writes it.
//!
//! In mainline, Thor's `trace_attributes` action calls this serializer itself
//! (there is no Odin step), with the request (its `trip` and `options`),
//! the `AttributesController` it built with a strict filter,
//! and, per match (the best, then each alternate), the confidence score, the raw score,
//! and the match result of every trace point. `valinor-thor` does the same.
//!
//! The serializer first adds the scores and matched points to each `TripRoute`
//! (`fill_trace_attributes`), then answers with the request as PBF,
//! or with Valhalla JSON: the best match's attributes at the top level
//! (edges and their end nodes, admins, the shape, the matched points, and so on),
//! and the alternates' in `alternate_paths`.
//!
//! The response is written with rapidjson's writer,
//! whose decimal-place limit is state that later numbers inherit;
//! the calls to [`JsonWriter::set_precision`] below follow mainline's exactly,
//! including where it never resets the limit (the matched points leave it at six places).

use super::Response;
use super::pbf;
use super::valhalla::{elevation, units_name};
use super::writer::JsonWriter;
use crate::attributes::{Attribute, AttributesController, Category};
use valhalla_proto::options::{DateTimeType, Format, HasId, Units};
use valhalla_proto::trip_leg::edge::{
    HasBeginOsmNodeId, HasEndOsmNodeId, HasSpeedsFaded, HasSpeedsNonFaded,
};
use valhalla_proto::trip_leg::{self, Edge, Speeds};
use valhalla_proto::trip_route::{MatchedPoint, matched_point};
use valhalla_proto::{
    Api, BicycleType, LatLng, Options, PedestrianType, TravelMode, TripLeg, VehicleType, lat_lng,
};
use valhalla_response::{ErrorCode, ValhallaError};
use valinor_meili::{MatchResult, MatchResultType};

/// `tyr::kDefaultPrecision`.
const DEFAULT_PRECISION: u32 = 3;
/// `tyr::kCoordinatePrecision`.
const COORDINATE_PRECISION: u32 = 6;
/// `midgard::kMilePerKm`, a float in mainline (the scale it multiplies by is a double).
const MILE_PER_KM: f32 = 0.621_371;
/// `midgard::kFeetPerMeter`, a float in mainline.
#[expect(
    clippy::excessive_precision,
    reason = "the literal as mainline writes it; it rounds to the same float"
)]
const FEET_PER_METER: f32 = 3.280_839_9;
/// `midgard::kSecPerMillisecond`.
const SEC_PER_MILLISECOND: f64 = 0.001;
/// `midgard::kKmPerDecimeter`.
const KM_PER_DECIMETER: f64 = 0.0001;
/// `midgard::kDecimeterPerSectoKPH`.
const DECIMETER_PER_SEC_TO_KPH: f64 = 0.36;
/// `baldr::kUnlimitedSpeedLimit`.
const UNLIMITED_SPEED_LIMIT: u32 = 255;
/// `baldr::kNoElevationData`.
const NO_ELEVATION_DATA: f32 = -500.0;

/// What Thor's map matching gives the serializer for one match (the best, then each alternate):
/// the confidence score, the raw score, and the match result of every trace point
/// (mainline's `std::tuple<float, float, std::vector<meili::MatchResult>>`).
///
/// An edge walk (`shape_match` `edge_walk`, or a `walk_or_snap` whose walk succeeded)
/// is one match with scores 1 and 0 and no match results.
pub type MapMatchResult = (f32, f32, Vec<MatchResult>);

/// Serializes a `trace_attributes` response, as mainline's `tyr::serializeTraceAttributes` does.
///
/// `api` holds Thor's `trip` (a route per match, each matching `map_match_results` in order)
/// and the request's `options`; `controller` is the strict `AttributesController` that Thor
/// built the trip with (`AttributesController::new(options, true)`).
///
/// Like mainline, this modifies the request: each route gets its `confidence_score`,
/// `raw_score`, and `matched_points` (the attributes the controller allows),
/// and a PBF response clears what the client didn't select (see [`super::serialize_directions`]).
///
/// # Errors
///
/// Fails with 499 (Thor's unknown error, since Thor's worker runs this) where mainline would
/// read out of bounds: when there are more matches than routes in the trip,
/// when a route has no legs, or when there are no matches at all for a JSON response
/// (mainline ends an array it never started).
/// A PBF response fails with 299, as [`super::serialize_directions`] does,
/// only for a request with no PBF output, which a `trace_attributes` request always has.
pub fn serialize_trace_attributes(
    api: &mut Api,
    controller: &AttributesController,
    map_match_results: &[MapMatchResult],
) -> Result<Response, ValhallaError> {
    fill_trace_attributes(api, controller, map_match_results)?;

    let options = api.options.clone().unwrap_or_default();
    if options.format == Format::Pbf as i32 {
        return pbf::serialize(api);
    }

    let mut writer = JsonWriter::with_capacity(4096);
    writer.start_object();
    if let Some(HasId::Id(id)) = &options.has_id
        && !id.is_empty()
    {
        writer.key("id").string(id);
    }
    writer.key("units").string(units_name(options.units));

    let routes = api
        .trip
        .as_ref()
        .map(|trip| trip.routes.as_slice())
        .unwrap_or_default();
    if map_match_results.is_empty() {
        return Err(thor_unknown("there are no map match results to serialize"));
    }
    for (i, map_match_result) in map_match_results.iter().enumerate() {
        let leg = routes
            .get(i)
            .and_then(|route| route.legs.first())
            .ok_or_else(|| thor_unknown(format!("map match result {i} has no trip leg")))?;
        if i == 0 {
            append_trace_info(&mut writer, controller, &options, map_match_result, leg)?;
            writer.key("alternate_paths").start_array();
        } else {
            writer.start_object();
            append_trace_info(&mut writer, controller, &options, map_match_result, leg)?;
            writer.end_object();
        }
    }
    writer.end_array();

    let warnings = api
        .info
        .as_ref()
        .map(|info| info.warnings.as_slice())
        .unwrap_or_default();
    if !warnings.is_empty() {
        writer.key("warnings").start_array();
        for warning in warnings {
            writer.start_object();
            writer.key("code").u64(warning.code);
            writer.key("text").string(&warning.description);
            writer.end_object();
        }
        writer.end_array();
    }
    writer.end_object();
    Ok(Response::json(writer.finish()))
}

/// Thor's error for an exception (`thor_worker_t::work` reports any `std::exception` as 499).
fn thor_unknown(detail: impl Into<String>) -> ValhallaError {
    ValhallaError::with_detail(ErrorCode::ThorUnknown, detail)
}

/// `GetType` as the proto's `MatchType`.
fn match_type(result: &MatchResult) -> matched_point::MatchType {
    match result.match_type() {
        MatchResultType::Matched => matched_point::MatchType::KMatched,
        MatchResultType::Interpolated => matched_point::MatchType::KInterpolated,
        MatchResultType::Unmatched => matched_point::MatchType::KUnmatched,
    }
}

/// A match result's `edge_index` as mainline's `size_t`: unset is `kInvalidEdgeIndex`.
fn edge_index(result: &MatchResult) -> u64 {
    result
        .edge_index
        .map_or(u64::MAX, |index| u64::try_from(index).unwrap_or(u64::MAX))
}

/// `fill_trace_attributes`: the scores and matched points of each route,
/// which the PBF response carries (and the JSON response doesn't read).
fn fill_trace_attributes(
    api: &mut Api,
    controller: &AttributesController,
    map_match_results: &[MapMatchResult],
) -> Result<(), ValhallaError> {
    if map_match_results.is_empty() {
        return Ok(());
    }
    let routes = api
        .trip
        .as_mut()
        .map(|trip| trip.routes.as_mut_slice())
        .unwrap_or_default();
    for (i, (confidence_score, raw_score, results)) in map_match_results.iter().enumerate() {
        let route = routes
            .get_mut(i)
            .ok_or_else(|| thor_unknown(format!("map match result {i} has no trip route")))?;
        if controller.get(Attribute::ConfidenceScore) {
            route.confidence_score = *confidence_score;
        }
        if controller.get(Attribute::RawScore) {
            route.raw_score = *raw_score;
        }
        for result in results {
            let matched = result.match_type() != MatchResultType::Unmatched;
            let mut point = MatchedPoint::default();
            if controller.get(Attribute::MatchedPoint) {
                point.has_latlng = Some(matched_point::HasLatlng::Latlng(LatLng {
                    has_lat: Some(lat_lng::HasLat::Lat(result.lnglat.1)),
                    has_lng: Some(lat_lng::HasLng::Lng(result.lnglat.0)),
                }));
            }
            if controller.get(Attribute::MatchedType) {
                point.r#type = match_type(result) as i32;
            }
            if controller.get(Attribute::MatchedEdgeIndex) && result.edge_id.is_some() {
                // Mainline stores its `size_t` in a `uint32`, keeping the low 32 bits
                // (so an unset index is `u32::MAX`).
                let low =
                    u32::try_from(edge_index(result) & u64::from(u32::MAX)).unwrap_or(u32::MAX);
                point.has_edge_index = Some(matched_point::HasEdgeIndex::EdgeIndex(low));
            }
            if controller.get(Attribute::MatchedDistanceAlongEdge) && matched {
                point.has_distance_along_edge = Some(
                    matched_point::HasDistanceAlongEdge::DistanceAlongEdge(result.distance_along),
                );
            }
            if controller.get(Attribute::MatchedBeginRouteDiscontinuity) {
                point.begins_discontinuity = result.begins_discontinuity;
            }
            if controller.get(Attribute::MatchedEndRouteDiscontinuity) {
                point.ends_discontinuity = result.ends_discontinuity;
            }
            if controller.get(Attribute::MatchedDistanceFromTracePoint) && matched {
                point.has_distance_from_trace_point = Some(
                    matched_point::HasDistanceFromTracePoint::DistanceFromTracePoint(
                        result.distance_from,
                    ),
                );
            }
            route.matched_points.push(point);
        }
    }
    Ok(())
}

/// `append_trace_info`: one match's attributes, as members of the current object.
fn append_trace_info(
    writer: &mut JsonWriter,
    controller: &AttributesController,
    options: &Options,
    (confidence_score, raw_score, match_results): &MapMatchResult,
    leg: &TripLeg,
) -> Result<(), ValhallaError> {
    if controller.get(Attribute::OsmChangeset) {
        writer.key("osm_changeset").u64(leg.osm_changeset);
    }
    if controller.get(Attribute::Shape) {
        writer.key("shape").string(&leg.shape);
    }
    if controller.get(Attribute::ConfidenceScore) {
        writer.set_precision(DEFAULT_PRECISION);
        writer
            .key("confidence_score")
            .double(f64::from(*confidence_score));
    }
    if controller.get(Attribute::RawScore) {
        writer.set_precision(DEFAULT_PRECISION);
        writer.key("raw_score").double(f64::from(*raw_score));
    }
    if !leg.admin.is_empty() {
        admins(leg, writer);
    }
    edges(controller, options, leg, writer);
    if options.elevation_interval > 0.0 {
        let unit_factor = if options.units == Units::Miles as i32 {
            FEET_PER_METER
        } else {
            1.0
        };
        let interval = options.elevation_interval;
        writer.set_precision(1);
        writer
            .key("elevation_interval")
            .double(f64::from(interval * unit_factor));
        // Mainline reads out of bounds (and Thor's worker reports it) when the leg's first edge
        // has no elevation; `elevation` fails with Odin's code there, so report Thor's.
        let heights = elevation(leg, interval)
            .map_err(|e| thor_unknown(e.detail().unwrap_or_default().to_owned()))?;
        writer.key("elevation").start_array();
        for height in heights {
            writer.double(f64::from(height * unit_factor));
        }
        writer.end_array();
    }
    if controller.category_enabled(Category::Matched) && !match_results.is_empty() {
        matched_points(controller, match_results, writer);
    }
    if controller.category_enabled(Category::ShapeAttributes) {
        shape_attributes(controller, leg, writer);
    }
    Ok(())
}

/// `serialize_admins`: only the codes and names that are set.
fn admins(leg: &TripLeg, writer: &mut JsonWriter) {
    writer.key("admins").start_array();
    for admin in &leg.admin {
        writer.start_object();
        for (key, value) in [
            ("country_code", &admin.country_code),
            ("country_text", &admin.country_text),
            ("state_code", &admin.state_code),
            ("state_text", &admin.state_text),
        ] {
            if !value.is_empty() {
                writer.key(key).string(value);
            }
        }
        writer.end_object();
    }
    writer.end_array();
}

/// `serialize_speed`: a speed in the request's units, rounded (`std::round`) to an integer.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "mainline converts the rounded double with static_cast<uint64_t>"
)]
fn serialize_speed(speed: f32, scale: f64) -> u64 {
    (f64::from(speed) * scale).round() as u64
}

/// `serialize_speeds`: an edge's speeds by source, the flows that are set and `no_flow`.
fn speeds(key: &str, speeds: &Speeds, scale: f64, writer: &mut JsonWriter) {
    writer.key(key).start_object();
    for (key, flow) in [
        ("current_flow", speeds.current_flow),
        ("predicted_flow", speeds.predicted_flow),
        ("constrained_flow", speeds.constrained_flow),
        ("free_flow", speeds.free_flow),
    ] {
        if flow != 0.0 {
            writer.key(key).u64(serialize_speed(flow, scale));
        }
    }
    writer
        .key("no_flow")
        .u64(serialize_speed(speeds.no_flow, scale));
    writer.end_object();
}

/// `serialize_edges`: each edge of the leg with the attributes the controller allows,
/// and its end node when any node attribute is on.
#[expect(
    clippy::too_many_lines,
    reason = "mirrors mainline's single function, which keeps the writer calls in order"
)]
#[expect(
    clippy::float_cmp,
    reason = "mainline compares exactly, and the fractions are set exactly"
)]
fn edges(
    controller: &AttributesController,
    options: &Options,
    leg: &TripLeg,
    writer: &mut JsonWriter,
) {
    let on = |attribute| controller.get(attribute);
    let miles = options.units == Units::Miles as i32;
    let scale = if miles { f64::from(MILE_PER_KM) } else { 1.0 };

    writer.key("edges").start_array();
    for pair in leg.node.windows(2) {
        let Some(edge) = pair[0].edge.as_ref() else {
            continue;
        };
        let end_node = &pair[1];
        writer.start_object();
        if on(Attribute::EdgeTruckRoute) {
            writer.key("truck_route").bool(edge.truck_route);
        }
        if on(Attribute::EdgeTruckSpeed) && edge.truck_speed > 0.0 {
            writer
                .key("truck_speed")
                .u64(serialize_speed(edge.truck_speed, scale));
        }
        if on(Attribute::EdgeSpeedLimit) && edge.speed_limit > 0 {
            if edge.speed_limit == UNLIMITED_SPEED_LIMIT {
                writer.key("speed_limit").string("unlimited");
            } else {
                // Mainline passes the integer to `serialize_speed`, which takes a float.
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "mainline converts the uint32 to float"
                )]
                let limit = edge.speed_limit as f32;
                writer.key("speed_limit").u64(serialize_speed(limit, scale));
            }
        }
        if on(Attribute::EdgeDensity) {
            writer.key("density").u64(u64::from(edge.density));
        }
        if on(Attribute::EdgeSacScale) {
            writer
                .key("sac_scale")
                .u64(i64::from(edge.sac_scale).cast_unsigned());
        }
        if on(Attribute::EdgeShoulder) {
            writer.key("shoulder").bool(edge.shoulder);
        }
        if on(Attribute::EdgeSidewalk) {
            writer.key("sidewalk").string(sidewalk_name(edge.sidewalk));
        }
        if on(Attribute::EdgeBicycleNetwork) {
            writer
                .key("bicycle_network")
                .u64(u64::from(edge.bicycle_network));
        }
        if on(Attribute::EdgeCycleLane) {
            writer
                .key("cycle_lane")
                .string(cycle_lane_name(edge.cycle_lane));
        }
        if on(Attribute::EdgeLaneCount) {
            writer.key("lane_count").u64(u64::from(edge.lane_count));
        }
        // Mainline writes lane connectivity whatever the controller says.
        if !edge.lane_connectivity.is_empty() {
            writer.key("lane_connectivity").start_array();
            for connectivity in &edge.lane_connectivity {
                writer.start_object();
                writer.key("from").u64(connectivity.from_way_id);
                writer.key("to_lanes").string(&connectivity.to_lanes);
                writer.key("from_lanes").string(&connectivity.from_lanes);
                writer.end_object();
            }
            writer.end_array();
        }
        if on(Attribute::EdgeMaxDownwardGrade) {
            writer
                .key("max_downward_grade")
                .i64(i64::from(edge.max_downward_grade));
        }
        if on(Attribute::EdgeMaxUpwardGrade) {
            writer
                .key("max_upward_grade")
                .i64(i64::from(edge.max_upward_grade));
        }
        if on(Attribute::EdgeWeightedGrade) {
            writer.set_precision(DEFAULT_PRECISION);
            writer
                .key("weighted_grade")
                .double(f64::from(edge.weighted_grade));
        }
        if on(Attribute::EdgeMeanElevation) {
            mean_elevation(edge, miles, writer);
        }
        if on(Attribute::EdgeWayId) {
            writer.key("way_id").u64(edge.way_id);
        }
        if on(Attribute::EdgeBeginOsmNodeId)
            && let Some(HasBeginOsmNodeId::BeginOsmNodeId(id)) = edge.has_begin_osm_node_id
        {
            writer.key("node_id").u64(id);
        }
        if on(Attribute::EdgeId) {
            writer.key("id").u64(edge.id);
        }
        travel_types(controller, edge, writer);
        if on(Attribute::EdgeSurface) {
            writer.key("surface").string(surface_name(edge.surface));
        }
        if on(Attribute::EdgeCurvature) {
            writer.key("curvature").u64(u64::from(edge.curvature));
        }
        for (attribute, key, value) in [
            (
                Attribute::EdgeDriveOnRight,
                "drive_on_right",
                !edge.drive_on_left,
            ),
            (
                Attribute::EdgeInternalIntersection,
                "internal_intersection",
                edge.internal_intersection,
            ),
            (Attribute::EdgeRoundabout, "roundabout", edge.roundabout),
            (Attribute::EdgeBridge, "bridge", edge.bridge),
            (Attribute::EdgeTunnel, "tunnel", edge.tunnel),
            (Attribute::EdgeUnpaved, "unpaved", edge.unpaved),
            (Attribute::EdgeToll, "toll", edge.toll),
        ] {
            if on(attribute) {
                writer.key(key).bool(value);
            }
        }
        if on(Attribute::EdgeUse) {
            writer.key("use").string(use_name(edge.r#use));
        }
        if on(Attribute::EdgeTraversability) {
            writer
                .key("traversability")
                .string(traversability_name(edge.traversability));
        }
        for (attribute, key, value) in [
            (
                Attribute::EdgeEndShapeIndex,
                "end_shape_index",
                edge.end_shape_index,
            ),
            (
                Attribute::EdgeBeginShapeIndex,
                "begin_shape_index",
                edge.begin_shape_index,
            ),
            (Attribute::EdgeEndHeading, "end_heading", edge.end_heading),
            (
                Attribute::EdgeBeginHeading,
                "begin_heading",
                edge.begin_heading,
            ),
        ] {
            if on(attribute) {
                writer.key(key).u64(u64::from(value));
            }
        }
        if on(Attribute::EdgeRoadClass) {
            writer
                .key("road_class")
                .string(road_class_name(edge.road_class));
        }
        if on(Attribute::EdgeSpeed) {
            writer.key("speed").u64(serialize_speed(edge.speed, scale));
        }
        if on(Attribute::EdgeSpeedType) {
            writer
                .key("speed_type")
                .string(speed_type_name(edge.speed_type));
        }
        if on(Attribute::EdgeSpeedsFaded)
            && options.date_time_type == DateTimeType::Current as i32
            && let Some(HasSpeedsFaded::SpeedsFaded(faded)) = &edge.has_speeds_faded
        {
            speeds("speeds_faded", faded, scale, writer);
        }
        if on(Attribute::EdgeSpeedsNonFaded) {
            // An unset message reads as one with every flow 0.
            let non_faded = match &edge.has_speeds_non_faded {
                Some(HasSpeedsNonFaded::SpeedsNonFaded(speeds)) => *speeds,
                None => Speeds::default(),
            };
            speeds("speeds_non_faded", &non_faded, scale, writer);
        }
        for (attribute, key, value) in [
            (
                Attribute::EdgeCountryCrossing,
                "country_crossing",
                edge.country_crossing,
            ),
            (Attribute::EdgeForward, "forward", edge.forward),
            (
                Attribute::EdgeTrafficSignal,
                "traffic_signal",
                edge.traffic_signal,
            ),
        ] {
            if on(attribute) {
                writer.key(key).bool(value);
            }
        }
        if on(Attribute::EdgeHovType) {
            writer.key("hov_type").string(hov_type_name(edge.hov_type));
        }
        if on(Attribute::EdgeLevels) && !edge.levels.is_empty() {
            writer.key("levels").start_array();
            writer.set_precision(edge.level_precision);
            for level in &edge.levels {
                writer.start_array();
                writer.double(f64::from(level.start));
                writer.double(f64::from(level.end));
                writer.end_array();
            }
            writer.end_array();
            writer.set_precision(DEFAULT_PRECISION);
        }
        if on(Attribute::EdgeLength) {
            writer.set_precision(DEFAULT_PRECISION);
            writer
                .key("length")
                .double(f64::from(edge.length_km) * scale);
            if edge.source_along_edge != 0.0 {
                writer
                    .key("source_percent_along")
                    .double(f64::from(edge.source_along_edge));
            }
            if edge.target_along_edge != 1.0 {
                writer
                    .key("target_percent_along")
                    .double(f64::from(edge.target_along_edge));
            }
        }
        // Mainline writes names, traffic segments, and signs whatever the controller says.
        if !edge.name.is_empty() {
            writer.key("names").start_array();
            for name in &edge.name {
                writer.string(&name.value);
            }
            writer.end_array();
        }
        if !edge.traffic_segment.is_empty() {
            writer.key("traffic_segments").start_array();
            for segment in &edge.traffic_segment {
                writer.start_object();
                writer.set_precision(DEFAULT_PRECISION);
                writer.key("segment_id").u64(segment.segment_id);
                writer
                    .key("begin_percent")
                    .double(f64::from(segment.begin_percent));
                writer
                    .key("end_percent")
                    .double(f64::from(segment.end_percent));
                writer.key("starts_segment").bool(segment.starts_segment);
                writer.key("ends_segment").bool(segment.ends_segment);
                writer.end_object();
            }
            writer.end_array();
        }
        if let Some(sign) = &edge.sign {
            writer.key("sign").start_object();
            for (key, elements) in [
                ("exit_number", &sign.exit_numbers),
                ("exit_branch", &sign.exit_onto_streets),
                ("exit_toward", &sign.exit_toward_locations),
                ("exit_name", &sign.exit_names),
            ] {
                if !elements.is_empty() {
                    writer.key(key).start_array();
                    for element in elements {
                        writer.string(&element.text);
                    }
                    writer.end_array();
                }
            }
            writer.end_object();
        }
        if controller.category_enabled(Category::Node) {
            node(controller, edge, end_node, writer);
        }
        writer.end_object();
    }
    writer.end_array();
}

/// The edge's mean elevation, in feet for miles, truncated to an integer.
#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline truncates with static_cast<int64_t>"
)]
#[expect(
    clippy::cast_precision_loss,
    reason = "mainline reads the int32 into a float"
)]
fn mean_elevation(edge: &Edge, miles: bool, writer: &mut JsonWriter) {
    let mut mean = edge.mean_elevation as f32;
    if mean != NO_ELEVATION_DATA {
        if miles {
            mean *= FEET_PER_METER;
        }
        writer.key("mean_elevation").i64(mean as i64);
    }
}

/// The travel mode, and the type for the mode (none for transit).
fn travel_types(controller: &AttributesController, edge: &Edge, writer: &mut JsonWriter) {
    let mode = TravelMode::try_from(edge.travel_mode).ok();
    if controller.get(Attribute::EdgeTravelMode) {
        writer
            .key("travel_mode")
            .string(travel_mode_name(edge.travel_mode));
    }
    if controller.get(Attribute::EdgeVehicleType) && mode == Some(TravelMode::KDrive) {
        writer
            .key("vehicle_type")
            .string(vehicle_type_name(edge.vehicle_type));
    }
    if controller.get(Attribute::EdgePedestrianType) && mode == Some(TravelMode::KPedestrian) {
        writer
            .key("pedestrian_type")
            .string(pedestrian_type_name(edge.pedestrian_type));
    }
    if controller.get(Attribute::EdgeBicycleType) && mode == Some(TravelMode::KBicycle) {
        writer
            .key("bicycle_type")
            .string(bicycle_type_name(edge.bicycle_type));
    }
}

/// The `end_node` of an edge: the edges that meet the path there, its cost, and its attributes.
fn node(
    controller: &AttributesController,
    edge: &Edge,
    node: &trip_leg::Node,
    writer: &mut JsonWriter,
) {
    let on = |attribute| controller.get(attribute);
    writer.key("end_node").start_object();
    if !node.intersecting_edge.is_empty() {
        writer.key("intersecting_edges").start_array();
        for xedge in &node.intersecting_edge {
            writer.start_object();
            let untraversable = trip_leg::Traversability::KNone as i32;
            for (attribute, key, value) in [
                (
                    Attribute::NodeIntersectingEdgeWalkability,
                    "walkability",
                    xedge.walkability,
                ),
                (
                    Attribute::NodeIntersectingEdgeCyclability,
                    "cyclability",
                    xedge.cyclability,
                ),
                (
                    Attribute::NodeIntersectingEdgeDriveability,
                    "driveability",
                    xedge.driveability,
                ),
            ] {
                if on(attribute) && value != untraversable {
                    writer.key(key).string(traversability_name(value));
                }
            }
            if on(Attribute::NodeIntersectingEdgeFromEdgeNameConsistency) {
                writer
                    .key("from_edge_name_consistency")
                    .bool(xedge.prev_name_consistency);
            }
            if on(Attribute::NodeIntersectingEdgeToEdgeNameConsistency) {
                writer
                    .key("to_edge_name_consistency")
                    .bool(xedge.curr_name_consistency);
            }
            if on(Attribute::NodeIntersectingEdgeBeginHeading) {
                writer
                    .key("begin_heading")
                    .u64(u64::from(xedge.begin_heading));
            }
            if on(Attribute::NodeIntersectingEdgeUse) {
                writer.key("use").string(use_name(xedge.r#use));
            }
            if on(Attribute::NodeIntersectingEdgeRoadClass) {
                writer
                    .key("road_class")
                    .string(road_class_name(xedge.road_class));
            }
            writer.end_object();
        }
        writer.end_array();
    }
    let cost = node.cost.unwrap_or_default();
    if on(Attribute::NodeElapsedTime) {
        let elapsed = cost.elapsed_cost.unwrap_or_default();
        writer.set_precision(DEFAULT_PRECISION);
        writer.key("elapsed_time").double(elapsed.seconds);
        writer.key("elapsed_cost").double(elapsed.cost);
    }
    if on(Attribute::EdgeEndOsmNodeId)
        && let Some(HasEndOsmNodeId::EndOsmNodeId(id)) = edge.has_end_osm_node_id
    {
        writer.key("node_id").u64(id);
    }
    if on(Attribute::NodeAdminIndex) {
        writer.key("admin_index").u64(u64::from(node.admin_index));
    }
    if on(Attribute::NodeType) {
        writer.key("type").string(node_type_name(node.r#type));
    }
    if on(Attribute::NodeTrafficSignal) {
        writer.key("traffic_signal").bool(node.traffic_signal);
    }
    if on(Attribute::NodeFork) {
        writer.key("fork").bool(node.fork);
    }
    if on(Attribute::NodeTimeZone) && !node.time_zone.is_empty() {
        writer.key("time_zone").string(&node.time_zone);
    }
    if on(Attribute::NodeTransitionTime) {
        writer.set_precision(DEFAULT_PRECISION);
        writer
            .key("transition_time")
            .double(cost.transition_cost.unwrap_or_default().seconds);
    }
    writer.end_object();
}

/// `serialize_matched_points`: every trace point's match.
fn matched_points(
    controller: &AttributesController,
    match_results: &[MatchResult],
    writer: &mut JsonWriter,
) {
    let on = |attribute| controller.get(attribute);
    writer.key("matched_points").start_array();
    for result in match_results {
        let kind = result.match_type();
        writer.start_object();
        if on(Attribute::MatchedPoint) {
            writer.set_precision(COORDINATE_PRECISION);
            writer.key("lon").double(result.lnglat.0);
            writer.key("lat").double(result.lnglat.1);
        }
        if on(Attribute::MatchedType) {
            writer.key("type").string(match kind {
                MatchResultType::Matched => "matched",
                MatchResultType::Interpolated => "interpolated",
                MatchResultType::Unmatched => "unmatched",
            });
        }
        if on(Attribute::MatchedEdgeIndex) && result.edge_id.is_some() {
            // An unset index is written as mainline's `kInvalidEdgeIndex` (the largest `size_t`).
            writer.key("edge_index").u64(edge_index(result));
        }
        if on(Attribute::MatchedBeginRouteDiscontinuity) && result.begins_discontinuity {
            writer.key("begin_route_discontinuity").bool(true);
        }
        if on(Attribute::MatchedEndRouteDiscontinuity) && result.ends_discontinuity {
            writer.key("end_route_discontinuity").bool(true);
        }
        if on(Attribute::MatchedDistanceAlongEdge) && kind != MatchResultType::Unmatched {
            writer.set_precision(COORDINATE_PRECISION);
            writer
                .key("distance_along_edge")
                .double(result.distance_along);
        }
        if on(Attribute::MatchedDistanceFromTracePoint) && kind != MatchResultType::Unmatched {
            writer.set_precision(COORDINATE_PRECISION);
            writer
                .key("distance_from_trace_point")
                .double(result.distance_from);
        }
        writer.end_object();
    }
    writer.end_array();
}

/// `serialize_shape_attributes`: per shape point times, lengths, speeds, congestion,
/// and speed limits, and the closures, in seconds, kilometers, and kilometers per hour.
fn shape_attributes(controller: &AttributesController, leg: &TripLeg, writer: &mut JsonWriter) {
    let attributes = leg.shape_attributes.clone().unwrap_or_default();
    writer.key("shape_attributes").start_object();
    writer.set_precision(DEFAULT_PRECISION);
    for (attribute, key, values, factor) in [
        (
            Attribute::ShapeAttributesTime,
            "time",
            &attributes.time,
            SEC_PER_MILLISECOND,
        ),
        (
            Attribute::ShapeAttributesLength,
            "length",
            &attributes.length,
            KM_PER_DECIMETER,
        ),
        (
            Attribute::ShapeAttributesSpeed,
            "speed",
            &attributes.speed,
            DECIMETER_PER_SEC_TO_KPH,
        ),
    ] {
        if controller.get(attribute) {
            writer.key(key).start_array();
            for &value in values {
                writer.double(f64::from(value) * factor);
            }
            writer.end_array();
        }
    }
    if controller.get(Attribute::ShapeAttributesCongestion) {
        writer.key("congestion").start_array();
        for &congestion in &attributes.congestion {
            writer.u64(u64::from(congestion));
        }
        writer.end_array();
    }
    if controller.get(Attribute::ShapeAttributesClosure) {
        writer.key("closure").start_array();
        for closure in &leg.closures {
            let begin = match closure.has_begin_shape_index {
                Some(trip_leg::closure::HasBeginShapeIndex::BeginShapeIndex(index)) => index,
                None => 0,
            };
            let end = match closure.has_end_shape_index {
                Some(trip_leg::closure::HasEndShapeIndex::EndShapeIndex(index)) => index,
                None => 0,
            };
            writer.start_object();
            writer.key("begin_shape_index").u64(u64::from(begin));
            writer.key("end_shape_index").u64(u64::from(end));
            writer.end_object();
        }
        writer.end_array();
    }
    if controller.get(Attribute::ShapeAttributesSpeedLimit) {
        writer.key("speed_limit").start_array();
        for &speed_limit in &attributes.speed_limit {
            writer.u64(u64::from(speed_limit));
        }
        writer.end_array();
    }
    writer.end_object();
}

// The names below are mainline's `to_string`s: `baldr`'s (in `graphconstants.h`, over the
// proto's values, which are the same numbers) and Odin's (`enhancedtrippath.h`).
// Values they don't list are "null".

fn sidewalk_name(value: i32) -> &'static str {
    match value {
        0 => "none",
        1 => "left",
        2 => "right",
        3 => "both",
        _ => "null",
    }
}

fn cycle_lane_name(value: i32) -> &'static str {
    match value {
        0 => "none",
        1 => "shared",
        2 => "dedicated",
        3 => "separated",
        _ => "null",
    }
}

fn surface_name(value: i32) -> &'static str {
    match value {
        0 => "paved_smooth",
        1 => "paved",
        2 => "paved_rough",
        3 => "compacted",
        4 => "dirt",
        5 => "gravel",
        6 => "path",
        7 => "impassable",
        _ => "null",
    }
}

fn use_name(value: i32) -> &'static str {
    match value {
        0 => "road",
        1 => "ramp",
        2 => "turn_channel",
        3 => "track",
        4 => "driveway",
        5 => "alley",
        6 => "parking_aisle",
        7 => "emergency_access",
        8 => "drive_through",
        9 => "culdesac",
        10 => "living_street",
        11 => "service_road",
        20 => "cycleway",
        21 => "mountain_bike",
        24 => "sidewalk",
        25 => "footway",
        26 => "steps",
        27 => "path",
        28 => "pedestrian",
        29 => "bridleway",
        30 => "rest_area",
        31 => "service_area",
        32 => "pedestrian_crossing",
        33 => "elevator",
        34 => "escalator",
        35 => "platform",
        40 => "other",
        41 => "ferry",
        42 => "rail-ferry",
        43 => "construction",
        50 => "rail",
        51 => "bus",
        52 => "egress_connection",
        53 => "platform_connection",
        54 => "transit_connection",
        _ => "null",
    }
}

fn traversability_name(value: i32) -> &'static str {
    match value {
        0 => "none",
        1 => "forward",
        2 => "backward",
        3 => "both",
        _ => "null",
    }
}

fn road_class_name(value: i32) -> &'static str {
    match value {
        0 => "motorway",
        1 => "trunk",
        2 => "primary",
        3 => "secondary",
        4 => "tertiary",
        5 => "unclassified",
        6 => "residential",
        7 => "service_other",
        _ => "null",
    }
}

fn speed_type_name(value: i32) -> &'static str {
    match value {
        0 => "tagged",
        1 => "classified",
        _ => "null",
    }
}

fn hov_type_name(value: i32) -> &'static str {
    match value {
        0 => "HOV-2",
        1 => "HOV-3",
        _ => "null",
    }
}

fn node_type_name(value: i32) -> &'static str {
    match value {
        0 => "street_intersection",
        1 => "gate",
        2 => "bollard",
        3 => "toll_booth",
        4 => "transit_egress",
        5 => "transit_station",
        6 => "multi_use_transit_platform",
        7 => "bike_share",
        8 => "parking",
        9 => "motor_way_junction",
        10 => "border_control",
        11 => "toll_gantry",
        12 => "sump_buster",
        13 => "building_entrance",
        14 => "elevator",
        _ => "null",
    }
}

fn travel_mode_name(value: i32) -> &'static str {
    match TravelMode::try_from(value) {
        Ok(TravelMode::KDrive) => "drive",
        Ok(TravelMode::KPedestrian) => "pedestrian",
        Ok(TravelMode::KBicycle) => "bicycle",
        Ok(TravelMode::KTransit) => "transit",
        Err(_) => "null",
    }
}

/// Odin's table has no motor scooter.
fn vehicle_type_name(value: i32) -> &'static str {
    match VehicleType::try_from(value) {
        Ok(VehicleType::KCar) => "car",
        Ok(VehicleType::KMotorcycle) => "motorcycle",
        Ok(VehicleType::KAutoBus) => "bus",
        Ok(VehicleType::KTruck) => "truck",
        Ok(VehicleType::KMotorScooter) | Err(_) => "null",
    }
}

/// Odin's table has no blind pedestrians.
fn pedestrian_type_name(value: i32) -> &'static str {
    match PedestrianType::try_from(value) {
        Ok(PedestrianType::KFoot) => "foot",
        Ok(PedestrianType::KWheelchair) => "wheelchair",
        Ok(PedestrianType::KBlind) | Err(_) => "null",
    }
}

fn bicycle_type_name(value: i32) -> &'static str {
    match BicycleType::try_from(value) {
        Ok(BicycleType::KRoad) => "road",
        Ok(BicycleType::KCross) => "cross",
        Ok(BicycleType::KHybrid) => "hybrid",
        Ok(BicycleType::KMountain) => "mountain",
        Err(_) => "null",
    }
}

#[cfg(test)]
mod tests;
