//! Fills in stop times a feed left blank, and checks the result.
//!
//! Only some stops of a trip need times in GTFS; the others are interpolated between
//! their neighbors, by distance along the shape if the feed gives one,
//! by straight-line distance between stops otherwise, and uniformly as a last resort.

use geo::{Distance, Haversine, Point};

use crate::compile::report::DropReason;

/// One stop of a trip before interpolation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RawTime {
    pub(super) arrival: Option<i32>,
    pub(super) departure: Option<i32>,
    pub(super) shape_dist: Option<f32>,
    pub(super) position: Option<(f64, f64)>,
}

/// Fills every stop's arrival and departure.
///
/// Returns the times and how many stops were interpolated, or why the trip can't be used.
pub(super) fn interpolate(rows: &[RawTime]) -> Result<(Vec<(i32, i32)>, usize), DropReason> {
    let (first, last) = match (rows.first(), rows.last()) {
        (Some(first), Some(last)) if rows.len() >= 2 => (first, last),
        _ => return Err(DropReason::TooFewStops),
    };
    if !has_time(first) || !has_time(last) {
        return Err(DropReason::MissingEndpointTimes);
    }

    // A known time on only one side of a stop stands in for the other.
    let mut filled: Vec<Option<(i32, i32)>> = rows
        .iter()
        .map(|row| match (row.arrival, row.departure) {
            (Some(a), Some(d)) => Some((a, d)),
            (Some(a), None) => Some((a, a)),
            (None, Some(d)) => Some((d, d)),
            (None, None) => None,
        })
        .collect();

    let mut interpolated = 0;
    let mut index = 0;
    while index < rows.len() {
        if filled[index].is_some() {
            index += 1;
            continue;
        }
        let before = index - 1;
        let mut after = index + 1;
        while filled[after].is_none() {
            after += 1;
        }
        let (Some((_, start)), Some((end, _))) = (filled[before], filled[after]) else {
            return Err(DropReason::MissingEndpointTimes);
        };
        let weights = gap_weights(rows, before, after);
        for (offset, slot) in filled[before + 1..after].iter_mut().enumerate() {
            let time = round(f64::from(start) + f64::from(end - start) * weights[offset]);
            *slot = Some((time, time));
            interpolated += 1;
        }
        index = after;
    }

    let mut times = Vec::with_capacity(rows.len());
    let mut previous = i32::MIN;
    for slot in filled {
        let (arrival, departure) = slot.ok_or(DropReason::MissingEndpointTimes)?;
        if arrival < previous || departure < arrival {
            return Err(DropReason::TimesOutOfOrder);
        }
        previous = departure;
        times.push((arrival, departure));
    }
    Ok((times, interpolated))
}

fn has_time(row: &RawTime) -> bool {
    row.arrival.is_some() || row.departure.is_some()
}

/// `f64` to `i32`, after the caller has bounded the magnitude by the endpoints of a real trip.
#[expect(
    clippy::cast_possible_truncation,
    reason = "bounded by real GTFS times"
)]
fn round(value: f64) -> i32 {
    value.round() as i32
}

/// The fraction of the way from `before` to `after` of each stop strictly between them.
fn gap_weights(rows: &[RawTime], before: usize, after: usize) -> Vec<f64> {
    let positions = before + 1..after;
    let cumulative = shape_cumulative(rows, before, after)
        .or_else(|| geodesic_cumulative(rows, before, after))
        .unwrap_or_else(|| (before..=after).map(index_as_f64).collect());
    let origin = cumulative[0];
    let span = cumulative[cumulative.len() - 1] - origin;
    positions
        .map(|i| {
            if span > 0.0 {
                (cumulative[i - before] - origin) / span
            } else {
                index_as_f64(i - before) / index_as_f64(after - before)
            }
        })
        .collect()
}

/// Cumulative distance from a feed's own `shape_dist_traveled`, when it's complete and increasing.
fn shape_cumulative(rows: &[RawTime], before: usize, after: usize) -> Option<Vec<f64>> {
    let mut dists = Vec::with_capacity(after - before + 1);
    for row in &rows[before..=after] {
        dists.push(f64::from(row.shape_dist?));
    }
    dists
        .windows(2)
        .all(|pair| pair[0] <= pair[1])
        .then_some(dists)
}

