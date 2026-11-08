//! `OpenLR` linear references for trip edges, as mainline's `openlr_edges` (`tyr/serializers.cc`)
//! writes them with `baldr::OpenLR::OpenLr::toBase64`.
//!
//! Each edge of a leg becomes a two-point `OpenLR` line reference:
//! the edge's first and last shape points,
//! with bearings sampled 20 meters into the edge (`midgard::tangent_angle`),
//! the edge length, its road class as the functional road class,
//! and a form of way derived from the edge's attributes.
//! The geometry helpers here are ports of `midgard::GeoPoint`'s
//! `Distance`, `Heading`, and `PointAlongSegment`, computed in the same order and precision,
//! so the quantized bearings come out the same as mainline's.

use super::polyline::{self, BadPolyline, LngLat};
use thiserror::Error;
use valhalla_proto::trip_leg::{Edge, Traversability, Use};
use valhalla_proto::{RoadClass, TripLeg};

const RAD_PER_DEG: f64 = core::f64::consts::PI / 180.0;
const DEG_PER_RAD: f64 = 180.0 / core::f64::consts::PI;
/// `midgard::kRadEarthMetersD`.
const RAD_EARTH_METERS: f64 = 6_378_160.187;
/// `midgard::kMetersPerKm`, a float in mainline.
const METERS_PER_KM: f32 = 1000.0;
/// `OpenLR::geom_scale`.
const GEOM_SCALE: f64 = 100_000.0;

/// Why a leg's linear references could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum OpenLrError {
    /// The leg's shape is not a valid polyline.
    #[error(transparent)]
    Shape(#[from] BadPolyline),
    /// An edge's shape indexes point past the end of the leg's shape.
    /// (Mainline reads out of bounds here.)
    #[error("build a linear reference: edge shape index {index} is outside the leg's {len} points")]
    ShapeIndex {
        /// The edge's shape index.
        index: u32,
        /// The number of points in the leg's shape.
        len: usize,
    },
}

/// `midgard::GeoPoint::Distance`: the haversine distance in meters.
pub(crate) fn distance(a: LngLat, b: LngLat) -> f64 {
    if a == b {
        return 0.0;
    }
    let phi1 = a.lat * RAD_PER_DEG;
    let phi2 = b.lat * RAD_PER_DEG;
    let dphi = phi2 - phi1;
    let lambda1 = a.lng * RAD_PER_DEG;
    let lambda2 = b.lng * RAD_PER_DEG;
    let dlambda = lambda2 - lambda1;
    let c1 = phi1.cos();
    let c2 = phi2.cos();
    let sdphi2 = (dphi / 2.0).sin();
    let sdlmb2 = (dlambda / 2.0).sin();
    let h = (sdphi2 * sdphi2 + c1 * c2 * sdlmb2 * sdlmb2).clamp(0.0, 1.0);
    RAD_EARTH_METERS * (2.0 * h.sqrt().asin())
}

/// `midgard::GeoPoint::Heading`: the initial bearing from `a` to `b`, in degrees.
pub(crate) fn heading(a: LngLat, b: LngLat) -> f64 {
    if a == b {
        return 0.0;
    }
    let lat1 = a.lat * RAD_PER_DEG;
    let lat2 = b.lat * RAD_PER_DEG;
    let dlng = (b.lng - a.lng) * RAD_PER_DEG;
    let y = dlng.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlng.cos();
    let bearing = y.atan2(x) * DEG_PER_RAD;
    if bearing < 0.0 {
        bearing + 360.0
    } else {
        bearing
    }
}

