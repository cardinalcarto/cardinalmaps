//! Route and step geometries (`route_geometry`, `full_shape`, `simplified_shape`,
//! and `maneuver_geometry` in mainline's `route_serializer_osrm.cc`).

use super::geo::generalize;
use super::json::{Json, Map};
use crate::tyr::polyline::{LngLat, decode, encode};
use crate::tyr::unknown;
use valhalla_proto::{DirectionsRoute, ShapeFormat};
use valhalla_response::ValhallaError;

/// mainline's `DECODE_PRECISION`: directions shapes are always polyline6.
const DECODE_PRECISION: f64 = 1e-6;
/// mainline's `ENCODE_PRECISION`.
const ENCODE_PRECISION: f64 = 1e6;

/// How the route-level geometry is chosen (`Options::generalize`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Generalize {
    /// No `generalize` option: the full shape.
    Unset,
    /// A `generalize` value: 0 simplifies for the route's zoom level,
    /// positive values keep the full shape, and negative values give an empty shape.
    Value(f32),
}

/// Decodes a directions leg shape.
pub(super) fn decode_shape(shape: &str) -> Result<Vec<LngLat>, ValhallaError> {
    decode(shape, DECODE_PRECISION).map_err(|e| unknown(e.to_string()))
}

/// Encodes a shape in the requested format, as a JSON value.
pub(super) fn shape_json(shape: &[LngLat], format: ShapeFormat) -> Json {
    match format {
        ShapeFormat::Geojson => geojson_shape(shape).into(),
        ShapeFormat::Polyline6 => Json::Str(encode(shape, ENCODE_PRECISION)),
        // Mainline encodes everything that isn't polyline6 at precision 5.
        ShapeFormat::Polyline5 | ShapeFormat::NoShape => Json::Str(encode(shape, 1e5)),
    }
}

/// `tyr::geojson_shape`: a `GeoJSON` line string with 6 decimals.
fn geojson_shape(shape: &[LngLat]) -> Map {
    let mut geojson = Map::new();
    let coordinates: Vec<Json> = shape
        .iter()
        .map(|p| Json::Array(vec![Json::Fixed(p.lng, 6), Json::Fixed(p.lat, 6)]))
        .collect();
    geojson.emplace("type", "LineString");
    geojson.emplace("coordinates", coordinates);
    geojson
}

/// `route_geometry`: the route's `geometry`, or `None` for `no_shape`.
pub(super) fn route_geometry(
    directions: &DirectionsRoute,
    format: ShapeFormat,
    generalize: Generalize,
) -> Result<Option<Json>, ValhallaError> {
    if format == ShapeFormat::NoShape {
        return Ok(None);
    }
    let shape = match generalize {
        Generalize::Value(0.0) => simplified_shape(directions)?,
        Generalize::Unset => full_shape(directions)?,
        Generalize::Value(value) if value > 0.0 => full_shape(directions)?,
        // Negative (or NaN) values match neither of mainline's branches.
        Generalize::Value(_) => Vec::new(),
    };
    Ok(Some(shape_json(&shape, format)))
}

/// `full_shape`: every leg's shape, joined (each leg starts where the last ended).
fn full_shape(directions: &DirectionsRoute) -> Result<Vec<LngLat>, ValhallaError> {
    let mut decoded: Vec<LngLat> = Vec::new();
    for leg in &directions.legs {
        let leg_shape = decode_shape(&leg.shape)?;
        let skip = usize::from(!decoded.is_empty());
        decoded.extend(leg_shape.into_iter().skip(skip));
    }
    Ok(decoded)
}

/// mainline's `toFixed`, which takes its argument as a `float`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "mainline narrows the coordinate to float and rounds to int32"
)]
fn to_fixed(value: f64) -> i32 {
    let narrowed = f64::from(value as f32);
    (narrowed * ENCODE_PRECISION).round() as i32
}

fn to_floating(fixed: i32) -> f64 {
    f64::from(fixed) * DECODE_PRECISION
}

const TILE_SIZE: f64 = 256.0;
const MAX_ZOOM: u32 = 18;
const MIN_ZOOM: u32 = 1;
const VIEWPORT_WIDTH: f64 = 8.0 * TILE_SIZE;
const VIEWPORT_HEIGHT: f64 = 5.0 * TILE_SIZE;
const DEGREE_TO_RAD: f64 = 0.017_453_292_519_943_295_769_236_907_684_886;
const RAD_TO_DEGREE: f64 = 1.0 / DEGREE_TO_RAD;
#[expect(clippy::excessive_precision, reason = "mainline's literal")]
const EPSG3857_MAX_LATITUDE: f64 = 85.051_128_779_806_592_378;

