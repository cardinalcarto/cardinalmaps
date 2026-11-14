//! The `meili` section of `valhalla.json` (mainline's `meili::Config`),
//! and merging a request's `trace_options` into it (`MapMatcherFactory::MergeConfig`).
//!
//! Mainline 3.9.1 reads only `meili.default` (the parameters), `meili.customizable`
//! (which parameters a request may change), and `meili.grid` (the candidate search grid).
//! The per-mode sections (`auto`, `pedestrian`, `bicycle`, `multimodal`), `mode`, and `verbose`
//! that `valhalla_build_config` writes are accepted and ignored, as mainline ignores them:
//! older versions merged a mode's section over the defaults, 3.9.1 doesn't.
//! So are the request's `max_route_distance_factor` and `max_route_time_factor`,
//! which 3.9.1's front doesn't parse.

use serde::Deserialize;
use thiserror::Error;
use valhalla_proto::Options;
use valhalla_proto::options::{
    HasBreakageDistance, HasGpsAccuracy, HasInterpolationDistance, HasSearchRadius,
    HasTurnPenaltyFactor,
};

/// A failure to read the `meili` config.
///
/// Mainline throws these from `meili::Config` when the matcher factory is built,
/// that is, when a Thor worker starts.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The JSON doesn't parse, or a value has the wrong type.
    ///
    /// (Mainline reads values through a property tree, which keeps a default for a value
    /// it can't convert; Valinor's configs are strict, as Loki's are.)
    #[error("parse the meili config: {0}")]
    Json(#[from] serde_json::Error),
    /// A parameter must be positive (mainline: "Expect '<name>' to be positive (got: <value>)").
    #[error("read meili.default.{name}: expected a positive value, got {value}")]
    NotPositive {
        /// The parameter.
        name: &'static str,
        /// Its value.
        value: f32,
    },
    /// A parameter must not be negative
    /// (mainline: "Expect '<name>' to be nonnegative (got: <value>)").
    #[error("read meili.default.{name}: expected a nonnegative value, got {value}")]
    Negative {
        /// The parameter.
        name: &'static str,
        /// Its value.
        value: f32,
    },
    /// `meili.grid.size` is 0.
    ///
    /// Mainline accepts it and then fails every match, reading outside a grid of no cells.
    #[error("read meili.grid.size: expected at least one cell per tile")]
    EmptyGrid,
}

/// Candidate search parameters (mainline's `Config::CandidateSearch`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateSearchConfig {
    /// The default search radius in meters (`default.search_radius`).
    pub search_radius_meters: f32,
    /// Whether a request may set `search_radius`.
    pub is_search_radius_customizable: bool,
    /// The largest search radius in meters, whatever a measurement asks for
    /// (`default.max_search_radius`).
    pub max_search_radius_meters: f32,
    /// How many grid cells (one per bin of a tile) the matcher keeps between requests
    /// before it drops them all (`grid.cache_size`).
    pub cache_size: usize,
    /// Cells per side of a local tile in the candidate grid (`grid.size`).
    pub grid_size: usize,
}

/// Transition cost parameters (mainline's `Config::TransitionCost`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionCostConfig {
    /// The scale of the exponential distribution of transition costs (`default.beta`).
    pub beta: f32,
    /// The longest route between two measurements, in meters (`default.breakage_distance`).
    pub breakage_distance_meters: f32,
    /// Whether a request may set `breakage_distance`.
    pub is_breakage_distance_customizable: bool,
    /// The longest route as a multiple of the distance between two measurements
    /// (`default.max_route_distance_factor`).
    pub max_route_distance_factor: f32,
    /// The longest route time as a multiple of the time between two measurements
    /// (`default.max_route_time_factor`).
    pub max_route_time_factor: f32,
    /// The turn penalty at a U-turn; turns cost less the straighter they are
    /// (`default.turn_penalty_factor`).
    pub turn_penalty_factor: f32,
    /// Whether a request may set `turn_penalty_factor`.
    pub is_turn_penalty_factor_customizable: bool,
}

/// Emission cost parameters (mainline's `Config::EmissionCost`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmissionCostConfig {
    /// The standard deviation of GPS measurements, in meters (`default.sigma_z`).
    pub sigma_z: f32,
    /// The default GPS accuracy in meters (`default.gps_accuracy`).
    pub gps_accuracy_meters: f32,
    /// Whether a request may set `gps_accuracy`.
    pub is_gps_accuracy_customizable: bool,
}

