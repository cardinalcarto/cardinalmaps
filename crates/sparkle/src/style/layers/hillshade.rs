//! `hillshade`: shading computed from a `raster-dem` source.

use super::{layer, properties};
use crate::style::{
    Color, ColorArray, HillshadeMethod, IlluminationAnchor, NumberArray, Resampling,
};

properties! {
    pub struct HillshadeLayout {}
}

properties! {
    pub struct HillshadePaint {
        /// In degrees, one per light for `multidirectional`.
        illumination_direction: NumberArray = "hillshade-illumination-direction",
        /// In degrees, one per light for `multidirectional`.
        illumination_altitude: NumberArray = "hillshade-illumination-altitude",
        illumination_anchor: IlluminationAnchor = "hillshade-illumination-anchor",
        exaggeration: f32 = "hillshade-exaggeration",
        shadow_color: ColorArray = "hillshade-shadow-color",
        highlight_color: ColorArray = "hillshade-highlight-color",
        accent_color: Color = "hillshade-accent-color",
        method: HillshadeMethod = "hillshade-method",
        resampling: Resampling = "resampling",
    }
}

layer!(
    HillshadeLayer = "hillshade",
    HillshadeLayout,
    HillshadePaint
);
