//! The route action (mainline's `thor_worker_t::route` and `route_action.cc`):
//! picking a path algorithm for each pair of locations, retrying with relaxed limits,
//! joining the segments between through locations into legs,
//! and moving the locations' times along the route.

use std::collections::BTreeMap;

use valhalla_graphtile::GraphId;
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::costing::HasOptions;
use valhalla_proto::location::{
    HasMinimumInboundReachability, HasMinimumOutboundReachability, Type as LocationType,
};
use valhalla_proto::options::{DateTimeType, HasAlternates};
use valhalla_proto::{
    Api, CodedDescription, HierarchyLimits, Info, Location, Options, PathEdge as CandidateEdge,
};
use valhalla_response::{ErrorCode, ValhallaError};
use valinor_sif::costing_options::costing_name;
use valinor_sif::{AnyCosting, Costing, UNLIMITED_TRANSITIONS};

use crate::bidirectional::{BidirectionalAStar, BidirectionalParams};
use crate::config::{HierarchyLimitsConfig, Retention, ThorConfig, is_unset};
use crate::error::ThorError;
use crate::graph;
use crate::path::{EdgeTrim, EdgeTrimming, Leg, PathEdge, Paths, Route};
use crate::search::{SearchContext, candidate_id, candidates, point_of};
use crate::time::offset_date;
use crate::unidirectional::UnidirectionalAStar;

/// Pedestrian routes with a ferry are searched again with the second pass's A* factor,
/// up to this distance in meters (`kPedestrianMultipassThreshold`).
const PEDESTRIAN_MULTIPASS_THRESHOLD: f64 = 50_000.0;

/// The largest snap score by costing (`kMaxDistances`), in "seconds".
fn max_distance(costing: &str) -> Option<f32> {
    Some(match costing {
        "auto" | "auto_data_fix" | "auto_shorter" | "bus" | "hov" | "truck" | "taxi"
        | "auto_pedestrian" => 43_200.0,
        "bicycle" | "multimodal" | "pedestrian" | "bikeshare" => 7200.0,
        "motor_scooter" | "motorcycle" | "transit" => 14_400.0,
        _ => return None,
    })
}

/// Scores grow with the square of the distance, scaled by this (`kDistanceScale`).
const DISTANCE_SCALE: f32 = 10.0;

/// Mainline's warning messages for the codes Thor's route action adds.
fn warning_message(code: u64) -> &'static str {
    match code {
        209 => {
            "Customized hierarchy limits are not allowed on this server, using default hierarchy limits"
        }
        210 => {
            "Provided hierarchy limits exceeded maximum allowed values, using max allowed hierarchy limits"
        }
        214 => "Distance exceeded max_timedep_distance for arrive_by, probably ignoring date_time",
        401 => "Routing failed on first pass, retrying with relaxed restrictions",
        402 => {
            "Distance exceeded max_timedep_distance for depart_at, falling back to bidirectional A*"
        }
        _ => "",
    }
}

/// Adds a warning to the request (`add_warning`).
fn add_warning(api: &mut Api, code: u64) {
    api.info
        .get_or_insert_with(Info::default)
        .warnings
        .push(CodedDescription {
            description: warning_message(code).to_owned(),
            code,
        });
}

/// Scores each location's candidate edges for path search (`adjust_locations`):
/// candidates filtered by heading or side of street are used only if nothing else is
/// (warning 215) and otherwise cost more, and every distance becomes a score
/// (ten times its square, less the smallest, capped by costing).
pub(crate) fn adjust_locations(api: &mut Api, costing_name: &str) {
    let max_score = max_distance(costing_name);
    let Some(options) = api.options.as_mut() else {
        return;
    };
    let mut warnings = 0;
    for locations in [
        &mut options.locations,
        &mut options.sources,
        &mut options.targets,
    ] {
        for location in locations.iter_mut() {
            let skip_ranking = location.skip_ranking_candidates;
            let correlation = location.correlation.get_or_insert_with(Default::default);
            if correlation.edges.is_empty() && !correlation.filtered_edges.is_empty() {
                correlation.edges.append(&mut correlation.filtered_edges);
                warnings += 1;
            }
            if !correlation.filtered_edges.is_empty() {
                // `std::max_element` by distance: the first of the largest.
                let mut max: Option<f64> = None;
                for e in &correlation.edges {
                    if max.is_none_or(|m| m < e.distance) {
                        max = Some(e.distance);
                    }
                }
                if let Some(max) = max {
                    for e in &mut correlation.filtered_edges {
                        e.distance = e.distance + 3600.0 + max;
                    }
                }
            }
            let mut min_score = f32::MAX;
            for candidate in correlation
                .edges
                .iter_mut()
                .chain(correlation.filtered_edges.iter_mut())
            {
                if skip_ranking {
                    candidate.distance = 0.0;
                } else {
                    candidate.distance =
                        candidate.distance * candidate.distance * f64::from(DISTANCE_SCALE);
                }
                // The minimum is a float.
                if f64::from(min_score) > candidate.distance {
                    #[expect(clippy::cast_possible_truncation, reason = "mainline's float")]
                    let narrowed = candidate.distance as f32;
                    min_score = narrowed;
                }
            }
            for candidate in correlation
                .edges
                .iter_mut()
                .chain(correlation.filtered_edges.iter_mut())
            {
                candidate.distance -= f64::from(min_score);
                if let Some(max) = max_score
                    && candidate.distance > f64::from(max)
                {
                    candidate.distance = f64::from(max);
                }
            }
        }
    }
    for _ in 0..warnings {
        // Mainline adds this warning with a code but no description.
        api.info
            .get_or_insert_with(Info::default)
            .warnings
            .push(CodedDescription {
                description: String::new(),
                code: 215,
            });
    }
}

