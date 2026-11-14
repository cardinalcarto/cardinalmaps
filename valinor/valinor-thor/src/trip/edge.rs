//! The attributes of one edge of a trip, and of the edges that meet the path at its start node
//! (mainline's `AddTripEdge`, `AddIntersectingEdges`, `SetElevation`, and their helpers
//! in `triplegbuilder.cc`).
//!
//! Each attribute is written only if the request's [`AttributesController`] has it on,
//! and in proto3 terms exactly as mainline writes it: a message mainline creates
//! (a sign with no elements, a pronunciation, the speeds) is created here too,
//! so the two trips decode the same.

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares fractions exactly"
)]

use enumset::EnumSet;
use valhalla_graphtile::graph_tile::predicted_speeds::{FlowSource, select_speed};
use valhalla_graphtile::graph_tile::{
    DirectedEdge, EdgeInfo, EdgeName, GraphTile, LinguisticMap, NodeInfo, SignInfo, SignType,
    TaggedValue as TileTaggedValue, TaggedValueKind,
};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_graphtile::{Access, GraphId, RoadUse};
use valhalla_proto::trip_leg::{self, edge as trip_edge};
use valhalla_proto::{Pronunciation, StreetName, TaggedValue, TripSign, TripSignElement, TurnLane};
use valinor_sif::{AnyCosting, Costing, TimeInfo, TravelMode};

use crate::error::ThorError;
use crate::graph;
use crate::path::PathEdge;
use valinor_tyr::{Attribute, AttributesController};

/// The speed conversion (`kMetersPerSectoKPH`, a double).
const METERS_PER_SEC_TO_KPH: f64 = 3600.0 / 1000.0;

/// Mainline's "no elevation" grade (`kNoElevationData`).
const NO_ELEVATION_DATA: f32 = -500.0;

/// The lowest elevation mainline stores (`kMinElevation`); a stored zero decodes to it.
const MIN_ELEVATION: f32 = -500.0;

/// The access restriction index of "none" (`kInvalidRestriction`).
const INVALID_RESTRICTION: u8 = u8::MAX;

/// The times of week mainline reads free and constrained flow speeds at
/// (`kFreeFlowSecondsOfWeek` and `kConstrainedSecondsOfWeek`).
const FREE_FLOW_SECONDS_OF_WEEK: u64 = 0;
const CONSTRAINED_SECONDS_OF_WEEK: u64 = 28_800;

/// Mainline's flow masks (`kNoFlowMask` and so on).
const NO_FLOW_MASK: u8 = 0;
const FREE_FLOW_MASK: u8 = 1;
const CONSTRAINED_FLOW_MASK: u8 = 2;
const PREDICTED_FLOW_MASK: u8 = 4;
const CURRENT_FLOW_MASK: u8 = 8;

/// The value-carrying tagged values (mainline's `GetTags`, which leaves out linguistic ones),
/// in its multimap's order: by tag, and in name list order within a tag.
///
/// # Errors
///
/// A malformed value, or a tag Valhalla doesn't define (mainline's "Unknown tag type").
pub(crate) fn tags<'a>(edge_info: &EdgeInfo<'a>) -> Result<Vec<TileTaggedValue<'a>>, ThorError> {
    let mut tags = Vec::new();
    for value in edge_info.tagged_values() {
        let value = value?;
        match value.kind() {
            Some(TaggedValueKind::Linguistic) => {}
            Some(_) => tags.push(value),
            None => return Err(ThorError::UnknownTag(value.tag())),
        }
    }
    // A stable sort keeps the name list order among equal tags, as the multimap does.
    tags.sort_by_key(TileTaggedValue::tag);
    Ok(tags)
}

/// The language tag for a raw linguistic language (`GetTripLanguageTag`): the same number
/// for the 72 languages the proto lists, and unspecified for the others.
#[inline]
fn language_tag(raw: u8) -> i32 {
    /// The last language in mainline's map (`kCy`).
    const LAST_MAPPED: u8 = 72;
    if raw <= LAST_MAPPED {
        i32::from(raw)
    } else {
        0
    }
}

/// The raw values of "no language" and "no pronunciation" (`Language::kNone` and
/// `PronunciationAlphabet::kNone`).
const NO_LANGUAGE: u8 = 255;
const NO_ALPHABET: u8 = 5;

/// The proto alphabet for a raw one (`GetTripPronunciationAlphabet`: IPA, Katakana, Jeita,
/// and X-SAMPA keep their numbers; mainline has no answer for other values).
#[inline]
fn alphabet(raw: u8) -> i32 {
    if (1..=4).contains(&raw) {
        i32::from(raw)
    } else {
        0
    }
}

