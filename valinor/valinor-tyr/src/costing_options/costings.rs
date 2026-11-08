//! Each costing's option parser (the `Parse*CostOptions` functions in `src/sif/*cost.cc`),
//! with the defaults and valid ranges mainline 3.9.1 uses.

use crate::costing_options::base::{
    BaseConfig, CostingExt, MAX_ASSUMED_SPEED, MAX_FACTOR, MAX_PENALTY, MAX_SPEED_KPH, MIN_FACTOR,
    RangedDefault, flag, nonzero_f32, nonzero_u32, oneof, parse_base_options, ranged, ranged_oneof,
};
use crate::costing_options::costing_name;
use crate::front::json::{JsonValue, double_to_u32};
use alloc::string::String;
use alloc::vec::Vec;
use valhalla_proto::costing::HasName;
use valhalla_proto::costing::Type as CostingType;
use valhalla_proto::costing::options::{
    HasAlleyFactor, HasAvoidBadSurfaces, HasAxleLoad, HasBikeShareCost, HasBikeSharePenalty,
    HasCyclingSpeed, HasDrivewayFactor, HasElevatorPenalty, HasHgvNoAccessPenalty,
    HasLowClassFactor, HasLowClassPenalty, HasMaxDistance, HasMaxGrade, HasMaxHikingDifficulty,
    HasModeFactor, HasMultimodalStartEndMaxDistance, HasRestrictionProbability, HasSidewalkFactor,
    HasStepPenalty, HasTopSpeed, HasTransferCost, HasTransferPenalty,
    HasTransitStartEndMaxDistance, HasTransitTransferMaxDistance, HasTransportType, HasUseBus,
    HasUseHighways, HasUseHills, HasUsePrimary, HasUseRail, HasUseRoads, HasUseTolls, HasUseTrails,
    HasUseTransfers, HasWalkingSpeed, HasWalkwayFactor,
};
use valhalla_proto::{CodedDescription, Costing, FilterAction};
use valhalla_response::{ErrorCode, ValhallaError};

/// The JSON object holding a costing's options (`rapidjson::get_child(doc, key, dummy)`):
/// the value at `key`, or null if there is none.
fn options_json<'a>(doc: &'a JsonValue, key: &str) -> &'a JsonValue {
    const NULL: &JsonValue = &JsonValue::Null;
    doc.pointer(key).unwrap_or(NULL)
}

/// Sets a costing's type and its default name (`c->set_type(...); c->set_name(...)`).
fn set_type_and_name(costing: &mut Costing, costing_type: CostingType) {
    costing.r#type = costing_type.into();
    costing.has_name = Some(HasName::Name(costing_name(costing_type).into()));
}

/// Reads a top speed given as an unsigned range into the float field mainline stores it in.
///
/// Mainline reads these as `uint32_t` (truncating a PBF value through a float-to-unsigned
/// conversion) and stores the result as a float.
#[expect(
    clippy::cast_precision_loss,
    reason = "speeds are at most a few hundred, so the conversion is exact"
)]
fn unsigned_top_speed(
    co: &mut valhalla_proto::costing::Options,
    json: &JsonValue,
    range: RangedDefault<u32>,
    warnings: &mut Vec<CodedDescription>,
) {
    let current = oneof!(co.has_top_speed, HasTopSpeed::TopSpeed)
        .map(|speed| double_to_u32(f64::from(speed)));
    let speed = ranged(json, "/top_speed", range, current, warnings);
    co.has_top_speed = Some(HasTopSpeed::TopSpeed(speed as f32));
}

