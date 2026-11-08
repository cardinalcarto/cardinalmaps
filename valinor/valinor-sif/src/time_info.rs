//! The time along a route (mainline's `baldr::TimeInfo`).
//!
//! Path search starts from the request's date and time at a location
//! ([`TimeInfo::make`]) and moves it along as the route progresses
//! ([`TimeInfo::forward`] and [`TimeInfo::reverse`]).
//! Costing reads [`TimeInfo::local_time`] for timed restrictions
//! (as [`TimeContext::current_time`](crate::TimeContext)) and
//! [`TimeInfo::second_of_week`] for predicted speeds.
//!
//! # Leap seconds
//!
//! Mainline converts the requested time to seconds with `date::to_utc_time`,
//! which counts the leap seconds since 1970,
//! but everything that reads the result treats it as Unix time.
//! So mainline's times are 27 seconds late (for dates since 2017):
//! a departure at 08:00 is evaluated at 08:00:27,
//! both for timed restrictions and for predicted speed buckets.
//! This reproduces that, so that time-dependent costs match mainline's.

use jiff::Timestamp;
use jiff::civil::{Date, DateTime, Time, Weekday};
use jiff::tz::{AmbiguousOffset, TimeZone};

use crate::datetime::time_zone;

/// No time of week (mainline's `kInvalidSecondsOfWeek`).
pub const INVALID_SECONDS_OF_WEEK: u32 = (1 << 20) - 1;

/// Seconds in a week (`kSecondsPerWeek`).
const SECONDS_PER_WEEK: i32 = 7 * 24 * 60 * 60;

/// Mainline's index of `Etc/UTC`, its default time zone.
pub const UTC_TIME_ZONE_INDEX: u32 = 291;

/// The days (UTC) at whose start a leap second had just been inserted.
const LEAP_SECOND_DATES: [(i16, i8, i8); 27] = [
    (1972, 7, 1),
    (1973, 1, 1),
    (1974, 1, 1),
    (1975, 1, 1),
    (1976, 1, 1),
    (1977, 1, 1),
    (1978, 1, 1),
    (1979, 1, 1),
    (1980, 1, 1),
    (1981, 7, 1),
    (1982, 7, 1),
    (1983, 7, 1),
    (1985, 7, 1),
    (1988, 1, 1),
    (1990, 1, 1),
    (1991, 1, 1),
    (1992, 7, 1),
    (1993, 7, 1),
    (1994, 7, 1),
    (1996, 1, 1),
    (1997, 7, 1),
    (1999, 1, 1),
    (2006, 1, 1),
    (2009, 1, 1),
    (2012, 7, 1),
    (2015, 7, 1),
    (2017, 1, 1),
];

/// The leap seconds inserted up to Unix time `sys` (`date::to_utc_time`'s offset).
fn leap_seconds_before(sys: i64) -> i64 {
    let mut count = 0;
    for (year, month, day) in LEAP_SECOND_DATES {
        let Ok(date) = Date::new(year, month, day) else {
            continue;
        };
        let Ok(at) = date.to_zoned(TimeZone::UTC) else {
            continue;
        };
        if at.timestamp().as_second() <= sys {
            count += 1;
        }
    }
    count
}

/// The time at a point along a route (mainline's `baldr::TimeInfo`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct TimeInfo {
    /// Whether the route has a time at all.
    pub valid: bool,
    /// The time zone index at this point.
    pub timezone_index: u32,
    /// Seconds since the epoch, as mainline counts them (see [the module docs](self)).
    pub local_time: u64,
    /// Seconds since Sunday 00:00, local time ([`INVALID_SECONDS_OF_WEEK`] without a time).
    pub second_of_week: u32,
    /// Seconds from now (negative for the past); only live traffic reads it.
    pub seconds_from_now: i64,
}

impl TimeInfo {
    /// No time (mainline's `TimeInfo::invalid()`).
    pub const INVALID: Self = Self {
        valid: false,
        timezone_index: 0,
        local_time: 0,
        second_of_week: INVALID_SECONDS_OF_WEEK,
        seconds_from_now: 0,
    };

