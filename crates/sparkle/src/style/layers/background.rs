//! `background`: covers the whole map. It has no source, so it never appears in a tile's
//! buckets; the renderer draws it directly.

use super::{layer, properties};
use crate::style::{Color, ResolvedImage};

properties! {
    pub struct BackgroundLayout {}
}

properties! {
    pub struct BackgroundPaint {
        color: Color = "background-color",
        /// Tiles the image instead of the color.
        pattern: Option<ResolvedImage> = "background-pattern",
        opacity: f32 = "background-opacity",
    }
}

layer!(
    BackgroundLayer = "background",
    BackgroundLayout,
    BackgroundPaint
);
