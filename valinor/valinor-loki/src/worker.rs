//! Loki's actions (mainline's `loki_worker_t`, `src/loki/*_action.cc` and `worker.cc`).
//!
//! Each action checks the request against the service limits,
//! fills in the per-location defaults, builds the costing, turns exclude locations and
//! polygons into excluded edges, and correlates the locations, exactly as mainline does
//! before it hands the request to Thor (or, for `/locate`, answers it itself).
//!
//! | Action | Mainline | Here |
//! |---|---|---|
//! | `route`, `centroid` | `route` | [`Loki::route`] |
//! | `locate` | `locate` | [`Loki::locate`] |
//! | `sources_to_targets`, `optimized_route` | `matrix` | [`Loki::matrix`] |
//! | `trace_route`, `trace_attributes` | `trace` | [`Loki::trace`] |
//! | `status` | `status` | [`Loki::status`] |
//!
//! Every action records its latency as a statistic (`<action>.info.loki.latency_ms`),
//! even when it fails, as mainline's `measure_scope_time` does.
//!
//! # What isn't here
//!
//! - Isochrones, expansion, heights, transit availability, and tiles (error 107),
//!   and verbose status where the server allows it (it needs the tile set's outline).
//! - Costings Sif doesn't have yet, including the transit and combined ones
//!   (`multimodal`, `transit`, `bikeshare`, and `auto_pedestrian`): error 107.
//! - Linear cost features (`cost_factor_lines`): error 107.
//!
//! # Quirks reproduced
//!
//! - When the server doesn't allow hard exclusions, mainline warns (208) that it ignores
//!   them, but it clears them on a copy of the options, so they still apply.
//! - Too many matrix pairs (150) reports the limit as a float (`2500.000000`).
//! - A matrix pair farther apart than `max_timedep_distance_matrix` (0 by default, so any pair
//!   that isn't at the same spot) loses its `date_time`, and only a nonzero limit warns (200).
//! - Failures while correlating exclude locations are ignored, keeping what was found.

use crate::config::LokiConfig;
use crate::connectivity::ConnectivityMap;
use crate::graph::GraphError;
use crate::polygon::{PolygonError, edges_in_rings};
use crate::search::{MAX_LEVEL, SearchError, lat_lng, point_of, search};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Instant;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_reader::{GraphReadError, GraphReader, edge as get_edge, shape};
use valhalla_graphtile::graph_tile::GraphTile;
use valhalla_graphtile::midgard::PointLL;
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::avoid_edge::{HasId, HasPercentAlong};
use valhalla_proto::costing::{HasOptions, Type as CostingType};
use valhalla_proto::location::{
    HasHeadingTolerance, HasMinimumInboundReachability, HasMinimumOutboundReachability,
    HasMinimumReachability, HasNodeSnapTolerance, HasRadius, HasSearchCutoff, HasStreetSideCutoff,
    HasStreetSideMaxDistance, HasStreetSideTolerance, SideOfStreet,
};
use valhalla_proto::options::{
    Action, HasAlternates, HasBreakageDistance, HasGpsAccuracy, HasVerbose,
};
use valhalla_proto::options::{HasSearchRadius, HasTurnPenaltyFactor};
use valhalla_proto::search_filter::{
    HasExcludeClosures, HasLevel, HasMaxRoadClass, HasMinRoadClass,
};
use valhalla_proto::{
    Api, AvoidEdge, CodedDescription, Costing as CostingProto, Info, Location, Options, RoadClass,
    SearchFilter, ShapeMatch, Statistic, StatisticType,
};
use valhalla_response::{ErrorCode, ValhallaError};
use valinor_sif::costing_options::costing_name;
use valinor_sif::{AnyCosting, CostingError};
use valinor_tyr::tyr::serialize_locate;

/// The default search cutoff for a location with a level filter (`kDefaultIndoorSearchCutoff`).
const DEFAULT_INDOOR_SEARCH_CUTOFF: u32 = 300;
/// The largest search cutoff for a location with a level filter (`kMaxIndoorSearchCutoff`).
const MAX_INDOOR_SEARCH_CUTOFF: u32 = 1000;

/// Mainline's warning messages for the codes Loki adds (`warning_codes`).
fn warning_message(code: u64) -> Option<&'static str> {
    Some(match code {
        200 => {
            "path distance exceeds the max distance limit for time-dependent matrix, ignoring date_time"
        }
        205 => {
            r#""disable_hierarchy_pruning" exceeded the max distance, ignoring disable_hierarchy_pruning"#
        }
        208 => "Hard exclusions are not allowed on this server, ignoring hard excludes",
        302 => {
            r#""search_filter.level" was specified without a custom "search_cutoff", setting default default cutoff to "#
        }
        303 => {
            r#""search_cutoff" exceeds maximum allowed value due to "search_filter.level" being specified, clamping cutoff to "#
        }
        _ => return None,
    })
}

/// Adds a warning to the request (`add_warning`).
fn add_warning(api: &mut Api, code: u64, extra: &str) {
    if let Some(message) = warning_message(code) {
        api.info
            .get_or_insert_with(Info::default)
            .warnings
            .push(CodedDescription {
                description: format!("{message}{extra}"),
                code,
            });
    }
}

/// What Loki produced for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LokiOutput {
    /// The request goes on to Thor.
    Forward,
    /// Loki answers the request itself (`/locate`), with this JSON body.
    Respond(String),
}

