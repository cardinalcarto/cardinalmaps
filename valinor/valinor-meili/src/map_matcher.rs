//! The map matcher (mainline's `MapMatcherFactory`, `MapMatcher`, `EmissionCostModel`, and
//! `TransitionCostModel`).
//!
//! [`MapMatcherFactory`] holds the `meili` config; [`MapMatcherFactory::create`] merges a
//! request's `trace_options` into it and builds the request's costing.
//! [`MapMatcher::offline_match`] is mainline's `OfflineMatch`: it finds candidates for each
//! measurement far enough from the last (closer ones are interpolated afterwards), runs the
//! Viterbi search for the best path through them, turns each state into a match result,
//! and builds the path's edge segments; for k > 1 it removes each path found and searches again.
//! A [`Workspace`] holds what a match needs, and the candidate grids, between requests.

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares floats exactly"
)]

use std::collections::{HashMap, HashSet};
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_reader::GraphReadError;
use valhalla_graphtile::graph_reader::{GraphReader, edge, node, shape};
use valhalla_graphtile::graph_tile::GraphTile;
use valhalla_graphtile::midgard::{DistanceApproximator, PointLL, Projector, narrow};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::costing::Type as CostingType;
use valhalla_proto::{Costing as CostingProto, Options};
use valinor_sif::{AnyCosting, Costing, TravelMode};

use crate::candidate_search::{Candidate, CandidateGrids};
use crate::config::{ConfigError, MeiliConfig};
use crate::error::MeiliError;
use crate::geometry::project;
use crate::match_result::{EdgeSegment, MatchResult, MatchResults, StateId};
use crate::match_route::{EdgeSequence, construct_route, merge_route};
use crate::measurement::Measurement;
use crate::routing::{Router, Search, edge_nodes};
use crate::state::{Container, Route};
use crate::topk::{Resolved, TopK};
use crate::viterbi::{CostModel, Scanned, Viterbi};

/// The cost added for each break in a path (`MAX_ACCUMULATED_COST`, a float).
const MAX_ACCUMULATED_COST: f32 = 99_999_999.0;

/// Makes map matchers from the `meili` config (mainline's `MapMatcherFactory`).
///
/// The factory is immutable and can be shared between threads; each match needs its own
/// [`Workspace`].
#[derive(Clone, Debug)]
pub struct MapMatcherFactory {
    config: MeiliConfig,
}

impl MapMatcherFactory {
    /// A factory with this config.
    #[inline]
    pub const fn new(config: MeiliConfig) -> Self {
        Self { config }
    }

    /// A factory with the `meili` section of a `valhalla.json`.
    ///
    /// # Errors
    ///
    /// As for [`MeiliConfig::from_valhalla_json`].
    pub fn from_valhalla_json(json: &str) -> Result<Self, ConfigError> {
        Ok(Self::new(MeiliConfig::from_valhalla_json(json)?))
    }

    /// The config (before any request's options).
    #[inline]
    pub const fn config(&self) -> &MeiliConfig {
        &self.config
    }

    /// The config with a request's customizable options (`MergeConfig`).
    #[inline]
    #[must_use]
    pub fn merge_config(&self, options: &Options) -> MeiliConfig {
        self.config.merge(options)
    }

    /// A matcher for a request (`Create`): the merged config and the request's costing
    /// (from `options.costings`, or the costing type's defaults).
    ///
    /// # Errors
    ///
    /// Fails if the costing can't be built, or if the merged turn penalty factor is negative
    /// (mainline: "Expect turn penalty factor to be nonnegative", which Thor reports as 499).
    pub fn create(&self, options: &Options) -> Result<MapMatcher, MeiliError> {
        let config = self.merge_config(options);
        let proto = options
            .costings
            .get(&options.costing_type)
            .cloned()
            .unwrap_or(CostingProto {
                r#type: options.costing_type,
                ..CostingProto::default()
            });
        let costing = AnyCosting::new(&proto)?;
        MapMatcher::new(config, costing)
    }

    /// A matcher for a costing type with its default options (`Create(costing_type)`).
    ///
    /// # Errors
    ///
    /// As for [`MapMatcherFactory::create`].
    pub fn create_for(&self, costing_type: CostingType) -> Result<MapMatcher, MeiliError> {
        self.create(&Options {
            costing_type: costing_type as i32,
            ..Options::default()
        })
    }
}

/// A matcher for one request: its config and costing (mainline's `MapMatcher`, whose search
/// state lives in the [`Workspace`] here).
#[derive(Clone, Debug)]
pub struct MapMatcher {
    config: MeiliConfig,
    costing: AnyCosting,
    /// `1.f / beta`.
    inv_beta: f32,
    /// `1.f / (sigma_z * sigma_z * 2.f)`, a float stored in a double.
    inv_double_sq_sigma_z: f64,
    /// The turn cost per degree of turn, from 0 to 180.
    turn_cost_table: [f32; 181],
}

