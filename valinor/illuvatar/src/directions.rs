//! Turning each trip leg into a directions leg (`odin::DirectionsBuilder`).

use crate::error::OdinError;
use crate::maneuver::Maneuver;
use crate::maneuvers::ManeuversBuilder;
use crate::markup_formatter::MarkupFormatter;
use crate::narrative::NarrativeBuilder;
use crate::street_names::StreetNames;
use crate::trip::{EdgeExt, LegExt};
use valhalla_proto::directions_leg::Maneuver as DirectionsManeuver;
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::{
    Api, BoundingBox, DirectionsLeg, DirectionsRoute, DirectionsType, LatLng, Options,
    Pronunciation, StreetName, Summary, TravelMode, TripLeg, TripSign, options,
};
use valhalla_response::ErrorCode;

/// Edges at most this long (km) take their headings from a neighbor (`kMinLengthHeading`, ~3 feet).
const MIN_LENGTH_HEADING: f32 = 0.001;

/// Builds the directions for every leg of every route (`DirectionsBuilder::Build`).
///
/// Directions are appended to `api.directions`, one route per trip route,
/// and the trip legs are updated in place as mainline updates them.
pub(crate) fn build(api: &mut Api, markup_formatter: &MarkupFormatter) -> Result<(), OdinError> {
    // A request without options reads as the default options (proto3 semantics), as in mainline.
    let default_options = Options::default();
    let options = api.options.as_ref().unwrap_or(&default_options);
    // Mainline calls `mutable_trip()`, which creates an empty trip if there is none.
    let trip = api.trip.get_or_insert_default();
    let directions = &mut api.directions;

    for trip_route in &mut trip.routes {
        let routes = &mut directions.get_or_insert_default().routes;
        routes.push(DirectionsRoute::default());
        for leg in &mut trip_route.legs {
            if leg.node.is_empty() {
                return Err(OdinError::Coded(ErrorCode::TripPathNoNodes));
            }
            reject_transit(leg)?;

            let mut maneuvers = Vec::new();
            if options.directions_type != DirectionsType::None as i32 {
                update_heading(leg);
                maneuvers = ManeuversBuilder::new(options, leg).build()?;
                if options.directions_type == DirectionsType::Instructions as i32 {
                    NarrativeBuilder::new(options, leg, markup_formatter)?.build(&mut maneuvers)?;
                }
            }

            let directions_leg = populate_directions_leg(options, leg, &maneuvers);
            if let Some(route) = routes.last_mut() {
                route.legs.push(directions_leg);
            }
        }
    }
    Ok(())
}

/// Refuses trips with transit, which this port does not narrate.
///
/// Transit routing is out of scope for Valinor (see `docs/routing.md`),
/// and mainline's transit narrative depends on locale-specific date and time formatting.
/// An error is better than directions that silently differ from mainline's.
fn reject_transit(leg: &TripLeg) -> Result<(), OdinError> {
    let has_transit = leg
        .node
        .iter()
        .filter_map(|node| node.edge.as_ref())
        .any(|edge| edge.has_mode(TravelMode::KTransit) || edge.is_transit_connection());
    if has_transit {
        Err(OdinError::TransitUnsupported)
    } else {
        Ok(())
    }
}

/// Gives near-zero-length edges without headings the heading of a neighbor (`UpdateHeading`).
///
/// This runs in node order and updates the leg, so a later edge sees an earlier one's update.
fn update_heading(leg: &mut TripLeg) {
    for x in 0..leg.node.len() {
        let Some(curr) = leg.curr_edge(x) else {
            continue;
        };
        if !(curr.length_km <= MIN_LENGTH_HEADING
            && curr.begin_heading == 0
            && curr.end_heading == 0)
        {
            continue;
        }
        let heading = if let Some(next) = leg
            .next_edge(x, 1)
            .filter(|next| next.length_km > MIN_LENGTH_HEADING)
        {
            Some(next.begin_heading)
        } else {
            leg.prev_edge(x, 1)
                .filter(|prev| prev.length_km > MIN_LENGTH_HEADING)
                .map(|prev| prev.end_heading)
        };
        if let Some(heading) = heading
            && let Some(edge) = crate::trip::edge_mut(leg, x)
        {
            edge.begin_heading = heading;
            edge.end_heading = heading;
        }
    }
}

fn street_names_to_proto(names: &StreetNames) -> Vec<StreetName> {
    names
        .iter()
        .map(|name| StreetName {
            value: name.value().to_owned(),
            is_route_number: name.is_route_number(),
            pronunciation: name.pronunciation().map(|p| Pronunciation {
                alphabet: p.alphabet,
                value: p.value.clone(),
            }),
            ..StreetName::default()
        })
        .collect()
}

fn lat_lng(source: Option<&LatLng>) -> LatLng {
    let lat = source
        .and_then(|ll| ll.has_lat)
        .map_or(0.0, |HasLat::Lat(lat)| lat);
    let lng = source
        .and_then(|ll| ll.has_lng)
        .map_or(0.0, |HasLng::Lng(lng)| lng);
    LatLng {
        has_lat: Some(HasLat::Lat(lat)),
        has_lng: Some(HasLng::Lng(lng)),
    }
}

