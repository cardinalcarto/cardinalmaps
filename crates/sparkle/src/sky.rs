//! The root `sky` at one zoom level, ready to draw the sky, the atmosphere around the
//! globe and the fog over distant terrain with.

use crate::style::{Color, Sky};

/// [`Sky`]'s properties evaluated at one zoom. Colors are premultiplied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyColors {
    pub sky: Color,
    pub horizon: Color,
    pub fog: Color,
    /// Where fog starts on the ground, from the center of the view (0) to the horizon (1).
    pub fog_ground_blend: f32,
    /// How much of the fog's color the haze at the horizon takes, rather than the
    /// horizon's.
    pub horizon_fog_blend: f32,
    /// How far up the sky the horizon color reaches, as a fraction of the sky shown.
    pub sky_horizon_blend: f32,
    /// How visible the atmosphere around the globe is.
    pub atmosphere_blend: f32,
}

impl SkyColors {
    pub fn evaluate(sky: &Sky, zoom: f64) -> Self {
        let unit = |v: f32| v.clamp(0.0, 1.0);
        Self {
            sky: sky.sky_color.evaluate_at_zoom(zoom),
            horizon: sky.horizon_color.evaluate_at_zoom(zoom),
            fog: sky.fog_color.evaluate_at_zoom(zoom),
            fog_ground_blend: unit(sky.fog_ground_blend.evaluate_at_zoom(zoom)),
            horizon_fog_blend: unit(sky.horizon_fog_blend.evaluate_at_zoom(zoom)),
            sky_horizon_blend: unit(sky.sky_horizon_blend.evaluate_at_zoom(zoom)),
            atmosphere_blend: unit(sky.atmosphere_blend.evaluate_at_zoom(zoom)),
        }
    }

    /// The haze where the ground meets the sky, which distant ground fades into: the fog
    /// color blended with the horizon's by `horizon-fog-blend`.
    pub fn haze(&self) -> Color {
        let t = self.horizon_fog_blend;
        Color(std::array::from_fn(|i| {
            self.horizon.0[i] + (self.fog.0[i] - self.horizon.0[i]) * t
        }))
    }
}

/// How strongly fog shows at a camera `pitch` (degrees): not at all looking down, where
/// no ground is far away, fading in toward the steep pitches where the horizon shows, as
/// MapLibre does.
pub fn fog_opacity(pitch: f64) -> f32 {
    ((pitch - 45.0) / 20.0).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn evaluates_with_defaults_and_zoom() {
        let style = crate::Style::from_json(&json!({
            "version": 8, "sources": {}, "layers": [],
            "sky": {
                "sky-color": "#000000",
                "fog-color": "#ff0000",
                "horizon-color": "#0000ff",
                "horizon-fog-blend": ["interpolate", ["linear"], ["zoom"], 0, 0, 10, 1],
            }
        }))
        .unwrap();
        let sky = SkyColors::evaluate(style.sky.as_ref().unwrap(), 5.0);
        assert_eq!(sky.sky, Color::BLACK);
        assert_eq!(sky.fog_ground_blend, 0.5);
        assert_eq!(sky.atmosphere_blend, 0.8);
        assert_eq!(sky.horizon_fog_blend, 0.5);
        assert_eq!(sky.haze(), Color([0.5, 0.0, 0.5, 1.0]));
    }

    #[test]
    fn fog_needs_pitch() {
        assert_eq!(fog_opacity(0.0), 0.0);
        assert!(fog_opacity(55.0) > 0.0 && fog_opacity(55.0) < 1.0);
        assert_eq!(fog_opacity(80.0), 1.0);
    }
}
