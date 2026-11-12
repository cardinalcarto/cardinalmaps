//! GTFS-RT's textual dates and times, parsed strictly.

use core::fmt;

/// A service date from a GTFS-RT `start_date`: `YYYYMMDD`.
///
/// Holds the digits as one number, which sorts in date order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CalendarDate(u32);

impl CalendarDate {
    /// Parses `YYYYMMDD`, rejecting anything but eight digits of a plausible date.
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 8 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let digits: u32 = text.parse().ok()?;
        let month = digits / 100 % 100;
        let day = digits % 100;
        ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some(Self(digits))
    }

    /// The date's `YYYYMMDD` digits as a number.
    pub fn digits(self) -> u32 {
        self.0
    }
}

impl fmt::Display for CalendarDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08}", self.0)
    }
}

/// Parses a GTFS-RT `start_time` (`HH:MM:SS`, hours may exceed 23) into seconds of a service day.
pub fn parse_start_time(text: &str) -> Option<u32> {
    let mut parts = text.split(':');
    let hours: u32 = parts.next()?.parse().ok()?;
    let minutes: u32 = parts.next()?.parse().ok()?;
    let seconds: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || minutes > 59 || seconds > 59 {
        return None;
    }
    hours
        .checked_mul(3600)?
        .checked_add(minutes * 60)?
        .checked_add(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_strict() {
        assert_eq!(
            CalendarDate::parse("20261108").map(CalendarDate::digits),
            Some(20_261_108)
        );
        assert!(CalendarDate::parse("2026118").is_none());
        assert!(CalendarDate::parse("202611088").is_none());
        assert!(CalendarDate::parse("2026-111").is_none());
        assert!(CalendarDate::parse("20261308").is_none());
        assert!(CalendarDate::parse("20261100").is_none());
        assert!(CalendarDate::parse("").is_none());
    }

    #[test]
    fn start_times_may_pass_midnight() {
        assert_eq!(parse_start_time("07:45:00"), Some(27_900));
        assert_eq!(parse_start_time("25:01:30"), Some(90_090));
        assert_eq!(parse_start_time("7:45"), None);
        assert_eq!(parse_start_time("07:60:00"), None);
        assert_eq!(parse_start_time("99999999:00:00"), None);
        assert_eq!(parse_start_time("a:b:c"), None);
    }
}
