//! `heatmap`: point density, accumulated offscreen and colored by `heatmap-color`.

use maplibre_expr::EvaluationContext;

use super::{layer, properties};
use crate::paint::PaintProperty;
use crate::style::Color;

properties! {
    pub struct HeatmapLayout {}
}

properties! {
    pub struct HeatmapPaint {
        /// In pixels.
        radius: f32 = "heatmap-radius",
        weight: f32 = "heatmap-weight",
        intensity: f32 = "heatmap-intensity",
        /// Reads `["heatmap-density"]`; the default is a blue to red ramp.
        color: Color = "heatmap-color",
        opacity: f32 = "heatmap-opacity",
    }
}

layer!(HeatmapLayer = "heatmap", HeatmapLayout, HeatmapPaint);

impl HeatmapPaint {
    /// Entries in [`Self::color_ramp`], as in MapLibre.
    pub const RAMP_SIZE: usize = 256;

    /// The paint properties the density shader reads per point, by their names in WGSL.
    /// `heatmap-intensity` and `heatmap-opacity` can only depend on the zoom, so renderers
    /// evaluate those themselves.
    pub fn attributes(&self) -> [(&'static str, &dyn PaintProperty); 2] {
        [("weight", &self.weight), ("radius", &self.radius)]
    }

    /// `heatmap-color` at densities `0..=1` in [`Self::RAMP_SIZE`] steps, as premultiplied
    /// RGBA8: the texture densities are looked up in. The spec doesn't let `heatmap-color`
    /// read the zoom or the feature, so one ramp serves the whole layer.
    pub fn color_ramp(&self) -> Vec<[u8; 4]> {
        let mut ctx = EvaluationContext::new();
        Color::ramp(Self::RAMP_SIZE, [0.0, 1.0], |density| {
            ctx.heatmap_density = Some(density);
            self.color.evaluate(&ctx)
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::style::PropertyValue;

    #[test]
    fn default_ramp_fades_in_from_transparent() {
        let ramp = HeatmapLayer::default().paint.color_ramp();
        assert_eq!(ramp.len(), HeatmapPaint::RAMP_SIZE);
        // The spec default: transparent blue at 0, royal blue at 0.1, ..., red at 1.
        assert_eq!(ramp[0], [0, 0, 0, 0]);
        assert_eq!(ramp[255], [255, 0, 0, 255]);
        let alpha: Vec<u8> = ramp.iter().map(|c| c[3]).collect();
        assert!(alpha.windows(2).all(|w| w[0] <= w[1]), "{alpha:?}");
    }

    #[test]
    fn ramp_is_premultiplied() {
        let paint = HeatmapPaint {
            color: PropertyValue::from_json(
                "heatmap-color",
                &json!([
                    "interpolate",
                    ["linear"],
                    ["heatmap-density"],
                    0,
                    "rgba(255, 255, 255, 0)",
                    1,
                    "rgba(255, 255, 255, 0.5)"
                ]),
                Color::TRANSPARENT,
            )
            .unwrap(),
            ..Default::default()
        };
        let ramp = paint.color_ramp();
        assert_eq!(ramp[0], [0; 4]);
        assert_eq!(ramp[255], [128; 4]);
        let mid = ramp[128];
        assert!(mid[0] == mid[3] && (63..=65).contains(&mid[3]), "{mid:?}");
    }
}
