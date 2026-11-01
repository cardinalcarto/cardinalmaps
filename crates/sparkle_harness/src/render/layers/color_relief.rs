//! `color-relief` layers: each DEM tile's elevations looked up in a color ramp baked once
//! per layer. Borders are backfilled from neighbouring tiles (see [`super::dem`]) so
//! interpolated elevations meet at tile edges.

use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

use sparkle::style::{LayerKind, Resampling, StyleLayer};
use sparkle::tile::ColorRamp;
use sparkle::{Bucket, BucketGeometry};

use super::dem::{DemTexture, DemTile, DemTiles};
use super::{DrawContext, LayerDraw, LayerRenderer};
use crate::render::gpu::{Gpu, PipelineSpec};

pub struct ColorReliefRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    tiles: DemTiles,
    /// Per style layer.
    ramps: HashMap<usize, Ramp>,
}

struct Ramp {
    view: wgpu::TextureView,
    min: f32,
    max: f32,
    size: u32,
}

struct GpuColorRelief {
    dem: Rc<DemTile>,
    group: wgpu::BindGroup,
    min: f32,
    max: f32,
    ramp_size: u32,
}

impl ColorReliefRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let layout = gpu.texture_layout("color relief", 2);
        let pipeline = gpu.create_pipeline(&PipelineSpec {
            label: "color relief",
            shader: include_str!("../../shaders/color_relief.wgsl"),
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            bind_group_layouts: &[&layout],
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            tiles: DemTiles::default(),
            ramps: HashMap::new(),
        }
    }
}

impl LayerRenderer for ColorReliefRenderer {
    fn layer_type(&self) -> &'static str {
        "color-relief"
    }

    fn style_changed(&mut self) {
        self.ramps.clear();
    }

    fn upload(&mut self, gpu: &Gpu, layer: &StyleLayer, bucket: &Bucket) -> Option<Box<dyn Any>> {
        let (BucketGeometry::ColorRelief(relief), LayerKind::ColorRelief(style)) =
            (&bucket.geometry, &layer.kind)
        else {
            return None;
        };
        let ramp = self.ramps.entry(bucket.style_layer).or_insert_with(|| {
            let ramp = ColorRamp::bake(&style.paint.color);
            let size = ramp.colors.len() as u32;
            let texture = gpu.rgba_texture(
                "color relief ramp",
                size,
                1,
                bytemuck::cast_slice(&ramp.colors),
            );
            Ramp {
                view: texture.create_view(&Default::default()),
                min: ramp.min,
                max: ramp.max,
                size,
            }
        });
        let dem = Rc::new(DemTile::new(
            gpu,
            relief.tile,
            (*relief.dem).clone(),
            DemTexture::Elevations,
            None,
        ));
        self.tiles.insert(gpu, bucket.style_layer, &dem);
        let view = dem.texture.create_view(&Default::default());
        let group = gpu.bind_textures(&self.layout, &[&view, &ramp.view], &gpu.linear_sampler);
        Some(Box::new(GpuColorRelief {
            dem,
            group,
            min: ramp.min,
            max: ramp.max,
            ramp_size: ramp.size,
        }))
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::ColorRelief(relief) = &layer.layer.kind else {
            return;
        };
        let zoom = cx.view.camera.zoom;
        let opacity = relief.paint.opacity.evaluate_at_zoom(zoom);
        let nearest = relief.paint.resampling.evaluate_at_zoom(zoom) == Resampling::Nearest;
        for tile in &layer.tiles {
            let Some(bucket) = tile.bucket::<GpuColorRelief>() else {
                continue;
            };
            let mut u = cx.tile_uniforms(tile);
            u.extra[0] = [bucket.dem.dim() as f32, bucket.min, bucket.max, opacity];
            u.extra[1] = [
                tile.extent as f32,
                if nearest { 1.0 } else { 0.0 },
                bucket.ramp_size as f32,
                0.0,
            ];
            cx.pass.set_pipeline(&self.pipeline);
            cx.bind_uniforms(&u);
            cx.pass.set_bind_group(1, &bucket.group, &[]);
            cx.pass.set_stencil_reference(tile.stencil_ref);
            cx.pass.draw(0..4, 0..1);
        }
    }
}
