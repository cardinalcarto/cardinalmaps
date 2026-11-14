//! Finding the edges near each location (mainline's `loki::Search`, `src/loki/search.cc`).
//!
//! Each location walks the local level's edge bins outward from its position, closest first,
//! projecting itself onto the shape of every edge in each bin.
//! It keeps the closest reachable edge, plus any others within its radius,
//! and stops once the next bin is farther than both its radius and its best edge
//! (or farther than its search cutoff).
//! Locations that share a bin are handled together, so that the bin's edges are decoded once.
//! Each winning candidate becomes one or more path edges on the location's correlation:
//! the edge and its opposing edge, or, when the point is at (or snaps to) a node,
//! every edge entering and leaving that node on every level.
//!
//! The order of operations, the arithmetic, and mainline's quirks are kept,
//! since they decide which edges win.
//! See the comments on each step; the notable quirks are:
//!
//! - The opposing edge of an edge correlated along its length gets no bounding circle
//!   (mainline sets it on the wrong, already moved, message).
//! - Edges that a location's side, heading, or layer filter rejects are added to
//!   `filtered_edges` without checking for duplicates when the location is along an edge,
//!   but with a check when it is at a node.
//! - When an edge isn't reachable enough for one location but its opposing edge is,
//!   the swap carries over to the locations after it in the same bin.
//!
//! # Bounding circles
//!
//! Tiles since 3.9 store a bounding circle for every edge in a bin,
//! which lets the search skip edges that can't be close enough without decoding their shapes.
//! Mainline decodes the circles with `cosf`, whose result varies between C libraries,
//! and an edge it keeps but we skip would be a missing candidate.
//! So we grow each circle by [`CIRCLE_MARGIN_METERS`] before deciding to skip:
//! we only skip edges that mainline skips too, and decode a few that it doesn't
//! (which can only matter when an edge's distance is within that margin of a threshold).
//!
//! # Processing order
//!
//! Mainline groups locations by bin and handles the largest group first,
//! breaking ties by the address of the bin's tile in memory, which we can't reproduce.
//! We break ties by tile ID instead.
//! The order only matters through the reach cache and the opposing edge swap above,
//! and only when several locations visit the same edges.

use crate::graph::GraphError;
use crate::reach::{DirectedReach, Reach};
use crate::tiles::{ClosestFirst, bin_center};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use thiserror::Error;
use valhalla_graphtile::graph_reader::{
    GraphReadError, GraphReader, Tile, edge as get_edge, node as get_node, node_ll, shape,
};
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile};
use valhalla_graphtile::midgard::{DistanceApproximator, PointLL, Projector, tangent_angle};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_graphtile::{GraphId, RoadUse};
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::location::{
    HasHeading, HasHeadingTolerance, HasMinimumInboundReachability, HasMinimumOutboundReachability,
    HasNodeSnapTolerance, HasPreferredLayer, HasRadius, HasSearchCutoff, HasStreetSideCutoff,
    HasStreetSideMaxDistance, HasStreetSideTolerance, PreferredSide, SideOfStreet,
    Type as LocationType,
};
use valhalla_proto::path_edge::BoundingCircle;
use valhalla_proto::search_filter::{HasLevel, HasMaxRoadClass, HasMinRoadClass};
use valhalla_proto::{Correlation, LatLng, Location, PathEdge, SearchFilter};
use valinor_sif::{Costing, DisallowMask};

/// How much bounding circles are grown before deciding that an edge is too far
/// (see the [module docs](self)).
///
/// The decoded circle's center depends on `cosf` through the meters per degree of longitude;
/// a difference of a few units in the last place moves it by well under a millimeter
/// over the 2.8 km a center can be from its bin's center.
pub(crate) const CIRCLE_MARGIN_METERS: f64 = 0.01;

/// "No level filter" (`kMaxLevel`).
pub(crate) const MAX_LEVEL: f32 = f32::MAX;

/// A failure of the search.
#[derive(Debug, Error)]
pub enum SearchError {
    /// The graph couldn't be read.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// A location's search cutoff covers the whole world and still found nothing
    /// (mainline throws "Subdivisions were exhausted").
    #[error("search the bins: subdivisions were exhausted")]
    Exhausted,
}

impl From<GraphReadError> for SearchError {
    #[inline]
    fn from(e: GraphReadError) -> Self {
        Self::Graph(e.into())
    }
}

/// A location's search filter, with mainline's accessor defaults for unset fields.
#[derive(Clone, Copy, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "the filter's flags")]
struct Filter {
    min_road_class: u32,
    max_road_class: u32,
    exclude_tunnel: bool,
    exclude_bridge: bool,
    exclude_toll: bool,
    exclude_ramp: bool,
    exclude_ferry: bool,
    level: f32,
}

impl Filter {
    fn from_proto(filter: Option<&SearchFilter>) -> Self {
        let filter = filter.copied().unwrap_or_default();
        let class = |c: i32| u32::try_from(c).unwrap_or(0);
        Self {
            min_road_class: match filter.has_min_road_class {
                Some(HasMinRoadClass::MinRoadClass(c)) => class(c),
                None => 0,
            },
            max_road_class: match filter.has_max_road_class {
                Some(HasMaxRoadClass::MaxRoadClass(c)) => class(c),
                None => 0,
            },
            exclude_tunnel: filter.exclude_tunnel,
            exclude_bridge: filter.exclude_bridge,
            exclude_toll: filter.exclude_toll,
            exclude_ramp: filter.exclude_ramp,
            exclude_ferry: filter.exclude_ferry,
            level: match filter.has_level {
                Some(HasLevel::Level(level)) => level,
                None => 0.0,
            },
        }
    }

