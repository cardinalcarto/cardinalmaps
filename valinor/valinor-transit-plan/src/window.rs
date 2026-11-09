//! Service windows: which days are materialized, and the clock they share.

use std::sync::Arc;

use valinor_gtfs::{CompiledTimetable, civil_date_of};
use valinor_gtfs_rt::{CalendarDate, WindowDay};
use valinor_transit::{BaseWindow, ServiceDate, ServiceDay, Timetable};

use crate::error::StateError;

/// The most service days a window may hold; see [`required_dates`].
pub(crate) const MAX_WINDOW_DAYS: usize = 16;

/// One materialized window of service days, shared by every snapshot built on it.
#[derive(Debug)]
pub(crate) struct WindowData {
    /// The service dates of the window, in slot order.
    pub(crate) dates: Vec<ServiceDate>,
    /// The POSIX time of the window clock's zero.
    pub(crate) origin_unix: i64,
    /// When each day starts in POSIX seconds, for the overlay builder.
    pub(crate) days: Vec<WindowDay>,
    /// Where each day is on the window's clock.
    pub(crate) service_days: Vec<ServiceDay>,
    /// The days' active trips, laid out for scanning.
    pub(crate) base: Arc<BaseWindow>,
}

impl WindowData {
    /// Materializes the days `dates`, in the order given.
    pub(crate) fn materialize(
        compiled: &CompiledTimetable<'_>,
        timetable: &Timetable,
        dates: Vec<ServiceDate>,
    ) -> Result<Self, StateError> {
        let window = compiled.service_window(&dates)?;
        let mut days = Vec::with_capacity(dates.len());
        for date in &dates {
            let start_unix = compiled
                .service_day_start_unix(*date)
                .ok_or(valinor_gtfs::DecodeError::DateNotCovered(date.into_inner()))?;
            let calendar = calendar_date(*date)
                .ok_or(valinor_gtfs::DecodeError::DateNotCovered(date.into_inner()))?;
            days.push(WindowDay {
                date: calendar,
                start_unix,
            });
        }
        let base = timetable.materialize(&window.days)?;
        Ok(Self {
            dates,
            origin_unix: window.origin_unix,
            days,
            service_days: window.days,
            base: Arc::new(base),
        })
    }

    /// The POSIX time at which `date`'s service day starts, if it is in this window.
    pub(crate) fn day_start_unix(&self, date: ServiceDate) -> Option<i64> {
        let slot = self.dates.iter().position(|d| *d == date)?;
        self.days.get(slot).map(|day| day.start_unix)
    }

    /// Whether every date is in this window.
    pub(crate) fn covers(&self, dates: &[ServiceDate]) -> bool {
        dates.iter().all(|date| self.dates.contains(date))
    }
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

/// The service date running at a POSIX time, or `None` outside the timetable's coverage.
pub(crate) fn service_date_at(compiled: &CompiledTimetable<'_>, unix: i64) -> Option<ServiceDate> {
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

/// The window a rider standing at `center` needs: yesterday, today, and tomorrow.
pub(crate) fn dates_around(
    compiled: &CompiledTimetable<'_>,
    center: ServiceDate,
) -> Vec<ServiceDate> {
    dates_between(compiled, center, center)
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
/// If `earliest` is outside the timetable's coverage, or the span needs too long a window.
pub(crate) fn required_dates(
    compiled: &CompiledTimetable<'_>,
    earliest: i64,
    latest: i64,
) -> Result<Vec<ServiceDate>, StateError> {
    let first = service_date_at(compiled, earliest).ok_or(StateError::DateNotCovered(earliest))?;
    let last = service_date_at(compiled, latest).unwrap_or(first);
    let dates = dates_between(compiled, first, last);
    if dates.len() > MAX_WINDOW_DAYS {
        return Err(StateError::WindowTooLong(dates.len()));
    }
    Ok(dates)
}