impl MapMatcher {
    /// A matcher with a (merged) config and a costing.
    ///
    /// # Errors
    ///
    /// Fails, as mainline's cost models throw, if the turn penalty factor is negative
    /// (the other parameters are checked when the config is read).
    pub fn new(config: MeiliConfig, costing: AnyCosting) -> Result<Self, MeiliError> {
        let tc = &config.transition_cost;
        if tc.turn_penalty_factor < 0.0 {
            return Err(MeiliError::TurnPenaltyFactor(tc.turn_penalty_factor));
        }
        let mut turn_cost_table = [0.0_f32; 181];
        if 0.0 < tc.turn_penalty_factor {
            for (i, cost) in turn_cost_table.iter_mut().enumerate() {
                // `factor * std::exp(-i / 45.f)`, in single precision.
                #[expect(clippy::cast_precision_loss, reason = "i is at most 180")]
                let degrees = i as f32;
                *cost = tc.turn_penalty_factor * (-degrees / 45.0).exp();
            }
        }
        let sigma_z = config.emission_cost.sigma_z;
        Ok(Self {
            config,
            costing,
            inv_beta: 1.0 / tc.beta,
            inv_double_sq_sigma_z: f64::from(1.0 / (sigma_z * sigma_z * 2.0)),
            turn_cost_table,
        })
    }

    /// The (merged) config, whose defaults Thor gives measurements without their own
    /// accuracy or radius.
    #[inline]
    pub const fn config(&self) -> &MeiliConfig {
        &self.config
    }

    /// The costing matching uses.
    #[inline]
    pub const fn costing(&self) -> &AnyCosting {
        &self.costing
    }

    /// The costing's travel mode.
    #[inline]
    pub fn travel_mode(&self) -> TravelMode {
        self.costing.base().travel_mode()
    }

    /// The emission cost of a candidate at a squared distance (`CalculateEmissionCost`).
    #[inline]
    fn emission_cost(&self, sq_distance: f32) -> f32 {
        narrow(f64::from(sq_distance) * self.inv_double_sq_sigma_z)
    }

    /// The transition cost of a route (`CalculateTransitionCost`).
    #[inline]
    fn transition_cost(
        &self,
        turn_cost: f32,
        route_distance: f32,
        measurement_distance: f32,
    ) -> f32 {
        (turn_cost + (route_distance - measurement_distance).abs()) * self.inv_beta
    }

