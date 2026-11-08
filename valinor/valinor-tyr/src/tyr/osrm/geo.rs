//! Mainline's `midgard` geometry, with the same arithmetic (and `float`/`double` types)
//! so that values rounded to a few decimals come out the same.

use crate::tyr::polyline::LngLat;

/// `midgard::kRadPerDeg`, computed in `float` as mainline does.
/// (`kPi`, `3.14159265f`, is the same `float` as `f32::consts::PI`.)
const RAD_PER_DEG: f32 = core::f32::consts::PI / 180.0_f32;
/// `midgard::kMetersPerDegreeLat` (a `double` initialized from a `float` literal).
const METERS_PER_DEGREE_LAT: f64 = 110_567.0;
/// `midgard::kMetersPerKm`.
pub(super) const METERS_PER_KM: f32 = 1000.0;
/// `midgard::kKmPerMile`.
const KM_PER_MILE: f32 = 1.609_344;

/// `midgard::units_to_meters`, in `float`.
pub(super) fn units_to_meters(units_km_or_mi: f32, is_metric: bool) -> f32 {
    METERS_PER_KM
        * if is_metric {
            units_km_or_mi
        } else {
            units_km_or_mi * KM_PER_MILE
        }
}

/// `DistanceApproximator::LngScalePerLat`: `cosf` of a value narrowed to `float`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline passes the double to cosf, narrowing it to float"
)]
fn lng_scale_per_lat(lat: f64) -> f64 {
    f64::from(((lat * f64::from(RAD_PER_DEG)) as f32).cos())
}

/// `PointLL::DistanceSquared` (`DistanceApproximator` centered on `center`).
fn approximate_distance_squared(center: LngLat, point: LngLat) -> f64 {
    let m_per_lng_degree = lng_scale_per_lat(center.lat) * METERS_PER_DEGREE_LAT;
    let lat = (point.lat - center.lat) * METERS_PER_DEGREE_LAT;
    let lng = (point.lng - center.lng) * m_per_lng_degree;
    lat * lat + lng * lng
}

#[expect(clippy::many_single_char_names, reason = "mirrors mainline's names")]
/// `LineSegment2<PointLL>::DistanceSquared`: the approximate squared distance in meters
/// from `p` to the segment from `a` to `b`.
fn segment_distance_squared(a: LngLat, b: LngLat, p: LngLat) -> f64 {
    let (vx, vy) = (b.lng - a.lng, b.lat - a.lat);
    let (wx, wy) = (p.lng - a.lng, p.lat - a.lat);
    let n = wx * vx + wy * vy;
    let closest = if n <= 0.0 {
        a
    } else {
        let d = vx * vx + vy * vy;
        if d <= n {
            b
        } else {
            let t = n / d;
            LngLat {
                lng: a.lng + vx * t,
                lat: a.lat + vy * t,
            }
        }
    };
    approximate_distance_squared(closest, p)
}

/// `Polyline2<PointLL>::Generalize` without self-intersection avoidance:
/// Douglas–Peucker with `epsilon` in meters, never removing the points at `exclusions`
/// (indexes into the original polyline).
pub(super) fn generalize(polyline: &mut Vec<LngLat>, epsilon: f64, exclusions: &[usize]) {
    if epsilon <= 0.0 || polyline.len() < 3 {
        return;
    }
    let epsilon = epsilon * epsilon;
    let mut keep = vec![false; polyline.len()];
    let last = polyline.len() - 1;
    keep[0] = true;
    keep[last] = true;
    // Mainline erases runs of points from a vector as it recurses from right to left,
    // which leaves indexes to the left of the erased run intact;
    // marking points to keep gives the same result.
    let mut stack = vec![(0, last)];
    while let Some((s, e)) = stack.pop() {
        let mut dmax = f64::MIN;
        let mut k = 0;
        for j in (s + 1..e).rev() {
            if exclusions.contains(&j) {
                dmax = epsilon;
                k = j;
                break;
            }
            let d = segment_distance_squared(polyline[s], polyline[e], polyline[j]);
            if d > dmax {
                dmax = d;
                k = j;
            }
        }
        if dmax >= epsilon {
            keep[k] = true;
            // The order the halves are processed in doesn't change which points are kept.
            if e - k > 1 {
                stack.push((k, e));
            }
            if k - s > 1 {
                stack.push((s, k));
            }
        }
    }
    let mut index = 0;
    polyline.retain(|_| {
        index += 1;
        keep[index - 1]
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ll(lng: f64, lat: f64) -> LngLat {
        LngLat { lng, lat }
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "the conversions are exact")]
    fn meters_from_units() {
        assert_eq!(units_to_meters(1.5, true), 1500.0);
        assert_eq!(units_to_meters(1.0, false), 1609.344);
    }

    #[test]
    fn douglas_peucker() {
        // A straight line loses its middle points, unless they are excluded.
        let line = vec![ll(0.0, 0.0), ll(0.001, 0.0), ll(0.002, 0.0), ll(0.003, 0.0)];
        let mut simplified = line.clone();
        generalize(&mut simplified, 1.0, &[]);
        assert_eq!(simplified, vec![line[0], line[3]]);
        let mut kept = line.clone();
        generalize(&mut kept, 1.0, &[2]);
        assert_eq!(kept, vec![line[0], line[2], line[3]]);

        // A spike well over the tolerance stays.
        let spike = vec![ll(0.0, 0.0), ll(0.001, 0.001), ll(0.002, 0.0)];
        let mut out = spike.clone();
        generalize(&mut out, 10.0, &[]);
        assert_eq!(out, spike);
        let mut flattened = spike.clone();
        generalize(&mut flattened, 1000.0, &[]);
        assert_eq!(flattened, vec![spike[0], spike[2]]);
    }
}
