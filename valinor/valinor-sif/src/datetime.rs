//! Time zones and conditional date and time ranges (the parts of mainline's `baldr::DateTime`
//! that costing needs).
//!
//! Tiles give each node a time zone as an index into a fixed table of zone names
//! ([`time_zone_name`]).
//! Timed access restrictions and timed complex restrictions are [`ConditionalTime`]s,
//! which are active or not at a moment in a time zone ([`is_conditional_active`]).
//!
//! Time zone rules come from the IANA time zone database via `jiff`
//! (the system's copy where there is one, as on Linux and Android).
//! The bundled database supplies legacy names omitted by some system databases.
//! Mainline reads the same database with Howard Hinnant's `date` library,
//! so the two agree wherever their database versions do.

mod zones;

use std::sync::OnceLock;

use jiff::Timestamp;
use jiff::civil::Weekday;
use jiff::tz::{self, TimeZone, TimeZoneDatabase};
use valhalla_graphtile::graph_tile::{DayType, RestrictionDateTime, TimeDomain};

use zones::{DENSE_ZONE_NAMES, EXTRA_ZONE_NAMES};

/// One past the largest time zone index mainline assigns.
const ZONE_INDEX_LIMIT: usize = 727;

/// The name of the time zone with the given index (mainline's `tz_name_to_id`, inverted).
///
/// Returns `None` for index 0 (no time zone) and for indexes mainline doesn't assign.
#[inline]
pub fn time_zone_name(index: u32) -> Option<&'static str> {
    let index = usize::try_from(index).ok()?;
    if let Some(dense) = index.checked_sub(1)
        && let Some(name) = DENSE_ZONE_NAMES.get(dense)
    {
        return Some(name);
    }
    EXTRA_ZONE_NAMES
        .iter()
        .find(|(extra, _)| usize::from(*extra) == index)
        .map(|(_, name)| *name)
}

/// The time zone with the given index (mainline's `tz_db_t::from_index`).
///
/// Returns `None` for index 0, for indexes mainline doesn't assign,
/// and for zones the time zone database doesn't have.
///
/// # Performance
///
/// The first lookup of each zone reads the time zone database;
/// later lookups of the same index are an array access.
#[inline]
pub fn time_zone(index: u32) -> Option<&'static TimeZone> {
    static ZONES: [OnceLock<Option<TimeZone>>; ZONE_INDEX_LIMIT] =
        [const { OnceLock::new() }; ZONE_INDEX_LIMIT];
    let slot = ZONES.get(usize::try_from(index).ok()?)?;
    slot.get_or_init(|| time_zone_name(index).and_then(|name| lookup_time_zone(tz::db(), name)))
        .as_ref()
}

fn lookup_time_zone(database: &TimeZoneDatabase, name: &str) -> Option<TimeZone> {
    // Prefer current system rules, but keep the names used by existing Valhalla tiles
    // resolvable even on distributions that omit legacy zones.
    database
        .get(name)
        .or_else(|_| TimeZoneDatabase::bundled().get(name))
        .ok()
}

/// A date and time range that a restriction applies in,
/// with the fields of a [`TimeDomain`] (mainline passes these one by one to
/// `DateTime::is_conditional_active`).
///
/// Every field is 0 when not set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConditionalTime {
    /// True for nth day of week ranges (`kNthDow`), false for day of month ranges (`kYMD`).
    pub nth_dow: bool,
    /// The days of the week, as a mask with bit 0 for Sunday.
    pub dow: u8,
    pub begin_hrs: u8,
    pub begin_mins: u8,
    pub begin_month: u8,
    pub begin_day_dow: u8,
    pub begin_week: u8,
    pub end_hrs: u8,
    pub end_mins: u8,
    pub end_month: u8,
    pub end_day_dow: u8,
    pub end_week: u8,
}

impl From<TimeDomain> for ConditionalTime {
    #[inline]
    fn from(td: TimeDomain) -> Self {
        Self {
            nth_dow: td.day_type() == DayType::NthDayOfWeek,
            dow: td.dow(),
            begin_hrs: td.begin_hrs(),
            begin_mins: td.begin_mins(),
            begin_month: td.begin_month(),
            begin_day_dow: td.begin_day_dow(),
            begin_week: td.begin_week(),
            end_hrs: td.end_hrs(),
            end_mins: td.end_mins(),
            end_month: td.end_month(),
            end_day_dow: td.end_day_dow(),
            end_week: td.end_week(),
        }
    }
}

