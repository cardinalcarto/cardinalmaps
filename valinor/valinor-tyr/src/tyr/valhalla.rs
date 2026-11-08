//! The Valhalla JSON route response, as mainline's `valhalla_serializers::serialize`
//! (`tyr/route_serializer_valhalla.cc`) writes it.
//!
//! The response is written with rapidjson's writer,
//! whose decimal-place limit is state that later numbers inherit;
//! the calls to [`JsonWriter::set_precision`] below follow mainline's exactly,
//! including the places where it never resets the limit.

use super::openlr::{OpenLrError, openlr_edges};
use super::trip::prev_edge;
use super::writer::JsonWriter;
use super::{Response, unknown};
use valhalla_proto::directions_leg::Maneuver;
use valhalla_proto::options::Units;
use valhalla_proto::trip_leg::Node;
use valhalla_proto::{
    Api, BicycleType, DirectionsLeg, LatLng, Options, PedestrianType, TransitType, TravelMode,
    TripLeg, TripSignElement, TurnLane, VehicleType, lat_lng, location, transit_platform_info,
    turn_lane,
};
use valhalla_response::ValhallaError;

/// `tyr::kDefaultPrecision`.
const DEFAULT_PRECISION: u32 = 3;
/// `tyr::kCoordinatePrecision`.
const COORDINATE_PRECISION: u32 = 6;
/// `midgard::kFeetPerMeter`, a float in mainline.
#[expect(
    clippy::excessive_precision,
    reason = "the literal as mainline writes it; it rounds to the same float"
)]
const FEET_PER_METER: f32 = 3.280_839_9;

/// Serializes the routes in `api.directions` as a Valhalla JSON response.
///
/// # Errors
///
/// Fails with 299, as mainline's Odin worker would report the exception,
/// where mainline throws (an unknown travel mode, no routes at all)
/// or where it would read out of bounds
/// (directions without a matching trip leg, nodes, or recosting).
pub(crate) fn serialize(api: &Api) -> Result<Response, ValhallaError> {
    let options = api.options.clone().unwrap_or_default();
    let routes = api
        .directions
        .as_ref()
        .map(|directions| directions.routes.as_slice())
        .unwrap_or_default();
    if routes.is_empty() {
        // Mainline ends an object it never started, which rapidjson asserts on;
        // with an `id`, it first fails to write the key's value at the root.
        return Err(unknown(if options.has_id.is_some() {
            "!hasRoot_"
        } else {
            "level_stack_.GetSize() >= sizeof(Level)"
        }));
    }
    let trip_routes = api
        .trip
        .as_ref()
        .map(|trip| trip.routes.as_slice())
        .unwrap_or_default();
    let warnings = api
        .info
        .as_ref()
        .map(|info| info.warnings.as_slice())
        .unwrap_or_default();

    let mut writer = JsonWriter::with_capacity(4096);
    for (i, route) in routes.iter().enumerate() {
        if i == 1 {
            writer.key("alternates").start_array();
        }
        let trip_legs = trip_routes
            .get(i)
            .map(|route| route.legs.as_slice())
            .ok_or_else(|| unknown(format!("directions route {i} has no trip route")))?;

        writer.start_object();
        writer.key("trip").start_object();
        locations(&route.legs, &mut writer);
        legs(&options, &route.legs, trip_legs, &mut writer)?;
        linear_references(&options, trip_legs, &mut writer)?;
        summary(&options, &route.legs, trip_legs, &mut writer)?;
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
        writer.end_object(); // trip
        // The first route's object is the response itself, closed after the alternates.
        if i > 0 {
            writer.end_object();
        }
    }
    if routes.len() > 1 {
        writer.end_array();
    }
    if let Some(valhalla_proto::options::HasId::Id(id)) = &options.has_id {
        writer.key("id").string(id);
    }
    writer.end_object();
    Ok(Response::json(writer.finish()))
}

fn lat(ll: Option<&LatLng>) -> f64 {
    match ll.and_then(|ll| ll.has_lat) {
        Some(lat_lng::HasLat::Lat(lat)) => lat,
        None => 0.0,
    }
}

