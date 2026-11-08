//! # Time domains
//!
//! A [`TimeDomain`] packs a single date and time range
//! (a subset of the OSM [`opening_hours`](https://wiki.openstreetmap.org/wiki/Key:opening_hours)
//! syntax) into 64 bits.
//! Valhalla uses them for timed access restrictions
//! and conditional speed limits ([`ConditionalSpeedLimit`]).

use bitfield_struct::bitfield;
use core::fmt::{self, Display, Formatter, Write};

/// How [`TimeDomain::begin_day_dow`] and [`TimeDomain::end_day_dow`] are interpreted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DayType {
    /// A day of the month, from 1 to 31 (`kYMD`).
    DayOfMonth,
    /// The day of the week of an nth weekday of the month,
    /// from 1 (Sunday) to 7 (Saturday) (`kNthDow`).
    NthDayOfWeek,
}

/// The value of the "last week of the month" for the week fields.
const LAST_WEEK: u8 = 5;

/// A mask of all days of the week.
const ALL_DAYS: u8 = 0b111_1111;

#[bitfield(u64)]
#[derive(PartialEq, Eq)]
struct TimeDomainBits {
    #[bits(1)]
    day_type: u8,
    #[bits(7)]
    dow: u8,
    #[bits(5)]
    begin_hrs: u8,
    #[bits(6)]
    begin_mins: u8,
    #[bits(4)]
    begin_month: u8,
    #[bits(5)]
    begin_day_dow: u8,
    #[bits(3)]
    begin_week: u8,
    #[bits(5)]
    end_hrs: u8,
    #[bits(6)]
    end_mins: u8,
    #[bits(4)]
    end_month: u8,
    #[bits(5)]
    end_day_dow: u8,
    #[bits(3)]
    end_week: u8,
    #[bits(10)]
    spare: u16,
}

/// A date and time range packed into 64 bits (mainline's `baldr::TimeDomain`).
///
/// Every field is 0 when not set.
/// The [`Display`] implementation matches mainline's `TimeDomain::to_string`,
/// which is what mainline writes into JSON output.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TimeDomain(TimeDomainBits);

impl TimeDomain {
    /// Interprets a raw 64-bit value.
    ///
    /// Only the low 54 bits are meaningful;
    /// the rest are ignored (conditional speed limits keep the speed there).
    #[inline]
    pub const fn from_raw(value: u64) -> Self {
        Self(TimeDomainBits::from_bits(value))
    }

    /// The raw 64-bit value.
    #[inline]
    pub const fn raw(&self) -> u64 {
        self.0.into_bits()
    }

    /// How the day fields are interpreted.
    #[inline]
    pub const fn day_type(&self) -> DayType {
        if self.0.day_type() == 0 {
            DayType::DayOfMonth
        } else {
            DayType::NthDayOfWeek
        }
    }

    /// The days of the week the range applies to,
    /// as a mask with bit 0 for Sunday through bit 6 for Saturday
    /// (for example, Monday to Friday is `0b0111110`).
    #[inline]
    pub const fn dow(&self) -> u8 {
        self.0.dow()
    }

    /// The begin hour.
    #[inline]
    pub const fn begin_hrs(&self) -> u8 {
        self.0.begin_hrs()
    }

    /// The begin minute.
    #[inline]
    pub const fn begin_mins(&self) -> u8 {
        self.0.begin_mins()
    }

    /// The begin month, from 1 (January) to 12 (December).
    #[inline]
    pub const fn begin_month(&self) -> u8 {
        self.0.begin_month()
    }

    /// The begin day of the month, or day of the week (see [`TimeDomain::day_type`]).
    #[inline]
    pub const fn begin_day_dow(&self) -> u8 {
        self.0.begin_day_dow()
    }

    /// The begin week of the month, from 1 to 5 (5 is the last week).
    #[inline]
    pub const fn begin_week(&self) -> u8 {
        self.0.begin_week()
    }

    /// The end hour.
    #[inline]
    pub const fn end_hrs(&self) -> u8 {
        self.0.end_hrs()
    }

    /// The end minute.
    #[inline]
    pub const fn end_mins(&self) -> u8 {
        self.0.end_mins()
    }

    /// The end month, from 1 (January) to 12 (December).
    #[inline]
    pub const fn end_month(&self) -> u8 {
        self.0.end_month()
    }

    /// The end day of the month, or day of the week (see [`TimeDomain::day_type`]).
    #[inline]
    pub const fn end_day_dow(&self) -> u8 {
        self.0.end_day_dow()
    }