impl From<RestrictionDateTime> for ConditionalTime {
    #[inline]
    fn from(dt: RestrictionDateTime) -> Self {
        Self {
            nth_dow: dt.nth_dow,
            dow: dt.dow,
            begin_hrs: dt.begin_hrs,
            begin_mins: dt.begin_mins,
            begin_month: dt.begin_month,
            begin_day_dow: dt.begin_day_dow,
            begin_week: dt.begin_week,
            end_hrs: dt.end_hrs,
            end_mins: dt.end_mins,
            end_month: dt.end_month,
            end_day_dow: dt.end_day_dow,
            end_week: dt.end_week,
        }
    }
}

/// Whether a conditional range is active at `current_time` in the time zone with index `tz_index`
/// (mainline's `DateTime::is_conditional_active`, with the zone looked up as
/// `get_tz_db().from_index(tz_index)`).
///
/// `current_time` is seconds since the Unix epoch.
/// Without a time zone (an index with no zone), nothing is active, as in mainline.
///
/// # Performance
///
/// This converts the time to local time in the zone, which is a search of its transitions;
/// costing only calls it for edges with timed restrictions on time-dependent routes.
#[inline]
pub fn is_conditional_active_in(
    condition: ConditionalTime,
    current_time: u64,
    tz_index: u32,
) -> bool {
    time_zone(tz_index).is_some_and(|tz| is_conditional_active(condition, current_time, tz))
}

/// A civil date as mainline's `date` library computes with it:
/// a year, month, and day that need not be valid,
/// and that convert to a day number the way `year_month_day::to_days` does.
#[derive(Clone, Copy, Debug)]
struct Ymd {
    year: i32,
    month: u32,
    day: u32,
}

impl Ymd {
    /// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`, as `date.h` writes it).
    ///
    /// Like `date.h`, this does unsigned arithmetic on the month and day,
    /// so an out-of-range day (such as 0, or 31 in a 30-day month) rolls into the
    /// neighboring month, as mainline's comparisons see it.
    fn to_days(self) -> i64 {
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let m = self.month;
        let d = self.day;
        let era = if y >= 0 { y } else { y - 399 } / 400;
        // In [0, 399].
        let yoe = y - era * 400;
        let doy = (153u32
            .wrapping_mul(if m > 2 { m.wrapping_sub(3) } else { m + 9 })
            .wrapping_add(2)
            / 5)
        .wrapping_add(d.wrapping_sub(1));
        // `date.h` adds these as unsigned and converts the sum to int,
        // so an underflowing day wraps back around here.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "yoe is in [0, 399]"
        )]
        let yoe = yoe as u32;
        let doe = yoe
            .wrapping_mul(365)
            .wrapping_add(yoe / 4)
            .wrapping_sub(yoe / 100)
            .wrapping_add(doy);
        #[expect(
            clippy::cast_possible_wrap,
            reason = "date.h converts the unsigned day of era to int the same way"
        )]
        let doe = doe as i32;
        era * 146_097 + i64::from(doe) - 719_468
    }
}

/// The weekday (0 for Sunday) of a day number.
fn weekday_of_days(days: i64) -> u8 {
    // 1970-01-01 was a Thursday.
    #[expect(clippy::cast_possible_truncation, reason = "in [0, 6]")]
    let wd = (days + 4).rem_euclid(7) as u8;
    wd
}

/// `date::weekday(unsigned)`: 7 also means Sunday, and other values are kept as is
/// (as an `unsigned char`), which makes the weekday invalid.
fn date_weekday(value: u32) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "date.h stores an unsigned char"
    )]
    let wd = if value == 7 { 0 } else { value as u8 };
    wd
}

/// `date::weekday` subtraction: the days from `y` forward to `x`, in [0, 6] for valid weekdays.
fn weekday_difference(x: u8, y: u8) -> i64 {
    let wdu = i64::from(x) - i64::from(y);
    let wk = if wdu >= 0 { wdu } else { wdu - 6 } / 7;
    wdu - wk * 7
}

