//! The part of mainline's `valhalla.json` that Loki reads.
//!
//! Mainline's Loki worker reads `loki.actions`, `loki.use_connectivity`,
//! `loki.service_defaults` (per-location defaults), `service_limits`
//! (request limits, per costing and for the actions), and `meili.default.breakage_distance`.
//! [`LokiConfig`] is that subset as a typed struct.
//! Its [`Default`] is what mainline's `valhalla_build_config` writes,
//! and [`LokiConfig::from_valhalla_json`] reads a mainline config file
//! (keys it doesn't know are ignored; keys it knows but are missing keep their defaults).

use serde::Deserialize;
use std::collections::BTreeMap;
use thiserror::Error;
use valhalla_proto::options::Action;

/// A failure to read a config.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The JSON doesn't parse, or a value has the wrong type.
    #[error("parse the config: {0}")]
    Json(#[from] serde_json::Error),
    /// `loki.actions` names an action mainline doesn't have.
    #[error("read loki.actions: unknown action {0:?}")]
    UnknownAction(String),
    /// `loki.actions` is empty (mainline: "The config actions for Loki are incorrectly loaded").
    #[error("read loki.actions: no actions are configured")]
    NoActions,
}

/// Per-location defaults (`loki.service_defaults`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServiceDefaults {
    /// The default search radius in meters.
    pub radius: u32,
    /// The default minimum reachability (both directions).
    pub minimum_reachability: u32,
    /// The default search cutoff in meters.
    pub search_cutoff: u32,
    /// The default node snap tolerance in meters.
    pub node_snap_tolerance: u32,
    /// The default street side tolerance in meters.
    pub street_side_tolerance: u32,
    /// The default street side max distance in meters.
    pub street_side_max_distance: u32,
    /// The default heading tolerance in degrees.
    pub heading_tolerance: u32,
}

impl Default for ServiceDefaults {
    fn default() -> Self {
        Self {
            radius: 0,
            minimum_reachability: 50,
            search_cutoff: 35_000,
            node_snap_tolerance: 5,
            street_side_tolerance: 5,
            street_side_max_distance: 1000,
            heading_tolerance: 60,
        }
    }
}

/// The limits for one costing (`service_limits.<costing>`).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct CostingLimits {
    /// The most locations a route may have.
    /// (`auto_pedestrian` has none in the config; Loki allows it 2.)
    #[serde(default)]
    pub max_locations: Option<usize>,
    /// The longest crow-flies distance along a route's locations, in meters.
    pub max_distance: f32,
    /// The longest crow-flies distance between a matrix source and target, in meters.
    #[serde(default)]
    pub max_matrix_distance: Option<f32>,
    /// The most source and target pairs a matrix may have.
    #[serde(default)]
    pub max_matrix_location_pairs: Option<f32>,
}

impl CostingLimits {
    const fn new(
        max_locations: usize,
        max_distance: f32,
        matrix_distance: f32,
        pairs: f32,
    ) -> Self {
        Self {
            max_locations: Some(max_locations),
            max_distance,
            max_matrix_distance: Some(matrix_distance),
            max_matrix_location_pairs: Some(pairs),
        }
    }
}

/// The limits for map matching (`service_limits.trace`).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct TraceLimits {
    /// The longest crow-flies distance along the shape, in meters.
    pub max_distance: f32,
    /// The largest `gps_accuracy`, in meters.
    pub max_gps_accuracy: f32,
    /// The largest `search_radius`, in meters.
    pub max_search_radius: f32,
    /// The most shape points.
    pub max_shape: usize,
    /// The most alternates for `trace_attributes`.
    pub max_alternates: u32,
    /// The most shape points when alternates are asked for.
    pub max_alternates_shape: usize,
}

impl Default for TraceLimits {
    fn default() -> Self {
        Self {
            max_distance: 200_000.0,
            max_gps_accuracy: 100.0,
            max_search_radius: 100.0,
            max_shape: 16_000,
            max_alternates: 3,
            max_alternates_shape: 100,
        }
    }
}

