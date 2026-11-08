//! Costing options: parsing them from a request and filling in mainline's defaults.
//!
//! In mainline this lives in Sif (`sif::ParseCosting`, `ParseBaseCostOptions`,
//! and each costing's `Parse*CostOptions`), but only the front calls it,
//! so in Valinor it lives here, next to the rest of the request parsing.
//! It depends on nothing but the protobuf types and [`JsonValue`],
//! so the Sif crate can reuse it as is
//! (for example [`parse_costing`] with an empty document gives a costing's defaults).
//!
//! Every value is read the way mainline reads it
//! (see [`crate::front::json`] for the conversions),
//! and an out-of-range value is replaced by the default with a warning (code 500),
//! as mainline's `JSON_PBF_RANGED_DEFAULT` macros do.
//! Values already present in the [`Costing`] (from a PBF request) are kept
//! unless the JSON gives a new one.

mod base;
mod costings;

pub use base::{DEFAULT_FLOW_MASK, RangedDefault, TIME_DEPENDENT_FLOW_MASK};
pub(crate) use costings::parse_filter_action;

use crate::front::json::JsonValue;
use alloc::format;
use alloc::vec::Vec;
use valhalla_proto::costing::Type as CostingType;
use valhalla_proto::{CodedDescription, Costing, Options};
use valhalla_response::{ErrorCode, ValhallaError};

/// Mainline's `Costing_Enum_Parse`: the costing for a request's `costing` name.
///
/// The empty string means no costing, like `none`.
/// The deprecated names (`auto_shorter`, `hov`, and `auto_data_fix`) are not costings;
/// the front rewrites them to `auto` before it gets here.
pub fn parse_costing_name(name: &str) -> Option<CostingType> {
    Some(match name {
        "auto" => CostingType::Auto,
        "bicycle" => CostingType::Bicycle,
        "bus" => CostingType::Bus,
        "taxi" => CostingType::Taxi,
        "motor_scooter" => CostingType::MotorScooter,
        "multimodal" => CostingType::Multimodal,
        "pedestrian" => CostingType::Pedestrian,
        "transit" => CostingType::Transit,
        "truck" => CostingType::Truck,
        "motorcycle" => CostingType::Motorcycle,
        "none" | "" => CostingType::None,
        "bikeshare" => CostingType::Bikeshare,
        "auto_pedestrian" => CostingType::AutoPedestrian,
        _ => return None,
    })
}

/// Mainline's `Costing_Enum_Name`: the name of a costing, as requests and responses spell it.
pub const fn costing_name(costing: CostingType) -> &'static str {
    match costing {
        CostingType::Auto => "auto",
        CostingType::Bicycle => "bicycle",
        CostingType::Bus => "bus",
        CostingType::Taxi => "taxi",
        CostingType::MotorScooter => "motor_scooter",
        CostingType::Multimodal => "multimodal",
        CostingType::Pedestrian => "pedestrian",
        CostingType::Transit => "transit",
        CostingType::Truck => "truck",
        CostingType::Motorcycle => "motorcycle",
        CostingType::None => "none",
        CostingType::Bikeshare => "bikeshare",
        CostingType::AutoPedestrian => "auto_pedestrian",
    }
}

/// The costings a request with this costing needs options for, in mainline's order
/// (`kCostingTypeMapping` in `sif/dynamiccost.h`).
///
/// The combined costings also need the options of the costings they combine.
pub const fn required_costings(costing: CostingType) -> &'static [CostingType] {
    match costing {
        CostingType::None => &[CostingType::None],
        CostingType::Bicycle => &[CostingType::Bicycle],
        CostingType::Bus => &[CostingType::Bus],
        CostingType::MotorScooter => &[CostingType::MotorScooter],
        CostingType::Multimodal => &[
            CostingType::Multimodal,
            CostingType::Transit,
            CostingType::Pedestrian,
        ],
        CostingType::Pedestrian => &[CostingType::Pedestrian],
        CostingType::Transit => &[CostingType::Transit, CostingType::Pedestrian],
        CostingType::Truck => &[CostingType::Truck],
        CostingType::Motorcycle => &[CostingType::Motorcycle],
        CostingType::Taxi => &[CostingType::Taxi],
        CostingType::Auto => &[CostingType::Auto],
        CostingType::Bikeshare => &[
            CostingType::Bikeshare,
            CostingType::Pedestrian,
            CostingType::Bicycle,
        ],
        CostingType::AutoPedestrian => &[
            CostingType::AutoPedestrian,
            CostingType::Pedestrian,
            CostingType::Auto,
        ],
    }
}