    /// The end week of the month, from 1 to 5 (5 is the last week).
    #[inline]
    pub const fn end_week(&self) -> u8 {
        self.0.end_week()
    }
}

impl fmt::Debug for TimeDomain {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TimeDomain")
            .field(&format_args!("{:#x}", self.raw()))
            .field(&format_args!("{self}"))
            .finish()
    }
}

/// Mainline's month names, indexed by month (0 is unset).
///
/// Mainline indexes a 13-element array with the 4-bit month,
/// so months 13 to 15 are undefined behavior there; we print nothing for them.
fn month_name(month: u8) -> &'static str {
    const MONTHS: [&str; 13] = [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS.get(usize::from(month)).copied().unwrap_or("")
}

/// Mainline's day names, indexed from 1 (Sunday) to 7 (Saturday) (0 is unset).
///
/// As with months, out-of-range values are undefined behavior in mainline;
/// we print nothing for them.
fn dow_name(dow: u8) -> &'static str {
    const DAYS: [&str; 8] = ["", "Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
    DAYS.get(usize::from(dow)).copied().unwrap_or("")
}

/// Formats a day of the week mask the way mainline's `format_dow` does
/// (`Su,Sa`, `Mo-Fr`, or `Mo-We,Fr`).
fn format_dow(mask: u8, f: &mut Formatter<'_>) -> fmt::Result {
    let mut prev = false;
    let mut preprev = false;
    let mut empty = true;
    for i in 0..7u8 {
        let curr = mask & (1 << i) != 0;
        if curr && !prev {
            if !empty {
                f.write_char(',')?;
            }
            f.write_str(dow_name(i + 1))?;
            empty = false;
        }
        if !curr && prev && preprev {
            write!(f, "-{}", dow_name(i))?;
        }
        preprev = prev;
        prev = curr;
    }
    if prev && preprev {
        f.write_str("-Sa")?;
    }
    Ok(())
}

/// Writes an nth weekday (`Mo[1]`, or `Su[-1]` for the last week) or a plain day number.
fn write_nth_dow(f: &mut Formatter<'_>, day_dow: u8, week: u8) -> fmt::Result {
    if week == 0 {
        write!(f, "{day_dow}")
    } else {
        let nth = if week == LAST_WEEK {
            -1
        } else {
            i32::from(week)
        };
        write!(f, "{}[{nth}]", dow_name(day_dow))
    }
}

impl Display for TimeDomain {
    /// Formats the range exactly as mainline's `TimeDomain::to_string` does
    /// (for example, `Mo-Fr 07:00-09:00` or `Dec Su[-1]-Jan 4`).
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut need_space = false;
        let (bm, bd, bw) = (self.begin_month(), self.begin_day_dow(), self.begin_week());
        let (em, ed, ew) = (self.end_month(), self.end_day_dow(), self.end_week());

        match self.day_type() {
            DayType::DayOfMonth => {
                if bm != 0 {
                    f.write_str(month_name(bm))?;
                    if bd != 0 {
                        write!(f, " {bd}")?;
                    }
                    need_space = true;
                }
                if em != 0 && (em != bm || ed != bd) {
                    if em != bm || ed != 0 {
                        f.write_char('-')?;
                    }
                    f.write_str(month_name(em))?;
                    if ed != 0 {
                        write!(f, " {ed}")?;
                    }
                    need_space = true;
                }
            }
            DayType::NthDayOfWeek => {
                if bm != 0 {
                    write!(f, "{} ", month_name(bm))?;
                }
                write_nth_dow(f, bd, bw)?;
                if ed != 0 && (ed != bd || ew != bw || em != bm) {
                    f.write_char('-')?;
                    if em != bm {
                        write!(f, "{} ", month_name(em))?;
                    }
                    write_nth_dow(f, ed, ew)?;
                }
                need_space = true;
            }
        }

        let dow = self.dow();
        if dow != 0 && dow != ALL_DAYS {
            if need_space {
                f.write_char(' ')?;
            }
            format_dow(dow, f)?;
            need_space = true;
        }

        let (bh, bmin, eh, emin) = (
            self.begin_hrs(),
            self.begin_mins(),
            self.end_hrs(),
            self.end_mins(),
        );
        if bh != 0 || eh != 0 || bmin != 0 || emin != 0 {
            if need_space {
                f.write_char(' ')?;
            }
            write!(f, "{bh:02}:{bmin:02}")?;
            if eh != 0 || emin != 0 {
                write!(f, "-{eh:02}:{emin:02}")?;
            }
        }
        Ok(())
    }
}

