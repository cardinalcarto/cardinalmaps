//! OSRM-format route responses, as mainline 3.9.1's `osrm_serializers::serialize`
//! (`src/tyr/route_serializer_osrm.cc`) writes them.
//!
//! This is the format Ferrostar reads: OSRM's route response
//! plus the Mapbox Directions extensions `bannerInstructions` and `voiceInstructions`
//! (when the request asks for them), built from Odin's maneuvers.
//! Route (`/route`), map matching (`/trace_route`), and optimized route responses are covered.
//!
//! Mainline builds the response as `baldr::json` maps, which are hash maps,
//! so clients cannot depend on key order; we keep insertion order.
//! Numbers follow `baldr::json::fixed_t`, so the arithmetic that feeds them
//! (including where mainline narrows to `float`) follows mainline exactly.

mod annotations;
mod banner;
mod countries;
mod geo;
mod geometry;
mod intersections;
mod json;
mod legs;
mod maneuvers;
mod signs;
mod summary;
mod turns;
mod voice;
mod waypoints;

#[cfg(test)]
mod tests;

use self::geo::units_to_meters;
use self::geometry::{Generalize, route_geometry};
use self::intersections::Attributes;
use self::json::{Json, Map};
use self::legs::{LegOptions, costing_name, serialize_legs};
use self::summary::summarize_route_legs;
use self::voice::VoiceContext;
use self::waypoints::{optimized_waypoints, route_waypoints, tracepoints};
use super::openlr::openlr_edges;
use super::unknown;
use valhalla_proto::options::{Action, HasGeneralize, HasLanguage, HasShapeFormat, Units};
use valhalla_proto::{Api, FilterAction, Options, ShapeFormat};
use valhalla_response::ValhallaError;

/// `options.language()`: unset reads as empty.
fn language(options: &Options) -> &str {
    match &options.has_language {
        Some(HasLanguage::Language(language)) => language,
        None => "",
    }
}

/// `options.shape_format()`: unset (and unknown values) read as polyline6.
fn shape_format(options: &Options) -> ShapeFormat {
    match options.has_shape_format {
        Some(HasShapeFormat::ShapeFormat(format)) => {
            ShapeFormat::try_from(format).unwrap_or(ShapeFormat::Polyline6)
        }
        None => ShapeFormat::Polyline6,
    }
}

/// The two attributes `AttributesController(options)` decides that this serializer uses.
fn attributes(options: &Options) -> Attributes {
    let mut attributes = Attributes {
        node_admin_index: true,
        edge_is_urban: false,
    };
    let listed = |name: &str| options.filter_attributes.iter().any(|a| a == name);
    match FilterAction::try_from(options.filter_action) {
        Ok(FilterAction::Include) => {
            attributes.node_admin_index |= listed("node.admin_index");
            attributes.edge_is_urban |= listed("edge.is_urban");
        }
        Ok(FilterAction::Exclude) => {
            attributes.node_admin_index &= !listed("node.admin_index");
            attributes.edge_is_urban &= !listed("edge.is_urban");
        }
        _ => {}
    }
    attributes
}

/// `route_references`: each edge's `OpenLR` reference, when requested for routes
/// and map matching.
fn linear_references(
    options: &Options,
    action: Option<Action>,
    route: &valhalla_proto::TripRoute,
) -> Result<Option<Vec<Json>>, ValhallaError> {
    if !options.linear_references || !matches!(action, Some(Action::Route | Action::TraceRoute)) {
        return Ok(None);
    }
    let mut references = Vec::new();
    for leg in &route.legs {
        let edges = openlr_edges(leg).map_err(|e| unknown(e.to_string()))?;
        references.extend(edges.into_iter().map(Json::Str));
    }
    Ok(Some(references))
}

/// `route_summary`: the route's `distance`, `duration`, `weight`, and `weight_name`,
/// plus each recosting's duration and weight.
fn route_summary(
    route: &mut Map,
    api: &Api,
    options: &Options,
    imperial: bool,
    route_index: usize,
) -> Result<(), ValhallaError> {
    let directions_route = api
        .directions
        .as_ref()
        .and_then(|directions| directions.routes.get(route_index))
        .ok_or_else(|| unknown("the directions have fewer routes than the trip"))?;
    let trip_route = api
        .trip
        .as_ref()
        .and_then(|trip| trip.routes.get(route_index))
        .ok_or_else(|| unknown("the trip has fewer routes than the directions"))?;

    let (mut duration, mut distance, mut weight) = (0.0_f64, 0.0_f64, 0.0_f64);
    let mut recosts = vec![(0.0_f64, 0.0_f64); options.recostings.len()];
    for (leg_index, leg) in directions_route.legs.iter().enumerate() {
        let summary = leg.summary.unwrap_or_default();
        distance += f64::from(summary.length);
        duration += summary.time;
        let last = trip_route
            .legs
            .get(leg_index)
            .and_then(|trip_leg| trip_leg.node.last())
            .ok_or_else(|| unknown("the trip has fewer legs than the directions"))?;
        let elapsed = |cost: Option<valhalla_proto::trip_leg::PathCost>| {
            cost.unwrap_or_default().elapsed_cost
        };
        weight += elapsed(last.cost).unwrap_or_default().cost;
        for (i, recost) in last.recosts.iter().enumerate() {
            let Some(total) = recosts.get_mut(i) else {
                continue;
            };
            match recost.elapsed_cost {
                Some(cost) if total.0 >= 0.0 => {
                    total.0 += cost.seconds;
                    total.1 += cost.cost;
                }
                _ => *total = (-1.0, -1.0),
            }
        }
    }

    // Mainline converts the total to meters in float.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "units_to_meters takes a float"
    )]
    let distance = f64::from(units_to_meters(distance as f32, !imperial));
    route.emplace("distance", Json::Fixed(distance, 3));
    route.emplace("duration", Json::Fixed(duration, 3));
    route.emplace("weight", Json::Fixed(weight, 3));
    let costing = options
        .costings
        .get(&options.costing_type)
        .ok_or_else(|| unknown("the request has no options for its costing"))?;
    route.emplace("weight_name", costing_name(costing));
    for (recosting, (seconds, cost)) in options.recostings.iter().zip(recosts) {
        let name = costing_name(recosting);
        if seconds < 0.0 {
            route.emplace(format!("duration_{name}"), Json::Null);
            route.emplace(format!("weight_{name}"), Json::Null);
        } else {
            route.emplace(format!("duration_{name}"), Json::Fixed(seconds, 3));
            route.emplace(format!("weight_{name}"), Json::Fixed(cost, 3));
        }
    }
    Ok(())
}

