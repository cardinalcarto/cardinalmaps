//! The style's root `light` as fill extrusions are shaded with it.
//!
//! [`ExtrusionLight`] is what a renderer evaluates once per frame and hands to its shader;
//! [`ExtrusionLight::shade`] is the per-vertex shading MapLibre's fill-extrusion shader
//! does, so that renderers can port it exactly (and so it can be tested here).

use crate::style::{Color, IlluminationAnchor, Light};

/// The light for one frame, in the coordinates of tiles (x east, y south, z up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExtrusionLight {
    /// Towards the light, `radial` long. With the `viewport` anchor it turns with the
    /// bearing, so the light stays put on screen as the map rotates.
    pub position: [f32; 3],
    /// Premultiplied, as every color is.
    pub color: [f32; 3],
    pub intensity: f32,
}

impl ExtrusionLight {
    /// The light at `zoom`, for a camera turned `bearing` degrees clockwise from north.
    pub fn new(light: &Light, zoom: f64, bearing: f64) -> Self {
        let [radial, azimuthal, polar] = light.position.evaluate_at_zoom(zoom);
        // MapLibre's `sphericalToCartesian`: azimuths are from north, but the math has 0
        // along x, hence the quarter turn.
        let azimuthal = (azimuthal + 90.0).to_radians();
        let polar = polar.to_radians();
        let mut position = [
            radial * azimuthal.cos() * polar.sin(),
            radial * azimuthal.sin() * polar.sin(),
            radial * polar.cos(),
        ];
        if light.anchor.evaluate_at_zoom(zoom) == IlluminationAnchor::Viewport {
            let (sin, cos) = (bearing.to_radians() as f32).sin_cos();
            let [x, y, _] = position;
            position[0] = cos * x - sin * y;
            position[1] = sin * x + cos * y;
        }
        let Color([r, g, b, _]) = light.color.evaluate_at_zoom(zoom);
        Self {
            position,
            color: [r, g, b],
            intensity: light.intensity.evaluate_at_zoom(zoom),
        }
    }

    /// The color of an extrusion vertex: `color` (premultiplied) lit by this light, on a
    /// surface with unit `normal` (as in [`WallVertex`](crate::tile::WallVertex), or up on roofs),
    /// `top` 1 at the roof and 0 at the base, between `base` and `height` meters. The
    /// result is opaque; `fill-extrusion-opacity` applies to the whole layer afterwards.
    ///
    /// With a pattern, MapLibre shades white instead and multiplies the pattern by the
    /// result, with a slightly different contrast curve: see `pattern` below.
    pub fn shade(
        &self,
        color: [f32; 4],
        normal: [f32; 3],
        top: f32,
        [base, height]: [f32; 2],
        vertical_gradient: bool,
        pattern: bool,
    ) -> [f32; 4] {
        let (base, height) = (base.max(0.0), height.max(0.0));
        let intensity = self.intensity;
        let [r, g, b, _] = color;
        // Relative luminance: darker colors get more contrast between lit and shaded sides.
        let luminance = r * 0.2126 + g * 0.7152 + b * 0.0722;
        // MapLibre packs normals as `n * 2^15` and divides by `2^14` in the shader, so they
        // reach the dot product doubled; the clamp makes the difference visible.
        let directional = (2.0 * dot(normal, self.position)).clamp(0.0, 1.0);
        let upper = if pattern {
            (0.5 + intensity).max(1.0)
        } else {
            (1.0 - luminance + intensity).max(1.0)
        };
        let mut directional = mix(1.0 - intensity, upper, directional);
        if normal[2] == 0.0 {
            // Walls darken towards the ground; without a vertical gradient, by a constant.
            let gradient = ((top + base) * (height / 150.0).sqrt())
                .clamp(mix(0.7, 0.98, 1.0 - intensity), 1.0);
            directional *= if vertical_gradient { gradient } else { 1.0 };
        }
        // A small ambient term so nothing is black, and shadows tinted with the light's
        // complementary color.
        let surface = if pattern {
            [1.0; 3]
        } else {
            [r + 0.03, g + 0.03, b + 0.03]
        };
        let mut out = [0.0, 0.0, 0.0, 1.0];
        for i in 0..3 {
            let light = self.color[i];
            out[i] = (surface[i] * directional * light).clamp(mix(0.0, 0.3, 1.0 - light), 1.0);
        }
        out
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Style;

    fn light(json: serde_json::Value) -> Light {
        let style: Style = json!({"version": 8, "sources": {}, "layers": [], "light": json})
            .to_string()
            .parse()
            .unwrap();
        style.light
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5)
    }

    #[test]
    fn default_position() {
        let l = ExtrusionLight::new(&light(json!({})), 15.0, 0.0);
        // [1.15, 210, 30]: azimuth 300 degrees in the math's terms.
        let h = 1.15 * 0.5;
        assert!(close(
            l.position,
            [h * 0.5, -h * 3f32.sqrt() / 2.0, 1.15 * 3f32.sqrt() / 2.0]
        ));
        assert_eq!(l.intensity, 0.5);
        assert_eq!(l.color, [1.0; 3]);
    }

    #[test]
    fn viewport_anchor_turns_with_the_bearing() {
        let json = json!({"position": [1, 90, 90]});
        let north = ExtrusionLight::new(&light(json.clone()), 15.0, 0.0);
        let turned = ExtrusionLight::new(&light(json.clone()), 15.0, 90.0);
        assert!(close(north.position, [-1.0, 0.0, 0.0]));
        assert!(close(turned.position, [0.0, -1.0, 0.0]));

        let mut map = json;
        map["anchor"] = json!("map");
        let fixed = ExtrusionLight::new(&light(map), 15.0, 90.0);
        assert!(close(fixed.position, north.position));
    }

    #[test]
    fn shading() {
        let l = ExtrusionLight::new(&light(json!({})), 15.0, 0.0);
        let gray = [0.5, 0.5, 0.5, 1.0];
        let shade = |normal| l.shade(gray, normal, 1.0, [0.0, 30.0], false, false);
        let roof = shade([0.0, 0.0, 1.0]);
        // Walls facing the light's side (normals are mirrored, see WallVertex).
        let lit = shade([0.0, -1.0, 0.0]);
        let shaded = shade([0.0, 1.0, 0.0]);
        assert!(roof[0] > lit[0] || roof[0] == 1.0);
        assert!(lit[0] > shaded[0], "{lit:?} {shaded:?}");
        assert_eq!(roof[3], 1.0);
        // Shaded sides never go fully dark: 1 - intensity, plus ambient.
        assert!((shaded[0] - 0.53 * 0.5).abs() < 1e-5, "{shaded:?}");

        // A vertical gradient darkens the bottom of tall enough walls.
        let bottom = l.shade(gray, [0.0, -1.0, 0.0], 0.0, [0.0, 150.0], true, false);
        let top = l.shade(gray, [0.0, -1.0, 0.0], 1.0, [0.0, 150.0], true, false);
        assert!(bottom[0] < top[0]);

        // A red light tints shadows cyan.
        let red = ExtrusionLight::new(&light(json!({"color": "red"})), 15.0, 0.0);
        let s = red.shade(
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0],
            1.0,
            [0.0, 10.0],
            false,
            false,
        );
        assert!(
            (s[1] - 0.3).abs() < 1e-6 && (s[2] - 0.3).abs() < 1e-6,
            "{s:?}"
        );
    }
}