    /// Whether the filter rules the edge out (`search_filter`).
    ///
    /// Closures come from live traffic, which Valinor doesn't have,
    /// so `exclude_closures` never rules anything out.
    fn excludes<T: GraphTile>(&self, edge: &DirectedEdge, tile: &T) -> Result<bool, GraphError> {
        let road_class = u32::from(edge.classification().discriminant());
        let road_use = edge.road_use().ok();
        if road_class > self.min_road_class
            || road_class < self.max_road_class
            || (self.exclude_tunnel && edge.is_tunnel())
            || (self.exclude_bridge && edge.is_bridge())
            || (self.exclude_toll && edge.toll())
            || (self.exclude_ramp && road_use == Some(RoadUse::Ramp))
            || (self.exclude_ferry && matches!(road_use, Some(RoadUse::Ferry | RoadUse::RailFerry)))
        {
            return Ok(true);
        }
        if self.level != MAX_LEVEL {
            let info = tile.get_edge_info(edge)?;
            return Ok(!info.includes_level(self.level)?);
        }
        Ok(false)
    }
}

/// What the search reads of a location, with mainline's accessor defaults for unset fields.
#[derive(Clone, Copy, Debug)]
struct Params {
    ll: PointLL,
    radius: u32,
    search_cutoff: u32,
    node_snap_tolerance: u32,
    heading: Option<u32>,
    heading_tolerance: u32,
    street_side_tolerance: u32,
    street_side_max_distance: u32,
    street_side_cutoff: u32,
    preferred_side: i32,
    preferred_layer: Option<i32>,
    display_ll: Option<PointLL>,
    is_through: bool,
    min_inbound: u32,
    min_outbound: u32,
    filter: Filter,
}

/// A `LatLng` as a point, with unset coordinates read as 0 (as mainline's accessors do).
pub(crate) fn point_of(ll: Option<&LatLng>) -> PointLL {
    let ll = ll.copied().unwrap_or_default();
    let lat = match ll.has_lat {
        Some(HasLat::Lat(lat)) => lat,
        None => 0.0,
    };
    let lng = match ll.has_lng {
        Some(HasLng::Lng(lng)) => lng,
        None => 0.0,
    };
    PointLL::new(lng, lat)
}

/// A point as a `LatLng` with both coordinates set.
pub(crate) fn lat_lng(p: PointLL) -> LatLng {
    LatLng {
        has_lat: Some(HasLat::Lat(p.lat)),
        has_lng: Some(HasLng::Lng(p.lng)),
    }
}

impl Params {
    fn from_location(location: &Location) -> Self {
        let location_type = LocationType::try_from(location.r#type).ok();
        Self {
            ll: point_of(location.ll.as_ref()),
            radius: match location.has_radius {
                Some(HasRadius::Radius(r)) => r,
                None => 0,
            },
            search_cutoff: match location.has_search_cutoff {
                Some(HasSearchCutoff::SearchCutoff(c)) => c,
                None => 0,
            },
            node_snap_tolerance: match location.has_node_snap_tolerance {
                Some(HasNodeSnapTolerance::NodeSnapTolerance(t)) => t,
                None => 0,
            },
            heading: location.has_heading.map(|HasHeading::Heading(h)| h),
            heading_tolerance: match location.has_heading_tolerance {
                Some(HasHeadingTolerance::HeadingTolerance(t)) => t,
                None => 0,
            },
            street_side_tolerance: match location.has_street_side_tolerance {
                Some(HasStreetSideTolerance::StreetSideTolerance(t)) => t,
                None => 0,
            },
            street_side_max_distance: match location.has_street_side_max_distance {
                Some(HasStreetSideMaxDistance::StreetSideMaxDistance(d)) => d,
                None => 0,
            },
            street_side_cutoff: match location.has_street_side_cutoff {
                Some(HasStreetSideCutoff::StreetSideCutoff(c)) => u32::try_from(c).unwrap_or(0),
                None => 0,
            },
            preferred_side: location.preferred_side,
            preferred_layer: location
                .has_preferred_layer
                .map(|HasPreferredLayer::PreferredLayer(l)| l),
            display_ll: location.display_ll.as_ref().map(|ll| point_of(Some(ll))),
            is_through: matches!(
                location_type,
                Some(LocationType::KThrough | LocationType::KBreakThrough)
            ),
            min_inbound: match location.has_minimum_inbound_reachability {
                Some(HasMinimumInboundReachability::MinimumInboundReachability(r)) => r,
                None => 0,
            },
            min_outbound: match location.has_minimum_outbound_reachability {
                Some(HasMinimumOutboundReachability::MinimumOutboundReachability(r)) => r,
                None => 0,
            },
            filter: Filter::from_proto(location.search_filter.as_ref()),
        }
    }

    /// Whether the location's heading rules out an edge at `angle` (`heading_filter`).
    fn heading_excludes(&self, angle: f32) -> bool {
        let Some(heading) = self.heading else {
            return false;
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "mainline compares the uint32 heading and tolerance as floats"
        )]
        let (heading, tolerance) = (heading as f32, self.heading_tolerance as f32);
        if heading > angle {
            (heading - angle).min((360.0 - heading) + angle) > tolerance
        } else {
            (angle - heading).min((360.0 - angle) + heading) > tolerance
        }
    }

    /// Whether the location's preferred layer rules out an edge on `layer` (`layer_filter`).
    fn layer_excludes(&self, layer: i8) -> bool {
        self.preferred_layer
            .is_some_and(|preferred| preferred != i32::from(layer))
    }
}

/// What a location has found on the edge being scanned (part of mainline's `candidate_t`).
#[derive(Clone, Copy, Debug, Default)]
struct Scan {
    sq_distance: f64,
    distance: f64,
    point: PointLL,
    index: usize,
    prefiltered: bool,
}

/// A candidate edge for a location (mainline's `candidate_t`).
#[derive(Clone)]
struct Candidate<T> {
    sq_distance: f64,
    distance: f64,
    point: PointLL,
    index: usize,
    /// The bounding circle's center and radius (stored as an `uint16_t`).
    bounding_circle: (PointLL, u16),
    edge_id: GraphId,
    edge: DirectedEdge,
    /// The edge's shape, as its edge info stores it.
    shape: Rc<[PointLL]>,
    tile: T,
}

