//! The part of mainline's `valhalla.json` that Thor's route actions read.
//!
//! - `thor.bidirectional_astar`: the search extension once paths meet (`threshold_delta`)
//!   and, for alternates, `alternative_cost_extend` and `alternative_iterations_delta`.
//! - `thor.extended_search`: whether a search goes on in one direction after the other
//!   is exhausted.
//! - `thor.{bidirectional_astar,unidirectional_astar}.hierarchy_limits` and
//!   `service_limits.hierarchy_limits`: the default and largest hierarchy limits.
//! - `service_limits.max_timedep_distance`: the longest time-dependent route.
//!
//! [`ThorConfig::default`] is mainline's defaults (as `valhalla_build_config` writes them);
//! [`ThorConfig::from_valhalla_json`] reads a config file, keeping defaults for missing keys
//! (mainline's `config.get(key, default)`).

use serde_json::Value;
use thiserror::Error;
use valhalla_proto::HierarchyLimits;
use valinor_sif::UNLIMITED_TRANSITIONS;

/// A failure to read a config.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The JSON doesn't parse.
    #[error("parse the config: {0}")]
    Json(#[from] serde_json::Error),
    /// A value has the wrong type.
    #[error("read {0}: it has the wrong type")]
    WrongType(String),
}

/// The routing graph's hierarchy levels (`TileHierarchy::levels()`).
pub(crate) const LEVELS: usize = 3;

/// Mainline's default upward transitions by level (`kDefaultMaxUpTransitions`).
const DEFAULT_MAX_UP_TRANSITIONS: [u32; LEVELS] = [0, 400, 100];
/// Mainline's default expansion distances by level (`kDefaultExpansionWithinDist`).
const DEFAULT_EXPANSION_WITHIN_DIST: [f32; LEVELS] = [f32::MAX, 100_000.0, 5000.0];
/// Bidirectional searches' defaults (`kDefaultExpansionWithinDistBidir`).
const DEFAULT_EXPANSION_WITHIN_DIST_BIDIR: [f32; LEVELS] = [f32::MAX, 20_000.0, 5000.0];

/// The hierarchy limits a search uses when the request has none,
/// and the largest it may ask for (mainline's `hierarchy_limits_config_t`).
#[derive(Clone, Debug, PartialEq)]
pub struct HierarchyLimitsConfig {
    /// The largest limits a request may set, by level (`service_limits.hierarchy_limits`).
    pub max_limits: Vec<HierarchyLimits>,
    /// The limits used when the request sets none, by level (`thor.<algorithm>.hierarchy_limits`).
    pub default_limits: Vec<HierarchyLimits>,
}

/// Thor's configuration for routes.
#[derive(Clone, Debug, PartialEq)]
pub struct ThorConfig {
    /// Cost added to the first connection's to end a bidirectional search
    /// (`thor.bidirectional_astar.threshold_delta`).
    pub threshold_delta: f32,
    /// Factor on the first connection's cost when looking for alternates
    /// (`thor.bidirectional_astar.alternative_cost_extend`).
    pub alternative_cost_extend: f32,
    /// More labels allowed after the first connection when looking for alternates
    /// (`thor.bidirectional_astar.alternative_iterations_delta`).
    pub alternative_iterations_delta: u32,
    /// Whether a bidirectional search goes on in one direction when the other is exhausted
    /// and started on a not-thru or closed edge (`thor.extended_search`).
    pub extended_search: bool,
    /// The longest time-dependent route, in meters (`service_limits.max_timedep_distance`).
    pub max_timedep_distance: f32,
    /// Whether requests may set their own hierarchy limits
    /// (`service_limits.hierarchy_limits.allow_modification`).
    pub allow_hierarchy_limits_modifications: bool,
    /// Hierarchy limits for bidirectional A*.
    pub hierarchy_limits_bidirectional: HierarchyLimitsConfig,
    /// Hierarchy limits for unidirectional (time-dependent) A*.
    pub hierarchy_limits_unidirectional: HierarchyLimitsConfig,
}

