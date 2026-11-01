//! `hillshade` layers, drawn from `raster-dem` tiles.
//!
//! Tile processing computes the slopes once per tile ([`DemData::hillshade_slopes`]); a
//! renderer shades them per pixel with the light from [`HillshadeLight::evaluate`]. When a
//! neighbouring tile arrives, renderers backfill [`HillshadeBucket::dem`]'s border and
//! recompute the slopes so the shading continues across the seam.

use std::f64::consts::PI;
use std::sync::Arc;

use super::TileId;
use super::raster::RasterImage;
use crate::dem::DemData;
use crate::style::{Color, HillshadeMethod, HillshadePaint, IlluminationAnchor};

#[derive(Debug, Clone)]
pub struct HillshadeBucket {
    pub tile: TileId,
    pub dem: Arc<DemData>,
    /// `dem.hillshade_slopes(tile.z)`, `dem.dim`² pixels.
    pub slopes: Arc<RasterImage>,
}

pub(crate) fn bucket(tile: TileId, dem: Arc<DemData>, slopes: Arc<RasterImage>) -> HillshadeBucket {
    HillshadeBucket { tile, dem, slopes }
}

/// One light of a hillshade layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightSource {
    /// Radians clockwise from the top of the map (the screen's top with a viewport anchor,
    /// which this already accounts for).
    pub azimuth: f32,
    /// Radians above the horizon.
    pub altitude: f32,
    pub shadow: Color,
    pub highlight: Color,
}

/// A hillshade layer's paint at one zoom, ready to shade with.
#[derive(Debug, Clone, PartialEq)]
pub struct HillshadeLight {
    pub method: HillshadeMethod,
    pub exaggeration: f32,
    pub accent: Color,
    /// One per `hillshade-illumination-direction`; only `multidirectional` uses more than
    /// the first. Never empty.
    pub sources: Vec<LightSource>,
}

impl HillshadeLight {
    /// Evaluate `paint` at `zoom`, for a map turned `bearing` degrees.
    ///
    /// The altitudes and colors pair up with the directions; a shorter array repeats its
    /// last value.
    pub fn evaluate(paint: &HillshadePaint, zoom: f64, bearing: f64) -> Self {
        let directions = paint.illumination_direction.evaluate_at_zoom(zoom).0;
        let altitudes = paint.illumination_altitude.evaluate_at_zoom(zoom).0;
        let shadows = paint.shadow_color.evaluate_at_zoom(zoom).0;
        let highlights = paint.highlight_color.evaluate_at_zoom(zoom).0;
        let anchor = paint.illumination_anchor.evaluate_at_zoom(zoom);
        let count = directions.len().max(1);
        let sources = (0..count)
            .map(|i| {
                let mut azimuth = f64::from(pick(&directions, i, 335.0)).to_radians();
                if anchor == IlluminationAnchor::Viewport {
                    azimuth += bearing.to_radians();
                }
                LightSource {
                    azimuth: azimuth.rem_euclid(2.0 * PI) as f32,
                    altitude: f64::from(pick(&altitudes, i, 45.0)).to_radians() as f32,
                    shadow: pick(&shadows, i, Color::BLACK),
                    highlight: pick(&highlights, i, Color::WHITE),
                }
            })
            .collect();
        Self {
            method: paint.method.evaluate_at_zoom(zoom),
            exaggeration: paint.exaggeration.evaluate_at_zoom(zoom),
            accent: paint.accent_color.evaluate_at_zoom(zoom),
            sources,
        }
    }
}

/// `values[i]`, else the last value, else `default`.
fn pick<T: Copy>(values: &[T], i: usize, default: T) -> T {
    values.get(i).or(values.last()).copied().unwrap_or(default)
}

/// The latitudes in degrees of `tile`'s top and bottom edges. Slopes are computed with the
/// meters per pixel at the equator, so shading divides them by the cosine of the latitude.
pub fn latitude_range(tile: TileId) -> [f64; 2] {
    let n = f64::from(1u32 << tile.z);
    let lat = |y: f64| (PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
    [lat(f64::from(tile.y)), lat(f64::from(tile.y) + 1.0)]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::style::LayerKind;

    fn paint(paint: serde_json::Value) -> HillshadePaint {
        let style = crate::Style::from_json(&json!({
            "version": 8,
            "sources": {"dem": {"type": "raster-dem"}},
            "layers": [{"id": "h", "type": "hillshade", "source": "dem", "paint": paint}]
        }))
        .unwrap();
        match &style.layers[0].kind {
            LayerKind::Hillshade(l) => l.paint.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn defaults_are_one_light_from_the_northwest() {
        let light = HillshadeLight::evaluate(&paint(json!({})), 10.0, 0.0);
        assert_eq!(light.method, HillshadeMethod::Standard);
        assert_eq!(light.exaggeration, 0.5);
        assert_eq!(light.sources.len(), 1);
        let s = light.sources[0];
        assert!((s.azimuth - 335f32.to_radians()).abs() < 1e-6);
        assert!((s.altitude - 45f32.to_radians()).abs() < 1e-6);
        assert_eq!(s.shadow, Color::BLACK);
        assert_eq!(s.highlight, Color::WHITE);
    }

    #[test]
    fn viewport_anchor_turns_with_the_map() {
        let p = paint(json!({"hillshade-illumination-direction": 90}));
        let light = HillshadeLight::evaluate(&p, 10.0, 30.0);
        assert!((light.sources[0].azimuth - 120f32.to_radians()).abs() < 1e-6);
        let p = paint(json!({
            "hillshade-illumination-direction": 90,
            "hillshade-illumination-anchor": "map"
        }));
        let light = HillshadeLight::evaluate(&p, 10.0, 30.0);
        assert!((light.sources[0].azimuth - 90f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn multidirectional_pairs_arrays_up() {
        let p = paint(json!({
            "hillshade-method": "multidirectional",
            "hillshade-illumination-direction": [270, 315, 0],
            "hillshade-illumination-altitude": [30, 60],
            "hillshade-shadow-color": ["#ff0000", "#00ff00", "#0000ff"],
            "hillshade-highlight-color": "#ffff00",
            "hillshade-illumination-anchor": "map"
        }));
        let light = HillshadeLight::evaluate(&p, 10.0, 0.0);
        assert_eq!(light.method, HillshadeMethod::Multidirectional);
        assert_eq!(light.sources.len(), 3);
        assert!((light.sources[2].altitude - 60f32.to_radians()).abs() < 1e-6);
        assert_eq!(light.sources[1].shadow, Color([0.0, 1.0, 0.0, 1.0]));
        assert_eq!(light.sources[2].highlight, Color([1.0, 1.0, 0.0, 1.0]));
    }

    #[test]
    fn latitude_range_spans_the_tile() {
        let [top, bottom] = latitude_range(TileId::new(0, 0, 0));
        assert!((top - 85.0511).abs() < 1e-3 && (bottom + 85.0511).abs() < 1e-3);
        let [top, bottom] = latitude_range(TileId::new(12, 656, 1430));
        assert!(
            top > 47.6 && bottom < top && bottom > 47.5,
            "{top} {bottom}"
        );
    }
}