#[inline]
fn location_type(location: &Location) -> LocationType {
    LocationType::try_from(location.r#type).unwrap_or(LocationType::KBreak)
}

#[inline]
fn is_through_point(location: &Location) -> bool {
    matches!(
        location_type(location),
        LocationType::KThrough | LocationType::KBreakThrough
    )
}

#[inline]
fn is_break_point(location: &Location) -> bool {
    matches!(
        location_type(location),
        LocationType::KBreak | LocationType::KBreakThrough
    )
}

/// Whether a candidate edge meets the location's reachability (`is_highly_reachable`).
fn is_highly_reachable(location: &Location, edge: &CandidateEdge) -> bool {
    let inbound = match location.has_minimum_inbound_reachability {
        Some(HasMinimumInboundReachability::MinimumInboundReachability(r)) => r,
        None => 0,
    };
    let outbound = match location.has_minimum_outbound_reachability {
        Some(HasMinimumOutboundReachability::MinimumOutboundReachability(r)) => r,
        None => 0,
    };
    edge.inbound_reach.cast_unsigned() >= inbound && edge.outbound_reach.cast_unsigned() >= outbound
}

/// Removes a location's candidates (filtered or not) that match (`remove_path_edges`).
fn remove_path_edges(location: &mut Location, mut remove: impl FnMut(&CandidateEdge) -> bool) {
    if let Some(correlation) = location.correlation.as_mut() {
        correlation.edges.retain(|e| !remove(e));
        correlation.filtered_edges.retain(|e| !remove(e));
    }
}

/// The path algorithms (mainline's `PathAlgorithm` subclasses that routes use).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Algorithm {
    Bidirectional,
    TimeDepForward,
    TimeDepReverse,
}

impl Algorithm {
    fn name(self) -> &'static str {
        match self {
            Self::Bidirectional => "bidirectional_a*",
            Self::TimeDepForward => UnidirectionalAStar::<true>::NAME,
            Self::TimeDepReverse => UnidirectionalAStar::<false>::NAME,
        }
    }
}

/// The searches, and their storage, which is kept between requests.
///
/// Storage beyond its [`Retention`] is given back when a search is cleared,
/// at the start of each search and in [`Workspace::trim`].
#[derive(Debug)]
pub struct Workspace {
    bidirectional: BidirectionalAStar,
    forward: UnidirectionalAStar<true>,
    reverse: UnidirectionalAStar<false>,
    retention: Retention,
}

impl Default for Workspace {
    /// A workspace with mainline's retention.
    fn default() -> Self {
        Self::new(Retention::default())
    }
}

impl Workspace {
    /// A workspace that keeps at most `retention`'s storage between requests.
    #[must_use]
    pub fn new(retention: Retention) -> Self {
        Self {
            bidirectional: BidirectionalAStar::new(),
            forward: UnidirectionalAStar::default(),
            reverse: UnidirectionalAStar::default(),
            retention,
        }
    }

    /// Gives back every search's storage beyond the workspace's retention.
    ///
    /// Searches only trim themselves when they next start,
    /// so call this when a workspace goes idle to release a long search's memory right away.
    ///
    /// # Performance
    ///
    /// Clearing is cheap; shrinking frees memory that the next long search allocates again.
    pub fn trim(&mut self) {
        self.bidirectional
            .clear(self.retention.bidirectional_labels);
        self.forward.clear(self.retention.unidirectional_labels);
        self.reverse.clear(self.retention.unidirectional_labels);
    }

    /// The labels and edge statuses every search has room for (for tests).
    #[cfg(test)]
    pub(crate) fn retained(&self) -> usize {
        self.bidirectional.retained() + self.forward.retained() + self.reverse.retained()
    }

    fn clear(&mut self, algorithm: Algorithm) {
        match algorithm {
            Algorithm::Bidirectional => self
                .bidirectional
                .clear(self.retention.bidirectional_labels),
            Algorithm::TimeDepForward => self.forward.clear(self.retention.unidirectional_labels),
            Algorithm::TimeDepReverse => self.reverse.clear(self.retention.unidirectional_labels),
        }
    }

    fn has_ferry(&self, algorithm: Algorithm) -> bool {
        match algorithm {
            Algorithm::Bidirectional => self.bidirectional.has_ferry,
            Algorithm::TimeDepForward => self.forward.has_ferry,
            Algorithm::TimeDepReverse => self.reverse.has_ferry,
        }
    }
}

/// Clamps the request's hierarchy limits to the service's, or replaces them with the defaults
/// (mainline's `check_hierarchy_limits`). Returns whether the request's limits were changed.
fn check_hierarchy_limits(
    limits: &mut [HierarchyLimits],
    costing: &mut AnyCosting,
    disable_hierarchy_pruning: bool,
    config: &HierarchyLimitsConfig,
    allow_modifications: bool,
    use_hierarchy_limits: bool,
) -> bool {
    let mut add_warning = false;
    let mut default_limits = true;
    for (i, limit) in limits.iter_mut().enumerate() {
        if disable_hierarchy_pruning {
            limit.max_up_transitions = UNLIMITED_TRANSITIONS;
            continue;
        }
        if !allow_modifications || is_unset(limit) {
            add_warning = add_warning || !is_unset(limit);
            if use_hierarchy_limits && let Some(default) = config.default_limits.get(i) {
                limit.max_up_transitions = default.max_up_transitions;
                limit.expand_within_dist = default.expand_within_dist;
            }
            continue;
        }
        default_limits = false;
        if let Some(max) = config.max_limits.get(i) {
            if limit.max_up_transitions > max.max_up_transitions {
                limit.max_up_transitions = max.max_up_transitions;
                add_warning = true;
            }
            if limit.expand_within_dist < 0.0 || limit.expand_within_dist > max.expand_within_dist {
                limit.expand_within_dist = max.expand_within_dist;
                add_warning = true;
            }
        }
    }
    costing
        .base_mut()
        .set_default_hierarchy_limits(default_limits);
    add_warning
}

/// What a route needs from Thor.
pub(crate) struct ThorRoute<'a, P> {
    pub(crate) config: &'a ThorConfig,
    pub(crate) provider: &'a P,
    pub(crate) now: jiff::Timestamp,
    pub(crate) interrupt: &'a dyn Fn() -> bool,
}

/// How a route is being put together (the variables `path_depart_at` and `path_arrive_by`
/// share with their `route_two_locations` lambdas).
struct RouteState {
    paths: Paths,
    /// The route that legs go into, if any yet.
    route: Option<usize>,
    /// The edge the previous segment ended (depart at) or started (arrive by) on.
    through_edge: Option<GraphId>,
    edge_trimming: BTreeMap<usize, (EdgeTrimming, EdgeTrimming)>,
    path: Vec<PathEdge>,
    algorithms: Vec<&'static str>,
    hierarchy_limits_bidirectional: Vec<HierarchyLimits>,
    hierarchy_limits_unidirectional: Vec<HierarchyLimits>,
    used_bidirectional: bool,
    used_unidirectional: bool,
    add_hierarchy_limits_warning: bool,
}