/// The language and pronunciation of a name or sign, from its linguistic record
/// (the shared part of `ProcessNonTaggedValue` and `PopulateSignElement`).
fn linguistics(
    map: &LinguisticMap<'_>,
    index: u8,
    language_tag_out: &mut i32,
    pronunciation_out: &mut Option<Pronunciation>,
) {
    let Some(record) = map.get(index) else {
        return;
    };
    if record.raw_language() != NO_LANGUAGE {
        *language_tag_out = language_tag(record.raw_language());
    }
    if record.raw_phonetic_alphabet() != NO_ALPHABET {
        *pronunciation_out = Some(Pronunciation {
            alphabet: alphabet(record.raw_phonetic_alphabet()),
            value: record.pronunciation_lossy().into_owned(),
        });
    }
}

/// A street name (`ProcessNonTaggedValue` and `ProcessTunnelBridgeTaggedValue`).
fn street_name(
    text: &[u8],
    is_route_number: bool,
    map: &LinguisticMap<'_>,
    index: u8,
) -> StreetName {
    let mut name = StreetName {
        value: String::from_utf8_lossy(text).into_owned(),
        is_route_number,
        ..StreetName::default()
    };
    linguistics(map, index, &mut name.language_tag, &mut name.pronunciation);
    name
}

/// An edge's names and their tags, with route numbers dropped for blind users
/// while more than one name remains (`GetNamesAndTypes(true)` and
/// `FilterUnneededStreetNumbers`).
fn names_and_types<'a>(
    edge_info: &EdgeInfo<'a>,
    blind: bool,
) -> Result<Vec<EdgeName<'a>>, ThorError> {
    let mut names = edge_info
        .names_and_types(true)
        .collect::<Result<Vec<_>, _>>()?;
    if blind && names.len() >= 2 {
        let mut i = 0;
        while i < names.len() {
            if names.get(i).is_some_and(EdgeName::is_route_number) && names.len() > 1 {
                names.remove(i);
            } else {
                i += 1;
            }
        }
    }
    Ok(names)
}

/// A sign element (`PopulateSignElement`).
fn sign_element(index: u8, sign: &SignInfo<'_>, map: &LinguisticMap<'_>) -> TripSignElement {
    let mut element = TripSignElement {
        text: String::from_utf8_lossy(sign.text()).into_owned(),
        is_route_number: sign.is_route_number(),
        ..TripSignElement::default()
    };
    linguistics(
        map,
        index,
        &mut element.language_tag,
        &mut element.pronunciation,
    );
    element
}

/// Adds an edge's signs to a trip sign, as the controller allows (`AddSignInfo`).
fn add_sign_info(
    controller: &AttributesController,
    signs: &[SignInfo<'_>],
    map: &LinguisticMap<'_>,
    trip_sign: &mut TripSign,
) {
    for (index, sign) in (0_u8..).zip(signs) {
        let (attribute, list) = match sign.sign_type() {
            Ok(SignType::ExitNumber) => {
                (Attribute::EdgeSignExitNumber, &mut trip_sign.exit_numbers)
            }
            Ok(SignType::ExitBranch) => (
                Attribute::EdgeSignExitBranch,
                &mut trip_sign.exit_onto_streets,
            ),
            Ok(SignType::ExitToward) => (
                Attribute::EdgeSignExitToward,
                &mut trip_sign.exit_toward_locations,
            ),
            Ok(SignType::ExitName) => (Attribute::EdgeSignExitName, &mut trip_sign.exit_names),
            Ok(SignType::GuideBranch) => (
                Attribute::EdgeSignGuideBranch,
                &mut trip_sign.guide_onto_streets,
            ),
            Ok(SignType::GuideToward) => (
                Attribute::EdgeSignGuideToward,
                &mut trip_sign.guide_toward_locations,
            ),
            Ok(SignType::GuidanceViewJunction) => (
                Attribute::EdgeSignGuidanceViewJunction,
                &mut trip_sign.guidance_view_junctions,
            ),
            Ok(SignType::GuidanceViewSignboard) => (
                Attribute::EdgeSignGuidanceViewSignboard,
                &mut trip_sign.guidance_view_signboards,
            ),
            _ => continue,
        };
        if controller.get(attribute) {
            list.push(sign_element(index, sign, map));
        }
    }
}

/// The trip's road use for an edge's (`GetTripLegUse`): the same number,
/// except that sidewalks are footways and uses Odin doesn't know are roads.
fn trip_use(edge: &DirectedEdge) -> i32 {
    match edge.road_use() {
        Ok(RoadUse::Sidewalk) => trip_leg::Use::KFootwayUse as i32,
        Ok(RoadUse::Platform) | Err(_) => trip_leg::Use::KRoadUse as i32,
        Ok(road_use) => i32::from(road_use as u8),
    }
}

/// The traversability of an edge for an access mode, given its access in the direction of
/// travel and against it (mainline's four-way test).
fn traversability(along: bool, against: bool) -> i32 {
    (match (along, against) {
        (true, true) => trip_leg::Traversability::KBoth,
        (true, false) => trip_leg::Traversability::KForward,
        (false, true) => trip_leg::Traversability::KBackward,
        (false, false) => trip_leg::Traversability::KNone,
    }) as i32
}

/// The access mode of a travel mode, for traversability (`kAccess` in `AddTripEdge`).
fn access_of(mode: TravelMode) -> Access {
    match mode {
        TravelMode::Bicycle => Access::Bicycle,
        TravelMode::Drive => Access::Auto,
        TravelMode::Pedestrian | TravelMode::PublicTransit => Access::Pedestrian,
    }
}

/// Everything [`add_trip_edge`] reads about an edge of the path.
pub(crate) struct TripEdgeInput<'a, T: GraphTile> {
    pub(crate) controller: &'a AttributesController,
    pub(crate) edge_id: GraphId,
    pub(crate) path_edge: &'a PathEdge,
    pub(crate) travel_type: u8,
    pub(crate) costing: &'a AnyCosting,
    pub(crate) edge: &'a DirectedEdge,
    pub(crate) drive_on_right: bool,
    pub(crate) tile: &'a T,
    pub(crate) time_info: &'a TimeInfo,
    /// The start node's index in its tile, and whether it has a junction name.
    pub(crate) start_node_index: u32,
    pub(crate) has_junction_name: bool,
    pub(crate) start_tile: &'a T,
    pub(crate) blind: bool,
    pub(crate) edge_info: &'a EdgeInfo<'a>,
    pub(crate) tags: &'a [TileTaggedValue<'a>],
    /// The edge's levels (start and end of each range) and their precision,
    /// as `EdgeInfo::levels` returns them.
    pub(crate) levels: &'a (Vec<(f32, f32)>, u32),
}

