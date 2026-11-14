//! Mainline's planar and spherical geometry (`midgard`), as Loki and Meili use it.
//!
//! Their results depend on the exact floating point values these produce:
//! a candidate wins by being closer, a node snap is decided by an exact comparison
//! of a projected point with a shape point, and headings are compared with tolerances.
//! So each function here follows mainline's arithmetic step by step,
//! including where it rounds to single precision
//! (mainline's constants are often `float`s, and its longitude scale comes from `cosf`).
//! The comments name the mainline function each one ports.
//!
//! # Platform dependence
//!
//! Mainline calls the C library's `sin`, `cos`, `asin`, `atan2`, `sqrt`, and `cosf`,
//! and so do Rust's `f64` and `f32` methods on Linux.
//! Results therefore match bit for bit on the platform the references were made on
//! (Linux with glibc); elsewhere they may differ in the last bits.

#![allow(
    clippy::float_cmp,
    clippy::many_single_char_names,
    reason = "a port of mainline's arithmetic, which compares floats exactly and names its terms tersely"
)]

use core::f64::consts::PI;
use geo::Coord;

/// Valhalla's (rounded) meters per degree of latitude (`kMetersPerDegreeLat`, a float constant
/// stored as a double).
pub const METERS_PER_DEGREE_LAT: f64 = 110_567.0;

/// The earth radius in double precision (`kRadEarthMetersD`).
const RAD_EARTH_METERS_D: f64 = 6_378_160.187;

/// Degrees to radians in double precision (`kRadPerDegD`).
const RAD_PER_DEG_D: f64 = PI / 180.0;

/// Radians to degrees in double precision (`kDegPerRadD`).
const DEG_PER_RAD_D: f64 = 180.0 / PI;

/// Degrees to radians in single precision (`kRadPerDeg`, `3.14159265f / 180.0f`).
///
/// `3.14159265f` rounds to the same `f32` as [`core::f32::consts::PI`].
pub const RAD_PER_DEG_F: f32 = core::f32::consts::PI / 180.0;

/// A longitude and latitude in degrees (mainline's `PointLL`, a pair of doubles).
///
/// Equality is exact, as for mainline's `std::pair` comparison.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointLL {
    pub lng: f64,
    pub lat: f64,
}

impl PointLL {
    #[inline]
    pub const fn new(lng: f64, lat: f64) -> Self {
        Self { lng, lat }
    }

    /// The great circle distance in meters (`GeoPoint::Distance`, a haversine).
    #[inline]
    pub fn distance(self, other: Self) -> f64 {
        if self == other {
            return 0.0;
        }
        let phi1 = self.lat * RAD_PER_DEG_D;
        let phi2 = other.lat * RAD_PER_DEG_D;
        let dphi = phi2 - phi1;
        let lambda1 = self.lng * RAD_PER_DEG_D;
        let lambda2 = other.lng * RAD_PER_DEG_D;
        let dlambda = lambda2 - lambda1;
        let c1 = phi1.cos();
        let c2 = phi2.cos();
        let sdphi2 = (dphi / 2.0).sin();
        let sdlmb2 = (dlambda / 2.0).sin();
        let h = sdphi2 * sdphi2 + c1 * c2 * sdlmb2 * sdlmb2;
        // `std::min(1, std::max(0, h))`, which keeps a NaN `h` as NaN.
        let h = cpp_min(1.0, cpp_max(0.0, h));
        let d = 2.0 * h.sqrt().asin();
        RAD_EARTH_METERS_D * d
    }

    /// The heading from this point to `other`, in degrees in `[0, 360)` (`GeoPoint::Heading`).
    #[inline]
    pub fn heading(self, other: Self) -> f64 {
        if self == other {
            return 0.0;
        }
        let lat1 = self.lat * RAD_PER_DEG_D;
        let lat2 = other.lat * RAD_PER_DEG_D;
        let dlng = (other.lng - self.lng) * RAD_PER_DEG_D;
        let y = dlng.sin() * lat2.cos();
        let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlng.cos();
        let bearing = y.atan2(x) * DEG_PER_RAD_D;
        if bearing < 0.0 {
            bearing + 360.0
        } else {
            bearing
        }
    }