impl<T> Candidate<T> {
    /// Which side of the edge the location is on (`candidate_t::get_side`).
    fn side(
        &self,
        original: PointLL,
        tang_angle: f32,
        sq_distance: f64,
        sq_tolerance: f64,
        sq_max_distance: f64,
    ) -> SideOfStreet {
        // Within 10 degrees of straight ahead or behind is neither side.
        const ANGLE_TOLERANCE: f64 = 10.0;
        if sq_distance < sq_tolerance || sq_distance > sq_max_distance {
            return SideOfStreet::KNone;
        }
        let angle_to_point = self.point.heading(original);
        let mut angle_diff = angle_to_point - f64::from(tang_angle);
        if angle_diff < 0.0 {
            angle_diff += 360.0;
        }
        if angle_diff > ANGLE_TOLERANCE && angle_diff < 180.0 - ANGLE_TOLERANCE {
            SideOfStreet::KRight
        } else if angle_diff > 180.0 + ANGLE_TOLERANCE && angle_diff < 360.0 - ANGLE_TOLERANCE {
            SideOfStreet::KLeft
        } else {
            SideOfStreet::KNone
        }
    }
}

/// The other side of the street (`flip_side`).
fn flip_side(side: SideOfStreet) -> SideOfStreet {
    match side {
        SideOfStreet::KNone => SideOfStreet::KNone,
        SideOfStreet::KLeft => SideOfStreet::KRight,
        SideOfStreet::KRight => SideOfStreet::KLeft,
    }
}

/// The distance along the shape over which headings are measured (`GetOffsetForHeading`).
pub(crate) fn offset_for_heading(edge: &DirectedEdge) -> f32 {
    let rc = edge.classification().discriminant();
    let mut offset = 15.0_f32;
    if rc < 2 {
        offset *= 1.6;
    } else if rc < 5 {
        offset *= 1.4;
    }
    if matches!(
        edge.road_use().ok(),
        Some(
            RoadUse::Cycleway
                | RoadUse::MountainBike
                | RoadUse::Footway
                | RoadUse::Steps
                | RoadUse::Path
                | RoadUse::Pedestrian
                | RoadUse::Bridleway
        )
    ) {
        offset *= 0.5;
    }
    offset
}

/// A location's progress through the bins (mainline's `projector_wrapper`).
struct Projection<T> {
    /// The index of the location in the slice being searched.
    location: usize,
    params: Params,
    binner: ClosestFirst,
    /// The tile of the current bin, or `None` once the location is finished.
    cur_tile: Option<T>,
    /// The tile's ID, for grouping and ordering.
    cur_tile_id: u64,
    bin_index: u16,
    bin_center: PointLL,
    bin_center_approximator: DistanceApproximator,
    sq_radius: f64,
    unreachable: Vec<Candidate<T>>,
    reachable: Vec<Candidate<T>>,
    closest_external_reachable: f64,
    project: Projector,
}

impl<T: GraphTile + Clone> Projection<T> {
    fn has_bin(&self) -> bool {
        self.cur_tile.is_some()
    }

    fn has_same_bin(&self, other: &Self) -> bool {
        self.cur_tile.is_some() == other.cur_tile.is_some()
            && self.cur_tile_id == other.cur_tile_id
            && self.bin_index == other.bin_index
    }

    /// Mainline's `operator<`: unfinished first, then by tile (descending), then by bin.
    fn less(&self, other: &Self) -> bool {
        let key = |p: &Self| if p.has_bin() { p.cur_tile_id } else { 0 };
        if key(self) != key(other) {
            return key(self) > key(other);
        }
        self.bin_index < other.bin_index
    }

    /// Moves to the next bin that might hold something useful (`next_bin`).
    fn next_bin<P: GraphTileProvider<TileHandle = T>>(
        &mut self,
        graph: GraphReader<'_, P>,
    ) -> Result<(), SearchError> {
        loop {
            let (tile_index, bin_index, distance) =
                self.binner.next_bin().ok_or(SearchError::Exhausted)?;
            self.bin_index = bin_index;
            if distance > f64::from(self.params.search_cutoff)
                || (!self.reachable.is_empty()
                    && distance > f64::from(self.params.radius)
                    && self
                        .reachable
                        .last()
                        .is_some_and(|best| distance > best.sq_distance.sqrt()))
            {
                self.cur_tile = None;
                return Ok(());
            }
            self.bin_center = bin_center(tile_index, bin_index);
            self.bin_center_approximator = DistanceApproximator::new(self.bin_center);
            let tile = u64::try_from(tile_index)
                .ok()
                .and_then(|t| GraphId::try_from_components(2, t, 0).ok());
            self.cur_tile = match tile {
                Some(id) => graph.tile(id)?,
                None => None,
            };
            if let Some(tile) = &self.cur_tile {
                self.cur_tile_id = tile.graph_id().value();
                return Ok(());
            }
        }
    }
}

/// The search over a set of locations (mainline's `bin_handler_t`).
struct Handler<'g, P: GraphTileProvider, C> {
    graph: GraphReader<'g, P>,
    costing: &'g C,
    pps: Vec<Projection<Tile<P>>>,
    max_reach_limit: u32,
    bin_candidates: Vec<Scan>,
    correlated_edges: HashSet<GraphId>,
    reach_finder: Reach,
    directed_reaches: HashMap<GraphId, DirectedReach>,
}

