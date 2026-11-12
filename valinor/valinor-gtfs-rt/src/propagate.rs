//! Applying a trip's stop time updates to its scheduled times, by the GTFS-RT rules.
//!
//! - A stop's update applies to every following stop until the next update.
//! - `SKIPPED` affects only its own stop, and delays propagate across it.
//! - `NO_DATA` clears the delay until the next update.
//! - Stops before the first update keep their scheduled times.
//! - The trip's overall `delay` applies from the start of the trip until the first stop's delay.
//! - A prediction further from the schedule than the configured limit is ignored.
//!
//! Updates can disagree with the schedule or with one another,
//! but the route scan requires times to move forward along each trip,
//! so the result is repaired: stops before the first update move back when they must,
//! and times from the first update on move forward (see [`repair`]).

use valinor_transit::{StopEvent, StopId, StopRules, Time};

use crate::lookup::DayClock;
use crate::proto::trip_update::StopTimeEvent;
use crate::proto::trip_update::StopTimeUpdate;
use crate::proto::trip_update::stop_time_update::ScheduleRelationship as StopRelationship;
use crate::proto::trip_update::stop_time_update::stop_time_properties::DropOffPickupType;

/// What's predicted at one stop position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Prediction {
    /// The predicted arrival; `None` when no information covers it.
    pub(crate) arrival: Option<Time>,
    /// The predicted departure; `None` when no information covers it.
    pub(crate) departure: Option<Time>,
    /// Which of the trip's stop time updates gave this stop information.
    pub(crate) update: Option<usize>,
}

/// A trip's times after its updates are applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Propagated {
    pub(crate) events: Vec<StopEvent>,
    pub(crate) predictions: Vec<Prediction>,
    /// How many of the update's times had to be moved to keep the trip's times in order.
    pub(crate) repaired: u32,
    /// How many stop time updates matched no stop of the trip.
    pub(crate) unmatched: u32,
    /// How many event times were out of the window's range and ignored.
    pub(crate) out_of_range: u32,
}

impl Propagated {
    /// Whether routing sees any difference from `scheduled`.
    pub(crate) fn differs_from(&self, scheduled: &[StopEvent]) -> bool {
        self.events != scheduled
    }
}

/// The input to [`propagate`].
pub(crate) struct Trip<'a> {
    pub(crate) events: &'a [StopEvent],
    pub(crate) sequences: &'a [Option<u32>],
    pub(crate) stops: &'a [StopId],
    /// How far a prediction may be from the schedule, in seconds, before it is ignored.
    pub(crate) max_offset: i64,
}

/// Pairs stop time updates with the stop positions of a trip.
///
/// Returns `(position, update index)`, positions strictly increasing, and how many didn't match.
fn match_updates(
    trip: &Trip<'_>,
    updates: &[StopTimeUpdate],
    clock: DayClock,
    resolve_stop: &dyn Fn(&str) -> Option<StopId>,
) -> (Vec<(usize, usize)>, u32) {
    let mut matched = Vec::with_capacity(updates.len());
    let mut unmatched = 0;
    // The specification requires updates sorted by `stop_sequence`, so each is looked for only
    // after the last one matched. Loop routes visit a stop twice, and this is what keeps
    // stop-ID matching on the right visit.
    let mut cursor = 0_usize;
    for (index, update) in updates.iter().enumerate() {
        let position = if let Some(sequence) = update.stop_sequence {
            let tail = trip.sequences.get(cursor..).unwrap_or(&[]);
            tail.iter()
                .position(|&s| s == Some(sequence))
                .map(|offset| cursor + offset)
        } else if let Some(stop_id) = update.stop_id.as_deref() {
            resolve_stop(stop_id).and_then(|stop| visit_of(trip, stop, update, cursor, clock))
        } else {
            None
        };
        match position {
            Some(position) => {
                matched.push((position, index));
                cursor = position + 1;
            }
            None => unmatched += 1,
        }
    }
    (matched, unmatched)
}

