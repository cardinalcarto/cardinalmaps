//! `circle`: a disc at every point.

use super::{layer, properties};
use crate::paint::PaintProperty;
use crate::style::{CirclePitchAlignment, CirclePitchScale, Color, TranslateAnchor};

properties! {
    pub struct CircleLayout {
        sort_key: Option<f32> = "circle-sort-key",
    }
}

properties! {
    pub struct CirclePaint {
        /// In pixels.
        radius: f32 = "circle-radius",
        color: Color = "circle-color",
        blur: f32 = "circle-blur",
        opacity: f32 = "circle-opacity",
        /// In pixels.
        translate: [f32; 2] = "circle-translate",
        translate_anchor: TranslateAnchor = "circle-translate-anchor",
        pitch_scale: CirclePitchScale = "circle-pitch-scale",
        pitch_alignment: CirclePitchAlignment = "circle-pitch-alignment",
        /// In pixels.
        stroke_width: f32 = "circle-stroke-width",
        stroke_color: Color = "circle-stroke-color",
        stroke_opacity: f32 = "circle-stroke-opacity",
    }
}

layer!(CircleLayer = "circle", CircleLayout, CirclePaint);

impl CirclePaint {
    /// The paint properties a circle shader reads, by their names in WGSL.
    pub fn attributes(&self) -> [(&'static str, &dyn PaintProperty); 7] {
        [
            ("color", &self.color),
            ("radius", &self.radius),
            ("blur", &self.blur),
            ("opacity", &self.opacity),
            ("stroke_color", &self.stroke_color),
            ("stroke_width", &self.stroke_width),
            ("stroke_opacity", &self.stroke_opacity),
        ]
    }
}
