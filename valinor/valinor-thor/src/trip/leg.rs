//! One leg of a trip (mainline's `TripLegBuilder::Build`): the leg's locations, a node per edge
//! of the path (and one at the end) with the edge's attributes and the edges that meet it,
//! the shape with its partial edges trimmed, and the leg's admins, bounding box, and summary.
//!
//! The order of the steps, and the precision of each (mainline mixes floats and doubles),
//! follow mainline, since they decide the shape and the values Odin reads.
//! A few results that look odd are mainline's, and are kept because Odin and clients see them:
//!
//! - Each location after the origin is checked against the edge index of the path,
//!   so a destination with no intermediate locations before it gets the shape index
//!   and distance of the end of the first edge (its `leg_shape_index` is 0 when it comes in).
//! - An `arrive_by` intermediate location at a node gets the start of its edge instead.
//! - Edges disabled by the request's filters still decide which messages exist
//!   (a sign with no elements, for one).

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares levels and fractions exactly"
)]

use jiff::Timestamp;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, IntersectionType};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_graphtile::{GraphId, RoadClass, RoadUse};
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::options::DateTimeType;
use valhalla_proto::search_filter::HasLevel;
use valhalla_proto::trip_leg::{self, PathCost};
use valhalla_proto::{BoundingBox, LatLng, LevelChange, Location, Options, Summary, TripLeg};
use valinor_sif::datetime::time_zone_name;
use valinor_sif::{AnyCosting, Costing, PedestrianType, TimeInfo, TravelMode, recost_forward};

use super::attributes::{Attribute, AttributesController, Category};
use super::edge::{TripEdgeInput, add_intersecting_edges, add_trip_edge, set_elevation, tags};
use super::shape::{
    bounding_box, encode_polyline, heading_along_polyline, heading_at_end_of_polyline, trim_shape,
};
use crate::error::ThorError;
use crate::geo::PointLL;
use crate::graph;
use crate::path::{EdgeTrim, Leg};
use crate::search::point_of;
use crate::time::{graph_id, location_time_info};

/// Mainline's "no level" (`kMaxLevel`).
const MAX_LEVEL: f32 = f32::MAX;

/// What every leg of a request shares.
pub(crate) struct LegContext<'a, P> {
    pub(crate) provider: &'a P,
    pub(crate) options: &'a Options,
    pub(crate) controller: &'a AttributesController,
    pub(crate) costing: &'a AnyCosting,
    /// The current time, for `current` times.
    pub(crate) now: Timestamp,
}

/// The admins of a leg, in the order the leg reaches them (mainline's `GetAdminIndex`).
#[derive(Default)]
struct Admins {
    list: Vec<[String; 4]>,
}

impl Admins {
    /// The index of an admin (country code and name, state code and name), added if new.
    fn index(&mut self, admin: [String; 4]) -> u32 {
        let index = if let Some(i) = self.list.iter().position(|a| *a == admin) {
            i
        } else {
            self.list.push(admin);
            self.list.len() - 1
        };
        u32::try_from(index).unwrap_or(u32::MAX)
    }
}

/// The admin of a node in its tile.
fn admin_of<T: GraphTile>(tile: &T, admin_index: u16) -> Result<[String; 4], ThorError> {
    let info = tile.get_admin_info(usize::from(admin_index))?;
    Ok([
        info.country_iso.into_owned(),
        info.country_text.into_owned(),
        info.state_iso.into_owned(),
        info.state_text.into_owned(),
    ])
}

/// A protobuf point.
fn lat_lng(p: PointLL) -> LatLng {
    LatLng {
        has_lat: Some(HasLat::Lat(p.lat)),
        has_lng: Some(HasLng::Lng(p.lng)),
    }
}

/// A cost as the trip stores it.
fn cost(secs: f32, cost: f32) -> trip_leg::Cost {
    trip_leg::Cost {
        seconds: f64::from(secs),
        cost: f64::from(cost),
    }
}

/// Keeps only the candidate edge of a location that the path uses (`RemovePathEdges`):
/// it moves to the front, and the others are dropped. Filtered edges are kept.
fn remove_path_edges(location: &mut Location, edge_id: GraphId) -> Result<(), ThorError> {
    let correlation = location
        .correlation
        .as_mut()
        .ok_or(ThorError::MissingPathCandidate)?;
    let pos = correlation
        .edges
        .iter()
        .position(|e| e.graph_id == edge_id.value())
        .ok_or(ThorError::MissingPathCandidate)?;
    if correlation.edges.len() > 1 {
        correlation.edges.swap(0, pos);
        correlation.edges.truncate(1);
    }
    Ok(())
}