/// Picks which visit to `stop`, at or after `cursor`, a `stop_id`-only update means.
///
/// With one candidate there is nothing to decide. On a loop, the update's absolute time picks
/// the nearest scheduled visit, as producers may have dropped earlier stops.
/// A delay alone can't say which visit it means, so that update is ambiguous and matches nothing.
fn visit_of(
    trip: &Trip<'_>,
    stop: StopId,
    update: &StopTimeUpdate,
    cursor: usize,
    clock: DayClock,
) -> Option<usize> {
    let tail = trip.stops.get(cursor..).unwrap_or(&[]);
    let mut visits = tail
        .iter()
        .enumerate()
        .filter(|&(_, &s)| s == stop)
        .map(|(offset, _)| cursor + offset);
    let first = visits.next()?;
    if !tail[first - cursor + 1..].contains(&stop) {
        return Some(first);
    }
    let predicted = update
        .arrival
        .as_ref()
        .or(update.departure.as_ref())
        .and_then(|event| event.time)
        .filter(|&time| time > 0)
        .map(|time| clock.window_seconds(time))?;
    core::iter::once(first)
        .chain(visits)
        .min_by_key(|&position| seconds_of(trip.events[position].arrival).abs_diff(predicted))
}

/// The window time an event gives, against the time scheduled for it.
///
/// An absolute `time` is the prediction itself; `delay` is relative to the schedule.
/// `time` of zero or less is the protobuf default or a data error, and counts as absent.
/// A result too far from the schedule, or outside the window's range, is ignored and counted.
fn event_seconds(
    event: Option<&StopTimeEvent>,
    scheduled: i64,
    clock: DayClock,
    max_offset: i64,
    out_of_range: &mut u32,
) -> Option<i64> {
    let event = event?;
    let seconds = match event.time.filter(|&time| time > 0) {
        Some(time) => clock.window_seconds(time),
        None => scheduled.saturating_add(i64::from(event.delay?)),
    };
    if seconds.abs_diff(scheduled) > max_offset.unsigned_abs() || window_time(seconds).is_err() {
        *out_of_range += 1;
        return None;
    }
    Some(seconds)
}

fn seconds_to_i32(seconds: i64) -> Option<i32> {
    i32::try_from(seconds).ok()
}

fn seconds_of(time: Time) -> i64 {
    i64::from(time.into_inner())
}

/// Applies the feed's stop-level overrides to the scheduled pickup and drop-off rules.
pub(crate) fn rules_of(update: Option<&StopTimeUpdate>, scheduled: StopRules) -> StopRules {
    let Some(update) = update else {
        return scheduled;
    };
    if update.schedule_relationship() == StopRelationship::Skipped {
        return StopRules::SKIPPED;
    }
    let Some(properties) = update.stop_time_properties.as_ref() else {
        return scheduled;
    };
    StopRules {
        pickup: properties.pickup_type.map_or(scheduled.pickup, |_| {
            properties.pickup_type() != DropOffPickupType::None
        }),
        drop_off: properties.drop_off_type.map_or(scheduled.drop_off, |_| {
            properties.drop_off_type() != DropOffPickupType::None
        }),
    }
}

