//! Converting resolved walks and access or egress legs into Thor's paths.
//!
//! [`Thor::build_trip`](valinor_thor::Thor::build_trip) reads
//! [`Paths`](valinor_thor::Paths): for each leg, the edges with their elapsed and transition
//! costs, plus the locations, whose candidate edges tell it how far along the first and last
//! edge the leg starts and ends.
//! These functions build that for a transit leg; [`Leg::origin`] and [`Leg::destination`] are
//! the indexes the caller gives its locations in the request's options.
//!
//! A walk of no length has no edges; those legs have no shape to draw and aren't built.

use valhalla_graphtile::GraphId;
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::{Correlation, LatLng, Location, PathEdge as CandidateEdge};
use valinor_sif::{Cost, INVALID_RESTRICTION, TravelMode};
use valinor_thor::{Leg, PathEdge, Paths, Route};

use crate::search::ResolvedWalk;
use crate::table::AccessLeg;

/// The name a transit leg gives as its path algorithm.
pub const WALK_ALGORITHM: &str = "valinor_walk";

/// Where a leg's end sits: a point, and the edge and fraction along it.
fn location_at(point: (f64, f64), edge: GraphId, percent_along: f32) -> Location {
    let ll = LatLng {
        has_lat: Some(HasLat::Lat(point.1)),
        has_lng: Some(HasLng::Lng(point.0)),
    };
    Location {
        ll: Some(ll),
        correlation: Some(Correlation {
            edges: vec![CandidateEdge {
                graph_id: edge.value(),
                percent_along: f64::from(percent_along),
                ll: Some(ll),
                ..CandidateEdge::default()
            }],
            ..Correlation::default()
        }),
        ..Location::default()
    }
}

/// One edge of a leg, from the running totals before it, its transition, and its traversal.
fn path_edge(
    id: GraphId,
    mode: TravelMode,
    so_far: Cost,
    transition: Cost,
    traversal: Cost,
) -> PathEdge {
    let elapsed = so_far + transition + traversal;
    PathEdge {
        edge_id: id,
        mode,
        elapsed_cost: elapsed,
        transition_cost: transition,
        path_distance: 0.0,
        restriction_index: INVALID_RESTRICTION,
        start_node_is_recovered: false,
    }
}

/// A complete Thor leg, from its edges and locations.
#[expect(clippy::too_many_arguments, reason = "the fields of the leg it fills")]
fn leg_from_parts(
    edges: Vec<PathEdge>,
    origin_index: usize,
    destination_index: usize,
    start: (f64, f64),
    start_edge: GraphId,
    start_pct: f32,
    end: (f64, f64),
    end_edge: GraphId,
    end_pct: f32,
) -> Leg {
    let locations = vec![
        location_at(start, start_edge, start_pct),
        location_at(end, end_edge, end_pct),
    ];
    Leg {
        origin: origin_index,
        destination: destination_index,
        intermediates: Vec::new(),
        locations,
        begin_percent: Some(f64::from(start_pct)),
        end_percent: Some(f64::from(end_pct)),
        edges,
        algorithms: vec![WALK_ALGORITHM],
        edge_trimming: Vec::new(),
    }
}

/// A transfer walk as a Thor leg, or `None` if the walk has no length.
///
/// `from` and `to` are the two stops' points (longitude and latitude),
/// and their indexes in the request's locations go in `origin_index` and `destination_index`.
#[must_use]
pub fn transfer_leg(
    walk: &ResolvedWalk,
    mode: TravelMode,
    origin_index: usize,
    destination_index: usize,
    from: (f64, f64),
    to: (f64, f64),
) -> Option<Leg> {
    let (&first, &last) = (walk.edges.first()?, walk.edges.last()?);
    let mut so_far = Cost::ZERO;
    let mut edges = Vec::with_capacity(walk.edges.len());
    for (&edge, &secs) in walk.edges.iter().zip(&walk.edge_secs) {
        let part = Cost::new(f32::from(secs), f32::from(secs));
        edges.push(path_edge(edge, mode, so_far, Cost::ZERO, part));
        so_far += part;
    }
    Some(leg_from_parts(
        edges,
        origin_index,
        destination_index,
        from,
        first,
        walk.begin_percent,
        to,
        last,
        walk.end_percent,
    ))
}

/// An access or egress leg as a Thor leg, or `None` if it has no edges.
#[must_use]
pub fn access_leg(
    leg: &AccessLeg,
    mode: TravelMode,
    origin_index: usize,
    destination_index: usize,
    start: (f64, f64),
    end: (f64, f64),
) -> Option<Leg> {
    let (&first, &last) = (leg.edges.first()?, leg.edges.last()?);
    let mut so_far = Cost::ZERO;
    let mut edges = Vec::with_capacity(leg.edges.len());
    for edge in &leg.edges {
        let at = path_edge(edge.edge, mode, so_far, edge.transition, edge.traversal);
        so_far = at.elapsed_cost;
        edges.push(at);
    }
    Some(leg_from_parts(
        edges,
        origin_index,
        destination_index,
        start,
        first.edge,
        leg.begin_percent,
        end,
        last.edge,
        leg.end_percent,
    ))
}

/// One route of the given legs, for [`Thor::build_trip`](valinor_thor::Thor::build_trip).
#[must_use]
pub fn route_paths(legs: Vec<Leg>) -> Paths {
    Paths {
        routes: vec![Route { legs }],
    }
}