impl Default for ThorConfig {
    fn default() -> Self {
        let limits = |dists: [f32; LEVELS]| {
            let levels: Vec<HierarchyLimits> = (0..LEVELS)
                .map(|level| HierarchyLimits {
                    up_transition_count: 0,
                    max_up_transitions: DEFAULT_MAX_UP_TRANSITIONS[level],
                    expand_within_dist: dists[level],
                })
                .collect();
            HierarchyLimitsConfig {
                max_limits: levels.clone(),
                default_limits: levels,
            }
        };
        Self {
            threshold_delta: 420.0,
            alternative_cost_extend: 1.2,
            alternative_iterations_delta: 100_000,
            extended_search: false,
            max_timedep_distance: 500_000.0,
            allow_hierarchy_limits_modifications: false,
            hierarchy_limits_bidirectional: limits(DEFAULT_EXPANSION_WITHIN_DIST_BIDIR),
            hierarchy_limits_unidirectional: limits(DEFAULT_EXPANSION_WITHIN_DIST),
        }
    }
}

/// The value at a path of keys, if every key is there.
fn at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |v, k| v.get(*k))
}

/// A number at a path, or `default` if it's missing (`config.get<T>(path, default)`).
///
/// Mainline's property tree also reads numbers stored as strings.
fn number(value: &Value, path: &[&str], default: f64) -> Result<f64, ConfigError> {
    match at(value, path) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(n)) => n
            .as_f64()
            .ok_or_else(|| ConfigError::WrongType(path.join("."))),
        Some(Value::String(s)) => s
            .trim()
            .parse()
            .map_err(|_| ConfigError::WrongType(path.join("."))),
        Some(_) => Err(ConfigError::WrongType(path.join("."))),
    }
}

fn boolean(value: &Value, path: &[&str], default: bool) -> Result<bool, ConfigError> {
    match at(value, path) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::String(s)) if s == "true" => Ok(true),
        Some(Value::String(s)) if s == "false" => Ok(false),
        Some(_) => Err(ConfigError::WrongType(path.join("."))),
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "config values are read as doubles and converted as mainline's ptree does"
)]
fn as_u32(v: f64) -> u32 {
    v as u32
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline reads these as floats"
)]
fn as_f32(v: f64) -> f32 {
    v as f32
}

/// Mainline's `parse_hierarchy_limits_from_config` (with `uses_dist`, as the routes' searches do).
fn hierarchy_limits(value: &Value, algorithm: &str) -> Result<HierarchyLimitsConfig, ConfigError> {
    let is_bidir = algorithm != "unidirectional_astar";
    let mut max_limits = Vec::with_capacity(LEVELS);
    let mut default_limits = Vec::with_capacity(LEVELS);
    for level in 0..LEVELS {
        let key = level.to_string();
        let default_dist = if is_bidir {
            DEFAULT_EXPANSION_WITHIN_DIST_BIDIR[level]
        } else {
            DEFAULT_EXPANSION_WITHIN_DIST[level]
        };
        let max_hl = HierarchyLimits {
            up_transition_count: 0,
            max_up_transitions: as_u32(number(
                value,
                &[
                    "service_limits",
                    "hierarchy_limits",
                    algorithm,
                    "max_allowed_up_transitions",
                    &key,
                ],
                f64::from(DEFAULT_MAX_UP_TRANSITIONS[level]),
            )?),
            expand_within_dist: match at(
                value,
                &[
                    "service_limits",
                    "hierarchy_limits",
                    algorithm,
                    "max_expand_within_distance",
                    &key,
                ],
            ) {
                // Mainline's fallback for a value that is there but unreadable
                // is the unidirectional default.
                Some(_) => as_f32(number(
                    value,
                    &[
                        "service_limits",
                        "hierarchy_limits",
                        algorithm,
                        "max_expand_within_distance",
                        &key,
                    ],
                    f64::from(DEFAULT_EXPANSION_WITHIN_DIST[level]),
                )?),
                None => default_dist,
            },
        };
        let default_hl = HierarchyLimits {
            up_transition_count: 0,
            max_up_transitions: as_u32(number(
                value,
                &[
                    "thor",
                    algorithm,
                    "hierarchy_limits",
                    "max_up_transitions",
                    &key,
                ],
                f64::from(DEFAULT_MAX_UP_TRANSITIONS[level]),
            )?),
            expand_within_dist: match at(
                value,
                &[
                    "thor",
                    algorithm,
                    "hierarchy_limits",
                    "expand_within_distance",
                    &key,
                ],
            ) {
                Some(_) => as_f32(number(
                    value,
                    &[
                        "thor",
                        algorithm,
                        "hierarchy_limits",
                        "expand_within_distance",
                        &key,
                    ],
                    f64::from(DEFAULT_EXPANSION_WITHIN_DIST[level]),
                )?),
                None => default_dist,
            },
        };
        max_limits.push(max_hl);
        default_limits.push(default_hl);
    }
    Ok(HierarchyLimitsConfig {
        max_limits,
        default_limits,
    })
}