/// The request limits (`service_limits`).
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceLimits {
    /// Limits by costing name (and `centroid`).
    pub costings: BTreeMap<String, CostingLimits>,
    /// Map matching limits.
    pub trace: TraceLimits,
    /// The most `exclude_locations`.
    pub max_exclude_locations: usize,
    /// The largest `minimum_reachability`.
    pub max_reachability: u32,
    /// The largest `radius`.
    pub max_radius: u32,
    /// Matrix pairs farther apart than this lose their `date_time` (0 means always).
    pub max_timedep_distance_matrix: usize,
    /// The most route alternates.
    pub max_alternates: u32,
    /// The longest total perimeter of `exclude_polygons`, in meters.
    pub max_exclude_polygons_length: f32,
    /// The most vertices in `exclude_polygons`.
    pub max_exclude_polygons_vertices: usize,
    /// How far apart locations may be for `disable_hierarchy_pruning`, in meters.
    pub max_distance_disable_hierarchy_culling: f32,
    /// Whether `exclude_bridges` and the other hard exclusions are allowed.
    pub allow_hard_exclusions: bool,
    /// Whether `/status` may be verbose (`service_limits.status.allow_verbose`).
    pub allow_verbose_status: bool,
    /// The shortest walk allowed at the ends of a multimodal route, in meters.
    pub min_multimodal_walking_distance: usize,
    /// The longest walk allowed at the ends of a multimodal route, in meters.
    pub max_multimodal_walking_distance: usize,
}

impl Default for ServiceLimits {
    fn default() -> Self {
        let mut costings = BTreeMap::new();
        for (name, limits) in [
            (
                "auto",
                CostingLimits::new(20, 5_000_000.0, 400_000.0, 2500.0),
            ),
            (
                "bus",
                CostingLimits::new(50, 5_000_000.0, 400_000.0, 2500.0),
            ),
            (
                "taxi",
                CostingLimits::new(20, 5_000_000.0, 400_000.0, 2500.0),
            ),
            (
                "pedestrian",
                CostingLimits::new(50, 250_000.0, 200_000.0, 2500.0),
            ),
            (
                "motor_scooter",
                CostingLimits::new(50, 500_000.0, 200_000.0, 2500.0),
            ),
            (
                "motorcycle",
                CostingLimits::new(50, 500_000.0, 200_000.0, 2500.0),
            ),
            (
                "bicycle",
                CostingLimits::new(50, 500_000.0, 200_000.0, 2500.0),
            ),
            ("multimodal", CostingLimits::new(50, 500_000.0, 0.0, 0.0)),
            (
                "transit",
                CostingLimits::new(50, 500_000.0, 200_000.0, 2500.0),
            ),
            (
                "truck",
                CostingLimits::new(20, 5_000_000.0, 400_000.0, 2500.0),
            ),
            (
                "bikeshare",
                CostingLimits::new(50, 500_000.0, 200_000.0, 2500.0),
            ),
            (
                "auto_pedestrian",
                CostingLimits {
                    max_locations: None,
                    max_distance: 5_000_000.0,
                    max_matrix_distance: Some(400_000.0),
                    max_matrix_location_pairs: Some(2500.0),
                },
            ),
            (
                "centroid",
                CostingLimits {
                    max_locations: Some(5),
                    max_distance: 200_000.0,
                    max_matrix_distance: None,
                    max_matrix_location_pairs: None,
                },
            ),
        ] {
            costings.insert(name.to_string(), limits);
        }
        Self {
            costings,
            trace: TraceLimits::default(),
            max_exclude_locations: 50,
            max_reachability: 100,
            max_radius: 200,
            max_timedep_distance_matrix: 0,
            max_alternates: 2,
            max_exclude_polygons_length: 10_000.0,
            max_exclude_polygons_vertices: 100,
            max_distance_disable_hierarchy_culling: 0.0,
            allow_hard_exclusions: false,
            allow_verbose_status: false,
            min_multimodal_walking_distance: 1,
            max_multimodal_walking_distance: 10_000,
        }
    }
}