/// An edge of the trip (mainline's `AddTripEdge`), without the parts that depend on the shape
/// (lengths, shape indexes, headings, and elevation), which the leg builder adds.
#[expect(
    clippy::too_many_lines,
    reason = "mainline's function, attribute by attribute"
)]
pub(crate) fn add_trip_edge<T: GraphTile>(
    input: &TripEdgeInput<'_, T>,
) -> Result<trip_leg::Edge, ThorError> {
    let c = input.controller;
    let edge = input.edge;
    let tile = input.tile;
    let edge_info = input.edge_info;
    let mode = input.path_edge.mode;
    let index = u32::try_from(input.edge_id.feature_index()).unwrap_or(u32::MAX);
    let mut out = trip_leg::Edge::default();

    if c.get(Attribute::EdgeNames) {
        let names = names_and_types(edge_info, input.blind)?;
        let map = edge_info.linguistic_map()?;
        for (name_index, name) in (0_u8..).zip(&names) {
            match name.tag() {
                0 => out.name.push(street_name(
                    name.text(),
                    name.is_route_number(),
                    &map,
                    name_index,
                )),
                t if t == TaggedValueKind::Tunnel as u8 || t == TaggedValueKind::Bridge as u8 => {
                    out.tunnel_name.push(street_name(
                        name.text(),
                        name.is_route_number(),
                        &map,
                        name_index,
                    ));
                }
                _ => {}
            }
        }
    }

    if c.get(Attribute::EdgeTaggedValues) {
        out.tagged_value = input
            .tags
            .iter()
            .map(|t| TaggedValue {
                value: t.value().to_vec(),
                r#type: i32::from(t.tag()),
            })
            .collect();
    }

    if edge.has_sign() {
        let (signs, map) = tile.get_signs_with_linguistics(index, false)?;
        if !signs.is_empty() {
            add_sign_info(c, &signs, &map, out.sign.get_or_insert_default());
        }
    }

    if input.has_junction_name {
        let (signs, map) = input
            .start_tile
            .get_signs_with_linguistics(input.start_node_index, true)?;
        if !signs.is_empty() {
            let trip_sign = out.sign.get_or_insert_default();
            for (sign_index, sign) in (0_u8..).zip(&signs) {
                if matches!(sign.sign_type(), Ok(SignType::JunctionName))
                    && c.get(Attribute::EdgeSignJunctionName)
                {
                    trip_sign
                        .junction_names
                        .push(sign_element(sign_index, sign, &map));
                }
            }
        }
    }

    if edge.has_turn_lanes()
        && let Some(lanes) = tile.get_turn_lanes(index)?
    {
        for mask in lanes.masks() {
            out.turn_lanes.push(TurnLane {
                directions_mask: u32::from(mask?.raw()),
                ..TurnLane::default()
            });
        }
    }

    if c.get(Attribute::EdgeRoadClass) {
        out.road_class = i32::from(edge.classification() as u8);
    }

    if c.get(Attribute::EdgeSpeed) {
        let secs = if mode == TravelMode::PublicTransit {
            input.path_edge.elapsed_cost.secs
        } else {
            input
                .costing
                .edge_cost(edge, input.edge_id, tile, input.time_info.seconds_of_week())?
                .cost
                .secs
        };
        // `length() / secs` divides in single precision; the conversion is a double.
        #[expect(clippy::cast_precision_loss, reason = "mainline converts to float")]
        let speed = f64::from(edge.length() as f32 / secs) * METERS_PER_SEC_TO_KPH;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the proto field is a float"
        )]
        let speed = speed as f32;
        out.speed = speed;
    }

    if c.get(Attribute::EdgeSpeedType) {
        out.speed_type = i32::from(edge.speed_type() as u8);
    }

    if c.get(Attribute::EdgeSpeedsFaded) || c.get(Attribute::EdgeSpeedsNonFaded) {
        // Faded speeds need live traffic (`speed_valid()`), which Valinor doesn't use.
        if c.get(Attribute::EdgeSpeedsNonFaded) {
            let speeds = non_faded_speeds(edge, input, index)?;
            out.has_speeds_non_faded = Some(trip_edge::HasSpeedsNonFaded::SpeedsNonFaded(speeds));
        }
    }

    if c.get(Attribute::EdgeCountryCrossing) {
        out.country_crossing = edge.country_crossing();
    }
    if c.get(Attribute::EdgeForward) {
        out.forward = edge.edge_info_is_forward();
    }
    if c.get(Attribute::EdgeTrafficSignal) {
        out.traffic_signal = edge.has_traffic_signal();
    }
    if c.get(Attribute::EdgeHovType) {
        out.hov_type = edge.hov_type().map_or(0, |h| i32::from(h as u8));
    }
    if c.get(Attribute::EdgeLevels) {
        out.level_precision = input.levels.1.max(1);
        out.levels = input
            .levels
            .0
            .iter()
            .map(|&(start, end)| trip_edge::Level { start, end })
            .collect();
    }

    if c.get(Attribute::EdgeTraversability) {
        let access = access_of(mode);
        let (forward, reverse) = (
            edge.forward_access().contains(access),
            edge.reverse_access().contains(access),
        );
        // An edge stored against its edge info's direction swaps the two.
        out.traversability = if edge.edge_info_is_forward() {
            traversability(forward, reverse)
        } else {
            traversability(reverse, forward)
        };
    }

    let restriction_index = input.path_edge.restriction_index;
    if !edge.access_restrictions().is_empty() && restriction_index != INVALID_RESTRICTION {
        let restrictions = tile.get_access_restrictions(index, input.costing.base().access_mode());
        let restriction = restrictions
            .get(usize::from(restriction_index))
            .ok_or(ThorError::MissingAccessRestriction(input.edge_id))?;
        out.restriction = Some(trip_leg::Restriction {
            r#type: u32::from(restriction.raw_restriction_type()),
        });
    }
    out.has_time_restrictions = restriction_index != INVALID_RESTRICTION;

    if c.get(Attribute::EdgeUse) {
        out.r#use = trip_use(edge);
    }
    out.toll = edge.toll() && c.get(Attribute::EdgeToll);
    out.unpaved = edge.is_unpaved() && c.get(Attribute::EdgeUnpaved);
    out.tunnel = edge.is_tunnel() && c.get(Attribute::EdgeTunnel);
    out.bridge = edge.is_bridge() && c.get(Attribute::EdgeBridge);
    out.roundabout = edge.roundabout() && c.get(Attribute::EdgeRoundabout);
    out.internal_intersection =
        edge.is_intersection_internal() && c.get(Attribute::EdgeInternalIntersection);
    if c.get(Attribute::EdgeDriveOnRight) {
        out.drive_on_left = !input.drive_on_right;
    }
    if c.get(Attribute::EdgeSurface) {
        out.surface = i32::from(edge.surface() as u8);
    }
    if c.get(Attribute::EdgeCurvature) {
        out.curvature = u32::from(edge.curvature());
    }
    out.destination_only = edge.dest_only() && c.get(Attribute::EdgeDestinationOnly);
    out.indoor = edge.is_indoor() && c.get(Attribute::EdgeIndoor);

    set_mode(&mut out, c, mode, edge, input.travel_type);

    if c.get(Attribute::EdgeId) {
        out.id = input.edge_id.value();
    }
    if c.get(Attribute::EdgeWayId) {
        out.way_id = edge_info.way_id();
    }
    if (c.get(Attribute::EdgeBeginOsmNodeId) || c.get(Attribute::EdgeEndOsmNodeId))
        && let Some(ids) = edge_info.osm_node_ids()?
    {
        let ids = ids.collect::<Result<Vec<u64>, _>>()?;
        if let (Some(&first), Some(&last)) = (ids.first(), ids.last()) {
            let forward = edge.edge_info_is_forward();
            if c.get(Attribute::EdgeBeginOsmNodeId) {
                out.has_begin_osm_node_id =
                    Some(trip_edge::HasBeginOsmNodeId::BeginOsmNodeId(if forward {
                        first
                    } else {
                        last
                    }));
            }
            if c.get(Attribute::EdgeEndOsmNodeId) {
                out.has_end_osm_node_id =
                    Some(trip_edge::HasEndOsmNodeId::EndOsmNodeId(if forward {
                        last
                    } else {
                        first
                    }));
            }
        }
    }

    if c.get(Attribute::EdgeWeightedGrade) {
        out.weighted_grade = (f32::from(edge.weighted_grade()) - 6.0) / 0.6;
    }
    let has_elevation = tile.header().has_elevation();
    if c.get(Attribute::EdgeMaxUpwardGrade) {
        out.max_upward_grade = if has_elevation {
            i32::from(edge.max_up_slope())
        } else {
            no_elevation_grade()
        };
    }
    if c.get(Attribute::EdgeMaxDownwardGrade) {
        out.max_downward_grade = if has_elevation {
            i32::from(edge.max_down_slope())
        } else {
            no_elevation_grade()
        };
    }
    if c.get(Attribute::EdgeMeanElevation) {
        // A float stored in an int32 field: truncated.
        #[expect(clippy::cast_possible_truncation, reason = "mainline truncates")]
        let mean = edge_info.mean_elevation().unwrap_or(MIN_ELEVATION) as i32;
        out.mean_elevation = mean;
    }
    if c.get(Attribute::EdgeLaneCount) {
        out.lane_count = u32::from(edge.lane_count());
    }
    if edge.has_lane_connectivity() && c.get(Attribute::EdgeLaneConnectivity) {
        out.lane_connectivity = tile
            .get_lane_connectivity(index)
            .iter()
            .map(|l| trip_leg::LaneConnectivity {
                from_way_id: l.from(),
                to_lanes: l.to_lanes().to_string(),
                from_lanes: l.from_lanes().to_string(),
            })
            .collect();
    }
    if c.get(Attribute::EdgeCycleLane) {
        out.cycle_lane = i32::from(edge.cycle_lane() as u8);
    }
    if c.get(Attribute::EdgeBicycleNetwork) {
        out.bicycle_network = edge.is_bike_network();
    }
    if c.get(Attribute::EdgeSacScale) {
        out.sac_scale = edge.sac_scale().map_or(0, |s| i32::from(s as u8));
    }
    if c.get(Attribute::EdgeShoulder) {
        out.shoulder = edge.has_shoulder();
    }
    if c.get(Attribute::EdgeSidewalk) {
        out.sidewalk = (match (edge.has_sidewalk_left(), edge.has_sidewalk_right()) {
            (true, true) => trip_leg::Sidewalk::KBothSides,
            (true, false) => trip_leg::Sidewalk::KLeft,
            (false, true) => trip_leg::Sidewalk::KRight,
            (false, false) => trip_leg::Sidewalk::KNoSidewalk,
        }) as i32;
    }
    if c.get(Attribute::EdgeDensity) {
        out.density = u32::from(edge.density());
    }
    if c.get(Attribute::EdgeIsUrban) {
        out.is_urban = edge.density() > 8;
    }
    if c.get(Attribute::EdgeSpeedLimit) {
        out.speed_limit = u32::from(edge_info.speed_limit());
    }
    if c.get(Attribute::EdgeConditionalSpeedLimits) {
        for limit in edge_info.conditional_speed_limits() {
            let limit = limit?;
            let td = limit.time_domain();
            out.conditional_speed_limits
                .push(trip_leg::ConditionalSpeedLimit {
                    speed_limit: u32::from(limit.speed()),
                    condition: Some(trip_leg::TimeDomain {
                        day_dow_type: td.day_type() as i32,
                        dow_mask: u32::from(td.dow()),
                        begin_hrs: u32::from(td.begin_hrs()),
                        begin_mins: u32::from(td.begin_mins()),
                        begin_month: u32::from(td.begin_month()),
                        begin_day_dow: u32::from(td.begin_day_dow()),
                        begin_week: u32::from(td.begin_week()),
                        end_hrs: u32::from(td.end_hrs()),
                        end_mins: u32::from(td.end_mins()),
                        end_month: u32::from(td.end_month()),
                        end_day_dow: u32::from(td.end_day_dow()),
                        end_week: u32::from(td.end_week()),
                    }),
                });
        }
    }
    if c.get(Attribute::EdgeDefaultSpeed) {
        out.default_speed = f32::from(edge.speed());
    }
    if c.get(Attribute::EdgeTruckSpeed) {
        out.truck_speed = f32::from(edge.truck_speed());
    }
    out.truck_route = edge.truck_route() && c.get(Attribute::EdgeTruckRoute);
    // Transit route info needs a transit trip, which routes without transit never have.
    Ok(out)
}

