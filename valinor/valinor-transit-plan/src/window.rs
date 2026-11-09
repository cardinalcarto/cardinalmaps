//! Service windows: which days are materialized, and the clock they share.
//!
//! Every loaded file keeps its own service-day clock, and all of them are brought onto one window
//! clock whose zero is the earliest service day start of any file in the window.
//! A slot is a service date; the same date starts at a different instant in each time zone,
//! so one slot has a different offset in each file ([`PartDay`]).

use std::collections::BTreeSet;
use std::sync::Arc;

use valinor_gtfs::{CompiledTimetable, TimetableSet, civil_date_of};
use valinor_gtfs_rt::{CalendarDate, WindowDay};
use valinor_transit::{
    BaseWindow, ServiceDate, ServiceDay, Time, Timetable, WindowPart, materialize_parts,
};

use crate::error::StateError;

/// The most service days a window may hold; see [`required_dates`].
pub(crate) const MAX_WINDOW_DAYS: usize = 16;

/// One file's service day of one slot, on the window's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PartDay {
    /// When the day starts, in POSIX seconds.
    pub(crate) start_unix: i64,
    /// Where the day starts on the window's clock.
    pub(crate) offset: Time,
}

/// One materialized window of service days, shared by every snapshot built on it.
#[derive(Debug)]
pub(crate) struct WindowData {
    /// The service dates of the window, in slot order.
    pub(crate) dates: Vec<ServiceDate>,
    /// The POSIX time of the window clock's zero.
    pub(crate) origin_unix: i64,
    /// When each day starts in POSIX seconds, for the overlay builder.
    ///
    /// This is the earliest start among the files that have the date.
    pub(crate) days: Vec<WindowDay>,
    /// Per file, per slot, that file's own clock; `None` where the file doesn't cover the date.
    pub(crate) part_days: Vec<Vec<Option<PartDay>>>,
    /// The days' active trips of every file, laid out for scanning.
    pub(crate) base: Arc<BaseWindow>,
}

impl WindowData {
    /// Materializes the days `dates` of every file, in the order given.
    pub(crate) fn materialize(
        set: &TimetableSet,
        timetables: &[Timetable],
        dates: Vec<ServiceDate>,
    ) -> Result<Self, StateError> {
        let uncovered = |date: ServiceDate| {
            StateError::Timetable(valinor_gtfs::DecodeError::DateNotCovered(date.into_inner()))
        };
        let origin_unix = set
            .parts()
            .flat_map(|part| {
                dates
                    .iter()
                    .filter_map(move |date| part.compiled().service_day_start_unix(*date))
            })
            .min()
            .ok_or_else(|| uncovered(dates.first().copied().unwrap_or(ServiceDate::new(0))))?;

        let mut days = Vec::with_capacity(dates.len());
        let mut service_days = Vec::with_capacity(dates.len());
        for &date in &dates {
            let start_unix = set
                .parts()
                .filter_map(|part| part.compiled().service_day_start_unix(date))
                .min()
                .ok_or_else(|| uncovered(date))?;
            let calendar = calendar_date(date).ok_or_else(|| uncovered(date))?;
            days.push(WindowDay {
                date: calendar,
                start_unix,
            });
            service_days.push(ServiceDay {
                date,
                offset: window_offset(start_unix, origin_unix)?,
            });
        }

        let mut part_days = Vec::with_capacity(set.part_count());
        for part in set.parts() {
            let mut days_of_part = Vec::with_capacity(dates.len());
            for &date in &dates {
                let day = match part.compiled().service_day_start_unix(date) {
                    Some(start_unix) => Some(PartDay {
                        start_unix,
                        offset: window_offset(start_unix, origin_unix)?,
                    }),
                    None => None,
                };
                days_of_part.push(day);
            }
            part_days.push(days_of_part);
        }

        let offsets: Vec<Vec<Option<Time>>> = part_days
            .iter()
            .map(|days| days.iter().map(|day| day.map(|d| d.offset)).collect())
            .collect();
        let parts: Vec<WindowPart<'_>> = set
            .parts()
            .zip(timetables)
            .zip(&offsets)
            .map(|((part, timetable), offsets)| WindowPart {
                timetable,
                first_trip: part.first_trip(),
                day_offsets: offsets,
            })
            .collect();
        let base = materialize_parts(&parts, &service_days)?;
        Ok(Self {
            dates,
            origin_unix,
            days,
            part_days,
            base: Arc::new(base),
        })
    }

