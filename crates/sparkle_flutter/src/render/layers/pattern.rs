//! What the fill, line and background renderers share for drawing images: resolving a
//! cross-faded `*-pattern` against the sprite atlas.

use sparkle::pattern::CrossFade;
use sparkle::sprite::AtlasImage;
use sparkle::style::{PropertyValue, ResolvedImage};

use crate::render::ImageTexture;

/// One image of a cross-faded pattern, for shaders.
#[derive(Debug, Clone, Copy)]
pub struct PatternImage {
    /// `[x, y, width, height]` in atlas pixels.
    pub rect: [f32; 4],
    /// The size of one repeat in logical pixels, cross-fade scale applied.
    pub size: [f32; 2],
}

/// A layer-wide `*-pattern` at the camera's zoom: the images to fade between, or `None`
/// when either is missing from the atlas (or there are no sprites), and the layer can't be
/// drawn.
///
/// `world` patterns are laid out in pixels of the fade's layout zoom, so their sizes get
/// the fade's scales; line patterns are sized by the line instead.
pub fn crossfaded_pattern(
    images: &ImageTexture,
    pattern: &PropertyValue<Option<ResolvedImage>>,
    fade: &CrossFade,
    world: bool,
) -> Option<[PatternImage; 2]> {
    let image = |zoom: f64, scale: f32| {
        let name = pattern.evaluate_at_zoom(zoom)?;
        let scale = if world { scale } else { 1.0 };
        Some(pattern_image(images.atlas.resolve(&name)?, scale))
    };
    Some([
        image(fade.from_zoom, fade.from_scale)?,
        image(fade.to_zoom, fade.to_scale)?,
    ])
}

fn pattern_image(image: &AtlasImage, scale: f32) -> PatternImage {
    let [w, h] = image.logical_size();
    PatternImage {
        rect: image.rect.map(|v| v as f32),
        size: [w * scale, h * scale],
    }
}