impl RouteState {
    fn new(costing: &AnyCosting) -> Self {
        let limits = costing.base().hierarchy_limits().to_vec();
        Self {
            paths: Paths::default(),
            route: None,
            through_edge: None,
            edge_trimming: BTreeMap::new(),
            path: Vec::new(),
            algorithms: Vec::new(),
            hierarchy_limits_bidirectional: limits.clone(),
            hierarchy_limits_unidirectional: limits,
            used_bidirectional: false,
            used_unidirectional: false,
            add_hierarchy_limits_warning: false,
        }
    }

    /// Starts over (the retry with highly reachable candidates).
    fn reset(&mut self) {
        self.paths.routes.clear();
        self.route = None;
        self.through_edge = None;
        self.edge_trimming.clear();
        self.path.clear();
        self.algorithms.clear();
    }

    /// Adds a leg to the current route, or to a new one (each alternate is its own route).
    fn push_leg(&mut self, leg: Leg, alternates: bool) {
        let index = match self.route {
            Some(r) if !alternates && !self.paths.routes.is_empty() => r,
            _ => {
                self.paths.routes.push(Route::default());
                self.paths.routes.len() - 1
            }
        };
        self.route = Some(index);
        if let Some(route) = self.paths.routes.get_mut(index) {
            route.legs.push(leg);
        }
    }

    fn take_trimming(&mut self) -> Vec<EdgeTrim> {
        core::mem::take(&mut self.edge_trimming)
            .into_iter()
            .map(|(edge_index, (begin, end))| EdgeTrim {
                edge_index,
                begin,
                end,
            })
            .collect()
    }
}

/// Where a leg meets an intermediate location mid-edge, how to trim the two edges there
/// (`intermediate_loc_edge_trimming`). Returns whether the location is at a node.
///
/// Also sets the location's `leg_shape_index` to the index of its edge in the path,
/// which trip building turns into a shape index.
fn intermediate_loc_edge_trimming(
    location: &mut Location,
    incoming: GraphId,
    outgoing: GraphId,
    edge_trimming: &mut BTreeMap<usize, (EdgeTrimming, EdgeTrimming)>,
    path_index: usize,
    arrive_by: bool,
) -> bool {
    let find = |id: GraphId| {
        candidates(location)
            .iter()
            .find(|e| e.graph_id == id.value())
            .cloned()
    };
    let (Some(in_pe), Some(out_pe)) = (find(incoming), find(outgoing)) else {
        // Mainline logs "Could not find connecting edges" and leaves the index unset.
        return false;
    };
    let correlation = location.correlation.get_or_insert_with(Default::default);
    let shift = usize::from(arrive_by);
    correlation.leg_shape_index = u32::try_from(path_index + shift).unwrap_or(u32::MAX);
    if in_pe.begin_node || in_pe.end_node || out_pe.begin_node || out_pe.end_node {
        // At a node no edge is cut (or doubled), so arrive by doesn't shift the index.
        correlation.leg_shape_index = u32::try_from(path_index).unwrap_or(u32::MAX);
        return true;
    }

    let ll = point_of(in_pe.ll.as_ref());
    let point = (ll.lng, ll.lat);
    let none = |percent_along| EdgeTrimming {
        trim: false,
        point: (0.0, 0.0),
        percent_along,
    };
    // Cut the end of the edge coming in at the location...
    let end_cut = EdgeTrimming {
        trim: true,
        point,
        percent_along: in_pe.percent_along,
    };
    edge_trimming
        .entry(path_index + shift)
        .and_modify(|(_, end)| *end = end_cut)
        .or_insert((none(0.0), end_cut));
    // ...and the start of the edge going out, by its own percent along
    // (for a U-turn it's the opposing edge, so the percentage flips).
    let begin_cut = EdgeTrimming {
        trim: true,
        point,
        percent_along: out_pe.percent_along,
    };
    edge_trimming
        .entry(path_index + 1 - shift)
        .and_modify(|(begin, _)| *begin = begin_cut)
        .or_insert((begin_cut, none(1.0)));
    false
}

/// The percent along of a path's edge among a location's candidates, if it's there.
fn percent_on(location: &Location, edge_id: Option<GraphId>) -> Option<f64> {
    let id = edge_id?;
    candidates(location)
        .iter()
        .find(|e| e.graph_id == id.value())
        .map(|e| e.percent_along)
}