/// Loki: a configuration, a tile set, and (optionally) its connectivity.
///
/// It holds nothing per request, so one can serve many threads at once
/// (given a thread-safe provider, such as
/// [`CachingTileProvider`](valhalla_graphtile::tile_provider::CachingTileProvider)).
pub struct Loki<P> {
    config: LokiConfig,
    provider: P,
    /// Replaced whole when tiles arrive ([`Loki::replace_connectivity`]),
    /// so a request reads one map throughout.
    connectivity: RwLock<Option<Arc<ConnectivityMap>>>,
    tileset_last_modified: u32,
}

/// The ll of a location as a point (`to_ll`).
fn to_ll(location: &Location) -> PointLL {
    point_of(location.ll.as_ref())
}

/// Fills in Loki's per-location defaults (`parse_location`).
///
/// Returns whether the search cutoff was defaulted (warning 302) or clamped (warning 303)
/// because of a level filter.
#[expect(
    clippy::too_many_lines,
    reason = "one step per default, as mainline sets them"
)]
pub fn parse_location(location: &mut Location, config: &LokiConfig) -> (bool, bool) {
    let defaults = &config.service_defaults;
    let limits = &config.service_limits;
    let mut modified = (false, false);

    // `minimum_reachability`, or its directed variants, capped; or the default.
    let has_reachability = location.has_minimum_reachability.is_some()
        || location.has_minimum_inbound_reachability.is_some()
        || location.has_minimum_outbound_reachability.is_some();
    let undirected = location
        .has_minimum_reachability
        .map(|HasMinimumReachability::MinimumReachability(r)| r);
    let inbound = location.has_minimum_inbound_reachability.map_or(
        0,
        |HasMinimumInboundReachability::MinimumInboundReachability(r)| r,
    );
    let outbound = location.has_minimum_outbound_reachability.map_or(
        0,
        |HasMinimumOutboundReachability::MinimumOutboundReachability(r)| r,
    );
    let (inbound, outbound) = if has_reachability {
        (
            undirected.unwrap_or(inbound).min(limits.max_reachability),
            undirected.unwrap_or(outbound).min(limits.max_reachability),
        )
    } else {
        (defaults.minimum_reachability, defaults.minimum_reachability)
    };
    location.has_minimum_inbound_reachability = Some(
        HasMinimumInboundReachability::MinimumInboundReachability(inbound),
    );
    location.has_minimum_outbound_reachability = Some(
        HasMinimumOutboundReachability::MinimumOutboundReachability(outbound),
    );

    match location.has_radius {
        None => location.has_radius = Some(HasRadius::Radius(defaults.radius)),
        Some(HasRadius::Radius(r)) if r > limits.max_radius => {
            location.has_radius = Some(HasRadius::Radius(limits.max_radius));
        }
        Some(_) => {}
    }
    if location.has_heading_tolerance.is_none() {
        location.has_heading_tolerance = Some(HasHeadingTolerance::HeadingTolerance(
            defaults.heading_tolerance,
        ));
    }
    if location.has_node_snap_tolerance.is_none() {
        location.has_node_snap_tolerance = Some(HasNodeSnapTolerance::NodeSnapTolerance(
            defaults.node_snap_tolerance,
        ));
    }

    let has_level = location.search_filter.as_ref().is_some_and(|f| {
        matches!(f.has_level, Some(HasLevel::Level(level)) if level != MAX_LEVEL)
            // An unset level reads as 0, which isn't "no level".
            || f.has_level.is_none()
    });
    match (location.has_search_cutoff, has_level) {
        (None, false) => {
            location.has_search_cutoff =
                Some(HasSearchCutoff::SearchCutoff(defaults.search_cutoff));
        }
        (Some(HasSearchCutoff::SearchCutoff(cutoff)), true) => {
            if cutoff > MAX_INDOOR_SEARCH_CUTOFF {
                modified.1 = true;
                location.has_search_cutoff =
                    Some(HasSearchCutoff::SearchCutoff(MAX_INDOOR_SEARCH_CUTOFF));
            }
        }
        (None, true) => {
            location.has_search_cutoff =
                Some(HasSearchCutoff::SearchCutoff(DEFAULT_INDOOR_SEARCH_CUTOFF));
            modified.0 = true;
        }
        (Some(_), false) => {}
    }

    if location.has_street_side_tolerance.is_none() {
        location.has_street_side_tolerance = Some(HasStreetSideTolerance::StreetSideTolerance(
            defaults.street_side_tolerance,
        ));
    }
    if location.has_street_side_cutoff.is_none() {
        location.has_street_side_cutoff = Some(HasStreetSideCutoff::StreetSideCutoff(
            RoadClass::KServiceOther.into(),
        ));
    }
    if location.has_street_side_max_distance.is_none() {
        location.has_street_side_max_distance = Some(
            HasStreetSideMaxDistance::StreetSideMaxDistance(defaults.street_side_max_distance),
        );
    }

    let filter = location
        .search_filter
        .get_or_insert_with(SearchFilter::default);
    if filter.has_min_road_class.is_none() {
        filter.has_min_road_class = Some(HasMinRoadClass::MinRoadClass(
            RoadClass::KServiceOther.into(),
        ));
    }
    if filter.has_max_road_class.is_none() {
        filter.has_max_road_class =
            Some(HasMaxRoadClass::MaxRoadClass(RoadClass::KMotorway.into()));
    }
    if filter.has_exclude_closures.is_none() {
        filter.has_exclude_closures = Some(HasExcludeClosures::ExcludeClosures(true));
    }
    if filter.has_level.is_none() {
        filter.has_level = Some(HasLevel::Level(MAX_LEVEL));
    }
    modified
}

