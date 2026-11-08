//! # Spatial utilities useful for routing
//!
//! None of these panic on non-finite input (NaN or infinity).
//! They propagate it through the arithmetic instead,
//! and NaN distances never compare as within any threshold.

use geo::{Coord, CoordFloat, Destination, Haversine, Point};
use num_traits::FromPrimitive;

const METERS_PER_DEGREE_LAT: f64 = 111_132.954;

/// Converts a small constant to `F`.
///
/// Every float type that `geo` supports represents these constants,
/// so the conversion only fails if a core invariant of `num_traits` is broken.
#[inline]
fn constant<F: CoordFloat + FromPrimitive>(value: f64) -> F {
    F::from_f64(value).expect("float types represent small constants")
}

/// Returns a bounding box centered upon `center` containing a circle with radius `radius` meters.
///
/// The bbox is in (N, E, S, W) order.
/// Non-finite input never panics; it propagates as NaN into the affected coordinates.
#[inline]
pub fn bbox_with_center<F: CoordFloat + FromPrimitive>(
    center: Point<F>,
    radius: F,
) -> (F, F, F, F) {
    // Per https://github.com/georust/geo/pull/1091/,
    // the longitude values should be normalized to [-180, 180].
    // We assert this again in a unit test below.
    let north = Haversine.destination(center, F::zero(), radius).y();
    let east = Haversine.destination(center, constant(90.0), radius).x();
    let south = Haversine.destination(center, constant(180.0), radius).y();
    let west = Haversine.destination(center, constant(270.0), radius).x();

    (north, east, south, west)
}

/// Fast distance approximation.
///
/// This is intended for cases when you need a _fast_ estimate over _short_ distances
/// (up to a few kilometers).
/// It is capable of giving estimates like "is point A within X meters of point B"
/// when you want to save potentially expensive trigonometry, especially at scale.
///
/// # Limitations
///
/// * Accuracy decreases at polar latitudes.
/// * Does NOT account for the antimeridian.
/// * Expected range of overestimation is less than 1m for short distances (up to a few kilometers),
///   but will get worse over larger distances, and closer to the poles.
///   Distances over about 20 km are not a great idea.
/// * A NaN center or point gives a NaN distance, which is never within any threshold.
pub struct DistanceApproximator<F: CoordFloat + FromPrimitive> {
    center: Coord<F>,
    meters_per_lon_degree: F,
    meters_per_lat_degree: F,
}

impl<F: CoordFloat + FromPrimitive> DistanceApproximator<F> {
    /// Create a new approximator centered on the given point.
    #[inline]
    pub fn new(center: Coord<F>) -> Self {
        let lon_scale = center.y.to_radians().cos();
        let meters_per_lat_degree = constant(METERS_PER_DEGREE_LAT);
        Self {
            center,
            meters_per_lon_degree: lon_scale * meters_per_lat_degree,
            meters_per_lat_degree,
        }
    }

    /// Returns an approximation of the **squared** distance in meters to the given point.
    ///
    /// This is helpful for spatial predicates which are allowed to over-estimate,
    /// including some sorts of filters, A-star heuristics, etc.
    /// The returned distance will always be _larger_ than the actual distance.
    ///
    /// Compare against `max_distance * max_distance` (to avoid `sqrt` in your code),
    /// or else use the [`DistanceApproximator::is_probably_within_distance_of`] helper.
    #[inline]
    pub fn distance_squared(&self, other: Coord<F>) -> F {
        let dlat = (other.y - self.center.y) * self.meters_per_lat_degree;
        let dlon = (other.x - self.center.x) * self.meters_per_lon_degree;
        (dlat * dlat) + (dlon * dlon)
    }

    /// Returns whether the other coordinate is *probably* within `meters` of the reference coordinate.
    ///
    /// See the [`DistanceApproximator`] docs for more details on the limitations;
    /// in particular, you really shouldn't be using this for distances over about 20 km.
    /// A NaN threshold is never satisfied.
    #[inline]
    pub fn is_probably_within_distance_of(&self, other: Coord<F>, meters: F) -> bool {
        let sq_dist = self.distance_squared(other);

        // Comparison that cleverly avoids sqrt (multiplication is cheap)
        sq_dist <= (meters * meters)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Distance, coord};
    use proptest::{prop_assert, proptest};