    /// Matches a trace (`OfflineMatch`), returning from 1 to `k` matches, best first.
    ///
    /// `interrupt` is called where mainline checks its interrupt (before and after the
    /// candidate search); matching stops if it returns true.
    /// Afterwards, [`Workspace::candidate`] gives each matched state's candidate.
    ///
    /// # Errors
    ///
    /// Fails if `k` is 0, if fewer than two measurements are left or none has candidates
    /// ([`MeiliError::NoCandidates`], error 443), if interrupted, if a tile can't be read,
    /// or where mainline throws from a consistency check
    /// (see [`MeiliError`] for how Thor reports each).
    ///
    /// # Performance
    ///
    /// This searches routes from every candidate the Viterbi search reaches, which is most
    /// of the work. The workspace keeps the candidate grids and the routes' storage.
    #[expect(
        clippy::too_many_lines,
        reason = "mainline's `OfflineMatch`, step by step"
    )]
    pub fn offline_match<P: GraphTileProvider>(
        &self,
        provider: &P,
        workspace: &mut Workspace,
        measurements: &[Measurement],
        k: u32,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<Vec<MatchResults>, MeiliError> {
        if k == 0 {
            return Err(MeiliError::ZeroK);
        }
        let graph = GraphReader::new(provider);
        let Workspace {
            grids,
            container,
            viterbi,
            topk,
            router,
            candidates,
            #[cfg(test)]
            candidate_order,
        } = workspace;
        let cs = &self.config.candidate_search;
        grids.prepare(cs.grid_size, cs.cache_size);
        viterbi.clear();
        topk.clear();
        container.clear();
        if interrupt() {
            return Err(MeiliError::Interrupted);
        }
        let k = k as usize;
        let mut best_paths: Vec<MatchResults> = Vec::with_capacity(k);
        if measurements.is_empty() {
            best_paths.push(MatchResults {
                results: Vec::new(),
                segments: Vec::new(),
                score: 0.0,
            });
            return Ok(best_paths);
        }
        let interpolated = self.append_measurements(
            graph,
            grids,
            container,
            viterbi,
            candidates,
            measurements,
            #[cfg(test)]
            candidate_order.as_deref(),
        )?;
        if !container.has_minimum_candidates() {
            return Err(MeiliError::NoCandidates);
        }
        if interrupt() {
            return Err(MeiliError::Interrupted);
        }

        macro_rules! models {
            () => {
                &mut Models {
                    matcher: self,
                    graph,
                    container: &mut *container,
                    topk: &mut *topk,
                    router: &mut *router,
                }
            };
        }

        let size = container.len();
        let mut sequences: Vec<EdgeSequence> = Vec::new();
        let mut state_ids: Vec<Option<StateId>> = Vec::with_capacity(size);
        let mut found_discontinuity = false;
        while best_paths.len() < k && !found_discontinuity {
            state_ids.clear();
            let mut accumulated_cost = 0.0_f64;
            while state_ids.len() < size {
                let time = u32::try_from(size - state_ids.len() - 1)
                    .map_err(|_| MeiliError::Logic("too many measurements"))?;
                viterbi.search_path(time, models!(), &mut state_ids)?;
                if let Some(winner) = viterbi.search_winner(time, models!())? {
                    accumulated_cost += viterbi.accumulated_cost(winner);
                } else {
                    accumulated_cost += f64::from(MAX_ACCUMULATED_COST);
                    found_discontinuity = true;
                }
                if state_ids.len() < size {
                    found_discontinuity = true;
                    accumulated_cost += f64::from(MAX_ACCUMULATED_COST);
                }
            }
            // Alternates aren't returned if they have discontinuities.
            if !best_paths.is_empty() && found_discontinuity {
                break;
            }
            let original_state_ids: Vec<Option<StateId>> = state_ids
                .iter()
                .rev()
                .map(|s| s.map(|id| topk.origin(id).unwrap_or(id)))
                .collect();
            let results = (0..original_state_ids.len())
                .map(|time| find_match_result(graph, container, &original_state_ids, time))
                .collect::<Result<Vec<_>, _>>()?;
            let mut best_path = Vec::with_capacity(measurements.len());
            for (time, result) in results.iter().enumerate() {
                best_path.push(*result);
                let Some(points) = interpolated.get(time).filter(|p| !p.is_empty()) else {
                    continue;
                };
                let this = original_state_ids[time];
                let next = original_state_ids.get(time + 1).copied().flatten();
                let last_result = results.get(time + 1).ok_or(MeiliError::Logic(
                    "interpolated points after the last measurement",
                ))?;
                self.interpolate_measurements(
                    graph,
                    container,
                    points,
                    this,
                    next,
                    result,
                    last_result,
                    &mut best_path,
                )?;
            }
            let segments = construct_route(container, &best_path)?;
            let sequence = EdgeSequence::new(&segments);
            // Keep it unless an earlier path counts as the same.
            if !sequences.iter().rev().any(|s| s.same(&sequence)) {
                best_paths.push(MatchResults {
                    results: best_path,
                    segments,
                    score: narrow(accumulated_cost),
                });
                sequences.push(sequence);
            }
            // Redundancies can't be removed from broken paths, and nothing is removed
            // after the last path.
            if !found_discontinuity && best_paths.len() < k {
                let original: Vec<StateId> = original_state_ids.iter().flatten().copied().collect();
                self.remove_redundancies(
                    graph, container, viterbi, topk, router, &original, &results,
                )?;
                for clone in topk.remove_path(&state_ids)? {
                    if !viterbi.add_state(clone) {
                        return Err(MeiliError::Logic(
                            "generated clone state IDs must be unique",
                        ));
                    }
                }
                viterbi.clear_search();
            }
        }
        if best_paths.is_empty() || best_paths.len() > k {
            return Err(MeiliError::Logic("the number of paths is out of range"));
        }
        Ok(best_paths)
    }

    /// Adds a column for each measurement far enough from the last one, and returns the
    /// others by the column they follow (`AppendMeasurements`).
    #[cfg_attr(
        test,
        expect(
            clippy::too_many_arguments,
            reason = "the workspace's parts, and the oracle test's candidate order"
        )
    )]
    fn append_measurements<P: GraphTileProvider>(
        &self,
        graph: GraphReader<'_, P>,
        grids: &mut CandidateGrids,
        container: &mut Container,
        viterbi: &mut Viterbi,
        candidates: &mut Vec<Candidate>,
        measurements: &[Measurement],
        #[cfg(test)] order: Option<&[Vec<Vec<u64>>]>,
    ) -> Result<Vec<Vec<Measurement>>, MeiliError> {
        let max = self.config.candidate_search.max_search_radius_meters;
        let sq_max_search_radius = max * max;
        let d = self.config.routing.interpolation_distance_meters;
        let sq_interpolation_distance = d * d;
        let mut interpolated: Vec<Vec<Measurement>> = Vec::new();
        let mut append = |m: &Measurement,
                          container: &mut Container,
                          interpolated: &mut Vec<Vec<Measurement>>|
         -> Result<u32, MeiliError> {
            // `std::min(max, std::max(radius, accuracy))`.
            let (r, a) = (m.sq_search_radius(), m.sq_gps_accuracy());
            let wanted = if r < a { a } else { r };
            let sq_radius = if wanted < sq_max_search_radius {
                wanted
            } else {
                sq_max_search_radius
            };
            candidates.clear();
            #[cfg(test)]
            {
                grids.keep_repeated_nodes = order.is_some();
            }
            grids.query(graph, m.point(), sq_radius, &self.costing, candidates)?;
            #[cfg(test)]
            replay_order(order, container.len(), candidates);
            let time = container.append_measurement(*m);
            interpolated.push(Vec::new());
            for candidate in candidates.drain(..) {
                let id = container
                    .append_candidate(candidate)
                    .ok_or(MeiliError::Logic("add measurement first"))?;
                viterbi.add_state(id);
            }
            Ok(time)
        };
        let Some(first) = measurements.first() else {
            return Ok(interpolated);
        };
        let mut last = first;
        let mut time = append(first, container, &mut interpolated)?;
        let mut interpolated_epoch_time = -1.0_f64;
        for (i, m) in measurements.iter().enumerate().skip(1) {
            let sq_distance = last.distance_squared(m);
            if sq_interpolation_distance < sq_distance || i + 1 == measurements.len() {
                if interpolated_epoch_time != -1.0
                    && let Some(lingered) = interpolated.get(time as usize).and_then(|p| p.last())
                {
                    // If the last interpolated point is much closer to the last match point
                    // than to this one, the trace lingered: it left at that point's time.
                    let p = Projector::new(lingered.point()).project(last.point(), m.point());
                    if p.distance(last.point()) / last.point().distance(m.point())
                        < f64::from(0.2_f32)
                    {
                        container.set_leave_time(time, interpolated_epoch_time);
                    }
                }
                time = append(m, container, &mut interpolated)?;
                last = m;
                interpolated_epoch_time = -1.0;
            } else {
                if let Some(points) = interpolated.get_mut(time as usize) {
                    points.push(*m);
                }
                interpolated_epoch_time = m.epoch_time();
            }
        }
        Ok(interpolated)
    }

    /// Matches the measurements interpolated between two states onto the route between them
    /// (`InterpolateMeasurements`), appending the results.
    #[expect(clippy::too_many_arguments, reason = "mainline's signature")]
    fn interpolate_measurements<P: GraphTileProvider>(
        &self,
        graph: GraphReader<'_, P>,
        container: &Container,
        measurements: &[Measurement],
        state: Option<StateId>,
        next_state: Option<StateId>,
        first_result: &MatchResult,
        last_result: &MatchResult,
        out: &mut Vec<MatchResult>,
    ) -> Result<(), MeiliError> {
        let (Some(state), Some(next_state)) = (state, next_state) else {
            out.extend(measurements.iter().map(|m| unmatched(m, None)));
            return Ok(());
        };
        let mut route = Vec::new();
        merge_route(container, state, next_state, &mut route, last_result)?;
        let match_measurement = *container
            .measurement(state.time)
            .ok_or(MeiliError::Logic("no such measurement"))?;
        let mut left_most_segment = 0;
        let mut left_most_offset = route.first().map_or(0.0, |s| narrow(s.source));
        let mut left_most_projection = PointLL::new(first_result.lnglat.0, first_result.lnglat.1);
        let right_most = PointLL::new(last_result.lnglat.0, last_result.lnglat.1);
        for m in measurements {
            let distance = m.distance(&match_measurement);
            let interp = self.interpolate_measurement(
                graph,
                m,
                &route,
                left_most_segment,
                left_most_offset,
                distance,
                left_most_projection,
                right_most,
            )?;
            match interp {
                Some(interp) => {
                    // The next point is interpolated after this one.
                    left_most_segment = interp.segment;
                    left_most_offset = narrow(interp.edge_distance);
                    left_most_projection = interp.projected;
                    out.push(MatchResult {
                        lnglat: (interp.projected.lng, interp.projected.lat),
                        distance_from: interp.sq_distance.sqrt(),
                        edge_id: Some(interp.edge_id),
                        distance_along: interp.edge_distance,
                        epoch_time: m.epoch_time(),
                        state_id: None,
                        is_break_point: m.is_break_point(),
                        begins_discontinuity: false,
                        ends_discontinuity: false,
                        edge_index: None,
                    });
                }
                None => out.push(unmatched(m, None)),
            }
        }
        Ok(())
    }

    /// The point along `route[begin..]` (from `begin_source_offset` on the first segment)
    /// where a measurement's transition and emission costs are least
    /// (`InterpolateMeasurement`), or `None` if there is none.
    #[expect(clippy::too_many_arguments, reason = "mainline's signature")]
    fn interpolate_measurement<P: GraphTileProvider>(
        &self,
        graph: GraphReader<'_, P>,
        measurement: &Measurement,
        route: &[EdgeSegment],
        begin: usize,
        begin_source_offset: f32,
        match_measurement_distance: f32,
        left_most_projected_point: PointLL,
        right_most_projected_point: PointLL,
    ) -> Result<Option<Interpolation>, MeiliError> {
        let projector = Projector::new(measurement.point());
        let mut segment_begin_route_distance = 0.0_f32;
        let mut minimal_cost = f32::INFINITY;
        let mut best = None;
        for (index, segment) in route.iter().enumerate().skip(begin) {
            let Some(tile) = graph.tile(segment.edge_id)? else {
                continue;
            };
            let de = edge(&tile, segment.edge_id)?;
            let points = shape(&tile.get_edge_info(de)?)?;
            let Some(projection) = project(&projector, &points, 0.0) else {
                continue;
            };
            // Mainline unpacks the projection into floats.
            let mut projected = projection.point;
            let mut sq_distance = narrow(projection.sq_distance);
            let mut offset = narrow(projection.percent_along);
            if !de.edge_info_is_forward() {
                offset = 1.0 - offset;
            }
            // Clip to the ends of the match.
            if index == begin && offset < begin_source_offset {
                offset = begin_source_offset;
                sq_distance = narrow(projector.approx.distance_squared(left_most_projected_point));
                projected = left_most_projected_point;
            }
            if index + 1 == route.len() && f64::from(offset) > segment.target {
                offset = narrow(segment.target);
                sq_distance = narrow(
                    projector
                        .approx
                        .distance_squared(right_most_projected_point),
                );
                projected = right_most_projected_point;
            }
            let length = f64::from(de.length());
            let distance_to_segment_ends = (length * (f64::from(offset) - segment.source)).abs();
            let route_distance = f64::from(segment_begin_route_distance) + distance_to_segment_ends;
            let edge_percent = segment.target - segment.source;
            let secs = self
                .costing
                .edge_cost_untimed(de, segment.edge_id, &tile)
                .map_err(MeiliError::EdgeCost)?
                .cost
                .secs;
            let route_time = f64::from(secs) * edge_percent;
            let interp = Interpolation {
                projected,
                edge_id: segment.edge_id,
                sq_distance: f64::from(sq_distance),
                route_distance,
                route_time,
                edge_distance: f64::from(offset),
                segment: index,
            };
            let cost = self.transition_cost(
                0.0,
                narrow(interp.route_distance),
                match_measurement_distance,
            ) + self.emission_cost(narrow(interp.sq_distance));
            if cost < minimal_cost {
                minimal_cost = cost;
                best = Some(interp);
            }
            // The segments are assumed to be connected.
            segment_begin_route_distance =
                narrow(f64::from(segment_begin_route_distance) + length * edge_percent);
        }
        Ok(best)
    }

    /// Removes the candidates whose routes to the next column are all already on the path
    /// found, so the next search finds a path that differs (`RemoveRedundancies`).
    #[expect(
        clippy::too_many_arguments,
        reason = "the matcher's state, split for borrowing"
    )]
    fn remove_redundancies<P: GraphTileProvider>(
        &self,
        graph: GraphReader<'_, P>,
        container: &mut Container,
        viterbi: &mut Viterbi,
        topk: &mut TopK,
        router: &mut Router,
        result: &[StateId],
        results: &[MatchResult],
    ) -> Result<(), MeiliError> {
        let Some(pairs) = result.len().checked_sub(1) else {
            return Ok(());
        };
        let mut edges = Vec::new();
        for pair in 0..pairs {
            let left = result[pair];
            let time = left.time;
            let next_time = time + 1;
            let target_result = results
                .get(next_time as usize)
                .ok_or(MeiliError::Logic("no match result for the next state"))?;
            let column_id = |t: u32, j: usize| StateId {
                time: t,
                id: u32::try_from(j).unwrap_or(u32::MAX),
            };
            let mut paths_from_winner: HashMap<StateId, EdgeSequence> = HashMap::new();
            for j in 0..container.column(next_time).len() {
                let right = column_id(next_time, j);
                edges.clear();
                if !topk.is_removed(right)
                    && merge_route(container, left, right, &mut edges, target_result)?
                {
                    paths_from_winner.insert(right, EdgeSequence::new(&edges));
                }
            }
            let mut right_uniques: HashSet<StateId> = HashSet::new();
            let mut redundancies: Vec<StateId> = Vec::new();
            for i in 0..container.column(time).len() {
                let loser = column_id(time, i);
                if loser == left || topk.is_removed(loser) {
                    continue;
                }
                if !container
                    .state(loser)
                    .is_some_and(crate::state::State::routed)
                {
                    let mut models = Models {
                        matcher: self,
                        graph,
                        container: &mut *container,
                        topk: &mut *topk,
                        router: &mut *router,
                    };
                    models.base_transition(viterbi.scanned(), loser, column_id(next_time, 0))?;
                }
                let mut found_unique = false;
                for (j, right_state) in container.column(next_time).iter().enumerate() {
                    let right = column_id(next_time, j);
                    edges.clear();
                    if topk.is_removed(right)
                        || !merge_route(container, loser, right, &mut edges, target_result)?
                    {
                        continue;
                    }
                    let from_loser = EdgeSequence::new(&edges);
                    let unique = match paths_from_winner.get(&right) {
                        None => true,
                        Some(from_winner) => {
                            (from_loser.edges.is_empty()
                                && !from_winner.crosses(&right_state.candidate))
                                || from_winner.differs(&from_loser)
                        }
                    };
                    if unique {
                        found_unique = true;
                        right_uniques.insert(right);
                        break;
                    }
                }
                if !found_unique {
                    redundancies.push(loser);
                }
            }
            for r in redundancies.drain(..) {
                topk.remove(r);
                viterbi.remove_state(r)?;
            }
            // After the last pair, right candidates without unique routes go too.
            if pair + 2 == result.len() {
                let winner = result[pair + 1];
                for j in 0..container.column(next_time).len() {
                    let right = column_id(next_time, j);
                    if right != winner && !right_uniques.contains(&right) {
                        redundancies.push(right);
                    }
                }
                for r in redundancies.drain(..) {
                    topk.remove(r);
                    viterbi.remove_state(r)?;
                }
            }
        }
        Ok(())
    }
}