impl<P: GraphTileProvider> ThorRoute<'_, P> {
    /// Picks the path algorithm for two locations (`get_path_algorithm`).
    fn path_algorithm(
        &self,
        api: &mut Api,
        origin: &Location,
        destination: &Location,
        options: &Options,
    ) -> Result<Algorithm, ThorError> {
        let dtt = DateTimeType::try_from(options.date_time_type).ok();
        let invariant = dtt == Some(DateTimeType::Invariant);
        let distance = || point_of(origin.ll.as_ref()).distance(point_of(destination.ll.as_ref()));
        let max = f64::from(self.config.max_timedep_distance);
        if !origin.date_time.is_empty() && !invariant && !options.prioritize_bidirectional {
            if distance() < max {
                return Ok(Algorithm::TimeDepForward);
            }
            add_warning(api, 402);
        }
        if !destination.date_time.is_empty() && !invariant {
            if distance() < max {
                return Ok(Algorithm::TimeDepReverse);
            }
            add_warning(api, 214);
        }
        // Bidirectional A* doesn't handle trivial routes on one-way edges, or very costly
        // origin or destination edges, so connected candidates go forward.
        for edge1 in candidates(origin) {
            for edge2 in candidates(destination) {
                let (id1, id2) = (candidate_id(edge1)?, candidate_id(edge2)?);
                if id1 == id2 || graph::are_edges_connected(self.provider, id1, id2)? {
                    return Ok(Algorithm::TimeDepForward);
                }
            }
        }
        Ok(Algorithm::Bidirectional)
    }

    /// Runs one algorithm.
    fn run(
        &self,
        workspace: &mut Workspace,
        algorithm: Algorithm,
        costing: &AnyCosting,
        origin: &mut Location,
        destination: &mut Location,
        options: &Options,
    ) -> Result<Vec<Vec<PathEdge>>, ThorError> {
        let ctx = SearchContext {
            provider: self.provider,
            costing,
            now: self.now,
            interrupt: self.interrupt,
        };
        match algorithm {
            Algorithm::Bidirectional => {
                let params = BidirectionalParams {
                    threshold_delta: self.config.threshold_delta,
                    alternative_cost_extend: self.config.alternative_cost_extend,
                    alternative_iterations_delta: self.config.alternative_iterations_delta,
                    extended_search: self.config.extended_search,
                };
                workspace
                    .bidirectional
                    .best_path(&ctx, &params, origin, destination, options)
            }
            Algorithm::TimeDepForward => workspace.forward.best_path(&ctx, origin, destination),
            Algorithm::TimeDepReverse => workspace.reverse.best_path(&ctx, origin, destination),
        }
    }

    /// Finds the path between two locations, with a second pass with relaxed limits
    /// if the first fails (`get_path`).
    #[expect(clippy::too_many_arguments, reason = "mainline's state")]
    fn get_path(
        &self,
        api: &mut Api,
        workspace: &mut Workspace,
        algorithm: Algorithm,
        costing: &mut AnyCosting,
        costing_name: &str,
        origin: &mut Location,
        destination: &mut Location,
        options: &Options,
    ) -> Result<Vec<Vec<PathEdge>>, ThorError> {
        // Bidirectional A* leaves destination-only edges for the second pass.
        costing
            .base_mut()
            .set_allow_destination_only(algorithm != Algorithm::Bidirectional);
        costing.base_mut().set_pass(0);
        let paths = self.run(workspace, algorithm, costing, origin, destination, options)?;

        // A pedestrian route with a ferry is searched again, as the A* heuristic can be wrong
        // across ferries (but not on long routes, for performance).
        let ped_second_pass = !paths.is_empty()
            && costing_name == "pedestrian"
            && workspace.has_ferry(algorithm)
            && point_of(origin.ll.as_ref()).distance(point_of(destination.ll.as_ref()))
                < PEDESTRIAN_MULTIPASS_THRESHOLD;

        if (paths.is_empty() || ped_second_pass) && costing.allow_multi_pass() {
            add_warning(api, 401);
            // Retry with the candidates filtered by heading too.
            for location in [&mut *origin, &mut *destination] {
                if let Some(c) = location.correlation.as_mut() {
                    let filtered = c.filtered_edges.clone();
                    c.edges.extend(filtered);
                }
            }
            workspace.clear(algorithm);
            costing.base_mut().set_pass(1);
            costing
                .base_mut()
                .relax_hierarchy_limits(algorithm == Algorithm::Bidirectional);
            costing.base_mut().set_allow_destination_only(true);
            if algorithm == Algorithm::Bidirectional {
                workspace.bidirectional.not_thru_pruning = false;
            }
            let relaxed = self.run(workspace, algorithm, costing, origin, destination, options)?;
            if !relaxed.is_empty() {
                return Ok(relaxed);
            }
        }
        Ok(paths)
    }

    /// Picks the algorithm and sets the hierarchy limits for a segment
    /// (the start of `route_two_locations`).
    #[expect(clippy::too_many_arguments, reason = "mainline's state")]
    fn prepare(
        &self,
        api: &mut Api,
        workspace: &mut Workspace,
        state: &mut RouteState,
        costing: &mut AnyCosting,
        disable_hierarchy_pruning: bool,
        origin: &Location,
        destination: &Location,
        options: &Options,
    ) -> Result<Algorithm, ThorError> {
        let algorithm = self.path_algorithm(api, origin, destination, options)?;
        workspace.clear(algorithm);
        let is_bidir = algorithm == Algorithm::Bidirectional;
        let use_hierarchy_limits = costing.base().use_hierarchy_limits();
        let (limits, used, config) = if is_bidir {
            (
                &mut state.hierarchy_limits_bidirectional,
                state.used_bidirectional,
                &self.config.hierarchy_limits_bidirectional,
            )
        } else {
            (
                &mut state.hierarchy_limits_unidirectional,
                state.used_unidirectional,
                &self.config.hierarchy_limits_unidirectional,
            )
        };
        let warn = !used
            && check_hierarchy_limits(
                limits,
                costing,
                disable_hierarchy_pruning,
                config,
                self.config.allow_hierarchy_limits_modifications,
                use_hierarchy_limits,
            );
        state.add_hierarchy_limits_warning = warn || state.add_hierarchy_limits_warning;
        let limits = limits.clone();
        if is_bidir {
            state.used_bidirectional = true;
        } else {
            state.used_unidirectional = true;
        }
        costing.base_mut().set_hierarchy_limits(limits);
        Ok(algorithm)
    }
}

/// The time zone index of a path edge (mainline's `GetTimezoneFromEdge`).
fn tz_of<P: GraphTileProvider>(provider: &P, edge: Option<&PathEdge>) -> Result<u32, ThorError> {
    match edge {
        Some(e) => graph::timezone_from_edge(provider, e.edge_id),
        None => Ok(0),
    }
}

/// Whether the options ask for alternates.
fn wants_alternates(options: &Options) -> bool {
    matches!(options.has_alternates, Some(HasAlternates::Alternates(n)) if n > 0)
}

/// Whether the request's costing options turn hierarchy pruning off (set by Loki).
fn disable_hierarchy_pruning(options: &Options) -> bool {
    options
        .costings
        .get(&options.costing_type)
        .and_then(|c| c.has_options.as_ref())
        .is_some_and(|HasOptions::Options(o)| o.disable_hierarchy_pruning)
}

