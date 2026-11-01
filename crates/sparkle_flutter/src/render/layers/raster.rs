//! `raster` layers: an image per tile (or per image source) on a quad, color-adjusted in
//! the fragment shader as MapLibre does.
//!
//! A newly uploaded tile fades in over `raster-fade-duration`, cross-fading from the
//! nearest loaded ancestor's image when there is one, so zooming in doesn't pop.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use sparkle::style::{LayerKind, RasterPaint, Resampling, StyleLayer};
use sparkle::tile::RasterVertex;
use sparkle::{Bucket, BucketGeometry, TileId};

use super::{BucketData, DrawContext, LayerDraw, LayerRenderer};
use crate::gpu::{
    Buffer, Pipeline, PipelineSpec, Texture, Topology, VertexBufferLayout, VertexFormat,
};
use crate::render::{Gpu, LINEAR, NEAREST};

/// How many levels up to look for an image to fade from.
const MAX_FADE_LEVELS: u8 = 4;

pub struct RasterRenderer {
    pipeline: Pipeline,
    /// Uploaded images by `(style layer, tile)`, to cross-fade from.
    images: HashMap<(usize, TileId), Weak<Texture>>,
    /// When the last fade started at upload ends.
    fading_until: Option<Instant>,
}

struct GpuRaster {
    tile: TileId,
    vertices: Buffer,
    image: Arc<Texture>,
    uploaded: Instant,
}

impl RasterRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let layout = VertexBufferLayout::packed(&[
            ("position", VertexFormat::Float32x2),
            ("texcoord", VertexFormat::Float32x2),
        ]);
        debug_assert_eq!(layout.stride as usize, size_of::<RasterVertex>());
        let pipeline = gpu.pipeline(&PipelineSpec {
            program: "Raster",
            vertex_buffers: vec![layout],
            topology: Topology::TriangleStrip,
            ..Default::default()
        });
        Self {
            pipeline,
            images: HashMap::new(),
            fading_until: None,
        }
    }

    /// The nearest loaded ancestor of `tile` for `layer`, with the scale and offset from
    /// `tile`'s texture coordinates to its.
    fn ancestor(&self, layer: usize, tile: TileId) -> Option<(Arc<Texture>, [f32; 3])> {
        (1..=tile.z.min(MAX_FADE_LEVELS)).find_map(|k| {
            let id = TileId::new(tile.z - k, tile.x >> k, tile.y >> k);
            let image = self.images.get(&(layer, id))?.upgrade()?;
            let scale = 1.0 / f32::from(1u16 << k);
            let mask = (1u32 << k) - 1;
            let offset = [
                (tile.x & mask) as f32 * scale,
                (tile.y & mask) as f32 * scale,
            ];
            Some((image, [scale, offset[0], offset[1]]))
        })
    }
}

/// Whether `paint` samples nearest pixels; the newer `resampling` wins over
/// `raster-resampling` when either says so.
fn nearest(paint: &RasterPaint, zoom: f64) -> bool {
    paint.resampling.evaluate_at_zoom(zoom) == Resampling::Nearest
        || paint.raster_resampling.evaluate_at_zoom(zoom) == Resampling::Nearest
}