    /// The point a fraction of the way along the great circle to `p`
    /// (`GeoPoint::PointAlongSegment`).
    #[inline]
    #[must_use]
    pub fn point_along_segment(self, p: Self, distance: f64) -> Self {
        if distance == 0.0 {
            return self;
        }
        if distance == 1.0 {
            return p;
        }
        let lon1 = self.lng * -RAD_PER_DEG_D;
        let lat1 = self.lat * RAD_PER_DEG_D;
        let lon2 = p.lng * -RAD_PER_DEG_D;
        let lat2 = p.lat * RAD_PER_DEG_D;
        let sl1 = lat1.sin();
        let sl2 = lat2.sin();
        let cl1 = lat1.cos();
        let cl2 = lat2.cos();
        let d = (sl1 * sl2 + cl1 * cl2 * (lon1 - lon2).cos()).acos();
        let sd = d.sin();
        let a = (d * (1.0 - distance)).sin() / sd;
        let b = (d * distance).sin() / sd;
        let acs1 = a * cl1;
        let bcs2 = b * cl2;
        let x = acs1 * lon1.cos() + bcs2 * lon2.cos();
        let y = acs1 * lon1.sin() + bcs2 * lon2.sin();
        let z = a * sl1 + b * sl2;
        Self::new(
            y.atan2(x) * -DEG_PER_RAD_D,
            z.atan2((x * x + y * y).sqrt()) * DEG_PER_RAD_D,
        )
    }

    /// The approximate squared distance in meters (`GeoPoint::DistanceSquared`).
    #[inline]
    pub fn distance_squared(self, other: Self) -> f64 {
        DistanceApproximator::new(self).distance_squared(other)
    }

    /// Which side of the line from `p1` to `p2` this point is on (`GeoPoint::IsLeft`):
    /// positive to the left, negative to the right, and zero on the line.
    #[inline]
    pub fn is_left(self, p1: Self, p2: Self) -> f64 {
        (p2.lng - p1.lng) * (self.lat - p1.lat) - (self.lng - p1.lng) * (p2.lat - p1.lat)
    }

    /// The closest point on a polyline, its distance in meters, and the index of its segment
    /// (`GeoPoint::ClosestPoint` with its default arguments: the whole polyline).
    ///
    /// Returns `None` for an empty polyline (mainline returns an invalid point and index -1).
    pub fn closest_point(self, pts: &[Self]) -> Option<(Self, f64, usize)> {
        let first = *pts.first()?;
        let approx = DistanceApproximator::new(self);
        let mut closest = first;
        let mut closest_segment = 0;
        let mut mindistsqr = approx.distance_squared(closest);
        // Mainline goes backwards from the pivot (index 0, so nowhere), then forwards,
        // with no distance cutoff.
        for index in 0..pts.len().saturating_sub(1) {
            let u = pts[index];
            let v = pts[index + 1];
            let bx = v.lng - u.lng;
            let by = v.lat - u.lat;
            let bx2 = bx * approx.lng_scale;
            let sq = bx2 * bx2 + by * by;
            let scale = if sq > 0.0 {
                ((self.lng - u.lng) * approx.lng_scale * bx2 + (self.lat - u.lat) * by) / sq
            } else {
                0.0
            };
            let mut right_most = false;
            let point = if scale <= 0.0 {
                u
            } else if scale >= 1.0 {
                right_most = true;
                v
            } else {
                Self::new(u.lng + bx * scale, u.lat + by * scale)
            };
            let sq_distance = approx.distance_squared(point);
            if sq_distance < mindistsqr {
                closest_segment = index + usize::from(right_most);
                mindistsqr = sq_distance;
                closest = point;
            }
        }
        Some((closest, mindistsqr.sqrt(), closest_segment))
    }
}