/// Applies `updates` to a trip; `trip_delay` is the trip update's own `delay`.
///
/// # Errors
///
/// `Err(())` only when propagated times leave the range [`Time`] can hold.
#[allow(clippy::too_many_lines)]
pub(crate) fn propagate(
    trip: &Trip<'_>,
    updates: &[StopTimeUpdate],
    trip_delay: Option<i32>,
    clock: DayClock,
    resolve_stop: &dyn Fn(&str) -> Option<StopId>,
) -> Result<Propagated, ()> {
    let (matched, unmatched) = match_updates(trip, updates, clock, resolve_stop);
    let mut out_of_range = 0;
    let stops = trip.events.len();
    let mut arrivals: Vec<i64> = trip.events.iter().map(|e| seconds_of(e.arrival)).collect();
    let mut departures: Vec<i64> = trip
        .events
        .iter()
        .map(|e| seconds_of(e.departure))
        .collect();
    let mut rules: Vec<StopRules> = trip.events.iter().map(|e| e.rules).collect();
    let mut known = vec![(false, false); stops];
    let mut predictions = vec![Prediction::default(); stops];

    // The trip's `delay` holds from the first stop until a stop gives its own.
    let mut delay: Option<i64> = trip_delay.map(i64::from).filter(|delay| {
        let in_bounds = delay.unsigned_abs() <= trip.max_offset.unsigned_abs();
        if !in_bounds {
            out_of_range += 1;
        }
        in_bounds
    });
    let mut first_timed: Option<usize> = delay.map(|_| 0);

    let mut next = matched.iter().copied().peekable();
    for position in 0..stops {
        let update = if next.peek().is_some_and(|&(p, _)| p == position) {
            next.next().and_then(|(_, index)| {
                let update = updates.get(index)?;
                predictions[position].update = Some(index);
                Some(update)
            })
        } else {
            None
        };
        rules[position] = rules_of(update, rules[position]);
        let (mut arrival, mut departure) = (arrivals[position], departures[position]);
        let scheduled_arrival = arrival;
        let scheduled_departure = departure;
        match update.map(|u| (u, u.schedule_relationship())) {
            Some((_, StopRelationship::NoData)) => {
                delay = None;
                continue;
            }
            Some((_, StopRelationship::Skipped)) | None => {
                // The stop has no timing of its own (or is skipped): the running delay carries over.
                if let Some(d) = delay {
                    arrival = scheduled_arrival.saturating_add(d);
                    departure = scheduled_departure.saturating_add(d);
                    known[position] = (update.is_none(), update.is_none());
                }
            }
            Some((update, StopRelationship::Scheduled | StopRelationship::Unscheduled)) => {
                let arrival_time = event_seconds(
                    update.arrival.as_ref(),
                    scheduled_arrival,
                    clock,
                    trip.max_offset,
                    &mut out_of_range,
                );
                let departure_time = event_seconds(
                    update.departure.as_ref(),
                    scheduled_departure,
                    clock,
                    trip.max_offset,
                    &mut out_of_range,
                );
                if let Some(time) = arrival_time {
                    arrival = time;
                    delay = Some(time.saturating_sub(scheduled_arrival));
                } else if let Some(d) = delay {
                    arrival = scheduled_arrival.saturating_add(d);
                }
                if let Some(time) = departure_time {
                    departure = time;
                    delay = Some(time.saturating_sub(scheduled_departure));
                    if arrival_time.is_none() && arrival > departure {
                        arrival = departure;
                    }
                } else if let Some(d) = delay {
                    departure = scheduled_departure.saturating_add(d);
                }
                if arrival_time.is_some() || departure_time.is_some() {
                    first_timed = first_timed.or(Some(position));
                    known[position] = (true, true);
                } else if delay.is_some() {
                    known[position] = (true, true);
                }
            }
        }
        arrivals[position] = arrival;
        departures[position] = departure;
    }

    let repaired = repair(&mut arrivals, &mut departures, first_timed);

    let mut events = Vec::with_capacity(stops);
    for position in 0..stops {
        let arrival = window_time(arrivals[position])?;
        let departure = window_time(departures[position])?;
        events.push(StopEvent {
            arrival,
            departure,
            rules: rules[position],
        });
        let (has_arrival, has_departure) = known[position];
        predictions[position].arrival = has_arrival.then_some(arrival);
        predictions[position].departure = has_departure.then_some(departure);
    }

    Ok(Propagated {
        events,
        predictions,
        repaired,
        unmatched,
        out_of_range,
    })
}

/// Converts seconds to a [`Time`], failing out of range.
pub(crate) fn window_time(seconds: i64) -> Result<Time, ()> {
    seconds_to_i32(seconds)
        .and_then(|seconds| Time::try_new(seconds).ok())
        .ok_or(())
}