fn lng(ll: Option<&LatLng>) -> f64 {
    match ll.and_then(|ll| ll.has_lng) {
        Some(lat_lng::HasLng::Lng(lng)) => lng,
        None => 0.0,
    }
}

fn units_name(units: i32) -> &'static str {
    match Units::try_from(units) {
        Ok(Units::Kilometers) => "kilometers",
        Ok(Units::Miles) => "miles",
        Err(_) => "",
    }
}

fn length_precision(options: &Options) -> u32 {
    if options.units == Units::Miles as i32 {
        4
    } else {
        3
    }
}

/// The locations of every leg, skipping each later leg's first (it repeats the previous end).
fn locations(legs: &[DirectionsLeg], writer: &mut JsonWriter) {
    writer.set_precision(COORDINATE_PRECISION);
    writer.key("locations").start_array();
    // Mainline only starts skipping once it has written a location.
    let mut skip = 0;
    for leg in legs {
        for location in leg.location.iter().skip(skip) {
            skip = 1;
            writer.start_object();
            writer
                .key("type")
                .string(location_type_name(location.r#type));
            writer.key("lat").double(lat(location.ll.as_ref()));
            writer.key("lon").double(lng(location.ll.as_ref()));
            if !location.name.is_empty() {
                writer.key("name").string(&location.name);
            }
            if !location.street.is_empty() {
                writer.key("street").string(&location.street);
            }
            if let Some(location::HasHeading::Heading(heading)) = location.has_heading {
                writer.key("heading").u64(u64::from(heading));
            }
            if !location.date_time.is_empty() {
                writer.key("date_time").string(&location.date_time);
            }
            if !location.time_zone_offset.is_empty() {
                writer
                    .key("time_zone_offset")
                    .string(&location.time_zone_offset);
            }
            if !location.time_zone_name.is_empty() {
                writer
                    .key("time_zone_name")
                    .string(&location.time_zone_name);
            }
            if location.waiting_secs != 0.0 {
                writer
                    .key("waiting")
                    .u64(float_to_u64(location.waiting_secs));
            }
            if location.side_of_street != location::SideOfStreet::KNone as i32 {
                writer
                    .key("side_of_street")
                    .string(side_of_street_name(location.side_of_street));
            }
            let original_index = location
                .correlation
                .as_ref()
                .map_or(0, |correlation| correlation.original_index);
            writer.key("original_index").u64(u64::from(original_index));
            writer.end_object();
        }
    }
    writer.end_array();
}

/// `static_cast<uint64_t>` of a float: truncation (negative and NaN values become 0 here,
/// where C++ leaves them undefined).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "mainline truncates with static_cast<uint64_t>"
)]
fn float_to_u64(value: f32) -> u64 {
    value as u64
}

fn location_type_name(value: i32) -> &'static str {
    match location::Type::try_from(value) {
        Ok(location::Type::KBreak) => "break",
        Ok(location::Type::KThrough) => "through",
        Ok(location::Type::KVia) => "via",
        Ok(location::Type::KBreakThrough) => "break_through",
        Err(_) => "",
    }
}

fn side_of_street_name(value: i32) -> &'static str {
    match location::SideOfStreet::try_from(value) {
        Ok(location::SideOfStreet::KNone) => "none",
        Ok(location::SideOfStreet::KLeft) => "left",
        Ok(location::SideOfStreet::KRight) => "right",
        Err(_) => "",
    }
}