/// The number of days in a month (`(year_month / last).day()`), for months 1 to 12.
///
/// `date.h` reads out of its table for other months; this gives 0 for them,
/// which tiles never hold.
fn last_day_of_month(year: i32, month: u32) -> u32 {
    const DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    if month == 2 && leap {
        29
    } else {
        month
            .checked_sub(1)
            .and_then(|i| DAYS.get(usize::try_from(i).ok()?))
            .copied()
            .unwrap_or(0)
    }
}

/// The day number of the `index`th weekday `weekday_value` of a month
/// (`sys_days(year_month_weekday)`), and whether that day exists (`year_month_weekday::ok`).
///
/// `weekday_value` is what mainline passes to `date::weekday`: the day of week field minus one.
fn nth_weekday(year: i32, month: u32, weekday_value: u32, index: u8) -> (i64, bool) {
    let wd = date_weekday(weekday_value);
    let first = Ymd {
        year,
        month,
        day: 1,
    }
    .to_days();
    let offset = weekday_difference(wd, weekday_of_days(first));
    let days = first + offset + (i64::from(index) - 1) * 7;
    let ok = (1..=12).contains(&month)
        && wd <= 6
        && index >= 1
        && (index <= 4 || {
            let day_in_month = offset + (i64::from(index) - 1) * 7 + 1;
            day_in_month <= i64::from(last_day_of_month(year, month))
        });
    (days, ok)
}