/// Serializes a route-like response in OSRM format.
///
/// The request is modified as mainline modifies it:
/// optimized routes record each location's `waypoint_index`.
///
/// # Errors
///
/// Fails with 299 (`OdinUnknown`) where mainline would throw or crash,
/// for example when the trip and directions disagree,
/// and with 503 when their leg counts differ, as mainline does.
pub(crate) fn serialize(api: &mut Api) -> Result<String, ValhallaError> {
    let mut options = api.options.clone().unwrap_or_default();
    let action = Action::try_from(options.action).ok();
    let mut response = Map::new();
    response.emplace("code", "Ok");
    match action {
        Some(Action::TraceRoute) => {
            response.emplace("tracepoints", tracepoints(&options.shape)?);
        }
        Some(Action::Route) => {
            let trip = api
                .trip
                .as_ref()
                .ok_or_else(|| unknown("the request has no trip"))?;
            response.emplace("waypoints", route_waypoints(trip)?);
        }
        Some(Action::OptimizedRoute) => {
            let waypoints = optimized_waypoints(&mut options.locations)?;
            response.emplace("waypoints", waypoints);
            if let Some(request_options) = api.options.as_mut() {
                request_options.locations.clone_from(&options.locations);
            }
        }
        _ => return Err(unknown("Unknown route serialization action")),
    }

    // OSRM output is metric except for narrative.
    let imperial = options.units == Units::Miles as i32;
    // Mainline only looks the language up (and fails on unknown ones) once it builds
    // a step's announcements.
    let voice = options
        .voice_instructions
        .then_some(VoiceContext { options: &options });
    let leg_options = LegOptions {
        options: &options,
        imperial,
        shape_format: shape_format(&options),
        attributes: attributes(&options),
        voice,
    };
    let generalize = match options.has_generalize {
        Some(HasGeneralize::Generalize(value)) => Generalize::Value(value),
        None => Generalize::Unset,
    };

    let directions_routes = api
        .directions
        .as_ref()
        .map(|directions| directions.routes.as_slice())
        .unwrap_or_default();
    let summaries = summarize_route_legs(directions_routes);
    let trip_routes = api
        .trip
        .as_ref()
        .map(|trip| trip.routes.as_slice())
        .unwrap_or_default();

    let mut routes = Vec::with_capacity(trip_routes.len());
    for (i, trip_route) in trip_routes.iter().enumerate() {
        let directions_route = directions_routes
            .get(i)
            .ok_or_else(|| unknown("the trip has more routes than the directions"))?;
        let mut route = Map::new();
        if action == Some(Action::TraceRoute) {
            // A placeholder in mainline too.
            route.emplace("confidence", Json::Fixed(1.0, 1));
        }
        if let Some(references) = linear_references(&options, action, trip_route)? {
            route.emplace("linear_references", references);
        }
        if let Some(geometry) =
            route_geometry(directions_route, leg_options.shape_format, generalize)?
        {
            route.emplace("geometry", geometry);
        }
        route_summary(&mut route, api, &options, imperial, i)?;
        let legs = serialize_legs(
            &directions_route.legs,
            summaries.get(i).map(Vec::as_slice).unwrap_or_default(),
            &trip_route.legs,
            &leg_options,
        )?;
        route.emplace("legs", legs);
        if options.voice_instructions {
            route.emplace("voiceLocale", language(&options));
        }
        routes.push(route.into());
    }
    let routes_key = if action == Some(Action::TraceRoute) {
        "matchings"
    } else {
        "routes"
    };
    response.emplace(routes_key, routes);

    add_warnings(&mut response, api);
    Ok(response.to_json())
}

/// `serializeWarnings`: the request's warnings, if it has any.
fn add_warnings(response: &mut Map, api: &Api) {
    let warnings = api
        .info
        .as_ref()
        .map(|info| info.warnings.as_slice())
        .unwrap_or_default();
    if !warnings.is_empty() {
        let warnings: Vec<Json> = warnings
            .iter()
            .map(|warning| {
                let mut out = Map::new();
                out.emplace("code", warning.code);
                out.emplace("text", warning.description.as_str());
                out.into()
            })
            .collect();
        response.emplace("warnings", warnings);
    }
}