/// Makes times move forward along a trip, counting how many times changed.
///
/// Stops before the first timed one have no information of their own,
/// so they give way to it: each departs no later than the next arrives.
/// From there on a time that would go backward is held at the previous time,
/// matching what riders experience as a vehicle running behind its next prediction.
///
/// `first_timed` is the first stop position with real timing, if there is any.
pub(crate) fn repair(
    arrivals: &mut [i64],
    departures: &mut [i64],
    first_timed: Option<usize>,
) -> u32 {
    let Some(first) = first_timed else {
        return 0;
    };
    let mut repaired = 0_u32;
    let mut set = |slot: &mut i64, value: i64| {
        if *slot != value {
            *slot = value;
            repaired += 1;
        }
    };
    // Scheduled stops before the first update.
    for position in (0..first).rev() {
        let next_arrival = arrivals[position + 1];
        if departures[position] > next_arrival {
            set(&mut departures[position], next_arrival);
        }
        let departure = departures[position];
        if arrivals[position] > departure {
            set(&mut arrivals[position], departure);
        }
    }
    for position in first..arrivals.len() {
        if position > 0 {
            let previous = departures[position - 1];
            if arrivals[position] < previous {
                set(&mut arrivals[position], previous);
            }
        }
        let arrival = arrivals[position];
        if departures[position] < arrival {
            set(&mut departures[position], arrival);
        }
    }
    repaired
}

#[cfg(test)]
mod tests {
    use valinor_transit::DaySlot;

    use super::*;
    use crate::date::CalendarDate;
    use crate::lookup::WindowDay;

    const DAY_START: i64 = 1_000_000;
    const TEST_MAX_OFFSET: i64 = 12 * 3600;

    fn clock() -> DayClock {
        DayClock::new(
            WindowDay {
                date: CalendarDate::parse("20260101").expect("valid"),
                start_unix: DAY_START,
            },
            DaySlot::new(0),
            Time::try_new(0).expect("in range"),
        )
    }

    fn t(seconds: i32) -> Time {
        Time::try_new(seconds).expect("in range")
    }

    fn schedule(times: &[(i32, i32)]) -> Vec<StopEvent> {
        times
            .iter()
            .map(|&(arrival, departure)| StopEvent {
                arrival: t(arrival),
                departure: t(departure),
                rules: StopRules::REGULAR,
            })
            .collect()
    }

    fn stu(sequence: u32) -> StopTimeUpdate {
        StopTimeUpdate {
            stop_sequence: Some(sequence),
            ..Default::default()
        }
    }

    fn delayed(sequence: u32, delay: i32) -> StopTimeUpdate {
        StopTimeUpdate {
            arrival: Some(StopTimeEvent {
                delay: Some(delay),
                ..Default::default()
            }),
            departure: Some(StopTimeEvent {
                delay: Some(delay),
                ..Default::default()
            }),
            ..stu(sequence)
        }
    }

    fn with_relationship(mut update: StopTimeUpdate, r: StopRelationship) -> StopTimeUpdate {
        update.set_schedule_relationship(r);
        update
    }

    fn run(
        events: &[StopEvent],
        updates: &[StopTimeUpdate],
        trip_delay: Option<i32>,
    ) -> Propagated {
        let sequences: Vec<Option<u32>> = (1..=events.len())
            .map(|i| Some(u32::try_from(i).expect("few stops")))
            .collect();
        let stops: Vec<StopId> = (0..events.len())
            .map(|i| StopId::new(u32::try_from(i).expect("few stops")))
            .collect();
        propagate(
            &Trip {
                events,
                sequences: &sequences,
                stops: &stops,
                max_offset: TEST_MAX_OFFSET,
            },
            updates,
            trip_delay,
            clock(),
            &|_| None,
        )
        .expect("in range")
    }

