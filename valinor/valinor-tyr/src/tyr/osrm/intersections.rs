//! A step's `intersections` (`intersections` in mainline's `route_serializer_osrm.cc`),
//! and the turn lane checks shared with banner instructions.

use super::json::{Json, Map};
use super::signs::destinations;
use super::turns::{TURN_LANE_EMPTY, TURN_LANE_NONE, lane_indications, turn_lane_direction};
use crate::tyr::polyline::LngLat;
use crate::tyr::trip::{self, curr_edge, prev_edge};
use crate::tyr::unknown;
use valhalla_proto::directions_leg::Maneuver;
use valhalla_proto::trip_leg::{self, Edge, IntersectingEdge, Node};
use valhalla_proto::turn_lane::State as LaneState;
use valhalla_proto::{RoadClass, TaggedValue, TravelMode, TripLeg, TurnLane, tagged_value};
use valhalla_response::ValhallaError;

/// The attributes of mainline's `AttributesController` that the OSRM serializer checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Attributes {
    /// `node.admin_index`, on by default.
    pub(super) node_admin_index: bool,
    /// `edge.is_urban`, off by default.
    pub(super) edge_is_urban: bool,
}

/// `EnhancedTripLeg_Edge::drive_on_right`.
pub(super) fn drive_on_right(edge: &Edge) -> bool {
    !edge.drive_on_left
}

/// `EnhancedTripLeg_Edge::HasActiveTurnLane`.
fn has_active_turn_lane(lanes: &[TurnLane]) -> bool {
    lanes
        .iter()
        .any(|lane| lane.state == LaneState::KActive as i32)
}

/// `EnhancedTripLeg_Edge::HasNonDirectionalTurnLane`.
fn has_non_directional_turn_lane(lanes: &[TurnLane]) -> bool {
    lanes.iter().any(|lane| {
        lane.directions_mask == TURN_LANE_EMPTY || lane.directions_mask & TURN_LANE_NONE != 0
    })
}

/// Whether an edge's turn lanes are worth showing: some are active and all are directional.
pub(super) fn has_usable_turn_lanes(edge: &Edge) -> bool {
    !edge.turn_lanes.is_empty()
        && has_active_turn_lane(&edge.turn_lanes)
        && !has_non_directional_turn_lane(&edge.turn_lanes)
}

/// `EnhancedTripLeg_IntersectingEdge::IsTraversableOutbound`.
fn is_traversable_outbound(edge: &IntersectingEdge, travel_mode: i32) -> bool {
    let traversability = if travel_mode == TravelMode::KDrive as i32 {
        edge.driveability
    } else if travel_mode == TravelMode::KBicycle as i32 {
        edge.cyclability
    } else {
        edge.walkability
    };
    traversability == trip_leg::Traversability::KForward as i32
        || traversability == trip_leg::Traversability::KBoth as i32
}

/// A bearing at an intersection and what it is (mainline's `IntersectionEdges`).
#[derive(Debug, Clone, Copy)]
struct IntersectionEdge {
    bearing: u32,
    routeable: bool,
    in_edge: bool,
    out_edge: bool,
}

/// Lossy text of a tagged value's bytes (mainline copies them into a `std::string`).
fn tagged_text(value: &TaggedValue) -> String {
    String::from_utf8_lossy(&value.value).into_owned()
}

/// The edge that leaves an intersection, which only the arrive maneuver's last one lacks.
fn outgoing(edge: Option<&Edge>) -> Result<&Edge, ValhallaError> {
    edge.ok_or_else(|| unknown("intersection without an outgoing edge"))
}