/// The leg's locations, each keeping only the candidate edge the path uses (`CopyLocations`).
///
/// # Errors
///
/// Mainline's "`leg_shape_index` not set for intermediate location" when the intermediate
/// locations' edge indexes go back, and "Could not find matching edge candidate"
/// when a location's edge isn't among its candidates.
fn copy_locations(leg: &Leg) -> Result<Vec<Location>, ThorError> {
    let (Some(first), Some(last)) = (leg.edges.first(), leg.edges.last()) else {
        return Err(ThorError::EmptyLeg);
    };
    let (Some(origin), Some(destination)) = (leg.locations.first(), leg.locations.last()) else {
        return Err(ThorError::EmptyLeg);
    };
    let intermediates = leg
        .locations
        .get(1..leg.locations.len().saturating_sub(1))
        .unwrap_or_default();
    let mut out = Vec::with_capacity(leg.locations.len());
    let mut copy = origin.clone();
    remove_path_edges(&mut copy, first.edge_id)?;
    out.push(copy);
    let mut last_index: Option<u32> = None;
    for intermediate in intermediates {
        let index = intermediate
            .correlation
            .as_ref()
            .map_or(0, |c| c.leg_shape_index);
        // Mainline also rejects an index equal to the previous one, which two intermediate
        // locations at the same node have; Valinor builds that trip (see "Known divergences"
        // in `docs/routing.md`). An index that goes back is still path search's mistake.
        if last_index.is_some_and(|last| index < last) {
            return Err(ThorError::LegShapeIndexNotSet);
        }
        last_index = Some(index);
        let edge = usize::try_from(index)
            .ok()
            .and_then(|i| leg.edges.get(i))
            .ok_or(ThorError::MissingPathCandidate)?;
        let mut copy = intermediate.clone();
        remove_path_edges(&mut copy, edge.edge_id)?;
        out.push(copy);
    }
    let mut copy = destination.clone();
    remove_path_edges(&mut copy, last.edge_id)?;
    out.push(copy);
    Ok(out)
}

/// Where a leg starts or ends on its edge: the percent along it, the side of the street,
/// and the correlated point (invalid if the edge isn't among the location's candidates).
fn partial_edge(location: &Location, edge_id: GraphId, default_pct: f32) -> (f32, i32, PointLL) {
    let candidates = location.correlation.as_ref().map_or(&[][..], |c| &c.edges);
    for e in candidates {
        if e.graph_id == edge_id.value() {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "mainline keeps the percent along as a float"
            )]
            let pct = e.percent_along as f32;
            return (pct, e.side_of_street, point_of(e.ll.as_ref()));
        }
    }
    (default_pct, 0, PointLL::INVALID)
}

/// The distance an edge's heading is measured over (`GetOffsetForHeading`).
fn heading_offset(edge: &DirectedEdge) -> f32 {
    /// `kMinMetersOffsetForHeading`.
    const MIN_METERS_OFFSET_FOR_HEADING: f32 = 15.0;
    let rc = edge.classification() as u8;
    let mut offset = MIN_METERS_OFFSET_FOR_HEADING;
    if rc < 2 {
        offset *= 1.6;
    } else if rc < 5 {
        offset *= 1.4;
    }
    if matches!(
        edge.road_use(),
        Ok(RoadUse::Cycleway
            | RoadUse::MountainBike
            | RoadUse::Footway
            | RoadUse::Steps
            | RoadUse::Path
            | RoadUse::Pedestrian
            | RoadUse::Bridleway)
    ) {
        offset *= 0.5;
    }
    offset
}

/// An edge's shape in the direction of travel.
fn edge_shape(
    edge_id: GraphId,
    edge: &DirectedEdge,
    edge_info: &valhalla_graphtile::graph_tile::EdgeInfo<'_>,
) -> Result<Vec<PointLL>, ThorError> {
    let mut shape: Vec<PointLL> = edge_info
        .decode_raw_shape::<f64>()
        .map_err(|e| ThorError::Shape(edge_id, e))?
        .into_iter()
        .map(|c| PointLL::new(c.x, c.y))
        .collect();
    if !edge.edge_info_is_forward() {
        shape.reverse();
    }
    Ok(shape)
}