/// `travel_mode_type` (`proto_conversions.cc`): the travel mode and type names.
///
/// # Errors
///
/// Fails as mainline does (`logic_error`) on a travel mode it does not know.
fn travel_mode_type(maneuver: &Maneuver) -> Result<(&'static str, &'static str), ValhallaError> {
    Ok(match TravelMode::try_from(maneuver.travel_mode) {
        Ok(TravelMode::KDrive) => (
            "drive",
            match VehicleType::try_from(maneuver.vehicle_type) {
                Ok(VehicleType::KCar) | Err(_) => "car",
                Ok(VehicleType::KMotorcycle) => "motorcycle",
                Ok(VehicleType::KAutoBus) => "bus",
                Ok(VehicleType::KTruck) => "truck",
                Ok(VehicleType::KMotorScooter) => "motor_scooter",
            },
        ),
        Ok(TravelMode::KPedestrian) => (
            "pedestrian",
            match PedestrianType::try_from(maneuver.pedestrian_type) {
                Ok(PedestrianType::KWheelchair) => "wheelchair",
                // kBlind has no name in mainline's table, so it falls back to the default.
                Ok(PedestrianType::KFoot | PedestrianType::KBlind) | Err(_) => "foot",
            },
        ),
        Ok(TravelMode::KBicycle) => (
            "bicycle",
            match BicycleType::try_from(maneuver.bicycle_type) {
                Ok(BicycleType::KRoad) | Err(_) => "road",
                Ok(BicycleType::KCross) => "cross",
                Ok(BicycleType::KHybrid) => "hybrid",
                Ok(BicycleType::KMountain) => "mountain",
            },
        ),
        Ok(TravelMode::KTransit) => (
            "transit",
            match TransitType::try_from(maneuver.transit_type) {
                Ok(TransitType::KTram) => "tram",
                Ok(TransitType::KMetro) => "metro",
                Ok(TransitType::KRail) | Err(_) => "rail",
                Ok(TransitType::KBus) => "bus",
                Ok(TransitType::KFerry) => "ferry",
                Ok(TransitType::KCableCar) => "cable_car",
                Ok(TransitType::KGondola) => "gondola",
                Ok(TransitType::KFunicular) => "funicular",
            },
        ),
        Err(_) => return Err(unknown("Unknown travel mode")),
    })
}

fn last_node(leg: &TripLeg) -> Result<&Node, ValhallaError> {
    leg.node
        .last()
        .ok_or_else(|| unknown("a trip leg has no nodes"))
}

fn node_at(leg: &TripLeg, index: u32) -> Result<&Node, ValhallaError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| leg.node.get(index))
        .ok_or_else(|| {
            unknown(format!(
                "maneuver path index {index} is outside the trip leg"
            ))
        })
}

fn elapsed(node: &Node) -> (f64, f64) {
    node.cost
        .as_ref()
        .and_then(|cost| cost.elapsed_cost)
        .map_or((0.0, 0.0), |cost| (cost.seconds, cost.cost))
}

/// The name of the recosting at `index`, where mainline would read past the options' list.
fn recosting_name(options: &Options, index: usize) -> Result<&str, ValhallaError> {
    options
        .recostings
        .get(index)
        .map(|recosting| match &recosting.has_name {
            Some(valhalla_proto::costing::HasName::Name(name)) => name.as_str(),
            None => "",
        })
        .ok_or_else(|| unknown("a trip node has more recosts than the request has recostings"))
}

