//! The geometry trip building does on a leg's shape (the parts of mainline's `midgard` it uses):
//! trimming an edge's shape where a location cuts it, headings along the shape,
//! the bounding box, and the encoded polyline.
//!
//! Each function follows mainline's arithmetic step by step, in the same precision
//! (mainline mixes floats and doubles here, and the encoded shape rounds every point),
//! so that shapes, headings, and shape attributes come out the same.
//!
//! # Platform dependence
//!
//! As in [`crate::geo`], mainline calls the C library's trigonometry,
//! and so do Rust's `f64` methods on Linux, so results match bit for bit on Linux with glibc.

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares points and fractions exactly"
)]

use crate::geo::PointLL;

/// Mainline's invalid coordinate (`INVALID_LL`, `(double)0xBADBADBAD`),
/// which a default `PointLL` holds.
pub(crate) const INVALID_LL: f64 = 50_159_344_557.0;

/// Degrees to radians and back, in double precision (`kRadPerDegD` and `kDegPerRadD`).
const RAD_PER_DEG: f64 = core::f64::consts::PI / 180.0;
const DEG_PER_RAD: f64 = 180.0 / core::f64::consts::PI;

/// The polyline precision Thor encodes the shape with (`ENCODE_PRECISION`, six digits).
const ENCODE_PRECISION: f64 = 1e6;

impl PointLL {
    /// Mainline's default (invalid) point.
    pub(crate) const INVALID: Self = Self::new(INVALID_LL, INVALID_LL);

    /// Whether neither coordinate is the invalid marker (`IsValid`).
    #[inline]
    pub(crate) fn is_valid(self) -> bool {
        self.lng != INVALID_LL && self.lat != INVALID_LL
    }

    /// The heading from this point to `other`, in degrees from 0 to 360 (`Heading`).
    pub(crate) fn heading(self, other: Self) -> f64 {
        if self == other {
            return 0.0;
        }
        let lat1 = self.lat * RAD_PER_DEG;
        let lat2 = other.lat * RAD_PER_DEG;
        let dlng = (other.lng - self.lng) * RAD_PER_DEG;
        let y = dlng.sin() * lat2.cos();
        let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlng.cos();
        let bearing = y.atan2(x) * DEG_PER_RAD;
        if bearing < 0.0 {
            bearing + 360.0
        } else {
            bearing
        }
    }
}

/// The heading at the start of `pts[idx0..=idx1]`, towards a point `dist` meters along it
/// (`PointLL::HeadingAlongPolyline`).
pub(crate) fn heading_along_polyline(pts: &[PointLL], dist: f64, idx0: usize, idx1: usize) -> f64 {
    let (Some(&first), Some(&last)) = (pts.get(idx0), pts.get(idx1)) else {
        return 0.0;
    };
    if idx1 <= idx0 {
        // Mainline logs "has < 2 vertices".
        return 0.0;
    }
    if idx1 - idx0 > 1 {
        let mut d = 0.0;
        let mut i = idx0;
        while d < dist && i < idx1 {
            let (Some(&pt0), Some(&pt1)) = (pts.get(i), pts.get(i + 1)) else {
                break;
            };
            let seglength = pt0.distance(pt1);
            if d + seglength > dist {
                let pct = (dist - d) / seglength;
                let ll = PointLL::new(
                    pt0.lng + (pt1.lng - pt0.lng) * pct,
                    pt0.lat + (pt1.lat - pt0.lat) * pct,
                );
                return first.heading(ll);
            }
            d += seglength;
            i += 1;
        }
    }
    first.heading(last)
}