/// `float = double * float`, as mainline passes trim distances.
#[inline]
fn narrow(value: f64) -> f32 {
    #[expect(clippy::cast_possible_truncation, reason = "mainline passes a float")]
    let narrowed = value as f32;
    narrowed
}

/// The per shape point attributes of an edge (`SetShapeAttributes`): time in milliseconds,
/// length in decimeters, speed in decimeters per second, and the speed limit,
/// for each point the edge added after `shape_begin`.
///
/// Without live traffic or incidents the edge has no cuts, so no points are added
/// and the speed is the edge's average over the part used.
#[expect(clippy::too_many_arguments, reason = "mainline's signature")]
fn set_shape_attributes(
    c: &AttributesController,
    edge: &DirectedEdge,
    speed_limit: u8,
    shape: &[PointLL],
    shape_begin: usize,
    leg: &mut TripLeg,
    src_pct: f64,
    tgt_pct: f64,
    edge_seconds: f64,
) {
    if !c.category_enabled(Category::ShapeAttributes) {
        return;
    }
    let attributes = leg.shape_attributes.get_or_insert_default();
    let speed = (f64::from(edge.length()) * (tgt_pct - src_pct)) / edge_seconds;
    for pair in shape.get(shape_begin..).unwrap_or_default().windows(2) {
        let [a, b] = pair else { continue };
        let distance = b.distance(*a);
        let mut time = distance / speed;
        if time.is_nan() {
            time = 0.0;
        }
        if c.get(Attribute::ShapeAttributesTime) {
            attributes.time.push(attribute_u32(time * 1000.0 + 0.5));
        }
        if c.get(Attribute::ShapeAttributesLength) {
            attributes.length.push(attribute_u32(distance * 10.0 + 0.5));
        }
        if c.get(Attribute::ShapeAttributesCongestion) {
            // `UNKNOWN_CONGESTION_VAL`.
            attributes.congestion.push(0);
        }
        if c.get(Attribute::ShapeAttributesSpeed) {
            let mut decimeters_sec = distance * 10.0 / time + 0.5;
            if decimeters_sec.is_nan() || time == 0.0 {
                decimeters_sec = 0.0;
            }
            attributes.speed.push(attribute_u32(decimeters_sec));
        }
        if c.get(Attribute::ShapeAttributesSpeedLimit) {
            attributes.speed_limit.push(u32::from(speed_limit));
        }
    }
}

/// A shape attribute, rounded by mainline's `+ 0.5`, as a `uint32`.
///
/// An infinite time is 0: it comes from a part of an edge with no length,
/// where the leg starts or ends at a node (the speed over it is 0),
/// and mainline's conversion of infinity to an integer gives 0 on x86-64.
/// Clients see these times (Ferrostar's annotations sum them), so the 0 is kept.
#[inline]
fn attribute_u32(value: f64) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "mainline converts the rounded double to a uint32"
    )]
    let converted = value as u32;
    converted
}

/// Whether an edge spans several levels (`is_multi_level`).
fn is_multi_level(levels: &[(f32, f32)]) -> bool {
    match levels {
        [(start, end)] => start != end,
        _ => !levels.is_empty(),
    }
}

/// The level a location filters on, if any.
fn search_level(location: &Location) -> Option<f32> {
    location
        .search_filter
        .as_ref()
        .and_then(|f| f.has_level.as_ref())
        .map(|HasLevel::Level(level)| *level)
}

