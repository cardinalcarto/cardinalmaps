//! The verbose parts of the `/locate` response: everything the tiles say about each edge and
//! node, as mainline's `json` methods of its tile records write it
//! (`DirectedEdge::json`, `EdgeInfo::json`, `NodeInfo::json`, `AccessRestriction::json`,
//! and `GraphId::json`), plus the edge's linear reference and speeds.
//!
//! There is no live traffic, so `live_speed` is always empty and there are no incidents.

use super::locate::{
    COORDINATE_PRECISION, DEFAULT_PRECISION, LocateError, data, lat_lng, side_name,
};
use super::openlr::point_along_line;
use super::polyline::{LngLat, encode};
use super::writer::JsonWriter;
use enumset::EnumSet;
use jiff::Timestamp;
use valhalla_graphtile::graph_tile::predicted_speeds::{FlowSource, select_speed};
use valhalla_graphtile::graph_tile::{
    AccessRestrictionType, DirectedEdge, EdgeInfo, GraphTile, NodeInfo, TaggedValue,
    TaggedValueKind,
};
use valhalla_graphtile::{Access, BicycleNetwork, GraphId, RoadClass, RoadUse};
use valhalla_proto::{Location, PathEdge};
use valinor_sif::TimeInfo;
use valinor_sif::datetime::time_zone_name;

/// The access modes `DirectedEdge::json` writes; `NodeInfo::json` leaves out the last two.
const ACCESS_MODES: [(&str, Access); 11] = [
    ("bicycle", Access::Bicycle),
    ("bus", Access::Bus),
    ("car", Access::Auto),
    ("emergency", Access::Emergency),
    ("HOV", Access::HOV),
    ("pedestrian", Access::Pedestrian),
    ("taxi", Access::Taxi),
    ("truck", Access::Truck),
    ("wheelchair", Access::Wheelchair),
    ("moped", Access::Moped),
    ("motorcycle", Access::Motorcycle),
];

/// `access_json`: a flag for each access mode.
fn access_json(writer: &mut JsonWriter, access: EnumSet<Access>, modes: &[(&str, Access)]) {
    for &(key, mode) in modes {
        writer.key(key).bool(access.contains(mode));
    }
}

/// `GraphId::json` (which writes nothing for an invalid ID).
fn graph_id_json(writer: &mut JsonWriter, id: GraphId) {
    writer.key("level").u64(u64::from(id.level()));
    writer.key("tile_id").u64(id.tile_id());
    writer.key("id").u64(id.feature_index());
    writer.key("value").u64(id.value());
}

fn road_class_name(class: RoadClass) -> &'static str {
    [
        "motorway",
        "trunk",
        "primary",
        "secondary",
        "tertiary",
        "unclassified",
        "residential",
        "service_other",
    ]
    .get(usize::from(class.discriminant()))
    .copied()
    .unwrap_or("null")
}