/// `midgard::GeoPoint::PointAlongSegment`: the point a fraction of the way along the arc.
#[expect(
    clippy::float_cmp,
    clippy::many_single_char_names,
    reason = "mirrors mainline's exact short-circuits and formula names"
)]
pub(crate) fn point_along_segment(a: LngLat, b: LngLat, fraction: f64) -> LngLat {
    if fraction == 0.0 {
        return a;
    }
    if fraction == 1.0 {
        return b;
    }
    let lon1 = a.lng * -RAD_PER_DEG;
    let lat1 = a.lat * RAD_PER_DEG;
    let lon2 = b.lng * -RAD_PER_DEG;
    let lat2 = b.lat * RAD_PER_DEG;
    let sl1 = lat1.sin();
    let sl2 = lat2.sin();
    let cl1 = lat1.cos();
    let cl2 = lat2.cos();
    let d = (sl1 * sl2 + cl1 * cl2 * (lon1 - lon2).cos()).acos();
    let sd = d.sin();
    let a_ = (d * (1.0 - fraction)).sin() / sd;
    let b_ = (d * fraction).sin() / sd;
    let acs1 = a_ * cl1;
    let bcs2 = b_ * cl2;
    let x = acs1 * lon1.cos() + bcs2 * lon2.cos();
    let y = acs1 * lon1.sin() + bcs2 * lon2.sin();
    let z = a_ * sl1 + b_ * sl2;
    LngLat {
        lng: y.atan2(x) * -DEG_PER_RAD,
        lat: z.atan2((x * x + y * y).sqrt()) * DEG_PER_RAD,
    }
}

/// `midgard::tangent_angle`: the bearing of the shape around `index`,
/// sampled over `sample_distance` meters within `[first, last]`.
///
/// `index` must be within `shape`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline keeps the remaining distance and the result as floats"
)]
fn tangent_angle(
    index: usize,
    point: LngLat,
    shape: &[LngLat],
    sample_distance: f32,
    forward: bool,
    first: usize,
    last: usize,
) -> f32 {
    let first = first.min(index);
    let last = last.max(index).min(shape.len().saturating_sub(1));
    // Signed indexes: the walks below step one past their start before reading.
    let (first, last, index) = (to_isize(first), to_isize(last), to_isize(index));
    let increment: isize = if forward { -1 } else { 1 };
    let (first_end, second_end) = if forward {
        (first, last)
    } else {
        (last, first)
    };
    let at = |i: isize| shape[usize::try_from(i).unwrap_or(0)];

    let mut remaining = sample_distance;
    let mut u = point;
    let mut i = index + isize::from(forward);
    while remaining > 0.0 && i != first_end {
        i += increment;
        let next = at(i);
        let d = u.distance_to(next);
        if f64::from(remaining) <= d {
            u = point_along_segment(u, next, f64::from(remaining) / d);
            return heading(u, point) as f32;
        }
        u = next;
        remaining = (f64::from(remaining) - d) as f32;
    }

    let mut v = point;
    i = index + isize::from(!forward);
    while remaining > 0.0 && i != second_end {
        i -= increment;
        let next = at(i);
        let d = v.distance_to(next);
        if f64::from(remaining) <= d {
            v = point_along_segment(v, next, f64::from(remaining) / d);
            return heading(u, v) as f32;
        }
        v = next;
        remaining = (f64::from(remaining) - d) as f32;
    }
    heading(u, v) as f32
}

fn to_isize(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

impl LngLat {
    fn distance_to(self, other: LngLat) -> f64 {
        distance(self, other)
    }
}

/// `OpenLR`'s form of way, as mainline's `road_class_to_fow` picks it.
fn form_of_way(edge: &Edge) -> u8 {
    const MOTORWAY: u8 = 1;
    const MULTIPLE_CARRIAGEWAY: u8 = 2;
    const SINGLE_CARRIAGEWAY: u8 = 3;
    const ROUNDABOUT: u8 = 4;
    const SLIPROAD: u8 = 6;
    const OTHER: u8 = 7;
    if edge.roundabout {
        ROUNDABOUT
    } else if edge.r#use == Use::KRampUse as i32 || edge.r#use == Use::KTurnChannelUse as i32 {
        SLIPROAD
    } else if edge.road_class == RoadClass::KMotorway as i32 {
        MOTORWAY
    } else if edge.traversability == Traversability::KBoth as i32 {
        MULTIPLE_CARRIAGEWAY
    } else if edge.traversability != Traversability::KNone as i32 {
        SINGLE_CARRIAGEWAY
    } else {
        OTHER
    }
}

fn sgn(value: f64) -> f64 {
    f64::from(i8::from(value > 0.0) - i8::from(value < 0.0))
}

