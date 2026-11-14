//! The map matching actions (mainline's `trace_route_action.cc`, `trace_attributes_action.cc`,
//! and the trace parts of `thor_worker_t`): `trace_route` and `trace_attributes`.
//!
//! Both start as mainline's do: Loki's candidates are scored (`adjust_locations`), the costing
//! is made, and so are the map matcher and the measurements (`parse_measurements`, even for an
//! edge walk). Then the request's `shape_match` picks how the shape becomes a path:
//!
//! - `edge_walk`: the shape is a route's own shape, walked along the graph
//!   ([`crate::route_matcher`]): mainline's walk, and where it finds no path, the same walk
//!   with lenient shape checks (see "Known divergences" in `docs/routing.md`).
//!   A walk that fails is error 443.
//! - `map_snap`: Meili matches the shape (best only for `trace_route`, with `alternates` more
//!   for `trace_attributes`), and its segments become paths ([`crate::map_matcher`]).
//!   `trace_route` makes a route per discontinuity and a leg per break point;
//!   `trace_attributes` joins every path of a match into one leg, with the edges after a
//!   discontinuity disconnected. A match that fails is error 442 for `trace_route`
//!   (Meili's own errors, such as 443 for a trace without candidates, pass through),
//!   and 444 for `trace_attributes`.
//! - `walk_or_snap`: an edge walk, and if it fails, the request as it was before the walk
//!   (without the warnings and statistics it had) is map matched.
//!
//! `trace_route` leaves the trip for Odin; `trace_attributes` answers itself, with Tyr's
//! `serialize_trace_attributes`, as mainline's Thor does.

use std::collections::BTreeMap;

use jiff::Timestamp;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::GraphTile;
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::options::{Action, Format, HasAlternates};
use valhalla_proto::{Api, LatLng, Location, Options, PathEdge as CandidateEdge, ShapeMatch, Trip};
use valhalla_response::{ErrorCode, ValhallaError};
use valinor_meili::{
    EdgeSegment, MapMatcher, MatchResult, Measurement, MeiliError, Workspace as MeiliWorkspace,
};
use valinor_sif::AnyCosting;
use valinor_sif::costing_options::costing_name;
use valinor_tyr::AttributesController;
use valinor_tyr::tyr::{MapMatchResult, Response, serialize_trace_attributes};

use crate::error::ThorError;
use crate::graph;
use crate::map_matcher::{self, MatchedPath};
use crate::path::{EdgeTrim, EdgeTrimming, Leg, PathEdge, Paths, Route};
use crate::route::{Thor, adjust_locations};
use crate::route_matcher;