/// `toll_collection`, `turn_duration`, `turn_weight`, `duration`, and `weight`.
fn add_costs(intersection: &mut Map, node: &Node, next: Option<&Node>) {
    let mut toll_collection = Map::new();
    if node.r#type == trip_leg::node::Type::KTollBooth as i32 {
        toll_collection.emplace("type", "toll_booth");
    } else if node.r#type == trip_leg::node::Type::KTollGantry as i32 {
        toll_collection.emplace("type", "toll_gantry");
    }
    if !toll_collection.is_empty() {
        intersection.emplace("toll_collection", toll_collection);
    }

    let cost = node.cost.unwrap_or_default();
    let transition = cost.transition_cost.unwrap_or_default();
    if transition.seconds > 0.0 {
        intersection.emplace("turn_duration", Json::Fixed(transition.seconds, 3));
    }
    if transition.cost > 0.0 {
        intersection.emplace("turn_weight", Json::Fixed(transition.cost, 3));
    }
    if let Some(next) = next {
        let elapsed = cost.elapsed_cost.unwrap_or_default();
        let next_elapsed = next
            .cost
            .unwrap_or_default()
            .elapsed_cost
            .unwrap_or_default();
        let secs = next_elapsed.seconds - elapsed.seconds;
        let weight = next_elapsed.cost - elapsed.cost;
        if secs > 0.0 {
            intersection.emplace("duration", Json::Fixed(secs, 3));
        }
        if weight > 0.0 {
            intersection.emplace("weight", Json::Fixed(weight, 3));
        }
    }
}

/// The first rest or service area the route could turn into here.
fn rest_stop(node: &Node, travel_mode: i32) -> Option<Map> {
    for intersecting in &node.intersecting_edge {
        let routeable = is_traversable_outbound(intersecting, travel_mode);
        let kind = if intersecting.r#use == trip_leg::Use::KRestAreaUse as i32 {
            "rest_area"
        } else if intersecting.r#use == trip_leg::Use::KServiceAreaUse as i32 {
            "service_area"
        } else {
            continue;
        };
        if !routeable {
            continue;
        }
        let mut rest_stop = Map::new();
        rest_stop.emplace("type", kind);
        let sign_text = intersecting
            .sign
            .as_ref()
            .map(destinations)
            .unwrap_or_default();
        if !sign_text.is_empty() {
            rest_stop.emplace("name", sign_text);
        }
        return Some(rest_stop);
    }
    None
}

/// `in`, `out`, `entry`, and `bearings`: the edge we leave by,
/// the other edges (except at the leg's start),
/// and the edge we came in on (except at the leg's start).
fn add_bearings(
    intersection: &mut Map,
    node: &Node,
    curr: Option<&Edge>,
    prev: Option<&Edge>,
    first_node: bool,
    arrive_maneuver: bool,
) -> Result<(), ValhallaError> {
    let mut edges = Vec::new();
    if !arrive_maneuver {
        let current = outgoing(curr)?;
        edges.push(IntersectionEdge {
            bearing: current.begin_heading,
            routeable: true,
            in_edge: false,
            out_edge: true,
        });
        if !first_node {
            for intersecting in &node.intersecting_edge {
                edges.push(IntersectionEdge {
                    bearing: intersecting.begin_heading,
                    routeable: is_traversable_outbound(intersecting, current.travel_mode),
                    in_edge: false,
                    out_edge: false,
                });
            }
        }
    }
    if !first_node {
        let prev = prev.ok_or_else(|| unknown("intersection without an incoming edge"))?;
        edges.push(IntersectionEdge {
            bearing: prev.end_heading.wrapping_add(180) % 360,
            // TODO (from mainline): a true U-turn should be routeable.
            routeable: arrive_maneuver,
            in_edge: true,
            out_edge: false,
        });
    }
    // Mainline uses std::sort, which libstdc++ runs as an insertion sort
    // (so stably) for up to 16 elements; a stable sort matches it there.
    edges.sort_by_key(|edge| edge.bearing);
    let (mut incoming_index, mut outgoing_index) = (0_u64, 0_u64);
    let mut bearings = Vec::with_capacity(edges.len());
    let mut entries = Vec::with_capacity(edges.len());
    for (n, edge) in edges.iter().enumerate() {
        if edge.in_edge {
            incoming_index = n as u64;
        }
        if edge.out_edge {
            outgoing_index = n as u64;
        }
        bearings.push(Json::U64(u64::from(edge.bearing)));
        entries.push(Json::Bool(edge.routeable));
    }
    if !first_node {
        intersection.emplace("in", incoming_index);
    }
    if !arrive_maneuver {
        intersection.emplace("out", outgoing_index);
    }
    intersection.emplace("entry", entries);
    intersection.emplace("bearings", bearings);
    Ok(())
}