/// Routing parameters (mainline's `Config::Routing`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoutingConfig {
    /// Measurements closer than this to the last matched one are interpolated, in meters
    /// (`default.interpolation_distance`).
    pub interpolation_distance_meters: f32,
    /// Whether a request may set `interpolation_distance`.
    pub is_interpolation_distance_customizable: bool,
}

/// The map matcher's parameters (mainline's `meili::Config`).
///
/// [`Default`] is mainline's built-in default (not what `valhalla_build_config` writes,
/// which sets, for example, `turn_penalty_factor` to 0 and `max_search_radius` to 100).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeiliConfig {
    pub candidate_search: CandidateSearchConfig,
    pub transition_cost: TransitionCostConfig,
    pub emission_cost: EmissionCostConfig,
    pub routing: RoutingConfig,
}

impl Default for MeiliConfig {
    fn default() -> Self {
        Self {
            candidate_search: CandidateSearchConfig {
                search_radius_meters: 50.0,
                is_search_radius_customizable: true,
                max_search_radius_meters: 200.0,
                cache_size: 100_240,
                grid_size: 500,
            },
            transition_cost: TransitionCostConfig {
                beta: 3.0,
                breakage_distance_meters: 2000.0,
                is_breakage_distance_customizable: false,
                max_route_distance_factor: 5.0,
                max_route_time_factor: 5.0,
                turn_penalty_factor: 200.0,
                is_turn_penalty_factor_customizable: true,
            },
            emission_cost: EmissionCostConfig {
                sigma_z: 4.07,
                gps_accuracy_meters: 5.0,
                is_gps_accuracy_customizable: false,
            },
            routing: RoutingConfig {
                interpolation_distance_meters: 10.0,
                is_interpolation_distance_customizable: false,
            },
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawRoot {
    #[serde(default)]
    meili: Option<RawMeili>,
}

#[derive(Debug, Default, Deserialize)]
struct RawMeili {
    #[serde(default)]
    default: RawDefault,
    #[serde(default)]
    customizable: Option<Vec<String>>,
    #[serde(default)]
    grid: RawGrid,
}

#[derive(Debug, Default, Deserialize)]
struct RawDefault {
    search_radius: Option<f32>,
    max_search_radius: Option<f32>,
    beta: Option<f32>,
    breakage_distance: Option<f32>,
    max_route_distance_factor: Option<f32>,
    max_route_time_factor: Option<f32>,
    turn_penalty_factor: Option<f32>,
    sigma_z: Option<f32>,
    gps_accuracy: Option<f32>,
    interpolation_distance: Option<f32>,
}

#[derive(Debug, Default, Deserialize)]
struct RawGrid {
    size: Option<usize>,
    cache_size: Option<usize>,
}

/// `CHECK_THROWS(value > 0, POSITIVE_VALUE_MSG(...))`.
fn positive(name: &'static str, value: f32) -> Result<(), ConfigError> {
    if value > 0.0 {
        Ok(())
    } else {
        Err(ConfigError::NotPositive { name, value })
    }
}

/// `CHECK_THROWS(value >= 0, NONNEGATIVE_VALUE_MSG(...))`.
fn nonnegative(name: &'static str, value: f32) -> Result<(), ConfigError> {
    if value >= 0.0 {
        Ok(())
    } else {
        Err(ConfigError::Negative { name, value })
    }
}

impl MeiliConfig {
    /// Reads the `meili` section of a whole `valhalla.json` (mainline's `MapMatcherFactory`
    /// reads `root.get_child("meili")`).
    ///
    /// A config without a `meili` section gets mainline's defaults.
    /// (Mainline requires the section; every config `valhalla_build_config` writes has one.)
    ///
    /// # Errors
    ///
    /// Fails if the JSON doesn't parse, a value has the wrong type,
    /// or a value is out of range (as mainline's `Config::Read` checks).
    pub fn from_valhalla_json(json: &str) -> Result<Self, ConfigError> {
        let raw: RawRoot = serde_json::from_str(json)?;
        Self::from_raw(&raw.meili.unwrap_or_default())
    }

    /// Reads a `meili` section on its own (mainline's `Config(ptree)`).
    ///
    /// # Errors
    ///
    /// As for [`MeiliConfig::from_valhalla_json`].
    pub fn from_meili_json(value: &serde_json::Value) -> Result<Self, ConfigError> {
        let raw = RawMeili::deserialize(value)?;
        Self::from_raw(&raw)
    }

    /// Mainline's `Config::Read`, in its order (so the first bad value is the one reported).
    fn from_raw(raw: &RawMeili) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        let d = &raw.default;
        let customizable = |name: &str| {
            raw.customizable
                .as_ref()
                .map(|names| names.iter().any(|n| n == name))
        };

        let cs = &mut config.candidate_search;
        cs.search_radius_meters = d.search_radius.unwrap_or(cs.search_radius_meters);
        nonnegative("search_radius", cs.search_radius_meters)?;
        if let Some(c) = customizable("search_radius") {
            cs.is_search_radius_customizable = c;
        }
        cs.max_search_radius_meters = d.max_search_radius.unwrap_or(cs.max_search_radius_meters);
        positive("max_search_radius", cs.max_search_radius_meters)?;
        cs.cache_size = raw.grid.cache_size.unwrap_or(cs.cache_size);
        cs.grid_size = raw.grid.size.unwrap_or(cs.grid_size);
        if cs.grid_size == 0 {
            return Err(ConfigError::EmptyGrid);
        }

        let tc = &mut config.transition_cost;
        tc.beta = d.beta.unwrap_or(tc.beta);
        positive("beta", tc.beta)?;
        tc.breakage_distance_meters = d.breakage_distance.unwrap_or(tc.breakage_distance_meters);
        positive("breakage_distance", tc.breakage_distance_meters)?;
        if let Some(c) = customizable("breakage_distance") {
            tc.is_breakage_distance_customizable = c;
        }
        tc.max_route_distance_factor = d
            .max_route_distance_factor
            .unwrap_or(tc.max_route_distance_factor);
        positive("max_route_distance_factor", tc.max_route_distance_factor)?;
        tc.max_route_time_factor = d.max_route_time_factor.unwrap_or(tc.max_route_time_factor);
        positive("max_route_time_factor", tc.max_route_time_factor)?;
        tc.turn_penalty_factor = d.turn_penalty_factor.unwrap_or(tc.turn_penalty_factor);
        nonnegative("turn_penalty_factor", tc.turn_penalty_factor)?;
        if let Some(c) = customizable("turn_penalty_factor") {
            tc.is_turn_penalty_factor_customizable = c;
        }

        let ec = &mut config.emission_cost;
        ec.sigma_z = d.sigma_z.unwrap_or(ec.sigma_z);
        positive("sigma_z", ec.sigma_z)?;
        ec.gps_accuracy_meters = d.gps_accuracy.unwrap_or(ec.gps_accuracy_meters);
        nonnegative("gps_accuracy", ec.gps_accuracy_meters)?;
        if let Some(c) = customizable("gps_accuracy") {
            ec.is_gps_accuracy_customizable = c;
        }

        let rc = &mut config.routing;
        rc.interpolation_distance_meters = d
            .interpolation_distance
            .unwrap_or(rc.interpolation_distance_meters);
        positive("interpolation_distance", rc.interpolation_distance_meters)?;
        if let Some(c) = customizable("interpolation_distance") {
            rc.is_interpolation_distance_customizable = c;
        }
        Ok(config)
    }

    /// This config with the request's customizable `trace_options` applied
    /// (mainline's `MapMatcherFactory::MergeConfig`).
    ///
    /// Only the parameters the config lists as customizable are taken from the request,
    /// and, as in mainline, they aren't checked here:
    /// Loki rejects negative accuracies, radii, and turn penalty factors (error 158),
    /// and a negative turn penalty factor that gets past it fails the matcher
    /// ([`MapMatcherFactory::create`](crate::MapMatcherFactory::create)).
    #[must_use]
    pub fn merge(&self, options: &Options) -> Self {
        let mut config = *self;
        if let Some(HasSearchRadius::SearchRadius(r)) = options.has_search_radius
            && config.candidate_search.is_search_radius_customizable
        {
            config.candidate_search.search_radius_meters = r;
        }
        if let Some(HasTurnPenaltyFactor::TurnPenaltyFactor(f)) = options.has_turn_penalty_factor
            && config.transition_cost.is_turn_penalty_factor_customizable
        {
            config.transition_cost.turn_penalty_factor = f;
        }
        if let Some(HasGpsAccuracy::GpsAccuracy(a)) = options.has_gps_accuracy
            && config.emission_cost.is_gps_accuracy_customizable
        {
            config.emission_cost.gps_accuracy_meters = a;
        }
        if let Some(HasBreakageDistance::BreakageDistance(d)) = options.has_breakage_distance
            && config.transition_cost.is_breakage_distance_customizable
        {
            config.transition_cost.breakage_distance_meters = d;
        }
        if let Some(HasInterpolationDistance::InterpolationDistance(d)) =
            options.has_interpolation_distance
            && config.routing.is_interpolation_distance_customizable
        {
            config.routing.interpolation_distance_meters = d;
        }
        config
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "the values are exact")]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_what_valhalla_build_config_writes() {
        let config = MeiliConfig::from_valhalla_json(
            &json!({"meili": {
                "mode": "auto",
                "customizable": ["mode", "search_radius", "turn_penalty_factor", "gps_accuracy",
                    "interpolation_distance", "sigma_z", "beta", "max_route_distance_factor",
                    "max_route_time_factor"],
                "verbose": false,
                "default": {"sigma_z": 4.07, "gps_accuracy": 5.0, "beta": 3,
                    "max_route_distance_factor": 5, "max_route_time_factor": 5,
                    "max_search_radius": 100, "breakage_distance": 2000,
                    "interpolation_distance": 10, "search_radius": 50, "geometry": false,
                    "route": true, "turn_penalty_factor": 0},
                "auto": {"turn_penalty_factor": 200, "search_radius": 50},
                "pedestrian": {"turn_penalty_factor": 100, "search_radius": 50},
                "grid": {"size": 500, "cache_size": 100_240}
            }})
            .to_string(),
        )
        .unwrap();
        // The per-mode sections are ignored, as in mainline 3.9.1.
        assert_eq!(config.transition_cost.turn_penalty_factor, 0.0);
        assert_eq!(config.candidate_search.max_search_radius_meters, 100.0);
        assert!(config.candidate_search.is_search_radius_customizable);
        assert!(config.emission_cost.is_gps_accuracy_customizable);
        assert!(config.routing.is_interpolation_distance_customizable);
        assert!(!config.transition_cost.is_breakage_distance_customizable);
    }

    #[test]
    fn keeps_mainline_defaults_without_customizable() {
        let config = MeiliConfig::from_valhalla_json(r#"{"meili": {}}"#).unwrap();
        assert_eq!(config, MeiliConfig::default());
        // An empty list makes nothing customizable.
        let config = MeiliConfig::from_valhalla_json(r#"{"meili": {"customizable": []}}"#).unwrap();
        assert!(!config.candidate_search.is_search_radius_customizable);
        assert!(!config.transition_cost.is_turn_penalty_factor_customizable);
    }

    #[test]
    fn rejects_values_out_of_range() {
        for (key, value) in [
            ("search_radius", -1.0),
            ("max_search_radius", 0.0),
            ("beta", 0.0),
            ("breakage_distance", 0.0),
            ("max_route_distance_factor", -1.0),
            ("max_route_time_factor", 0.0),
            ("turn_penalty_factor", -0.5),
            ("sigma_z", 0.0),
            ("gps_accuracy", -1.0),
            ("interpolation_distance", 0.0),
        ] {
            let json = json!({"meili": {"default": {key: value}}}).to_string();
            let err = MeiliConfig::from_valhalla_json(&json).unwrap_err();
            assert!(err.to_string().contains(key), "{key}: {err}");
        }
        assert!(matches!(
            MeiliConfig::from_valhalla_json(r#"{"meili": {"grid": {"size": 0}}}"#),
            Err(ConfigError::EmptyGrid)
        ));
        // Zero is allowed where mainline asks for nonnegative values.
        let json = json!({"meili": {"default": {"search_radius": 0, "gps_accuracy": 0,
            "turn_penalty_factor": 0}}})
        .to_string();
        assert!(MeiliConfig::from_valhalla_json(&json).is_ok());
    }

    #[test]
    fn merges_only_customizable_options() {
        let config = MeiliConfig::from_valhalla_json(
            r#"{"meili": {"customizable": ["gps_accuracy", "breakage_distance"]}}"#,
        )
        .unwrap();
        let options = Options {
            has_search_radius: Some(HasSearchRadius::SearchRadius(7.0)),
            has_turn_penalty_factor: Some(HasTurnPenaltyFactor::TurnPenaltyFactor(-3.0)),
            has_gps_accuracy: Some(HasGpsAccuracy::GpsAccuracy(12.0)),
            has_breakage_distance: Some(HasBreakageDistance::BreakageDistance(150.0)),
            has_interpolation_distance: Some(HasInterpolationDistance::InterpolationDistance(1.0)),
            ..Options::default()
        };
        let merged = config.merge(&options);
        assert_eq!(merged.candidate_search.search_radius_meters, 50.0);
        assert_eq!(merged.transition_cost.turn_penalty_factor, 200.0);
        assert_eq!(merged.emission_cost.gps_accuracy_meters, 12.0);
        assert_eq!(merged.transition_cost.breakage_distance_meters, 150.0);
        assert_eq!(merged.routing.interpolation_distance_meters, 10.0);
        // Unset options keep the config's values.
        assert_eq!(config.merge(&Options::default()), config);
    }
}