impl<P: GraphTileProvider> ThorRoute<'_, P> {
    /// Routes from each location to the next, with times moving forward (`path_depart_at`).
    pub(crate) fn depart_at(
        &self,
        api: &mut Api,
        workspace: &mut Workspace,
        costing: &mut AnyCosting,
        costing_name: &str,
    ) -> Result<Paths, ValhallaError> {
        let options = api.options.clone().unwrap_or_default();
        let alternates = wants_alternates(&options);
        let invariant = DateTimeType::try_from(options.date_time_type)
            .is_ok_and(|t| t == DateTimeType::Invariant);
        let disable_pruning = disable_hierarchy_pruning(&options);
        let mut state = RouteState::new(costing);
        let mut correlated = options.locations.clone();
        let mut allow_retry = true;
        let count = correlated.len();

        let mut destination = 1;
        while destination < count {
            let origin = destination - 1;
            let routed = self.depart_at_segment(
                api,
                workspace,
                costing,
                costing_name,
                &options,
                &mut state,
                &mut correlated,
                origin,
                destination,
                alternates,
                invariant,
                disable_pruning,
            )?;
            if !routed {
                // A through location may have snapped to a small island of the graph:
                // retry once with only highly reachable candidates at intermediate locations.
                if allow_retry
                    && origin != 0
                    && let Some(loc) = correlated.get(origin)
                    && is_through_point(loc)
                    && let Some(first) = candidates(loc).first()
                    && !is_highly_reachable(loc, first)
                {
                    allow_retry = false;
                    correlated.clone_from(&options.locations);
                    let last = correlated.len().saturating_sub(1);
                    for loc in correlated.iter_mut().take(last).skip(1) {
                        let probe = loc.clone();
                        remove_path_edges(loc, |e| !is_highly_reachable(&probe, e));
                        if candidates(loc).is_empty() {
                            return Err(ErrorCode::NoPathFound.into());
                        }
                    }
                    state.reset();
                    destination = 1;
                    continue;
                }
                return Err(ErrorCode::NoPathFound.into());
            }
            destination += 1;
        }
        if state.add_hierarchy_limits_warning {
            add_warning(
                api,
                if self.config.allow_hierarchy_limits_modifications {
                    210
                } else {
                    209
                },
            );
        }
        if let Some(options) = api.options.as_mut() {
            options.locations = correlated;
        }
        Ok(state.paths)
    }

    /// One segment of `path_depart_at` (its `route_two_locations`).
    /// Returns false if no path was found.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "mainline's lambda and its captures"
    )]
    fn depart_at_segment(
        &self,
        api: &mut Api,
        workspace: &mut Workspace,
        costing: &mut AnyCosting,
        costing_name: &str,
        options: &Options,
        state: &mut RouteState,
        correlated: &mut [Location],
        origin: usize,
        destination: usize,
        alternates: bool,
        invariant: bool,
        disable_pruning: bool,
    ) -> Result<bool, ValhallaError> {
        let (Some(o), Some(d)) = (correlated.get(origin), correlated.get(destination)) else {
            return Ok(false);
        };
        let algorithm = self.prepare(
            api,
            workspace,
            state,
            costing,
            disable_pruning,
            o,
            d,
            options,
        )?;
        state.algorithms.push(algorithm.name());

        // Continuing through a location: only along the edge the route arrived on (no U-turn).
        if let Some(last_edge) = state.through_edge
            && let Some(o) = correlated.get_mut(origin)
            && is_through_point(o)
        {
            remove_path_edges(o, |e| e.graph_id != last_edge.value());
        }

        let (head, tail) = correlated.split_at_mut(destination);
        let (Some(origin_loc), Some(destination_loc)) = (head.get_mut(origin), tail.first_mut())
        else {
            return Ok(false);
        };
        let temp_paths = self.get_path(
            api,
            workspace,
            algorithm,
            costing,
            costing_name,
            origin_loc,
            destination_loc,
            options,
        )?;
        if temp_paths.is_empty() {
            return Ok(false);
        }

        for mut temp_path in temp_paths {
            let in_tz = tz_of(self.provider, temp_path.first())?;
            let out_tz = tz_of(self.provider, temp_path.last())?;
            let original_index = correlated
                .get(origin)
                .and_then(|l| l.correlation.as_ref())
                .map_or(0, |c| c.original_index);
            if let Some(o) = correlated.get_mut(origin)
                && original_index == 0
                && (in_tz != 0 || out_tz != 0)
            {
                offset_date(&o.date_time, in_tz, in_tz, 0.0).apply(o);
            }
            // Move the time forward to the destination.
            if let Some(origin_dt) = correlated
                .get(origin)
                .map(|l| l.date_time.clone())
                .filter(|dt| !dt.is_empty())
                && (in_tz != 0 || out_tz != 0)
                && let Some(d) = correlated.get_mut(destination)
            {
                let offset = if invariant {
                    0.0
                } else {
                    temp_path.last().map_or(0.0, |e| e.elapsed_cost.secs) + d.waiting_secs
                };
                offset_date(&origin_dt, in_tz, out_tz, offset).apply(d);
            }

            state.through_edge = temp_path.last().map(|e| e.edge_id);

            // Join the segments of a leg at through locations.
            if let Some(last) = state.path.last().copied() {
                for e in &mut temp_path {
                    e.elapsed_cost += last.elapsed_cost;
                    e.path_distance += last.path_distance;
                }
                let first = temp_path.first().map(|e| e.edge_id);
                let at_node = match (correlated.get_mut(origin), first) {
                    (Some(o), Some(first)) => intermediate_loc_edge_trimming(
                        o,
                        last.edge_id,
                        first,
                        &mut state.edge_trimming,
                        state.path.len() - 1,
                        false,
                    ),
                    _ => false,
                };
                if Some(last.edge_id) == first && at_node {
                    state.path.pop();
                }
                state.path.append(&mut temp_path);
            } else {
                state.path = temp_path;
            }

            // A leg ends at each break.
            if correlated.get(destination).is_some_and(is_break_point) {
                let mut leg_origin = origin;
                while leg_origin > 0 && !correlated.get(leg_origin).is_some_and(is_break_point) {
                    leg_origin -= 1;
                }
                let leg = Self::make_leg(state, correlated, leg_origin, destination);
                state.push_leg(leg, alternates);
                state.path.clear();
                state.edge_trimming.clear();
            }
        }
        if state.path.is_empty() {
            state.algorithms.clear();
        }
        Ok(true)
    }

    /// The leg from `origin` to `destination`, from the route's path so far.
    fn make_leg(
        state: &mut RouteState,
        correlated: &[Location],
        origin: usize,
        destination: usize,
    ) -> Leg {
        let intermediates: Vec<usize> = (origin + 1..destination).collect();
        let locations: Vec<Location> = correlated
            .get(origin..=destination)
            .map(<[Location]>::to_vec)
            .unwrap_or_default();
        let edges = state.path.clone();
        let begin_percent = correlated
            .get(origin)
            .and_then(|l| percent_on(l, edges.first().map(|e| e.edge_id)));
        let end_percent = correlated
            .get(destination)
            .and_then(|l| percent_on(l, edges.last().map(|e| e.edge_id)));
        Leg {
            origin,
            destination,
            intermediates,
            locations,
            begin_percent,
            end_percent,
            edges,
            algorithms: state.algorithms.clone(),
            edge_trimming: state.take_trimming(),
        }
    }
}