impl From<Coord<f64>> for PointLL {
    #[inline]
    fn from(c: Coord<f64>) -> Self {
        Self::new(c.x, c.y)
    }
}

/// `std::min(a, b)`: `a` unless `b` is smaller.
#[inline]
pub fn cpp_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `std::max(a, b)`: `a` unless it is smaller than `b`.
#[inline]
pub fn cpp_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// The scale of a degree of longitude relative to one of latitude
/// (`DistanceApproximator::LngScalePerLat`): `cosf(lat * kRadPerDeg)`.
///
/// The product is a double (the float constant is promoted), rounded to a float for `cosf`.
#[inline]
pub fn lng_scale_per_lat(lat: f64) -> f64 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mainline passes the double product to cosf, which takes a float"
    )]
    let angle = (lat * f64::from(RAD_PER_DEG_F)) as f32;
    f64::from(angle.cos())
}

/// Meters per degree of longitude at a latitude (`DistanceApproximator::MetersPerLngDegree`).
#[inline]
pub fn meters_per_lng_degree(lat: f64) -> f64 {
    lng_scale_per_lat(lat) * METERS_PER_DEGREE_LAT
}

/// Fast squared distances from a fixed point (mainline's `DistanceApproximator<PointLL>`).
#[derive(Clone, Copy, Debug)]
pub struct DistanceApproximator {
    center: PointLL,
    lng_scale: f64,
    m_per_lng_degree: f64,
}

impl DistanceApproximator {
    #[inline]
    pub fn new(center: PointLL) -> Self {
        let lng_scale = lng_scale_per_lat(center.lat);
        Self {
            center,
            lng_scale,
            m_per_lng_degree: lng_scale * METERS_PER_DEGREE_LAT,
        }
    }

    /// The longitude scale at the center (`GetLngScale`).
    #[inline]
    pub const fn lng_scale(&self) -> f64 {
        self.lng_scale
    }

    /// The approximate squared distance in meters (`DistanceSquared`).
    #[inline]
    pub fn distance_squared(&self, ll: PointLL) -> f64 {
        let dlat = (ll.lat - self.center.lat) * METERS_PER_DEGREE_LAT;
        let dlng = (ll.lng - self.center.lng) * self.m_per_lng_degree;
        dlat * dlat + dlng * dlng
    }
}

/// Projects one point onto many segments (mainline's `projector_t`).
#[derive(Clone, Copy, Debug)]
pub struct Projector {
    lon_scale: f64,
    lat: f64,
    lng: f64,
    pub approx: DistanceApproximator,
}

impl Projector {
    #[inline]
    pub fn new(ll: PointLL) -> Self {
        let approx = DistanceApproximator::new(ll);
        Self {
            lon_scale: approx.lng_scale(),
            lat: ll.lat,
            lng: ll.lng,
            approx,
        }
    }

    /// The closest point to ours on the segment from `u` to `v` (`projector_t::operator()`).
    #[inline]
    pub fn project(&self, u: PointLL, v: PointLL) -> PointLL {
        if u == v {
            return u;
        }
        let bx = v.lng - u.lng;
        let by = v.lat - u.lat;
        let bx2 = bx * self.lon_scale;
        let sq = bx2 * bx2 + by * by;
        let mut scale = (self.lng - u.lng) * self.lon_scale * bx2 + (self.lat - u.lat) * by;
        if scale <= 0.0 {
            return u;
        } else if scale >= sq {
            return v;
        }
        scale /= sq;
        PointLL::new(u.lng + bx * scale, u.lat + by * scale)
    }
}