/// `tunnel_name` and `classes`, from the edge leaving the intersection.
fn add_edge_classes(intersection: &mut Map, maneuver: &Maneuver, current: &Edge) {
    if current.tunnel && !current.tagged_value.is_empty() {
        for value in &current.tagged_value {
            if value.r#type == tagged_value::Type::KTunnel as i32 {
                intersection.emplace("tunnel_name", tagged_text(value));
            }
        }
    }

    let mut classes = Vec::new();
    if current.tunnel {
        classes.push(Json::str("tunnel"));
    }
    if maneuver.portions_toll || current.toll {
        classes.push(Json::str("toll"));
    }
    if current.road_class == RoadClass::KMotorway as i32 {
        classes.push(Json::str("motorway"));
    }
    if current.r#use == trip_leg::Use::KFerryUse as i32 {
        classes.push(Json::str("ferry"));
    }
    if current.destination_only {
        classes.push(Json::str("restricted"));
    }
    if !classes.is_empty() {
        intersection.emplace("classes", classes);
    }
}

/// `lanes`: the turn lanes of the edge leading into the intersection.
fn lanes(prev: &Edge) -> Vec<Json> {
    prev.turn_lanes
        .iter()
        .map(|turn_lane| {
            let mut lane = Map::new();
            let is_active = turn_lane.state == LaneState::KActive as i32;
            let is_valid = is_active || turn_lane.state == LaneState::KValid as i32;
            lane.emplace("active", is_active);
            lane.emplace("valid", is_valid);
            if turn_lane.state != LaneState::KInvalid as i32 {
                lane.emplace(
                    "valid_indication",
                    turn_lane_direction(turn_lane.active_direction),
                );
            }
            let indications: Vec<Json> =
                lane_indications(drive_on_right(prev), turn_lane.directions_mask)
                    .into_iter()
                    .map(Json::from)
                    .collect();
            lane.emplace("indications", indications);
            lane.into()
        })
        .collect()
}

/// The `intersections` of a step, and how many there are
/// (which the next step's maneuver type depends on).
pub(super) fn intersections(
    maneuver: &Maneuver,
    leg: &TripLeg,
    shape: &[LngLat],
    arrive_maneuver: bool,
    attributes: Attributes,
) -> Result<(Vec<Json>, u32), ValhallaError> {
    let mut out = Vec::new();
    let end = if arrive_maneuver {
        maneuver.end_path_index.saturating_add(1)
    } else {
        maneuver.end_path_index
    };
    for i in maneuver.begin_path_index..end {
        let index = i as usize;
        let mut intersection = Map::new();
        let node =
            trip::node(leg, index).ok_or_else(|| unknown("maneuver path index past the leg"))?;
        let curr = curr_edge(leg, index);
        let prev = prev_edge(leg, index);

        let shape_index = if arrive_maneuver {
            shape.len().saturating_sub(1)
        } else {
            outgoing(curr)?.begin_shape_index as usize
        };
        let ll = shape
            .get(shape_index)
            .ok_or_else(|| unknown("edge shape index past the leg's shape"))?;
        intersection.emplace(
            "location",
            vec![Json::Fixed(ll.lng, 6), Json::Fixed(ll.lat, 6)],
        );
        intersection.emplace("geometry_index", shape_index as u64);
        if attributes.node_admin_index {
            intersection.emplace("admin_index", node.admin_index);
        }
        if !arrive_maneuver && attributes.edge_is_urban {
            intersection.emplace("is_urban", outgoing(curr)?.is_urban);
        }

        let next = if i + 1 < end {
            trip::node(leg, index + 1)
        } else {
            None
        };
        add_costs(&mut intersection, node, next);

        // Rest stops passed by (not at the start of the leg, and not on arrival).
        if i > 0
            && !arrive_maneuver
            && let Some(rest_stop) = rest_stop(node, outgoing(curr)?.travel_mode)
        {
            intersection.emplace("rest_stop", rest_stop);
        }

        add_bearings(&mut intersection, node, curr, prev, i == 0, arrive_maneuver)?;
        if !arrive_maneuver {
            add_edge_classes(&mut intersection, maneuver, outgoing(curr)?);
        }
        if let Some(prev) = prev
            && has_usable_turn_lanes(prev)
        {
            intersection.emplace("lanes", lanes(prev));
        }
        out.push(intersection.into());
    }
    let count = u32::try_from(out.len()).unwrap_or(u32::MAX);
    Ok((out, count))
}