/// The heading at the end of `pts[idx0..=idx1]`, from a point `dist` meters before its end
/// (`PointLL::HeadingAtEndOfPolyline`).
pub(crate) fn heading_at_end_of_polyline(
    pts: &[PointLL],
    dist: f64,
    idx0: usize,
    idx1: usize,
) -> f64 {
    let (Some(&first), Some(&last)) = (pts.get(idx0), pts.get(idx1)) else {
        return 0.0;
    };
    if idx1 <= idx0 {
        return 0.0;
    }
    if idx1 - idx0 > 1 {
        let mut d = 0.0;
        // `pt0` is `pts[i - 1]` and `pt1` is `pts[i]`.
        let mut i = idx1;
        while d < dist && i > idx0 {
            let (Some(&pt0), Some(&pt1)) = (pts.get(i - 1), pts.get(i)) else {
                break;
            };
            let seglength = pt0.distance(pt1);
            if d + seglength > dist {
                let pct = (dist - d) / seglength;
                let ll = PointLL::new(
                    pt1.lng + (pt0.lng - pt1.lng) * pct,
                    pt1.lat + (pt0.lat - pt1.lat) * pct,
                );
                return ll.heading(last);
            }
            // Mainline stops at the start of the whole shape (not of the range).
            if i - 1 == 0 {
                break;
            }
            d += seglength;
            i -= 1;
        }
    }
    first.heading(last)
}

/// Cuts a shape to the part between `start` and `end` meters along it, replacing its ends with
/// the given vertices (mainline's `trim_shape(start, start_vertex, end, end_vertex, shape)`).
///
/// An invalid vertex leaves that end alone. As in mainline, the distances along the shape
/// are summed in single precision, and the cut is at the first vertex past each distance.
pub(crate) fn trim_shape(
    start: f32,
    start_vertex: PointLL,
    end: f32,
    end_vertex: PointLL,
    shape: &mut Vec<PointLL>,
) {
    let mut along: f32 = 0.0;
    if start_vertex.is_valid() && !shape.is_empty() {
        let current = cut_index(shape, &mut along, start);
        // Mainline steps back one (an edge's shape always has two points or more,
        // so it never steps before the start).
        let current = current.saturating_sub(1);
        if let Some(p) = shape.get_mut(current) {
            *p = start_vertex;
        }
        shape.drain(..current);
        along = start;
    }
    if end_vertex.is_valid() && !shape.is_empty() {
        let current = cut_index(shape, &mut along, end);
        if let Some(p) = shape.get_mut(current) {
            *p = end_vertex;
        }
        shape.truncate(current + 1);
    }
}

/// Walks the shape from its start, adding each segment's length to `along`,
/// until `along` passes `distance` or the last point is reached; returns the point reached
/// (the loops of mainline's `trim_shape`).
fn cut_index(shape: &[PointLL], along: &mut f32, distance: f32) -> usize {
    let mut current = 0;
    while *along <= distance
        && let (Some(&a), Some(&b)) = (shape.get(current), shape.get(current + 1))
    {
        *along = add_distance(*along, b.distance(a));
        current += 1;
    }
    current
}

/// `float += double`: the sum in double precision, rounded back to a float.
#[inline]
fn add_distance(along: f32, distance: f64) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "C++ compound assignment of a double to a float"
    )]
    let sum = (f64::from(along) + distance) as f32;
    sum
}

/// The bounding box of a shape: the minimum and maximum longitude and latitude
/// (mainline's `AABB2::Create`, which starts from the first point).
pub(crate) fn bounding_box(shape: &[PointLL]) -> Option<(PointLL, PointLL)> {
    let (&first, rest) = shape.split_first()?;
    let (mut minx, mut maxx, mut miny, mut maxy) = (first.lng, first.lng, first.lat, first.lat);
    for p in rest {
        if p.lng < minx {
            minx = p.lng;
        } else if p.lng > maxx {
            maxx = p.lng;
        }
        if p.lat < miny {
            miny = p.lat;
        } else if p.lat > maxy {
            maxy = p.lat;
        }
    }
    Some((PointLL::new(minx, miny), PointLL::new(maxx, maxy)))
}

/// Encodes a shape as a polyline with six digits of precision (mainline's `encode`).
pub(crate) fn encode_polyline(shape: &[PointLL]) -> String {
    let mut output = String::with_capacity(shape.len() * 8);
    let mut write_varint = |mut number: u32| {
        while number >= 0x20 {
            output.push(char::from(u8_of((0x20 | (number & 0x1f)) + 63)));
            number >>= 5;
        }
        output.push(char::from(u8_of(number + 63)));
    };
    let (mut last_lon, mut last_lat) = (0_i32, 0_i32);
    for p in shape {
        let lon = round_to_i32(p.lng * ENCODE_PRECISION);
        let lat = round_to_i32(p.lat * ENCODE_PRECISION);
        write_varint(zigzag(lat.wrapping_sub(last_lat)));
        write_varint(zigzag(lon.wrapping_sub(last_lon)));
        last_lon = lon;
        last_lat = lat;
    }
    output
}