/// Estimates the heading of a shape at a point on it, in degrees from north
/// (mainline's `tangent_angle`, with its default first and last segment indices).
///
/// `index` is the index of the shape point at the start of the segment that `point` is on.
/// Mainline mostly looks at the shape leading into the point (going backwards along the
/// direction of travel) and only uses the shape after it when there isn't enough.
///
/// `shape` must not be empty and `index` must be within it (mainline asserts both);
/// out-of-range input returns 0.
pub fn tangent_angle(
    index: usize,
    point: PointLL,
    shape: &[PointLL],
    sample_distance: f32,
    forward: bool,
) -> f32 {
    if shape.is_empty() || index >= shape.len() {
        return 0.0;
    }
    // first_segment_index = min(0, index) = 0;
    // last_segment_index = min(max(SIZE_MAX, index), size - 1) = size - 1.
    let last = shape.len() - 1;
    // Positions are signed so that the walk can be followed exactly as mainline does it.
    let (increment, first_end, second_end): (isize, isize, isize) = if forward {
        (-1, 0, to_isize(last))
    } else {
        (1, to_isize(last), 0)
    };

    let mut remaining = sample_distance;
    let mut u = point;
    let mut i = to_isize(index) + isize::from(forward);
    while remaining > 0.0 && i != first_end {
        i += increment;
        let Some(&next) = at(shape, i) else {
            break;
        };
        let d = u.distance(next);
        if f64::from(remaining) <= d {
            let coef = f64::from(remaining) / d;
            u = u.point_along_segment(next, coef);
            return narrow(u.heading(point));
        }
        u = next;
        remaining = narrow(f64::from(remaining) - d);
    }

    let mut v = point;
    i = to_isize(index) + isize::from(!forward);
    while remaining > 0.0 && i != second_end {
        i -= increment;
        let Some(&next) = at(shape, i) else {
            break;
        };
        let d = v.distance(next);
        if f64::from(remaining) <= d {
            let coef = f64::from(remaining) / d;
            v = v.point_along_segment(next, coef);
            return narrow(u.heading(v));
        }
        v = next;
        remaining = narrow(f64::from(remaining) - d);
    }
    narrow(u.heading(v))
}

#[inline]
fn to_isize(i: usize) -> isize {
    isize::try_from(i).unwrap_or(isize::MAX)
}

#[inline]
fn at(shape: &[PointLL], i: isize) -> Option<&PointLL> {
    usize::try_from(i).ok().and_then(|i| shape.get(i))
}

/// Rounds a double to a float, as mainline does when it stores a double in a `float`.
#[inline]
pub fn narrow(value: f64) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mainline stores the double in a float"
    )]
    let narrowed = value as f32;
    narrowed
}

/// An axis-aligned bounding box in degrees (mainline's `AABB2<PointLL>`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub minx: f64,
    pub miny: f64,
    pub maxx: f64,
    pub maxy: f64,
}

impl Aabb {
    /// The bounding box of some points (`AABB2::Create`); `None` if there are none
    /// (mainline reads past the end of an empty vector).
    pub fn of_points(points: &[PointLL]) -> Option<Self> {
        let (first, rest) = points.split_first()?;
        let mut bbox = Self {
            minx: first.lng,
            miny: first.lat,
            maxx: first.lng,
            maxy: first.lat,
        };
        for p in rest {
            if p.lng < bbox.minx {
                bbox.minx = p.lng;
            } else if p.lng > bbox.maxx {
                bbox.maxx = p.lng;
            }
            if p.lat < bbox.miny {
                bbox.miny = p.lat;
            } else if p.lat > bbox.maxy {
                bbox.maxy = p.lat;
            }
        }
        Some(bbox)
    }

    /// Whether the boxes overlap, edges included (`AABB2::Intersects(const AABB2&)`).
    #[inline]
    pub fn intersects(&self, r2: &Self) -> bool {
        !((r2.minx < self.minx && r2.maxx < self.minx)
            || (r2.miny < self.miny && r2.maxy < self.miny)
            || (r2.minx > self.maxx && r2.maxx > self.maxx)
            || (r2.miny > self.maxy && r2.maxy > self.maxy))
    }

