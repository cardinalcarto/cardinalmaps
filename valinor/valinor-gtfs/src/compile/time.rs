//! Parsing of the two GTFS time formats.

use jiff::civil::Date;

/// Parses `HH:MM:SS` or `H:MM:SS`, where hours may be 24 or more.
///
/// `None` means that the text isn't a GTFS time or is too large for an `i32` of seconds.
pub(super) fn parse_time(text: &str) -> Option<i32> {
    let mut parts = text.trim().split(':');
    let hours = digits(parts.next()?)?;
    let minutes = digits(parts.next()?)?;
    let seconds = digits(parts.next()?)?;
    if parts.next().is_some() || minutes >= 60 || seconds >= 60 {
        return None;
    }
    hours
        .checked_mul(3600)?
        .checked_add(minutes * 60)?
        .checked_add(seconds)
}

fn digits(text: &str) -> Option<i32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Parses a GTFS `YYYYMMDD` date.
pub(super) fn parse_date(text: &str) -> Option<Date> {
    let bytes = text.trim().as_bytes();
    if bytes.len() != 8 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let number = |range: core::ops::Range<usize>| -> Option<i32> {
        core::str::from_utf8(&bytes[range]).ok()?.parse().ok()
    };
    Date::new(
        i16::try_from(number(0..4)?).ok()?,
        i8::try_from(number(4..6)?).ok()?,
        i8::try_from(number(6..8)?).ok()?,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn times_past_midnight() {
        assert_eq!(parse_time("08:15:30"), Some(8 * 3600 + 15 * 60 + 30));
        assert_eq!(parse_time("8:15:30"), Some(8 * 3600 + 15 * 60 + 30));
        assert_eq!(parse_time("29:30:00"), Some(29 * 3600 + 30 * 60));
        assert_eq!(parse_time(" 24:00:00 "), Some(86_400));
    }

    #[test]
    fn invalid_times() {
        for text in [
            "", "1:2", "1:2:3:4", "08:60:00", "08:00:60", "a:00:00", "-1:00:00",
        ] {
            assert_eq!(parse_time(text), None, "{text}");
        }
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("20261110"), Some(Date::constant(2026, 11, 10)));
        assert_eq!(parse_date("2026111"), None);
        assert_eq!(parse_date("20261310"), None);
    }

    proptest! {
        #[test]
        fn time_round_trip(hours in 0_i32..1_000, minutes in 0_i32..60, seconds in 0_i32..60) {
            let text = format!("{hours:02}:{minutes:02}:{seconds:02}");
            prop_assert_eq!(parse_time(&text), Some(hours * 3600 + minutes * 60 + seconds));
        }
    }
}