/// `to_string(Use)`.
fn use_name(road_use: Option<RoadUse>) -> &'static str {
    let Some(road_use) = road_use else {
        return "null";
    };
    match road_use as u8 {
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

/// `DirectedEdge::json`.
#[expect(
    clippy::too_many_lines,
    reason = "one line per field, as mainline writes them"
)]
fn directed_edge_json(writer: &mut JsonWriter, edge: &DirectedEdge) {
    writer.key("end_node").start_object();
    graph_id_json(writer, edge.end_node_id());
    writer.end_object();

    writer.key("speeds").start_object();
    writer.key("default").u64(u64::from(edge.speed()));
    writer.key("type").string(match edge.speed_type() as u8 {
        0 => "tagged",
        _ => "classified",
    });
    writer
        .key("free_flow")
        .u64(u64::from(edge.free_flow_speed()));
    writer
        .key("constrained_flow")
        .u64(u64::from(edge.constrained_flow_speed()));
    writer.key("predicted").bool(edge.has_predicted_speed());
    writer.end_object();

    writer
        .key("access_restriction")
        .bool(!edge.access_restrictions().is_empty());
    writer.key("start_restriction").start_object();
    access_json(writer, edge.start_restriction(), &ACCESS_MODES);
    writer.end_object();
    writer.key("end_restriction").start_object();
    access_json(writer, edge.end_restriction(), &ACCESS_MODES);
    writer.end_object();

    writer
        .key("part_of_complex_restriction")
        .bool(edge.part_of_complex_restriction());
    writer.key("has_sign").bool(edge.has_sign());
    writer.key("toll").bool(edge.toll());
    writer.key("destination_only").bool(edge.dest_only());
    writer
        .key("destination_only_hgv")
        .bool(edge.dest_only_hgv());
    writer.key("tunnel").bool(edge.is_tunnel());
    writer.key("bridge").bool(edge.is_bridge());
    writer.key("round_about").bool(edge.roundabout());
    writer.key("traffic_signal").bool(edge.has_traffic_signal());
    writer.key("forward").bool(edge.edge_info_is_forward());
    writer.key("not_thru").bool(edge.no_thru());
    writer.key("stop_sign").bool(edge.has_stop_sign());
    writer.key("yield_sign").bool(edge.has_yield_sign());
    writer
        .key("cycle_lane")
        .string(match edge.cycle_lane() as u8 {
            0 => "none",
            1 => "shared",
            2 => "dedicated",
            3 => "separated",
            _ => "null",
        });
    writer.key("bike_network").bool(edge.is_bike_network());
    writer.key("truck_route").bool(edge.truck_route());
    writer.key("lane_count").u64(u64::from(edge.lane_count()));
    writer.key("country_crossing").bool(edge.country_crossing());
    writer.key("sidewalk_left").bool(edge.has_sidewalk_left());
    writer.key("sidewalk_right").bool(edge.has_sidewalk_right());
    writer
        .key("sac_scale")
        .string(match edge.sac_scale().map(|s| s as u8) {
            Ok(0) => "none",
            Ok(1) => "hiking",
            Ok(2) => "mountain hiking",
            Ok(3) => "demanding mountain hiking",
            Ok(4) => "alpine hiking",
            Ok(5) => "demanding alpine hiking",
            Ok(6) => "difficult alpine hiking",
            _ => "null",
        });
    writer.key("deadend").bool(edge.is_dead_end());

    writer.key("geo_attributes").start_object();
    writer.key("length").u64(u64::from(edge.length()));
    writer.set_precision(2);
    writer
        .key("weighted_grade")
        .double((f64::from(edge.weighted_grade()) - 6.0) / 0.6);
    writer
        .key("max_up_slope")
        .double(f64::from(edge.max_up_slope()));
    writer
        .key("max_down_slope")
        .double(f64::from(edge.max_down_slope()));
    writer.set_precision(3);
    writer.key("curvature").u64(u64::from(edge.curvature()));
    writer.end_object();

    writer.key("access").start_object();
    access_json(writer, edge.forward_access(), &ACCESS_MODES);
    writer.end_object();

    writer.key("classification").start_object();
    writer
        .key("classification")
        .string(road_class_name(edge.classification()));
    writer.key("use").string(use_name(edge.road_use().ok()));
    writer.key("surface").string(
        [
            "paved_smooth",
            "paved",
            "paved_rough",
            "compacted",
            "dirt",
            "gravel",
            "path",
            "impassable",
        ]
        .get(edge.surface() as usize)
        .copied()
        .unwrap_or("null"),
    );
    writer.key("link").bool(edge.is_link());
    writer.key("internal").bool(edge.is_intersection_internal());
    writer.end_object();

    if edge.is_hov_only()
        && let Some(hov) = edge.hov_type()
    {
        writer.key("hov_type").string(match hov as u8 {
            0 => "HOV-2",
            _ => "HOV-3",
        });
    }
}