impl<P: GraphTileProvider> ThorRoute<'_, P> {
    /// Routes from each location to the previous one, with times moving back from the arrival
    /// (`path_arrive_by`).
    pub(crate) fn arrive_by(
        &self,
        api: &mut Api,
        workspace: &mut Workspace,
        costing: &mut AnyCosting,
        costing_name: &str,
    ) -> Result<Paths, ValhallaError> {
        let options = api.options.clone().unwrap_or_default();
        let alternates = wants_alternates(&options);
        let disable_pruning = disable_hierarchy_pruning(&options);
        let mut state = RouteState::new(costing);
        let mut correlated = options.locations.clone();
        let mut allow_retry = true;
        let count = correlated.len();
        if count < 2 {
            return Err(ErrorCode::NoPathFound.into());
        }

        let mut origin = count - 2;
        loop {
            let destination = origin + 1;
            let routed = self.arrive_by_segment(
                api,
                workspace,
                costing,
                costing_name,
                &options,
                &mut state,
                &mut correlated,
                origin,
                destination,
                alternates,
                disable_pruning,
            )?;
            if !routed {
                if allow_retry
                    && destination != count - 1
                    && let Some(loc) = correlated.get(destination)
                    && is_through_point(loc)
                    && let Some(first) = candidates(loc).first()
                    && !is_highly_reachable(loc, first)
                {
                    allow_retry = false;
                    correlated.clone_from(&options.locations);
                    let last = correlated.len().saturating_sub(1);
                    for loc in correlated.iter_mut().take(last).skip(1) {
                        let probe = loc.clone();
                        remove_path_edges(loc, |e| !is_highly_reachable(&probe, e));
                        if candidates(loc).is_empty() {
                            return Err(ErrorCode::NoPathFound.into());
                        }
                    }
                    state.reset();
                    origin = count - 2;
                    continue;
                }
                return Err(ErrorCode::NoPathFound.into());
            }
            if origin == 0 {
                break;
            }
            origin -= 1;
        }
        if state.add_hierarchy_limits_warning {
            add_warning(
                api,
                if self.config.allow_hierarchy_limits_modifications {
                    210
                } else {
                    209
                },
            );
        }
        // The legs were made from the last to the first.
        if let Some(route) = state.route.and_then(|r| state.paths.routes.get_mut(r)) {
            route.legs.reverse();
        }
        if let Some(options) = api.options.as_mut() {
            options.locations = correlated;
        }
        Ok(state.paths)
    }

    /// One segment of `path_arrive_by` (its `route_two_locations`).
    /// Returns false if no path was found.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "mainline's lambda and its captures"
    )]
    fn arrive_by_segment(
        &self,
        api: &mut Api,
        workspace: &mut Workspace,
        costing: &mut AnyCosting,
        costing_name: &str,
        options: &Options,
        state: &mut RouteState,
        correlated: &mut [Location],
        origin: usize,
        destination: usize,
        alternates: bool,
        disable_pruning: bool,
    ) -> Result<bool, ValhallaError> {
        let (Some(o), Some(d)) = (correlated.get(origin), correlated.get(destination)) else {
            return Ok(false);
        };
        let algorithm = self.prepare(
            api,
            workspace,
            state,
            costing,
            disable_pruning,
            o,
            d,
            options,
        )?;
        state.algorithms.push(algorithm.name());

        // Continuing through a location: only along the edge the later segment starts on.
        if let Some(first_edge) = state.through_edge
            && let Some(d) = correlated.get_mut(destination)
            && is_through_point(d)
        {
            remove_path_edges(d, |e| e.graph_id != first_edge.value());
        }

        let (head, tail) = correlated.split_at_mut(destination);
        let (Some(origin_loc), Some(destination_loc)) = (head.get_mut(origin), tail.first_mut())
        else {
            return Ok(false);
        };
        let temp_paths = self.get_path(
            api,
            workspace,
            algorithm,
            costing,
            costing_name,
            origin_loc,
            destination_loc,
            options,
        )?;
        if temp_paths.is_empty() {
            return Ok(false);
        }
        let last_index = u32::try_from(options.locations.len().saturating_sub(1)).unwrap_or(0);

        for mut temp_path in temp_paths {
            let out_tz = tz_of(self.provider, temp_path.last())?;
            let in_tz = tz_of(self.provider, temp_path.first())?;
            let original_index = correlated
                .get(destination)
                .and_then(|l| l.correlation.as_ref())
                .map_or(0, |c| c.original_index);
            // At the final destination, mainline adds its waiting time again,
            // so that intermediate locations get departure times.
            if original_index == last_index
                && (in_tz != 0 || out_tz != 0)
                && let Some(d) = correlated.get_mut(destination)
            {
                offset_date(&d.date_time, out_tz, out_tz, d.waiting_secs).apply(d);
            }
            // Move the time back to the origin.
            if let Some(dest_dt) = correlated
                .get(destination)
                .map(|l| l.date_time.clone())
                .filter(|dt| !dt.is_empty())
                && let Some(o) = correlated.get_mut(origin)
            {
                let elapsed = temp_path.last().map_or(0.0, |e| e.elapsed_cost.secs);
                offset_date(&dest_dt, out_tz, in_tz, -elapsed).apply(o);
            }

            state.through_edge = temp_path.first().map(|e| e.edge_id);
            // Put this (earlier) segment first and join the later ones after it.
            core::mem::swap(&mut temp_path, &mut state.path);
            if !temp_path.is_empty()
                && let Some(last) = state.path.last().copied()
            {
                for e in &mut temp_path {
                    e.elapsed_cost += last.elapsed_cost;
                    e.path_distance += last.path_distance;
                }
                let first = temp_path.first().map(|e| e.edge_id);
                let at_node = match (correlated.get_mut(destination), first) {
                    (Some(d), Some(first)) => intermediate_loc_edge_trimming(
                        d,
                        last.edge_id,
                        first,
                        &mut state.edge_trimming,
                        temp_path.len(),
                        true,
                    ),
                    _ => false,
                };
                if Some(last.edge_id) == first && at_node {
                    state.path.pop();
                }
                state.path.append(&mut temp_path);
            }

            // A leg starts at each break.
            if correlated.get(origin).is_some_and(is_break_point) {
                let path_len = state.path.len();
                let mut leg_destination = destination;
                while let Some(d) = correlated.get_mut(leg_destination)
                    && !is_break_point(d)
                {
                    // The intermediate indexes were counted from the end; flip them.
                    let c = d.correlation.get_or_insert_with(Default::default);
                    let index = path_len.wrapping_sub(c.leg_shape_index as usize);
                    c.leg_shape_index = u32::try_from(index).unwrap_or(u32::MAX);
                    leg_destination += 1;
                }
                state.edge_trimming = core::mem::take(&mut state.edge_trimming)
                    .into_iter()
                    .map(|(k, v)| (path_len.wrapping_sub(k), v))
                    .collect();
                let leg = Self::make_leg(state, correlated, origin, leg_destination);
                state.push_leg(leg, alternates);

                // The next segment (an earlier one) arrives before this origin's waiting time.
                if let Some(o) = correlated.get_mut(origin)
                    && o.waiting_secs != 0.0
                    && !o.date_time.is_empty()
                {
                    offset_date(&o.date_time, in_tz, in_tz, -o.waiting_secs).apply(o);
                }
                state.path.clear();
                state.edge_trimming.clear();
            }
        }
        if state.path.is_empty() {
            state.algorithms.clear();
        }
        Ok(true)
    }
}