/// Sets the trace defaults on a location (`apply_trace_location_defaults`).
fn apply_trace_location_defaults(location: &mut Location) {
    location.has_node_snap_tolerance = Some(HasNodeSnapTolerance::NodeSnapTolerance(0));
    location.has_radius = Some(HasRadius::Radius(10));
    location.has_minimum_reachability = Some(HasMinimumReachability::MinimumReachability(0));
}

/// Which locations of the options an action reads.
#[derive(Clone, Copy, Debug)]
enum Which {
    Locations,
    Sources,
    Targets,
    ExcludeLocations,
}

fn locations_mut(options: &mut Options, which: Which) -> &mut Vec<Location> {
    match which {
        Which::Locations => &mut options.locations,
        Which::Sources => &mut options.sources,
        Which::Targets => &mut options.targets,
        Which::ExcludeLocations => &mut options.exclude_locations,
    }
}

/// The options of the request's costing.
fn costing_options_mut(options: &mut Options) -> Option<&mut valhalla_proto::costing::Options> {
    let costing = options.costings.get_mut(&options.costing_type)?;
    match costing
        .has_options
        .get_or_insert_with(|| HasOptions::Options(valhalla_proto::costing::Options::default()))
    {
        HasOptions::Options(o) => Some(o),
    }
}

fn costing_options(options: &Options) -> Option<&valhalla_proto::costing::Options> {
    match options
        .costings
        .get(&options.costing_type)?
        .has_options
        .as_ref()?
    {
        HasOptions::Options(o) => Some(o),
    }
}

/// An internal failure, reported as mainline reports exceptions that aren't its own:
/// in the search of most actions as 171, elsewhere as an unknown error (199).
fn unknown(error: &impl core::fmt::Display) -> ValhallaError {
    ValhallaError::with_detail(ErrorCode::LokiUnknown, error.to_string())
}

impl<P: GraphTileProvider> Loki<P> {
    /// A Loki over `provider`.
    ///
    /// With `config.use_connectivity`, pass the tile set's [`ConnectivityMap`]
    /// to reject routes between unconnected regions (error 170), as mainline does;
    /// without one, that check is skipped.
    pub fn new(config: LokiConfig, provider: P, connectivity: Option<ConnectivityMap>) -> Self {
        let connectivity = if config.use_connectivity {
            connectivity.map(Arc::new)
        } else {
            None
        };
        Self {
            config,
            provider,
            connectivity: RwLock::new(connectivity),
            tileset_last_modified: 0,
        }
    }

