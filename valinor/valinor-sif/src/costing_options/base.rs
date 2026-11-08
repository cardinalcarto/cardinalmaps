//! The options common to most costings (`ParseBaseCostOptions` in `src/sif/dynamiccost.cc`),
//! and the helpers every costing's parser uses (the `JSON_PBF_*` macros in `sif/dynamiccost.h`).

use crate::json::{FromJson, JsonValue};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use valhalla_proto::costing::HasName;
use valhalla_proto::costing::Options as CostingOptions;
use valhalla_proto::costing::options::{
    HasAlleyPenalty, HasClosureFactor, HasCountryCrossingCost, HasCountryCrossingPenalty,
    HasDestinationOnlyPenalty, HasFerryCost, HasFlowMask, HasGateCost, HasGatePenalty, HasHeight,
    HasIgnoreClosures, HasLength, HasManeuverPenalty, HasPrivateAccessPenalty, HasRailFerryCost,
    HasServiceFactor, HasServicePenalty, HasSpeedPenaltyFactor, HasTollBoothCost,
    HasTollBoothPenalty, HasUseDistance, HasUseFerry, HasUseLivingStreets, HasUseRailFerry,
    HasUseTracks, HasWeight, HasWidth,
};
use valhalla_proto::{CodedDescription, Costing, HierarchyLimits};

/// The longest penalty mainline allows, in seconds (`kMaxPenalty`: 12 hours).
pub(crate) const MAX_PENALTY: f32 = 12.0 * 3600.0;
/// The smallest factor mainline allows (`kMinFactor`).
pub(crate) const MIN_FACTOR: f32 = 0.1;
/// The largest factor mainline allows (`kMaxFactor`).
pub(crate) const MAX_FACTOR: f32 = 100_000.0;
/// The fastest speed mainline knows from static data, in km/h (`kMaxAssumedSpeed`).
pub(crate) const MAX_ASSUMED_SPEED: u32 = 140;
/// The fastest speed mainline allows, in km/h (`kMaxSpeedKph`, the traffic maximum).
pub(crate) const MAX_SPEED_KPH: u32 = 252;
/// Every speed source (`kDefaultFlowMask`): free flow, constrained, predicted, and current.
pub const DEFAULT_FLOW_MASK: u32 = 1 | 2 | 4 | 8;
/// The predicted and current speed sources, which need a time.
pub const TIME_DEPENDENT_FLOW_MASK: u32 = 4 | 8;

/// A valid range and the default for an option (mainline's `ranged_default_t`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangedDefault<T> {
    /// The smallest valid value.
    pub min: T,
    /// The value used when none is given, or when the given one is out of range.
    pub def: T,
    /// The largest valid value.
    pub max: T,
}

impl<T: PartialOrd + Copy> RangedDefault<T> {
    pub(crate) const fn new(min: T, def: T, max: T) -> Self {
        Self { min, def, max }
    }

    /// The value, or the default if it is out of range (and whether it was).
    ///
    /// Like mainline's, the comparison lets NaN through.
    fn clamp(&self, value: T) -> (T, bool) {
        if value < self.min || value > self.max {
            (self.def, true)
        } else {
            (value, false)
        }
    }
}

/// A value type of a ranged option, as `std::to_string` writes it in clamping warnings.
pub(crate) trait RangedValue: FromJson + PartialOrd + Copy {
    fn cpp_to_string(self) -> String;
}

impl RangedValue for f32 {
    fn cpp_to_string(self) -> String {
        // `std::to_string(float)` is `%f`.
        format!("{:.6}", f64::from(self))
    }
}

impl RangedValue for u32 {
    fn cpp_to_string(self) -> String {
        format!("{self}")
    }
}

/// `JSON_PBF_RANGED_DEFAULT` and `JSON_PBF_RANGED_DEFAULT_V2`:
/// the JSON value, else the current (PBF) value, else the default,
/// replaced by the default (with warning 500) if it is out of range.
pub(crate) fn ranged<T: RangedValue>(
    json: &JsonValue,
    key: &str,
    range: RangedDefault<T>,
    current: Option<T>,
    warnings: &mut Vec<CodedDescription>,
) -> T {
    let (value, clamped) = range.clamp(json.get_or(key, current.unwrap_or(range.def)));
    if clamped {
        warnings.push(CodedDescription {
            description: format!("'{key}' has been clamped to {}", range.def.cpp_to_string()),
            code: 500,
        });
    }
    value
}