/// A speed limit that applies during a time domain
/// (from OSM `maxspeed:conditional`).
///
/// This is mainline's `ConditionalSpeedLimit`:
/// a [`TimeDomain`] with the speed in otherwise spare bits 54 to 61.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ConditionalSpeedLimit(u64);

impl ConditionalSpeedLimit {
    /// The bit offset of the speed.
    const SPEED_SHIFT: u32 = 54;

    /// Interprets a raw 64-bit value.
    #[inline]
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    /// The raw 64-bit value.
    #[inline]
    pub const fn raw(&self) -> u64 {
        self.0
    }

    /// When the limit applies.
    #[inline]
    pub const fn time_domain(&self) -> TimeDomain {
        TimeDomain::from_raw(self.0)
    }

    /// The speed limit in kilometers per hour.
    #[inline]
    pub const fn speed(&self) -> u8 {
        // Masked to 8 bits, so the truncation is exact.
        #[allow(clippy::cast_possible_truncation)]
        let speed = ((self.0 >> Self::SPEED_SHIFT) & 0xff) as u8;
        speed
    }
}

impl fmt::Debug for ConditionalSpeedLimit {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConditionalSpeedLimit")
            .field("speed", &self.speed())
            .field("time_domain", &self.time_domain())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a time domain from fields, the way mainline's setters lay them out.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn td(
        day_type: u8,
        dow: u8,
        begin: (u8, u8, u8, u8, u8),
        end: (u8, u8, u8, u8, u8),
    ) -> TimeDomain {
        // (month, week, day_dow, hrs, mins)
        TimeDomain(
            TimeDomainBits::new()
                .with_day_type(day_type)
                .with_dow(dow)
                .with_begin_month(begin.0)
                .with_begin_week(begin.1)
                .with_begin_day_dow(begin.2)
                .with_begin_hrs(begin.3)
                .with_begin_mins(begin.4)
                .with_end_month(end.0)
                .with_end_week(end.1)
                .with_end_day_dow(end.2)
                .with_end_hrs(end.3)
                .with_end_mins(end.4),
        )
    }

    #[test]
    fn test_bit_layout() {
        // Mo-Fr 07:30-09:45, laid out by hand from the field widths in timedomain.h
        let raw = (0b011_1110 << 1) | (7 << 8) | (30 << 13) | (9 << 31) | (45 << 36);
        let t = TimeDomain::from_raw(raw);
        assert_eq!(t.day_type(), DayType::DayOfMonth);
        assert_eq!(t.dow(), 0b011_1110);
        assert_eq!((t.begin_hrs(), t.begin_mins()), (7, 30));
        assert_eq!((t.end_hrs(), t.end_mins()), (9, 45));
        assert_eq!(t.to_string(), "Mo-Fr 07:30-09:45");
        assert_eq!(t, td(0, 0b011_1110, (0, 0, 0, 7, 30), (0, 0, 0, 9, 45)));
    }

    #[test]
    fn test_format_dow() {
        let cases = [
            (0b100_0001, "Su,Sa"),
            (0b011_1110, "Mo-Fr"),
            (0b010_1110, "Mo-We,Fr"),
            (0b000_0110, "Mo-Tu"),
            (0b000_0010, "Mo"),
            (0b110_0000, "Fr-Sa"),
            (0b111_1110, "Mo-Sa"),
        ];
        for (mask, expected) in cases {
            assert_eq!(
                td(0, mask, (0, 0, 0, 0, 0), (0, 0, 0, 0, 0)).to_string(),
                expected
            );
        }
        // All days and no days are both omitted.
        assert_eq!(
            td(0, ALL_DAYS, (0, 0, 0, 0, 0), (0, 0, 0, 0, 0)).to_string(),
            ""
        );
    }

    #[test]
    fn test_conditional_speed_limit() {
        let t = td(0, 0b011_1110, (0, 0, 0, 7, 0), (0, 0, 0, 9, 0));
        let limit = ConditionalSpeedLimit::from_raw(t.raw() | (30 << 54));
        assert_eq!(limit.speed(), 30);
        // As in mainline's union, the time domain still holds the speed bits.
        assert_eq!(limit.time_domain().raw(), limit.raw());
        assert_eq!(limit.time_domain().raw() & ((1 << 54) - 1), t.raw());
        assert_eq!(limit.time_domain().to_string(), "Mo-Fr 07:00-09:00");
    }

    #[test]
    fn test_out_of_range_names_do_not_panic() {
        // Months 13-15 and days past Saturday are undefined behavior in mainline.
        let t = td(1, 0, (15, 2, 31, 0, 0), (13, 5, 9, 0, 0));
        assert_eq!(t.to_string(), " [2]- [-1]");
    }
}