/// Copies a maneuver into the directions message (the loop body of `PopulateDirectionsLeg`).
fn maneuver_to_proto(maneuver: &Maneuver, miles: bool) -> DirectionsManeuver {
    let signs = &maneuver.signs;
    let sign = (signs.has_exit() || signs.has_guide() || signs.has_junction_name()).then(|| {
        let to_proto =
            |list: &[crate::signs::Sign]| list.iter().map(crate::signs::Sign::to_proto).collect();
        TripSign {
            exit_numbers: to_proto(&signs.exit_number_list),
            exit_onto_streets: to_proto(&signs.exit_branch_list),
            exit_toward_locations: to_proto(&signs.exit_toward_list),
            exit_names: to_proto(&signs.exit_name_list),
            guide_onto_streets: to_proto(&signs.guide_branch_list),
            guide_toward_locations: to_proto(&signs.guide_toward_list),
            junction_names: to_proto(&signs.junction_name_list),
            ..TripSign::default()
        }
    });

    let mut proto = DirectionsManeuver {
        r#type: maneuver.maneuver_type,
        text_instruction: maneuver.instruction.clone(),
        begin_path_index: maneuver.begin_node_index,
        end_path_index: maneuver.end_node_index,
        street_name: street_names_to_proto(&maneuver.street_names),
        begin_street_name: street_names_to_proto(&maneuver.begin_street_names),
        length: maneuver.length_in(miles),
        time: maneuver.time,
        begin_cardinal_direction: maneuver.begin_cardinal_direction,
        begin_heading: maneuver.begin_heading,
        turn_degree: maneuver.turn_degree,
        begin_shape_index: maneuver.begin_shape_index,
        end_shape_index: maneuver.end_shape_index,
        portions_toll: maneuver.portions_toll,
        portions_highway: maneuver.portions_highway,
        portions_ferry: maneuver.ferry,
        has_time_restrictions: maneuver.has_time_restrictions,
        portions_unpaved: maneuver.portions_unpaved,
        verbal_succinct_transition_instruction: maneuver
            .verbal_succinct_transition_instruction
            .clone(),
        verbal_transition_alert_instruction: maneuver.verbal_transition_alert_instruction.clone(),
        verbal_pre_transition_instruction: maneuver.verbal_pre_transition_instruction.clone(),
        verbal_post_transition_instruction: maneuver.verbal_post_transition_instruction.clone(),
        sign,
        guidance_views: maneuver.guidance_views.clone(),
        roundabout_exit_count: maneuver.roundabout_exit_count,
        roundabout_exit_street_names: street_names_to_proto(&maneuver.roundabout_exit_street_names),
        verbal_multi_cue: maneuver.has_verbal_multi_cue(),
        to_stay_on: maneuver.to_stay_on,
        travel_mode: maneuver.travel_mode,
        bss_maneuver_type: maneuver.bss_maneuver_type,
        // Mainline always sets the bike share info, even when it is empty.
        bss_info: Some(maneuver.bss_info.clone()),
        landmarks: maneuver.landmarks.clone(),
        ..DirectionsManeuver::default()
    };
    // Only the type that matches the travel mode is set.
    match TravelMode::try_from(maneuver.travel_mode) {
        Ok(TravelMode::KDrive) => proto.vehicle_type = maneuver.vehicle_type,
        Ok(TravelMode::KPedestrian) => proto.pedestrian_type = maneuver.pedestrian_type,
        Ok(TravelMode::KBicycle) => proto.bicycle_type = maneuver.bicycle_type,
        Ok(TravelMode::KTransit) => proto.transit_type = maneuver.transit_type,
        Err(_) => {}
    }
    proto
}

/// Fills in a directions leg from the trip leg and its maneuvers (`PopulateDirectionsLeg`).
fn populate_directions_leg(
    options: &Options,
    leg: &TripLeg,
    maneuvers: &[Maneuver],
) -> DirectionsLeg {
    let miles = options.units == options::Units::Miles as i32;
    let bbox = leg.bbox.as_ref();
    let has_time_restrictions = leg.node.iter().any(|node| {
        node.edge
            .as_ref()
            .is_some_and(|edge| edge.has_time_restrictions)
    });
    let trip_summary = leg.summary.as_ref();
    DirectionsLeg {
        trip_id: leg.trip_id,
        leg_id: leg.leg_id,
        leg_count: leg.leg_count,
        location: leg.location.clone(),
        summary: Some(Summary {
            length: leg.length(miles),
            time: leg.elapsed_seconds(leg.last_node_index()),
            bbox: Some(BoundingBox {
                min_ll: Some(lat_lng(bbox.and_then(|bbox| bbox.min_ll.as_ref()))),
                max_ll: Some(lat_lng(bbox.and_then(|bbox| bbox.max_ll.as_ref()))),
            }),
            has_time_restrictions,
            has_toll: trip_summary.is_some_and(|s| s.has_toll),
            has_highway: trip_summary.is_some_and(|s| s.has_highway),
            has_ferry: trip_summary.is_some_and(|s| s.has_ferry),
        }),
        maneuver: maneuvers
            .iter()
            .map(|m| maneuver_to_proto(m, miles))
            .collect(),
        shape: leg.shape.clone(),
        level_changes: leg.level_changes.clone(),
    }
}
