//! Places a trip's stops along its shape, for feeds that don't say where they are.
//!
//! The stops of a trip must appear on its shape in order,
//! so we pick the assignment that minimizes the summed squared distance from stop to segment,
//! over all assignments in non-decreasing segment order.
//! Using dynamic programming over stops and segments means loops and out-and-backs,
//! where a stop may be close to two parts of its shape, resolve correctly.

/// A point of a shape: latitude, longitude, and distance along it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ShapePoint {
    pub(super) lat: f64,
    pub(super) lon: f64,
    pub(super) dist: f32,
}

/// The distance along `points` of each stop in `stops`.
///
/// `stops` are latitude and longitude. An empty shape yields zeros.
pub(super) fn project(points: &[ShapePoint], stops: &[(f64, f64)]) -> Vec<f32> {
    let Some(first) = points.first() else {
        return vec![0.0; stops.len()];
    };
    if points.len() == 1 || stops.is_empty() {
        return vec![first.dist; stops.len()];
    }
    // Projecting onto a plane, scaled so a degree of longitude and of latitude are comparable.
    let scale =
        points.iter().map(|p| p.lat.to_radians().cos()).sum::<f64>() / len_f64(points.len());
    let plane: Vec<(f64, f64)> = points.iter().map(|p| (p.lon * scale, p.lat)).collect();
    let segments = plane.len() - 1;

    let mut best = vec![0.0; segments];
    let mut back: Vec<Vec<u32>> = Vec::with_capacity(stops.len());
    for &(lat, lon) in stops {
        let mut row = vec![0_u32; segments];
        let mut running_min = f64::INFINITY;
        let mut argmin = 0_u32;
        let mut next = vec![f64::INFINITY; segments];
        for segment in 0..segments {
            let (distance, _) =
                point_to_segment((lon * scale, lat), plane[segment], plane[segment + 1]);
            if best[segment] < running_min {
                running_min = best[segment];
                argmin = u32::try_from(segment).unwrap_or(u32::MAX);
            }
            if running_min.is_finite() {
                next[segment] = running_min + distance;
                row[segment] = argmin;
            }
        }
        best = next;
        back.push(row);
    }

    let mut segment = best
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(index, _)| index);
    let mut chosen = vec![0_usize; stops.len()];
    for stop_index in (0..stops.len()).rev() {
        chosen[stop_index] = segment;
        segment = back[stop_index][segment] as usize;
    }

    let mut previous = f32::NEG_INFINITY;
    let mut out = Vec::with_capacity(stops.len());
    for (&segment, &(lat, lon)) in chosen.iter().zip(stops) {
        let (a, b) = (&points[segment], &points[segment + 1]);
        let (_, fraction) =
            point_to_segment((lon * scale, lat), plane[segment], plane[segment + 1]);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a fraction of a shape distance"
        )]
        let dist = a.dist + (b.dist - a.dist) * (fraction as f32);
        let dist = dist.max(previous);
        previous = dist;
        out.push(dist);
    }
    out
}

fn len_f64(len: usize) -> f64 {
    f64::from(u32::try_from(len).unwrap_or(u32::MAX))
}

/// Squared distance from a stop to a segment, and how far along the segment it projects.
fn point_to_segment(stop: (f64, f64), a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let fraction = if length > 0.0 {
        (((stop.0 - a.0) * dx + (stop.1 - a.1) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (px, py) = (a.0 + fraction * dx, a.1 + fraction * dy);
    let (ex, ey) = (stop.0 - px, stop.1 - py);
    (ex * ex + ey * ey, fraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Vec<ShapePoint> {
        (0..=10)
            .map(|i| ShapePoint {
                lat: 47.0,
                lon: -122.0 + f64::from(i) * 0.001,
                dist: 100.0 * f32::from(i16::try_from(i).expect("small")),
            })
            .collect()
    }

    #[test]
    fn stops_project_onto_a_line() {
        let dists = project(
            &line(),
            &[(47.0, -122.0), (47.0, -121.995), (47.0, -121.99)],
        );
        assert!(dists[0].abs() < 1.0, "{dists:?}");
        assert!((dists[1] - 500.0).abs() < 1.0, "{dists:?}");
        assert!((dists[2] - 1000.0).abs() < 1.0, "{dists:?}");
    }

    #[test]
    fn an_out_and_back_keeps_stops_in_order() {
        let mut points: Vec<ShapePoint> = (0..=10)
            .map(|i| ShapePoint {
                lat: 47.0,
                lon: -122.0 + f64::from(i) * 0.001,
                dist: f32::from(i16::try_from(i).expect("small")),
            })
            .collect();
        points.extend((0..=10).rev().map(|i| ShapePoint {
            lat: 47.0,
            lon: -122.0 + f64::from(i) * 0.001,
            dist: 20.0 - f32::from(i16::try_from(i).expect("small")),
        }));
        // The same stop is visited on the way out and on the way back.
        let dists = project(
            &points,
            &[(47.0, -121.995), (47.0, -121.99), (47.0, -121.995)],
        );
        assert!(dists[0] <= dists[1] && dists[1] <= dists[2], "{dists:?}");
        assert!(dists[2] > 10.0, "{dists:?}");
    }

    #[test]
    fn a_single_point_shape_gives_its_distance() {
        let point = ShapePoint {
            lat: 47.0,
            lon: -122.0,
            dist: 5.0,
        };
        assert_eq!(
            project(&[point], &[(47.0, -122.0), (47.0, -122.0)]),
            vec![5.0, 5.0]
        );
    }
}