/// `kNoElevationData` stored in an int32 grade.
#[inline]
fn no_elevation_grade() -> i32 {
    #[expect(clippy::cast_possible_truncation, reason = "-500 is exact")]
    let grade = NO_ELEVATION_DATA as i32;
    grade
}

/// The travel mode and vehicle, pedestrian, or bicycle type of an edge
/// (bicycles walk where they must dismount or on steps).
fn set_mode(
    out: &mut trip_leg::Edge,
    c: &AttributesController,
    mode: TravelMode,
    edge: &DirectedEdge,
    travel_type: u8,
) {
    match mode {
        TravelMode::Bicycle => {
            if edge.dismount() || matches!(edge.road_use(), Ok(RoadUse::Steps)) {
                if c.get(Attribute::EdgeTravelMode) {
                    out.travel_mode = valhalla_proto::TravelMode::KPedestrian as i32;
                }
                if c.get(Attribute::EdgePedestrianType) {
                    out.pedestrian_type = valhalla_proto::PedestrianType::KFoot as i32;
                }
            } else {
                if c.get(Attribute::EdgeTravelMode) {
                    out.travel_mode = valhalla_proto::TravelMode::KBicycle as i32;
                }
                if c.get(Attribute::EdgeBicycleType) {
                    // `GetTripLegBicycleType`: road, cross, hybrid, mountain, else road.
                    out.bicycle_type = if travel_type <= 3 {
                        i32::from(travel_type)
                    } else {
                        0
                    };
                }
            }
        }
        TravelMode::Drive => {
            if c.get(Attribute::EdgeTravelMode) {
                out.travel_mode = valhalla_proto::TravelMode::KDrive as i32;
            }
            if c.get(Attribute::EdgeVehicleType) {
                // `GetTripLegVehicleType`: car through motor scooter, else car.
                out.vehicle_type = if travel_type <= 4 {
                    i32::from(travel_type)
                } else {
                    0
                };
            }
        }
        TravelMode::Pedestrian => {
            if c.get(Attribute::EdgeTravelMode) {
                out.travel_mode = valhalla_proto::TravelMode::KPedestrian as i32;
            }
            if c.get(Attribute::EdgePedestrianType) {
                // `GetTripLegPedestrianType`: foot, wheelchair, blind, else foot.
                out.pedestrian_type = if travel_type <= 2 {
                    i32::from(travel_type)
                } else {
                    0
                };
            }
        }
        TravelMode::PublicTransit => {
            if c.get(Attribute::EdgeTravelMode) {
                out.travel_mode = valhalla_proto::TravelMode::KTransit as i32;
            }
        }
    }
}

