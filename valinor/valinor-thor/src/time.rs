//! Times at locations: where a search's time starts, and the times Thor writes back.
//!
//! - [`location_time_info`] is mainline's `TimeInfo::make(location, reader)`:
//!   the time zone comes from the first candidate edge whose end node has one,
//!   and `current` is replaced by the current time (to the minute) in that zone.
//! - [`offset_date`] is mainline's `DateTime::offset_date`, which moves a location's
//!   `date_time` along a leg and formats it in another time zone.
//!   As in mainline, it counts leap seconds when reading the time but not when writing it,
//!   so a time comes out 27 seconds late (and then is cut to the minute).

use jiff::Timestamp;
use jiff::tz::TimeZone;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::Location;
use valinor_sif::TimeInfo;
use valinor_sif::datetime::{time_zone, time_zone_name};
use valinor_sif::time_info::UTC_TIME_ZONE_INDEX;

use crate::error::ThorError;
use crate::graph;

/// A candidate edge's graph ID.
pub(crate) fn graph_id(raw: u64) -> Result<GraphId, ThorError> {
    GraphId::try_from_id(raw).map_err(|_| ThorError::InvalidGraphId(raw))
}

/// The time zone for a location's time (the loop in mainline's `TimeInfo::make`):
/// the end node of the first candidate edge with a time zone, or 0.
fn location_timezone<P: GraphTileProvider>(
    provider: &P,
    location: &Location,
) -> Result<u32, ThorError> {
    let Some(correlation) = &location.correlation else {
        return Ok(0);
    };
    for pe in &correlation.edges {
        let id = graph_id(pe.graph_id)?;
        let Some(tile) = graph::tile(provider, id)? else {
            continue;
        };
        let end = graph::edge(&tile, id)?.end_node_id();
        let tz = graph::timezone_of_node(provider, end)?;
        if tz != 0 {
            return Ok(tz);
        }
    }
    Ok(0)
}

/// `%FT%R` of an instant in a zone.
fn format_minute(tz: &TimeZone, at: Timestamp) -> String {
    tz.to_datetime(at).strftime("%Y-%m-%dT%H:%M").to_string()
}

/// The time info at a location (mainline's `TimeInfo::make(location, reader)`),
/// with `now` as the current time.
///
/// A `current` date and time is replaced in the location by `now`, to the minute,
/// in the location's time zone (or UTC), as mainline does.
///
/// # Errors
///
/// Fails if a candidate edge's tile can't be read.
pub(crate) fn location_time_info<P: GraphTileProvider>(
    provider: &P,
    location: &mut Location,
    now: Timestamp,
) -> Result<TimeInfo, ThorError> {
    if location.date_time.is_empty() {
        return Ok(TimeInfo::INVALID);
    }
    let tz_index = location_timezone(provider, location)?;
    if location.date_time == "current" {
        let zone = if tz_index == 0 {
            UTC_TIME_ZONE_INDEX
        } else {
            tz_index
        };
        if let Some(tz) = time_zone(zone) {
            location.date_time = format_minute(tz, now);
        }
    }
    Ok(TimeInfo::make(&location.date_time, tz_index, now))
}

/// A location's date and time, UTC offset, and time zone name
/// (mainline's `dt_info_t`), all empty when invalid.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DateTimeInfo {
    pub(crate) date_time: String,
    pub(crate) time_zone_offset: String,
    pub(crate) time_zone_name: String,
}

impl DateTimeInfo {
    /// Writes the time into a location (`set_date_time` and friends).
    pub(crate) fn apply(self, location: &mut Location) {
        location.date_time = self.date_time;
        location.time_zone_offset = self.time_zone_offset;
        location.time_zone_name = self.time_zone_name;
    }
}