/// An interpolated point (mainline's `Interpolation`).
#[derive(Clone, Copy, Debug)]
struct Interpolation {
    projected: PointLL,
    edge_id: GraphId,
    sq_distance: f64,
    route_distance: f64,
    #[expect(
        dead_code,
        reason = "mainline computes it, but its transition cost ignores time"
    )]
    route_time: f64,
    edge_distance: f64,
    segment: usize,
}

/// A result without an edge (`CreateMatchResult(measurement, state_id)`).
fn unmatched(m: &Measurement, state_id: Option<StateId>) -> MatchResult {
    MatchResult {
        lnglat: m.lnglat(),
        distance_from: 0.0,
        edge_id: None,
        distance_along: -1.0,
        epoch_time: m.epoch_time(),
        state_id,
        is_break_point: m.is_break_point(),
        begins_discontinuity: false,
        ends_discontinuity: false,
        edge_index: None,
    }
}

/// The end node of the directed edge with this ID, if its tile is there
/// (`GraphReader::directededge(id)->endnode()`).
fn end_node<P: GraphTileProvider>(
    graph: GraphReader<'_, P>,
    id: GraphId,
) -> Result<Option<GraphId>, GraphReadError> {
    let Some(tile) = graph.tile(id)? else {
        return Ok(None);
    };
    Ok(Some(edge(&tile, id)?.end_node_id()))
}

