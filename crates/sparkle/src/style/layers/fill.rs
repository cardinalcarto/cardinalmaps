//! `fill`: filled polygons, optionally with an outline.

use super::{layer, properties};
use crate::paint::PaintProperty;
use crate::style::{Color, ResolvedImage, TranslateAnchor};

properties! {
    pub struct FillLayout {
        sort_key: Option<f32> = "fill-sort-key",
    }
}

properties! {
    pub struct FillPaint {
        antialias: bool = "fill-antialias",
        opacity: f32 = "fill-opacity",
        color: Color = "fill-color",
        /// `None` when unset, in which case the outline uses [`Self::color`].
        outline_color: Option<Color> = "fill-outline-color",
        /// In pixels.
        translate: [f32; 2] = "fill-translate",
        translate_anchor: TranslateAnchor = "fill-translate-anchor",
        pattern: Option<ResolvedImage> = "fill-pattern",
        layer_opacity: f32 = "fill-layer-opacity",
    }
}

layer!(FillLayer = "fill", FillLayout, FillPaint);

impl FillPaint {
    /// The paint properties a fill shader reads, by their names in WGSL.
    pub fn attributes(&self) -> [(&'static str, &dyn PaintProperty); 3] {
        [
            ("color", &self.color),
            ("opacity", &self.opacity),
            ("outline_color", &self.outline_color),
        ]
    }
}