/// Mainline's `DateTime::seconds_since_epoch`: the local time in a zone as seconds,
/// counting leap seconds (so 27 seconds late since 2017), or 0 without a zone.
fn seconds_since_epoch(date_time: &str, tz_index: u32) -> Option<u64> {
    if date_time.is_empty() || time_zone(tz_index).is_none() {
        return Some(0);
    }
    // `TimeInfo::make` reads the time the same way (it only uses `now` for `current`).
    let info = TimeInfo::make(date_time, tz_index, Timestamp::UNIX_EPOCH);
    info.valid.then_some(info.local_time)
}

/// Moves a date and time by `offset` seconds and formats it in another time zone
/// (mainline's `DateTime::offset_date`).
///
/// The result is invalid (all empty) without a date and time or an output time zone.
/// A date and time that doesn't parse is also invalid here
/// (mainline would read it as the start of 1970; Loki rejects such times before Thor).
pub(crate) fn offset_date(date_time: &str, in_tz: u32, out_tz: u32, offset: f32) -> DateTimeInfo {
    if date_time.is_empty() {
        return DateTimeInfo::default();
    }
    let Some(iepoch) = seconds_since_epoch(date_time, in_tz) else {
        return DateTimeInfo::default();
    };
    #[expect(clippy::cast_precision_loss, reason = "mainline converts to double")]
    let sum = iepoch as f64 + f64::from(offset + 0.5);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "mainline's static_cast<uint64_t>"
    )]
    let oepoch = sum as u64;
    let (Some(tz), Some(name)) = (time_zone(out_tz), time_zone_name(out_tz)) else {
        return DateTimeInfo::default();
    };
    if oepoch == 0 {
        return DateTimeInfo::default();
    }
    let Some(at) = i64::try_from(oepoch)
        .ok()
        .and_then(|s| Timestamp::from_second(s).ok())
    else {
        return DateTimeInfo::default();
    };
    let zoned = at.to_zoned(tz.clone());
    let date_time = zoned.strftime("%Y-%m-%dT%H:%M").to_string();
    // `%z` with a colon inserted (`+01:00`).
    let z = zoned.strftime("%z").to_string();
    let offset = match z.len() {
        5 => format!("{}:{}", &z[..3], &z[3..]),
        _ => z,
    };
    DateTimeInfo {
        date_time,
        time_zone_offset: offset,
        time_zone_name: name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Andorra (`Europe/Andorra`) and Los Angeles.
    const ANDORRA: u32 = 293;
    const LOS_ANGELES: u32 = 94;

    #[test]
    fn offsets_dates_as_mainline() {
        // Mainline: "2026-11-09T08:00" + 0 s stays 08:00 (08:00:27 cut to the minute).
        let d = offset_date("2026-11-09T08:00", ANDORRA, ANDORRA, 0.0);
        assert_eq!(d.date_time, "2026-11-09T08:00");
        assert_eq!(d.time_zone_offset, "+01:00");
        assert_eq!(d.time_zone_name, "Europe/Andorra");
        // 32.6 s more reaches 08:01 because of the 27 leap seconds.
        assert_eq!(
            offset_date("2026-11-09T08:00", ANDORRA, ANDORRA, 32.6).date_time,
            "2026-11-09T08:01"
        );
        assert_eq!(
            offset_date("2026-11-09T08:00", ANDORRA, ANDORRA, 32.4).date_time,
            "2026-11-09T08:00"
        );
        let la = offset_date("2026-07-01T12:00", ANDORRA, LOS_ANGELES, -3600.0);
        assert_eq!(la.date_time, "2026-07-01T02:00");
        assert_eq!(la.time_zone_offset, "-07:00");
        assert_eq!(la.time_zone_name, "America/Los_Angeles");
        assert_eq!(
            offset_date("", ANDORRA, ANDORRA, 0.0),
            DateTimeInfo::default()
        );
        assert_eq!(
            offset_date("2026-07-01T12:00", ANDORRA, 0, 0.0),
            DateTimeInfo::default()
        );
        // Without an input zone, mainline starts from 0 (the start of 1970).
        assert_eq!(
            offset_date("2026-07-01T12:00", 0, ANDORRA, 60.0).date_time,
            "1970-01-01T01:01"
        );
    }
}
