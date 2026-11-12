//! `fill-extrusion`: polygons extruded into 3D prisms.

use super::{layer, properties};
use crate::paint::PaintProperty;
use crate::style::{Color, ResolvedImage, TranslateAnchor};

properties! {
    pub struct FillExtrusionLayout {}
}

properties! {
    pub struct FillExtrusionPaint {
        opacity: f32 = "fill-extrusion-opacity",
        color: Color = "fill-extrusion-color",
        /// In pixels.
        translate: [f32; 2] = "fill-extrusion-translate",
        translate_anchor: TranslateAnchor = "fill-extrusion-translate-anchor",
        pattern: Option<ResolvedImage> = "fill-extrusion-pattern",
        /// In meters.
        height: f32 = "fill-extrusion-height",
        /// In meters.
        base: f32 = "fill-extrusion-base",
        vertical_gradient: bool = "fill-extrusion-vertical-gradient",
    }
}

layer!(
    FillExtrusionLayer = "fill-extrusion",
    FillExtrusionLayout,
    FillExtrusionPaint
);

impl FillExtrusionPaint {
    /// The paint properties the extrusion shader reads per vertex, by their names in WGSL.
    /// `opacity`, `translate` and `vertical-gradient` can't depend on the feature, so they
    /// are passed per layer instead.
    pub fn attributes(&self) -> [(&'static str, &dyn PaintProperty); 3] {
        [
            ("color", &self.color),
            ("height", &self.height),
            ("base", &self.base),
        ]
    }

    /// [`Self::attributes`] that roofs read: all but `base`, at the same indices.
    pub fn roof_attributes(&self) -> [(&'static str, &dyn PaintProperty); 2] {
        [("color", &self.color), ("height", &self.height)]
    }
}