    /// Replaces the connectivity map (for a tile set that has gained tiles),
    /// if this Loki checks connectivity (`config.use_connectivity`).
    ///
    /// It is safe while requests run: each request reads the map it started with,
    /// or the new one.
    pub fn replace_connectivity(&self, connectivity: ConnectivityMap) {
        if self.config.use_connectivity {
            *self
                .connectivity
                .write()
                .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(connectivity));
        }
    }

    /// Sets when the tile set was last modified, in seconds since the Unix epoch,
    /// for `/status` (mainline reads the modification time of the tile extract or directory).
    #[must_use]
    pub fn with_tileset_last_modified(mut self, seconds: u32) -> Self {
        self.tileset_last_modified = seconds;
        self
    }

    /// The configuration.
    #[inline]
    pub fn config(&self) -> &LokiConfig {
        &self.config
    }

    /// The tile provider.
    #[inline]
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// Runs the request's action, after checking that this Loki offers it (error 106).
    ///
    /// # Errors
    ///
    /// Mainline's errors for the request; see the actions.
    pub fn handle(&self, api: &mut Api) -> Result<LokiOutput, ValhallaError> {
        let action = api.options.as_ref().map_or(0, |o| o.action);
        let action = Action::try_from(action)
            .ok()
            .filter(|a| self.config.actions.contains(a));
        let Some(action) = action else {
            return Err(ValhallaError::with_detail(
                ErrorCode::UnknownAction,
                self.config.action_list(),
            ));
        };
        match action {
            Action::Route | Action::Centroid => self.route(api).map(|()| LokiOutput::Forward),
            Action::Locate => self.locate(api).map(LokiOutput::Respond),
            Action::SourcesToTargets | Action::OptimizedRoute => {
                self.matrix(api).map(|()| LokiOutput::Forward)
            }
            Action::TraceRoute | Action::TraceAttributes => {
                self.trace(api).map(|()| LokiOutput::Forward)
            }
            Action::Status => self.status(api).map(|()| LokiOutput::Forward),
            _ => Err(ErrorCode::NotImplemented.into()),
        }
    }

    /// Records the action's latency (`measure_scope_time`), whatever the outcome.
    fn timed<T>(
        api: &mut Api,
        action: impl FnOnce(&mut Api) -> Result<T, ValhallaError>,
    ) -> Result<T, ValhallaError> {
        let start = Instant::now();
        let result = action(api);
        let name = api
            .options
            .as_ref()
            .and_then(|o| Action::try_from(o.action).ok())
            .map_or("no_action", |a| a.as_str_name());
        api.info
            .get_or_insert_with(Info::default)
            .statistics
            .push(Statistic {
                key: format!("{name}.info.loki.latency_ms"),
                value: start.elapsed().as_secs_f64() * 1000.0,
                frequency: 0.0,
                r#type: StatisticType::Timing.into(),
            });
        result
    }

    /// Fills in the defaults of a set of locations, with warnings 302 and 303
    /// (`parse_locations`); an empty set fails with `required`, if given.
    fn parse_locations(
        &self,
        api: &mut Api,
        which: Which,
        required: Option<ErrorCode>,
    ) -> Result<(), ValhallaError> {
        let options = api.options.get_or_insert_with(Options::default);
        let locations = locations_mut(options, which);
        if locations.is_empty() {
            return match required {
                Some(code) => Err(code.into()),
                None => Ok(()),
            };
        }
        // Mainline keeps only the last location's flags.
        let mut flags = (false, false);
        for location in locations.iter_mut() {
            flags = parse_location(location, &self.config);
        }
        if flags.0 {
            add_warning(api, 302, &DEFAULT_INDOOR_SEARCH_CUTOFF.to_string());
        }
        if flags.1 {
            add_warning(api, 303, &MAX_INDOOR_SEARCH_CUTOFF.to_string());
        }
        Ok(())
    }

    /// Builds the costing, and turns exclude polygons and locations into excluded edges
    /// (`parse_costing`).
    #[expect(
        clippy::too_many_lines,
        reason = "a step by step port of mainline's function"
    )]
    fn parse_costing(&self, api: &mut Api, allow_none: bool) -> Result<AnyCosting, ValhallaError> {
        let options = api.options.get_or_insert_with(Options::default);
        let costing_type = CostingType::try_from(options.costing_type).ok();
        if !allow_none && costing_type == Some(CostingType::None) {
            return Err(ErrorCode::CostingRequired.into());
        }

        if !self.config.service_limits.allow_hard_exclusions {
            let detected = options.costings.values().any(|c| match &c.has_options {
                Some(HasOptions::Options(o)) => {
                    o.exclude_bridges
                        || o.exclude_tolls
                        || o.exclude_tunnels
                        || o.exclude_highways
                        || o.exclude_ferries
                }
                None => false,
            });
            // Mainline clears the exclusions on a copy, so they stay in effect.
            if detected {
                add_warning(api, 208, "");
            }
        }

        let options = api.options.get_or_insert_with(Options::default);
        if !options.cost_factor_lines.is_empty() {
            return Err(ValhallaError::with_detail(
                ErrorCode::NotImplemented,
                "cost_factor_lines",
            ));
        }
        let costing = match costing_type {
            Some(
                CostingType::Multimodal
                | CostingType::Transit
                | CostingType::Bikeshare
                | CostingType::AutoPedestrian,
            )
            | None => return Err(ErrorCode::NotImplemented.into()),
            Some(t) => {
                let proto = options
                    .costings
                    .get(&options.costing_type)
                    .cloned()
                    .unwrap_or(CostingProto {
                        r#type: t.into(),
                        ..CostingProto::default()
                    });
                AnyCosting::new(&proto).map_err(|e| match e {
                    CostingError::NotImplemented(_) => ErrorCode::NotImplemented.into(),
                    _ => ValhallaError::with_detail(
                        ErrorCode::UnknownCosting,
                        format!("'{}'", costing_name(t)),
                    ),
                })?
            }
        };

        let graph = GraphReader::new(&self.provider);
        if !options.exclude_polygons.is_empty() {
            let limits = &self.config.service_limits;
            let edges = edges_in_rings(
                options,
                graph,
                &costing,
                limits.max_exclude_polygons_length,
                limits.max_exclude_polygons_vertices,
            )
            .map_err(|e| match e {
                PolygonError::Limit(e) => e,
                PolygonError::Graph(e) => unknown(&e),
            })?;
            if let Some(co) = costing_options_mut(options) {
                co.exclude_edges
                    .extend(edges.into_iter().map(|id| AvoidEdge {
                        has_id: Some(HasId::Id(id)),
                        has_percent_along: Some(HasPercentAlong::PercentAlong(0.0)),
                    }));
            }
        }

        if !options.exclude_locations.is_empty() {
            let max = self.config.service_limits.max_exclude_locations;
            if options.exclude_locations.len() > max {
                return Err(ValhallaError::with_detail(
                    ErrorCode::TooManyAvoidLocations,
                    max.to_string(),
                ));
            }
            // Mainline swallows every failure here, keeping what it found.
            if self
                .parse_locations(api, Which::ExcludeLocations, None)
                .is_ok()
            {
                let options = api.options.get_or_insert_with(Options::default);
                if search(&mut options.exclude_locations, &self.provider, &costing).is_ok() {
                    let _ = self.add_exclude_edges(options);
                }
            }
        }

        // Cap the alternates.
        let options = api.options.get_or_insert_with(Options::default);
        let alternates = match options.has_alternates {
            Some(HasAlternates::Alternates(a)) => a,
            None => 0,
        };
        let limits = &self.config.service_limits;
        let max = if options.action == i32::from(Action::TraceAttributes) {
            limits.trace.max_alternates
        } else {
            limits.max_alternates
        };
        if alternates > max {
            options.has_alternates = Some(HasAlternates::Alternates(max));
        }
        Ok(costing)
    }

    /// Adds the correlated exclude locations' edges (and their shortcuts) to the costing's
    /// excluded edges.
    fn add_exclude_edges(&self, options: &mut Options) -> Result<(), GraphError> {
        let mut avoids = std::collections::HashSet::new();
        let mut added = Vec::new();
        for location in &options.exclude_locations {
            let Some(correlation) = &location.correlation else {
                continue;
            };
            for edge in &correlation.edges {
                if !avoids.insert(edge.graph_id) {
                    continue;
                }
                added.push((edge.graph_id, edge.percent_along));
                let Ok(id) = GraphId::try_from_id(edge.graph_id) else {
                    continue;
                };
                let shortcut = match self.provider.get_shortcut(id) {
                    Ok(shortcut) => shortcut,
                    Err(e) => {
                        // Keep what was found so far, as mainline does when it throws.
                        Self::push_excludes(options, added);
                        return Err(e.into());
                    }
                };
                if let Some(shortcut) = shortcut
                    && avoids.insert(shortcut.value())
                {
                    added.push((shortcut.value(), 0.0));
                }
            }
        }
        Self::push_excludes(options, added);
        Ok(())
    }

    fn push_excludes(options: &mut Options, added: Vec<(u64, f64)>) {
        if let Some(co) = costing_options_mut(options) {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the protobuf's percent along is a float"
            )]
            co.exclude_edges
                .extend(added.into_iter().map(|(id, percent)| AvoidEdge {
                    has_id: Some(HasId::Id(id)),
                    has_percent_along: Some(HasPercentAlong::PercentAlong(percent as f32)),
                }));
        }
    }

    /// Turns off `disable_hierarchy_pruning` (warning 205) if the locations are too far apart
    /// (`check_hierarchy_distance`).
    fn check_hierarchy_distance(&self, api: &mut Api) {
        let Some(options) = api.options.as_mut() else {
            return;
        };
        let costing_type = CostingType::try_from(options.costing_type).ok();
        if matches!(
            costing_type,
            Some(CostingType::Bicycle | CostingType::Pedestrian)
        ) {
            return;
        }
        if !costing_options(options).is_some_and(|o| o.disable_hierarchy_pruning) {
            return;
        }
        let max = self
            .config
            .service_limits
            .max_distance_disable_hierarchy_culling;
        let exceeded = if options.action == i32::from(Action::SourcesToTargets) {
            options.sources.iter().any(|s| {
                options
                    .targets
                    .iter()
                    .any(|t| to_ll(s).distance(to_ll(t)) > f64::from(max))
            })
        } else {
            let mut arc_distance = 0.0_f32;
            let mut exceeded = false;
            for pair in options.locations.windows(2) {
                arc_distance = valhalla_graphtile::midgard::narrow(
                    f64::from(arc_distance) + to_ll(&pair[0]).distance(to_ll(&pair[1])),
                );
                if arc_distance > max {
                    exceeded = true;
                    break;
                }
            }
            exceeded
        };
        if exceeded {
            if let Some(o) = costing_options_mut(options) {
                o.disable_hierarchy_pruning = false;
            }
            add_warning(api, 205, "");
        }
    }

    /// Whether the locations are all in one connected region (error 170 if not).
    fn check_connectivity<'a>(
        &self,
        locations: impl IntoIterator<Item = &'a Location>,
    ) -> Result<(), ValhallaError> {
        let map = self
            .connectivity
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match &map {
            Some(map) if !map.connected(locations) => Err(ErrorCode::UnconnectedRegions.into()),
            _ => Ok(()),
        }
    }

    /// Adds Loki's part of `/status` (mainline's `status` action): the version,
    /// when the tile set was last modified, and the actions this Loki offers
    /// (in mainline's order of actions, not the config's).
    ///
    /// # Errors
    ///
    /// 107 for a verbose status on a server that allows it
    /// (mainline then adds the tile set's outline, which we don't build yet).
    pub fn status(&self, api: &mut Api) -> Result<(), ValhallaError> {
        let verbose = api
            .options
            .as_ref()
            .is_some_and(|o| matches!(o.has_verbose, Some(HasVerbose::Verbose(true))));
        if verbose && self.config.service_limits.allow_verbose_status {
            return Err(ValhallaError::with_detail(
                ErrorCode::NotImplemented,
                "verbose status",
            ));
        }
        let mut actions: Vec<Action> = self.config.actions.clone();
        actions.sort_by_key(|a| *a as i32);
        actions.dedup();
        let status = api
            .status
            .get_or_insert_with(valhalla_proto::Status::default);
        status.version = "3.9.1".to_string();
        status.tileset_last_modified = self.tileset_last_modified;
        status.available_actions = actions
            .iter()
            .map(|a| a.as_str_name().to_string())
            .collect();
        Ok(())
    }

    /// Correlates a route's locations (mainline's `route` action, for `route` and `centroid`).
    ///
    /// # Errors
    ///
    /// 110 without locations, 120 with fewer than two, 124 without a costing,
    /// 150 with too many, 154 if they are too far apart, 157 with too many exclude locations,
    /// the exclude polygon limits (167, 176, and 177), 171 if a location has no edges near it,
    /// and 170 if they are in unconnected regions.
    pub fn route(&self, api: &mut Api) -> Result<(), ValhallaError> {
        Self::timed(api, |api| self.route_inner(api))
    }

    fn route_inner(&self, api: &mut Api) -> Result<(), ValhallaError> {
        self.parse_locations(api, Which::Locations, Some(ErrorCode::LocationsRequired))?;
        let options = api.options.get_or_insert_with(Options::default);
        if options.locations.len() < 2 {
            return Err(ErrorCode::NotEnoughLocations.into());
        }
        let costing = self.parse_costing(api, false)?;
        let options = api.options.get_or_insert_with(Options::default);
        let name = CostingType::try_from(options.costing_type)
            .map(costing_name)
            .unwrap_or_default();
        let limits_name = if options.action == i32::from(Action::Centroid) {
            "centroid"
        } else {
            name
        };
        if let Some(max) = self.config.max_locations(limits_name)
            && options.locations.len() > max
        {
            return Err(ValhallaError::with_detail(
                ErrorCode::TooManyLocations,
                max.to_string(),
            ));
        }
        if let Some(limits) = self.config.costing_limits(limits_name) {
            check_distance(
                &options.locations,
                limits.max_distance,
                limits_name == "centroid",
            )?;
        }
        self.check_hierarchy_distance(api);

        let options = api.options.get_or_insert_with(Options::default);
        let locations = &mut options.locations;
        if let Some(first) = locations.first_mut() {
            first.has_minimum_inbound_reachability =
                Some(HasMinimumInboundReachability::MinimumInboundReachability(0));
        }
        if let Some(last) = locations.last_mut() {
            last.has_minimum_outbound_reachability = Some(
                HasMinimumOutboundReachability::MinimumOutboundReachability(0),
            );
        }
        // Mainline reports any failure of the search as 171.
        search(locations, &self.provider, &costing)
            .map_err(|_| ValhallaError::new(ErrorCode::NoSuitableEdges))?;
        if locations.iter().any(no_edges) {
            return Err(ErrorCode::NoSuitableEdges.into());
        }
        self.check_connectivity(locations.iter())
    }

    /// Correlates the locations of a `/locate` request and serializes the response
    /// (mainline's `locate` action and `tyr::serializeLocate`).
    ///
    /// # Errors
    ///
    /// 110 without locations, the costing errors (a costing isn't required),
    /// and 199 if the graph can't be read.
    pub fn locate(&self, api: &mut Api) -> Result<String, ValhallaError> {
        Self::timed(api, |api| {
            self.parse_locations(api, Which::Locations, Some(ErrorCode::LocationsRequired))?;
            if api.options.as_ref().is_none_or(|o| o.locations.is_empty()) {
                return Err(ErrorCode::NotEnoughLocations.into());
            }
            let costing = self.parse_costing(api, true)?;
            let options = api.options.get_or_insert_with(Options::default);
            search(&mut options.locations, &self.provider, &costing).map_err(|e| unknown(&e))?;
            Ok(serialize_locate(api, &self.provider))
        })
    }

    /// Correlates the sources and targets of a matrix
    /// (mainline's `matrix` action, for `sources_to_targets` and `optimized_route`).
    ///
    /// `optimized_route` copies its locations to both the sources and the targets,
    /// and clears its locations.
    ///
    /// # Errors
    ///
    /// 112 without locations (or sources or targets), 120 with fewer than two locations,
    /// 121 or 122 without sources or targets, 124 without a costing,
    /// 150 with too many pairs, 154 if a pair is too far apart, the exclusion errors,
    /// 171 if a location has no edges near it, and 170 if they are in unconnected regions.
    pub fn matrix(&self, api: &mut Api) -> Result<(), ValhallaError> {
        Self::timed(api, |api| self.matrix_inner(api))
    }

    fn matrix_inner(&self, api: &mut Api) -> Result<(), ValhallaError> {
        let action = api.options.as_ref().map_or(0, |o| o.action);
        if action == i32::from(Action::SourcesToTargets) || action == i32::from(Action::Expansion) {
            self.parse_locations(
                api,
                Which::Sources,
                Some(ErrorCode::MatrixLocationsRequired),
            )?;
            self.parse_locations(
                api,
                Which::Targets,
                Some(ErrorCode::MatrixLocationsRequired),
            )?;
        } else {
            self.parse_locations(
                api,
                Which::Locations,
                Some(ErrorCode::MatrixLocationsRequired),
            )?;
            let options = api.options.get_or_insert_with(Options::default);
            if options.locations.len() < 2 {
                return Err(ErrorCode::NotEnoughLocations.into());
            }
            options.targets.clone_from(&options.locations);
            options.sources.clone_from(&options.locations);
        }
        let options = api.options.get_or_insert_with(Options::default);
        if options.sources.is_empty() {
            return Err(ErrorCode::NotEnoughSources.into());
        }
        if options.targets.is_empty() {
            return Err(ErrorCode::NotEnoughTargets.into());
        }
        options.locations.clear();
        let costing = self.parse_costing(api, false)?;

        let options = api.options.get_or_insert_with(Options::default);
        let name = CostingType::try_from(options.costing_type)
            .map(costing_name)
            .unwrap_or_default();
        let limits = self.config.costing_limits(name).copied();
        if let Some(max) = limits.and_then(|l| l.max_matrix_location_pairs) {
            #[expect(
                clippy::cast_precision_loss,
                reason = "mainline compares the size_t product with the float limit"
            )]
            let pairs = (options.sources.len() * options.targets.len()) as f32;
            if pairs > max {
                return Err(ValhallaError::with_detail(
                    ErrorCode::TooManyLocations,
                    format!("{max:.6}"),
                ));
            }
        }
        if let Some(max) = limits.and_then(|l| l.max_matrix_distance) {
            self.check_matrix_distance(api, max)?;
        }
        self.check_hierarchy_distance(api);

        let options = api.options.get_or_insert_with(Options::default);
        let mut sources_targets: Vec<Location> = options
            .sources
            .iter()
            .chain(&options.targets)
            .cloned()
            .collect();
        search(&mut sources_targets, &self.provider, &costing)
            .map_err(|_| ValhallaError::new(ErrorCode::NoSuitableEdges))?;
        let sources = options.sources.len();
        let mut found = sources_targets.into_iter();
        for (slot, location) in options
            .sources
            .iter_mut()
            .chain(options.targets.iter_mut())
            .zip(&mut found)
        {
            *slot = location;
        }
        debug_assert!(sources <= options.sources.len() + options.targets.len());
        self.check_connectivity(options.sources.iter().chain(&options.targets))
    }

    /// Checks the distance of every matrix pair (154), clearing the `date_time` of pairs too
    /// far apart for a time-dependent matrix (warning 200 if the limit is set).
    fn check_matrix_distance(&self, api: &mut Api, max: f32) -> Result<(), ValhallaError> {
        let max_timedep = self.config.service_limits.max_timedep_distance_matrix;
        let mut added_warning = false;
        let options = api.options.get_or_insert_with(Options::default);
        let targets: Vec<PointLL> = options.targets.iter().map(to_ll).collect();
        let mut clear_targets = vec![false; targets.len()];
        let mut warn = false;
        for source in &mut options.sources {
            let s = to_ll(source);
            for (t, clear) in targets.iter().zip(&mut clear_targets) {
                let distance = s.distance(*t);
                if distance > f64::from(max) {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "mainline prints the limit as a size_t"
                    )]
                    let limit = max as usize;
                    return Err(ValhallaError::with_detail(
                        ErrorCode::PathDistanceExceeded,
                        format!("{limit} meters"),
                    ));
                }
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "mainline converts the distance to size_t"
                )]
                let whole = distance as usize;
                if whole > max_timedep {
                    source.date_time.clear();
                    *clear = true;
                    if max_timedep != 0 && !added_warning {
                        warn = true;
                        added_warning = true;
                    }
                }
            }
        }
        for (target, clear) in options.targets.iter_mut().zip(clear_targets) {
            if clear {
                target.date_time.clear();
            }
        }
        if warn {
            add_warning(api, 200, "");
        }
        Ok(())
    }

    /// Checks a map matching request and correlates the first and last shape points
    /// (mainline's `trace` action, for `trace_route` and `trace_attributes`).
    ///
    /// # Errors
    ///
    /// The costing errors, 114 without a shape, 123 with fewer than two points,
    /// 153 with too many, 154 if it is too long, 172 if no two consecutive points are within the
    /// breakage distance, 158 for out of range trace options,
    /// and 171 if the ends can't be correlated.
    pub fn trace(&self, api: &mut Api) -> Result<(), ValhallaError> {
        Self::timed(api, |api| self.trace_inner(api))
    }

    fn trace_inner(&self, api: &mut Api) -> Result<(), ValhallaError> {
        let costing = self.parse_costing(api, false)?;
        self.check_hierarchy_distance(api);
        let options = api.options.get_or_insert_with(Options::default);
        if options.shape.is_empty() {
            return Err(ErrorCode::ShapeOrPolylineRequired.into());
        }
        let edge_walk = options.shape_match == i32::from(ShapeMatch::EdgeWalk);
        let max_factor = if edge_walk { 5.0_f32 } else { 1.0 };
        let limits = self.config.service_limits.trace;

        // `check_shape`
        if options.shape.len() < 2 {
            return Err(ErrorCode::NotEnoughShape.into());
        }
        if options.shape.len() > limits.max_shape {
            return Err(ValhallaError::with_detail(
                ErrorCode::TooManyShapePoints,
                format!(
                    "({}). The limit is {}",
                    options.shape.len(),
                    limits.max_shape
                ),
            ));
        }
        let breakage = match options.has_breakage_distance {
            Some(HasBreakageDistance::BreakageDistance(d)) => d,
            None => self.config.default_breakage_distance,
        };
        check_trace_distance(&options.shape, limits.max_distance * max_factor, breakage)?;
        if options.shape_match == i32::from(ShapeMatch::MapSnap) {
            let alternates = match options.has_alternates {
                Some(HasAlternates::Alternates(a)) => a,
                None => 0,
            };
            if alternates.saturating_add(1) > 1 && options.shape.len() > limits.max_alternates_shape
            {
                return Err(ValhallaError::with_detail(
                    ErrorCode::TooManyShapePoints,
                    format!(
                        "({}). The best paths shape limit is {}",
                        options.shape.len(),
                        limits.max_alternates_shape
                    ),
                ));
            }
        }
        if let Some(HasGpsAccuracy::GpsAccuracy(a)) = options.has_gps_accuracy
            && (a > limits.max_gps_accuracy || a < 0.0)
        {
            return Err(ErrorCode::TraceOptionOutOfBounds.into());
        }
        if let Some(HasSearchRadius::SearchRadius(r)) = options.has_search_radius
            && (r > limits.max_search_radius || r < 0.0)
        {
            return Err(ErrorCode::TraceOptionOutOfBounds.into());
        }
        if let Some(HasTurnPenaltyFactor::TurnPenaltyFactor(f)) = options.has_turn_penalty_factor
            && f < 0.0
        {
            return Err(ErrorCode::TraceOptionOutOfBounds.into());
        }
        self.locations_from_shape(api, &costing)
            .map_err(|_| ValhallaError::new(ErrorCode::NoSuitableEdges))
    }

    /// Correlates the first and last shape points as the locations (`locations_from_shape`).
    ///
    /// With two locations in the request, their points replace the correlated ones,
    /// and the side of the street of each correlated edge is recomputed from them.
    fn locations_from_shape(
        &self,
        api: &mut Api,
        costing: &AnyCosting,
    ) -> Result<(), LocationsError> {
        let options = api.options.get_or_insert_with(Options::default);
        let provided = (options.locations.len() == 2)
            .then(|| (to_ll(&options.locations[0]), to_ll(&options.locations[1])));
        let (Some(first), Some(last)) = (options.shape.first(), options.shape.last()) else {
            return Ok(());
        };
        options.locations = vec![first.clone(), last.clone()];
        for location in &mut options.locations {
            apply_trace_location_defaults(location);
        }
        self.parse_locations(api, Which::Locations, Some(ErrorCode::LocationsRequired))
            .map_err(LocationsError::Valhalla)?;
        let options = api.options.get_or_insert_with(Options::default);
        search(&mut options.locations, &self.provider, costing)?;

        if let Some((orig, dest)) = provided {
            for (location, ll) in options.locations.iter_mut().zip([orig, dest]) {
                location.ll = Some(lat_lng(ll));
                let Some(correlation) = location.correlation.as_mut() else {
                    continue;
                };
                for e in &mut correlation.edges {
                    e.side_of_street = self.side_from(e.graph_id, ll)?.into();
                }
            }
        }
        Ok(())
    }

    /// The side of an edge a point is on, as `locations_from_shape` decides it.
    fn side_from(&self, graph_id: u64, ll: PointLL) -> Result<SideOfStreet, GraphError> {
        let graph = GraphReader::new(&self.provider);
        let Some(tile) = GraphId::try_from_id(graph_id)
            .ok()
            .map(|id| graph.tile(id).map(|t| t.map(|t| (id, t))))
            .transpose()?
            .flatten()
        else {
            return Ok(SideOfStreet::KNone);
        };
        let (id, tile) = tile;
        let edge = get_edge(&tile, id)?;
        let points = shape(&tile.get_edge_info(edge)?)?;
        let Some((_, distance, index)) = ll.closest_point(&points) else {
            return Ok(SideOfStreet::KNone);
        };
        // Close to the centerline: no side.
        if distance < 5.0 {
            return Ok(SideOfStreet::KNone);
        }
        // When the closest point is the last shape point, mainline's segment ends one past the
        // end of the shape: an out of bounds read, whose result varies from run to run.
        // We use the last segment instead (see `reads_past_end`).
        let index = index.min(points.len().saturating_sub(2));
        let (Some(&a), Some(&b)) = (points.get(index), points.get(index + 1)) else {
            return Ok(SideOfStreet::KNone);
        };
        let is_left = ll.is_left(a, b) > 0.0;
        Ok(if is_left == edge.edge_info_is_forward() {
            SideOfStreet::KLeft
        } else {
            SideOfStreet::KRight
        })
    }
}

