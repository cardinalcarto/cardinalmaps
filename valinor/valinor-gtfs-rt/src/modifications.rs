//! Applying `TripModifications`: detours that replace stops of a trip with other stops.

use thiserror::Error;
use valinor_transit::{StopEvent, StopId, StopRules, Time};

use crate::propagate::repair;
use crate::proto::StopSelector;
use crate::proto::trip_modifications::Modification;

/// A trip's stops and times after modifications, which then may go on to be updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModifiedLayout {
    pub(crate) stops: Vec<StopId>,
    pub(crate) events: Vec<StopEvent>,
    /// The original `stop_sequence`, for stops not replaced; replacements have none.
    pub(crate) sequences: Vec<Option<u32>>,
}

/// Why a modification can't be applied to a trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum ModificationError {
    #[error("find the stop a modification selects")]
    UnknownSelector,
    #[error("apply modifications whose stop ranges overlap")]
    Overlap,
    #[error("resolve a replacement stop that isn't in the feed or timetable")]
    UnknownReplacementStop,
    #[error("apply a modification that leaves fewer than two stops")]
    TooShort,
    #[error("hold a modified time inside the window")]
    TimeOutOfRange,
}

/// Finds the position a stop selector names.
fn select(
    selector: &StopSelector,
    stops: &[StopId],
    sequences: &[u32],
    resolve_stop: &dyn Fn(&str) -> Option<StopId>,
) -> Option<usize> {
    if let Some(sequence) = selector.stop_sequence {
        return sequences.iter().position(|&s| s == sequence);
    }
    let stop = resolve_stop(selector.stop_id.as_deref()?)?;
    stops.iter().position(|&s| s == stop)
}

/// Applies `modifications`, in stop order, to a scheduled trip.
///
/// Each replaced range's stops are swapped for the replacement stops, timed from the
/// reference stop, and `propagated_modification_delay` accumulates over later stops.
///
/// # Errors
///
/// If a selector or a replacement stop can't be resolved, ranges overlap, too few stops remain,
/// a time falls out of range, or a time or delay is further than `max_offset` seconds from
/// its reference.
#[allow(clippy::too_many_lines)]
pub(crate) fn apply_modifications(
    stops: &[StopId],
    events: &[StopEvent],
    sequences: &[u32],
    modifications: &[Modification],
    max_offset: i64,
    resolve_stop: &dyn Fn(&str) -> Option<StopId>,
) -> Result<ModifiedLayout, ModificationError> {
    // Stable: positions are unique except for equal starts, which are overlaps anyway.
    let mut ranges = Vec::with_capacity(modifications.len());
    for modification in modifications {
        let start = modification
            .start_stop_selector
            .as_ref()
            .and_then(|s| select(s, stops, sequences, resolve_stop))
            .ok_or(ModificationError::UnknownSelector)?;
        let end = match modification.end_stop_selector.as_ref() {
            Some(selector) => {
                select(selector, stops, sequences, resolve_stop)
                    .filter(|&end| end >= start)
                    .ok_or(ModificationError::UnknownSelector)?
                    + 1
            }
            // No end selector means an insertion before `start`, with nothing removed.
            None => start,
        };
        ranges.push((start, end, modification));
    }
    ranges.sort_by_key(|&(start, _, _)| start);

    let mut out_stops = Vec::with_capacity(stops.len());
    let mut arrivals: Vec<i64> = Vec::with_capacity(stops.len());
    let mut departures: Vec<i64> = Vec::with_capacity(stops.len());
    let mut rules = Vec::with_capacity(stops.len());
    let mut out_sequences = Vec::with_capacity(stops.len());
    let mut delay = 0_i64;
    let mut cursor = 0_usize;

    let copy_stop = |position: usize,
                     delay: i64,
                     out_stops: &mut Vec<StopId>,
                     arrivals: &mut Vec<i64>,
                     departures: &mut Vec<i64>,
                     rules: &mut Vec<StopRules>,
                     out_sequences: &mut Vec<Option<u32>>| {
        let event = events[position];
        out_stops.push(stops[position]);
        arrivals.push(i64::from(event.arrival.into_inner()) + delay);
        departures.push(i64::from(event.departure.into_inner()) + delay);
        rules.push(event.rules);
        out_sequences.push(Some(sequences[position]));
    };

    let within = |seconds: i32| i64::from(seconds).unsigned_abs() <= max_offset.unsigned_abs();
    for (start, end, modification) in ranges {
        if !within(
            modification
                .propagated_modification_delay
                .unwrap_or_default(),
        ) || modification
            .replacement_stops
            .iter()
            .any(|r| !within(r.travel_time_to_stop.unwrap_or_default()))
        {
            return Err(ModificationError::TimeOutOfRange);
        }
        if start < cursor || end < start {
            return Err(ModificationError::Overlap);
        }
        for position in cursor..start {
            copy_stop(
                position,
                delay,
                &mut out_stops,
                &mut arrivals,
                &mut departures,
                &mut rules,
                &mut out_sequences,
            );
        }
        // The reference is the stop before the modified range, or the trip's first stop
        // when the range begins there.
        let reference = arrivals
            .last()
            .copied()
            .or_else(|| events.first().map(|e| i64::from(e.arrival.into_inner())))
            .ok_or(ModificationError::TooShort)?;
        for replacement in &modification.replacement_stops {
            let stop = replacement
                .stop_id
                .as_deref()
                .and_then(resolve_stop)
                .ok_or(ModificationError::UnknownReplacementStop)?;
            let time = reference.saturating_add(i64::from(
                replacement.travel_time_to_stop.unwrap_or_default(),
            ));
            out_stops.push(stop);
            arrivals.push(time);
            departures.push(time);
            rules.push(StopRules::REGULAR);
            out_sequences.push(None);
        }
        delay = delay.saturating_add(i64::from(
            modification
                .propagated_modification_delay
                .unwrap_or_default(),
        ));
        cursor = end;
    }
    for position in cursor..stops.len() {
        copy_stop(
            position,
            delay,
            &mut out_stops,
            &mut arrivals,
            &mut departures,
            &mut rules,
            &mut out_sequences,
        );
    }

    if out_stops.len() < 2 {
        return Err(ModificationError::TooShort);
    }
    repair(&mut arrivals, &mut departures, Some(0));
    let mut out_events = Vec::with_capacity(out_stops.len());
    for ((&arrival, &departure), &rule) in arrivals.iter().zip(&departures).zip(&rules) {
        out_events.push(StopEvent {
            arrival: time(arrival)?,
            departure: time(departure)?,
            rules: rule,
        });
    }
    Ok(ModifiedLayout {
        stops: out_stops,
        events: out_events,
        sequences: out_sequences,
    })
}