/// `EdgeInfo::json`.
fn edge_info_json(writer: &mut JsonWriter, info: &EdgeInfo<'_>) -> Result<(), LocateError> {
    writer.key("way_id").u64(info.way_id());
    let networks = info.bicycle_network();
    writer.key("bike_network").start_object();
    writer
        .key("national")
        .bool(networks.contains(BicycleNetwork::National));
    writer
        .key("regional")
        .bool(networks.contains(BicycleNetwork::Regional));
    writer
        .key("local")
        .bool(networks.contains(BicycleNetwork::Local));
    writer
        .key("mountain")
        .bool(networks.contains(BicycleNetwork::Mountain));
    writer.end_object();

    writer.key("names").start_array();
    for name in info.names() {
        writer.string(&name.map_err(data)?.text_lossy());
    }
    writer.end_array();

    let shape: Vec<LngLat> = info
        .decode_raw_shape::<f64>()
        .map_err(data)?
        .into_iter()
        .map(|c| LngLat { lng: c.x, lat: c.y })
        .collect();
    writer.key("shape").string(&encode(&shape, 1e6));

    match info.mean_elevation() {
        None => writer.key("mean_elevation").null(),
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline converts the float elevation to int64"
        )]
        Some(elevation) => writer.key("mean_elevation").i64(elevation as i64),
    };
    if info.speed_limit() == u8::MAX {
        writer.key("speed_limit").string("unlimited");
    } else {
        writer.key("speed_limit").u64(u64::from(info.speed_limit()));
    }

    // Mainline walks the tagged values sorted by tag (a multimap), writing levels and OSM node
    // IDs as it meets them and collecting conditional speed limits for the end.
    let mut tagged = info
        .tagged_values()
        .collect::<Result<Vec<_>, _>>()
        .map_err(data)?;
    tagged.sort_by_key(TaggedValue::tag);
    let mut conditional_speed_limits = Vec::new();
    for value in &tagged {
        match value.kind() {
            Some(TaggedValueKind::Levels) => {
                let levels = value.levels().map_err(data)?;
                writer.key("levels").start_array();
                let precision = levels.precision();
                if precision != 0 {
                    writer.set_precision(precision);
                }
                for range in levels.ranges() {
                    let (low, high) = range.map_err(data)?;
                    #[expect(clippy::float_cmp, reason = "mainline compares the ends exactly")]
                    let single = low == high;
                    if single {
                        writer.double(f64::from(low));
                    } else {
                        writer.start_array();
                        writer.double(f64::from(low));
                        writer.double(f64::from(high));
                        writer.end_array();
                    }
                }
                writer.end_array();
            }
            Some(TaggedValueKind::OsmNodeIds) => {
                writer.key("osm_node_ids").start_array();
                for id in value.osm_node_ids().map_err(data)? {
                    writer.u64(id.map_err(data)?);
                }
                writer.end_array();
            }
            Some(TaggedValueKind::ConditionalSpeedLimits) => {
                let limit = value.conditional_speed_limit().map_err(data)?;
                conditional_speed_limits.push((limit.time_domain().to_string(), limit.speed()));
            }
            _ => {}
        }
    }
    if !conditional_speed_limits.is_empty() {
        writer.key("conditional_speed_limits").start_object();
        for (condition, speed) in &conditional_speed_limits {
            writer.key(condition).u64(u64::from(*speed));
        }
        writer.end_object();
    }
    Ok(())
}

/// `AccessRestriction::json` for each of an edge's access restrictions.
fn access_restrictions_json<T: GraphTile>(writer: &mut JsonWriter, tile: &T, edge_index: u32) {
    for restriction in tile.edge_access_restrictions(edge_index) {
        let restriction_type = restriction.restriction_type().ok();
        writer.start_object();
        writer
            .key("type")
            .string(restriction_type.map_or("unsupported", |t| t.as_str()));
        writer
            .key("edge_index")
            .u64(u64::from(restriction.edge_index()));
        let modes = restriction.affected_access_modes();
        for (key, mode) in [
            ("bus", Access::Bus),
            ("car", Access::Auto),
            ("emergency", Access::Emergency),
            ("HOV", Access::HOV),
            ("pedestrian", Access::Pedestrian),
            ("taxi", Access::Taxi),
            ("truck", Access::Truck),
            ("wheelchair", Access::Wheelchair),
            ("moped", Access::Moped),
            ("motorcycle", Access::Motorcycle),
        ] {
            writer.key(key).bool(modes.contains(mode));
        }
        writer
            .key("except_destination")
            .bool(restriction.except_destination());
        match restriction_type {
            Some(
                AccessRestrictionType::TimedAllowed
                | AccessRestrictionType::TimedDenied
                | AccessRestrictionType::DestinationAllowed,
            ) => {
                writer.key("value").start_object();
                writer.key("time_domain").u64(restriction.value());
                writer.end_object();
            }
            Some(AccessRestrictionType::MaxAxles) => {
                writer.key("value").u64(restriction.value());
            }
            Some(AccessRestrictionType::Hazmat) => {
                writer.key("value").bool(restriction.value() != 0);
            }
            _ => {
                writer.set_precision(2);
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "mainline converts the value to a double"
                )]
                let value = restriction.value() as f64 * 0.01;
                writer.key("value").double(value);
                writer.set_precision(3);
            }
        }
        writer.end_object();
    }
}