    proptest! {
        #[test]
        fn haversine_oracle_f32(lat in -90.0f32..90.0, lon in -180.0f32..180.0,
            dlat in -0.1f32..0.1, dlon in -0.1f32..0.1) {
            // Construct a test with coordinates fairly close together.
            // 0.001 degrees is about 1.1km at the equator.
            // We expect the real use cases for this to be much smaller.
            let a = coord! {x: lon, y: lat};
            let b = coord! {x: lon + dlon, y: lat + dlat};
            let approximator = DistanceApproximator::new(a);

            let sq_dist = approximator.distance_squared(b);
            let haversine_dist = Haversine.distance(a.into(), b.into());

            prop_assert!(sq_dist >= haversine_dist, "Expected sq dist ({sq_dist}) to be >= haversine_dist ({haversine_dist})");

            let delta = sq_dist.sqrt() - haversine_dist;

            prop_assert!(delta < 1.0, "Expected a delta of less than 1m; was {delta}");
            prop_assert!(approximator.is_probably_within_distance_of(b, haversine_dist), "The approximator should return the same result as a Haversine test over short distances");
        }

        #[test]
        fn haversine_oracle_f64(lat in -90.0..90.0, lon in -180.0f64..180.0,
            dlat in -0.1..0.1, dlon in -0.1..0.1) {
            // Construct a test with coordinates fairly close together.
            // 0.001 degrees is about 1.1km at the equator.
            // We expect the real use cases for this to be much smaller.
            let a = coord! {x: lon, y: lat};
            let b = coord! {x: lon + dlon, y: lat + dlat};
            let approximator = DistanceApproximator::new(a);

            let sq_dist = approximator.distance_squared(b);
            let haversine_dist = Haversine.distance(a.into(), b.into());

            prop_assert!(sq_dist >= haversine_dist, "Expected sq dist to be > haversine_dist");

            let delta = sq_dist.sqrt() - haversine_dist;

            prop_assert!(delta < 1.0, "Expected a delta of less than 1m; was {delta}");
            prop_assert!(approximator.is_probably_within_distance_of(b, haversine_dist), "The approximator should return the same result as a Haversine test over short distances");
        }
    }

    proptest! {
        #[test]
        fn non_finite_input_never_panics(
            lat in proptest::num::f64::ANY,
            lon in proptest::num::f64::ANY,
            other_lat in proptest::num::f64::ANY,
            other_lon in proptest::num::f64::ANY,
            meters in proptest::num::f64::ANY,
        ) {
            let center = coord! {x: lon, y: lat};
            let other = coord! {x: other_lon, y: other_lat};
            let approximator = DistanceApproximator::new(center);
            let sq_dist = approximator.distance_squared(other);
            let within = approximator.is_probably_within_distance_of(other, meters);
            if sq_dist.is_nan() || meters.is_nan() {
                prop_assert!(!within);
            }
            let _ = bbox_with_center(center.into(), meters);
        }
    }

    #[test]
    fn test_nan_is_never_within_distance() {
        let here = coord! {x: 1.5, y: 42.5};
        let approximator = DistanceApproximator::new(here);
        assert!(approximator.is_probably_within_distance_of(here, 1.0));
        assert!(!approximator.is_probably_within_distance_of(here, f64::NAN));
        assert!(!approximator.is_probably_within_distance_of(coord! {x: f64::NAN, y: 42.5}, 1.0));
        // Large thresholds are inaccurate, but allowed.
        assert!(approximator.is_probably_within_distance_of(here, 50_000.0));
        assert!(approximator.is_probably_within_distance_of(here, f64::INFINITY));

        let (north, east, south, west) = bbox_with_center(Point::new(1.5, f64::NAN), 100.0);
        assert!(north.is_nan() && east.is_nan() && south.is_nan() && west.is_nan());
    }
}