/// The edge's speeds by source, without fading in live traffic
/// (the `set_speeds(…, false)` lambda of `AddTripEdge`).
///
/// Without live traffic, current flow never applies; predicted, constrained, and free flow
/// speeds are written if the edge has them, and the speed without any flow always is.
fn non_faded_speeds<T: GraphTile>(
    edge: &DirectedEdge,
    input: &TripEdgeInput<'_, T>,
    index: u32,
) -> Result<trip_leg::Speeds, ThorError> {
    let second_of_week = input.time_info.seconds_of_week().map(u64::from);
    let tile = input.tile;
    let edge_index = usize::try_from(index).unwrap_or(usize::MAX);
    // `get_speed(flow_mask, faded = false, second_of_week)`.
    let get_speed = |flow_mask: u8, seconds: Option<u64>| -> Result<Option<f32>, ThorError> {
        let seconds = match flow_mask {
            CURRENT_FLOW_MASK => Some(0),
            CONSTRAINED_FLOW_MASK => Some(CONSTRAINED_SECONDS_OF_WEEK),
            FREE_FLOW_MASK => Some(FREE_FLOW_SECONDS_OF_WEEK),
            _ => seconds,
        };
        let selected = select_speed(
            edge,
            EnumSet::<FlowSource>::from_repr_truncated(flow_mask),
            seconds,
            false,
            |s| {
                tile.get_predicted_speed(edge_index, s)
                    .ok_or(ThorError::MissingSpeedProfile(input.edge_id))
            },
        )?;
        let sources = selected.flow_sources.as_repr();
        if sources & flow_mask != 0 || flow_mask == NO_FLOW_MASK {
            #[expect(clippy::cast_precision_loss, reason = "a speed in kph")]
            let kph = selected.kph as f32;
            Ok(Some(kph))
        } else {
            Ok(None)
        }
    };
    let mut speeds = trip_leg::Speeds::default();
    if let Some(s) = get_speed(CURRENT_FLOW_MASK, second_of_week)? {
        speeds.current_flow = s;
    }
    if let Some(s) = get_speed(PREDICTED_FLOW_MASK, second_of_week)?
        && edge.has_predicted_speed()
    {
        speeds.predicted_flow = s;
    }
    if let Some(s) = get_speed(CONSTRAINED_FLOW_MASK, second_of_week)?
        && edge.constrained_flow_speed() > 0
    {
        speeds.constrained_flow = s;
    }
    if let Some(s) = get_speed(FREE_FLOW_MASK, second_of_week)?
        && edge.free_flow_speed() > 0
    {
        speeds.free_flow = s;
    }
    if let Some(s) = get_speed(NO_FLOW_MASK, second_of_week)? {
        speeds.no_flow = s;
    }
    Ok(speeds)
}