/// Douglas–Peucker tolerances in meters for zoom levels 0 to 18.
const DOUGLAS_PEUCKER_THRESHOLDS: [f64; 19] = [
    703_125.0, 351_562.5, 175_781.2, 87_890.6, 43_945.3, 21_972.6, 10_986.3, 5_493.1, 2_746.5,
    1_373.2, 686.6, 343.3, 171.6, 85.8, 42.9, 21.4, 10.7, 5.3, 2.6,
];

#[expect(
    clippy::manual_clamp,
    reason = "std::max(std::min(…)) passes NaN through like this"
)]
fn lat_to_y(latitude: f64) -> f64 {
    let clamped = latitude
        .min(EPSG3857_MAX_LATITUDE)
        .max(-EPSG3857_MAX_LATITUDE);
    let f = (DEGREE_TO_RAD * clamped).sin();
    RAD_TO_DEGREE * 0.5 * ((1.0 + f) / (1.0 - f)).ln()
}

fn lng_to_pixel(lon: f64, zoom: u32) -> f64 {
    let shift = f64::from(1_u32 << zoom) * TILE_SIZE;
    let b = shift / 2.0;
    b * (1.0 + lon / 180.0)
}

fn lat_to_pixel(lat: f64, zoom: u32) -> f64 {
    let shift = f64::from(1_u32 << zoom) * TILE_SIZE;
    let b = shift / 2.0;
    b * (1.0 - lat_to_y(lat) / 180.0)
}

/// `getFittedZoom`: the zoom level at which the bounding box fits a large viewport.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "mainline converts the zoom to unsigned, truncating"
)]
fn fitted_zoom(south_west: (i32, i32), north_east: (i32, i32)) -> u32 {
    let min_x = lng_to_pixel(to_floating(south_west.0), MAX_ZOOM);
    let max_y = lat_to_pixel(to_floating(south_west.1), MAX_ZOOM);
    let max_x = lng_to_pixel(to_floating(north_east.0), MAX_ZOOM);
    let min_y = lat_to_pixel(to_floating(north_east.1), MAX_ZOOM);
    let width_ratio = (max_x - min_x) / VIEWPORT_WIDTH;
    let height_ratio = (max_y - min_y) / VIEWPORT_HEIGHT;
    let zoom = f64::from(MAX_ZOOM) - width_ratio.ln().max(height_ratio.ln()) * (1.0 / 2.0_f64.ln());
    if zoom.is_finite() {
        // Zooms are never negative for real routes (a route would have to be wider than
        // the world at zoom 0); `as` saturates where C++ would be undefined.
        MIN_ZOOM.max(zoom as u32)
    } else {
        MIN_ZOOM
    }
}

/// `simplified_shape`: the joined shape, generalized for the zoom level the route fits,
/// keeping every maneuver's first point.
fn simplified_shape(directions: &DirectionsRoute) -> Result<Vec<LngLat>, ValhallaError> {
    let mut south_west = (i32::MAX, i32::MAX);
    let mut north_east = (i32::MIN, i32::MIN);
    let mut simple_shape: Vec<LngLat> = Vec::new();
    let mut indices: Vec<usize> = Vec::new();
    for leg in &directions.legs {
        let decoded_leg = decode_shape(&leg.shape)?;
        for coord in &decoded_leg {
            south_west.0 = south_west.0.min(to_fixed(coord.lng));
            south_west.1 = south_west.1.min(to_fixed(coord.lat));
            north_east.0 = north_east.0.max(to_fixed(coord.lng));
            north_east.1 = north_east.1.max(to_fixed(coord.lat));
        }
        for maneuver in &leg.maneuver {
            let begin = maneuver.begin_shape_index as usize;
            indices.push(match simple_shape.len() {
                0 => begin,
                len => len - 1 + begin,
            });
        }
        let skip = usize::from(!simple_shape.is_empty());
        simple_shape.extend(decoded_leg.into_iter().skip(skip));
    }
    let zoom = MAX_ZOOM.min(fitted_zoom(south_west, north_east));
    let threshold = DOUGLAS_PEUCKER_THRESHOLDS[zoom as usize];
    generalize(&mut simple_shape, threshold, &indices);
    Ok(simple_shape)
}