    /// The overlap of two boxes, or an all-zero box if they don't overlap
    /// (`AABB2::Intersection`).
    #[inline]
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        if !self.intersects(other) {
            return Self {
                minx: 0.0,
                miny: 0.0,
                maxx: 0.0,
                maxy: 0.0,
            };
        }
        Self {
            minx: cpp_max(self.minx, other.minx),
            miny: cpp_max(self.miny, other.miny),
            maxx: cpp_min(self.maxx, other.maxx),
            maxy: cpp_min(self.maxy, other.maxy),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp, reason = "the values are exact")]
    use super::*;

    #[test]
    fn distance_is_zero_for_equal_points_and_symmetric() {
        let a = PointLL::new(1.5211, 42.5078);
        let b = PointLL::new(1.5203, 42.5074);
        assert_eq!(a.distance(a), 0.0);
        assert_eq!(a.distance(b), b.distance(a));
        // About 79 meters.
        assert!((a.distance(b) - 78.9).abs() < 1.0, "{}", a.distance(b));
    }

    #[test]
    fn heading_points_the_right_way() {
        let a = PointLL::new(1.5, 42.5);
        assert_eq!(a.heading(a), 0.0);
        assert!((a.heading(PointLL::new(1.5, 42.6)) - 0.0).abs() < 1e-9);
        assert!((a.heading(PointLL::new(1.6, 42.5)) - 90.0).abs() < 0.1);
        assert!((a.heading(PointLL::new(1.5, 42.4)) - 180.0).abs() < 1e-9);
        assert!((a.heading(PointLL::new(1.4, 42.5)) - 270.0).abs() < 0.1);
    }

    #[test]
    fn projection_clamps_to_the_segment() {
        let projector = Projector::new(PointLL::new(1.0, 1.0));
        let u = PointLL::new(0.0, 0.0);
        let v = PointLL::new(2.0, 0.0);
        let p = projector.project(u, v);
        assert_eq!(p.lat, 0.0);
        assert!((p.lng - 1.0).abs() < 1e-12);
        assert_eq!(projector.project(PointLL::new(3.0, 0.0), v), v);
        assert_eq!(projector.project(u, u), u);
    }

    #[test]
    fn tangent_angle_of_a_straight_line() {
        let shape = [
            PointLL::new(1.5, 42.5),
            PointLL::new(1.5, 42.501),
            PointLL::new(1.5, 42.502),
        ];
        // Northbound, so 0 degrees whichever way we sample.
        let mid = PointLL::new(1.5, 42.5015);
        assert!(tangent_angle(1, mid, &shape, 30.0, true).abs() < 1e-3);
        // Southbound.
        let angle = tangent_angle(1, mid, &shape, 30.0, false);
        assert!((angle - 180.0).abs() < 1e-3, "{angle}");
        // Out of range input does not panic.
        assert_eq!(tangent_angle(5, mid, &shape, 30.0, true), 0.0);
        assert_eq!(tangent_angle(0, mid, &[], 30.0, true), 0.0);
    }

    #[test]
    fn closest_point_on_a_polyline() {
        let pts = [
            PointLL::new(0.0, 0.0),
            PointLL::new(1.0, 0.0),
            PointLL::new(1.0, 1.0),
        ];
        let (p, _, index) = PointLL::new(1.2, 0.5).closest_point(&pts).unwrap();
        assert_eq!(index, 1);
        assert!((p.lng - 1.0).abs() < 1e-12 && (p.lat - 0.5).abs() < 1e-12);
        assert!(PointLL::new(0.0, 0.0).closest_point(&[]).is_none());
    }

    #[test]
    fn boxes() {
        let bbox = Aabb::of_points(&[PointLL::new(0.0, 1.0), PointLL::new(2.0, -1.0)]).unwrap();
        assert_eq!(
            (bbox.minx, bbox.miny, bbox.maxx, bbox.maxy),
            (0.0, -1.0, 2.0, 1.0)
        );
        let other = Aabb {
            minx: 1.0,
            miny: 0.0,
            maxx: 3.0,
            maxy: 3.0,
        };
        assert!(bbox.intersects(&other));
        let overlap = bbox.intersection(&other);
        assert_eq!((overlap.minx, overlap.maxy), (1.0, 1.0));
        assert!(Aabb::of_points(&[]).is_none());
    }
}