/// A polyline character: at most `0x3f + 63`, so it is always ASCII.
#[inline]
fn u8_of(value: u32) -> u8 {
    u8::try_from(value).unwrap_or(b'?')
}

/// `static_cast<int32_t>(round(value))` (saturating where C++ would be undefined).
#[inline]
fn round_to_i32(value: f64) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "coordinates times 1e6 fit in an i32"
    )]
    let rounded = value.round() as i32;
    rounded
}

/// Zigzag encoding of a signed number (`zigzag_encode`).
#[inline]
const fn zigzag(value: i32) -> u32 {
    (value << 1).cast_unsigned() ^ (value >> 31).cast_unsigned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Vec<PointLL> {
        vec![
            PointLL::new(1.50, 42.50),
            PointLL::new(1.501, 42.50),
            PointLL::new(1.502, 42.501),
            PointLL::new(1.503, 42.501),
        ]
    }

    #[test]
    fn encodes_like_mainline() {
        // Google's example, at six digits (lat first).
        let shape = [
            PointLL::new(-120.2, 38.5),
            PointLL::new(-120.95, 40.7),
            PointLL::new(-126.453, 43.252),
        ];
        assert_eq!(encode_polyline(&shape), "_izlhA~rlgdF_{geC~ywl@_kwzCn`{nI");
        assert_eq!(encode_polyline(&[]), "");
    }

    #[test]
    fn trims_at_distances() {
        let shape = line();
        let total: f64 = shape.windows(2).map(|w| w[0].distance(w[1])).sum();
        // Not trimming at either end (invalid vertices) leaves the shape alone.
        let mut s = shape.clone();
        trim_shape(0.0, PointLL::INVALID, 0.0, PointLL::INVALID, &mut s);
        assert_eq!(s, shape);

        // A cut inside the second segment replaces the first vertex past it.
        let first = shape[0].distance(shape[1]);
        #[expect(clippy::cast_possible_truncation, reason = "test distances")]
        let start = (first + 1.0) as f32;
        let start_vertex = PointLL::new(1.5011, 42.5001);
        let end_vertex = PointLL::new(1.5025, 42.501);
        #[expect(clippy::cast_possible_truncation, reason = "test distances")]
        let end = (total - 30.0) as f32;
        let mut s = shape.clone();
        trim_shape(start, start_vertex, end, end_vertex, &mut s);
        assert_eq!(s, vec![start_vertex, shape[2], end_vertex]);

        // Cuts at both ends of the shape keep two points.
        let mut s = shape.clone();
        trim_shape(0.0, shape[0], 0.0, shape[0], &mut s);
        assert_eq!(s, vec![shape[0], shape[0]]);
    }

    #[test]
    fn headings_along_polylines() {
        let shape = line();
        let east = shape[0].heading(shape[1]);
        assert!((east - 90.0).abs() < 0.01, "{east}");
        assert_eq!(shape[0].heading(shape[0]), 0.0);
        // A short distance stays on the first segment.
        assert!((heading_along_polyline(&shape, 15.0, 0, 3) - east).abs() < 1e-3);
        // Too short a range (one point).
        assert_eq!(heading_along_polyline(&shape, 15.0, 2, 2), 0.0);
        // Past the end, the heading from the first point to the last.
        let whole = shape[0].heading(shape[3]);
        assert_eq!(heading_along_polyline(&shape, 1e6, 0, 3), whole);
        let end = heading_at_end_of_polyline(&shape, 15.0, 0, 3);
        assert!((end - shape[2].heading(shape[3])).abs() < 1e-3, "{end}");
        assert_eq!(heading_at_end_of_polyline(&shape, 1e6, 0, 3), whole);
    }

    #[test]
    fn validity_and_boxes() {
        let a = PointLL::new(1.5, 42.5);
        assert!(!PointLL::INVALID.is_valid());
        assert!(a.is_valid());
        let (min, max) = bounding_box(&line()).expect("a box");
        assert_eq!(
            (min, max),
            (PointLL::new(1.5, 42.5), PointLL::new(1.503, 42.501))
        );
        assert_eq!(bounding_box(&[]), None);
    }
}