/// `maneuver_geometry`: the step's shape, from its first to its last shape index
/// (the arrive step repeats the destination, making a two-point line).
pub(super) fn maneuver_geometry(
    begin: u32,
    end: u32,
    shape: &[LngLat],
    is_arrive_maneuver: bool,
    format: ShapeFormat,
) -> Result<Json, ValhallaError> {
    let (begin, end) = (begin as usize, end as usize);
    let mut maneuver_shape = shape
        .get(begin..=end)
        .ok_or_else(|| unknown("maneuver shape indexes are outside the leg's shape"))?
        .to_vec();
    if is_arrive_maneuver && let Some(&last) = shape.last() {
        maneuver_shape.push(last);
    }
    Ok(shape_json(&maneuver_shape, format))
}

#[cfg(test)]
mod tests {
    use super::*;
    use valhalla_proto::DirectionsLeg;
    use valhalla_proto::directions_leg::Maneuver;

    fn route(legs: &[&[LngLat]], maneuver_starts: &[&[u32]]) -> DirectionsRoute {
        DirectionsRoute {
            legs: legs
                .iter()
                .zip(maneuver_starts)
                .map(|(shape, starts)| DirectionsLeg {
                    shape: encode(shape, ENCODE_PRECISION),
                    maneuver: starts
                        .iter()
                        .map(|&begin_shape_index| Maneuver {
                            begin_shape_index,
                            ..Maneuver::default()
                        })
                        .collect(),
                    ..DirectionsLeg::default()
                })
                .collect(),
        }
    }

    fn ll(lng: f64, lat: f64) -> LngLat {
        LngLat { lng, lat }
    }

    fn decoded(json: Option<Json>) -> Vec<LngLat> {
        match json {
            Some(Json::Str(s)) => decode(&s, DECODE_PRECISION).expect("valid"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn legs_are_joined() {
        let a = [ll(1.0, 42.0), ll(1.001, 42.0)];
        let b = [ll(1.001, 42.0), ll(1.002, 42.0)];
        let r = route(&[&a, &b], &[&[0], &[0]]);
        let full =
            decoded(route_geometry(&r, ShapeFormat::Polyline6, Generalize::Unset).expect("ok"));
        assert_eq!(full.len(), 3);
        let positive = decoded(
            route_geometry(&r, ShapeFormat::Polyline6, Generalize::Value(5.0)).expect("ok"),
        );
        assert_eq!(positive, full);
        let negative = decoded(
            route_geometry(&r, ShapeFormat::Polyline6, Generalize::Value(-1.0)).expect("ok"),
        );
        assert!(negative.is_empty());
        assert_eq!(
            route_geometry(&r, ShapeFormat::NoShape, Generalize::Unset).expect("ok"),
            None
        );
    }

    #[test]
    fn simplified_keeps_maneuver_points() {
        // A nearly straight 4-point line, with a maneuver starting at the second leg's middle point.
        let a = [ll(1.0, 42.0), ll(1.0005, 42.00001), ll(1.001, 42.0)];
        let b = [ll(1.001, 42.0), ll(1.0015, 42.00001), ll(1.002, 42.0)];
        let r = route(&[&a, &b], &[&[0, 2], &[0, 1]]);
        let simplified = decoded(
            route_geometry(&r, ShapeFormat::Polyline6, Generalize::Value(0.0)).expect("ok"),
        );
        // Kept: both ends, index 2 (end of the first leg), and index 3 (the second leg's maneuver).
        assert_eq!(
            encode(&simplified, ENCODE_PRECISION),
            encode(&[a[0], a[2], b[1], b[2]], ENCODE_PRECISION)
        );
    }

    #[test]
    fn geojson_and_polyline5() {
        let a = [ll(1.123_456_7, 42.0), ll(1.2, 42.1)];
        match shape_json(&a, ShapeFormat::Geojson) {
            Json::Map(map) => assert_eq!(
                map.to_json(),
                r#"{"type":"LineString","coordinates":[[1.123457,42.000000],[1.200000,42.100000]]}"#
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            shape_json(&a, ShapeFormat::Polyline5),
            Json::Str(encode(&a, 1e5))
        );
    }

    #[test]
    fn step_geometry() {
        let shape = [ll(1.0, 42.0), ll(1.1, 42.0), ll(1.2, 42.0)];
        let arrive = maneuver_geometry(2, 2, &shape, true, ShapeFormat::Polyline6).expect("ok");
        assert_eq!(decoded(Some(arrive)), vec![shape[2], shape[2]]);
        assert!(maneuver_geometry(1, 3, &shape, false, ShapeFormat::Polyline6).is_err());
    }
}