/// `OpenLR`'s `decimal2integer`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline truncates with static_cast<int32_t>"
)]
fn decimal_to_integer(value: f64) -> i32 {
    (sgn(value) * 0.5 + value * f64::from(1_i32 << 24) / 360.0) as i32
}

/// `OpenLR`'s `integer2decimal`.
fn integer_to_decimal(value: i32) -> f64 {
    (f64::from(value) - sgn(f64::from(value)) * 0.5) * 360.0 / f64::from(1_i32 << 24)
}

/// `OpenLR`'s `bearing2integer`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline truncates with static_cast<int32_t>"
)]
fn bearing_to_integer(value: f64) -> i32 {
    (value / 11.25) as i32
}

/// `OpenLR`'s `integer2bearing`.
fn integer_to_bearing(value: i32) -> f64 {
    f64::from(value) * 11.25 + 11.25 / 2.0
}

/// `OpenLR`'s `distance2integer`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline truncates with static_cast<int32_t>"
)]
fn distance_to_integer(value: f64) -> i32 {
    (value / 58.6) as i32
}

/// `OpenLR`'s `integer2distance`, which takes the integer as unsigned.
fn integer_to_distance(value: i32) -> f64 {
    f64::from(value.cast_unsigned()) * 58.6
}

/// `static_cast<int32_t>(std::round(x))`, saturating where C++ is undefined.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the value is rounded; out-of-range values saturate"
)]
fn round_i32(value: f64) -> i32 {
    value.round() as i32
}

/// The low byte of an integer, as `std::string::push_back(char)` keeps it.
fn low_byte(value: i32) -> u8 {
    value.to_le_bytes()[0]
}

/// A location reference point, as `OpenLR::LocationReferencePoint` stores it after quantizing.
struct ReferencePoint {
    longitude: f64,
    latitude: f64,
    bearing: f64,
    distance: f64,
    frc: u8,
    lfrcnp: u8,
    fow: u8,
}

impl ReferencePoint {
    /// The quantizing constructor, relative to the previous point if there is one.
    fn new(
        point: LngLat,
        bearing: f64,
        frc: u8,
        fow: u8,
        prev: Option<&ReferencePoint>,
        distance: f64,
        lfrcnp: u8,
    ) -> Self {
        let (longitude, latitude) = match prev {
            None => (
                integer_to_decimal(decimal_to_integer(point.lng)),
                integer_to_decimal(decimal_to_integer(point.lat)),
            ),
            Some(prev) => (
                prev.longitude + ((point.lng - prev.longitude) * GEOM_SCALE).round() / GEOM_SCALE,
                prev.latitude + ((point.lat - prev.latitude) * GEOM_SCALE).round() / GEOM_SCALE,
            ),
        };
        Self {
            longitude,
            latitude,
            bearing: integer_to_bearing(bearing_to_integer(bearing)),
            distance: integer_to_distance(distance_to_integer(distance)),
            frc,
            lfrcnp,
            fow,
        }
    }
}

/// `OpenLr::toBinary` for a two-point line reference without offsets.
fn line_binary(first: &ReferencePoint, last: &ReferencePoint) -> [u8; 16] {
    // OpenLrStatus::Line(): version 3, has_attributes, as GCC lays out the bitfield.
    const LINE_STATUS: u8 = 0x0B;
    let mut out = [0_u8; 16];
    out[0] = LINE_STATUS;
    let longitude = decimal_to_integer(first.longitude).to_be_bytes();
    let latitude = decimal_to_integer(first.latitude).to_be_bytes();
    out[1..4].copy_from_slice(&longitude[1..]);
    out[4..7].copy_from_slice(&latitude[1..]);
    out[7] = ((first.frc & 0x7) << 3) | (first.fow & 0x7);
    out[8] = ((first.lfrcnp & 0x7) << 5) | (low_byte(bearing_to_integer(first.bearing)) & 0x1f);
    out[9] = low_byte(distance_to_integer(first.distance));
    let dlng = round_i32(GEOM_SCALE * (last.longitude - first.longitude)).to_be_bytes();
    let dlat = round_i32(GEOM_SCALE * (last.latitude - first.latitude)).to_be_bytes();
    out[10..12].copy_from_slice(&dlng[2..]);
    out[12..14].copy_from_slice(&dlat[2..]);
    // Side of the road (bits 6 and 7) is "directly on the road" for lines.
    out[14] = ((last.frc & 0x7) << 3) | (last.fow & 0x7);
    // No positive or negative offsets.
    out[15] = low_byte(bearing_to_integer(last.bearing)) & 0x1f;
    out
}