/// The end node of an edge's opposing edge (its start node), if the tiles are there
/// (`GraphReader::GetOpposingEdge(id)->endnode()`).
fn opposing_end_node<P: GraphTileProvider>(
    graph: GraphReader<'_, P>,
    id: GraphId,
) -> Result<Option<GraphId>, GraphReadError> {
    let Some(tile) = graph.tile(id)? else {
        return Ok(None);
    };
    let Some((opp_id, opp_tile)) = graph.opposing(id, &tile)? else {
        return Ok(None);
    };
    Ok(Some(edge(&opp_tile, opp_id)?.end_node_id()))
}

/// The match result of the state at `time`, from the edges of the routes into and out of it
/// (`FindMatchResult`).
///
/// A candidate at a node is matched to the edge the path arrives on (at 1) or leaves on (at 0),
/// since the routes go to and from the node, not the candidate's own edge.
#[expect(
    clippy::too_many_lines,
    reason = "mainline's `FindMatchResult`, step by step"
)]
fn find_match_result<P: GraphTileProvider>(
    graph: GraphReader<'_, P>,
    container: &Container,
    ids: &[Option<StateId>],
    time: usize,
) -> Result<MatchResult, MeiliError> {
    let m = container
        .measurement(u32::try_from(time).unwrap_or(u32::MAX))
        .ok_or(MeiliError::Logic("no such measurement"))?;
    let Some(state_id) = ids[time] else {
        return Ok(unmatched(m, None));
    };
    let mut begins_discontinuity = false;
    let mut ends_discontinuity = false;
    let mut prev_edge: Option<GraphId> = None;
    let mut node_id: Option<GraphId> = None;
    // The last edge of the path into the state; a route to a node may have none,
    // so look further back.
    let mut t = time;
    while t > 0 && prev_edge.is_none() {
        let Some(prev_state) = ids[t - 1] else {
            if t == time {
                ends_discontinuity = true;
            }
            break;
        };
        let Some(state) = ids[t] else {
            break;
        };
        let mut route = container.route(prev_state, state).peekable();
        let Some(first) = route.peek() else {
            break;
        };
        node_id = first.node_id;
        for label in route {
            prev_edge = label.edge_id;
            if prev_edge.is_some() {
                break;
            }
        }
        t -= 1;
    }
    // The first edge of the path out of the state, looking further ahead likewise.
    let mut next_edge: Option<GraphId> = None;
    let mut t = time;
    while t + 1 < ids.len() && next_edge.is_none() {
        let Some(next_state) = ids[t + 1] else {
            if t == time {
                begins_discontinuity = true;
            }
            break;
        };
        let Some(state) = ids[t] else {
            break;
        };
        let mut route = container.route(state, next_state).peekable();
        if route.peek().is_none() {
            break;
        }
        for label in route {
            if label.node_id.is_some() {
                node_id = label.node_id;
            }
            if label.edge_id.is_some() {
                next_edge = label.edge_id;
            }
        }
        t += 1;
    }

    let state = container
        .state(state_id)
        .ok_or(MeiliError::Logic("no such state"))?;
    let matched =
        |e: &crate::candidate_search::CandidateEdge, edge_id: GraphId, along: f64| MatchResult {
            lnglat: e.point,
            distance_from: e.sq_distance.sqrt(),
            edge_id: Some(edge_id),
            distance_along: along,
            epoch_time: m.epoch_time(),
            state_id: Some(state_id),
            is_break_point: m.is_break_point(),
            begins_discontinuity,
            ends_discontinuity,
            edge_index: None,
        };
    if prev_edge.is_none() && next_edge.is_none() {
        // No routes, or only a route from a node to itself: any candidate edge will do.
        if node_id.is_none() {
            return Ok(unmatched(m, Some(state_id)));
        }
        let candidate = state
            .candidate
            .edges
            .first()
            .ok_or(MeiliError::Logic("a state without candidate edges"))?;
        return Ok(matched(
            candidate,
            candidate.edge_id,
            candidate.percent_along,
        ));
    }
    let prev_end = match prev_edge {
        Some(id) => end_node(graph, id)?,
        None => None,
    };
    let next_start = match next_edge {
        Some(id) => opposing_end_node(graph, id)?,
        None => None,
    };
    for candidate in &state.candidate.edges {
        if Some(candidate.edge_id) == prev_edge || Some(candidate.edge_id) == next_edge {
            return Ok(matched(
                candidate,
                candidate.edge_id,
                candidate.percent_along,
            ));
        }
        // Other edges can only match at intersections.
        if candidate.percent_along > 0.0 && candidate.percent_along < 1.0 {
            continue;
        }
        let candidate_node = match edge_nodes(graph, candidate.edge_id)? {
            Some((start, end)) => {
                if candidate.percent_along == 0.0 {
                    start
                } else {
                    Some(end)
                }
            }
            None => None,
        };
        if let (Some(end), Some(id)) = (prev_end, prev_edge)
            && Some(end) == candidate_node
        {
            return Ok(matched(candidate, id, 1.0));
        }
        if let (Some(start), Some(id)) = (next_start, next_edge)
            && Some(start) == candidate_node
        {
            return Ok(matched(candidate, id, 0.0));
        }
        // The route's node may be the candidate's node on another level.
        // Mainline reads the node's transitions from whichever tile it loaded last
        // (another tile's node at the same index, or no tile at all), which is a bug no client
        // can depend on: this reads the node's own tile (see "Known divergences" in
        // `docs/routing.md`).
        for (end, target, along) in [(prev_end, prev_edge, 1.0), (next_start, next_edge, 0.0)] {
            let (Some(end), Some(target), Some(candidate_node)) = (end, target, candidate_node)
            else {
                continue;
            };
            if candidate.edge_id.level() == end.level() {
                continue;
            }
            let Some(tile) = graph.tile(end)? else {
                continue;
            };
            let end_info = node(&tile, end)?;
            for transition in tile.get_transitions(end_info) {
                let other = transition.corresponding_end_node_id();
                if other.level() != candidate_node.level() {
                    continue;
                }
                if graph.tile(other)?.is_none() || other != candidate_node {
                    break;
                }
                return Ok(matched(candidate, target, along));
            }
        }
    }
    // Mainline expects never to get here.
    Ok(unmatched(m, Some(state_id)))
}

