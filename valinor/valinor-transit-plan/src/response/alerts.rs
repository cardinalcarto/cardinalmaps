//! Which alerts apply to a leg, and the marks a client needs for them.
//!
//! Alerts are found through the [`valinor_gtfs_rt::RiderData`] indexes, which union every field
//! of every selector, so a candidate must then pass the selector's real AND semantics
//! and be active during the ride.

use std::collections::HashSet;

use valinor_gtfs::CompiledTimetable;
use valinor_gtfs_rt::proto::TranslatedString;
use valinor_gtfs_rt::proto::alert::Effect;
use valinor_gtfs_rt::{
    AlertDetails, CalendarDate, FeedId, InformedEntity, Period, RiderData, ScopedId,
};
use valinor_transit::{ServiceDate, StopId, TripId};

use crate::response::vehicle::lower;
use crate::response::{
    AlertResponse, ImageResponse, InformedEntityResponse, LegAlert, MovedStop, PeriodResponse,
    Translation,
};

/// Which run of a trip a leg rides, to compare with a selector's `start_time`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ServiceRun {
    /// The static trip, if the run has one.
    pub(crate) trip: Option<TripId>,
    /// The service date, in the compiled file's numbering.
    pub(crate) date: ServiceDate,
    /// The run's start in seconds from the start of its service day:
    /// a real-time descriptor's value, else the trip's first departure.
    pub(crate) start_time: Option<u32>,
}

/// A stop on the leg, with its feed-scoped identity.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LegStop<'a> {
    pub(crate) stop: StopId,
    pub(crate) gtfs_id: Option<&'a str>,
}

/// What a leg presents to an alert selector.
#[derive(Debug)]
pub(crate) struct LegSelectorContext<'a> {
    pub(crate) feed: &'a FeedId,
    pub(crate) agency_id: Option<&'a str>,
    pub(crate) route_id: Option<&'a str>,
    pub(crate) route_type: Option<i32>,
    pub(crate) direction_id: Option<u32>,
    pub(crate) trip_id: &'a str,
    pub(crate) date: Option<CalendarDate>,
    /// The run on the service date, for telling apart runs of a frequency trip.
    pub(crate) run: Option<ServiceRun>,
    /// Every stop of the leg, the boarding first and the alighting last.
    pub(crate) stops: Vec<LegStop<'a>>,
    /// When the ride starts, in POSIX seconds.
    pub(crate) from: i64,
    /// When the ride ends, in POSIX seconds.
    pub(crate) to: i64,
}

/// The alerts that apply to a leg, each once, with `STOP_MOVED` marks for its boarding and alighting.
pub(crate) fn leg_alerts<'a>(
    compiled: &CompiledTimetable<'_>,
    rider: &'a RiderData,
    context: &LegSelectorContext<'_>,
) -> Vec<LegAlert> {
    let feed = context.feed;
    let mut candidates: Vec<&'a AlertDetails> = Vec::new();
    let mut seen: HashSet<*const AlertDetails> = HashSet::new();
    let mut push = |alerts: Vec<&'a AlertDetails>| {
        for alert in alerts {
            if seen.insert(std::ptr::from_ref(alert)) {
                candidates.push(alert);
            }
        }
    };
    push(rider.alerts_for_trip(&ScopedId::new(feed, context.trip_id)));
    if let Some(route_id) = context.route_id {
        push(rider.alerts_for_route(&ScopedId::new(feed, route_id)));
    }
    if let Some(agency_id) = context.agency_id {
        push(rider.alerts_for_agency(&ScopedId::new(feed, agency_id)));
    }
    if let Some(route_type) = context.route_type {
        push(rider.alerts_for_route_type(feed, route_type));
    }
    for stop in &context.stops {
        if let Some(gtfs_id) = stop.gtfs_id {
            push(rider.alerts_for_stop(&ScopedId::new(feed, gtfs_id)));
        }
    }

    let mut attached = Vec::new();
    for alert in candidates {
        if !active_between(alert, context.from, context.to) {
            continue;
        }
        let mut matched = false;
        let mut stop_indices: Vec<usize> = Vec::new();
        for selector in &alert.informed {
            if let Some(indices) = selector_matches(compiled, feed, selector, context) {
                matched = true;
                for index in indices {
                    if !stop_indices.contains(&index) {
                        stop_indices.push(index);
                    }
                }
            }
        }
        if !matched {
            continue;
        }
        let moved_stops = moved_stops(alert, &stop_indices, compiled, context);
        attached.push(LegAlert {
            alert: alert_response(alert),
            moved_stops: (!moved_stops.is_empty()).then_some(moved_stops),
        });
    }
    attached
}