/// An edge's linear reference: a point along the line of its two ends, in `OpenLR`
/// (`linear_reference` in mainline's locate serializer).
fn linear_reference(
    edge: &DirectedEdge,
    percent_along: f32,
    info: &EdgeInfo<'_>,
) -> Result<String, LocateError> {
    // `get_fow`
    let vehicular = Access::Auto
        | Access::Truck
        | Access::Moped
        | Access::Motorcycle
        | Access::Taxi
        | Access::Bus
        | Access::HOV;
    let road_use = edge.road_use().ok();
    let fow = if edge.classification() == RoadClass::Motorway {
        1
    } else if edge.roundabout() {
        4
    } else if matches!(road_use, Some(RoadUse::Ramp | RoadUse::TurnChannel)) {
        6
    } else if !edge.forward_access().is_disjoint(vehicular)
        && !edge.reverse_access().is_disjoint(vehicular)
    {
        2
    } else if !edge.forward_access().is_disjoint(vehicular)
        || !edge.reverse_access().is_disjoint(vehicular)
    {
        3
    } else {
        7
    };
    let mut shape: Vec<LngLat> = info
        .decode_raw_shape::<f64>()
        .map_err(data)?
        .into_iter()
        .map(|c| LngLat { lng: c.x, lat: c.y })
        .collect();
    if !edge.edge_info_is_forward() {
        shape.reverse();
    }
    // Mainline stores the offset bucket as `uint8_t(min(255.f, percent * 255 + .5f))`.
    let poff = (percent_along * 255.0 + 0.5).min(255.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "mainline converts the float bucket to uint8_t"
    )]
    let poff = poff as u8;
    point_along_line(
        &shape,
        edge.classification().discriminant(),
        fow,
        edge.length(),
        poff,
    )
    .ok_or_else(|| data("an edge shape has fewer than two points"))
}

/// Writes a verbose edge (the `verbose` branch of `serialize_edges`).
pub(super) fn edge_verbose<T: GraphTile>(
    writer: &mut JsonWriter,
    location: &Location,
    edge: &PathEdge,
    tile: &T,
    id: GraphId,
    directed: &DirectedEdge,
    info: &EdgeInfo<'_>,
) -> Result<(), LocateError> {
    let edge_index = u32::try_from(id.feature_index()).map_err(data)?;
    writer.key("access_restrictions").start_array();
    access_restrictions_json(writer, tile, edge_index);
    writer.end_array();
    // No live traffic.
    writer.key("live_speed").start_object();
    writer.end_object();

    let (lat, lng) = lat_lng(edge.ll.as_ref());
    writer.set_precision(COORDINATE_PRECISION);
    writer.key("correlated_lat").double(lat);
    writer.key("correlated_lon").double(lng);
    writer
        .key("side_of_street")
        .string(side_name(edge.side_of_street));
    let circle = edge.bounding_circle.unwrap_or_default();
    let (circle_lat, circle_lng) = lat_lng(circle.latlng.as_ref());
    writer.key("bounding_circle").start_object();
    writer.key("lat").double(circle_lat);
    writer.key("lon").double(circle_lng);
    writer.key("radius").u64(u64::from(circle.radius));
    writer.end_object();

    #[expect(
        clippy::cast_possible_truncation,
        reason = "mainline passes the double percent along as a float"
    )]
    let percent_along = edge.percent_along as f32;
    writer
        .key("linear_reference")
        .string(&linear_reference(directed, percent_along, info)?);
    writer.set_precision(5);
    writer.key("percent_along").double(edge.percent_along);
    writer.set_precision(1);
    writer.key("distance").double(edge.distance);
    writer.key("shoulder").bool(directed.has_shoulder());
    writer.key("heading").double(f64::from(edge.heading));
    writer.set_precision(DEFAULT_PRECISION);
    writer
        .key("outbound_reach")
        .i64(i64::from(edge.outbound_reach));
    writer
        .key("inbound_reach")
        .i64(i64::from(edge.inbound_reach));

    writer.key("edge_info").start_object();
    edge_info_json(writer, info)?;
    writer.end_object();

    writer.key("edge").start_object();
    directed_edge_json(writer, directed);
    writer.end_object();

    writer.key("edge_id").start_object();
    graph_id_json(writer, id);
    writer.end_object();

    let speed = |seconds: Option<u32>| -> Result<u64, LocateError> {
        let selected = select_speed(
            directed,
            FlowSource::Predicted.into(),
            seconds.map(u64::from),
            false,
            |s| {
                tile.get_predicted_speed(usize::try_from(edge_index).unwrap_or(usize::MAX), s)
                    .ok_or_else(|| data("an edge has no predicted speed profile"))
            },
        )?;
        Ok(u64::from(selected.kph))
    };
    if !location.date_time.is_empty() && directed.has_predicted_speed() {
        // Mainline reads the time without a time zone (`TimeInfo::make(dt, 0)`);
        // a time that doesn't parse falls back to the edge's speed.
        let when = TimeInfo::make(&location.date_time, 0, Timestamp::now());
        writer
            .key("timed_predicted_speed")
            .u64(speed(when.seconds_of_week())?);
    }
    writer.key("predicted_speeds").start_array();
    if directed.has_predicted_speed() {
        for seconds in (0..7 * 24 * 3600).step_by(5 * 60) {
            writer.u64(speed(Some(seconds))?);
        }
    }
    writer.end_array();
    Ok(())
}