/// The current value of a field without presence (the `_V2` macros), which counts as unset when 0.
pub(crate) fn nonzero_f32(value: f32) -> Option<f32> {
    (value != 0.0).then_some(value)
}

/// [`nonzero_f32`] for integers.
pub(crate) fn nonzero_u32(value: u32) -> Option<u32> {
    (value != 0).then_some(value)
}

/// `JSON_PBF_DEFAULT_V2` for booleans: the JSON value, else the current value if set, else the default.
pub(crate) fn flag(json: &JsonValue, key: &str, current: bool, default: bool) -> bool {
    json.get_or(key, current || default)
}

/// Reads a oneof field's value, if it is set.
macro_rules! oneof {
    ($field:expr, $variant:path) => {
        match $field {
            Some($variant(value)) => Some(value),
            None => None,
        }
    };
}

/// Sets a oneof field from [`ranged`].
macro_rules! ranged_oneof {
    ($co:ident . $field:ident, $variant:path, $json:expr, $key:literal, $range:expr, $warnings:expr) => {
        $co.$field = Some($variant($crate::costing_options::base::ranged(
            $json,
            $key,
            $range,
            oneof!($co.$field.clone(), $variant),
            $warnings,
        )));
    };
}

pub(crate) use {oneof, ranged_oneof};

/// The defaults (and which options to parse) for [`parse_base_options`]
/// (mainline's `BaseCostingOptionsConfig`).
///
/// Each costing starts from [`BaseConfig::DEFAULT`] and overrides some of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaseConfig {
    pub(crate) dest_only_penalty: RangedDefault<f32>,
    pub(crate) maneuver_penalty: RangedDefault<f32>,
    pub(crate) alley_penalty: RangedDefault<f32>,
    pub(crate) gate_cost: RangedDefault<f32>,
    pub(crate) gate_penalty: RangedDefault<f32>,
    pub(crate) private_access_penalty: RangedDefault<f32>,
    pub(crate) country_crossing_cost: RangedDefault<f32>,
    pub(crate) country_crossing_penalty: RangedDefault<f32>,
    pub(crate) disable_toll_booth: bool,
    pub(crate) toll_booth_cost: RangedDefault<f32>,
    pub(crate) toll_booth_penalty: RangedDefault<f32>,
    pub(crate) disable_ferry: bool,
    pub(crate) ferry_cost: RangedDefault<f32>,
    pub(crate) use_ferry: RangedDefault<f32>,
    pub(crate) disable_rail_ferry: bool,
    pub(crate) rail_ferry_cost: RangedDefault<f32>,
    pub(crate) use_rail_ferry: RangedDefault<f32>,
    pub(crate) service_penalty: RangedDefault<f32>,
    pub(crate) service_factor: RangedDefault<f32>,
    pub(crate) use_tracks: RangedDefault<f32>,
    pub(crate) use_living_streets: RangedDefault<f32>,
    pub(crate) use_lit: RangedDefault<f32>,
    pub(crate) closure_factor: RangedDefault<f32>,
    pub(crate) speed_penalty_factor: RangedDefault<f32>,
    pub(crate) height: RangedDefault<f32>,
    pub(crate) width: RangedDefault<f32>,
    pub(crate) length: RangedDefault<f32>,
    pub(crate) weight: RangedDefault<f32>,
    pub(crate) use_distance: RangedDefault<f32>,
}