    /// The time info for a request's `date_time` at a location in time zone `timezone_index`
    /// (mainline's `TimeInfo::make`), with `now` as the current time.
    ///
    /// `date_time` is `YYYY-MM-DDTHH:MM` (or a date alone, meaning midnight), or `current`.
    /// An empty `date_time` means no time; one that doesn't parse gives an invalid time,
    /// as in mainline.
    /// A time zone index of 0 means the location has none, and `Etc/UTC` is used.
    /// Local times in a daylight saving gap or overlap resolve to the later instant
    /// (mainline's `choose::latest`).
    #[must_use]
    pub fn make(date_time: &str, timezone_index: u32, now: Timestamp) -> Self {
        if date_time.is_empty() {
            return Self::INVALID;
        }
        let timezone_index = if timezone_index == 0 {
            UTC_TIME_ZONE_INDEX
        } else {
            timezone_index
        };
        let Some(tz) = time_zone(timezone_index) else {
            return Self::INVALID;
        };

        // Mainline formats the current time to the minute and parses it like any other.
        let now_local = tz.to_datetime(now);
        let now_local = now_local
            .with()
            .second(0)
            .subsec_nanosecond(0)
            .build()
            .unwrap_or(now_local);
        let parsed = if date_time == "current" {
            Some(now_local)
        } else {
            parse_date_time(date_time)
        };
        let Some(parsed) = parsed else {
            return Self::INVALID;
        };
        let Some(sys) = latest_instant(tz, parsed) else {
            return Self::INVALID;
        };
        let local_time = sys + leap_seconds_before(sys);
        let Ok(local_time) = u64::try_from(local_time) else {
            return Self::INVALID;
        };
        let seconds_from_now = parsed.duration_since(now_local).as_secs();
        Self {
            valid: true,
            timezone_index,
            local_time,
            second_of_week: second_of_week(local_time, tz),
            seconds_from_now,
        }
    }

    /// The time `seconds_offset` seconds later, in time zone `next_tz_index`
    /// (mainline's `TimeInfo::forward`).
    ///
    /// As in mainline, the offset is relative to this time info
    /// (path search always offsets the origin's), and the time of week wraps.
    #[must_use]
    #[inline]
    pub fn forward(&self, seconds_offset: f32, next_tz_index: u32) -> Self {
        if !self.valid {
            return *self;
        }
        let local_time = self.local_time.wrapping_add(float_to_u64(seconds_offset));
        #[expect(clippy::cast_precision_loss, reason = "mainline adds in float")]
        let mut sw = float_to_i32(self.second_of_week as f32 + seconds_offset);
        sw += self.timezone_diff(local_time, next_tz_index);
        self.shifted(local_time, sw, seconds_offset, next_tz_index, true)
    }

    /// The time `seconds_offset` seconds earlier, in time zone `next_tz_index`
    /// (mainline's `TimeInfo::reverse`).
    #[must_use]
    #[inline]
    pub fn reverse(&self, seconds_offset: f32, next_tz_index: u32) -> Self {
        if !self.valid {
            return *self;
        }
        let local_time = self.local_time.wrapping_sub(float_to_u64(seconds_offset));
        let sow = i32::try_from(self.second_of_week).unwrap_or(i32::MAX);
        let mut sw = sow.wrapping_sub(float_to_i32(seconds_offset));
        sw += self.timezone_diff(local_time, next_tz_index);
        self.shifted(local_time, sw, seconds_offset, next_tz_index, false)
    }

    /// The offset change between this time zone and another at `local_time`
    /// (mainline's `DateTime::timezone_diff`), or 0 if either is unknown.
    fn timezone_diff(&self, local_time: u64, next_tz_index: u32) -> i32 {
        if next_tz_index == self.timezone_index {
            return 0;
        }
        let (Some(from), Some(to)) = (time_zone(self.timezone_index), time_zone(next_tz_index))
        else {
            return 0;
        };
        let Some(at) = i64::try_from(local_time)
            .ok()
            .and_then(|s| Timestamp::from_second(s).ok())
        else {
            return 0;
        };
        to.to_offset(at).seconds() - from.to_offset(at).seconds()
    }