/// Whether mainline's side of the street for a trace end given as a location reads past the
/// end of the edge's shape (see [`Loki::locations_from_shape`]), which makes its result
/// unpredictable: the point's closest point on the shape is its last point, and it is farther
/// than 5 m away.
#[cfg(test)]
pub(crate) fn reads_past_end<P: GraphTileProvider>(
    provider: &P,
    graph_id: u64,
    ll: PointLL,
) -> bool {
    let graph = GraphReader::new(provider);
    let Ok(id) = GraphId::try_from_id(graph_id) else {
        return false;
    };
    let Ok(Some(tile)) = graph.tile(id) else {
        return false;
    };
    let Ok(edge) = get_edge(&tile, id) else {
        return false;
    };
    let Ok(points) = tile
        .get_edge_info(edge)
        .map_err(GraphReadError::from)
        .and_then(|i| shape(&i))
    else {
        return false;
    };
    ll.closest_point(&points)
        .is_some_and(|(_, distance, index)| distance >= 5.0 && index + 1 >= points.len())
}

/// A failure while correlating a trace's ends.
#[derive(Debug, thiserror::Error)]
enum LocationsError {
    #[error(transparent)]
    Search(#[from] SearchError),
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error("{}", .0.message())]
    Valhalla(ValhallaError),
}

