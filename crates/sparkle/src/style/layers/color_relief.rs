//! `color-relief`: a `raster-dem` source colored by elevation.

use super::{layer, properties};
use crate::style::{Color, Resampling};

properties! {
    pub struct ColorReliefLayout {}
}

properties! {
    pub struct ColorReliefPaint {
        opacity: f32 = "color-relief-opacity",
        /// Reads `["elevation"]`, in meters.
        color: Option<Color> = "color-relief-color",
        resampling: Resampling = "resampling",
    }
}

layer!(
    ColorReliefLayer = "color-relief",
    ColorReliefLayout,
    ColorReliefPaint
);