    fn shifted(
        &self,
        local_time: u64,
        mut sw: i32,
        seconds_offset: f32,
        tz: u32,
        forward: bool,
    ) -> Self {
        if sw < 0 {
            sw += SECONDS_PER_WEEK;
        } else if sw > SECONDS_PER_WEEK {
            sw -= SECONDS_PER_WEEK;
        }
        let offset = float_to_i64(seconds_offset);
        let seconds_from_now = if forward {
            self.seconds_from_now.wrapping_add(offset)
        } else {
            self.seconds_from_now.wrapping_sub(offset)
        };
        Self {
            valid: self.valid,
            timezone_index: tz,
            local_time,
            // Mainline stores this in 20 bits.
            second_of_week: u32::try_from(sw).unwrap_or(0) & INVALID_SECONDS_OF_WEEK,
            seconds_from_now,
        }
    }

    /// The time of week to look speeds up at (`None` without a time).
    #[inline]
    pub fn seconds_of_week(&self) -> Option<u32> {
        (self.second_of_week != INVALID_SECONDS_OF_WEEK).then_some(self.second_of_week)
    }

    /// The time for access checks (0 without a time, as mainline passes it).
    #[inline]
    pub fn current_time(&self) -> u64 {
        if self.valid { self.local_time } else { 0 }
    }
}

/// `static_cast<uint64_t>(float)` as x86-64 does it: through a signed conversion.
fn float_to_u64(value: f32) -> u64 {
    float_to_i64(value).cast_unsigned()
}

fn float_to_i64(value: f32) -> i64 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the C++ conversion truncates"
    )]
    let converted = value as i64;
    converted
}

fn float_to_i32(value: f32) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the C++ conversion truncates"
    )]
    let converted = value as i32;
    converted
}

/// Parses mainline's `%FT%R` (or `%F`) local date and time.
fn parse_date_time(text: &str) -> Option<DateTime> {
    let (date, time) = match text.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (text, None),
    };
    let mut parts = date.splitn(3, '-');
    let year: i16 = parts.next()?.parse().ok()?;
    let month: i8 = parts.next()?.parse().ok()?;
    let day: i8 = parts.next()?.parse().ok()?;
    let date = Date::new(year, month, day).ok()?;
    let time = match time {
        None => Time::midnight(),
        Some(time) => {
            let (hour, minute) = time.split_once(':')?;
            // `%R` reads the hour and minute; anything after them is ignored.
            let minute = minute.get(..2).unwrap_or(minute);
            Time::new(hour.parse().ok()?, minute.parse().ok()?, 0, 0).ok()?
        }
    };
    Some(date.to_datetime(time))
}

/// The instant of a local time, taking the later one in an overlap
/// and the transition itself in a gap (`date::choose::latest`).
fn latest_instant(tz: &TimeZone, local: DateTime) -> Option<i64> {
    let ambiguous = tz.to_ambiguous_timestamp(local);
    let local_seconds = local.to_zoned(TimeZone::UTC).ok()?.timestamp().as_second();
    Some(match ambiguous.offset() {
        AmbiguousOffset::Unambiguous { offset } => local_seconds - i64::from(offset.seconds()),
        // Two instants: the later is the one with the smaller offset.
        AmbiguousOffset::Fold { before, after } => {
            local_seconds - i64::from(before.seconds().min(after.seconds()))
        }
        // No instant: the transition, which is the latest instant before the gap ends.
        AmbiguousOffset::Gap { before, .. } => {
            let candidate =
                Timestamp::from_second(local_seconds - i64::from(before.seconds())).ok()?;
            let transition = tz
                .preceding(
                    candidate
                        .checked_add(jiff::SignedDuration::from_secs(1))
                        .ok()?,
                )
                .next()?;
            transition.timestamp().as_second()
        }
    })
}

