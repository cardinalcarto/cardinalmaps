//! `background` layers: a full-screen color drawn in style order like any other layer, or
//! a pattern drawn over the tiles that cover the view.

use std::any::Any;

use sparkle::Bucket;
use sparkle::pattern::{CrossFade, pattern_placement};
use sparkle::style::{LayerKind, StyleLayer};

use super::pattern::{AtlasGroup, TextureGroup, crossfaded_pattern};
use super::{DrawContext, LayerDraw, LayerRenderer};
use crate::camera::TILE_SIZE;
use crate::render::gpu::{Gpu, PipelineSpec};

/// The deepest tiles a background pattern is placed with; patterns are laid out at the
/// camera's zoom whatever the tiles' zoom, so this only bounds how finely they're split.
const MAX_PATTERN_TILE_ZOOM: u8 = 24;

pub struct BackgroundRenderer {
    pipeline: wgpu::RenderPipeline,
    textures: TextureGroup,
    atlas: AtlasGroup,
}

impl BackgroundRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let textures = TextureGroup::new(gpu);
        Self {
            pipeline: gpu.create_pipeline(&PipelineSpec {
                label: "background",
                shader: include_str!("../../shaders/background.wgsl"),
                clip: false,
                bind_group_layouts: &[&textures.layout],
                ..Default::default()
            }),
            textures,
            atlas: AtlasGroup::default(),
        }
    }
}

impl LayerRenderer for BackgroundRenderer {
    fn layer_type(&self) -> &'static str {
        "background"
    }

    fn upload(&mut self, _: &Gpu, _: &StyleLayer, _: &Bucket) -> Option<Box<dyn Any>> {
        None
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::Background(bg) = &layer.layer.kind else {
            return;
        };
        let paint = &bg.paint;
        let zoom = cx.view.camera.zoom;
        let opacity = paint.opacity.evaluate_at_zoom(zoom);
        cx.pass.set_pipeline(&self.pipeline);

        if matches!(paint.pattern.as_constant(), Some(None)) {
            let mut u = cx.view.uniforms();
            u.extra[0] = paint.color.evaluate_at_zoom(zoom).0.map(|c| c * opacity);
            cx.bind_uniforms(&u);
            cx.pass.set_bind_group(1, &self.textures.empty, &[]);
            cx.pass.draw(0..3, 0..1);
            return;
        }

        // A pattern whose image is missing isn't drawn, as in MapLibre.
        let fade = CrossFade::at(zoom);
        let Some(images) = &cx.gpu.images else {
            return;
        };
        let Some([a, b]) = crossfaded_pattern(images, &paint.pattern, &fade, true) else {
            return;
        };
        let Some(group) = self.atlas.get(cx.gpu, &self.textures) else {
            return;
        };
        let camera = &cx.view.camera;
        let mut u = cx.view.uniforms();
        u.extra[1] = [1.0, fade.t, opacity, 0.0];
        u.extra[2] = a.rect;
        u.extra[3] = b.rect;
        u.extra[7] = [images.size[0] as f32, images.size[1] as f32, 0.0, 0.0];
        cx.pass.set_bind_group(1, group, &[]);
        for tile in camera.covering_tiles(0, MAX_PATTERN_TILE_ZOOM, TILE_SIZE as u32) {
            u.matrix = camera.tile_matrix(&cx.view.view_projection, tile, 1);
            for (i, image) in [(4, &a), (5, &b)] {
                let (origin, per_unit) = pattern_placement(
                    (tile.z, tile.x, tile.y),
                    1,
                    TILE_SIZE,
                    fade.layout_zoom,
                    image.size,
                );
                u.extra[i] = [origin[0], origin[1], per_unit[0], per_unit[1]];
            }
            cx.bind_uniforms(&u);
            cx.pass.draw(0..6, 0..1);
        }
    }
}
