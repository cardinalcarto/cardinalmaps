//! Encoded polylines, as mainline's `midgard::encode` and `midgard::decode` write and read them.

use thiserror::Error;

/// A longitude and latitude in degrees, as mainline's `PointLL` (`first` is the longitude).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LngLat {
    pub(crate) lng: f64,
    pub(crate) lat: f64,
}

/// An encoded polyline ended in the middle of a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("decode an encoded polyline: it ends in the middle of a number")]
pub(crate) struct BadPolyline;

/// Decodes a polyline, as `midgard::decode` (`Shape5Decoder`) does.
///
/// `precision` is what one encoded unit is worth in degrees, for example `1e-6` for polyline6.
/// Each coordinate is the accumulated integer times `precision`, computed in `f64`.
///
/// # Errors
///
/// Fails, as mainline does, when the string ends inside a number.
pub(crate) fn decode(encoded: &str, precision: f64) -> Result<Vec<LngLat>, BadPolyline> {
    let bytes = encoded.as_bytes();
    let mut points = Vec::with_capacity(bytes.len() / 4);
    let mut pos = 0;
    let (mut lat, mut lng) = (0_i32, 0_i32);
    while pos < bytes.len() {
        lat = lat.wrapping_add(zigzag_decode(read_varint(bytes, &mut pos)?));
        lng = lng.wrapping_add(zigzag_decode(read_varint(bytes, &mut pos)?));
        points.push(LngLat {
            lng: f64::from(lng) * precision,
            lat: f64::from(lat) * precision,
        });
    }
    Ok(points)
}

fn read_varint(bytes: &[u8], pos: &mut usize) -> Result<u32, BadPolyline> {
    let (mut result, mut shift) = (0_u32, 0_u32);
    loop {
        let &byte = bytes.get(*pos).ok_or(BadPolyline)?;
        *pos += 1;
        // Mainline reads the byte as a signed char before subtracting,
        // which only matters for bytes no encoder writes.
        let byte = u32::from_ne_bytes(i32::from(byte.cast_signed()).to_ne_bytes()).wrapping_sub(63);
        // Shifts past 31 bits are masked, as on the x86-64 builds mainline runs on.
        result |= (byte & 0x1f).wrapping_shl(shift);
        shift += 5;
        if byte < 0x20 {
            return Ok(result);
        }
    }
}

fn zigzag_decode(value: u32) -> i32 {
    (value >> 1).cast_signed() ^ (-(value & 1).cast_signed())
}

fn zigzag_encode(value: i32) -> u32 {
    (value.wrapping_shl(1) ^ (value >> 31)).cast_unsigned()
}

/// Encodes points as a polyline, as `midgard::encode` does.
///
/// `factor` is the number of encoded units per degree, for example `1e6` for polyline6.
pub(crate) fn encode(points: &[LngLat], factor: f64) -> String {
    let mut out = String::with_capacity(points.len() * 8);
    let (mut last_lat, mut last_lng) = (0_i32, 0_i32);
    for point in points {
        let lng = round_to_i32(point.lng * factor);
        let lat = round_to_i32(point.lat * factor);
        write_varint(&mut out, zigzag_encode(lat.wrapping_sub(last_lat)));
        write_varint(&mut out, zigzag_encode(lng.wrapping_sub(last_lng)));
        last_lat = lat;
        last_lng = lng;
    }
    out
}

/// `static_cast<int32_t>(round(x))`; out-of-range values saturate
/// (C++ leaves them undefined; they cannot occur for valid coordinates).
#[expect(
    clippy::cast_possible_truncation,
    reason = "the value is rounded and saturates outside the i32 range"
)]
fn round_to_i32(value: f64) -> i32 {
    value.round() as i32
}

fn write_varint(out: &mut String, mut number: u32) {
    while number >= 0x20 {
        out.push(char::from(digit((0x20 | (number & 0x1f)) + 63)));
        number >>= 5;
    }
    out.push(char::from(digit(number + 63)));
}

/// A polyline character; every value passed here is below 127.
fn digit(value: u32) -> u8 {
    u8::try_from(value).unwrap_or(b'?')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        // The Google polyline documentation's example, at precision 5.
        let points = [
            LngLat {
                lng: -120.2,
                lat: 38.5,
            },
            LngLat {
                lng: -120.95,
                lat: 40.7,
            },
            LngLat {
                lng: -126.453,
                lat: 43.252,
            },
        ];
        let encoded = encode(&points, 1e5);
        assert_eq!(encoded, "_p~iF~ps|U_ulLnnqC_mqNvxq`@");
        let decoded = decode(&encoded, 1e-5).expect("valid polyline");
        assert_eq!(decoded.len(), 3);
        for (a, b) in decoded.iter().zip(&points) {
            assert!((a.lng - b.lng).abs() < 1e-9 && (a.lat - b.lat).abs() < 1e-9);
        }
        assert_eq!(encode(&decoded, 1e5), encoded);
    }

    #[test]
    fn truncated() {
        assert_eq!(decode("", 1e-6), Ok(Vec::new()));
        assert_eq!(decode("_p~iF", 1e-6), Err(BadPolyline));
        assert_eq!(decode("_p~iF~ps|", 1e-6), Err(BadPolyline));
    }
}
