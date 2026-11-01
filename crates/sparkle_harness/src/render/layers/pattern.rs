//! What the fill, line and background renderers share for drawing images: one texture
//! bind group layout, and resolving a cross-faded `*-pattern` against the sprite atlas.

use sparkle::pattern::CrossFade;
use sparkle::sprite::AtlasImage;
use sparkle::style::{PropertyValue, ResolvedImage};

use crate::render::gpu::{Gpu, ImageTexture};

/// Group 1 of the image-drawing pipelines: a texture at binding 0 and its sampler at 1.
pub struct TextureGroup {
    pub layout: wgpu::BindGroupLayout,
    /// A transparent texel, for draws that don't sample anything.
    pub empty: wgpu::BindGroup,
}

impl TextureGroup {
    pub fn new(gpu: &Gpu) -> Self {
        let layout = gpu.texture_layout("texture", 1);
        let texel = gpu.rgba_texture("empty", 1, 1, &[0; 4]);
        let view = texel.create_view(&Default::default());
        let empty = Self::bind_with(gpu, &layout, &view, &gpu.nearest_sampler);
        Self { layout, empty }
    }

    pub fn bind(
        &self,
        gpu: &Gpu,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        Self::bind_with(gpu, &self.layout, view, sampler)
    }

    fn bind_with(
        gpu: &Gpu,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        gpu.bind_textures(layout, &[view], sampler)
    }
}

/// The sprite atlas as a bind group, made the first time a pattern is drawn.
#[derive(Default)]
pub struct AtlasGroup(Option<wgpu::BindGroup>);

impl AtlasGroup {
    pub fn get(&mut self, gpu: &Gpu, group: &TextureGroup) -> Option<&wgpu::BindGroup> {
        let images = gpu.images.as_ref()?;
        Some(
            self.0
                .get_or_insert_with(|| group.bind(gpu, &images.view, &gpu.linear_sampler)),
        )
    }
}

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