/// What Loki reads from mainline's config.
#[derive(Clone, Debug, PartialEq)]
pub struct LokiConfig {
    /// The actions this Loki offers (`loki.actions`), in config order.
    pub actions: Vec<Action>,
    /// Whether routes check that their locations are in connected regions
    /// (`loki.use_connectivity`).
    pub use_connectivity: bool,
    /// Per-location defaults.
    pub service_defaults: ServiceDefaults,
    /// Request limits.
    pub service_limits: ServiceLimits,
    /// The default breakage distance for map matching (`meili.default.breakage_distance`).
    pub default_breakage_distance: f32,
}

impl Default for LokiConfig {
    fn default() -> Self {
        Self {
            actions: vec![
                Action::Locate,
                Action::Route,
                Action::Height,
                Action::SourcesToTargets,
                Action::OptimizedRoute,
                Action::Isochrone,
                Action::TraceRoute,
                Action::TraceAttributes,
                Action::TransitAvailable,
                Action::Expansion,
                Action::Centroid,
                Action::Status,
                Action::Tile,
            ],
            use_connectivity: true,
            service_defaults: ServiceDefaults::default(),
            service_limits: ServiceLimits::default(),
            default_breakage_distance: 2000.0,
        }
    }
}

/// The JSON layout of the parts we read, with every field optional.
#[derive(Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    loki: RawLoki,
    #[serde(default)]
    service_limits: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    meili: RawMeili,
}

#[derive(Deserialize, Default)]
struct RawLoki {
    actions: Option<Vec<String>>,
    use_connectivity: Option<bool>,
    service_defaults: Option<ServiceDefaults>,
}

#[derive(Deserialize, Default)]
struct RawMeili {
    #[serde(default)]
    default: RawMeiliDefault,
}

#[derive(Deserialize, Default)]
struct RawMeiliDefault {
    breakage_distance: Option<f32>,
}

/// `service_limits` keys that aren't costings (mainline skips these when it reads per-costing
/// limits).
const NOT_COSTINGS: &[&str] = &[
    "max_exclude_locations",
    "max_reachability",
    "max_radius",
    "max_timedep_distance",
    "max_timedep_distance_matrix",
    "max_alternates",
    "max_exclude_polygons_length",
    "max_exclude_polygons_vertices",
    "max_distance_disable_hierarchy_culling",
    "skadi",
    "status",
    "allow_hard_exclusions",
    "hierarchy_limits",
    "min_linear_cost_factor",
    "max_linear_cost_edges",
    "trace",
    "isochrone",
];