/// The elevation along the used part of an edge (mainline's `SetElevation`):
/// its sampling interval and the samples.
///
/// A partial edge is sampled again at an interval that fits its length;
/// a whole edge keeps its stored samples.
pub(crate) fn set_elevation<P: GraphTileProvider>(
    out: &mut trip_leg::Edge,
    start_pct: f64,
    end_pct: f64,
    start_node: &NodeInfo,
    edge: &DirectedEdge,
    edge_info: &EdgeInfo<'_>,
    provider: &P,
) -> Result<(), ThorError> {
    /// Mainline's `kEpsilon` (a float), added to the end distance.
    const EPSILON: f32 = 0.000_001;
    let h1 = start_node.elevation().unwrap_or(MIN_ELEVATION);
    let encoded = edge_info.encoded_elevation(edge.length())?;
    let interval = encoded.interval();
    let end_node = edge.end_node_id();
    let h2 = match graph::tile(provider, end_node)? {
        Some(end_tile) => graph::node(&end_tile, end_node)?
            .elevation()
            .unwrap_or(MIN_ELEVATION),
        None => h1,
    };
    let elevation = encoded.decode(h1, h2, edge.edge_info_is_forward());
    let find_elevation = |d: f64| -> f32 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mainline truncates the sample index to a uint32"
        )]
        let index = (d / interval) as u32 as usize;
        if index >= elevation.len().saturating_sub(1) {
            return elevation.last().copied().unwrap_or(h1);
        }
        #[expect(clippy::cast_precision_loss, reason = "a small index")]
        let pct = d / interval - index as f64;
        let (Some(&a), Some(&b)) = (elevation.get(index), elevation.get(index + 1)) else {
            return h1;
        };
        #[expect(clippy::cast_possible_truncation, reason = "mainline returns a float")]
        let value = (f64::from(a) * (1.0 - pct) + f64::from(b) * pct) as f32;
        value
    };

    let length = f64::from(edge.length());
    if ((0.0 < start_pct && start_pct < 1.0) || (0.0 < end_pct && end_pct < 1.0))
        && start_pct != end_pct
    {
        let partial_length = length * (end_pct - start_pct);
        let new_interval = sampling_interval(partial_length);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the proto field is a float"
        )]
        let stored_interval = new_interval as f32;
        out.elevation_sampling_interval = stored_interval;
        let mut d = length * start_pct;
        let end_distance = length * end_pct;
        while d < end_distance + f64::from(EPSILON) {
            out.elevation.push(find_elevation(d));
            d += new_interval;
        }
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the proto field is a float"
        )]
        let stored_interval = interval as f32;
        out.elevation_sampling_interval = stored_interval;
        out.elevation.extend_from_slice(&elevation);
    }
    Ok(())
}