/// The cost models of the Viterbi search over a workspace's state
/// (mainline's `EmissionCostModel` and `TransitionCostModel`, through the top-k layers).
struct Models<'a, 'p, P: GraphTileProvider> {
    matcher: &'a MapMatcher,
    graph: GraphReader<'p, P>,
    container: &'a mut Container,
    topk: &'a mut TopK,
    router: &'a mut Router,
}

/// `EmissionCostModel::operator()`: from the squared distance of the state's first edge.
fn base_emission(
    matcher: &MapMatcher,
    container: &Container,
    id: StateId,
) -> Result<f32, MeiliError> {
    let candidate = container
        .state(id)
        .and_then(|s| s.candidate.edges.first())
        .ok_or(MeiliError::Logic("no such state"))?;
    Ok(matcher.emission_cost(narrow(candidate.sq_distance)))
}

impl<P: GraphTileProvider> Models<'_, '_, P> {
    /// The time between leaving one column's measurement and reaching another's,
    /// or -1 without times (`TransitionCostModel::ClockDistance`).
    fn clock_distance(&self, lhs: u32, rhs: u32) -> f32 {
        let leave = self.container.leave_time(lhs).unwrap_or(-1.0);
        let epoch = self
            .container
            .measurement(rhs)
            .map_or(-1.0, Measurement::epoch_time);
        if 0.0 <= leave && 0.0 <= epoch {
            narrow(epoch - leave)
        } else {
            -1.0
        }
    }