/// Whether a conditional range is active at `current_time` (seconds since the Unix epoch)
/// in `time_zone` (mainline's `DateTime::is_conditional_active`).
///
/// This follows mainline step by step, including its quirks:
///
/// - A month-only range (such as `Dec`) is unreachable:
///   mainline first turns every day of month range with months but no days into a range
///   from the first of the begin month to the end of the end month (in the current year).
/// - When the end of an nth weekday range asks for a fifth weekday a month doesn't have,
///   mainline means to fall back to the fourth, but drops the result,
///   so the range ends in the following month.
/// - Ranges across New Year (such as `Dec 5-Mar 3`) and ranges whose end day comes
///   before their begin day within one month are handled as mainline handles them.
/// - Time ranges are inclusive at both ends, to the minute.
#[expect(
    clippy::too_many_lines,
    reason = "this follows mainline's function line by line, which is easier to check as one"
)]
pub fn is_conditional_active(
    condition: ConditionalTime,
    current_time: u64,
    time_zone: &TimeZone,
) -> bool {
    let Ok(seconds) = i64::try_from(current_time) else {
        return false;
    };
    let Ok(timestamp) = Timestamp::from_second(seconds) else {
        return false;
    };
    let local = time_zone.to_datetime(timestamp);
    let d = Ymd {
        year: i32::from(local.year()),
        month: u32::from(local.month().unsigned_abs()),
        day: u32::from(local.day().unsigned_abs()),
    };
    let today = d.to_days();
    let td = u32::from(local.hour().unsigned_abs()) * 60 + u32::from(local.minute().unsigned_abs());

    let ConditionalTime {
        nth_dow,
        dow,
        begin_hrs,
        begin_mins,
        begin_month,
        begin_day_dow,
        begin_week,
        end_hrs,
        end_mins,
        end_month,
        end_day_dow,
        end_week,
    } = condition;

    let mut b_td: u32 = 0;
    let mut e_td: u32 = 23 * 60 + 59;
    let has_times = begin_hrs != 0 || begin_mins != 0 || end_hrs != 0 || end_mins != 0;
    let begin_minutes = u32::from(begin_hrs) * 60 + u32::from(begin_mins);
    let end_minutes = u32::from(end_hrs) * 60 + u32::from(end_mins);

    let mut dow_in_range = true;
    if dow != 0 {
        let wd = match local.weekday() {
            Weekday::Sunday => 0,
            Weekday::Monday => 1,
            Weekday::Tuesday => 2,
            Weekday::Wednesday => 3,
            Weekday::Thursday => 4,
            Weekday::Friday => 5,
            Weekday::Saturday => 6,
        };
        dow_in_range = dow & (1 << wd) != 0;
    }

    let mut b_month = u32::from(begin_month);
    let mut e_month = u32::from(end_month);
    let mut b_day_dow = u32::from(begin_day_dow);
    let mut e_day_dow = u32::from(end_day_dow);
    let mut b_week = begin_week;
    let mut e_week = end_week;

    // Su[-1]: an nth weekday with no month means every month.
    if nth_dow && begin_week != 0 && begin_day_dow == 0 && begin_month == 0 {
        b_month = d.month;
    }
    if nth_dow && end_week != 0 && end_day_dow == 0 && end_month == 0 {
        e_month = d.month;
    }

    if nth_dow
        && begin_week != 0
        && begin_day_dow == 0
        && begin_month == 0
        && end_week == 0
        && end_day_dow == 0
        && end_month == 0
    {
        // Only Su[-1] set in begin: that weekday of every month only.
        e_month = b_month;
        b_day_dow = u32::from(dow);
        e_day_dow = u32::from(dow);
        e_week = b_week;
    } else if !nth_dow && b_month != 0 && e_month != 0 && b_day_dow == 0 && e_day_dow == 0 {
        // Sep-Jun We 08:15-08:45: from the first of the begin month to the end of the end month.
        b_day_dow = 1;
        e_day_dow = last_day_of_month(d.year, e_month);
    }

    let mut edge_case = false;
    let mut b_year = d.year;
    let mut e_year = d.year;
    let begin_date: i64;
    let end_date: i64;

    if !nth_dow
        && b_month != 0
        && e_month != 0
        && b_day_dow == 0
        && e_day_dow == 0
        && b_week == 0
        && e_week == 0
        && b_month == e_month
    {
        // Month only (unreachable, as the branch above fills in the days).
        let mut dt_in_range = b_month <= d.month && d.month <= e_month;
        if has_times {
            b_td = begin_minutes;
            e_td = end_minutes;
        }
        dt_in_range = dt_in_range && b_td <= td && td <= e_td;
        return dow_in_range && dt_in_range;
    } else if !nth_dow && b_month != 0 && b_day_dow != 0 {
        if b_month == e_month {
            if b_day_dow > e_day_dow {
                // Mar 15 - Mar 1
                edge_case = true;
            }
        } else if b_month > e_month {
            // Oct 10 - Mar 3
            if b_month > d.month {
                b_year = d.year - 1;
            } else {
                e_year = d.year + 1;
            }
        }
        begin_date = Ymd {
            year: b_year,
            month: b_month,
            day: b_day_dow,
        }
        .to_days();
        end_date = Ymd {
            year: e_year,
            month: e_month,
            day: e_day_dow,
        }
        .to_days();
    } else if nth_dow && b_month != 0 && b_day_dow != 0 && e_month != 0 && e_day_dow != 0 {
        // Nth weekday ranges can mix in days of the month
        // (for example, Dec Su[-1]-Mar 3 Sat 15:00-17:00).
        if b_month == e_month {
            if b_day_dow > e_day_dow {
                edge_case = true;
            }
        } else if b_month > e_month {
            if b_month > d.month {
                b_year = d.year - 1;
            } else {
                e_year = d.year + 1;
            }
        }

        begin_date = if b_week != 0 && b_week <= 5 {
            let (mut days, ok) = nth_weekday(b_year, b_month, b_day_dow.wrapping_sub(1), b_week);
            if b_week == 5 && !ok {
                // There is no fifth such weekday this month, so take the fourth.
                b_week -= 1;
                days = nth_weekday(b_year, b_month, b_day_dow.wrapping_sub(1), b_week).0;
            }
            days
        } else {
            Ymd {
                year: b_year,
                month: b_month,
                day: b_day_dow,
            }
            .to_days()
        };

        end_date = if e_week != 0 && e_week <= 5 {
            // Mainline computes the fourth weekday when there is no fifth, but drops it,
            // so the fifth (in the following month) stands.
            nth_weekday(e_year, e_month, e_day_dow.wrapping_sub(1), e_week).0
        } else {
            Ymd {
                year: e_year,
                month: e_month,
                day: e_day_dow,
            }
            .to_days()
        };
    } else {
        // Just a time, or days of the week with or without a time.
        let mut dt_in_range = true;
        if has_times {
            b_td = begin_minutes;
            e_td = end_minutes;
            dt_in_range = if begin_hrs > end_hrs {
                // 19:00 - 06:00
                !(e_td <= td && td <= b_td)
            } else {
                b_td <= td && td <= e_td
            };
        }
        return dow_in_range && dt_in_range;
    }

    if has_times {
        b_td = begin_minutes;
        e_td = end_minutes;
    }

    // Mainline compares the local times of zoned midnights,
    // which order the same way as the days themselves.
    let mut dt_in_range = if edge_case {
        // Jan 04 to Jan 02: check to the end of the year, and from the start of the year.
        let year_end = Ymd {
            year: b_year,
            month: 12,
            day: 31,
        }
        .to_days();
        let year_start = Ymd {
            year: b_year,
            month: 1,
            day: 1,
        }
        .to_days();
        (begin_date <= today && today <= year_end) || (year_start <= today && today <= end_date)
    } else {
        begin_date <= today && today <= end_date
    };

    let time_in_range = if begin_hrs > end_hrs {
        !(e_td <= td && td <= b_td)
    } else {
        b_td <= td && td <= e_td
    };
    dt_in_range = dt_in_range && time_in_range;
    dow_in_range && dt_in_range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_names_match_mainline_indexes() {
        assert_eq!(time_zone_name(0), None);
        assert_eq!(time_zone_name(1), Some("Africa/Abidjan"));
        assert_eq!(time_zone_name(94), Some("America/Los_Angeles"));
        assert_eq!(time_zone_name(293), Some("Europe/Andorra"));
        assert_eq!(time_zone_name(387), Some("WET"));
        assert_eq!(time_zone_name(388), None);
        assert_eq!(time_zone_name(629), Some("America/Ciudad_Juarez"));
        assert_eq!(time_zone_name(726), Some("Asia/Qostanay"));
        assert_eq!(time_zone_name(727), None);
        assert_eq!(time_zone_name(u32::MAX), None);
    }

    #[test]
    fn zones_resolve() {
        assert_eq!(
            time_zone(293).and_then(TimeZone::iana_name),
            Some("Europe/Andorra")
        );
        assert!(time_zone(0).is_none());
        assert!(time_zone(500).is_none());
        // Every zone mainline names must resolve using the system or bundled database.
        let missing: Vec<_> = (1..ZONE_INDEX_LIMIT)
            .filter_map(|i| {
                let i = u32::try_from(i).ok()?;
                let name = time_zone_name(i)?;
                time_zone(i).is_none().then_some(name)
            })
            .collect();
        assert!(missing.is_empty(), "missing zones: {missing:?}");
    }

    #[test]
    fn zones_resolve_without_system_data() {
        let database = TimeZoneDatabase::none();
        let missing: Vec<_> = (1..ZONE_INDEX_LIMIT)
            .filter_map(|i| time_zone_name(i as u32))
            .filter(|name| lookup_time_zone(&database, name).is_none())
            .collect();
        assert!(missing.is_empty(), "missing bundled zones: {missing:?}");
        assert!(lookup_time_zone(&database, "Invalid/Zone").is_none());
    }

    #[test]
    fn days_match_date_h() {
        let days = |year, month, day| Ymd { year, month, day }.to_days();
        assert_eq!(days(1970, 1, 1), 0);
        assert_eq!(days(2000, 3, 1), 11_017);
        assert_eq!(days(1969, 12, 31), -1);
        // Out-of-range days roll over, as in date.h.
        assert_eq!(days(2025, 3, 0), days(2025, 2, 28));
        assert_eq!(days(2024, 2, 30), days(2024, 3, 1));
        assert_eq!(days(2025, 4, 31), days(2025, 5, 1));
        assert_eq!(weekday_of_days(days(2026, 11, 7)), 6);
        // The fifth Friday of November 2026 doesn't exist; the date rolls into December.
        let (fifth, ok) = nth_weekday(2026, 11, 5, 5);
        assert!(!ok);
        assert_eq!(fifth, days(2026, 12, 4));
        let (fourth, ok) = nth_weekday(2026, 11, 5, 4);
        assert!(ok);
        assert_eq!(fourth, days(2026, 11, 27));
        // Months with five Sundays (November 2026 starts on a Sunday).
        assert_eq!(nth_weekday(2026, 11, 0, 5), (days(2026, 11, 29), true));
        assert_eq!(nth_weekday(2026, 11, 0, 1), (days(2026, 11, 1), true));
    }
}
