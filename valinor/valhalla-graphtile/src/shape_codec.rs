//! # Shape encoding/decoding
//!
//! Valhalla uses a mix of varint and delta encoding to efficiently pack shape information.
//! This module contains various helpers for converting back and forth.
//!
//! See Google's [protobuf docs](https://protobuf.dev/programming-guides/encoding/)
//! for info on varint encoding generally.
//!
//! Shapes come from tile data, so decoding never panics:
//! malformed varints and coordinates that overflow are both errors.

use geo::{Coord, CoordFloat, coord};
use integer_encoding::VarIntReader;
use num_traits::FromPrimitive;
use std::io::{Error, ErrorKind};

const DECODE_PRECISION: f64 = 1e-6;

/// Accumulates one delta-encoded coordinate component.
///
/// # Errors
///
/// Fails if the running total overflows an `i32`,
/// which no valid shape does (the totals are micro-degrees).
#[inline]
fn add_delta(total: i32, delta: i32) -> std::io::Result<i32> {
    total.checked_add(delta).ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidData,
            "the shape's coordinates overflow a 32-bit integer",
        )
    })
}

/// Scales a fixed-precision coordinate to degrees.
///
/// # Errors
///
/// Fails if `T` cannot represent the value (this never happens for `f32` or `f64`).
#[inline]
fn to_degrees<T: CoordFloat + FromPrimitive>(value: i32, precision: T) -> std::io::Result<T> {
    T::from(value)
        .map(|value| value * precision)
        .ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidData,
                "the coordinate does not fit in the float type",
            )
        })
}

/// The decoding precision as a `T`.
///
/// # Errors
///
/// Fails if `T` cannot represent the value (this never happens for `f32` or `f64`).
#[inline]
fn precision<T: CoordFloat + FromPrimitive>() -> std::io::Result<T> {
    T::from(DECODE_PRECISION).ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidData,
            "the decoding precision does not fit in the float type",
        )
    })
}

/// Decodes a Valhalla encoded shape from a byte buffer of exact size.
///
/// # Errors
///
/// Decoding may fail if the byte buffer is not exactly sized
/// (you cannot include extra information at the end of a slice).
/// It will also fail if the varint data is malformed,
/// or if the coordinates overflow.
pub fn decode_shape<T: CoordFloat + FromPrimitive>(bytes: &[u8]) -> std::io::Result<Vec<Coord<T>>> {
    // Pre-allocating 1/4 the byte len is an optimization from Valhalla.
    // This sounds about right since we can expect most values to be small due to delta encoding.
    let mut coords: Vec<Coord<T>> = Vec::with_capacity(bytes.len() / 4);

    let prec = precision()?;

    let mut lat = 0;
    let mut lon = 0;
    let mut bytes = bytes;
    while !bytes.is_empty() {
        lat = add_delta(lat, bytes.read_varint::<i32>()?)?;
        lon = add_delta(lon, bytes.read_varint::<i32>()?)?;
        coords.push(coord! {
            x: to_degrees(lon, prec)?,
            y: to_degrees(lat, prec)?,
        });
    }

    Ok(coords)
}

/// Decodes the _first_ coordinate in a Valhalla encoded shape from a byte buffer.
///
/// # Errors
///
/// Decoding may fail if the byte buffer is incorrectly sized or corrupt.
pub fn decode_first_coordinate<T: CoordFloat + FromPrimitive>(
    bytes: &[u8],
) -> std::io::Result<Coord<T>> {
    let mut bytes = bytes;

    let prec = precision()?;

    let lat = bytes.read_varint::<i32>()?;
    let lon = bytes.read_varint::<i32>()?;

    Ok(coord! {
        x: to_degrees(lon, prec)?,
        y: to_degrees(lat, prec)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use integer_encoding::VarInt;
    use proptest::{prop_assert_eq, proptest};

    fn encode(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.encode_var_vec()).collect()
    }

    #[test]
    fn test_decode_shape() {
        // Two points: (lon 1.5, lat 42.5), then a delta of (+0.000001, -0.000002).
        let bytes = encode(&[42_500_000, 1_500_000, -2, 1]);
        let shape = decode_shape::<f64>(&bytes).expect("valid shape");
        assert_eq!(shape.len(), 2);
        assert!((shape[0].x - 1.5).abs() < 1e-9);
        assert!((shape[0].y - 42.5).abs() < 1e-9);
        assert!((shape[1].x - 1.500_001).abs() < 1e-9);
        assert!((shape[1].y - 42.499_998).abs() < 1e-9);

        let first = decode_first_coordinate::<f64>(&bytes).expect("valid shape");
        assert_eq!(first, shape[0]);
    }

    #[test]
    fn test_overflowing_shape_is_an_error() {
        let bytes = encode(&[i32::MAX, 0, 1, 0]);
        let error = decode_shape::<f64>(&bytes).expect_err("the latitude overflows");
        assert_eq!(error.kind(), ErrorKind::InvalidData);

        let bytes = encode(&[0, i32::MIN, 0, -1]);
        let error = decode_shape::<f32>(&bytes).expect_err("the longitude underflows");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn test_truncated_shape_is_an_error() {
        // A latitude without a longitude.
        let bytes = encode(&[42_500_000]);
        assert!(decode_shape::<f64>(&bytes).is_err());
        assert!(decode_first_coordinate::<f64>(&bytes).is_err());
        assert!(decode_first_coordinate::<f64>(&[]).is_err());
    }

    proptest! {
        #[test]
        fn decoding_arbitrary_bytes_never_panics(bytes: Vec<u8>) {
            let shape = decode_shape::<f64>(&bytes);
            let first = decode_first_coordinate::<f64>(&bytes);
            // Whenever both succeed on a non-empty shape, they agree on the first point.
            if let (Ok(shape), Ok(first)) = (shape, first)
                && let Some(&expected) = shape.first()
            {
                prop_assert_eq!(first, expected);
            }
            let _ = decode_shape::<f32>(&bytes);
        }
    }
}