/// Cumulative straight-line distance between stops, when every stop has a position.
fn geodesic_cumulative(rows: &[RawTime], before: usize, after: usize) -> Option<Vec<f64>> {
    let mut total = 0.0;
    let mut out = vec![0.0];
    let mut previous = rows[before].position?;
    for row in &rows[before + 1..=after] {
        let position = row.position?;
        total += Haversine.distance(
            Point::new(previous.1, previous.0),
            Point::new(position.1, position.0),
        );
        out.push(total);
        previous = position;
    }
    Some(out)
}

fn index_as_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn at(time: i32) -> RawTime {
        RawTime {
            arrival: Some(time),
            departure: Some(time),
            shape_dist: None,
            position: None,
        }
    }

    fn blank() -> RawTime {
        RawTime {
            arrival: None,
            departure: None,
            shape_dist: None,
            position: None,
        }
    }

    #[test]
    fn evenly_interpolates_without_distances() {
        let (times, count) = interpolate(&[at(0), blank(), blank(), at(300)]).expect("ok");
        assert_eq!(times, vec![(0, 0), (100, 100), (200, 200), (300, 300)]);
        assert_eq!(count, 2);
    }

    #[test]
    fn interpolates_by_shape_distance_when_given() {
        let mut rows = [at(0), blank(), at(100)];
        rows[0].shape_dist = Some(0.0);
        rows[1].shape_dist = Some(75.0);
        rows[2].shape_dist = Some(100.0);
        let (times, _) = interpolate(&rows).expect("ok");
        assert_eq!(times[1], (75, 75));
    }

    #[test]
    fn interpolates_by_straight_line_distance_when_positions_are_known() {
        let mut rows = [at(0), blank(), at(300)];
        rows[0].position = Some((47.0, -122.0));
        rows[1].position = Some((47.001, -122.0));
        rows[2].position = Some((47.003, -122.0));
        let (times, _) = interpolate(&rows).expect("ok");
        // Stop 2 is a third of the way along the distance.
        assert_eq!(times[1], (100, 100));
    }

    #[test]
    fn missing_endpoints_are_rejected() {
        assert_eq!(
            interpolate(&[blank(), at(10)]),
            Err(DropReason::MissingEndpointTimes),
        );
        assert_eq!(
            interpolate(&[at(10), blank()]),
            Err(DropReason::MissingEndpointTimes),
        );
    }

    #[test]
    fn single_row_is_too_few_stops() {
        assert_eq!(interpolate(&[at(10)]), Err(DropReason::TooFewStops));
    }

    #[test]
    fn backward_times_are_rejected() {
        assert_eq!(
            interpolate(&[at(100), at(50)]),
            Err(DropReason::TimesOutOfOrder),
        );
    }

    #[test]
    fn departure_before_arrival_is_rejected() {
        let row = RawTime {
            arrival: Some(100),
            departure: Some(90),
            shape_dist: None,
            position: None,
        };
        assert_eq!(
            interpolate(&[row, at(200)]),
            Err(DropReason::TimesOutOfOrder)
        );
    }

    #[test]
    fn a_single_sided_time_fills_the_other_side() {
        let row = RawTime {
            arrival: None,
            departure: Some(50),
            shape_dist: None,
            position: None,
        };
        let (times, _) = interpolate(&[at(0), row, at(100)]).expect("ok");
        assert_eq!(times[1], (50, 50));
    }

    #[test]
    fn identical_stop_positions_fall_back_to_even_spacing() {
        let mut rows = [at(0), blank(), at(100)];
        for row in &mut rows {
            row.position = Some((47.0, -122.0));
        }
        let (times, _) = interpolate(&rows).expect("ok");
        assert_eq!(times[1], (50, 50));
    }

    proptest! {
        /// Interpolation never moves time backward and keeps the known times.
        #[test]
        fn interpolation_is_monotone(
            start in 0_i32..100_000,
            gaps in proptest::collection::vec((0_i32..2_000, proptest::bool::ANY), 2..20),
        ) {
            let mut rows = vec![at(start)];
            let mut clock = start;
            for (gap, known) in &gaps {
                clock += gap;
                rows.push(if *known { at(clock) } else { blank() });
            }
            let last = rows.len() - 1;
            rows[last] = at(clock);
            let (times, _) = interpolate(&rows).expect("valid");
            prop_assert_eq!(times[0].0, start);
            prop_assert_eq!(times[last].1, clock);
            for pair in times.windows(2) {
                prop_assert!(pair[0].1 <= pair[1].0);
            }
            for (row, time) in rows.iter().zip(&times) {
                if let Some(arrival) = row.arrival {
                    prop_assert_eq!(arrival, time.0);
                }
            }
        }
    }
}
