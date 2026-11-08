//! # Discretized bounding circles
//!
//! Since Valhalla 3.9, level 2 tiles may store a bounding circle
//! for every entry in the edge bins.
//! Loki uses these to skip decoding edge shapes that cannot be near a search location.
//!
//! Each circle is packed into 32 bits relative to the center of the bin it belongs to:
//! a quantized x and y offset from the bin center,
//! and an index into a fixed table of radii.
//! The layout and decoding mirror `baldr/boundingcircle.h` and `baldr/boundingcircle.cc`.

use bitfield_struct::bitfield;
use core::f32::consts::PI;
use geo::{Coord, coord};
use zerocopy::{LE, U32};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The number of bits used for each of the x and y offsets (`kCoordinateBits`).
const COORDINATE_BITS: u32 = 13;

/// The largest stored offset value (`kMaxOffsetValue`).
const MAX_OFFSET_VALUE: u16 = (1 << COORDINATE_BITS) - 1;

/// Valhalla's (rounded) number of meters per degree of latitude (`midgard::kMetersPerDegreeLat`).
///
/// NOTE: This differs from the constant in [`crate::spatial`];
/// decoding must use Valhalla's value to reproduce its results.
const METERS_PER_DEGREE_LAT: f64 = 110_567.0;

/// Valhalla's single precision degrees to radians factor (`midgard::kRadPerDeg`).
///
/// Valhalla spells pi as `3.14159265f`, which rounds to the same `f32` as [`PI`].
const RAD_PER_DEG: f32 = PI / 180.0;

/// The radii (in meters) that a discretized bounding circle can have,
/// indexed by [`DiscretizedBoundingCircle::radius_index`] (`kBoundingCircleRadii`).
///
/// The table has an entry for every possible 6-bit index.
const BOUNDING_CIRCLE_RADII: [f64; 1 << (32 - 2 * COORDINATE_BITS)] = [
    1., 2., 3., 4., 5., 6., 7., 8., 9., 10., 13., 15., 17., 18., 20., 23., 25., 27., 30., 35., 40.,
    43., 45., 50., 55., 60., 65., 70., 75., 80., 85., 90., 95., 100., 110., 120., 130., 140., 150.,
    160., 170., 180., 190., 200., 210., 220., 230., 240., 250., 275., 300., 325., 350., 375., 400.,
    500., 550., 600., 700., 800., 1000., 1500., 2000., 2500.,
];

/// The largest offset (in meters) from the bin center along either axis (`kMaxOffsetMeters`).
///
/// This is half of a level 2 bin (0.05 degrees) plus the largest radius.
const MAX_OFFSET_METERS: f64 =
    0.05 * METERS_PER_DEGREE_LAT / 2.0 + BOUNDING_CIRCLE_RADII[BOUNDING_CIRCLE_RADII.len() - 1];

/// A bounding circle of an edge shape, quantized relative to the center of an edge bin.
///
/// There is one of these for every entry in a tile's edge bins
/// (see [`GraphTile::bounding_circles_in_bin`](super::GraphTile::bounding_circles_in_bin)).
/// Valhalla stores an invalid circle
/// when it could not compute or represent a circle for an edge,
/// so that the circles stay aligned with the edge bins.
#[bitfield(u32,
    repr = U32<LE>,
    from = bit_twiddling_helpers::conv_u32le::from_inner,
    into = bit_twiddling_helpers::conv_u32le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, KnownLayout, PartialEq, Eq)]
pub struct DiscretizedBoundingCircle {
    /// The quantized y offset from the bin center.
    #[bits(13)]
    pub y_offset: u16,
    /// The quantized x offset from the bin center.
    #[bits(13)]
    pub x_offset: u16,
    /// The index of the radius in the fixed table of radii.
    #[bits(6)]
    pub radius_index: u8,
}

/// A decoded bounding circle.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BoundingCircle {
    /// The center of the circle.
    pub center: Coord<f64>,
    /// The radius of the circle, in meters.
    pub radius_meters: f64,
}

impl DiscretizedBoundingCircle {
    /// Does this record hold an actual bounding circle?
    ///
    /// Valhalla marks a missing circle with an impossible combination:
    /// the largest offsets on both axes with the smallest radius.
    #[inline]
    pub const fn is_valid(&self) -> bool {
        !(self.x_offset() == MAX_OFFSET_VALUE
            && self.y_offset() == MAX_OFFSET_VALUE
            && self.radius_index() == 0)
    }

    /// Decodes the absolute center and radius of the circle.
    ///
    /// `bin_center` must be the center of the bin that this circle belongs to,
    /// as returned by [`GraphTileHeader::bin_center`](super::GraphTileHeader::bin_center).
    ///
    /// Returns `None` if the record does not hold a circle (see [`Self::is_valid`]).
    /// Otherwise, this computes exactly what Valhalla's `DiscretizedBoundingCircle::get` does,
    /// including its mix of single and double precision arithmetic.
    /// The only platform dependence is the C library's `cosf`,
    /// which Rust and Valhalla both call.
    #[inline]
    pub fn decode(&self, bin_center: Coord<f64>) -> Option<BoundingCircle> {
        if !self.is_valid() {
            return None;
        }

        // The stored values are shifted so that 0 is the largest negative offset,
        // the middle value is the bin center,
        // and the largest value is the largest positive offset.
        let y_offset_meters =
            (f64::from(self.y_offset()) / f64::from(MAX_OFFSET_VALUE)) * MAX_OFFSET_METERS * 2.0
                - MAX_OFFSET_METERS;
        let x_offset_meters =
            (f64::from(self.x_offset()) / f64::from(MAX_OFFSET_VALUE)) * MAX_OFFSET_METERS * 2.0
                - MAX_OFFSET_METERS;

        Some(BoundingCircle {
            center: coord! {
                x: x_offset_meters / meters_per_lng_degree(bin_center.y) + bin_center.x,
                y: y_offset_meters / METERS_PER_DEGREE_LAT + bin_center.y,
            },
            // The index has 6 bits, and the table has 64 entries.
            radius_meters: BOUNDING_CIRCLE_RADII[usize::from(self.radius_index())],
        })
    }
}