    /// `TransitionCostModel::operator()`: routes from `lhs` if it hasn't been, then costs
    /// the route to `rhs` (-1 for none).
    fn base_transition(
        &mut self,
        scanned: &Scanned,
        lhs: StateId,
        rhs: StateId,
    ) -> Result<f32, MeiliError> {
        let routed = self
            .container
            .state(lhs)
            .ok_or(MeiliError::Logic("no such state"))?
            .routed();
        if !routed {
            self.update_route(scanned, lhs, rhs)?;
        }
        let Some((labels, idx)) = self.container.last_label(lhs, rhs) else {
            return Ok(-1.0);
        };
        let label = labels
            .get(idx as usize)
            .ok_or(MeiliError::Logic("a route points past its labels"))?;
        let (Some(left), Some(right)) = (
            self.container.measurement(lhs.time),
            self.container.measurement(rhs.time),
        ) else {
            return Err(MeiliError::Logic("no such measurement"));
        };
        Ok(self
            .matcher
            .transition_cost(label.turn_cost, label.cost.cost, left.distance(right)))
    }

    /// Routes from `lhs` to every state of `rhs`'s column (`UpdateRoute`), starting from the
    /// last label of the route that reached `lhs` in the search so far.
    fn update_route(
        &mut self,
        scanned: &Scanned,
        lhs: StateId,
        rhs: StateId,
    ) -> Result<(), MeiliError> {
        let edgelabel = match scanned.get(&lhs).and_then(|l| l.predecessor) {
            Some(prev) => {
                let original = self.topk.origin(prev).unwrap_or(prev);
                let prev_state = self
                    .container
                    .state(original)
                    .ok_or(MeiliError::Logic("no such state"))?;
                if !prev_state.routed() {
                    return Err(MeiliError::Logic(
                        "The predecessor of current state must have been routed.",
                    ));
                }
                self.container
                    .last_label(original, lhs)
                    .and_then(|(labels, idx)| labels.get(idx as usize).copied())
            }
            None => None,
        };
        let (Some(&left_m), Some(&right_m)) = (
            self.container.measurement(lhs.time),
            self.container.measurement(rhs.time),
        ) else {
            return Err(MeiliError::Logic("no such measurement"));
        };
        let tc = &self.matcher.config.transition_cost;
        // `std::min(distance * factor, breakage)`, then `std::ceil(std::max(d, 1.f))`.
        let wanted = left_m.distance(&right_m) * tc.max_route_distance_factor;
        let max_route_distance = if tc.breakage_distance_meters < wanted {
            tc.breakage_distance_meters
        } else {
            wanted
        };
        let max_route_distance = if max_route_distance < 1.0 {
            1.0
        } else {
            max_route_distance
        }
        .ceil();
        let mut max_route_time = self.clock_distance(lhs.time, rhs.time) * tc.max_route_time_factor;
        if 0.0 <= max_route_time {
            max_route_time = max_route_time.ceil();
        }
        let mut labels = self.container.take_labels();
        {
            let left = self
                .container
                .state(lhs)
                .ok_or(MeiliError::Logic("no such state"))?;
            let destinations: Vec<&Candidate> = core::iter::once(&left.candidate)
                .chain(self.container.column(rhs.time).iter().map(|s| &s.candidate))
                .collect();
            let search = Search {
                graph: self.graph,
                costing: &self.matcher.costing,
                destinations: &destinations,
                origin_type: left_m.stop_type(),
                approximator: DistanceApproximator::new(right_m.point()),
                radius: right_m.search_radius(),
                edgelabel,
                turn_cost_table: &self.matcher.turn_cost_table,
                max_dist: max_route_distance,
                max_time: max_route_time,
            };
            self.router.find_shortest_path(&search, &mut labels)?;
        }
        let dests = self.router.results.get(1..).unwrap_or(&[]).to_vec();
        if let Some(state) = self.container.state_mut(lhs) {
            state.route = Some(Route { labels, dests });
        }
        Ok(())
    }
}