fn time(seconds: i64) -> Result<Time, ModificationError> {
    i32::try_from(seconds)
        .ok()
        .and_then(|seconds| Time::try_new(seconds).ok())
        .ok_or(ModificationError::TimeOutOfRange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::ReplacementStop;

    const MAX_OFFSET: i64 = 12 * 3600;

    fn t(seconds: i32) -> Time {
        Time::try_new(seconds).expect("in range")
    }

    fn events(times: &[i32]) -> Vec<StopEvent> {
        times
            .iter()
            .map(|&time| StopEvent {
                arrival: t(time),
                departure: t(time),
                rules: StopRules::REGULAR,
            })
            .collect()
    }

    fn stops(ids: &[u32]) -> Vec<StopId> {
        ids.iter().map(|&id| StopId::new(id)).collect()
    }

    fn selector(sequence: u32) -> StopSelector {
        StopSelector {
            stop_sequence: Some(sequence),
            stop_id: None,
        }
    }

    fn replacement(travel: i32, stop: &str) -> ReplacementStop {
        ReplacementStop {
            travel_time_to_stop: Some(travel),
            stop_id: Some(stop.into()),
        }
    }

    fn resolver(id: &str) -> Option<StopId> {
        id.parse().ok().map(StopId::new)
    }

    fn modification(
        start: u32,
        end: Option<u32>,
        replacements: Vec<ReplacementStop>,
    ) -> Modification {
        Modification {
            start_stop_selector: Some(selector(start)),
            end_stop_selector: end.map(selector),
            propagated_modification_delay: None,
            replacement_stops: replacements,
            service_alert_id: None,
            last_modified_time: None,
        }
    }

    #[test]
    fn a_detour_replaces_stops_and_times_them_from_the_reference() {
        let mut detour = modification(2, Some(3), vec![replacement(50, "99")]);
        detour.propagated_modification_delay = Some(30);
        let layout = apply_modifications(
            &stops(&[10, 20, 30, 40]),
            &events(&[0, 100, 200, 300]),
            &[1, 2, 3, 4],
            &[detour],
            MAX_OFFSET,
            &resolver,
        )
        .expect("valid");
        assert_eq!(layout.stops, stops(&[10, 99, 40]));
        let arrivals: Vec<i32> = layout
            .events
            .iter()
            .map(|e| e.arrival.into_inner())
            .collect();
        assert_eq!(arrivals, vec![0, 50, 330]);
        assert_eq!(layout.sequences, vec![Some(1), None, Some(4)]);
    }

    #[test]
    fn a_modification_at_the_first_stop_may_reach_backward() {
        let layout = apply_modifications(
            &stops(&[10, 20]),
            &events(&[100, 200]),
            &[1, 2],
            &[modification(1, Some(1), vec![replacement(-60, "99")])],
            MAX_OFFSET,
            &resolver,
        )
        .expect("valid");
        assert_eq!(layout.stops, stops(&[99, 20]));
        assert_eq!(layout.events[0].arrival, t(40));
    }

    #[test]
    fn no_end_selector_inserts_without_removing() {
        let layout = apply_modifications(
            &stops(&[10, 20]),
            &events(&[0, 100]),
            &[1, 2],
            &[modification(2, None, vec![replacement(40, "99")])],
            MAX_OFFSET,
            &resolver,
        )
        .expect("valid");
        assert_eq!(layout.stops, stops(&[10, 99, 20]));
    }

    #[test]
    fn delays_accumulate_across_modifications() {
        let mut first = modification(2, Some(2), vec![replacement(10, "91")]);
        first.propagated_modification_delay = Some(5);
        let mut second = modification(3, Some(3), vec![replacement(10, "92")]);
        second.propagated_modification_delay = Some(7);
        let layout = apply_modifications(
            &stops(&[10, 20, 30, 40]),
            &events(&[0, 100, 200, 300]),
            &[1, 2, 3, 4],
            &[second, first],
            MAX_OFFSET,
            &resolver,
        )
        .expect("valid");
        assert_eq!(layout.stops, stops(&[10, 91, 92, 40]));
        assert_eq!(layout.events[3].arrival, t(312));
    }

    #[test]
    fn overlapping_ranges_are_rejected() {
        let result = apply_modifications(
            &stops(&[10, 20, 30]),
            &events(&[0, 100, 200]),
            &[1, 2, 3],
            &[
                modification(1, Some(2), vec![replacement(10, "91")]),
                modification(2, Some(3), vec![replacement(10, "92")]),
            ],
            MAX_OFFSET,
            &resolver,
        );
        assert_eq!(result, Err(ModificationError::Overlap));
    }

    #[test]
    fn unknown_stops_and_selectors_are_rejected() {
        let trip = (stops(&[10, 20]), events(&[0, 100]));
        let bad_stop = apply_modifications(
            &trip.0,
            &trip.1,
            &[1, 2],
            &[modification(1, Some(1), vec![replacement(10, "nope")])],
            MAX_OFFSET,
            &resolver,
        );
        assert_eq!(bad_stop, Err(ModificationError::UnknownReplacementStop));
        let bad_selector = apply_modifications(
            &trip.0,
            &trip.1,
            &[1, 2],
            &[modification(9, None, vec![])],
            MAX_OFFSET,
            &resolver,
        );
        assert_eq!(bad_selector, Err(ModificationError::UnknownSelector));
    }

    #[test]
    fn a_travel_time_past_the_limit_is_rejected() {
        let result = apply_modifications(
            &stops(&[10, 20]),
            &events(&[0, 100]),
            &[1, 2],
            &[modification(2, Some(2), vec![replacement(1_000_000, "99")])],
            MAX_OFFSET,
            &resolver,
        );
        assert_eq!(result, Err(ModificationError::TimeOutOfRange));
    }

    #[test]
    fn removing_almost_every_stop_is_too_short() {
        let result = apply_modifications(
            &stops(&[10, 20]),
            &events(&[0, 100]),
            &[1, 2],
            &[modification(1, Some(2), vec![replacement(10, "99")])],
            MAX_OFFSET,
            &resolver,
        );
        assert_eq!(result, Err(ModificationError::TooShort));
    }
}