/// The meters per degree of longitude at a latitude,
/// computed like Valhalla's `DistanceApproximator::GetMetersPerLngDegree`.
///
/// Valhalla computes the longitude scale with `cosf`,
/// so the angle is rounded to single precision before taking the cosine.
#[inline]
fn meters_per_lng_degree(lat: f64) -> f64 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Valhalla passes the angle to cosf, which rounds it to single precision"
    )]
    let angle = (lat * f64::from(RAD_PER_DEG)) as f32;
    f64::from(angle.cos()) * METERS_PER_DEGREE_LAT
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    #[expect(
        clippy::float_cmp,
        clippy::approx_constant,
        clippy::excessive_precision,
        reason = "the constants must equal Valhalla's exactly, including its spelling of pi"
    )]
    fn constants_match_valhalla() {
        assert_eq!(MAX_OFFSET_VALUE, 8191);
        assert_eq!(BOUNDING_CIRCLE_RADII.len(), 64);
        assert_eq!(BOUNDING_CIRCLE_RADII[63], 2500.0);
        // 0.05 * 110567 / 2 + 2500
        assert_eq!(MAX_OFFSET_METERS, 5264.175);
        assert_eq!(RAD_PER_DEG, 3.141_592_65_f32 / 180.0);
    }

    #[test]
    fn bit_layout() {
        // y_offset is in the lowest bits, then x_offset, then radius_index (as in the C++ struct).
        let circle =
            DiscretizedBoundingCircle::from_bits(U32::new((37 << 26) | (0x1abc << 13) | 0x0123));
        assert_eq!(circle.y_offset(), 0x0123);
        assert_eq!(circle.x_offset(), 0x1abc);
        assert_eq!(circle.radius_index(), 37);
    }

    #[test]
    fn invalid_circle() {
        let invalid = DiscretizedBoundingCircle::new()
            .with_x_offset(MAX_OFFSET_VALUE)
            .with_y_offset(MAX_OFFSET_VALUE)
            .with_radius_index(0);
        assert!(!invalid.is_valid());
        assert_eq!(invalid.decode(coord! {x: 0.0, y: 0.0}), None);

        // Any other radius makes it valid again.
        let valid = invalid.with_radius_index(1);
        assert!(valid.is_valid());
        assert!(valid.decode(coord! {x: 0.0, y: 0.0}).is_some());
    }

    #[test]
    fn decode_extremes_at_equator() {
        // At the equator, cosf(0) is exactly 1,
        // so the longitude and latitude scales are both 110567 meters per degree.
        let bin_center = coord! {x: 10.0, y: 0.0};

        // The smallest stored values are the largest negative offsets.
        let min = DiscretizedBoundingCircle::new().with_radius_index(63);
        assert_eq!(
            min.decode(bin_center),
            Some(BoundingCircle {
                center: coord! {
                    x: -5264.175 / 110_567.0 + 10.0,
                    y: -5264.175 / 110_567.0,
                },
                radius_meters: 2500.0,
            })
        );

        // The largest x offset is the largest positive offset (8191 / 8191 is exactly 1).
        // The radius index alone keeps this from being the invalid marker.
        let max = DiscretizedBoundingCircle::new()
            .with_x_offset(MAX_OFFSET_VALUE)
            .with_y_offset(MAX_OFFSET_VALUE)
            .with_radius_index(1);
        assert_eq!(
            max.decode(bin_center),
            Some(BoundingCircle {
                center: coord! {
                    x: 5264.175 / 110_567.0 + 10.0,
                    y: 5264.175 / 110_567.0,
                },
                radius_meters: 2.0,
            })
        );
    }

    proptest! {
        #[test]
        fn accessors_cover_all_bits(raw: u32) {
            let circle = DiscretizedBoundingCircle::from_bits(U32::new(raw));
            let recomposed = u32::from(circle.y_offset())
                | (u32::from(circle.x_offset()) << 13)
                | (u32::from(circle.radius_index()) << 26);
            prop_assert_eq!(recomposed, raw);
        }

        #[test]
        fn decoded_center_stays_within_max_offset(
            raw: u32,
            lng in -179.9f64..179.9,
            lat in -84.0f64..84.0,
        ) {
            let circle = DiscretizedBoundingCircle::from_bits(U32::new(raw));
            let bin_center = coord! {x: lng, y: lat};
            match circle.decode(bin_center) {
                None => prop_assert!(!circle.is_valid()),
                Some(decoded) => {
                    prop_assert!(circle.is_valid());
                    prop_assert!(BOUNDING_CIRCLE_RADII.contains(&decoded.radius_meters));
                    let dy = (decoded.center.y - lat) * METERS_PER_DEGREE_LAT;
                    let dx = (decoded.center.x - lng) * meters_per_lng_degree(lat);
                    // Allow for rounding in the conversion to degrees and back.
                    prop_assert!(dy.abs() <= MAX_OFFSET_METERS + 1e-6);
                    prop_assert!(dx.abs() <= MAX_OFFSET_METERS + 1e-6);
                }
            }
        }
    }
}