/// The elevation sampling interval for a length (`midgard::sampling_interval`):
/// equal steps of at most 32 meters.
fn sampling_interval(length: f64) -> f64 {
    /// `kMaxEdgeElevationSampling`.
    const MAX_SAMPLING: f64 = 32.0;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "mainline truncates the count to a uint32"
    )]
    let sample_count = (length / MAX_SAMPLING) as u32;
    length / (f64::from(sample_count) + 1.0)
}

/// One edge that meets the path at a node (mainline's `AddTripIntersectingEdge`).
#[expect(clippy::too_many_arguments, reason = "mainline's signature")]
fn intersecting_edge<T: GraphTile>(
    c: &AttributesController,
    tile: &T,
    edge: &DirectedEdge,
    prev_edge: Option<&DirectedEdge>,
    local_edge_index: u8,
    node: &NodeInfo,
    intersecting: &DirectedEdge,
    intersecting_index: u32,
    blind: bool,
) -> Result<trip_leg::IntersectingEdge, ThorError> {
    let mut out = trip_leg::IntersectingEdge::default();
    if c.get(Attribute::NodeIntersectingEdgeBeginHeading) {
        out.begin_heading = node.heading(local_edge_index).map_or(0, u32::from);
    }
    let fwd = intersecting.forward_access();
    let rev = intersecting.reverse_access();
    if c.get(Attribute::NodeIntersectingEdgeWalkability) {
        out.walkability = traversability(
            fwd.contains(Access::Pedestrian),
            rev.contains(Access::Pedestrian),
        );
    }
    if c.get(Attribute::NodeIntersectingEdgeCyclability) {
        out.cyclability =
            traversability(fwd.contains(Access::Bicycle), rev.contains(Access::Bicycle));
    }
    if c.get(Attribute::NodeIntersectingEdgeDriveability) {
        out.driveability = node
            .local_driveability(local_edge_index)
            .map_or(0, |t| i32::from(t as u8));
    }
    if c.get(Attribute::NodeIntersectingEdgeFromEdgeNameConsistency) {
        out.prev_name_consistency =
            prev_edge.is_some_and(|p| p.name_consistency().contains(local_edge_index));
    }
    if c.get(Attribute::NodeIntersectingEdgeToEdgeNameConsistency) {
        out.curr_name_consistency = edge.name_consistency().contains(local_edge_index);
    }
    if c.get(Attribute::EdgeNames) {
        let edge_info = tile.get_edge_info(intersecting)?;
        let names = names_and_types(&edge_info, blind)?;
        let map = edge_info.linguistic_map()?;
        for (name_index, name) in (0_u8..).zip(&names) {
            if name.tag() == 0 {
                out.name.push(street_name(
                    name.text(),
                    name.is_route_number(),
                    &map,
                    name_index,
                ));
            }
        }
    }
    if c.get(Attribute::NodeIntersectingEdgeUse) {
        out.r#use = trip_use(intersecting);
    }
    if c.get(Attribute::NodeIntersectingEdgeRoadClass) {
        out.road_class = i32::from(intersecting.classification() as u8);
    }
    if c.get(Attribute::NodeIntersectingEdgeLaneCount) {
        out.lane_count = u32::from(intersecting.lane_count());
    }
    if c.get(Attribute::NodeIntersectingEdgeSignInfo) && intersecting.has_sign() {
        let (signs, map) = tile.get_signs_with_linguistics(intersecting_index, false)?;
        if !signs.is_empty() {
            add_sign_info(c, &signs, &map, out.sign.get_or_insert_default());
        }
    }
    Ok(out)
}