/// Writes a verbose node (`NodeInfo::json` and its ID).
pub(super) fn node_verbose<T: GraphTile>(
    writer: &mut JsonWriter,
    tile: &T,
    id: GraphId,
    node: &NodeInfo,
) -> Result<(), LocateError> {
    let ll = node.coordinate_f64(tile.header().sw_corner());
    writer.set_precision(6);
    writer.key("lon").double(ll.x);
    writer.key("lat").double(ll.y);
    writer.set_precision(2);
    // Mainline reads a stored elevation of 0 as -500 m.
    writer
        .key("elevation")
        .double(f64::from(node.elevation().unwrap_or(-500.0)));
    writer.set_precision(3);
    writer.key("edge_count").u64(u64::from(node.edge_count()));
    writer.key("access").start_object();
    access_json(writer, node.access(), &ACCESS_MODES[..9]);
    writer.end_object();
    writer.key("tagged_access").bool(node.tagged_access());
    writer
        .key("intersection_type")
        .string(match node.intersection_type().map(|t| t as u8) {
            Ok(0) => "regular",
            Ok(1) => "false",
            Ok(2) => "dead-end",
            Ok(3) => "fork",
            _ => "null",
        });

    let admin = tile
        .get_admin_info(usize::from(node.admin_index()))
        .map_err(data)?;
    writer.key("administrative").start_object();
    writer.key("iso_3166-1").string(&admin.country_iso);
    writer.key("country").string(&admin.country_text);
    writer.key("iso_3166-2").string(&admin.state_iso);
    writer.key("state").string(&admin.state_text);
    if let Some(name) = node
        .time_zone_index()
        .and_then(|index| time_zone_name(u32::from(index.get())))
    {
        writer.key("time_zone_name").string(name);
    }
    writer.end_object();

    writer.key("density").u64(u64::from(node.density()));
    writer
        .key("local_edge_count")
        .u64(u64::from(node.local_edge_count()));
    writer.key("drive_on_right").bool(node.drive_on_right());
    writer.key("mode_change").bool(node.mode_change_allowed());
    writer.key("private_access").bool(node.private_access());
    writer.key("traffic_signal").bool(node.is_traffic_signal());
    writer
        .key("type")
        .string(match node.node_type().map(|t| t as u8) {
            Ok(0) => "street_intersection",
            Ok(1) => "gate",
            Ok(2) => "bollard",
            Ok(3) => "toll_booth",
            Ok(4) => "transit_egress",
            Ok(5) => "transit_station",
            Ok(6) => "multi_use_transit_platform",
            Ok(7) => "bike_share",
            Ok(8) => "parking",
            Ok(9) => "motor_way_junction",
            Ok(10) => "border_control",
            Ok(11) => "toll_gantry",
            Ok(12) => "sump_buster",
            Ok(13) => "building_entrance",
            Ok(14) => "elevator",
            _ => "null",
        });
    writer
        .key("transition_count")
        .u64(u64::from(node.transition_count()));
    writer
        .key("named_intersection")
        .bool(node.is_named_intersection());
    if node.is_transit() {
        writer
            .key("stop_index")
            .u64(u64::from(node.transition_index()));
    }

    writer.key("node_id").start_object();
    graph_id_json(writer, id);
    writer.end_object();
    Ok(())
}
