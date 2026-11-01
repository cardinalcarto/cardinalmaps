//! `raster`: images from a raster source.

use super::{layer, properties};
use crate::style::Resampling;

properties! {
    pub struct RasterLayout {}
}

properties! {
    pub struct RasterPaint {
        opacity: f32 = "raster-opacity",
        /// In degrees.
        hue_rotate: f32 = "raster-hue-rotate",
        brightness_min: f32 = "raster-brightness-min",
        brightness_max: f32 = "raster-brightness-max",
        saturation: f32 = "raster-saturation",
        contrast: f32 = "raster-contrast",
        /// The newer spelling of `raster-resampling`, which takes precedence when set.
        resampling: Resampling = "resampling",
        raster_resampling: Resampling = "raster-resampling",
        /// In milliseconds.
        fade_duration: f32 = "raster-fade-duration",
    }
}

layer!(RasterLayer = "raster", RasterLayout, RasterPaint);