    fn arrivals(p: &Propagated) -> Vec<i32> {
        p.events.iter().map(|e| e.arrival.into_inner()).collect()
    }

    #[test]
    fn a_delay_propagates_until_the_next_update() {
        let s = schedule(&[(0, 0), (100, 100), (200, 200), (300, 300)]);
        let p = run(&s, &[delayed(2, 30), delayed(4, 60)], None);
        assert_eq!(arrivals(&p), vec![0, 130, 230, 360]);
        assert_eq!(p.predictions[0].arrival, None, "before any update");
        assert_eq!(p.predictions[2].arrival, Some(t(230)), "propagated");
    }

    #[test]
    fn delay_carries_across_a_skipped_stop() {
        let s = schedule(&[(0, 0), (100, 100), (200, 200)]);
        let p = run(
            &s,
            &[
                delayed(1, 20),
                with_relationship(stu(2), StopRelationship::Skipped),
            ],
            None,
        );
        assert_eq!(arrivals(&p), vec![20, 120, 220]);
        assert_eq!(p.events[1].rules, StopRules::SKIPPED);
        assert_eq!(p.predictions[1].arrival, None);
        assert_eq!(p.predictions[2].arrival, Some(t(220)));
    }

    #[test]
    fn no_data_clears_the_delay() {
        let s = schedule(&[(0, 0), (100, 100), (200, 200), (300, 300)]);
        let p = run(
            &s,
            &[
                delayed(1, 20),
                with_relationship(stu(2), StopRelationship::NoData),
                delayed(4, 5),
            ],
            None,
        );
        assert_eq!(arrivals(&p), vec![20, 100, 200, 305]);
        assert_eq!(p.predictions[2].arrival, None, "unspecified after NO_DATA");
    }

    #[test]
    fn absolute_time_wins_over_delay() {
        let s = schedule(&[(0, 0), (100, 100)]);
        let update = StopTimeUpdate {
            arrival: Some(StopTimeEvent {
                delay: Some(5),
                time: Some(DAY_START + 150),
                ..Default::default()
            }),
            ..stu(2)
        };
        assert_eq!(arrivals(&run(&s, &[update], None)), vec![0, 150]);
    }

    #[test]
    fn trip_delay_applies_until_the_first_stop_with_a_delay() {
        let s = schedule(&[(0, 0), (100, 100)]);
        assert_eq!(arrivals(&run(&s, &[], Some(50))), vec![50, 150]);
        assert_eq!(
            arrivals(&run(&s, &[delayed(2, 5)], Some(50))),
            vec![50, 105]
        );
    }

    #[test]
    fn a_trip_delay_past_the_limit_is_ignored() {
        let s = schedule(&[(0, 0), (100, 100)]);
        let p = run(&s, &[], Some(i32::MAX));
        assert_eq!(p.out_of_range, 1);
        assert_eq!(arrivals(&p), vec![0, 100]);
    }

    #[test]
    fn a_prediction_past_the_limit_is_ignored() {
        let s = schedule(&[(0, 0), (100, 100)]);
        let far = StopTimeUpdate {
            arrival: Some(StopTimeEvent {
                time: Some(DAY_START + 100 + TEST_MAX_OFFSET + 1),
                ..Default::default()
            }),
            ..stu(2)
        };
        let p = run(&s, &[far], None);
        assert_eq!(p.out_of_range, 1);
        assert_eq!(arrivals(&p), vec![0, 100]);
    }

    #[test]
    fn a_prediction_at_the_limit_is_used() {
        let s = schedule(&[(0, 0), (100, 100)]);
        let far = StopTimeUpdate {
            arrival: Some(StopTimeEvent {
                time: Some(DAY_START + 100 + TEST_MAX_OFFSET),
                ..Default::default()
            }),
            ..stu(2)
        };
        let p = run(&s, &[far], None);
        assert_eq!(p.out_of_range, 0);
        assert_eq!(
            arrivals(&p)[1],
            100 + i32::try_from(TEST_MAX_OFFSET).expect("small")
        );
    }