impl LokiConfig {
    /// Reads the Loki subset of a mainline `valhalla.json`.
    ///
    /// # Errors
    ///
    /// Fails if the JSON doesn't parse, a value we read has the wrong type,
    /// or `loki.actions` is empty or names an unknown action.
    pub fn from_valhalla_json(json: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = serde_json::from_str(json)?;
        let mut config = Self::default();
        if let Some(actions) = raw.loki.actions {
            config.actions = actions
                .iter()
                .map(|name| {
                    Action::from_str_name(name)
                        .ok_or_else(|| ConfigError::UnknownAction(name.clone()))
                })
                .collect::<Result<_, _>>()?;
            if config.actions.is_empty() {
                return Err(ConfigError::NoActions);
            }
        }
        if let Some(use_connectivity) = raw.loki.use_connectivity {
            config.use_connectivity = use_connectivity;
        }
        if let Some(defaults) = raw.loki.service_defaults {
            config.service_defaults = defaults;
        }
        if let Some(distance) = raw.meili.default.breakage_distance {
            config.default_breakage_distance = distance;
        }

        let limits = &mut config.service_limits;
        let get = |key: &str| raw.service_limits.get(key);
        let number = |key: &str| get(key).and_then(serde_json::Value::as_f64);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mainline reads these as integers; the config holds small whole numbers"
        )]
        {
            if let Some(v) = number("max_exclude_locations") {
                limits.max_exclude_locations = v as usize;
            }
            if let Some(v) = number("max_reachability") {
                limits.max_reachability = v as u32;
            }
            if let Some(v) = number("max_radius") {
                limits.max_radius = v as u32;
            }
            if let Some(v) = number("max_timedep_distance_matrix") {
                limits.max_timedep_distance_matrix = v as usize;
            }
            if let Some(v) = number("max_alternates") {
                limits.max_alternates = v as u32;
            }
            if let Some(v) = number("max_exclude_polygons_length") {
                limits.max_exclude_polygons_length = v as f32;
            }
            if let Some(v) = number("max_exclude_polygons_vertices") {
                limits.max_exclude_polygons_vertices = v as usize;
            }
            if let Some(v) = number("max_distance_disable_hierarchy_culling") {
                limits.max_distance_disable_hierarchy_culling = v as f32;
            }
        }
        if let Some(v) = get("allow_hard_exclusions").and_then(serde_json::Value::as_bool) {
            limits.allow_hard_exclusions = v;
        }
        if let Some(v) = get("status")
            .and_then(|s| s.get("allow_verbose"))
            .and_then(serde_json::Value::as_bool)
        {
            limits.allow_verbose_status = v;
        }
        if let Some(trace) = get("trace") {
            limits.trace = TraceLimits::deserialize(trace)?;
        }
        if let Some(pedestrian) = get("pedestrian") {
            // Mainline falls back to the old names for these.
            let walk = |new: &str, old: &str| {
                pedestrian
                    .get(new)
                    .or_else(|| pedestrian.get(old))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|v| usize::try_from(v).ok())
            };
            if let Some(v) = walk(
                "min_multimodal_walking_distance",
                "min_transit_walking_distance",
            ) {
                limits.min_multimodal_walking_distance = v;
            }
            if let Some(v) = walk(
                "max_multimodal_walking_distance",
                "max_transit_walking_distance",
            ) {
                limits.max_multimodal_walking_distance = v;
            }
        }
        for (name, value) in &raw.service_limits {
            if NOT_COSTINGS.contains(&name.as_str()) || !value.is_object() {
                continue;
            }
            limits
                .costings
                .insert(name.clone(), CostingLimits::deserialize(value)?);
        }
        Ok(config)
    }

    /// The limits for a costing (or `centroid`), if the config has them.
    #[inline]
    pub fn costing_limits(&self, name: &str) -> Option<&CostingLimits> {
        self.service_limits.costings.get(name)
    }

    /// The most locations a route with this costing may have
    /// (`auto_pedestrian` is always 2, whatever the config says).
    pub fn max_locations(&self, name: &str) -> Option<usize> {
        if name == "auto_pedestrian" {
            return self
                .costing_limits(name)
                .and_then(|l| l.max_locations)
                .or(Some(2));
        }
        self.costing_limits(name).and_then(|l| l.max_locations)
    }

    /// The configured actions as mainline lists them in error 106:
    /// `'/locate' '/route' ...`, each followed by a space.
    pub fn action_list(&self) -> String {
        let mut list = String::new();
        for action in &self.actions {
            list.push_str("'/");
            list.push_str(action.as_str_name());
            list.push_str("' ");
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORACLE_CONFIG: &str = include_str!("../fixtures/valhalla.json");

    #[test]
    fn reads_the_oracle_config() {
        let config = LokiConfig::from_valhalla_json(ORACLE_CONFIG).unwrap();
        // The oracle's config is the one `valhalla_build_config` writes, which is our default.
        assert_eq!(config, LokiConfig::default());
        assert_eq!(config.max_locations("auto"), Some(20));
        assert_eq!(config.max_locations("auto_pedestrian"), Some(2));
        assert_eq!(config.max_locations("centroid"), Some(5));
        assert!(config.action_list().starts_with("'/locate' '/route' "));
    }

    #[test]
    fn missing_keys_keep_their_defaults() {
        let config = LokiConfig::from_valhalla_json(r#"{"loki": {"actions": ["route"]}}"#).unwrap();
        assert_eq!(config.actions, vec![Action::Route]);
        assert_eq!(config.service_limits, ServiceLimits::default());
        assert!(matches!(
            LokiConfig::from_valhalla_json(r#"{"loki": {"actions": ["teleport"]}}"#),
            Err(ConfigError::UnknownAction(_))
        ));
        assert!(matches!(
            LokiConfig::from_valhalla_json(r#"{"loki": {"actions": []}}"#),
            Err(ConfigError::NoActions)
        ));
        assert!(LokiConfig::from_valhalla_json("{").is_err());
    }
}