impl ThorConfig {
    /// Reads Thor's part of a mainline `valhalla.json`.
    ///
    /// # Errors
    ///
    /// Fails if the JSON doesn't parse or a value Thor reads has the wrong type.
    pub fn from_valhalla_json(json: &str) -> Result<Self, ConfigError> {
        Self::from_value(&serde_json::from_str(json)?)
    }

    fn from_value(value: &Value) -> Result<Self, ConfigError> {
        let bd = ["thor", "bidirectional_astar"];
        Ok(Self {
            threshold_delta: as_f32(number(value, &[bd[0], bd[1], "threshold_delta"], 420.0)?),
            alternative_cost_extend: as_f32(number(
                value,
                &[bd[0], bd[1], "alternative_cost_extend"],
                1.2,
            )?),
            alternative_iterations_delta: as_u32(number(
                value,
                &[bd[0], bd[1], "alternative_iterations_delta"],
                100_000.0,
            )?),
            extended_search: boolean(value, &["thor", "extended_search"], false)?,
            max_timedep_distance: as_f32(number(
                value,
                &["service_limits", "max_timedep_distance"],
                500_000.0,
            )?),
            allow_hierarchy_limits_modifications: boolean(
                value,
                &["service_limits", "hierarchy_limits", "allow_modification"],
                false,
            )?,
            hierarchy_limits_bidirectional: hierarchy_limits(value, "bidirectional_astar")?,
            hierarchy_limits_unidirectional: hierarchy_limits(value, "unidirectional_astar")?,
        })
    }
}

/// Whether a set of hierarchy limits is the "unset" sentinel a request leaves
/// (no limit on transitions, and the largest distance).
#[expect(
    clippy::float_cmp,
    reason = "mainline compares with kMaxDistance exactly"
)]
pub(crate) fn is_unset(limits: &HierarchyLimits) -> bool {
    limits.max_up_transitions == UNLIMITED_TRANSITIONS && limits.expand_within_dist == f32::MAX
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "exact defaults")]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_mainline() {
        let c = ThorConfig::default();
        assert_eq!(ThorConfig::from_value(&Value::Null).unwrap(), c);
        assert_eq!(c.threshold_delta, 420.0);
        assert_eq!(c.alternative_iterations_delta, 100_000);
        assert!(!c.extended_search);
        assert_eq!(c.max_timedep_distance, 500_000.0);
        let bd = &c.hierarchy_limits_bidirectional.default_limits;
        assert_eq!(bd[1].max_up_transitions, 400);
        assert_eq!(bd[1].expand_within_dist, 20_000.0);
        let ud = &c.hierarchy_limits_unidirectional.default_limits;
        assert_eq!(ud[1].expand_within_dist, 100_000.0);
        assert_eq!(ud[0].max_up_transitions, 0);
    }

    #[test]
    fn reads_a_mainline_config() {
        let json = r#"{"thor":{"extended_search":true,"bidirectional_astar":{"threshold_delta":100,
            "hierarchy_limits":{"max_up_transitions":{"1":"7"},"expand_within_distance":{"2":"9"}}}},
            "service_limits":{"max_timedep_distance":1000}}"#;
        let c = ThorConfig::from_valhalla_json(json).unwrap();
        assert!(c.extended_search);
        assert_eq!(c.threshold_delta, 100.0);
        assert_eq!(c.max_timedep_distance, 1000.0);
        let bd = &c.hierarchy_limits_bidirectional.default_limits;
        assert_eq!(bd[1].max_up_transitions, 7);
        assert_eq!(bd[2].expand_within_dist, 9.0);
        assert_eq!(bd[1].expand_within_dist, 20_000.0);
        assert!(ThorConfig::from_valhalla_json(r#"{"thor":{"extended_search":3}}"#).is_err());
    }
}