    #[test]
    fn stops_before_the_first_update_give_way_to_an_early_prediction() {
        let s = schedule(&[(0, 0), (100, 100), (200, 200)]);
        let p = run(&s, &[delayed(3, -150)], None);
        assert_eq!(arrivals(&p), vec![0, 50, 50]);
        assert!(p.repaired > 0);
        assert_eq!(p.predictions[0].arrival, None);
    }

    #[test]
    fn a_later_prediction_cannot_go_backward() {
        let s = schedule(&[(0, 0), (100, 100), (200, 200)]);
        let p = run(&s, &[delayed(1, 300), delayed(3, -50)], None);
        // Stop 2 follows the propagated 300 s delay; stop 3's prediction would go backward.
        assert_eq!(arrivals(&p), vec![300, 400, 400]);
        assert!(p.repaired > 0);
    }

    fn loop_trip_run(updates: &[StopTimeUpdate]) -> Propagated {
        let s = schedule(&[(0, 0), (100, 100), (200, 200)]);
        let stops = [StopId::new(7), StopId::new(8), StopId::new(7)];
        let sequences = [Some(1), Some(2), Some(3)];
        let trip = Trip {
            events: &s,
            sequences: &sequences,
            stops: &stops,
            max_offset: TEST_MAX_OFFSET,
        };
        let by_id = |id: &str| (id == "A").then_some(StopId::new(7));
        propagate(&trip, updates, None, clock(), &by_id).expect("in range")
    }

    fn at_a(time: Option<i64>, delay: Option<i32>) -> StopTimeUpdate {
        StopTimeUpdate {
            stop_id: Some("A".into()),
            arrival: Some(StopTimeEvent {
                delay,
                time,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_stop_id_update_on_a_loop_finds_the_nearest_visit() {
        let first = loop_trip_run(&[at_a(Some(DAY_START + 10), None)]);
        assert_eq!(arrivals(&first), vec![10, 110, 210]);
        let last = loop_trip_run(&[at_a(Some(DAY_START + 210), None)]);
        assert_eq!(arrivals(&last), vec![0, 100, 210]);
    }

    #[test]
    fn a_delay_alone_cannot_pick_a_visit_on_a_loop() {
        let p = loop_trip_run(&[at_a(None, Some(10))]);
        assert_eq!(p.unmatched, 1);
        assert_eq!(arrivals(&p), vec![0, 100, 200]);
    }

    #[test]
    fn a_stop_visited_once_needs_no_time_to_match() {
        let s = schedule(&[(0, 0), (100, 100)]);
        let stops = [StopId::new(7), StopId::new(8)];
        let sequences = [Some(1), Some(2)];
        let trip = Trip {
            events: &s,
            sequences: &sequences,
            stops: &stops,
            max_offset: TEST_MAX_OFFSET,
        };
        let by_id = |id: &str| (id == "A").then_some(StopId::new(7));
        let p = propagate(&trip, &[at_a(None, Some(10))], None, clock(), &by_id).expect("in range");
        assert_eq!(arrivals(&p), vec![10, 110]);
    }

    #[test]
    fn unmatched_updates_are_counted_not_fatal() {
        let s = schedule(&[(0, 0), (100, 100)]);
        let p = run(&s, &[delayed(99, 10)], None);
        assert_eq!(p.unmatched, 1);
        assert_eq!(arrivals(&p), vec![0, 100]);
    }

    #[test]
    fn nonsense_times_are_ignored() {
        let s = schedule(&[(0, 0), (100, 100)]);
        let bad = StopTimeUpdate {
            arrival: Some(StopTimeEvent {
                time: Some(i64::MAX),
                delay: Some(1),
                ..Default::default()
            }),
            ..stu(2)
        };
        let p = run(&s, &[bad], None);
        assert_eq!(p.out_of_range, 1);
        assert_eq!(arrivals(&p), vec![0, 100]);
    }
}