/// Correlates each location to the edges near it, filling in its `correlation`
/// (`Search::search`).
///
/// The locations must have Loki's defaults filled in (see [`crate::parse_location`]).
///
/// # Errors
///
/// Fails if the graph can't be read, or if a location's search cutoff covers the world.
pub fn search<P: GraphTileProvider, C: Costing>(
    locations: &mut [Location],
    provider: &P,
    costing: &C,
) -> Result<(), SearchError> {
    if locations.is_empty() {
        return Ok(());
    }
    let graph = GraphReader::new(provider);
    let mut handler = Handler {
        graph,
        costing,
        pps: Vec::with_capacity(locations.len()),
        max_reach_limit: 0,
        bin_candidates: Vec::new(),
        correlated_edges: HashSet::new(),
        reach_finder: Reach::default(),
        directed_reaches: HashMap::new(),
    };
    handler.run(locations)
}

impl<P: GraphTileProvider, C: Costing> Handler<'_, P, C> {
    fn run(&mut self, locations: &mut [Location]) -> Result<(), SearchError> {
        for (index, location) in locations.iter().enumerate() {
            let params = Params::from_location(location);
            self.max_reach_limit = self
                .max_reach_limit
                .max(params.min_outbound)
                .max(params.min_inbound);
            let mut projection = Projection {
                location: index,
                params,
                binner: ClosestFirst::new(params.ll),
                cur_tile: None,
                cur_tile_id: 0,
                bin_index: 0,
                bin_center: PointLL::default(),
                bin_center_approximator: DistanceApproximator::new(PointLL::default()),
                sq_radius: f64::from(params.radius) * f64::from(params.radius),
                unreachable: Vec::new(),
                reachable: Vec::new(),
                closest_external_reachable: f64::MAX,
                project: Projector::new(params.ll),
            };
            projection.next_bin(self.graph)?;
            self.pps.push(projection);
        }
        self.bin_candidates.resize(self.pps.len(), Scan::default());

        sort(&mut self.pps);
        while self.pps.first().is_some_and(Projection::has_bin) {
            let (begin, end) = self.find_best_range();
            self.handle_bin(begin, end)?;
            let finished = partition(&mut self.pps);
            sort(&mut self.pps[..finished]);
        }
        self.finalize(locations)
    }

    /// The largest run of locations in the same bin (`find_best_range`).
    fn find_best_range(&self) -> (usize, usize) {
        let pps = &self.pps;
        let mut best = (0, 0);
        let mut cur = (0, 0);
        while cur.1 != pps.len() && pps[cur.0].has_bin() {
            cur.0 = cur.1;
            let first = &pps[cur.0];
            cur.1 = cur.0
                + pps[cur.0..]
                    .iter()
                    .position(|pp| !first.has_same_bin(pp))
                    .unwrap_or(pps.len() - cur.0);
            if pps[cur.0].has_bin() && cur.1 - cur.0 > best.1 - best.0 {
                best = cur;
            }
        }
        best
    }

    /// The reach of an edge, from the cache or computed (`get_reach`).
    fn get_reach(
        &mut self,
        edge_id: GraphId,
        edge: &DirectedEdge,
    ) -> Result<DirectedReach, GraphError> {
        if let Some(reach) = self.directed_reaches.get(&edge_id) {
            return Ok(*reach);
        }
        let reach = self.reach_finder.reach(
            edge,
            edge_id,
            self.max_reach_limit,
            self.graph,
            self.costing,
        )?;
        self.directed_reaches.insert(edge_id, reach);
        Ok(reach)
    }

    /// The reach of an edge if it could become a location's best reachable candidate,
    /// or an assumed full reach otherwise (`check_reachability`).
    fn check_reachability(
        &mut self,
        begin: usize,
        end: usize,
        tile: &Tile<P>,
        edge: &DirectedEdge,
        edge_id: GraphId,
    ) -> Result<DirectedReach, GraphError> {
        if self.max_reach_limit == 0 {
            return Ok(DirectedReach::default());
        }
        if let Some(reach) = self.directed_reaches.get(&edge_id) {
            return Ok(*reach);
        }
        let check = self.pps[begin..end]
            .iter()
            .zip(&self.bin_candidates)
            .any(|(pp, candidate)| {
                pp.reachable
                    .last()
                    .is_none_or(|best| candidate.sq_distance < best.sq_distance)
            });
        if !check {
            return Ok(DirectedReach::new(
                self.max_reach_limit,
                self.max_reach_limit,
            ));
        }
        let reach = self.reach_finder.reach(
            edge,
            edge_id,
            self.max_reach_limit,
            self.graph,
            self.costing,
        )?;
        self.directed_reaches.insert(edge_id, reach);
        // A two-way edge reaches as far as its opposing edge, if that's allowed.
        if reach.outbound > 0
            && reach.inbound > 0
            && let Some(end_tile) = self.graph.tile_from(edge.end_node_id(), tile)?
        {
            let opp_id = valhalla_graphtile::graph_reader::opposing_in_end_tile(&end_tile, edge)?;
            let opp = get_edge(&end_tile, opp_id)?;
            if self
                .costing
                .edge_filter(opp, &end_tile, DisallowMask::SHORTCUT)
            {
                self.directed_reaches.insert(opp_id, reach);
            }
        }
        Ok(reach)
    }

    /// Projects the locations in `pps[begin..end]` (which share a bin) onto the bin's edges
    /// (`handle_bin`).
    #[expect(
        clippy::too_many_lines,
        reason = "a step by step port of mainline's loop"
    )]
    fn handle_bin(&mut self, begin: usize, end: usize) -> Result<(), SearchError> {
        let Some(bin_tile) = self.pps[begin].cur_tile.clone() else {
            return Ok(());
        };
        let bin_index = usize::from(self.pps[begin].bin_index);
        let bin_center = self.pps[begin].bin_center;
        let has_bounding_circles = bin_tile.header().has_bounding_circles();
        let edges = bin_tile.edges_in_bin(bin_index);
        let circles = bin_tile.bounding_circles_in_bin(bin_index);
        let count = end - begin;
        let mut tile = bin_tile.clone();

        for (edge_index, &bin_edge_id) in edges.iter().enumerate() {
            let mut edge_id = bin_edge_id;
            let mut all_prefiltered = true;
            let mut circle = (PointLL::new(0.0, 0.0), 0.0_f64);
            if has_bounding_circles
                && let Some(discretized) = circles.get(edge_index)
                && let Some(decoded) = discretized.decode(geo::coord! {
                    x: bin_center.lng,
                    y: bin_center.lat,
                })
            {
                circle = (PointLL::from(decoded.center), decoded.radius_meters);
            }
            let radius = circle.1;

            for candidate in &mut self.bin_candidates[..count] {
                candidate.prefiltered = false;
            }

            // A zero radius means there is no circle.
            if radius != 0.0 {
                let margin_radius = radius + CIRCLE_MARGIN_METERS;
                for k in 0..count {
                    let pp = &self.pps[begin + k];
                    let candidate = &mut self.bin_candidates[k];
                    let dsqr = pp.project.approx.distance_squared(circle.0);
                    let cutoff = f64::from(pp.params.search_cutoff);
                    if dsqr > (cutoff + margin_radius) * (cutoff + margin_radius) {
                        candidate.prefiltered = true;
                        continue;
                    }
                    // Without a reachable candidate, nothing within the cutoff can be ruled out.
                    let Some(best) = pp.reachable.last() else {
                        all_prefiltered = false;
                        break;
                    };
                    let location_radius = f64::from(pp.params.radius);
                    if best.sq_distance < pp.sq_radius
                        && dsqr
                            > (location_radius + margin_radius) * (location_radius + margin_radius)
                    {
                        // Something is in the radius, and this edge is entirely outside it.
                        candidate.prefiltered = true;
                    } else {
                        // Could the closest point of this edge beat the best so far?
                        candidate.distance = best.distance;
                        let min_distance = dsqr.sqrt() - margin_radius;
                        candidate.prefiltered = min_distance >= cutoff
                            || (min_distance >= location_radius
                                && min_distance >= candidate.distance);
                    }
                    if !candidate.prefiltered {
                        all_prefiltered = false;
                        break;
                    }
                }
                if all_prefiltered {
                    continue;
                }
            }
            all_prefiltered = true;

            let Some(edge_tile) = self.graph.tile_from(edge_id, &tile)? else {
                continue;
            };
            tile = edge_tile;
            let mut edge = get_edge(&tile, edge_id)?.clone();

            // If the edge isn't allowed, try its opposing edge instead.
            if !self
                .costing
                .edge_filter(&edge, &tile, DisallowMask::SHORTCUT)
            {
                let Some((opp_id, opp_tile)) = self.graph.opposing(edge_id, &tile)? else {
                    continue;
                };
                let opp = get_edge(&opp_tile, opp_id)?.clone();
                if !self
                    .costing
                    .edge_filter(&opp, &opp_tile, DisallowMask::SHORTCUT)
                {
                    continue;
                }
                edge = opp;
                tile = opp_tile;
                edge_id = opp_id;
            }

            // Reset the candidates and apply the search filters. A filter rules out the pair
            // only if it rules out both directions (closures can close one).
            for k in 0..count {
                let filter = self.pps[begin + k].params.filter;
                let candidate = &mut self.bin_candidates[k];
                candidate.sq_distance = f64::MAX;
                candidate.distance = f64::MAX;
                if !candidate.prefiltered
                    && filter.excludes(&edge, &tile)?
                    && let Some((opp_id, opp_tile)) = self.graph.opposing(edge_id, &tile)?
                {
                    let opp = get_edge(&opp_tile, opp_id)?;
                    candidate.prefiltered = filter.excludes(opp, &opp_tile)?;
                }
                all_prefiltered = all_prefiltered && self.bin_candidates[k].prefiltered;
            }
            if all_prefiltered {
                continue;
            }

            // Project each location onto each segment of the shape.
            let info = tile.get_edge_info(&edge).map_err(GraphError::from)?;
            let edge_shape: Rc<[PointLL]> = Rc::from(shape(&info)?);
            if let Some((&first, rest)) = edge_shape.split_first() {
                let mut v = first;
                for (i, &next) in rest.iter().enumerate() {
                    let u = v;
                    v = next;
                    for k in 0..count {
                        let candidate = &mut self.bin_candidates[k];
                        if candidate.prefiltered {
                            continue;
                        }
                        let project = &self.pps[begin + k].project;
                        let point = project.project(u, v);
                        let sq_distance = project.approx.distance_squared(point);
                        if sq_distance < candidate.sq_distance {
                            candidate.sq_distance = sq_distance;
                            candidate.distance = sq_distance.sqrt();
                            candidate.point = point;
                            candidate.index = i;
                        }
                    }
                }
            }

            let mut reach = self.check_reachability(begin, end, &tile, &edge, edge_id)?;

            for k in 0..count {
                if self.bin_candidates[k].prefiltered {
                    continue;
                }
                let params = self.pps[begin + k].params;
                let mut reachable =
                    reach.outbound >= params.min_outbound && reach.inbound >= params.min_inbound;
                // It may not be reachable while its opposing edge is: switch to that if so.
                if !reachable
                    && let Some((opp_id, opp_tile)) = self.graph.opposing(edge_id, &tile)?
                {
                    let opp = get_edge(&opp_tile, opp_id)?.clone();
                    if self
                        .costing
                        .edge_filter(&opp, &opp_tile, DisallowMask::SHORTCUT)
                        && !params.filter.excludes(&opp, &opp_tile)?
                    {
                        let opp_reach =
                            self.check_reachability(begin, end, &opp_tile, &opp, opp_id)?;
                        if opp_reach.outbound >= params.min_outbound
                            && opp_reach.inbound >= params.min_inbound
                        {
                            tile = opp_tile;
                            edge = opp;
                            edge_id = opp_id;
                            reach = opp_reach;
                            reachable = true;
                        }
                    }
                }

                let scan = self.bin_candidates[k];
                // The radius is stored in an unsigned short.
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "mainline converts the double radius to uint16_t"
                )]
                let stored_radius = circle.1 as u16;
                let candidate = Candidate {
                    sq_distance: scan.sq_distance,
                    distance: scan.distance,
                    point: scan.point,
                    index: scan.index,
                    bounding_circle: (circle.0, stored_radius),
                    edge_id,
                    edge: edge.clone(),
                    shape: Rc::clone(&edge_shape),
                    tile: tile.clone(),
                };

                let pp = &mut self.pps[begin + k];
                let sq_radius = pp.sq_radius;
                let batch = if reachable {
                    &mut pp.reachable
                } else {
                    &mut pp.unreachable
                };
                let Some(last) = batch.last() else {
                    let sq_distance = candidate.sq_distance;
                    batch.push(candidate);
                    if reachable && sq_distance < pp.closest_external_reachable {
                        pp.closest_external_reachable = sq_distance;
                    }
                    continue;
                };
                let in_radius = candidate.sq_distance < sq_radius;
                let better = candidate.sq_distance < last.sq_distance;
                let last_in_radius = last.sq_distance < sq_radius;
                let last_sq_distance = last.sq_distance;
                let closer_external_reachable =
                    reachable && candidate.sq_distance < pp.closest_external_reachable;
                if in_radius || better {
                    if !last_in_radius {
                        // The last one wasn't in the radius, so this one replaces it.
                        if closer_external_reachable {
                            pp.closest_external_reachable = last_sq_distance;
                        }
                        if let Some(last) = batch.last_mut() {
                            *last = candidate;
                        }
                    } else if better {
                        // Both are in the radius, and this one is better: it goes last.
                        batch.push(candidate);
                    } else {
                        // Both are in the radius, but the last one is better: it stays last.
                        batch.push(candidate);
                        let n = batch.len();
                        batch.swap(n - 1, n - 2);
                    }
                } else if closer_external_reachable {
                    pp.closest_external_reachable = candidate.sq_distance;
                }
            }
        }

        // The bin is done; move each location on to its next bin.
        for pp in &mut self.pps[begin..end] {
            pp.next_bin(self.graph)?;
        }
        Ok(())
    }

    /// Turns each location's winning candidates into path edges (`finalize`).
    fn finalize(&mut self, locations: &mut [Location]) -> Result<(), SearchError> {
        let mut pps = core::mem::take(&mut self.pps);
        for pp in &mut pps {
            let Some(location) = locations.get_mut(pp.location) else {
                continue;
            };
            // Drop unreachable candidates farther than the closest reachable one outside the radius.
            let closest = pp.closest_external_reachable;
            pp.unreachable.retain(|c| c.sq_distance <= closest);
            let mut candidates = core::mem::take(&mut pp.reachable);
            candidates.append(&mut pp.unreachable);
            // Closest first. Mainline's `std::sort` isn't stable; we break ties by edge ID.
            candidates.sort_by(|a, b| {
                a.sq_distance
                    .total_cmp(&b.sq_distance)
                    .then(a.edge_id.value().cmp(&b.edge_id.value()))
            });
            self.correlated_edges.clear();
            let correlation = location
                .correlation
                .get_or_insert_with(Correlation::default);
            let mut out = Output {
                edges: core::mem::take(&mut correlation.edges),
                filtered: core::mem::take(&mut correlation.filtered_edges),
            };
            let pt = pp.params.ll;
            let snap = f64::from(pp.params.node_snap_tolerance);
            for candidate in &candidates {
                let edge = &candidate.edge;
                let (Some(&front), Some(&back)) = (candidate.shape.first(), candidate.shape.last())
                else {
                    continue;
                };
                // The point may be at a node, because it was closest or by the snap tolerance.
                let at_front = candidate.point == front || pt.distance(front) < snap;
                let at_back = candidate.point == back || pt.distance(back) < snap;
                let forward = edge.edge_info_is_forward();
                if (at_front && forward) || (at_back && !forward) {
                    // The begin node: the end node of the opposing edge.
                    let Some((opp_id, opp_tile)) =
                        self.graph.opposing(candidate.edge_id, &candidate.tile)?
                    else {
                        continue;
                    };
                    let begin = get_edge(&opp_tile, opp_id)?.end_node_id();
                    self.correlate_node(&pp.params, begin, candidate, &mut out)?;
                } else if (at_back && forward) || (at_front && !forward) {
                    self.correlate_node(&pp.params, edge.end_node_id(), candidate, &mut out)?;
                } else {
                    self.correlate_edge(&pp.params, candidate, &mut out)?;
                }
            }

            // For a through location with a heading, Loki takes the heading to be the one to
            // leave on: at a node, only the outbound edges stay.
            if pp.params.is_through && pp.params.heading.is_some() {
                let (keep, moved): (Vec<_>, Vec<_>) =
                    out.edges.into_iter().partition(|e| !e.end_node);
                out.edges = keep;
                out.filtered.extend(moved);
            }
            for e in out.edges.iter_mut().chain(out.filtered.iter_mut()) {
                e.names = self.names(e.graph_id)?;
            }
            let correlation = location
                .correlation
                .get_or_insert_with(Correlation::default);
            correlation.edges = out.edges;
            correlation.filtered_edges = out.filtered;
        }
        Ok(())
    }

    /// An edge's names, without tagged values (`EdgeInfo::GetNames()`).
    fn names(&self, graph_id: u64) -> Result<Vec<String>, GraphError> {
        let Ok(id) = GraphId::try_from_id(graph_id) else {
            return Ok(Vec::new());
        };
        let Some(tile) = self.graph.tile(id)? else {
            return Ok(Vec::new());
        };
        let edge = get_edge(&tile, id)?;
        let info = tile.get_edge_info(edge)?;
        Ok(info
            .names()
            .map(|name| name.map(|name| name.text_lossy().into_owned()))
            .collect::<Result<_, _>>()?)
    }

    /// Adds every edge at a node (on every level) for a candidate at that node
    /// (`correlate_node`).
    fn correlate_node(
        &mut self,
        params: &Params,
        found_node: GraphId,
        candidate: &Candidate<Tile<P>>,
        out: &mut Output,
    ) -> Result<(), GraphError> {
        let pt = params.ll;
        // The search cutoff is a hard filter.
        if candidate.point.distance(pt) > f64::from(params.search_cutoff) {
            return Ok(());
        }
        let mut distance = None;
        let Some(tile) = self.graph.tile(found_node)? else {
            return Ok(());
        };
        let node = get_node(&tile, found_node)?;
        let mut nodes = vec![(found_node, tile.clone())];
        for transition in tile.get_transitions(node) {
            let id = transition.corresponding_end_node_id();
            if let Some(t) = self.graph.tile(id)? {
                nodes.push((id, t));
            }
        }
        for (node_id, tile) in nodes {
            self.crawl(params, node_id, &tile, candidate, &mut distance, out)?;
        }
        Ok(())
    }

    fn crawl(
        &mut self,
        params: &Params,
        node_id: GraphId,
        tile: &Tile<P>,
        candidate: &Candidate<Tile<P>>,
        distance: &mut Option<f64>,
        out: &mut Output,
    ) -> Result<(), GraphError> {
        let pt = params.ll;
        let node = get_node(tile, node_id)?.clone();
        let ll = node_ll(tile, &node);
        let distance = *distance.get_or_insert_with(|| ll.distance(pt));
        let first = u64::from(node.edge_index());
        let circle = bounding_circle(candidate.bounding_circle);
        for (i, edge) in tile.get_outbound_edges_from_node(&node).iter().enumerate() {
            let id = tile
                .graph_id()
                .with_feature_index(first + i as u64)
                .map_err(valhalla_graphtile::tile_provider::GraphTileProviderError::from)?;
            let info = tile.get_edge_info(edge)?;
            let edge_shape = shape(&info)?;
            // The heading of the shape at the snapped point.
            let forward = edge.edge_info_is_forward();
            let index = if forward {
                0
            } else {
                edge_shape.len().wrapping_sub(2)
            };
            let angle = tangent_angle(
                index,
                candidate.point,
                &edge_shape,
                offset_for_heading(edge),
                forward,
            );
            let layer = info.layer()?;
            if self.costing.edge_filter(edge, tile, DisallowMask::SHORTCUT)
                && !params.filter.excludes(edge, tile)?
            {
                let reach = self.get_reach(id, edge)?;
                let path_edge = PathEdge {
                    graph_id: id.value(),
                    percent_along: 0.0,
                    ll: Some(crate::search::lat_lng(ll)),
                    distance,
                    inbound_reach: reach_i32(reach.inbound),
                    outbound_reach: reach_i32(reach.outbound),
                    begin_node: true,
                    heading: angle,
                    side_of_street: SideOfStreet::KNone.into(),
                    bounding_circle: Some(circle),
                    ..PathEdge::default()
                };
                self.add_at_node(params, angle, layer, id, path_edge, out);
            }

            // The opposing edge.
            let Some((other_id, other_tile)) = self.graph.opposing(id, tile)? else {
                continue;
            };
            let other = get_edge(&other_tile, other_id)?.clone();
            if self
                .costing
                .edge_filter(&other, &other_tile, DisallowMask::SHORTCUT)
                && !params.filter.excludes(&other, &other_tile)?
            {
                let opp_angle = (angle + 180.0) % 360.0;
                let reach = self.get_reach(other_id, &other)?;
                let path_edge = PathEdge {
                    graph_id: other_id.value(),
                    percent_along: 1.0,
                    ll: Some(crate::search::lat_lng(ll)),
                    distance,
                    inbound_reach: reach_i32(reach.inbound),
                    outbound_reach: reach_i32(reach.outbound),
                    end_node: true,
                    heading: opp_angle,
                    side_of_street: SideOfStreet::KNone.into(),
                    bounding_circle: Some(circle),
                    ..PathEdge::default()
                };
                self.add_at_node(params, opp_angle, layer, other_id, path_edge, out);
            }
        }
        Ok(())
    }

    /// Adds a path edge at a node, unless it is already there
    /// (filtered if the heading or layer rules it out).
    fn add_at_node(
        &mut self,
        params: &Params,
        angle: f32,
        layer: i8,
        id: GraphId,
        path_edge: PathEdge,
        out: &mut Output,
    ) {
        if params.heading_excludes(angle) || params.layer_excludes(layer) {
            if self.correlated_edges.insert(id) {
                out.filtered.push(path_edge);
            }
        } else if self.correlated_edges.insert(id) {
            out.edges.push(path_edge);
        }
    }

    /// Adds a candidate along an edge, and its opposing edge (`correlate_edge`).
    #[expect(
        clippy::too_many_lines,
        reason = "a step by step port of mainline's function"
    )]
    fn correlate_edge(
        &mut self,
        params: &Params,
        candidate: &Candidate<Tile<P>>,
        out: &mut Output,
    ) -> Result<(), GraphError> {
        let pt = params.ll;
        let distance = candidate.point.distance(pt);
        if distance > f64::from(params.search_cutoff) {
            return Ok(());
        }
        let (edge, tile) = (&candidate.edge, &candidate.tile);
        let shape = &candidate.shape;
        // The fraction of the edge, in the edge's direction.
        let mut partial_length = 0.0;
        for i in 0..candidate.index {
            if let (Some(a), Some(b)) = (shape.get(i), shape.get(i + 1)) {
                partial_length += a.distance(*b);
            }
        }
        if let Some(a) = shape.get(candidate.index) {
            partial_length += a.distance(candidate.point);
        }
        let length = f64::from(edge.length());
        // The edge length has meter resolution, so the shape can be a little longer.
        partial_length = partial_length.min(length);
        let mut length_ratio = partial_length / length;
        let forward = edge.edge_info_is_forward();
        if !forward {
            length_ratio = 1.0 - length_ratio;
        }
        let angle = tangent_angle(
            candidate.index,
            candidate.point,
            shape,
            offset_for_heading(edge),
            forward,
        );
        let info = tile.get_edge_info(edge)?;
        let layer = info.layer()?;
        let sq_tolerance =
            f64::from(params.street_side_tolerance) * f64::from(params.street_side_tolerance);
        let sq_max_distance =
            f64::from(params.street_side_max_distance) * f64::from(params.street_side_max_distance);
        let side = match params.display_ll {
            Some(display) => candidate.side(
                display,
                angle,
                display.distance_squared(candidate.point),
                sq_tolerance,
                sq_max_distance,
            ),
            None => candidate.side(
                pt,
                angle,
                candidate.sq_distance,
                sq_tolerance,
                sq_max_distance,
            ),
        };
        let reach = self.get_reach(candidate.edge_id, edge)?;
        let path_edge = PathEdge {
            graph_id: candidate.edge_id.value(),
            percent_along: length_ratio,
            ll: Some(lat_lng(candidate.point)),
            distance,
            inbound_reach: reach_i32(reach.inbound),
            outbound_reach: reach_i32(reach.outbound),
            heading: angle,
            side_of_street: side.into(),
            bounding_circle: Some(bounding_circle(candidate.bounding_circle)),
            ..PathEdge::default()
        };

        let hard_filtered = params.filter.excludes(edge, tile)?;
        if !hard_filtered
            && (self.side_excludes(params, &path_edge)?
                || params.heading_excludes(angle)
                || params.layer_excludes(layer))
        {
            out.filtered.push(path_edge);
        } else if !hard_filtered && self.correlated_edges.insert(candidate.edge_id) {
            out.edges.push(path_edge);
        }

        // Its evil twin.
        if let Some((opp_id, opp_tile)) = self.graph.opposing(candidate.edge_id, tile)? {
            let opp = get_edge(&opp_tile, opp_id)?.clone();
            if self
                .costing
                .edge_filter(&opp, &opp_tile, DisallowMask::SHORTCUT)
                && !params.filter.excludes(&opp, &opp_tile)?
            {
                let opp_angle = (angle + 180.0) % 360.0;
                let reach = self.get_reach(opp_id, &opp)?;
                // Mainline sets the bounding circle on the first path edge (already moved into
                // the correlation) instead of this one, so this one has none.
                let other = PathEdge {
                    graph_id: opp_id.value(),
                    percent_along: 1.0 - length_ratio,
                    ll: Some(lat_lng(candidate.point)),
                    distance,
                    inbound_reach: reach_i32(reach.inbound),
                    outbound_reach: reach_i32(reach.outbound),
                    heading: opp_angle,
                    side_of_street: flip_side(side).into(),
                    ..PathEdge::default()
                };
                if self.side_excludes(params, &other)?
                    || params.heading_excludes(opp_angle)
                    || params.layer_excludes(layer)
                {
                    out.filtered.push(other);
                } else if self.correlated_edges.insert(opp_id) {
                    out.edges.push(other);
                }
            }
        }
        Ok(())
    }

    /// Whether the location's preferred side rules out a path edge (`side_filter`).
    fn side_excludes(&self, params: &Params, path_edge: &PathEdge) -> Result<bool, GraphError> {
        if path_edge.side_of_street == i32::from(SideOfStreet::KNone)
            || params.preferred_side == i32::from(PreferredSide::Either)
        {
            return Ok(false);
        }
        let Ok(id) = GraphId::try_from_id(path_edge.graph_id) else {
            return Ok(false);
        };
        let Some(tile) = self.graph.tile(id)? else {
            return Ok(false);
        };
        let Some((opp_id, opp_tile)) = self.graph.opposing(id, &tile)? else {
            return Ok(false);
        };
        let opp = get_edge(&opp_tile, opp_id)?;
        // Minor roads aren't filtered (a higher road class number is a smaller road).
        if u32::from(opp.classification().discriminant()) > params.street_side_cutoff {
            return Ok(false);
        }
        let Some(end_tile) = self.graph.tile_from(opp.end_node_id(), &opp_tile)? else {
            return Ok(false);
        };
        let node = get_node(&end_tile, opp.end_node_id())?;
        // On the right and driving on the right, or not and not, is the side you drive on.
        let same =
            node.drive_on_right() == (path_edge.side_of_street == i32::from(SideOfStreet::KRight));
        Ok(same != (params.preferred_side == i32::from(PreferredSide::Same)))
    }
}