/// Whether an alert's service periods overlap a ride.
fn active_between(alert: &AlertDetails, from: i64, to: i64) -> bool {
    // `impact_period` is when service is affected; the older field is the same thing.
    let periods: &[Period] = if alert.impact_periods.is_empty() {
        &alert.active_periods
    } else {
        &alert.impact_periods
    };
    periods.is_empty()
        || periods.iter().any(|period| {
            period.start.is_none_or(|start| start <= to) && period.end.is_none_or(|end| from < end)
        })
}

/// The leg stops an informed entity selects, or `None` if it doesn't select this leg.
///
/// An empty vector means it selects the leg without naming a stop on it.
fn selector_matches(
    compiled: &CompiledTimetable<'_>,
    feed: &FeedId,
    selector: &InformedEntity,
    context: &LegSelectorContext<'_>,
) -> Option<Vec<usize>> {
    if selector.agency_id.is_none()
        && selector.route_id.is_none()
        && selector.route_type.is_none()
        && selector.trip_id.is_none()
        && selector.stop_id.is_none()
    {
        // An empty or direction-only selector informs nothing.
        return None;
    }
    if let Some(agency) = selector.agency_id.as_deref()
        && context.agency_id != Some(agency)
    {
        return None;
    }
    if let Some(route) = selector.route_id.as_deref()
        && context.route_id != Some(route)
    {
        return None;
    }
    if selector.route_type.is_some() && selector.route_type != context.route_type {
        return None;
    }
    if selector.direction_id.is_some() && selector.direction_id != context.direction_id {
        return None;
    }
    if let Some(trip) = selector.trip_id.as_deref()
        && trip != context.trip_id
    {
        return None;
    }
    if selector.trip_date.is_some() && selector.trip_date != context.date {
        return None;
    }
    if !start_time_matches(compiled, feed, selector, context) {
        return None;
    }
    let Some(stop_id) = selector.stop_id.as_deref() else {
        return Some(Vec::new());
    };
    // A selector can name the stop itself or its parent station.
    let at_station = compiled
        .location_by_gtfs_id(feed.as_str(), stop_id)
        .and_then(|location| compiled.stops_at(location));
    let indices: Vec<usize> = context
        .stops
        .iter()
        .enumerate()
        .filter(|(_, stop)| {
            stop.gtfs_id == Some(stop_id)
                || at_station
                    .as_ref()
                    .is_some_and(|members| members.contains(&stop.stop))
        })
        .map(|(index, _)| index)
        .collect();
    (!indices.is_empty()).then_some(indices)
}

/// Whether a selector's `start_time` names the run this leg rides.
///
/// A descriptor with a `trip_id` resolves through the timetable as routing does:
/// to the run itself for a scheduled trip, and to a frequency run by its start.
/// One without a `trip_id` compares starts directly, as routing's run search does.
fn start_time_matches(
    compiled: &CompiledTimetable<'_>,
    feed: &FeedId,
    selector: &InformedEntity,
    context: &LegSelectorContext<'_>,
) -> bool {
    let Some(start) = selector.trip_start_time else {
        return true;
    };
    let Some(run) = context.run else {
        return false;
    };
    let Some(trip_id) = selector.trip_id.as_deref() else {
        return run.start_time == Some(start);
    };
    let Some(static_trip) = run.trip else {
        // A trip only real-time data knows, so only its own start can identify it.
        return run.start_time == Some(start);
    };
    let start = i32::try_from(start).ok();
    compiled
        .trip_run_for_start_time(feed.as_str(), trip_id, run.date, start)
        .is_some_and(|found| found.trip == static_trip)
}

