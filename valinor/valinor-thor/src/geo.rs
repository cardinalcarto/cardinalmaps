//! The parts of mainline's `midgard` geometry that path search uses.
//!
//! The A* heuristic orders the search, so its distances must round as mainline's do:
//! [`DistanceApproximator`] works in double precision with a longitude scale from `cosf`,
//! and the heuristic takes a single precision square root of its result.
//! [`PointLL::distance`] (a haversine) picks the algorithm and the alternates' sharing limit.
//!
//! These follow `valinor-loki`'s ports of the same functions, step by step.
//!
//! # Platform dependence
//!
//! Mainline calls the C library's `sin`, `cos`, `asin`, `sqrt`, and `cosf`,
//! and so do Rust's `f64` and `f32` methods on Linux,
//! so results match bit for bit on Linux with glibc.

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares points exactly"
)]

use core::f64::consts::PI;

/// Valhalla's meters per degree of latitude (`kMetersPerDegreeLat`).
const METERS_PER_DEGREE_LAT: f64 = 110_567.0;

/// The earth radius in double precision (`kRadEarthMetersD`).
const RAD_EARTH_METERS_D: f64 = 6_378_160.187;

/// Degrees to radians in double precision (`kRadPerDegD`).
const RAD_PER_DEG_D: f64 = PI / 180.0;

/// Degrees to radians in single precision (`kRadPerDeg`).
const RAD_PER_DEG_F: f32 = core::f32::consts::PI / 180.0;

/// A longitude and latitude in degrees (mainline's `PointLL`, a pair of doubles).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PointLL {
    pub(crate) lng: f64,
    pub(crate) lat: f64,
}

impl PointLL {
    #[inline]
    pub(crate) const fn new(lng: f64, lat: f64) -> Self {
        Self { lng, lat }
    }

    /// The great circle distance in meters (`GeoPoint::Distance`, a haversine).
    pub(crate) fn distance(self, other: Self) -> f64 {
        if self == other {
            return 0.0;
        }
        let phi1 = self.lat * RAD_PER_DEG_D;
        let phi2 = other.lat * RAD_PER_DEG_D;
        let dphi = phi2 - phi1;
        let dlambda = other.lng * RAD_PER_DEG_D - self.lng * RAD_PER_DEG_D;
        let c1 = phi1.cos();
        let c2 = phi2.cos();
        let sdphi2 = (dphi / 2.0).sin();
        let sdlmb2 = (dlambda / 2.0).sin();
        let h = sdphi2 * sdphi2 + c1 * c2 * sdlmb2 * sdlmb2;
        // `std::min(1, std::max(0, h))`, which keeps a NaN `h` as NaN.
        let h = if h < 0.0 { 0.0 } else { h };
        let h = if 1.0 < h { 1.0 } else { h };
        RAD_EARTH_METERS_D * (2.0 * h.sqrt().asin())
    }
}

/// The scale of a degree of longitude relative to one of latitude
/// (`DistanceApproximator::LngScalePerLat`): `cosf(lat * kRadPerDeg)`,
/// with the double product rounded to a float for `cosf`.
#[inline]
fn lng_scale_per_lat(lat: f64) -> f64 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mainline passes the double product to cosf, which takes a float"
    )]
    let angle = (lat * f64::from(RAD_PER_DEG_F)) as f32;
    f64::from(angle.cos())
}

/// Fast squared distances from a fixed point (mainline's `DistanceApproximator<PointLL>`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct DistanceApproximator {
    center: PointLL,
    m_per_lng_degree: f64,
}

impl DistanceApproximator {
    #[inline]
    pub(crate) fn new(center: PointLL) -> Self {
        Self {
            center,
            m_per_lng_degree: lng_scale_per_lat(center.lat) * METERS_PER_DEGREE_LAT,
        }
    }

    /// The approximate squared distance in meters (`DistanceSquared`).
    #[inline]
    pub(crate) fn distance_squared(&self, ll: PointLL) -> f64 {
        let dlat = (ll.lat - self.center.lat) * METERS_PER_DEGREE_LAT;
        let dlng = (ll.lng - self.center.lng) * self.m_per_lng_degree;
        dlat * dlat + dlng * dlng
    }
}

impl Default for DistanceApproximator {
    fn default() -> Self {
        Self::new(PointLL::default())
    }
}

/// Mainline's A* heuristic (`AStarHeuristic`): the approximate distance to a point,
/// times a cost factor (seconds per meter at the costing's top speed).
#[derive(Clone, Copy, Debug)]
pub(crate) struct AStarHeuristic {
    approx: DistanceApproximator,
    cost_factor: f32,
}

impl Default for AStarHeuristic {
    fn default() -> Self {
        Self {
            approx: DistanceApproximator::default(),
            cost_factor: 1.0,
        }
    }
}

impl AStarHeuristic {
    /// Aims the heuristic at `ll` (`Init`).
    #[inline]
    pub(crate) fn init(&mut self, ll: PointLL, cost_factor: f32) {
        self.approx = DistanceApproximator::new(ll);
        self.cost_factor = cost_factor;
    }

    /// The distance to `ll`, as a float (`GetDistance`: `sqrtf` of the double).
    #[inline]
    pub(crate) fn distance(&self, ll: PointLL) -> f32 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline passes the double to sqrtf"
        )]
        let squared = self.approx.distance_squared(ll) as f32;
        squared.sqrt()
    }

    /// The heuristic cost for a distance (`Get(float)`).
    #[inline]
    pub(crate) fn cost(&self, distance: f32) -> f32 {
        distance * self.cost_factor
    }

    /// The heuristic cost to `ll`, and the distance (`Get(ll, dist)`).
    #[inline]
    pub(crate) fn cost_to(&self, ll: PointLL) -> (f32, f32) {
        let distance = self.distance(ll);
        (self.cost(distance), distance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_and_approximation() {
        let a = PointLL::new(1.5211, 42.5078);
        let b = PointLL::new(1.5331, 42.5562);
        let d = a.distance(b);
        assert!((d - 5481.0).abs() < 10.0, "{d}");
        assert_eq!(a.distance(a), 0.0);
        let approx = DistanceApproximator::new(a);
        let ratio = approx.distance_squared(b).sqrt() / d;
        assert!((ratio - 1.0).abs() < 0.01, "{ratio}");
        let mut h = AStarHeuristic::default();
        h.init(a, 0.5);
        let (cost, dist) = h.cost_to(b);
        assert_eq!(cost, dist * 0.5);
        assert_eq!(h.cost(0.0), 0.0);
    }
}