/// The path edges found for a location.
struct Output {
    edges: Vec<PathEdge>,
    filtered: Vec<PathEdge>,
}

/// A bounding circle for a path edge.
fn bounding_circle((center, radius): (PointLL, u16)) -> BoundingCircle {
    BoundingCircle {
        radius: u32::from(radius),
        latlng: Some(lat_lng(center)),
    }
}

/// A reach as the protobuf's `int32`.
fn reach_i32(reach: u32) -> i32 {
    i32::try_from(reach).unwrap_or(i32::MAX)
}

/// Sorts the projections (see [`Projection::less`]); a stable sort, so groups keep their order.
fn sort<T: GraphTile + Clone>(pps: &mut [Projection<T>]) {
    pps.sort_by(|a, b| {
        if a.less(b) {
            core::cmp::Ordering::Less
        } else if b.less(a) {
            core::cmp::Ordering::Greater
        } else {
            core::cmp::Ordering::Equal
        }
    });
}

/// Moves the unfinished projections first (keeping their order);
/// returns where the finished ones start.
fn partition<T: GraphTile + Clone>(pps: &mut [Projection<T>]) -> usize {
    pps.sort_by_key(|pp| !pp.has_bin());
    pps.iter().take_while(|pp| pp.has_bin()).count()
}