    /// The POSIX time at which `date`'s service day starts in file `part`, if it has that date.
    pub(crate) fn day_start_unix(&self, part: usize, date: ServiceDate) -> Option<i64> {
        let slot = self.dates.iter().position(|d| *d == date)?;
        self.part_days
            .get(part)?
            .get(slot)?
            .map(|day| day.start_unix)
    }

    /// Whether every date is in this window.
    pub(crate) fn covers(&self, dates: &[ServiceDate]) -> bool {
        dates.iter().all(|date| self.dates.contains(date))
    }
}

fn window_offset(start_unix: i64, origin_unix: i64) -> Result<Time, StateError> {
    i32::try_from(start_unix - origin_unix)
        .ok()
        .and_then(|offset| Time::try_new(offset).ok())
        .ok_or(StateError::Timetable(
            valinor_gtfs::DecodeError::InvalidValue {
                table: "window",
                reason: "a day's offset is out of range",
            },
        ))
}

/// The GTFS-RT `YYYYMMDD` form of a service date.
pub(crate) fn calendar_date(date: ServiceDate) -> Option<CalendarDate> {
    let civil = civil_date_of(date).ok()?;
    CalendarDate::parse(&format!(
        "{:04}{:02}{:02}",
        civil.year(),
        civil.month(),
        civil.day()
    ))
}

/// The service date running in one file at a POSIX time, or `None` outside its coverage.
fn service_date_at(compiled: &CompiledTimetable<'_>, unix: i64) -> Option<ServiceDate> {
    // A service day starts within about half a day of the UTC date of its own number,
    // so the answer is one of these three, and the latest whose start has passed is it.
    let utc_day = unix.div_euclid(86_400);
    [utc_day + 1, utc_day, utc_day - 1]
        .into_iter()
        .filter_map(|day| u32::try_from(day).ok().map(ServiceDate::new))
        .find(|&date| {
            compiled
                .service_day_start_unix(date)
                .is_some_and(|start| start <= unix)
        })
}

/// The window riders at `unix` need: yesterday, today, and tomorrow in each file's own time zone.
///
/// Empty when no file covers `unix`. Files are in different time zones,
/// so different files may be on different service dates at one moment, and all of them are kept.
pub(crate) fn dates_around(set: &TimetableSet, unix: i64) -> Vec<ServiceDate> {
    let mut dates = BTreeSet::new();
    for part in set.parts() {
        if let Some(center) = service_date_at(part.compiled(), unix) {
            dates.extend(dates_between(part.compiled(), center, center));
        }
    }
    dates.into_iter().collect()
}

fn dates_between(
    compiled: &CompiledTimetable<'_>,
    first: ServiceDate,
    last: ServiceDate,
) -> Vec<ServiceDate> {
    // Yesterday's trips run past midnight, and a journey can continue onto tomorrow's.
    let from = first.into_inner().saturating_sub(1);
    let to = last.into_inner().saturating_add(1);
    (from..=to)
        .map(ServiceDate::new)
        .filter(|date| compiled.service_day_start_unix(*date).is_some())
        .collect()
}

/// The service days a query departing between two POSIX times needs materialized.
///
/// # Errors
///
/// If no file covers `earliest`, or the span needs too long a window.
pub(crate) fn required_dates(
    set: &TimetableSet,
    earliest: i64,
    latest: i64,
) -> Result<Vec<ServiceDate>, StateError> {
    if !set
        .parts()
        .any(|part| service_date_at(part.compiled(), earliest).is_some())
    {
        return Err(StateError::DateNotCovered(earliest));
    }
    let mut dates = BTreeSet::new();
    for part in set.parts() {
        let compiled = part.compiled();
        let first = service_date_at(compiled, earliest);
        let last = service_date_at(compiled, latest);
        let (Some(first), Some(last)) = (first.or(last), last.or(first)) else {
            continue;
        };
        dates.extend(dates_between(compiled, first, last));
    }
    if dates.len() > MAX_WINDOW_DAYS {
        return Err(StateError::WindowTooLong(dates.len()));
    }
    Ok(dates.into_iter().collect())
}