impl BaseConfig {
    /// Mainline's `BaseCostingOptionsConfig()` constructor.
    ///
    /// Every `exclude_*` and `include_*` default is false, so those are not configurable here.
    pub(crate) const DEFAULT: BaseConfig = BaseConfig {
        dest_only_penalty: RangedDefault::new(0.0, 600.0, MAX_PENALTY),
        maneuver_penalty: RangedDefault::new(0.0, 5.0, MAX_PENALTY),
        alley_penalty: RangedDefault::new(0.0, 5.0, MAX_PENALTY),
        gate_cost: RangedDefault::new(0.0, 30.0, MAX_PENALTY),
        gate_penalty: RangedDefault::new(0.0, 300.0, MAX_PENALTY),
        private_access_penalty: RangedDefault::new(0.0, 450.0, MAX_PENALTY),
        country_crossing_cost: RangedDefault::new(0.0, 600.0, MAX_PENALTY),
        country_crossing_penalty: RangedDefault::new(0.0, 0.0, MAX_PENALTY),
        disable_toll_booth: false,
        toll_booth_cost: RangedDefault::new(0.0, 15.0, MAX_PENALTY),
        toll_booth_penalty: RangedDefault::new(0.0, 0.0, MAX_PENALTY),
        disable_ferry: false,
        ferry_cost: RangedDefault::new(0.0, 300.0, MAX_PENALTY),
        use_ferry: RangedDefault::new(0.0, 0.5, 1.0),
        disable_rail_ferry: false,
        rail_ferry_cost: RangedDefault::new(0.0, 300.0, MAX_PENALTY),
        use_rail_ferry: RangedDefault::new(0.0, 0.4, 1.0),
        service_penalty: RangedDefault::new(0.0, 15.0, MAX_PENALTY),
        service_factor: RangedDefault::new(MIN_FACTOR, 1.0, MAX_FACTOR),
        use_tracks: RangedDefault::new(0.0, 0.5, 1.0),
        use_living_streets: RangedDefault::new(0.0, 0.1, 1.0),
        use_lit: RangedDefault::new(0.0, 0.0, 1.0),
        closure_factor: RangedDefault::new(1.0, 9.0, 10.0),
        speed_penalty_factor: RangedDefault::new(0.0, 0.05, 1.0),
        height: RangedDefault::new(0.0, 1.6, 10.0),
        width: RangedDefault::new(0.0, 1.9, 10.0),
        length: RangedDefault::new(0.0, 2.7, 50.0),
        weight: RangedDefault::new(0.0, 0.8, 100.0),
        use_distance: RangedDefault::new(0.0, 0.0, 1.0),
    };
}

/// The speed sources named in `speed_types` (mainline's `SpeedMask_Parse`).
fn speed_mask(speed_types: Option<&JsonValue>) -> u32 {
    match speed_types {
        Some(JsonValue::Array(types)) => types
            .iter()
            .filter_map(|t| match t {
                JsonValue::String(name) => match name.as_str() {
                    "freeflow" => Some(1),
                    "constrained" => Some(2),
                    "predicted" => Some(4),
                    "current" => Some(8),
                    _ => None,
                },
                _ => None,
            })
            .fold(0, |mask, bit| mask | bit),
        _ => DEFAULT_FLOW_MASK,
    }
}

/// The routing graph's hierarchy levels (highway, arterial, and local).
const HIERARCHY_LEVELS: [u32; 3] = [0, 1, 2];