impl LayerRenderer for RasterRenderer {
    fn layer_type(&self) -> &'static str {
        "raster"
    }

    fn upload(&mut self, gpu: &Gpu, layer: &StyleLayer, bucket: &Bucket) -> Option<BucketData> {
        let (BucketGeometry::Raster(raster), LayerKind::Raster(style)) =
            (&bucket.geometry, &layer.kind)
        else {
            return None;
        };
        let image = &raster.image;
        let texture = Arc::new(gpu.rgba_texture(image.width, image.height, &image.pixels));
        self.images.retain(|_, t| t.strong_count() > 0);
        self.images
            .insert((bucket.style_layer, raster.tile), Arc::downgrade(&texture));

        let now = Instant::now();
        let fade = style
            .paint
            .fade_duration
            .evaluate_at_zoom(f64::from(raster.tile.z));
        if fade > 0.0 {
            let until = now + Duration::from_secs_f32(fade / 1000.0);
            self.fading_until = Some(self.fading_until.map_or(until, |t| t.max(until)));
        }
        Some(Box::new(GpuRaster {
            tile: raster.tile,
            vertices: gpu.buffer(bytemuck::cast_slice(&raster.vertices)),
            image: texture,
            uploaded: now,
        }))
    }

    fn animating(&self) -> bool {
        self.fading_until.is_some_and(|t| Instant::now() < t)
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::Raster(raster) = &layer.layer.kind else {
            return;
        };
        let paint = &raster.paint;
        let zoom = cx.view.camera.zoom;
        let opacity = paint.opacity.evaluate_at_zoom(zoom);
        let fade = paint.fade_duration.evaluate_at_zoom(zoom).max(0.0) / 1000.0;
        let hue_rotate = paint.hue_rotate.evaluate_at_zoom(zoom).to_radians();
        let (sin, cos) = hue_rotate.sin_cos();
        let saturation = paint.saturation.evaluate_at_zoom(zoom);
        let contrast = paint.contrast.evaluate_at_zoom(zoom);
        let sampler = if nearest(paint, zoom) {
            NEAREST
        } else {
            LINEAR
        };
        let now = Instant::now();
        for tile in &layer.tiles {
            let Some(bucket) = tile.bucket::<GpuRaster>() else {
                continue;
            };
            let mut u = cx.tile_uniforms(tile);
            // How far along the fade-in this tile is.
            let t = if fade > 0.0 {
                (now.duration_since(bucket.uploaded).as_secs_f32() / fade).min(1.0)
            } else {
                1.0
            };
            let mut tile_opacity = opacity;
            let mut parent_weight = 0.0;
            let mut parent_transform = [1.0, 0.0, 0.0];
            let fading_from = (t < 1.0)
                .then(|| self.ancestor(layer.index, bucket.tile))
                .flatten();
            match &fading_from {
                Some((_, transform)) => {
                    parent_weight = 1.0 - t;
                    parent_transform = *transform;
                }
                None => tile_opacity *= t,
            }
            u.extra[0] = [
                tile_opacity,
                parent_weight,
                paint.brightness_min.evaluate_at_zoom(zoom),
                paint.brightness_max.evaluate_at_zoom(zoom),
            ];
            u.extra[1] = [
                // MapLibre's saturation and contrast factors.
                if saturation > 0.0 {
                    1.0 - 1.0 / (1.001 - saturation)
                } else {
                    -saturation
                },
                if contrast > 0.0 {
                    1.0 / (1.0 - contrast)
                } else {
                    1.0 + contrast
                },
                0.0,
                0.0,
            ];
            let sqrt3 = 3f32.sqrt();
            u.extra[2] = [
                (2.0 * cos + 1.0) / 3.0,
                (-sqrt3 * sin - cos + 1.0) / 3.0,
                (sqrt3 * sin - cos + 1.0) / 3.0,
                0.0,
            ];
            u.extra[3] = [
                parent_transform[0],
                parent_transform[1],
                parent_transform[2],
                0.0,
            ];

            cx.pass.set_pipeline(&self.pipeline);
            cx.bind_uniforms(&u, Some(tile));
            cx.pass.set_texture("image", &bucket.image, sampler);
            // Without a fade the image stands in for the parent, at zero weight.
            let parent = fading_from.as_ref().map_or(&bucket.image, |(p, _)| p);
            cx.pass.set_texture("fade_from", parent, sampler);
            cx.pass.set_stencil_reference(tile.stencil_ref);
            cx.pass.set_vertex_buffer(0, bucket.vertices.slice());
            cx.pass.draw(4, 1);
        }
    }
}