/// Whether an edge at a node is left out of the intersecting edges:
/// shortcuts, the edges in and out of the path, and roads under construction.
fn skip_intersecting(edge: &DirectedEdge, candidate: &DirectedEdge, prior_opp: Option<u8>) -> bool {
    candidate.is_shortcut()
        || Some(candidate.local_edge_index()) == prior_opp
        || candidate.local_edge_index() == edge.local_edge_index()
        || matches!(candidate.road_use(), Ok(RoadUse::Construction))
}

/// The edges that meet the path at its node, on this level and the others it connects to
/// (mainline's `AddIntersectingEdges`).
///
/// `prior_opp` is the local index of the previous path edge's opposing edge,
/// which is skipped (`None` at the start of the path, where mainline's index is `-1`).
#[expect(clippy::too_many_arguments, reason = "mainline's signature")]
pub(crate) fn add_intersecting_edges<P: GraphTileProvider>(
    c: &AttributesController,
    provider: &P,
    start_tile: &P::TileHandle,
    node: &NodeInfo,
    edge: &DirectedEdge,
    prev_edge: Option<&DirectedEdge>,
    prior_opp: Option<u8>,
    trip_node: &mut trip_leg::Node,
    blind: bool,
) -> Result<(), ThorError> {
    let first = node.edge_index();
    for (idx, candidate) in (first..).zip(start_tile.get_outbound_edges_from_node(node)) {
        // On the edge's own level, the edges a shortcut on the path supersedes are skipped too.
        if skip_intersecting(edge, candidate, prior_opp)
            || (edge.is_shortcut()
                && graph::shortcut_mask(edge) & graph::superseded_mask(candidate) != 0)
        {
            continue;
        }
        trip_node.intersecting_edge.push(intersecting_edge(
            c,
            start_tile,
            edge,
            prev_edge,
            candidate.local_edge_index(),
            node,
            candidate,
            idx,
            blind,
        )?);
    }
    for transition in start_tile.get_transitions(node) {
        let end = transition.corresponding_end_node_id();
        let Some(end_tile) = graph::tile(provider, end)? else {
            continue;
        };
        let node2 = graph::node(&end_tile, end)?;
        let first = node2.edge_index();
        for (idx, candidate) in (first..).zip(end_tile.get_outbound_edges_from_node(node2)) {
            if skip_intersecting(edge, candidate, prior_opp) {
                continue;
            }
            trip_node.intersecting_edge.push(intersecting_edge(
                c,
                &end_tile,
                edge,
                prev_edge,
                candidate.local_edge_index(),
                node2,
                candidate,
                idx,
                blind,
            )?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_enums_as_mainline() {
        assert_eq!(language_tag(1), 1);
        assert_eq!(language_tag(72), 72);
        // Tamil and Malay aren't in mainline's map.
        assert_eq!(language_tag(73), 0);
        assert_eq!(language_tag(74), 0);
        assert_eq!(alphabet(1), 1);
        assert_eq!(alphabet(4), 4);
        assert_eq!(alphabet(0), 0);
        assert_eq!(traversability(true, false), 1);
        assert_eq!(traversability(false, true), 2);
        assert_eq!(traversability(true, true), 3);
        assert_eq!(traversability(false, false), 0);
        assert_eq!(access_of(TravelMode::PublicTransit), Access::Pedestrian);
    }

    #[test]
    fn samples_elevation_like_mainline() {
        assert_eq!(sampling_interval(10.0), 10.0);
        assert_eq!(sampling_interval(64.0), 64.0 / 3.0);
        assert_eq!(sampling_interval(100.0), 25.0);
    }
}