impl<P: GraphTileProvider> CostModel for Models<'_, '_, P> {
    fn emission(&mut self, state: StateId) -> Result<f32, MeiliError> {
        let Self {
            matcher,
            container,
            topk,
            ..
        } = self;
        topk.emission(state, &mut |id| base_emission(matcher, container, id))
    }

    fn transition(
        &mut self,
        scanned: &Scanned,
        lhs: StateId,
        rhs: StateId,
    ) -> Result<f32, MeiliError> {
        match self.topk.resolve_transition(lhs, rhs) {
            Resolved::Original((lhs, rhs)) => self.base_transition(scanned, lhs, rhs),
            Resolved::Invalid => Ok(-1.0),
        }
    }
}

/// What a match needs between calls: the candidate grids (kept across requests, as mainline
/// keeps them in its factory) and the search state of the last match
/// (as mainline keeps it in the matcher), whose storage is reused.
///
/// Use one workspace per thread and per tile set.
#[derive(Debug, Default)]
pub struct Workspace {
    grids: CandidateGrids,
    container: Container,
    viterbi: Viterbi,
    topk: TopK,
    router: Router,
    candidates: Vec<Candidate>,
    /// For the oracle tests: the order mainline found each column's candidates in
    /// (by the point of their first edge), to replay it.
    #[cfg(test)]
    pub(crate) candidate_order: Option<Vec<Vec<Vec<u64>>>>,
}

/// Replaces a column's candidates with mainline's, if the oracle test gave them (by their
/// edges): `candidates` must be every candidate, without dropping repeated nodes.
#[cfg(test)]
fn replay_order(order: Option<&[Vec<Vec<u64>>]>, time: usize, candidates: &mut Vec<Candidate>) {
    let Some(states) = order.and_then(|o| o.get(time)) else {
        return;
    };
    let mut all = core::mem::take(candidates);
    for edges in states {
        if let Some(at) = all.iter().position(|c| {
            c.edges
                .iter()
                .map(|e| e.edge_id.value())
                .eq(edges.iter().copied())
        }) {
            candidates.push(all.swap_remove(at));
        }
    }
}

impl Workspace {
    /// An empty workspace.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// The candidate of a state of the last match, as its match results name it
    /// (mainline's `state_container().state(id).candidate()`).
    /// Thor reports the number of its edges as OSRM's `alternatives_count` (plus one).
    #[inline]
    pub fn candidate(&self, state: StateId) -> Option<&Candidate> {
        self.container.state(state).map(|s| &s.candidate)
    }

    /// The candidate grids.
    #[inline]
    pub const fn grids(&self) -> &CandidateGrids {
        &self.grids
    }

    /// Drops the candidate grids (mainline's `ClearCache`), as when the tile set changes.
    pub fn clear_cache(&mut self) {
        self.grids = CandidateGrids::default();
    }
}