/// `midgard::encode64`: standard base64 with padding.
pub(crate) fn encode64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let sextets = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for &sextet in &sextets[..=chunk.len()] {
            out.push(char::from(ALPHABET[sextet as usize]));
        }
        for _ in chunk.len()..3 {
            out.push('=');
        }
    }
    out
}

/// The base64 `OpenLR` line reference of every edge in a leg, as `openlr_edges` builds them.
///
/// Stops at the first node without an edge (the leg's last node), as mainline does.
///
/// # Errors
///
/// Fails if the leg's shape does not decode, or if an edge's shape indexes are out of range.
pub(crate) fn openlr_edges(leg: &TripLeg) -> Result<Vec<String>, OpenLrError> {
    let shape = polyline::decode(&leg.shape, 1e-6)?;
    let mut references = Vec::with_capacity(leg.node.len());
    for node in &leg.node {
        let Some(edge) = &node.edge else { break };
        let fow = form_of_way(edge);
        let frc = low_byte(edge.road_class);
        let point = |index: u32| {
            usize::try_from(index)
                .ok()
                .and_then(|i| shape.get(i).copied().map(|p| (i, p)))
                .ok_or(OpenLrError::ShapeIndex {
                    index,
                    len: shape.len(),
                })
        };
        let (begin, start) = point(edge.begin_shape_index)?;
        let (end, finish) = point(edge.end_shape_index)?;
        let forward_heading = tangent_angle(begin, start, &shape, 20.0, true, begin, end);
        let reverse_heading = tangent_angle(end, finish, &shape, 20.0, false, begin, end);
        let first = ReferencePoint::new(
            start,
            f64::from(forward_heading),
            frc,
            fow,
            None,
            f64::from(edge.length_km * METERS_PER_KM),
            frc,
        );
        let last = ReferencePoint::new(
            finish,
            f64::from(reverse_heading),
            frc,
            fow,
            Some(&first),
            0.0,
            0,
        );
        references.push(encode64(&line_binary(&first, &last)));
    }
    Ok(references)
}

#[cfg(test)]
mod tests {
    use super::*;
    use valhalla_proto::trip_leg::Node;

    #[test]
    fn base64() {
        assert_eq!(encode64(b""), "");
        assert_eq!(encode64(b"f"), "Zg==");
        assert_eq!(encode64(b"fo"), "Zm8=");
        assert_eq!(encode64(b"foo"), "Zm9v");
        assert_eq!(encode64(b"foob"), "Zm9vYg==");
        assert_eq!(encode64(&[0xff, 0xfe]), "//4=");
    }

    #[test]
    fn quantization_round_trips() {
        for value in [0.0, 1.5211, -1.5211, 42.5078, -179.999, 179.999] {
            let decimal = integer_to_decimal(decimal_to_integer(value));
            assert!(
                (decimal - value).abs() < 360.0 / f64::from(1_i32 << 24),
                "{value}"
            );
            assert_eq!(
                decimal_to_integer(decimal),
                decimal_to_integer(value),
                "{value}"
            );
        }
        assert_eq!(bearing_to_integer(integer_to_bearing(7)), 7);
        assert_eq!(distance_to_integer(integer_to_distance(3)), 3);
    }

    #[test]
    fn geometry() {
        let a = LngLat {
            lng: 1.5211,
            lat: 42.5078,
        };
        let b = LngLat {
            lng: 1.5211,
            lat: 42.5178,
        };
        // 0.01 degrees of latitude on mainline's sphere.
        assert!((distance(a, b) - 1113.2).abs() < 0.1);
        assert!(heading(a, b).abs() < 1e-9);
        assert!((heading(b, a) - 180.0).abs() < 1e-9);
        assert!(distance(a, a).abs() <= 0.0);
        let mid = point_along_segment(a, b, 0.5);
        assert!((mid.lat - 42.5128).abs() < 1e-9 && (mid.lng - 1.5211).abs() < 1e-9);
        assert_eq!(point_along_segment(a, b, 0.0), a);
        assert_eq!(point_along_segment(a, b, 1.0), b);
    }