/// Parses the options common to most costings from `json`, the costing's options object
/// (mainline's `ParseBaseCostOptions`).
///
/// Values already in `costing` (from a PBF request) are kept unless the JSON overrides them.
#[expect(
    clippy::too_many_lines,
    reason = "this follows mainline's function option by option, which is easier to check as one"
)]
pub(crate) fn parse_base_options(
    json: &JsonValue,
    costing: &mut Costing,
    cfg: &BaseConfig,
    warnings: &mut Vec<CodedDescription>,
) {
    let co = costing.has_options_mut();

    // Ignore bogus input.
    if oneof!(co.has_flow_mask, HasFlowMask::FlowMask).is_some_and(|mask| mask > DEFAULT_FLOW_MASK)
    {
        co.has_flow_mask = None;
    }
    // Defer to the JSON, or the default if no PBF value is present.
    let speed_types = json.pointer("/speed_types");
    if speed_types.is_some() || co.has_flow_mask.is_none() {
        co.has_flow_mask = Some(HasFlowMask::FlowMask(speed_mask(speed_types)));
    }

    if let Some(name) = json.get::<String>("/name") {
        costing.has_name = Some(HasName::Name(name));
    }
    let co = costing.has_options_mut();

    co.ignore_restrictions = flag(json, "/ignore_restrictions", co.ignore_restrictions, false);
    co.ignore_oneways = flag(json, "/ignore_oneways", co.ignore_oneways, false);
    co.ignore_access = flag(json, "/ignore_access", co.ignore_access, false);
    // `ignore_closures` is a oneof, but `JSON_PBF_DEFAULT_V2` reads it like a plain field.
    let ignore_closures = oneof!(co.has_ignore_closures, HasIgnoreClosures::IgnoreClosures);
    co.has_ignore_closures = Some(HasIgnoreClosures::IgnoreClosures(flag(
        json,
        "/ignore_closures",
        ignore_closures.unwrap_or(false),
        false,
    )));
    co.ignore_construction = flag(json, "/ignore_construction", co.ignore_construction, false);
    co.ignore_non_vehicular_restrictions = flag(
        json,
        "/ignore_non_vehicular_restrictions",
        co.ignore_non_vehicular_restrictions,
        false,
    );
    co.shortest = flag(json, "/shortest", co.shortest, false);
    co.disable_hierarchy_pruning =
        json.get_or("/disable_hierarchy_pruning", co.disable_hierarchy_pruning);

    for level in HIERARCHY_LEVELS {
        let max_up_transitions: u32 = json.get_or(
            &format!("/hierarchy_limits/{level}/max_up_transitions"),
            u32::MAX,
        );
        let expand_within_dist: f32 = json.get_or(
            &format!("/hierarchy_limits/{level}/expand_within_distance"),
            f32::MAX,
        );
        // Don't set anything if the user sent nothing.
        #[expect(clippy::float_cmp, reason = "comparing against the sentinel exactly")]
        if max_up_transitions == u32::MAX && expand_within_dist == f32::MAX {
            continue;
        }
        // Like protobuf's `Map::insert`, this keeps a value that is already there.
        co.hierarchy_limits.entry(level).or_insert(HierarchyLimits {
            up_transition_count: 0,
            max_up_transitions,
            expand_within_dist,
        });
    }

    ranged_oneof!(
        co.has_destination_only_penalty,
        HasDestinationOnlyPenalty::DestinationOnlyPenalty,
        json,
        "/destination_only_penalty",
        cfg.dest_only_penalty,
        warnings
    );
    ranged_oneof!(
        co.has_maneuver_penalty,
        HasManeuverPenalty::ManeuverPenalty,
        json,
        "/maneuver_penalty",
        cfg.maneuver_penalty,
        warnings
    );
    ranged_oneof!(
        co.has_alley_penalty,
        HasAlleyPenalty::AlleyPenalty,
        json,
        "/alley_penalty",
        cfg.alley_penalty,
        warnings
    );
    ranged_oneof!(
        co.has_gate_cost,
        HasGateCost::GateCost,
        json,
        "/gate_cost",
        cfg.gate_cost,
        warnings
    );
    ranged_oneof!(
        co.has_gate_penalty,
        HasGatePenalty::GatePenalty,
        json,
        "/gate_penalty",
        cfg.gate_penalty,
        warnings
    );
    ranged_oneof!(
        co.has_private_access_penalty,
        HasPrivateAccessPenalty::PrivateAccessPenalty,
        json,
        "/private_access_penalty",
        cfg.private_access_penalty,
        warnings
    );
    ranged_oneof!(
        co.has_country_crossing_cost,
        HasCountryCrossingCost::CountryCrossingCost,
        json,
        "/country_crossing_cost",
        cfg.country_crossing_cost,
        warnings
    );
    ranged_oneof!(
        co.has_country_crossing_penalty,
        HasCountryCrossingPenalty::CountryCrossingPenalty,
        json,
        "/country_crossing_penalty",
        cfg.country_crossing_penalty,
        warnings
    );

    if !cfg.disable_toll_booth {
        ranged_oneof!(
            co.has_toll_booth_cost,
            HasTollBoothCost::TollBoothCost,
            json,
            "/toll_booth_cost",
            cfg.toll_booth_cost,
            warnings
        );
        ranged_oneof!(
            co.has_toll_booth_penalty,
            HasTollBoothPenalty::TollBoothPenalty,
            json,
            "/toll_booth_penalty",
            cfg.toll_booth_penalty,
            warnings
        );
    }
    if !cfg.disable_ferry {
        ranged_oneof!(
            co.has_ferry_cost,
            HasFerryCost::FerryCost,
            json,
            "/ferry_cost",
            cfg.ferry_cost,
            warnings
        );
        ranged_oneof!(
            co.has_use_ferry,
            HasUseFerry::UseFerry,
            json,
            "/use_ferry",
            cfg.use_ferry,
            warnings
        );
    }
    if !cfg.disable_rail_ferry {
        ranged_oneof!(
            co.has_rail_ferry_cost,
            HasRailFerryCost::RailFerryCost,
            json,
            "/rail_ferry_cost",
            cfg.rail_ferry_cost,
            warnings
        );
        ranged_oneof!(
            co.has_use_rail_ferry,
            HasUseRailFerry::UseRailFerry,
            json,
            "/use_rail_ferry",
            cfg.use_rail_ferry,
            warnings
        );
    }

    co.exclude_unpaved = flag(json, "/exclude_unpaved", co.exclude_unpaved, false);
    co.exclude_bridges = flag(json, "/exclude_bridges", co.exclude_bridges, false);
    co.exclude_tunnels = flag(json, "/exclude_tunnels", co.exclude_tunnels, false);
    co.exclude_tolls = flag(json, "/exclude_tolls", co.exclude_tolls, false);
    co.exclude_highways = flag(json, "/exclude_highways", co.exclude_highways, false);
    co.exclude_ferries = flag(json, "/exclude_ferries", co.exclude_ferries, false);
    co.exclude_cash_only_tolls = flag(
        json,
        "/exclude_cash_only_tolls",
        co.exclude_cash_only_tolls,
        false,
    );

    ranged_oneof!(
        co.has_service_penalty,
        HasServicePenalty::ServicePenalty,
        json,
        "/service_penalty",
        cfg.service_penalty,
        warnings
    );
    ranged_oneof!(
        co.has_use_distance,
        HasUseDistance::UseDistance,
        json,
        "/use_distance",
        cfg.use_distance,
        warnings
    );
    ranged_oneof!(
        co.has_service_factor,
        HasServiceFactor::ServiceFactor,
        json,
        "/service_factor",
        cfg.service_factor,
        warnings
    );
    ranged_oneof!(
        co.has_use_tracks,
        HasUseTracks::UseTracks,
        json,
        "/use_tracks",
        cfg.use_tracks,
        warnings
    );
    ranged_oneof!(
        co.has_use_living_streets,
        HasUseLivingStreets::UseLivingStreets,
        json,
        "/use_living_streets",
        cfg.use_living_streets,
        warnings
    );
    co.use_lit = ranged(
        json,
        "/use_lit",
        cfg.use_lit,
        nonzero_f32(co.use_lit),
        warnings,
    );
    ranged_oneof!(
        co.has_closure_factor,
        HasClosureFactor::ClosureFactor,
        json,
        "/closure_factor",
        cfg.closure_factor,
        warnings
    );
    ranged_oneof!(
        co.has_speed_penalty_factor,
        HasSpeedPenaltyFactor::SpeedPenaltyFactor,
        json,
        "/speed_penalty_factor",
        cfg.speed_penalty_factor,
        warnings
    );

    co.include_hot = flag(json, "/include_hot", co.include_hot, false);
    co.include_hov2 = flag(json, "/include_hov2", co.include_hov2, false);
    co.include_hov3 = flag(json, "/include_hov3", co.include_hov3, false);

    co.fixed_speed = ranged(
        json,
        "/fixed_speed",
        RangedDefault::new(0, 0, MAX_SPEED_KPH),
        nonzero_u32(co.fixed_speed),
        warnings,
    );

    ranged_oneof!(
        co.has_height,
        HasHeight::Height,
        json,
        "/height",
        cfg.height,
        warnings
    );
    ranged_oneof!(
        co.has_width,
        HasWidth::Width,
        json,
        "/width",
        cfg.width,
        warnings
    );
    ranged_oneof!(
        co.has_length,
        HasLength::Length,
        json,
        "/length",
        cfg.length,
        warnings
    );
    ranged_oneof!(
        co.has_weight,
        HasWeight::Weight,
        json,
        "/weight",
        cfg.weight,
        warnings
    );
}

/// Access to a costing's options, creating them if needed (protobuf's `mutable_options`).
pub(crate) trait CostingExt {
    fn has_options_mut(&mut self) -> &mut CostingOptions;
}

impl CostingExt for Costing {
    fn has_options_mut(&mut self) -> &mut CostingOptions {
        let valhalla_proto::costing::HasOptions::Options(options) =
            self.has_options.get_or_insert_with(|| {
                valhalla_proto::costing::HasOptions::Options(CostingOptions::default())
            });
        options
    }
}