impl From<GraphReadError> for LocationsError {
    #[inline]
    fn from(e: GraphReadError) -> Self {
        Self::Graph(e.into())
    }
}

/// Whether a location has no candidates at all.
fn no_edges(location: &Location) -> bool {
    location
        .correlation
        .as_ref()
        .is_none_or(|c| c.edges.is_empty() && c.filtered_edges.is_empty())
}

/// Checks the crow-flies distance along the locations (or, for `all_pairs`, between any two)
/// against the limit (`check_distance` in the route action; error 154).
fn check_distance(
    locations: &[Location],
    max_distance: f32,
    all_pairs: bool,
) -> Result<(), ValhallaError> {
    let error = || {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mainline prints the limit as a size_t"
        )]
        let limit = max_distance as usize;
        ValhallaError::with_detail(ErrorCode::PathDistanceExceeded, format!("{limit} meters"))
    };
    let mut total = 0.0_f32;
    for i in 0..locations.len() {
        for j in i + 1..locations.len() {
            let distance = to_ll(&locations[i]).distance(to_ll(&locations[j]));
            if i + 1 == j {
                total = valhalla_graphtile::midgard::narrow(f64::from(total) + distance);
            }
            if (!all_pairs && total > max_distance)
                || (all_pairs && distance > f64::from(max_distance))
            {
                return Err(error());
            }
            if !all_pairs {
                break;
            }
        }
    }
    Ok(())
}

/// Checks a trace's length (154) and that some consecutive points are within the breakage
/// distance (172) (`check_distance` in the trace action).
fn check_trace_distance(
    shape: &[Location],
    max_distance: f32,
    breakage: f32,
) -> Result<(), ValhallaError> {
    let mut can_be_matched = false;
    let mut crow_distance = 0.0_f32;
    for pair in shape.windows(2) {
        let point_to_point =
            valhalla_graphtile::midgard::narrow(to_ll(&pair[0]).distance(to_ll(&pair[1])));
        crow_distance += point_to_point;
        if point_to_point <= breakage {
            can_be_matched = true;
        }
        if crow_distance > max_distance {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "mainline prints the limit as a size_t"
            )]
            let limit = max_distance as usize;
            return Err(ValhallaError::with_detail(
                ErrorCode::PathDistanceExceeded,
                format!("{limit} meters"),
            ));
        }
    }
    if !can_be_matched {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mainline prints the limit as a size_t"
        )]
        let limit = breakage as usize;
        return Err(ValhallaError::with_detail(
            ErrorCode::BreakageDistanceExceeded,
            format!("{limit} meters"),
        ));
    }
    Ok(())
}