/// Why map matching (`map_match`) failed, as mainline's actions tell its exceptions apart.
#[derive(Debug)]
enum MatchError {
    /// A `valhalla_exception_t`, which `trace_route` passes on
    /// (Meili's 443 for a trace without candidates, or `build_trace`'s 442).
    Valhalla(ValhallaError),
    /// Any other failure (no path, a missing tile, a failed consistency check),
    /// which `trace_route` reports as 442.
    Other(#[expect(dead_code, reason = "mainline logs it; clients see only the code")] ThorError),
    /// The interrupt asked to stop.
    Interrupted,
}

impl From<ThorError> for MatchError {
    fn from(error: ThorError) -> Self {
        match error {
            ThorError::Interrupted => Self::Interrupted,
            other => Self::Other(other),
        }
    }
}

impl From<MeiliError> for MatchError {
    fn from(error: MeiliError) -> Self {
        match error {
            MeiliError::Interrupted => Self::Interrupted,
            // `throw valhalla_exception_t{443}`.
            MeiliError::NoCandidates => {
                Self::Valhalla(ValhallaError::new(ErrorCode::ExactRouteMatchFailed))
            }
            other => Self::Other(ThorError::Options(other.to_string())),
        }
    }
}

/// What both actions set up before matching (the start of `trace_route` and `trace_attributes`).
struct Prepared {
    costing: AnyCosting,
    matcher: MapMatcher,
    trace: Vec<Measurement>,
    /// The options before matching, for `walk_or_snap`'s fallback.
    options: Options,
}

/// A failure where mainline throws `std::exception{}` (the `catch (...)` blocks).
fn walk_failed() -> ThorError {
    ThorError::EdgeWalk("the edge walk found no path")
}

/// A shape point as a protobuf point.
fn lat_lng(lnglat: (f64, f64)) -> LatLng {
    LatLng {
        has_lat: Some(HasLat::Lat(lnglat.1)),
        has_lng: Some(HasLng::Lng(lnglat.0)),
    }
}

/// Replaces a location's candidates with the one edge a match uses (`add_path_edge`):
/// mainline passes the percent along and distance as floats, and sets no side of street.
fn add_path_edge(
    location: &mut Location,
    edge_id: GraphId,
    percent_along: f64,
    ll: (f64, f64),
    distance: f64,
) {
    #[expect(clippy::cast_possible_truncation, reason = "mainline passes floats")]
    let (percent_along, distance) = (percent_along as f32, distance as f32);
    let correlation = location.correlation.get_or_insert_default();
    correlation.edges.clear();
    correlation.edges.push(CandidateEdge {
        graph_id: edge_id.value(),
        percent_along: f64::from(percent_along),
        ll: Some(lat_lng(ll)),
        distance: f64::from(distance),
        ..CandidateEdge::default()
    });
}

/// The edge trimming of a leg as trip building takes it, from mainline's map by edge index.
fn trimming_of(map: BTreeMap<usize, (EdgeTrimming, EdgeTrimming)>) -> Vec<EdgeTrim> {
    map.into_iter()
        .map(|(edge_index, (begin, end))| EdgeTrim {
            edge_index,
            begin,
            end,
        })
        .collect()
}

/// No trimming (a default `EdgeTrimmingInfo`).
const NO_TRIM: EdgeTrimming = EdgeTrimming {
    trim: false,
    point: (0.0, 0.0),
    percent_along: 0.0,
};

/// A trim at a matched point.
fn trim_at(m: &MatchResult, percent_along: f64) -> EdgeTrimming {
    EdgeTrimming {
        trim: true,
        point: m.lnglat,
        percent_along,
    }
}

/// A leg of one location to another along `edges` (what mainline passes `TripLegBuilder::Build`).
fn leg(
    origin: (usize, &Location),
    destination: (usize, &Location),
    edges: Vec<PathEdge>,
    algorithm: &'static str,
    edge_trimming: Vec<EdgeTrim>,
) -> Leg {
    Leg {
        origin: origin.0,
        destination: destination.0,
        intermediates: Vec::new(),
        locations: vec![origin.1.clone(), destination.1.clone()],
        begin_percent: None,
        end_percent: None,
        edges,
        algorithms: vec![algorithm],
        edge_trimming,
    }
}

/// The index of a match result, where mainline reads it without a check.
fn match_index(index: Option<usize>, len: usize) -> Result<usize, ThorError> {
    index.filter(|&i| i < len).ok_or(ThorError::Options(
        "a matched path ends without a match".to_owned(),
    ))
}

impl<P: GraphTileProvider> Thor<P> {
    /// The start of both actions: `adjust_locations`, `parse_costing`, and `parse_measurements`.
    fn prepare_trace(&self, api: &mut Api) -> Result<Prepared, ValhallaError> {
        let options = api
            .options
            .as_ref()
            .ok_or(ThorError::Options("the request has no options".to_owned()))?;
        let costing_type = valhalla_proto::costing::Type::try_from(options.costing_type)
            .map_err(|_| ThorError::Options(format!("unknown costing {}", options.costing_type)))?;
        adjust_locations(api, costing_name(costing_type));
        let options = api
            .options
            .as_ref()
            .ok_or(ThorError::Options("the request has no options".to_owned()))?;
        let proto = options
            .costings
            .get(&options.costing_type)
            .cloned()
            .unwrap_or(valhalla_proto::Costing {
                r#type: options.costing_type,
                ..valhalla_proto::Costing::default()
            });
        let costing = AnyCosting::new(&proto).map_err(|e| ThorError::Options(e.to_string()))?;
        // Mainline rethrows the matcher's `std::invalid_argument` as a `std::runtime_error`
        // (error 499 with its message).
        let matcher = self
            .matcher_factory()
            .create(options)
            .map_err(|e| match e {
                MeiliError::TurnPenaltyFactor(_) => {
                    ThorError::Options("Expect turn penalty factor to be nonnegative".to_owned())
                }
                other => ThorError::Options(other.to_string()),
            })?;
        let trace = Measurement::from_shape(&options.shape, matcher.config())
            .map_err(|_| ValhallaError::new(ErrorCode::ThorShapeParseFailed))?;
        Ok(Prepared {
            costing,
            matcher,
            trace,
            options: options.clone(),
        })
    }

    /// Matches a `trace_route` request that Loki has correlated and builds its trip
    /// (mainline's `thor_worker_t::trace_route`), leaving the trip for Odin.
    ///
    /// The request is updated as mainline updates it: the shape points the paths start and end
    /// at get the edges they use (and, for OSRM responses, every matched point gets its edge,
    /// names, and the number of other candidates), each point gets its route and waypoint index,
    /// and points at the start of a leg may get a `date_time`.
    ///
    /// # Errors
    ///
    /// Error 443 if an `edge_walk` fails (or Meili finds no candidates),
    /// 442 if map matching fails, 424 for a shape point with a negative accuracy or radius,
    /// 400 for an unknown `shape_match`, and 499 where mainline throws otherwise.
    pub fn trace_route(
        &self,
        workspace: &mut MeiliWorkspace,
        api: &mut Api,
    ) -> Result<(), ValhallaError> {
        self.trace_route_interruptible(workspace, api, Timestamp::now(), &|| false)
    }