#[expect(
    clippy::too_many_lines,
    reason = "mirrors mainline's single function, which keeps the writer calls in order"
)]
fn legs(
    options: &Options,
    directions_legs: &[DirectionsLeg],
    trip_legs: &[TripLeg],
    writer: &mut JsonWriter,
) -> Result<(), ValhallaError> {
    writer.key("legs").start_array();
    let length_prec = length_precision(options);
    for (leg_index, directions_leg) in directions_legs.iter().enumerate() {
        let trip_leg = trip_legs
            .get(leg_index)
            .ok_or_else(|| unknown(format!("directions leg {leg_index} has no trip leg")))?;
        writer.start_object();
        let mut has_time_restrictions = false;
        let mut has_toll = false;
        let mut has_highway = false;
        let mut has_ferry = false;

        if !directions_leg.maneuver.is_empty() {
            writer.key("maneuvers").start_array();
        }
        let maneuver_count = directions_leg.maneuver.len();
        for (maneuver_index, maneuver) in directions_leg.maneuver.iter().enumerate() {
            writer.start_object();
            writer
                .key("type")
                .u64(i64::from(maneuver.r#type).cast_unsigned());
            writer.key("instruction").string(&maneuver.text_instruction);
            for (key, value) in [
                (
                    "verbal_transition_alert_instruction",
                    &maneuver.verbal_transition_alert_instruction,
                ),
                (
                    "verbal_succinct_transition_instruction",
                    &maneuver.verbal_succinct_transition_instruction,
                ),
                (
                    "verbal_pre_transition_instruction",
                    &maneuver.verbal_pre_transition_instruction,
                ),
                (
                    "verbal_post_transition_instruction",
                    &maneuver.verbal_post_transition_instruction,
                ),
            ] {
                if !value.is_empty() {
                    writer.key(key).string(value);
                }
            }
            for (key, names) in [
                ("street_names", &maneuver.street_name),
                ("begin_street_names", &maneuver.begin_street_name),
            ] {
                if !names.is_empty() {
                    writer.key(key).start_array();
                    for name in names {
                        writer.string(&name.value);
                    }
                    writer.end_array();
                }
            }

            // Absolute bearings before and after the maneuver.
            if maneuver_index != 0 {
                let node_index = usize::try_from(maneuver.begin_path_index).unwrap_or(usize::MAX);
                let in_bearing = prev_edge(trip_leg, node_index)
                    .ok_or_else(|| unknown("a maneuver has no edge before it"))?
                    .end_heading;
                writer.key("bearing_before").u64(u64::from(in_bearing));
            }
            if maneuver_index + 1 != maneuver_count {
                writer
                    .key("bearing_after")
                    .u64(u64::from(maneuver.begin_heading));
            }

            let end_node = node_at(trip_leg, maneuver.end_path_index)?;
            let begin_node = node_at(trip_leg, maneuver.begin_path_index)?;
            let cost = elapsed(end_node).1 - elapsed(begin_node).1;
            writer.set_precision(DEFAULT_PRECISION);
            writer.key("time").double(maneuver.time);
            writer.set_precision(length_prec);
            writer.key("length").double(f64::from(maneuver.length));
            writer.set_precision(DEFAULT_PRECISION);
            writer.key("cost").double(cost);
            writer
                .key("begin_shape_index")
                .u64(u64::from(maneuver.begin_shape_index));
            writer
                .key("end_shape_index")
                .u64(u64::from(maneuver.end_shape_index));
            // Mainline never advances its iterator over the begin node's recosts,
            // so every recosted time is relative to the first one.
            for (i, end_recost) in end_node.recosts.iter().enumerate() {
                let key = format!("time_{}", recosting_name(options, i)?);
                if let Some(end_cost) = end_recost.elapsed_cost {
                    let begin_seconds = begin_node
                        .recosts
                        .first()
                        .ok_or_else(|| unknown("a maneuver's first node has no recosts"))?
                        .elapsed_cost
                        .map_or(0.0, |cost| cost.seconds);
                    writer.key(&key).double(end_cost.seconds - begin_seconds);
                } else {
                    writer.key(&key).null();
                }
            }

            if maneuver.portions_toll {
                writer.key("toll").bool(true);
                has_toll = true;
            }
            if maneuver.portions_highway {
                writer.key("highway").bool(true);
                has_highway = true;
            }
            if maneuver.portions_ferry {
                writer.key("ferry").bool(true);
                has_ferry = true;
            }
            if maneuver.portions_unpaved {
                writer.key("rough").bool(true);
            }
            if maneuver.has_time_restrictions {
                writer.key("has_time_restrictions").bool(true);
                has_time_restrictions = true;
            }

            if let Some(sign) = &maneuver.sign {
                writer.key("sign").start_object();
                for (key, elements) in [
                    ("exit_number_elements", &sign.exit_numbers),
                    ("exit_branch_elements", &sign.exit_onto_streets),
                    ("exit_toward_elements", &sign.exit_toward_locations),
                    ("exit_name_elements", &sign.exit_names),
                ] {
                    sign_elements(key, elements, writer);
                }
                writer.end_object();
            }

            if maneuver.roundabout_exit_count > 0 {
                writer
                    .key("roundabout_exit_count")
                    .u64(u64::from(maneuver.roundabout_exit_count));
            }

            for (key, value) in [
                ("depart_instruction", &maneuver.depart_instruction),
                (
                    "verbal_depart_instruction",
                    &maneuver.verbal_depart_instruction,
                ),
                ("arrive_instruction", &maneuver.arrive_instruction),
                (
                    "verbal_arrive_instruction",
                    &maneuver.verbal_arrive_instruction,
                ),
            ] {
                if !value.is_empty() {
                    writer.key(key).string(value);
                }
            }

            if let Some(transit_info) = &maneuver.transit_info {
                transit(transit_info, writer);
            }

            if maneuver.verbal_multi_cue {
                writer.key("verbal_multi_cue").bool(true);
            }

            let (mode, travel_type) = travel_mode_type(maneuver)?;
            writer.key("travel_mode").string(mode);
            writer.key("travel_type").string(travel_type);

            if options.turn_lanes {
                turn_lanes(trip_leg, maneuver, writer);
            }

            writer.end_object();
        }
        if !directions_leg.maneuver.is_empty() {
            writer.end_array();
        }

        if options.elevation_interval > 0.0 {
            writer.set_precision(1);
            let unit_factor = if options.units == Units::Miles as i32 {
                FEET_PER_METER
            } else {
                1.0
            };
            let interval = options.elevation_interval;
            writer
                .key("elevation_interval")
                .double(f64::from(interval * unit_factor));
            writer.key("elevation").start_array();
            for height in elevation(trip_leg, interval)? {
                writer.double(f64::from(height * unit_factor));
            }
            writer.end_array();
        }

        writer.key("summary").start_object();
        if options.admin_crossings {
            admins(trip_leg, writer);
        }
        if !directions_leg.level_changes.is_empty() {
            writer.key("level_changes").start_array();
            for level_change in &directions_leg.level_changes {
                writer.start_array();
                writer.u64(u64::from(level_change.shape_index));
                writer.set_precision(level_change.precision.max(1));
                writer.double(f64::from(level_change.level));
                writer.set_precision(DEFAULT_PRECISION);
                writer.end_array();
            }
            writer.end_array();
        }

        let summary = directions_leg.summary.unwrap_or_default();
        let bbox = summary.bbox.unwrap_or_default();
        writer
            .key("has_time_restrictions")
            .bool(has_time_restrictions);
        writer.key("has_toll").bool(has_toll || summary.has_toll);
        writer
            .key("has_highway")
            .bool(has_highway || summary.has_highway);
        writer.key("has_ferry").bool(has_ferry || summary.has_ferry);
        writer.set_precision(COORDINATE_PRECISION);
        writer.key("min_lat").double(lat(bbox.min_ll.as_ref()));
        writer.key("min_lon").double(lng(bbox.min_ll.as_ref()));
        writer.key("max_lat").double(lat(bbox.max_ll.as_ref()));
        writer.key("max_lon").double(lng(bbox.max_ll.as_ref()));
        writer.set_precision(DEFAULT_PRECISION);
        writer.key("time").double(summary.time);
        writer.set_precision(length_prec);
        writer.key("length").double(f64::from(summary.length));
        writer.set_precision(DEFAULT_PRECISION);
        let last = last_node(trip_leg)?;
        writer.key("cost").double(elapsed(last).1);
        for (i, recost) in last.recosts.iter().enumerate() {
            let key = format!("time_{}", recosting_name(options, i)?);
            match recost.elapsed_cost {
                Some(cost) => writer.key(&key).double(cost.seconds),
                None => writer.key(&key).null(),
            };
        }
        writer.end_object();

        writer.key("shape").string(&directions_leg.shape);
        writer.end_object();
    }
    writer.end_array();
    Ok(())
}

fn sign_elements(key: &str, elements: &[TripSignElement], writer: &mut JsonWriter) {
    if elements.is_empty() {
        return;
    }
    writer.key(key).start_array();
    for element in elements {
        writer.start_object();
        writer.key("text").string(&element.text);
        if element.consecutive_count > 0 {
            writer
                .key("consecutive_count")
                .u64(u64::from(element.consecutive_count));
        }
        writer.end_object();
    }
    writer.end_array();
}

fn transit(info: &valhalla_proto::TransitRouteInfo, writer: &mut JsonWriter) {
    writer.key("transit_info").start_object();
    for (key, value) in [
        ("onestop_id", &info.onestop_id),
        ("short_name", &info.short_name),
        ("long_name", &info.long_name),
        ("headsign", &info.headsign),
    ] {
        if !value.is_empty() {
            writer.key(key).string(value);
        }
    }
    writer.key("color").u64(u64::from(info.color));
    writer.key("text_color").u64(u64::from(info.text_color));
    for (key, value) in [
        ("description", &info.description),
        ("operator_onestop_id", &info.operator_onestop_id),
        ("operator_name", &info.operator_name),
        ("operator_url", &info.operator_url),
    ] {
        if !value.is_empty() {
            writer.key(key).string(value);
        }
    }
    if !info.transit_stops.is_empty() {
        writer.key("transit_stops").start_array();
        for stop in &info.transit_stops {
            writer.start_object();
            let stop_type = if stop.r#type == transit_platform_info::Type::KStation as i32 {
                "station"
            } else {
                "stop"
            };
            writer.key("type").string(stop_type);
            for (key, value) in [
                ("onestop_id", &stop.station_onestop_id),
                ("name", &stop.station_name),
                ("arrival_date_time", &stop.arrival_date_time),
                ("departure_date_time", &stop.departure_date_time),
            ] {
                if !value.is_empty() {
                    writer.key(key).string(value);
                }
            }
            writer.key("assumed_schedule").bool(stop.assumed_schedule);
            if stop.ll.is_some() {
                // Mainline never resets this precision: every later double in the response
                // (until the next explicit `set_precision`) keeps six decimal places.
                writer.set_precision(COORDINATE_PRECISION);
                writer.key("lat").double(lat(stop.ll.as_ref()));
                writer.key("lon").double(lng(stop.ll.as_ref()));
            }
            writer.end_object();
        }
        writer.end_array();
    }
    writer.end_object();
}

/// The turn lanes of the edge before the maneuver, when it has more than one.
fn turn_lanes(leg: &TripLeg, maneuver: &Maneuver, writer: &mut JsonWriter) {
    let Ok(begin) = usize::try_from(maneuver.begin_path_index) else {
        return;
    };
    if begin == 0 || begin >= leg.node.len() {
        return;
    }
    let Some(prev) = prev_edge(leg, begin) else {
        return;
    };
    if prev.turn_lanes.len() > 1 {
        writer.key("lanes").start_array();
        for lane in &prev.turn_lanes {
            turn_lane(lane, writer);
        }
        writer.end_array();
    }
}

fn turn_lane(lane: &TurnLane, writer: &mut JsonWriter) {
    writer.start_object();
    writer
        .key("directions")
        .u64(u64::from(lane.directions_mask));
    if lane.state == turn_lane::State::KActive as i32 {
        writer.key("active").u64(u64::from(lane.active_direction));
    } else if lane.state == turn_lane::State::KValid as i32 {
        writer.key("valid").u64(u64::from(lane.active_direction));
    }
    writer.end_object();
}

fn admins(leg: &TripLeg, writer: &mut JsonWriter) {
    writer.key("admins").start_array();
    for admin in &leg.admin {
        writer.start_object();
        writer.key("country_code").string(&admin.country_code);
        writer.key("country_text").string(&admin.country_text);
        writer.key("state_code").string(&admin.state_code);
        writer.key("state_text").string(&admin.state_text);
        writer.end_object();
    }
    writer.end_array();

    if leg.admin.len() > 1 {
        writer.key("admin_crossings").start_array();
        for pair in leg.node.windows(2) {
            let (node, next) = (&pair[0], &pair[1]);
            if next.admin_index != node.admin_index {
                let edge = super::trip::edge_of(node);
                writer.start_object();
                writer
                    .key("from_admin_index")
                    .u64(u64::from(node.admin_index));
                writer
                    .key("to_admin_index")
                    .u64(u64::from(next.admin_index));
                writer
                    .key("begin_shape_index")
                    .u64(u64::from(edge.begin_shape_index));
                writer
                    .key("end_shape_index")
                    .u64(u64::from(edge.end_shape_index));
                writer.end_object();
            }
        }
        writer.end_array();
    }
}

fn linear_references(
    options: &Options,
    legs: &[TripLeg],
    writer: &mut JsonWriter,
) -> Result<(), ValhallaError> {
    use valhalla_proto::options::Action;
    let action = options.action;
    if !options.linear_references
        || (action != Action::TraceRoute as i32 && action != Action::Route as i32)
    {
        return Ok(());
    }
    writer.key("linear_references").start_array();
    for leg in legs {
        for reference in openlr_edges(leg).map_err(|e: OpenLrError| unknown(e.to_string()))? {
            writer.string(&reference);
        }
    }
    writer.end_array();
    Ok(())
}

/// The route summary, plus the status, units, and language that follow it.
fn summary(
    options: &Options,
    directions_legs: &[DirectionsLeg],
    trip_legs: &[TripLeg],
    writer: &mut JsonWriter,
) -> Result<(), ValhallaError> {
    let mut route_time = 0.0_f64;
    let mut route_length = 0.0_f64;
    let mut route_cost = 0.0_f64;
    let mut has_time_restrictions = false;
    let mut has_toll = false;
    let mut has_highway = false;
    let mut has_ferry = false;
    // AABB2's initial box: inverted, with float literals.
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (10000.0, 10000.0, -10000.0, -10000.0);
    let mut recost_times = vec![0.0_f64; options.recostings.len()];
    for (leg_index, leg) in directions_legs.iter().enumerate() {
        let trip_leg = trip_legs
            .get(leg_index)
            .ok_or_else(|| unknown(format!("directions leg {leg_index} has no trip leg")))?;
        let summary = leg.summary.unwrap_or_default();
        route_time += summary.time;
        route_length += f64::from(summary.length);
        let last = last_node(trip_leg)?;
        route_cost += elapsed(last).1;
        for (i, recost) in last.recosts.iter().enumerate() {
            let time = recost_times
                .get_mut(i)
                .ok_or_else(|| unknown("a trip node has more recosts than the request has"))?;
            match recost.elapsed_cost {
                Some(cost) if *time >= 0.0 => *time += cost.seconds,
                _ => *time = -1.0,
            }
        }
        let bbox = summary.bbox.unwrap_or_default();
        let (leg_min_x, leg_min_y) = (lng(bbox.min_ll.as_ref()), lat(bbox.min_ll.as_ref()));
        let (leg_max_x, leg_max_y) = (lng(bbox.max_ll.as_ref()), lat(bbox.max_ll.as_ref()));
        // AABB2::Expand
        if leg_min_x < min_x {
            min_x = leg_min_x;
        }
        if leg_min_y < min_y {
            min_y = leg_min_y;
        }
        if leg_max_x > max_x {
            max_x = leg_max_x;
        }
        if leg_max_y > max_y {
            max_y = leg_max_y;
        }
        has_time_restrictions = has_time_restrictions || summary.has_time_restrictions;
        has_toll = has_toll || summary.has_toll;
        has_highway = has_highway || summary.has_highway;
        has_ferry = has_ferry || summary.has_ferry;
    }

    writer.key("summary").start_object();
    writer
        .key("has_time_restrictions")
        .bool(has_time_restrictions);
    writer.key("has_toll").bool(has_toll);
    writer.key("has_highway").bool(has_highway);
    writer.key("has_ferry").bool(has_ferry);
    writer.set_precision(COORDINATE_PRECISION);
    writer.key("min_lat").double(min_y);
    writer.key("min_lon").double(min_x);
    writer.key("max_lat").double(max_y);
    writer.key("max_lon").double(max_x);
    writer.set_precision(DEFAULT_PRECISION);
    writer.key("time").double(route_time);
    writer.set_precision(length_precision(options));
    writer.key("length").double(route_length);
    writer.set_precision(DEFAULT_PRECISION);
    writer.key("cost").double(route_cost);
    for (i, time) in recost_times.iter().enumerate() {
        let key = format!("time_{}", recosting_name(options, i)?);
        if *time < 0.0 {
            writer.key(&key).null();
        } else {
            writer.key(&key).double(*time);
        }
    }
    writer.end_object();

    writer
        .key("status_message")
        .string("Found route between points");
    writer.key("status").i64(0);
    writer.key("units").string(units_name(options.units));
    writer.key("language").string(match &options.has_language {
        Some(valhalla_proto::options::HasLanguage::Language(language)) => language,
        None => "",
    });
    Ok(())
}

/// `tyr::get_elevation`: the leg's elevation sampled every `interval` meters,
/// with runs of bridges and tunnels interpolated linearly.
///
/// # Errors
///
/// Fails where mainline reads out of bounds: when the first edge has no elevation.
pub(crate) fn elevation(leg: &TripLeg, interval: f32) -> Result<Vec<f32>, ValhallaError> {
    let Some(first_node) = leg.node.first() else {
        return Ok(Vec::new());
    };
    let first_elevation = *super::trip::edge_of(first_node)
        .elevation
        .first()
        .ok_or_else(|| unknown("the trip's first edge has no elevation"))?;
    let mut elevation = vec![first_elevation];
    let mut bridge_edge_count = 0_u32;
    let mut first_bridge_index = 0_usize;
    let mut remaining = 0.0_f32;
    let mut prior_elevation = first_elevation;
    let mut bridges = Vec::new();
    for node in &leg.node {
        let edge = super::trip::edge_of(node);
        let edge_interval = edge.elevation_sampling_interval;
        if edge.bridge || edge.tunnel {
            if bridge_edge_count == 0 {
                first_bridge_index = elevation.len();
            }
            bridge_edge_count += 1;
        } else {
            if bridge_edge_count > 1 && !elevation.is_empty() {
                bridges.push((first_bridge_index, elevation.len() - 1));
            }
            bridge_edge_count = 0;
        }
        for &elev in edge.elevation.iter().skip(1) {
            let mut distance = interval - remaining;
            remaining += edge_interval;
            while remaining > interval {
                let p1 = distance / edge_interval;
                elevation.push(prior_elevation * (1.0 - p1) + elev * p1);
                remaining -= interval;
                distance += interval;
            }
            prior_elevation = elev;
        }
    }
    elevation.push(prior_elevation);
    if bridge_edge_count > 1 {
        bridges.push((first_bridge_index, elevation.len() - 1));
    }
    update_bridge_elevations(&mut elevation, &bridges);
    Ok(elevation)
}

/// `tyr::update_bridge_elevations`.
#[expect(
    clippy::cast_precision_loss,
    reason = "mainline divides by the int32 count as a float"
)]
fn update_bridge_elevations(elevation: &mut [f32], bridges: &[(usize, usize)]) {
    for &(first, last) in bridges {
        let n = last.saturating_sub(first);
        if n > 0 && first < elevation.len() && last < elevation.len() {
            let start = if first > 0 {
                elevation[first - 1]
            } else {
                elevation[0]
            };
            let end = if last < elevation.len() - 1 {
                elevation[last + 1]
            } else {
                elevation[elevation.len() - 1]
            };
            let delta = (end - start) / n as f32;
            let mut elev = start + delta;
            for value in &mut elevation[first..=last] {
                *value = elev;
                elev += delta;
            }
        }
    }
}

#[cfg(test)]
mod tests;
