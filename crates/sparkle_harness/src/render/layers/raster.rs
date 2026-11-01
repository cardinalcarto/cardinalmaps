//! `raster` layers: an image per tile (or per image source) on a quad, color-adjusted in
//! the fragment shader as MapLibre does.
//!
//! A newly uploaded tile fades in over `raster-fade-duration`, cross-fading from the
//! nearest loaded ancestor's image when there is one, so zooming in doesn't pop.

use std::any::Any;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use sparkle::style::{LayerKind, RasterPaint, Resampling, StyleLayer};
use sparkle::tile::RasterVertex;
use sparkle::{Bucket, BucketGeometry, TileId};

use super::{DrawContext, LayerDraw, LayerRenderer};
use crate::render::gpu::{Gpu, PipelineSpec};

/// How many levels up to look for an image to fade from.
const MAX_FADE_LEVELS: u8 = 4;

pub struct RasterRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// Uploaded images by `(style layer, tile)`, to cross-fade from.
    images: HashMap<(usize, TileId), Weak<wgpu::TextureView>>,
    /// When the last fade started at upload ends.
    fading_until: Option<Instant>,
}

struct GpuRaster {
    tile: TileId,
    vertices: wgpu::Buffer,
    image: Rc<wgpu::TextureView>,
    /// The image alone, by whether sampling is linear.
    groups: [wgpu::BindGroup; 2],
    uploaded: Instant,
}

impl RasterRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let layout = gpu.texture_layout("raster", 2);
        let pipeline = gpu.create_pipeline(&PipelineSpec {
            label: "raster",
            shader: include_str!("../../shaders/raster.wgsl"),
            vertex_buffers: &[wgpu::VertexBufferLayout {
                array_stride: size_of::<RasterVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
            }],
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            bind_group_layouts: &[&layout],
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            images: HashMap::new(),
            fading_until: None,
        }
    }

    /// The nearest loaded ancestor of `tile` for `layer`, with the scale and offset from
    /// `tile`'s texture coordinates to its.
    fn ancestor(&self, layer: usize, tile: TileId) -> Option<(Rc<wgpu::TextureView>, [f32; 3])> {
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

    fn upload(&mut self, gpu: &Gpu, layer: &StyleLayer, bucket: &Bucket) -> Option<Box<dyn Any>> {
        let (BucketGeometry::Raster(raster), LayerKind::Raster(style)) =
            (&bucket.geometry, &layer.kind)
        else {
            return None;
        };
        let image = &raster.image;
        let texture = gpu.rgba_texture("raster", image.width, image.height, &image.pixels);
        let view = Rc::new(texture.create_view(&Default::default()));
        let groups = [&gpu.nearest_sampler, &gpu.linear_sampler]
            .map(|sampler| gpu.bind_textures(&self.layout, &[&view, &view], sampler));
        self.images.retain(|_, t| t.strong_count() > 0);
        self.images
            .insert((bucket.style_layer, raster.tile), Rc::downgrade(&view));

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
            vertices: gpu.buffer(
                "raster vertices",
                bytemuck::cast_slice(&raster.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            image: view,
            groups,
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
        let linear = !nearest(paint, zoom);
        let sampler = if linear {
            &cx.gpu.linear_sampler
        } else {
            &cx.gpu.nearest_sampler
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
            cx.bind_uniforms(&u);
            match &fading_from {
                Some((parent, _)) => {
                    let group =
                        cx.gpu
                            .bind_textures(&self.layout, &[&bucket.image, parent], sampler);
                    cx.pass.set_bind_group(1, &group, &[]);
                }
                None => cx
                    .pass
                    .set_bind_group(1, &bucket.groups[usize::from(linear)], &[]),
            }
            cx.pass.set_stencil_reference(tile.stencil_ref);
            cx.pass.set_vertex_buffer(0, bucket.vertices.slice(..));
            cx.pass.draw(0..4, 0..1);
        }
    }
}