/// Parses the options of every costing `options.costing_type` needs into `options.costings`
/// (mainline's `sif::ParseCosting(doc, key, options, warnings)`).
///
/// Each costing's options are read from `<costing_options_key>/<costing name>`,
/// for example `/costing_options/auto`.
///
/// # Errors
///
/// Only transit costing can fail (see [`parse_costing`]).
pub fn parse_costings(
    doc: &JsonValue,
    costing_options_key: &str,
    options: &mut Options,
    warnings: &mut Vec<CodedDescription>,
) -> Result<(), ValhallaError> {
    let costing_type = CostingType::try_from(options.costing_type).unwrap_or(CostingType::None);
    for &costing_type in required_costings(costing_type) {
        let key = format!("{costing_options_key}/{}", costing_name(costing_type));
        let costing = options.costings.entry(costing_type.into()).or_default();
        parse_costing(doc, &key, costing, warnings, Some(costing_type))?;
    }
    Ok(())
}

/// Parses one costing's options from the object at `key`
/// (mainline's `sif::ParseCosting(doc, key, costing, warnings, costing_type)`).
///
/// Without a `costing_type`, the object must name its costing in a `costing` member,
/// as recostings do.
///
/// # Errors
///
/// - 127 if no `costing_type` is given and the object has no string `costing` member.
/// - 125 if that member is not a costing name.
/// - 199 (as mainline reports a failed rapidjson assertion)
///   if a transit filter's `ids` are not all strings.
pub fn parse_costing(
    doc: &JsonValue,
    key: &str,
    costing: &mut Costing,
    warnings: &mut Vec<CodedDescription>,
    costing_type: Option<CostingType>,
) -> Result<(), ValhallaError> {
    let costing_type = if let Some(costing_type) = costing_type {
        costing_type
    } else {
        let Some(JsonValue::String(name)) =
            doc.pointer(key).and_then(|json| json.member("costing"))
        else {
            return Err(ErrorCode::RecostingCostingRequired.into());
        };
        parse_costing_name(name).ok_or_else(|| {
            ValhallaError::with_detail(ErrorCode::UnknownCosting, format!("'{name}'"))
        })?
    };
    match costing_type {
        CostingType::Auto => costings::parse_auto(doc, key, costing, warnings),
        CostingType::Bicycle => costings::parse_bicycle(doc, key, costing, warnings),
        CostingType::Bus => costings::parse_bus(doc, key, costing, warnings),
        CostingType::Taxi => costings::parse_taxi(doc, key, costing, warnings),
        CostingType::MotorScooter => costings::parse_motor_scooter(doc, key, costing, warnings),
        CostingType::Pedestrian => costings::parse_pedestrian(doc, key, costing, warnings),
        CostingType::Transit => costings::parse_transit(doc, key, costing, warnings)?,
        CostingType::Truck => costings::parse_truck(doc, key, costing, warnings),
        CostingType::Motorcycle => costings::parse_motorcycle(doc, key, costing, warnings),
        CostingType::None => costings::parse_none(costing),
        // Nothing to parse for these: they combine the options of other costings.
        CostingType::Multimodal | CostingType::Bikeshare | CostingType::AutoPedestrian => {}
    }
    costing.r#type = costing_type.into();
    Ok(())
}

#[cfg(test)]
mod tests;