    /// A northbound and then eastbound two-edge leg,
    /// checked against the bytes mainline's `OpenLr::toBinary` writes for the same points.
    #[test]
    fn edges_become_line_references() {
        let shape = [
            LngLat {
                lng: 1.0,
                lat: 42.0,
            },
            LngLat {
                lng: 1.0,
                lat: 42.001,
            },
            LngLat {
                lng: 1.001,
                lat: 42.001,
            },
        ];
        let edge = |begin, end, road_class, traversability| Edge {
            begin_shape_index: begin,
            end_shape_index: end,
            road_class,
            traversability,
            length_km: 0.111,
            ..Edge::default()
        };
        let leg = TripLeg {
            shape: polyline::encode(&shape, 1e6),
            node: vec![
                Node {
                    edge: Some(edge(0, 1, RoadClass::KPrimary as i32, 3)),
                    ..Node::default()
                },
                Node {
                    edge: Some(edge(1, 2, RoadClass::KMotorway as i32, 1)),
                    ..Node::default()
                },
                Node::default(),
            ],
            ..TripLeg::default()
        };
        let references = openlr_edges(&leg).expect("valid leg");
        assert_eq!(references.len(), 2);

        // Decode the first by hand: status, the start point, its attributes, and the end.
        let first = decode64(&references[0]);
        assert_eq!(first.len(), 16);
        assert_eq!(first[0], 0x0B);
        let lng = i32::from_be_bytes([0, first[1], first[2], first[3]]);
        let lat = i32::from_be_bytes([0, first[4], first[5], first[6]]);
        assert_eq!(
            lng,
            decimal_to_integer(integer_to_decimal(decimal_to_integer(1.0)))
        );
        assert_eq!(
            lat,
            decimal_to_integer(integer_to_decimal(decimal_to_integer(42.0)))
        );
        // Primary (frc 2), multiple carriageway (fow 2).
        assert_eq!(first[7], (2 << 3) | 2);
        // lfrcnp 2, bearing 0 (north).
        assert_eq!(first[8], 2 << 5);
        // 111 m is the first 58.6 m bucket.
        assert_eq!(first[9], 1);
        // The reverse bearing (south) is 180 degrees: bucket 16.
        assert_eq!(first[15], 16);
        assert_eq!(first[14], (2 << 3) | 2);

        let second = decode64(&references[1]);
        // Motorway (frc 0, fow 1), heading just north of east
        // (great circles leave a parallel poleward): bucket 7, not 8.
        assert_eq!(second[7], 1);
        assert_eq!(second[8], 7);
    }

    #[test]
    fn bad_shape_index() {
        let leg = TripLeg {
            shape: polyline::encode(&[LngLat { lng: 1.0, lat: 2.0 }], 1e6),
            node: vec![Node {
                edge: Some(Edge {
                    end_shape_index: 3,
                    ..Edge::default()
                }),
                ..Node::default()
            }],
            ..TripLeg::default()
        };
        assert_eq!(
            openlr_edges(&leg),
            Err(OpenLrError::ShapeIndex { index: 3, len: 1 })
        );
    }

    fn decode64(text: &str) -> Vec<u8> {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let sextets: Vec<u32> = text
            .bytes()
            .filter(|&c| c != b'=')
            .map(|c| {
                u32::try_from(ALPHABET.iter().position(|&a| a == c).expect("base64"))
                    .expect("small")
            })
            .collect();
        let mut out = Vec::new();
        for chunk in sextets.chunks(4) {
            let mut n = 0_u32;
            for (i, s) in chunk.iter().enumerate() {
                n |= s << (18 - 6 * i);
            }
            let bytes = n.to_be_bytes();
            out.extend_from_slice(&bytes[1..chunk.len()]);
        }
        out
    }
}