/// Builds one leg of the trip from its path (mainline's `TripLegBuilder::Build`).
///
/// # Errors
///
/// Mainline's exceptions: tiles that are gone, inconsistent locations (see
/// [`copy_locations`]), and corrupt tile data.
#[expect(clippy::too_many_lines, reason = "mainline's function, step by step")]
pub(crate) fn build_leg<P: GraphTileProvider>(
    ctx: &LegContext<'_, P>,
    leg: &Leg,
) -> Result<TripLeg, ThorError> {
    let c = ctx.controller;
    let provider = ctx.provider;
    let costing = ctx.costing;
    let path = leg.edges.as_slice();
    let (Some(first_edge), Some(last_edge)) = (path.first(), path.last()) else {
        return Err(ThorError::EmptyLeg);
    };
    let (Some(origin), Some(dest)) = (leg.locations.first(), leg.locations.last()) else {
        return Err(ThorError::EmptyLeg);
    };

    let mut trip = TripLeg {
        algorithms: leg.algorithms.iter().map(|&a| a.to_owned()).collect(),
        location: copy_locations(leg)?,
        ..TripLeg::default()
    };

    let mut origin_for_time = origin.clone();
    let forward_time_info = location_time_info(provider, &mut origin_for_time, ctx.now)?;
    let invariant = DateTimeType::try_from(ctx.options.date_time_type)
        .is_ok_and(|t| t == DateTimeType::Invariant);
    let arrive_by = DateTimeType::try_from(ctx.options.date_time_type)
        .is_ok_and(|t| t == DateTimeType::ArriveBy);

    // The path starts at the end node of the first edge's opposing edge.
    let first_tile = graph::tile(provider, first_edge.edge_id)?
        .ok_or(ThorError::TileGone(first_edge.edge_id))?;
    let mut startnode = graph::start_node(provider, &first_tile, first_edge.edge_id)?
        .ok_or(ThorError::TileGone(first_edge.edge_id))?;

    // Where the leg starts and ends on its first and last edges.
    let (start_pct, start_sos, start_vrt) = partial_edge(origin, first_edge.edge_id, 0.0);
    let (end_pct, end_sos, end_vrt) = partial_edge(dest, last_edge.edge_id, 1.0);
    if let Some(location) = trip.location.first_mut() {
        let correlation = location.correlation.get_or_insert_default();
        correlation.projected_ll = Some(lat_lng(start_vrt));
        if start_sos != 0 {
            location.side_of_street = start_sos;
        }
    }
    if let Some(location) = trip.location.last_mut() {
        let correlation = location.correlation.get_or_insert_default();
        correlation.projected_ll = Some(lat_lng(end_vrt));
        if end_sos != 0 {
            location.side_of_street = end_sos;
        }
    }

    let mut admins = Admins::default();
    let mut prior_opp: Option<u8> = None;
    let mut shape: Vec<PointLL> = Vec::new();
    let mut osm_changeset = 0_u64;
    let mut prev_edge: Option<DirectedEdge> = None;
    let mut tile: Option<P::TileHandle> = None;
    let mut prev_level = MAX_LEVEL;
    // The locations after the origin, whose edge indexes become shape indexes.
    let mut intermediate = 1;
    let mut total_distance = 0.0_f64;
    let (mut has_toll, mut has_ferry, mut has_highway) = (false, false, false);
    let costing_mode = costing.base().travel_mode();
    trip.node.reserve(path.len() + 1);

    for (edge_index, path_edge) in path.iter().enumerate() {
        let edge_id = path_edge.edge_id;
        let edge_tile = match &tile {
            Some(t) => graph::tile_for(provider, t, edge_id)?,
            None => graph::tile(provider, edge_id)?,
        }
        .ok_or(ThorError::TileGone(edge_id))?;
        tile = Some(edge_tile.clone());
        let edge = graph::edge(&edge_tile, edge_id)?.clone();
        let mode = path_edge.mode;
        // Mainline's `mode_costing` holds the request's costing at its own mode only.
        let travel_type = if mode == costing_mode {
            costing.travel_type()
        } else {
            0
        };
        let is_first = edge_index == 0;
        let is_last = edge_index + 1 == path.len();
        let prev = edge_index.checked_sub(1).and_then(|i| path.get(i));

        has_toll |= edge.toll();
        has_ferry |= matches!(edge.road_use(), Ok(RoadUse::Ferry));
        has_highway |= edge.classification() == RoadClass::Motorway;

        let start_tile = graph::tile_for(provider, &edge_tile, startnode)?
            .ok_or(ThorError::TileGone(startnode))?;
        let node = graph::node(&start_tile, startnode)?.clone();

        if osm_changeset == 0 && c.get(Attribute::OsmChangeset) {
            osm_changeset = start_tile.header().dataset_id.get();
        }

        // The time at the start of the edge, in its node's time zone.
        let seconds_offset = match prev {
            Some(p) if !invariant => p.elapsed_cost.secs,
            _ => 0.0,
        };
        let node_tz = node.time_zone_index().map_or(0, |t| u32::from(t.get()));
        let time_info = forward_time_info.forward(seconds_offset, node_tz);

        let mut trip_node = trip_leg::Node::default();
        if c.get(Attribute::NodeType) {
            trip_node.r#type = i32::from(node.node_type()? as u8);
        }
        if c.get(Attribute::NodeTrafficSignal) && node.is_traffic_signal() {
            trip_node.traffic_signal = true;
        }
        if node.intersection_type().ok() == Some(IntersectionType::Fork)
            && c.get(Attribute::NodeFork)
        {
            trip_node.fork = true;
        }
        if c.get(Attribute::NodeElapsedTime) {
            let elapsed = prev.map_or(cost(0.0, 0.0), |p| {
                cost(p.elapsed_cost.secs, p.elapsed_cost.cost)
            });
            trip_node.cost.get_or_insert_default().elapsed_cost = Some(elapsed);
        }
        if c.get(Attribute::NodeAdminIndex) {
            trip_node.admin_index = admins.index(admin_of(&start_tile, node.admin_index())?);
        }
        if c.get(Attribute::NodeTimeZone)
            && let Some(name) = time_zone_name(node_tz)
        {
            name.clone_into(&mut trip_node.time_zone);
        }
        if c.get(Attribute::NodeTransitionTime) {
            trip_node.cost.get_or_insert_default().transition_cost = Some(cost(
                path_edge.transition_cost.secs,
                path_edge.transition_cost.cost,
            ));
        }
        // Bike share and transit nodes need costings and tiles Valinor doesn't route with.

        let include_first_point = is_first;
        let begin_index = if include_first_point {
            shape.len()
        } else {
            shape.len().saturating_sub(1)
        };
        let edge_info = edge_tile.get_edge_info(&edge)?;
        let edge_tags = tags(&edge_info)?;
        let levels = match edge_info.levels()? {
            Some(levels) => (
                levels.ranges().collect::<Result<Vec<_>, _>>()?,
                levels.precision(),
            ),
            None => (Vec::new(), 0),
        };
        let blind = travel_type == PedestrianType::Blind as u8 && mode == TravelMode::Pedestrian;
        let start_node_index = u32::try_from(startnode.feature_index()).unwrap_or(u32::MAX);
        let mut trip_edge = add_trip_edge(&TripEdgeInput {
            controller: c,
            edge_id,
            path_edge,
            travel_type,
            costing,
            edge: &edge,
            drive_on_right: node.drive_on_right(),
            tile: &edge_tile,
            time_info: &time_info,
            start_node_index,
            has_junction_name: node.is_named_intersection(),
            start_tile: &start_tile,
            blind,
            edge_info: &edge_info,
            tags: &edge_tags,
            levels: &levels,
        })?;

        let mut trim_start_pct: f32 = if is_first { start_pct } else { 0.0 };
        let mut trim_end_pct: f32 = if is_last { end_pct } else { 1.0 };
        let trimming: Option<&EdgeTrim> = leg
            .edge_trimming
            .iter()
            .find(|t| t.edge_index == edge_index);
        let skip = usize::from(!include_first_point);
        if let Some(trimming) = trimming {
            let mut edge_shape = edge_shape(edge_id, &edge, &edge_info)?;
            let (mut begin_trim_dist, mut end_trim_dist) = (0.0_f64, 1.0_f64);
            let mut begin_trim_vrt = edge_shape.first().copied().unwrap_or(PointLL::INVALID);
            let mut end_trim_vrt = edge_shape.last().copied().unwrap_or(PointLL::INVALID);
            if trimming.begin.trim {
                begin_trim_dist = trimming.begin.percent_along;
                begin_trim_vrt = PointLL::new(trimming.begin.point.0, trimming.begin.point.1);
            } else if is_first {
                begin_trim_dist = f64::from(start_pct);
                begin_trim_vrt = start_vrt;
            }
            if trimming.end.trim {
                end_trim_dist = trimming.end.percent_along;
                end_trim_vrt = PointLL::new(trimming.end.point.0, trimming.end.point.1);
            } else if is_last {
                end_trim_dist = f64::from(end_pct);
                end_trim_vrt = end_vrt;
            }
            trim_start_pct = narrow(begin_trim_dist);
            trim_end_pct = narrow(end_trim_dist);
            #[expect(clippy::cast_precision_loss, reason = "mainline's float edge length")]
            let edge_length = f64::from(edge.length() as f32);
            trim_shape(
                narrow(begin_trim_dist * edge_length),
                begin_trim_vrt,
                narrow(end_trim_dist * edge_length),
                end_trim_vrt,
                &mut edge_shape,
            );
            shape.extend(edge_shape.into_iter().skip(skip));
        } else if is_first || is_last {
            let mut edge_shape = edge_shape(edge_id, &edge, &edge_info)?;
            #[expect(clippy::cast_precision_loss, reason = "mainline's float edge length")]
            let total = edge.length() as f32;
            let front = edge_shape.first().copied().unwrap_or(PointLL::INVALID);
            let back = edge_shape.last().copied().unwrap_or(PointLL::INVALID);
            if is_first && is_last {
                trim_shape(
                    start_pct * total,
                    start_vrt,
                    end_pct * total,
                    end_vrt,
                    &mut edge_shape,
                );
            } else if is_first {
                trim_shape(start_pct * total, start_vrt, total, back, &mut edge_shape);
            } else {
                trim_shape(0.0, front, end_pct * total, end_vrt, &mut edge_shape);
            }
            shape.extend(edge_shape.into_iter().skip(skip));
        } else {
            shape.extend(
                edge_shape(edge_id, &edge, &edge_info)?
                    .into_iter()
                    .skip(skip),
            );
        }
        let last_index = shape.len().saturating_sub(1);

        // Level changes: edges on one level, and multi-level edges at the ends of the leg
        // (whose level is the one the location filters on).
        let multilevel = is_multi_level(&levels.0);
        let single_level = matches!(levels.0.as_slice(), [(start, end)] if start == end);
        if ((is_first || is_last) && multilevel) || single_level {
            let mut lvl = MAX_LEVEL;
            if !multilevel {
                if let Some(&(start, _)) = levels.0.first() {
                    lvl = start;
                }
            } else if (is_first && search_level(origin).is_some())
                || (is_last && search_level(dest).is_some())
            {
                // On a one-edge leg this is the origin's level, even when only the destination
                // has one (an unset level reads as 0, as in mainline).
                let location = if is_first { origin } else { dest };
                lvl = search_level(location).unwrap_or(0.0);
            }
            let precision = levels.1.max(1);
            if lvl != MAX_LEVEL
                && !(is_last && multilevel && !is_first)
                && (lvl - prev_level).abs() >= f32::EPSILON
            {
                trip.level_changes.push(LevelChange {
                    shape_index: u32::try_from(begin_index).unwrap_or(u32::MAX),
                    level: lvl,
                    precision,
                });
                prev_level = lvl;
            }
            if is_last
                && multilevel
                && let Some(level) = search_level(dest)
            {
                trip.level_changes.push(LevelChange {
                    shape_index: u32::try_from(last_index).unwrap_or(u32::MAX),
                    level,
                    precision,
                });
            }
        }

        trip_edge.source_along_edge = trim_start_pct;
        trip_edge.target_along_edge = trim_end_pct;

        // The distance from the start of the leg, for intermediate locations.
        let previous_total_distance = total_distance;
        #[expect(clippy::cast_precision_loss, reason = "mainline's float arithmetic")]
        let used = edge.length() as f32 * (trim_end_pct - trim_start_pct);
        total_distance += f64::from(used);

        // Several intermediate locations can be at the same node, with no path between them
        // (two via locations snapped to one node; see "Known divergences" in `docs/routing.md`),
        // so after the first location this edge reaches, the next ones are checked too
        // (but not the destination, which mainline's single check would never reach there).
        let locations = trip.location.len();
        let mut first_at_edge = true;
        while (first_at_edge || intermediate + 1 < locations)
            && let Some(location) = trip.location.get_mut(intermediate)
            && usize::try_from(
                location
                    .correlation
                    .as_ref()
                    .map_or(0, |c| c.leg_shape_index),
            )
            .ok()
                == Some(edge_index)
        {
            first_at_edge = false;
            let correlation = location.correlation.get_or_insert_default();
            correlation.leg_shape_index = u32::try_from(last_index).unwrap_or(u32::MAX);
            correlation.distance_from_leg_origin = total_distance;
            // At a node (no trimming), an arrive by index is one edge late:
            // the location is at the end of the previous edge.
            if trimming.is_none() && arrive_by {
                correlation.leg_shape_index = u32::try_from(begin_index).unwrap_or(u32::MAX);
                correlation.distance_from_leg_origin = previous_total_distance;
            }
            intermediate += 1;
        }

        if c.get(Attribute::EdgeLength) {
            #[expect(clippy::cast_precision_loss, reason = "mainline's float arithmetic")]
            let km = (edge.length() as f32 * 0.001 * (trim_end_pct - trim_start_pct)).max(0.0);
            trip_edge.length_km = km;
        }

        // The time on this edge, without the transition onto it.
        let mut edge_seconds = path_edge.elapsed_cost.secs - path_edge.transition_cost.secs;
        if let Some(p) = prev {
            edge_seconds -= p.elapsed_cost.secs;
        }
        set_shape_attributes(
            c,
            &edge,
            edge_info.speed_limit(),
            &shape,
            begin_index,
            &mut trip,
            f64::from(trim_start_pct),
            f64::from(trim_end_pct),
            f64::from(edge_seconds),
        );

        if c.get(Attribute::EdgeBeginShapeIndex) {
            trip_edge.begin_shape_index = u32::try_from(begin_index).unwrap_or(u32::MAX);
        }
        if c.get(Attribute::EdgeEndShapeIndex) {
            trip_edge.end_shape_index = u32::try_from(last_index).unwrap_or(u32::MAX);
        }
        if c.get(Attribute::EdgeBeginHeading) || c.get(Attribute::EdgeEndHeading) {
            let offset = f64::from(heading_offset(&edge));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "mainline rounds the heading into a uint32"
            )]
            {
                if c.get(Attribute::EdgeBeginHeading) {
                    trip_edge.begin_heading =
                        heading_along_polyline(&shape, offset, begin_index, last_index).round()
                            as u32;
                }
                if c.get(Attribute::EdgeEndHeading) {
                    trip_edge.end_heading =
                        heading_at_end_of_polyline(&shape, offset, begin_index, last_index).round()
                            as u32;
                }
            }
        }
        if c.get(Attribute::EdgeElevation) {
            set_elevation(
                &mut trip_edge,
                f64::from(trim_start_pct),
                f64::from(trim_end_pct),
                &node,
                &edge,
                &edge_info,
                provider,
            )?;
        }
        // Landmarks: Valinor's tile builds have none (see `tools/oracle/README.md`).

        // The edges at the start node, unless it is inside a recovered shortcut.
        if !path_edge.start_node_is_recovered {
            add_intersecting_edges(
                c,
                provider,
                &start_tile,
                &node,
                &edge,
                prev_edge.as_ref(),
                prior_opp,
                &mut trip_node,
                blind,
            )?;
        }
        trip_node.edge = Some(trip_edge);
        trip.node.push(trip_node);

        // The next edge starts at this one's end node, unless it is this edge again
        // (the path continues past a location in the middle of it).
        if path
            .get(edge_index + 1)
            .is_none_or(|next| next.edge_id != edge_id)
        {
            startnode = edge.end_node_id();
        }
        // The opposing edge, for name consistency at the next node.
        let Some(end_tile) = graph::end_tile(provider, &edge_tile, &edge)? else {
            // Mainline skips the rest of the iteration.
            continue;
        };
        let opp = graph::opposing_in_end_tile(&end_tile, &edge)?;
        prev_edge = Some(graph::edge(&end_tile, opp)?.clone());
        prior_opp = Some(edge.opposing_local_index());
    }

    // The node at the end of the leg.
    let mut end_node = trip_leg::Node::default();
    if c.get(Attribute::NodeAdminIndex) {
        let end_tile = graph::tile(provider, startnode)?.ok_or(ThorError::TileGone(startnode))?;
        let node = graph::node(&end_tile, startnode)?;
        end_node.admin_index = admins.index(admin_of(&end_tile, node.admin_index())?);
    }
    if c.get(Attribute::NodeElapsedTime) {
        end_node.cost.get_or_insert_default().elapsed_cost = Some(cost(
            last_edge.elapsed_cost.secs,
            last_edge.elapsed_cost.cost,
        ));
    }
    if c.get(Attribute::NodeTransitionTime) {
        end_node.cost.get_or_insert_default().transition_cost = Some(cost(0.0, 0.0));
    }
    trip.node.push(end_node);

    if c.category_enabled(Category::Admin) {
        trip.admin = admins
            .list
            .into_iter()
            .map(
                |[country_code, country_text, state_code, state_text]| trip_leg::Admin {
                    country_code: if c.get(Attribute::AdminCountryCode) {
                        country_code
                    } else {
                        String::new()
                    },
                    country_text: if c.get(Attribute::AdminCountryText) {
                        country_text
                    } else {
                        String::new()
                    },
                    state_code: if c.get(Attribute::AdminStateCode) {
                        state_code
                    } else {
                        String::new()
                    },
                    state_text: if c.get(Attribute::AdminStateText) {
                        state_text
                    } else {
                        String::new()
                    },
                },
            )
            .collect();
    }

    if let Some((min, max)) = bounding_box(&shape) {
        trip.bbox = Some(BoundingBox {
            min_ll: Some(lat_lng(min)),
            max_ll: Some(lat_lng(max)),
        });
    }
    if c.get(Attribute::Shape) {
        trip.shape = encode_polyline(&shape);
    }
    if osm_changeset != 0 && c.get(Attribute::OsmChangeset) {
        trip.osm_changeset = osm_changeset;
    }
    trip.summary = Some(Summary {
        has_toll,
        has_ferry,
        has_highway,
        ..Summary::default()
    });

    accumulate_recostings(
        ctx,
        start_pct,
        end_pct,
        forward_time_info,
        invariant,
        &mut trip,
    )?;
    Ok(trip)
}