fn moved_stops(
    alert: &AlertDetails,
    stop_indices: &[usize],
    compiled: &CompiledTimetable<'_>,
    context: &LegSelectorContext<'_>,
) -> Vec<MovedStop> {
    if alert.effect != Some(Effect::StopMoved) {
        return Vec::new();
    }
    let last = context.stops.len().saturating_sub(1);
    stop_indices
        .iter()
        .filter_map(|&index| {
            // A moved intermediate stop is shown like any alert: the rider doesn't walk to it.
            let role = match index {
                0 => "boarding",
                i if i == last => "alighting",
                _ => return None,
            };
            let stop = context.stops.get(index)?;
            Some(MovedStop {
                role,
                stop_id: stop.gtfs_id?.to_owned(),
                name: compiled
                    .stop(stop.stop)
                    .and_then(|info| info.name.map(str::to_owned)),
            })
        })
        .collect()
}

/// The JSON form of an alert.
pub(crate) fn alert_response(alert: &AlertDetails) -> AlertResponse {
    AlertResponse {
        feed: alert.feed.as_ref().map(ToString::to_string),
        id: alert.entity_id.clone(),
        cause: alert.cause.map(|cause| lower(cause.as_str_name())),
        effect: alert.effect.map(|effect| lower(effect.as_str_name())),
        severity: alert.severity.map(|severity| lower(severity.as_str_name())),
        active_periods: periods(&alert.active_periods),
        communication_periods: periods(&alert.communication_periods),
        impact_periods: periods(&alert.impact_periods),
        header: translations(alert.header.as_ref()),
        description: translations(alert.description.as_ref()),
        url: translations(alert.url.as_ref()),
        tts_header: translations(alert.tts_header.as_ref()),
        tts_description: translations(alert.tts_description.as_ref()),
        cause_detail: translations(alert.cause_detail.as_ref()),
        effect_detail: translations(alert.effect_detail.as_ref()),
        image: alert.image.as_ref().map_or_else(Vec::new, |image| {
            image
                .localized_image
                .iter()
                .map(|localized| ImageResponse {
                    url: localized.url.clone(),
                    media_type: localized.media_type.clone(),
                    language: localized.language.clone(),
                })
                .collect()
        }),
        image_alternative_text: translations(alert.image_alternative_text.as_ref()),
        informed: alert.informed.iter().map(informed_entity).collect(),
    }
}

fn periods(periods: &[Period]) -> Vec<PeriodResponse> {
    periods
        .iter()
        .map(|period| PeriodResponse {
            start: period.start,
            end: period.end,
        })
        .collect()
}

fn translations(text: Option<&TranslatedString>) -> Vec<Translation> {
    text.map_or_else(Vec::new, |text| {
        text.translation
            .iter()
            .map(|translation| Translation {
                text: translation.text.clone(),
                language: translation.language.clone(),
            })
            .collect()
    })
}

fn informed_entity(entity: &InformedEntity) -> InformedEntityResponse {
    InformedEntityResponse {
        agency_id: entity.agency_id.clone(),
        route_id: entity.route_id.clone(),
        route_type: entity.route_type,
        direction_id: entity.direction_id,
        trip_id: entity.trip_id.clone(),
        trip_date: entity.trip_date.map(|date| date.to_string()),
        trip_start_time: entity.trip_start_time,
        stop_id: entity.stop_id.clone(),
    }
}