/// Mainline's Thor for routes: a configuration and a tile set.
///
/// It holds nothing per request, so one can serve many threads at once
/// (given a thread-safe provider); each thread brings its own [`Workspace`].
pub struct Thor<P> {
    config: ThorConfig,
    provider: P,
}

impl<P: GraphTileProvider> Thor<P> {
    /// Thor over a tile set.
    pub fn new(config: ThorConfig, provider: P) -> Self {
        Self { config, provider }
    }

    /// The configuration.
    pub fn config(&self) -> &ThorConfig {
        &self.config
    }

    /// The tile set.
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// Finds the paths of a `route` request that Loki has correlated
    /// (mainline's `thor_worker_t::route`, up to trip building).
    ///
    /// The request is updated as mainline updates it: the candidates' distances become
    /// scores, through locations keep only the edge the route uses, filtered candidates
    /// are added on second passes, intermediate locations get their `leg_shape_index`,
    /// locations get the times the route reaches them (`date_time`, `time_zone_offset`,
    /// and `time_zone_name`), and warnings are added.
    /// `trip` is left for trip building, which turns the [`Paths`] into `TripLeg`s.
    ///
    /// `current` times are the time of the call.
    ///
    /// # Errors
    ///
    /// Error 442 if no path can be found, and error 499 for failures mainline reports as
    /// exceptions (tiles that can't be read, inconsistent correlations).
    pub fn route(&self, workspace: &mut Workspace, api: &mut Api) -> Result<Paths, ValhallaError> {
        self.route_at(workspace, api, jiff::Timestamp::now())
    }

    /// [`Thor::route`], with `now` as the current time.
    ///
    /// # Errors
    ///
    /// As for [`Thor::route`].
    pub fn route_at(
        &self,
        workspace: &mut Workspace,
        api: &mut Api,
        now: jiff::Timestamp,
    ) -> Result<Paths, ValhallaError> {
        self.route_interruptible(workspace, api, now, &|| false)
    }

    /// [`Thor::route_at`], giving up when `interrupt` returns true
    /// (it is called every 5000 search iterations, as mainline checks its interrupt).
    ///
    /// # Errors
    ///
    /// As for [`Thor::route`]; an interrupted search is error 499, which callers should
    /// drop rather than send (mainline's workers send nothing for interrupted requests).
    pub fn route_interruptible(
        &self,
        workspace: &mut Workspace,
        api: &mut Api,
        now: jiff::Timestamp,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<Paths, ValhallaError> {
        let options = api
            .options
            .as_ref()
            .ok_or(ThorError::Options("the request has no options".to_owned()))?;
        let costing_type = valhalla_proto::costing::Type::try_from(options.costing_type)
            .map_err(|_| ThorError::Options(format!("unknown costing {}", options.costing_type)))?;
        let costing_name = costing_name(costing_type);
        let proto = options
            .costings
            .get(&options.costing_type)
            .cloned()
            .unwrap_or(valhalla_proto::Costing {
                r#type: options.costing_type,
                ..valhalla_proto::Costing::default()
            });
        let arrive_by = DateTimeType::try_from(options.date_time_type)
            .is_ok_and(|t| t == DateTimeType::ArriveBy);
        let mut costing = AnyCosting::new(&proto).map_err(|e| ThorError::Options(e.to_string()))?;
        adjust_locations(api, costing_name);
        let route = ThorRoute {
            config: &self.config,
            provider: &self.provider,
            now,
            interrupt,
        };
        if arrive_by {
            route.arrive_by(api, workspace, &mut costing, costing_name)
        } else {
            route.depart_at(api, workspace, &mut costing, costing_name)
        }
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "mainline's values are exact")]
mod tests {
    use super::*;
    use valhalla_proto::lat_lng::{HasLat, HasLng};
    use valhalla_proto::{Correlation, LatLng};

    fn pe(id: u64, distance: f64) -> CandidateEdge {
        CandidateEdge {
            graph_id: id,
            distance,
            ..CandidateEdge::default()
        }
    }

    fn location(edges: Vec<CandidateEdge>, filtered: Vec<CandidateEdge>) -> Location {
        Location {
            correlation: Some(Correlation {
                edges,
                filtered_edges: filtered,
                ..Correlation::default()
            }),
            ..Location::default()
        }
    }

    fn distances(location: &Location) -> (Vec<f64>, Vec<f64>) {
        let c = location.correlation.as_ref().unwrap();
        (
            c.edges.iter().map(|e| e.distance).collect(),
            c.filtered_edges.iter().map(|e| e.distance).collect(),
        )
    }

    #[test]
    fn adjusts_locations_as_mainline() {
        let mut api = Api {
            options: Some(Options {
                locations: vec![
                    location(vec![pe(1, 2.0), pe(2, 5.0)], vec![pe(3, 1.0)]),
                    // Only filtered candidates: they become the candidates (warning 215).
                    location(vec![], vec![pe(4, 3.0), pe(5, 100.0)]),
                ],
                ..Options::default()
            }),
            ..Api::default()
        };
        adjust_locations(&mut api, "auto");
        let locations = &api.options.as_ref().unwrap().locations;
        // Filtered: 1 + 3600 + 5 = 3606 m. Scores are 10 d², less the smallest (40),
        // capped at auto's 43,200.
        assert_eq!(distances(&locations[0]), (vec![0.0, 210.0], vec![43_200.0]));
        assert_eq!(distances(&locations[1]), (vec![0.0, 43_200.0], vec![]));
        let warnings = &api.info.as_ref().unwrap().warnings;
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            (warnings[0].code, warnings[0].description.as_str()),
            (215, "")
        );

        // Without ranking, every score is 0.
        let mut api = Api {
            options: Some(Options {
                locations: vec![Location {
                    skip_ranking_candidates: true,
                    ..location(vec![pe(1, 2.0)], vec![])
                }],
                ..Options::default()
            }),
            ..Api::default()
        };
        adjust_locations(&mut api, "pedestrian");
        assert_eq!(
            distances(&api.options.as_ref().unwrap().locations[0]).0,
            [0.0]
        );
    }

    fn limits(max_up_transitions: u32, expand_within_dist: f32) -> HierarchyLimits {
        HierarchyLimits {
            up_transition_count: 0,
            max_up_transitions,
            expand_within_dist,
        }
    }

