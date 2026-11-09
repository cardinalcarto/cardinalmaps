//! The zoom 12 slippy-map tile of a coordinate.

use valinor_walk::transfer_tile::TILE_ZOOM;

/// The largest latitude the Web Mercator tile scheme covers, in degrees.
const MAX_LATITUDE: f64 = 85.051_128_779_806_59;

/// The `(x, y)` of the zoom 12 tile holding a coordinate.
///
/// A latitude past what the scheme covers lands in the first or last row;
/// a longitude outside ±180 wraps into the valid range the way a tile server would.
#[must_use]
pub fn tile_of(lat: f64, lon: f64) -> (u32, u32) {
    let scale = f64::from(1_u32 << TILE_ZOOM);
    let wrapped = (lon + 180.0).rem_euclid(360.0);
    let lat = lat.clamp(-MAX_LATITUDE, MAX_LATITUDE).to_radians();
    let raw_x = wrapped / 360.0 * scale;
    let raw_y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / core::f64::consts::PI) / 2.0 * scale;
    (to_tile(raw_x, scale), to_tile(raw_y, scale))
}

fn to_tile(value: f64, scale: f64) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "floored and clamped into the zoom's range"
    )]
    let tile = value.floor().clamp(0.0, scale - 1.0) as u32;
    tile
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_cities_land_in_the_right_tiles() {
        // Checked against the standard OSM slippy-map formula.
        assert_eq!(tile_of(47.6062, -122.3321), (656, 1430));
        assert_eq!(tile_of(0.0, 0.0), (2048, 2048));
        assert_eq!(tile_of(-33.8688, 151.2093), (3768, 2457));
    }

    #[test]
    fn extremes_stay_in_range() {
        assert_eq!(tile_of(90.0, 180.0).1, 0);
        assert_eq!(tile_of(-90.0, -180.0).1, 4095);
        assert_eq!(tile_of(85.5, -181.0), (4084, 0));
        assert_eq!(tile_of(47.0, 180.0).0, 0);
    }

    #[test]
    fn neighbors_in_one_tile_share_it() {
        assert_eq!(tile_of(47.6062, -122.3321), tile_of(47.6063, -122.3322));
    }
}