/// Mainline's `DateTime::second_of_week`: the local time of week of an epoch second
/// (which mainline narrows to 32 bits first).
fn second_of_week(epoch_time: u64, tz: &TimeZone) -> u32 {
    #[expect(clippy::cast_possible_truncation, reason = "mainline takes a uint32_t")]
    let narrowed = i64::from(epoch_time as u32);
    let Ok(at) = Timestamp::from_second(narrowed) else {
        return INVALID_SECONDS_OF_WEEK;
    };
    let local = tz.to_datetime(at);
    let day: u32 = match local.weekday() {
        Weekday::Sunday => 0,
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
    };
    let since_midnight = u32::from(local.hour().unsigned_abs()) * 3600
        + u32::from(local.minute().unsigned_abs()) * 60
        + u32::from(local.second().unsigned_abs());
    day * 86_400 + since_midnight
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mainline's values from `sif timeinfo 2026-11-09T08:00 293 100.5 293 3600 94 -50 293`.
    #[test]
    fn matches_mainline() {
        let now = Timestamp::from_second(1_794_116_280).expect("now");
        let ti = TimeInfo::make("2026-11-09T08:00", 293, now);
        assert_eq!(
            (
                ti.valid,
                ti.timezone_index,
                ti.local_time,
                ti.second_of_week
            ),
            (true, 293, 1_794_207_627, 115_227)
        );
        let check = |t: TimeInfo, expected: (u32, u64, u32)| {
            assert_eq!((t.timezone_index, t.local_time, t.second_of_week), expected);
        };
        check(ti.forward(100.5, 293), (293, 1_794_207_727, 115_327));
        check(ti.reverse(100.5, 293), (293, 1_794_207_527, 115_127));
        check(ti.forward(3600.0, 94), (94, 1_794_211_227, 86_427));
        check(ti.reverse(3600.0, 94), (94, 1_794_204_027, 79_227));
        check(ti.forward(-50.0, 293), (293, 1_794_207_577, 115_177));
        check(ti.reverse(-50.0, 293), (293, 1_794_207_677, 115_277));
        assert_eq!(ti.seconds_of_week(), Some(115_227));
        assert_eq!(TimeInfo::INVALID.seconds_of_week(), None);
        assert_eq!(TimeInfo::make("", 293, now), TimeInfo::INVALID);
        assert!(!TimeInfo::make("nonsense", 293, now).valid);
    }

    /// Mainline's `sif timeinfo <date_time> <tz>` for daylight saving gaps and overlaps,
    /// dates alone, and lenient formats.
    #[test]
    fn edge_cases_match_mainline() {
        let now = Timestamp::from_second(1_794_116_280).expect("now");
        for (date_time, tz, local_time, second_of_week) in [
            // Andorra's spring gap: the transition (03:00).
            ("2026-03-29T02:30", 293, 1_774_746_027, 10_827),
            // Andorra's autumn overlap: the later 02:30.
            ("2026-10-25T02:30", 293, 1_792_891_827, 9_027),
            ("2026-03-08T02:30", 94, 1_772_964_027, 10_827),
            ("2026-11-01T01:30", 94, 1_793_525_427, 5_427),
            ("2026-07-12", 94, 1_783_839_627, 27),
            ("2026-07-12T1:5", 293, 1_783_811_127, 3_927),
            ("2026-07-12T13:45:59", 293, 1_783_856_727, 49_527),
            ("2026-7-2T13:45", 293, 1_782_992_727, 395_127),
        ] {
            let ti = TimeInfo::make(date_time, tz, now);
            assert_eq!(
                (ti.valid, ti.local_time, ti.second_of_week),
                (true, local_time, second_of_week),
                "{date_time}"
            );
        }
        // Mainline fails to parse February 30.
        assert!(!TimeInfo::make("2026-02-30T10:00", 293, now).valid);
        // Without a time zone, UTC.
        let utc = TimeInfo::make("2026-07-12T13:45", 0, now);
        assert_eq!(utc.timezone_index, UTC_TIME_ZONE_INDEX);
        assert_eq!(utc.local_time, 1_783_863_900 + 27);
    }

    #[test]
    fn leap_seconds() {
        assert_eq!(leap_seconds_before(0), 0);
        assert_eq!(leap_seconds_before(1_483_228_799), 26);
        assert_eq!(leap_seconds_before(1_483_228_800), 27);
    }
}