/// `ParseAutoCostOptions` (`src/sif/autocost.cc`).
pub(crate) fn parse_auto(
    doc: &JsonValue,
    key: &str,
    costing: &mut Costing,
    warnings: &mut Vec<CodedDescription>,
) {
    const BASE: BaseConfig = {
        let mut cfg = BaseConfig::DEFAULT;
        cfg.service_penalty.def = 75.0;
        cfg.use_tracks.def = 0.0;
        cfg
    };
    set_type_and_name(costing, CostingType::Auto);
    let json = options_json(doc, key);
    parse_base_options(json, costing, &BASE, warnings);
    let co = costing.has_options_mut();
    ranged_oneof!(
        co.has_alley_factor,
        HasAlleyFactor::AlleyFactor,
        json,
        "/alley_factor",
        RangedDefault::new(MIN_FACTOR, 1.0, MAX_FACTOR),
        warnings
    );
    ranged_oneof!(
        co.has_use_highways,
        HasUseHighways::UseHighways,
        json,
        "/use_highways",
        RangedDefault::new(0.0, 0.5, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_use_tolls,
        HasUseTolls::UseTolls,
        json,
        "/use_tolls",
        RangedDefault::new(0.0, 0.5, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_restriction_probability,
        HasRestrictionProbability::RestrictionProbability,
        json,
        "/restriction_probability",
        RangedDefault::new(0, 100, 100),
        warnings
    );
    unsigned_top_speed(
        co,
        json,
        RangedDefault::new(10, MAX_ASSUMED_SPEED, MAX_SPEED_KPH),
        warnings,
    );
}

/// `ParseBusCostOptions`: the auto options under the bus type and name.
pub(crate) fn parse_bus(
    doc: &JsonValue,
    key: &str,
    costing: &mut Costing,
    warnings: &mut Vec<CodedDescription>,
) {
    parse_auto(doc, key, costing, warnings);
    set_type_and_name(costing, CostingType::Bus);
}

/// `ParseTaxiCostOptions`: the auto options under the taxi type and name.
pub(crate) fn parse_taxi(
    doc: &JsonValue,
    key: &str,
    costing: &mut Costing,
    warnings: &mut Vec<CodedDescription>,
) {
    parse_auto(doc, key, costing, warnings);
    set_type_and_name(costing, CostingType::Taxi);
}

/// `ParseBicycleCostOptions` (`src/sif/bicyclecost.cc`).
pub(crate) fn parse_bicycle(
    doc: &JsonValue,
    key: &str,
    costing: &mut Costing,
    warnings: &mut Vec<CodedDescription>,
) {
    const BASE: BaseConfig = {
        let mut cfg = BaseConfig::DEFAULT;
        cfg.alley_penalty.def = 60.0;
        cfg.gate_penalty.def = 300.0;
        cfg.disable_toll_booth = true;
        cfg.disable_rail_ferry = true;
        cfg.use_living_streets.def = 0.5;
        cfg
    };
    set_type_and_name(costing, CostingType::Bicycle);
    let json = options_json(doc, key);
    parse_base_options(json, costing, &BASE, warnings);
    let co = costing.has_options_mut();
    ranged_oneof!(
        co.has_use_roads,
        HasUseRoads::UseRoads,
        json,
        "/use_roads",
        RangedDefault::new(0.0, 0.25, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_use_hills,
        HasUseHills::UseHills,
        json,
        "/use_hills",
        RangedDefault::new(0.0, 0.25, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_avoid_bad_surfaces,
        HasAvoidBadSurfaces::AvoidBadSurfaces,
        json,
        "/avoid_bad_surfaces",
        RangedDefault::new(0.0, 0.25, 1.0),
        warnings
    );
    let transport_type = transport_type(co, json, "/bicycle_type", "hybrid");
    // Road, cross, hybrid (the fallback), and mountain bicycles have different default speeds.
    let default_speed = match transport_type.as_str() {
        "road" => 25.0,
        "cross" => 20.0,
        "mountain" => 16.0,
        _ => 18.0,
    };
    ranged_oneof!(
        co.has_cycling_speed,
        HasCyclingSpeed::CyclingSpeed,
        json,
        "/cycling_speed",
        RangedDefault::new(5.0, default_speed, 60.0),
        warnings
    );
    ranged_oneof!(
        co.has_bike_share_cost,
        HasBikeShareCost::BikeShareCost,
        json,
        "/bss_return_cost",
        RangedDefault::new(0.0, 120.0, MAX_PENALTY),
        warnings
    );
    ranged_oneof!(
        co.has_bike_share_penalty,
        HasBikeSharePenalty::BikeSharePenalty,
        json,
        "/bss_return_penalty",
        RangedDefault::new(0.0, 0.0, MAX_PENALTY),
        warnings
    );
}

/// `JSON_PBF_DEFAULT` for the transport type, lowercased as mainline does (ASCII only).
fn transport_type(
    co: &mut valhalla_proto::costing::Options,
    json: &JsonValue,
    key: &str,
    default: &str,
) -> String {
    let current = oneof!(
        co.has_transport_type.clone(),
        HasTransportType::TransportType
    );
    let mut value: String = json.get_or(key, current.unwrap_or_else(|| default.into()));
    value.make_ascii_lowercase();
    co.has_transport_type = Some(HasTransportType::TransportType(value.clone()));
    value
}

/// `ParsePedestrianCostOptions` (`src/sif/pedestriancost.cc`).
#[expect(
    clippy::too_many_lines,
    reason = "this follows mainline's function option by option, which is easier to check as one"
)]
pub(crate) fn parse_pedestrian(
    doc: &JsonValue,
    key: &str,
    costing: &mut Costing,
    warnings: &mut Vec<CodedDescription>,
) {
    const BASE: BaseConfig = {
        let mut cfg = BaseConfig::DEFAULT;
        cfg.gate_penalty.def = 10.0;
        cfg.private_access_penalty.def = 600.0;
        cfg.disable_toll_booth = true;
        cfg.disable_rail_ferry = true;
        cfg.service_penalty.def = 0.0;
        cfg.use_ferry.def = 1.0;
        cfg.use_living_streets.def = 0.6;
        cfg.use_lit.def = 0.0;
        cfg
    };
    const MAX_DISTANCE_FOOT: u32 = 100_000;
    const MAX_GRADE_FOOT: u32 = 90;
    const MULTIMODAL_START_END_MAX_DISTANCE: RangedDefault<u32> =
        RangedDefault::new(0, 2415, 100_000);

    set_type_and_name(costing, CostingType::Pedestrian);
    let json = options_json(doc, key);
    parse_base_options(json, costing, &BASE, warnings);
    let co = costing.has_options_mut();
    let transport_type = transport_type(co, json, "/type", "foot");

    let (max_distance, walking_speed, step_penalty, max_grade) = if transport_type == "wheelchair" {
        (
            RangedDefault::new(0, 10_000, MAX_DISTANCE_FOOT),
            RangedDefault::new(0.5, 4.0, 25.0),
            RangedDefault::new(0.0, 600.0, MAX_PENALTY),
            RangedDefault::new(0, 12, MAX_GRADE_FOOT),
        )
    } else {
        (
            RangedDefault::new(0, MAX_DISTANCE_FOOT, MAX_DISTANCE_FOOT),
            RangedDefault::new(0.5, 5.1, 25.0),
            RangedDefault::new(0.0, 30.0, MAX_PENALTY),
            RangedDefault::new(0, MAX_GRADE_FOOT, MAX_GRADE_FOOT),
        )
    };
    ranged_oneof!(
        co.has_max_distance,
        HasMaxDistance::MaxDistance,
        json,
        "/max_distance",
        max_distance,
        warnings
    );
    ranged_oneof!(
        co.has_walking_speed,
        HasWalkingSpeed::WalkingSpeed,
        json,
        "/walking_speed",
        walking_speed,
        warnings
    );
    ranged_oneof!(
        co.has_step_penalty,
        HasStepPenalty::StepPenalty,
        json,
        "/step_penalty",
        step_penalty,
        warnings
    );
    ranged_oneof!(
        co.has_max_grade,
        HasMaxGrade::MaxGrade,
        json,
        "/max_grade",
        max_grade,
        warnings
    );
    ranged_oneof!(
        co.has_max_hiking_difficulty,
        HasMaxHikingDifficulty::MaxHikingDifficulty,
        json,
        "/max_hiking_difficulty",
        RangedDefault::new(0, 1, 6),
        warnings
    );
    ranged_oneof!(
        co.has_mode_factor,
        HasModeFactor::ModeFactor,
        json,
        "/mode_factor",
        RangedDefault::new(MIN_FACTOR, 1.5, MAX_FACTOR),
        warnings
    );
    ranged_oneof!(
        co.has_walkway_factor,
        HasWalkwayFactor::WalkwayFactor,
        json,
        "/walkway_factor",
        RangedDefault::new(MIN_FACTOR, 1.0, MAX_FACTOR),
        warnings
    );
    ranged_oneof!(
        co.has_sidewalk_factor,
        HasSidewalkFactor::SidewalkFactor,
        json,
        "/sidewalk_factor",
        RangedDefault::new(MIN_FACTOR, 1.0, MAX_FACTOR),
        warnings
    );
    ranged_oneof!(
        co.has_alley_factor,
        HasAlleyFactor::AlleyFactor,
        json,
        "/alley_factor",
        RangedDefault::new(MIN_FACTOR, 2.0, MAX_FACTOR),
        warnings
    );
    ranged_oneof!(
        co.has_driveway_factor,
        HasDrivewayFactor::DrivewayFactor,
        json,
        "/driveway_factor",
        RangedDefault::new(MIN_FACTOR, 5.0, MAX_FACTOR),
        warnings
    );
    // Mainline checks the deprecated option against the range of its replacement.
    ranged_oneof!(
        co.has_transit_start_end_max_distance,
        HasTransitStartEndMaxDistance::TransitStartEndMaxDistance,
        json,
        "/transit_start_end_max_distance",
        MULTIMODAL_START_END_MAX_DISTANCE,
        warnings
    );
    ranged_oneof!(
        co.has_transit_transfer_max_distance,
        HasTransitTransferMaxDistance::TransitTransferMaxDistance,
        json,
        "/transit_transfer_max_distance",
        RangedDefault::new(0, 805, 50_000),
        warnings
    );
    ranged_oneof!(
        co.has_bike_share_cost,
        HasBikeShareCost::BikeShareCost,
        json,
        "/bss_rent_cost",
        RangedDefault::new(0.0, 120.0, MAX_PENALTY),
        warnings
    );
    ranged_oneof!(
        co.has_bike_share_penalty,
        HasBikeSharePenalty::BikeSharePenalty,
        json,
        "/bss_rent_penalty",
        RangedDefault::new(0.0, 0.0, MAX_PENALTY),
        warnings
    );
    ranged_oneof!(
        co.has_use_hills,
        HasUseHills::UseHills,
        json,
        "/use_hills",
        RangedDefault::new(0.0, 0.5, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_elevator_penalty,
        HasElevatorPenalty::ElevatorPenalty,
        json,
        "/elevator_penalty",
        RangedDefault::new(0.0, 5.0, MAX_PENALTY),
        warnings
    );
    ranged_oneof!(
        co.has_multimodal_start_end_max_distance,
        HasMultimodalStartEndMaxDistance::MultimodalStartEndMaxDistance,
        json,
        "/multimodal_start_end_max_distance",
        MULTIMODAL_START_END_MAX_DISTANCE,
        warnings
    );
}

/// `ParseMotorScooterCostOptions` (`src/sif/motorscootercost.cc`).
pub(crate) fn parse_motor_scooter(
    doc: &JsonValue,
    key: &str,
    costing: &mut Costing,
    warnings: &mut Vec<CodedDescription>,
) {
    const BASE: BaseConfig = {
        let mut cfg = BaseConfig::DEFAULT;
        cfg.dest_only_penalty.def = 120.0;
        cfg.disable_toll_booth = true;
        cfg.disable_rail_ferry = true;
        cfg
    };
    set_type_and_name(costing, CostingType::MotorScooter);
    let json = options_json(doc, key);
    parse_base_options(json, costing, &BASE, warnings);
    let co = costing.has_options_mut();
    unsigned_top_speed(co, json, RangedDefault::new(20, 45, 120), warnings);
    ranged_oneof!(
        co.has_use_hills,
        HasUseHills::UseHills,
        json,
        "/use_hills",
        RangedDefault::new(0.0, 0.5, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_use_primary,
        HasUsePrimary::UsePrimary,
        json,
        "/use_primary",
        RangedDefault::new(0.0, 0.5, 1.0),
        warnings
    );
}

/// `ParseMotorcycleCostOptions` (`src/sif/motorcyclecost.cc`).
pub(crate) fn parse_motorcycle(
    doc: &JsonValue,
    key: &str,
    costing: &mut Costing,
    warnings: &mut Vec<CodedDescription>,
) {
    const BASE: BaseConfig = {
        let mut cfg = BaseConfig::DEFAULT;
        cfg.disable_rail_ferry = true;
        cfg
    };
    set_type_and_name(costing, CostingType::Motorcycle);
    let json = options_json(doc, key);
    parse_base_options(json, costing, &BASE, warnings);
    let co = costing.has_options_mut();
    ranged_oneof!(
        co.has_use_highways,
        HasUseHighways::UseHighways,
        json,
        "/use_highways",
        RangedDefault::new(0.0, 0.5, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_use_tolls,
        HasUseTolls::UseTolls,
        json,
        "/use_tolls",
        RangedDefault::new(0.0, 0.5, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_use_trails,
        HasUseTrails::UseTrails,
        json,
        "/use_trails",
        RangedDefault::new(0.0, 0.0, 1.0),
        warnings
    );
    unsigned_top_speed(
        co,
        json,
        RangedDefault::new(10, MAX_ASSUMED_SPEED, MAX_SPEED_KPH),
        warnings,
    );
}

/// `ParseTruckCostOptions` (`src/sif/truckcost.cc`).
pub(crate) fn parse_truck(
    doc: &JsonValue,
    key: &str,
    costing: &mut Costing,
    warnings: &mut Vec<CodedDescription>,
) {
    const BASE: BaseConfig = {
        let mut cfg = BaseConfig::DEFAULT;
        cfg.service_penalty.def = 0.0;
        cfg.use_tracks.def = 0.0;
        cfg.use_living_streets.def = 0.0;
        cfg.height.def = 4.11;
        cfg.width.def = 2.6;
        cfg.length.def = 21.64;
        cfg.weight.def = 21.77;
        cfg
    };
    /// `kMaxAssumedTruckSpeed`.
    const MAX_ASSUMED_TRUCK_SPEED: f32 = 120.0;
    #[expect(
        clippy::cast_precision_loss,
        reason = "the maximum speed is small enough to be exact"
    )]
    const MAX_SPEED: f32 = MAX_SPEED_KPH as f32;

    set_type_and_name(costing, CostingType::Truck);
    let json = options_json(doc, key);
    parse_base_options(json, costing, &BASE, warnings);
    let co = costing.has_options_mut();
    ranged_oneof!(
        co.has_low_class_penalty,
        HasLowClassPenalty::LowClassPenalty,
        json,
        "/low_class_penalty",
        RangedDefault::new(0.0, 15.0, MAX_PENALTY),
        warnings
    );
    ranged_oneof!(
        co.has_low_class_factor,
        HasLowClassFactor::LowClassFactor,
        json,
        "/low_class_factor",
        RangedDefault::new(MIN_FACTOR, 1.5, MAX_FACTOR),
        warnings
    );
    co.hazmat = flag(json, "/hazmat", co.hazmat, false);
    ranged_oneof!(
        co.has_axle_load,
        HasAxleLoad::AxleLoad,
        json,
        "/axle_load",
        RangedDefault::new(0.0, 9.07, 40.0),
        warnings
    );
    ranged_oneof!(
        co.has_use_tolls,
        HasUseTolls::UseTolls,
        json,
        "/use_tolls",
        RangedDefault::new(0.0, 0.5, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_use_highways,
        HasUseHighways::UseHighways,
        json,
        "/use_highways",
        RangedDefault::new(0.0, 0.5, 1.0),
        warnings
    );
    co.axle_count = ranged(
        json,
        "/axle_count",
        RangedDefault::new(2, 5, 20),
        nonzero_u32(co.axle_count),
        warnings,
    );
    ranged_oneof!(
        co.has_top_speed,
        HasTopSpeed::TopSpeed,
        json,
        "/top_speed",
        RangedDefault::new(10.0, MAX_ASSUMED_TRUCK_SPEED, MAX_SPEED),
        warnings
    );
    ranged_oneof!(
        co.has_hgv_no_access_penalty,
        HasHgvNoAccessPenalty::HgvNoAccessPenalty,
        json,
        "/hgv_no_access_penalty",
        RangedDefault::new(0.0, MAX_PENALTY, MAX_PENALTY),
        warnings
    );
    co.use_truck_route = ranged(
        json,
        "/use_truck_route",
        RangedDefault::new(0.0, 0.0, 1.0),
        nonzero_f32(co.use_truck_route),
        warnings,
    );
}

/// `ParseTransitCostOptions` (`src/sif/transitcost.cc`).
///
/// Transit parses none of the base options.
///
/// # Errors
///
/// A filter's `ids` that are not all strings fail rapidjson's `GetString` assertion,
/// which mainline reports as an unknown Loki error (199).
pub(crate) fn parse_transit(
    doc: &JsonValue,
    key: &str,
    costing: &mut Costing,
    warnings: &mut Vec<CodedDescription>,
) -> Result<(), ValhallaError> {
    set_type_and_name(costing, CostingType::Transit);
    let json = options_json(doc, key);
    let co = costing.has_options_mut();
    ranged_oneof!(
        co.has_mode_factor,
        HasModeFactor::ModeFactor,
        json,
        "/mode_factor",
        RangedDefault::new(MIN_FACTOR, 1.0, MAX_FACTOR),
        warnings
    );
    co.wheelchair = flag(json, "/wheelchair", co.wheelchair, false);
    co.bicycle = flag(json, "/bicycle", co.bicycle, false);
    ranged_oneof!(
        co.has_use_bus,
        HasUseBus::UseBus,
        json,
        "/use_bus",
        RangedDefault::new(0.0, 0.3, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_use_rail,
        HasUseRail::UseRail,
        json,
        "/use_rail",
        RangedDefault::new(0.0, 0.6, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_use_transfers,
        HasUseTransfers::UseTransfers,
        json,
        "/use_transfers",
        RangedDefault::new(0.0, 0.3, 1.0),
        warnings
    );
    ranged_oneof!(
        co.has_transfer_cost,
        HasTransferCost::TransferCost,
        json,
        "/transfer_cost",
        RangedDefault::new(0.0, 15.0, MAX_PENALTY),
        warnings
    );
    ranged_oneof!(
        co.has_transfer_penalty,
        HasTransferPenalty::TransferPenalty,
        json,
        "/transfer_penalty",
        RangedDefault::new(0.0, 300.0, MAX_PENALTY),
        warnings
    );

    for (filter, action, ids) in [
        ("stops", &mut co.filter_stop_action, &mut co.filter_stop_ids),
        (
            "operators",
            &mut co.filter_operator_action,
            &mut co.filter_operator_ids,
        ),
        (
            "routes",
            &mut co.filter_route_action,
            &mut co.filter_route_ids,
        ),
    ] {
        let Some(filter_action) = json
            .get::<String>(&alloc::format!("/filters/{filter}/action"))
            .as_deref()
            .and_then(parse_filter_action)
        else {
            continue;
        };
        *action = filter_action.into();
        if let Some(json_ids) = json.get_array(&alloc::format!("/filters/{filter}/ids")) {
            ids.clear();
            for id in json_ids {
                match id {
                    JsonValue::String(id) => ids.push(id.clone()),
                    _ => {
                        return Err(ValhallaError::with_detail(
                            ErrorCode::LokiUnknown,
                            "IsString()",
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Mainline's `FilterAction_Enum_Parse`.
pub(crate) fn parse_filter_action(name: &str) -> Option<FilterAction> {
    match name {
        "exclude" => Some(FilterAction::Exclude),
        "include" => Some(FilterAction::Include),
        _ => None,
    }
}

/// `ParseNoCostOptions` (`src/sif/nocost.cc`), which only sets the type and name.
pub(crate) fn parse_none(costing: &mut Costing) {
    set_type_and_name(costing, CostingType::None);
}