/// The elapsed and transition costs of the leg under each of the request's `recostings`,
/// at every node (mainline's `AccumulateRecostingInfoForward`).
///
/// A costing that can't travel the path (a difference in access, for one) gets an empty cost
/// at every node.
fn accumulate_recostings<P: GraphTileProvider>(
    ctx: &LegContext<'_, P>,
    src_pct: f32,
    tgt_pct: f32,
    time_info: TimeInfo,
    invariant: bool,
    leg: &mut TripLeg,
) -> Result<(), ThorError> {
    if leg.node.is_empty() {
        return Ok(());
    }
    for recosting in &ctx.options.recostings {
        let costing = AnyCosting::new(recosting).map_err(|e| ThorError::Options(e.to_string()))?;
        // The edges, read back from the trip as mainline does (an edge without an ID ends it).
        let mut edges = Vec::with_capacity(leg.node.len());
        for node in &leg.node {
            let Some(edge) = &node.edge else { break };
            edges.push(graph_id(edge.id)?);
        }
        let elapsed = |secs: f32, c: f32| PathCost {
            elapsed_cost: Some(cost(secs, c)),
            transition_cost: None,
        };
        if let Some(first) = leg.node.first_mut() {
            first.recosts.push(elapsed(0.0, 0.0));
        }
        let mut at = 0;
        let nodes = &mut leg.node;
        let result = recost_forward(
            ctx.provider,
            &costing,
            edges,
            src_pct,
            tgt_pct,
            time_info,
            invariant,
            false,
            |label| {
                if let Some(last) = nodes.get_mut(at).and_then(|n| n.recosts.last_mut()) {
                    last.transition_cost =
                        Some(cost(label.transition_cost.secs, label.transition_cost.cost));
                }
                at += 1;
                if let Some(node) = nodes.get_mut(at) {
                    node.recosts.push(elapsed(label.cost.secs, label.cost.cost));
                }
            },
        );
        if result.is_ok() {
            if let Some(last) = leg.node.get_mut(at).and_then(|n| n.recosts.last_mut()) {
                last.transition_cost = Some(cost(0.0, 0.0));
            }
        } else {
            // A costing that can't take the path gets an empty cost at every node,
            // in place of what it had added before it failed.
            let should_have = leg.node.first().map_or(0, |n| n.recosts.len());
            for node in &mut leg.node {
                if node.recosts.len() == should_have {
                    node.recosts.pop();
                }
                node.recosts.push(PathCost::default());
            }
        }
    }
    Ok(())
}