    #[test]
    fn checks_hierarchy_limits_as_mainline() {
        let config = ThorConfig::default();
        let config = &config.hierarchy_limits_bidirectional;
        let mut costing = AnyCosting::new(&valhalla_proto::Costing {
            r#type: valhalla_proto::costing::Type::Auto.into(),
            ..valhalla_proto::Costing::default()
        })
        .unwrap();
        let unset = || vec![limits(UNLIMITED_TRANSITIONS, f32::MAX); 3];
        let mut check = |l: &mut Vec<HierarchyLimits>, disable, allow, use_limits| {
            check_hierarchy_limits(l, &mut costing, disable, config, allow, use_limits)
        };

        // No limits from the request: the defaults, and no warning.
        let mut l = unset();
        assert!(!check(&mut l, false, false, true));
        assert_eq!(l[1], limits(400, 20_000.0));

        // Limits from the request where they aren't allowed: the defaults, with a warning.
        let mut l = unset();
        l[1] = limits(5, 10.0);
        assert!(check(&mut l, false, false, true));
        assert_eq!(l[1], limits(400, 20_000.0));

        // Where they are allowed: clamped to the largest, with a warning.
        let mut l = unset();
        l[1] = limits(5000, -1.0);
        assert!(check(&mut l, false, true, true));
        assert_eq!(l[1], limits(400, 20_000.0));
        let mut l = unset();
        l[2] = limits(7, 100.0);
        assert!(!check(&mut l, false, true, true));
        assert_eq!(l[2], limits(7, 100.0));

        // Costings without hierarchy limits keep none; disabled pruning lifts them.
        let mut l = unset();
        assert!(!check(&mut l, false, false, false));
        assert_eq!(l, unset());
        let mut l = vec![limits(1, 1.0); 3];
        assert!(!check(&mut l, true, false, true));
        assert!(
            l.iter()
                .all(|l| l.max_up_transitions == UNLIMITED_TRANSITIONS)
        );

        // Whether the defaults were used is recorded on the costing.
        let mut l = unset();
        check_hierarchy_limits(&mut l, &mut costing, false, config, false, true);
        assert!(costing.base().default_hierarchy_limits());
        let mut l = unset();
        l[1] = limits(5, 10.0);
        check_hierarchy_limits(&mut l, &mut costing, false, config, true, true);
        assert!(!costing.base().default_hierarchy_limits());
    }

    fn at(id: u64, percent_along: f64, begin_node: bool, end_node: bool) -> CandidateEdge {
        CandidateEdge {
            graph_id: id,
            percent_along,
            begin_node,
            end_node,
            ll: Some(LatLng {
                has_lat: Some(HasLat::Lat(42.5)),
                has_lng: Some(HasLng::Lng(1.5)),
            }),
            ..CandidateEdge::default()
        }
    }

    #[test]
    fn trims_edges_at_intermediate_locations() {
        let id = |i| GraphId::try_from_id(i).unwrap();
        // Mid-edge, continuing on the same edge: the edge is cut in two.
        let mut loc = location(
            vec![at(10, 0.25, false, false), at(11, 0.75, false, false)],
            vec![],
        );
        let mut trimming = BTreeMap::new();
        assert!(!intermediate_loc_edge_trimming(
            &mut loc,
            id(10),
            id(10),
            &mut trimming,
            4,
            false
        ));
        assert_eq!(loc.correlation.as_ref().unwrap().leg_shape_index, 4);
        let (begin4, end4) = trimming[&4];
        assert!(!begin4.trim && end4.trim && end4.percent_along == 0.25);
        assert_eq!(end4.point, (1.5, 42.5));
        let (begin5, end5) = trimming[&5];
        assert!(begin5.trim && begin5.percent_along == 0.25 && !end5.trim);
        assert_eq!(end5.percent_along, 1.0);

        // A U-turn onto the opposing edge cuts it at its own percent along;
        // arrive by counts from the end, so the cuts swap sides.
        let mut trimming = BTreeMap::new();
        assert!(!intermediate_loc_edge_trimming(
            &mut loc,
            id(10),
            id(11),
            &mut trimming,
            2,
            true
        ));
        assert_eq!(loc.correlation.as_ref().unwrap().leg_shape_index, 3);
        assert_eq!(trimming[&3].1.percent_along, 0.25);
        assert_eq!(trimming[&2].0.percent_along, 0.75);

        // At a node nothing is cut, and arrive by doesn't shift the index.
        let mut loc = location(
            vec![at(10, 1.0, false, true), at(12, 0.0, true, false)],
            vec![],
        );
        let mut trimming = BTreeMap::new();
        assert!(intermediate_loc_edge_trimming(
            &mut loc,
            id(10),
            id(12),
            &mut trimming,
            6,
            true
        ));
        assert_eq!(loc.correlation.as_ref().unwrap().leg_shape_index, 6);
        assert!(trimming.is_empty());

        // An edge that isn't a candidate: nothing is set.
        let mut loc = location(vec![at(10, 0.5, false, false)], vec![]);
        assert!(!intermediate_loc_edge_trimming(
            &mut loc,
            id(10),
            id(99),
            &mut trimming,
            1,
            false
        ));
        assert_eq!(loc.correlation.as_ref().unwrap().leg_shape_index, 0);
    }

    #[test]
    fn reachability_and_candidate_removal() {
        let reach = |id, inbound_reach, outbound_reach| CandidateEdge {
            inbound_reach,
            outbound_reach,
            ..pe(id, 0.0)
        };
        let mut loc = location(
            vec![reach(1, 10, 50), reach(2, 10, 49), reach(3, -1, 60)],
            vec![pe(4, 0.0)],
        );
        loc.has_minimum_inbound_reachability = Some(
            HasMinimumInboundReachability::MinimumInboundReachability(10),
        );
        loc.has_minimum_outbound_reachability = Some(
            HasMinimumOutboundReachability::MinimumOutboundReachability(50),
        );
        let c = loc.correlation.clone().unwrap();
        assert!(is_highly_reachable(&loc, &c.edges[0]));
        assert!(!is_highly_reachable(&loc, &c.edges[1]));
        // A negative reach reads as a huge unsigned one, as in mainline.
        assert!(is_highly_reachable(&loc, &c.edges[2]));
        assert!(!is_highly_reachable(&loc, &c.filtered_edges[0]));
        remove_path_edges(&mut loc, |e| e.graph_id != 2);
        let c = loc.correlation.unwrap();
        assert_eq!(c.edges.len(), 1);
        assert!(c.filtered_edges.is_empty());
    }
}