    /// [`Thor::trace_route`], with `now` as the current time, giving up when `interrupt`
    /// returns true (checked where Meili checks it, and before each leg is built).
    ///
    /// # Errors
    ///
    /// As for [`Thor::trace_route`]; an interrupt is error 499, which callers should drop
    /// rather than send.
    pub fn trace_route_interruptible(
        &self,
        workspace: &mut MeiliWorkspace,
        api: &mut Api,
        now: Timestamp,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<(), ValhallaError> {
        let prepared = self.prepare_trace(api)?;
        let controller = AttributesController::new(&prepared.options, false);
        let shape_match = ShapeMatch::try_from(prepared.options.shape_match).map_err(|_| {
            ValhallaError::with_detail(ErrorCode::ThorUnknownAction, "Invalid shape_match value")
        })?;
        let walk_failed_error = || {
            ValhallaError::with_detail(
                ErrorCode::ExactRouteMatchFailed,
                format!(
                    "{} algorithm failed to find exact route match.  Try using \
                     shape_match:'walk_or_snap' to fallback to map-matching algorithm",
                    shape_match.as_str_name()
                ),
            )
        };
        let snap = |api: &mut Api, workspace: &mut MeiliWorkspace| match self.map_match(
            workspace,
            api,
            &prepared,
            &controller,
            now,
            interrupt,
        ) {
            Ok(_) => Ok(()),
            Err(MatchError::Valhalla(e)) => Err(e),
            Err(MatchError::Interrupted) => Err(ThorError::Interrupted.into()),
            Err(MatchError::Other(_)) => Err(ValhallaError::new(ErrorCode::NoPathFound)),
        };
        match shape_match {
            ShapeMatch::EdgeWalk => {
                match self.route_match(api, &prepared, &controller, now, interrupt) {
                    Ok(()) => Ok(()),
                    Err(ThorError::Interrupted) => Err(ThorError::Interrupted.into()),
                    Err(_) => Err(walk_failed_error()),
                }
            }
            ShapeMatch::MapSnap => snap(api, workspace),
            ShapeMatch::WalkOrSnap => {
                match self.route_match(api, &prepared, &controller, now, interrupt) {
                    Ok(()) => Ok(()),
                    Err(ThorError::Interrupted) => Err(ThorError::Interrupted.into()),
                    Err(_) => {
                        reset(api, &prepared.options);
                        snap(api, workspace)
                    }
                }
            }
        }
    }

    /// Matches a `trace_attributes` request that Loki has correlated and answers it
    /// (mainline's `thor_worker_t::trace_attributes`): the trip, a route per match
    /// (the best, then up to `alternates` more), with the attributes the request's strict
    /// filter allows, serialized by Tyr's `serialize_trace_attributes`.
    ///
    /// # Errors
    ///
    /// Error 443 if an `edge_walk` fails, 444 if map matching fails (including a trace
    /// without candidates, and fewer distinct matches than `alternates` asks for),
    /// 424 for a shape point with a negative accuracy or radius,
    /// 400 for an unknown `shape_match`, and 499 where mainline throws otherwise.
    pub fn trace_attributes(
        &self,
        workspace: &mut MeiliWorkspace,
        api: &mut Api,
    ) -> Result<Response, ValhallaError> {
        self.trace_attributes_interruptible(workspace, api, Timestamp::now(), &|| false)
    }

    /// [`Thor::trace_attributes`], with `now` as the current time, giving up when `interrupt`
    /// returns true (checked where Meili checks it, and before each leg is built).
    ///
    /// # Errors
    ///
    /// As for [`Thor::trace_attributes`]; an interrupt is error 499, which callers should
    /// drop rather than send.
    pub fn trace_attributes_interruptible(
        &self,
        workspace: &mut MeiliWorkspace,
        api: &mut Api,
        now: Timestamp,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<Response, ValhallaError> {
        let prepared = self.prepare_trace(api)?;
        let controller = AttributesController::new(&prepared.options, true);
        let shape_match = ShapeMatch::try_from(prepared.options.shape_match).map_err(|_| {
            ValhallaError::with_detail(ErrorCode::ThorUnknownAction, "Invalid shape_match value")
        })?;
        let name = shape_match.as_str_name();
        let snap = |api: &mut Api, workspace: &mut MeiliWorkspace| {
            match self.map_match(workspace, api, &prepared, &controller, now, interrupt) {
                Ok(results) => Ok(results),
                Err(MatchError::Interrupted) => Err(ThorError::Interrupted.into()),
                // `catch (const std::exception& e)`: every failure, mainline's own errors too.
                Err(MatchError::Valhalla(_) | MatchError::Other(_)) => {
                    Err(ValhallaError::with_detail(
                        ErrorCode::MapMatchFailed,
                        format!(
                            "{name} algorithm failed to snap the shape points to the correct shape."
                        ),
                    ))
                }
            }
        };
        let walked = || -> MapMatchResult { (1.0, 0.0, Vec::new()) };
        let results = match shape_match {
            ShapeMatch::EdgeWalk => {
                match self.route_match(api, &prepared, &controller, now, interrupt) {
                    Ok(()) => vec![walked()],
                    Err(ThorError::Interrupted) => return Err(ThorError::Interrupted.into()),
                    Err(_) => {
                        return Err(ValhallaError::with_detail(
                            ErrorCode::ExactRouteMatchFailed,
                            format!(
                                "{name} algorithm failed to find exact route match.  Try using \
                                 shape_match:'walk_or_snap' to fallback to map-matching algorithm"
                            ),
                        ));
                    }
                }
            }
            ShapeMatch::MapSnap => snap(api, workspace)?,
            ShapeMatch::WalkOrSnap => {
                match self.route_match(api, &prepared, &controller, now, interrupt) {
                    Ok(()) => vec![walked()],
                    Err(ThorError::Interrupted) => return Err(ThorError::Interrupted.into()),
                    Err(_) => {
                        reset(api, &prepared.options);
                        snap(api, workspace)?
                    }
                }
            }
        };
        serialize_trace_attributes(api, &controller, &results)
    }

    /// Builds one leg's `TripLeg` into a new route of the trip, checking the interrupt first
    /// (`TripLegBuilder::Build`).
    fn build_legs(
        &self,
        api: &mut Api,
        paths: &Paths,
        controller: &AttributesController,
        now: Timestamp,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<(), ThorError> {
        let options = api
            .options
            .as_ref()
            .ok_or(ThorError::Options("the request has no options".to_owned()))?;
        let trip = self.trip(options, paths, controller, now, interrupt)?;
        let routes = &mut api.trip.get_or_insert_with(Trip::default).routes;
        routes.extend(trip.routes);
        Ok(())
    }

    /// The edge walk and its trip (mainline's `route_match`): a route of one leg.
    fn route_match(
        &self,
        api: &mut Api,
        prepared: &Prepared,
        controller: &AttributesController,
        now: Timestamp,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<(), ThorError> {
        let options = api
            .options
            .as_mut()
            .ok_or(ThorError::Options("the request has no options".to_owned()))?;
        let use_timestamps = options.use_timestamps;
        let walk = |options: &mut Options, lenient| {
            route_matcher::form_path(
                self.provider(),
                &prepared.costing,
                options,
                use_timestamps,
                false,
                lenient,
                now,
            )
        };
        // Mainline's walk, and where it finds no path, the same walk with lenient shape checks
        // (see "Known divergences" in `docs/routing.md`).
        let mut path = walk(options, false)?;
        if path.is_none() && lenient_edge_walk() {
            path = walk(options, true)?;
        }
        let Some(path) = path else {
            return Err(walk_failed());
        };
        // The leg goes from the first shape point to the first break after it (or the last).
        let last = options.shape.len().saturating_sub(1);
        let destination = options
            .shape
            .iter()
            .enumerate()
            .skip(1)
            .find(|(i, l)| {
                *i == last
                    || matches!(
                        valhalla_proto::location::Type::try_from(l.r#type),
                        Ok(valhalla_proto::location::Type::KBreak
                            | valhalla_proto::location::Type::KBreakThrough)
                    )
            })
            .map(|(i, _)| i)
            .ok_or_else(walk_failed)?;
        let (Some(origin_location), Some(destination_location)) =
            (options.shape.first(), options.shape.get(destination))
        else {
            return Err(walk_failed());
        };
        let paths = Paths {
            routes: vec![Route {
                legs: vec![leg(
                    (0, origin_location),
                    (destination, destination_location),
                    path,
                    "edge_walk",
                    Vec::new(),
                )],
            }],
        };
        self.build_legs(api, &paths, controller, now, interrupt)
    }

    /// Map matching and its trip (mainline's `map_match`), returning each match's scores
    /// and results for `trace_attributes`.
    fn map_match(
        &self,
        workspace: &mut MeiliWorkspace,
        api: &mut Api,
        prepared: &Prepared,
        controller: &AttributesController,
        now: Timestamp,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<Vec<MapMatchResult>, MatchError> {
        if prepared.trace.is_empty() {
            return Ok(Vec::new());
        }
        let (action, format, alternates) = {
            let options = api
                .options
                .as_ref()
                .ok_or(ThorError::Options("the request has no options".to_owned()))?;
            let alternates = match options.has_alternates {
                Some(HasAlternates::Alternates(a)) => a,
                None => 0,
            };
            (options.action, options.format, alternates)
        };
        let is_trace_attributes = action == Action::TraceAttributes as i32;
        // Only trace_attributes has alternates; discontinuities give trace_route more routes.
        let topk = if is_trace_attributes {
            alternates.saturating_add(1)
        } else {
            1
        };
        let matches = prepared.matcher.offline_match(
            self.provider(),
            workspace,
            &prepared.trace,
            topk,
            interrupt,
        )?;

        let mut map_match_results: Vec<MapMatchResult> = Vec::with_capacity(matches.len());
        for result in matches {
            if result.segments.is_empty() {
                return Err(MatchError::Other(ThorError::Options(
                    "the match has no path".to_owned(),
                )));
            }
            let mut results = result.results;
            let options = api
                .options
                .as_mut()
                .ok_or(ThorError::Options("the request has no options".to_owned()))?;
            let paths = map_matcher::form_path(
                self.provider(),
                &prepared.costing,
                &results,
                &result.segments,
                options,
                now,
            )?;

            // OSRM responses list every matched point with its edge's names,
            // and its other candidates as empty edges.
            if action == Action::TraceRoute as i32 && format == Format::Osrm as i32 {
                self.correlate_matches(workspace, &results, options)?;
            }

            let trip_paths = if is_trace_attributes {
                build_trace(&paths, &result.segments, &mut results, options)?
            } else {
                build_route(&paths, &result.segments, &results, options)?
            };
            self.build_legs(api, &trip_paths, controller, now, interrupt)?;

            let confidence = map_match_results
                .first()
                .map_or(1.0, |first| first.1 / result.score);
            map_match_results.push((confidence, result.score, results));
        }
        Ok(map_match_results)
    }

    /// Gives each matched shape point its edge, with the edge's names, and as many empty
    /// edges as its candidate had other edges (for OSRM's `tracepoints`).
    fn correlate_matches(
        &self,
        workspace: &MeiliWorkspace,
        results: &[MatchResult],
        options: &mut Options,
    ) -> Result<(), ThorError> {
        for (i, m) in results.iter().enumerate() {
            let Some(edge_id) = m.edge_id else {
                continue;
            };
            let tile =
                graph::tile(self.provider(), edge_id)?.ok_or(ThorError::TileGone(edge_id))?;
            let edge = graph::edge(&tile, edge_id)?;
            let names = tile
                .get_edge_info(edge)?
                .names()
                .map(|n| n.map(|n| n.text_lossy().into_owned()))
                .collect::<Result<Vec<_>, _>>()?;
            let Some(location) = options.shape.get_mut(i) else {
                continue;
            };
            let correlation = location.correlation.get_or_insert_default();
            correlation.edges.push(CandidateEdge {
                ll: Some(lat_lng(m.lnglat)),
                names,
                ..CandidateEdge::default()
            });
            let Some(state) = m.state_id else {
                continue;
            };
            let others = workspace
                .candidate(state)
                .map_or(0, |c| c.edges.len().saturating_sub(1));
            correlation
                .edges
                .extend(core::iter::repeat_n(CandidateEdge::default(), others));
        }
        Ok(())
    }
}

#[cfg(test)]
std::thread_local! {
    /// Whether this thread's edge walks try again with lenient shape checks (the oracle tests
    /// turn it off to reproduce mainline's results exactly).
    pub(crate) static LENIENT_EDGE_WALK: core::cell::Cell<bool> =
        const { core::cell::Cell::new(true) };
}

/// Whether an edge walk mainline's walk fails on is walked again with lenient shape checks.
#[inline]
fn lenient_edge_walk() -> bool {
    #[cfg(test)]
    {
        LENIENT_EDGE_WALK.with(core::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        true
    }
}

/// Resets the request to its options before the edge walk (`request.Clear()`, then the options
/// copied back), so the walk leaves nothing behind for map matching: no trip, and no warnings.
fn reset(api: &mut Api, options: &Options) {
    *api = Api {
        options: Some(options.clone()),
        ..Api::default()
    };
}

/// One leg of every path of a match, for `trace_attributes` (mainline's `build_trace`):
/// each match result gets the index of its edge in the leg, discontinuities are trimmed at
/// their matched points, and the edges after one are disconnected.
#[expect(clippy::too_many_lines, reason = "mainline's function, step by step")]
fn build_trace(
    paths: &[MatchedPath],
    segments: &[EdgeSegment],
    results: &mut [MatchResult],
    options: &mut Options,
) -> Result<Paths, MatchError> {
    let (Some(first_path), Some(last_path)) = (paths.first(), paths.last()) else {
        return Err(MatchError::Valhalla(ValhallaError::new(
            ErrorCode::NoPathFound,
        )));
    };
    if first_path.segments.is_empty() {
        return Err(MatchError::Valhalla(ValhallaError::new(
            ErrorCode::NoPathFound,
        )));
    }
    let segment = |i: usize| {
        segments.get(i).ok_or(ThorError::Options(
            "a path's segment does not exist".to_owned(),
        ))
    };

    // The edge index of each trace point, across discontinuities.
    let mut edge_trimming: BTreeMap<usize, (EdgeTrimming, EdgeTrimming)> = BTreeMap::new();
    let mut last_id: Option<GraphId> = None;
    // `size_t edge_index = -1`, incremented before its first use.
    let mut edge_index = usize::MAX;
    let mut prev_path_had_discontinuity = false;
    for path in paths {
        let (Some(&first_segment), Some(&last_segment)) =
            (path.segments.first(), path.segments.last())
        else {
            return Err(ThorError::EmptyLeg.into());
        };
        let mut path_starts = true;
        for &s in &path.segments {
            let segment = segment(s)?;
            // A new edge, or the same edge visited again after a discontinuity.
            if last_id != Some(segment.edge_id) || (path_starts && prev_path_had_discontinuity) {
                edge_index = edge_index.wrapping_add(1);
            }
            let low = segment.first_match_idx.or(segment.last_match_idx);
            let high = segment.last_match_idx.or(segment.first_match_idx);
            if let (Some(low), Some(high)) = (low, high) {
                for m in results.iter_mut().take(high + 1).skip(low) {
                    if m.edge_id == Some(segment.edge_id) {
                        m.edge_index = Some(edge_index);
                    }
                }
            }
            last_id = Some(segment.edge_id);
            path_starts = false;
        }

        // The end of a discontinuity: trim at the first match.
        let first_match =
            results[match_index(segment(first_segment)?.first_match_idx, results.len())?];
        if (first_match.ends_discontinuity || prev_path_had_discontinuity)
            && let Some(index) = first_match.edge_index
        {
            let begin = trim_at(&first_match, first_match.distance_along);
            edge_trimming.insert(
                index,
                (
                    begin,
                    EdgeTrimming {
                        percent_along: 1.0,
                        ..NO_TRIM
                    },
                ),
            );
        }
        // The start of one: trim at the last match (perhaps on the edge where one just ended).
        let last = segment(last_segment)?;
        let last_match = results[match_index(last.last_match_idx, results.len())?];
        if (last_match.begins_discontinuity || last.discontinuity)
            && let Some(index) = last_match.edge_index
        {
            edge_trimming.entry(index).or_insert((NO_TRIM, NO_TRIM)).1 =
                trim_at(&last_match, last_match.distance_along);
        }
        prev_path_had_discontinuity = last.discontinuity;
    }

    // Every path's edges in one leg, disconnected after a discontinuity.
    let mut path_edges: Vec<PathEdge> = Vec::with_capacity(edge_index.wrapping_add(1));
    prev_path_had_discontinuity = false;
    for path in paths {
        let Some(first) = path.edges.first() else {
            return Err(ThorError::EmptyLeg.into());
        };
        // An edge visited again across a discontinuity stays a separate edge.
        let merge_last_edge = path_edges
            .last()
            .is_some_and(|e| e.edge_id == first.edge_id)
            && !prev_path_had_discontinuity;
        let first_inserted = path_edges.len();
        path_edges.extend(
            path.edges
                .iter()
                .skip(usize::from(merge_last_edge))
                .copied(),
        );
        if prev_path_had_discontinuity
            && !merge_last_edge
            && let Some(e) = path_edges.get_mut(first_inserted)
        {
            e.is_disconnected = true;
        }
        let last = path.segments.last().copied().map(segment).transpose()?;
        prev_path_had_discontinuity = last.is_some_and(|s| s.discontinuity);
    }

    // The origin and destination, as Loki would have correlated them:
    // the segments give the edge and percent along, and the matches the point.
    let origin_segment = segment(first_path.segments[0])?;
    let origin_index = match_index(origin_segment.first_match_idx, results.len())?;
    let dest_segment = segment(*last_path.segments.last().ok_or(ThorError::EmptyLeg)?)?;
    let dest_index = match_index(dest_segment.last_match_idx, results.len())?;
    let (origin_match, dest_match) = (results[origin_index], results[dest_index]);
    let shape_len = options.shape.len();
    let shape = &mut options.shape;
    let origin_location = shape
        .get_mut(origin_index)
        .ok_or(ThorError::NoCandidates(origin_index))?;
    add_path_edge(
        origin_location,
        origin_segment.edge_id,
        origin_segment.source,
        origin_match.lnglat,
        origin_match.distance_from,
    );
    let dest_location = shape
        .get_mut(dest_index)
        .ok_or(ThorError::NoCandidates(shape_len))?;
    add_path_edge(
        dest_location,
        dest_segment.edge_id,
        dest_segment.target,
        dest_match.lnglat,
        dest_match.distance_from,
    );

    Ok(Paths {
        routes: vec![Route {
            legs: vec![leg(
                (origin_index, &shape[origin_index]),
                (dest_index, &shape[dest_index]),
                path_edges,
                "map_snap",
                trimming_of(edge_trimming),
            )],
        }],
    })
}

/// The routes of a match, for `trace_route` (mainline's `build_route`): a leg per path, and a
/// route per discontinuity, with each trace point's route and waypoint index for OSRM,
/// and the shape trimmed where the path turns around on an edge.
#[expect(clippy::too_many_lines, reason = "mainline's function, step by step")]
fn build_route(
    paths: &[MatchedPath],
    segments: &[EdgeSegment],
    results: &[MatchResult],
    options: &mut Options,
) -> Result<Paths, MatchError> {
    let segment = |i: usize| {
        segments.get(i).ok_or(ThorError::Options(
            "a path's segment does not exist".to_owned(),
        ))
    };
    let mut routes: Vec<Route> = Vec::new();
    let mut route: Option<Route> = None;
    let mut way_point_index: u32 = 0;
    let mut route_index: u32 = 0;
    for path in paths {
        let current = route.get_or_insert_with(|| {
            way_point_index = 0;
            Route::default()
        });
        let (Some(&first), Some(&last)) = (path.segments.first(), path.segments.last()) else {
            return Err(ThorError::EmptyLeg.into());
        };
        let origin_segment = segment(first)?;
        let dest_segment = segment(last)?;
        let origin_index = match_index(origin_segment.first_match_idx, results.len())?;
        let dest_index = match_index(dest_segment.last_match_idx, results.len())?;

        for location in options
            .shape
            .iter_mut()
            .take(dest_index + 1)
            .skip(origin_index)
        {
            let correlation = location.correlation.get_or_insert_default();
            correlation.route_index = route_index;
            correlation.waypoint_index = u32::MAX;
        }
        // OSRM needs both the matching index (the route index) and the waypoint index.
        let shape_len = options.shape.len();
        let origin_location = options
            .shape
            .get_mut(origin_index)
            .ok_or(ThorError::NoCandidates(shape_len))?;
        origin_location
            .correlation
            .get_or_insert_default()
            .waypoint_index = way_point_index;
        let (origin_match, dest_match) = (results[origin_index], results[dest_index]);
        add_path_edge(
            origin_location,
            origin_segment.edge_id,
            origin_segment.source,
            origin_match.lnglat,
            origin_match.distance_from,
        );
        way_point_index += 1;
        let dest_location = options
            .shape
            .get_mut(dest_index)
            .ok_or(ThorError::NoCandidates(shape_len))?;
        dest_location
            .correlation
            .get_or_insert_default()
            .waypoint_index = way_point_index;
        add_path_edge(
            dest_location,
            dest_segment.edge_id,
            dest_segment.target,
            dest_match.lnglat,
            dest_match.distance_from,
        );

        // Trim where the path turns around on an edge, keyed by the segment's index in the path
        // (one path edge per segment).
        let mut edge_trimming: BTreeMap<usize, (EdgeTrimming, EdgeTrimming)> = BTreeMap::new();
        for (i, &s) in path.segments.iter().enumerate() {
            let segment = segment(s)?;
            let prev = i
                .checked_sub(1)
                .and_then(|p| path.segments.get(p))
                .map(|&p| segment_of(segments, p))
                .transpose()?;
            let next = path
                .segments
                .get(i + 1)
                .map(|&n| segment_of(segments, n))
                .transpose()?;
            // Onto this edge after a U-turn: trim the beginning.
            if let Some(prev) = prev
                && prev.edge_id != segment.edge_id
                && prev.target < 1.0
                && let Some(m) = segment.first_match_idx.and_then(|f| results.get(f))
            {
                edge_trimming.entry(i).or_insert((NO_TRIM, NO_TRIM)).0 = trim_at(m, segment.source);
            }
            // Off this edge with a U-turn: trim the end.
            if let Some(next) = next
                && segment.edge_id != next.edge_id
                && segment.target < 1.0
                && let Some(m) = segment.last_match_idx.and_then(|l| results.get(l))
            {
                edge_trimming.entry(i).or_insert((NO_TRIM, NO_TRIM)).1 = trim_at(m, segment.target);
            }
        }

        current.legs.push(leg(
            (origin_index, &options.shape[origin_index]),
            (dest_index, &options.shape[dest_index]),
            path.edges.clone(),
            "map_snap",
            trimming_of(edge_trimming),
        ));

        if dest_segment.discontinuity {
            route_index += 1;
            if let Some(done) = route.take() {
                routes.push(done);
            }
        }
    }
    if let Some(done) = route {
        routes.push(done);
    }
    Ok(Paths { routes })
}

/// A segment by index.
fn segment_of(segments: &[EdgeSegment], i: usize) -> Result<&EdgeSegment, ThorError> {
    segments.get(i).ok_or(ThorError::Options(
        "a path's segment does not exist".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;
    use std::path::PathBuf;

    use valhalla_graphtile::graph_tile::GraphTileView;
    use valhalla_graphtile::tile_provider::{DirectoryGraphTileProvider, GraphTileProviderError};

    use super::*;
    use crate::trace_oracle_tests::{andorra_tiles, message, oracle_config};

    /// A fixture case's Loki→Thor message.
    fn input(case: &str) -> Api {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/traces/andorra")
            .join(case);
        message(&dir, "loki-thor").expect("the case")
    }

    /// The tile set without one tile, as a regional extract without its neighbor.
    struct WithoutTile {
        tiles: DirectoryGraphTileProvider,
        missing: GraphId,
    }

    impl WithoutTile {
        fn check(&self, id: GraphId) -> Result<(), GraphTileProviderError> {
            if id.tile_base_id() == self.missing {
                return Err(GraphTileProviderError::TileNotAvailable(self.missing));
            }
            Ok(())
        }
    }

    impl GraphTileProvider for WithoutTile {
        type TileHandle = <DirectoryGraphTileProvider as GraphTileProvider>::TileHandle;

        fn with_tile_containing<F, T>(
            &self,
            graph_id: GraphId,
            process: F,
        ) -> Result<T, GraphTileProviderError>
        where
            F: FnOnce(&GraphTileView) -> T,
        {
            self.check(graph_id)?;
            self.tiles.with_tile_containing(graph_id, process)
        }

        fn get_handle_for_tile_containing(
            &self,
            graph_id: GraphId,
        ) -> Result<Self::TileHandle, GraphTileProviderError> {
            self.check(graph_id)?;
            self.tiles.get_handle_for_tile_containing(graph_id)
        }

        fn contains_tile(&self, graph_id: GraphId) -> Result<bool, GraphTileProviderError> {
            if graph_id.tile_base_id() == self.missing {
                return Ok(false);
            }
            self.tiles.contains_tile(graph_id)
        }
    }

    /// The fixture tiles (level and tile ID).
    const ANDORRA_TILES: [(u8, u64); 7] = [
        (0, 3015),
        (1, 47_701),
        (2, 762_485),
        (2, 762_486),
        (2, 763_925),
        (2, 763_926),
        (2, 763_927),
    ];

    /// The tiles of the trip's edges.
    fn tiles_of(api: &Api) -> Vec<GraphId> {
        let mut tiles: Vec<GraphId> = api
            .trip
            .iter()
            .flat_map(|t| &t.routes)
            .flat_map(|r| &r.legs)
            .flat_map(|l| &l.node)
            .filter_map(|n| n.edge.as_ref())
            .filter_map(|e| GraphId::try_from_id(e.id).ok())
            .map(|id| id.tile_base_id())
            .collect();
        tiles.sort_by_key(GraphId::value);
        tiles.dedup();
        tiles
    }

    /// An interrupt that asks to stop on its `n`th check, counting the checks.
    struct Countdown {
        calls: Cell<usize>,
        stop_at: usize,
    }

    impl Countdown {
        fn new(stop_at: usize) -> Self {
            Self {
                calls: Cell::new(0),
                stop_at,
            }
        }

        fn check(&self) -> bool {
            self.calls.set(self.calls.get() + 1);
            self.calls.get() >= self.stop_at
        }
    }

    fn interrupted(error: &ValhallaError) -> bool {
        error.code() == ErrorCode::ThorUnknown
            && error.detail() == Some(ThorError::Interrupted.to_string().as_str())
    }

    /// Interrupts stop map matching (in Meili's search, and before each leg is built)
    /// and the edge walk's trip, and are reported as such, never as a failed match
    /// (mainline sends nothing for an interrupted request).
    #[test]
    fn interrupts_map_matching_and_trip_building() {
        let thor = Thor::new(oracle_config(), andorra_tiles());
        let mut workspace = MeiliWorkspace::new();
        for case in [
            "andorra-0000-clean",
            "andorra-0016-stationary",
            "andorra-0041-walk_or_snap_exact",
            "andorra-0086-edge_walk_exact",
        ] {
            let request = input(case);
            let is_route =
                request.options.as_ref().map(|o| o.action) == Some(Action::TraceRoute as i32);
            let run = |interrupt: &dyn Fn() -> bool, workspace: &mut MeiliWorkspace| {
                let mut api = request.clone();
                if is_route {
                    thor.trace_route_interruptible(
                        workspace,
                        &mut api,
                        Timestamp::UNIX_EPOCH,
                        interrupt,
                    )
                } else {
                    thor.trace_attributes_interruptible(
                        workspace,
                        &mut api,
                        Timestamp::UNIX_EPOCH,
                        interrupt,
                    )
                    .map(|_| ())
                }
            };
            // How many checks an uninterrupted request makes.
            let counter = Countdown::new(usize::MAX);
            run(&|| counter.check(), &mut workspace).expect("the case succeeds");
            let checks = counter.calls.get();
            assert!(checks > 0, "{case}");
            // Stopping at any of them is an interrupt.
            for stop_at in 1..=checks {
                let countdown = Countdown::new(stop_at);
                let error =
                    run(&|| countdown.check(), &mut workspace).expect_err("an interrupted request");
                assert!(interrupted(&error), "{case} at check {stop_at}: {error:?}");
            }
        }
    }

    /// Runs a case's action.
    fn run_case<P: GraphTileProvider>(
        thor: &Thor<P>,
        workspace: &mut MeiliWorkspace,
        request: &Api,
    ) -> (Api, Result<(), ValhallaError>) {
        let mut api = request.clone();
        let is_route =
            request.options.as_ref().map(|o| o.action) == Some(Action::TraceRoute as i32);
        let result = if is_route {
            thor.trace_route(workspace, &mut api)
        } else {
            thor.trace_attributes(workspace, &mut api).map(|_| ())
        };
        (api, result)
    }

    /// A tile that is missing (as at the edge of a regional extract) is the edge of the graph
    /// for matching, and where trip building needs it, the match fails as mainline's would
    /// (442 for `trace_route`, 444 for `trace_attributes`), never with a panic or an internal error.
    #[test]
    fn matches_without_tiles() {
        let mut workspace = MeiliWorkspace::new();
        let full = Thor::new(oracle_config(), andorra_tiles());
        let (mut tested, mut failed) = (0, 0);
        for case in [
            "andorra-0000-clean",
            "andorra-0022-loop",
            "andorra-0033-clean",
            "andorra-0131-gap",
            "andorra-0086-edge_walk_exact",
            "andorra-0041-walk_or_snap_exact",
        ] {
            let request = input(case);
            let (api, result) = run_case(&full, &mut workspace, &request);
            result.expect("the case succeeds with every tile");
            // trace_attributes leaves the trip on the request too.
            assert!(!tiles_of(&api).is_empty(), "{case}");
            for missing in ANDORRA_TILES.map(|(level, tile)| {
                GraphId::try_from_components(level, tile, 0).expect("a tile ID")
            }) {
                let thor = Thor::new(
                    oracle_config(),
                    WithoutTile {
                        tiles: andorra_tiles(),
                        missing,
                    },
                );
                // The candidate grids are of one tile set.
                let mut workspace = MeiliWorkspace::new();
                let (_, result) = run_case(&thor, &mut workspace, &request);
                failed += usize::from(result.is_err());
                if let Err(error) = result {
                    let code = error.code();
                    assert!(
                        matches!(
                            code,
                            ErrorCode::NoPathFound
                                | ErrorCode::ExactRouteMatchFailed
                                | ErrorCode::MapMatchFailed
                        ),
                        "{case} without {missing}: {error:?}"
                    );
                }
                tested += 1;
            }
        }
        assert_eq!(tested, 6 * ANDORRA_TILES.len());
        // Some cases still match without a tile, and some can't.
        assert!(failed > 0 && failed < tested, "{failed} of {tested} failed");
    }
}
