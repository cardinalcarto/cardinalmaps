//! `color-relief` layers: each DEM tile's elevations looked up in a color ramp baked once
//! per layer. Borders are backfilled from neighbouring tiles (see [`super::dem`]) so
//! interpolated elevations meet at tile edges.

use std::collections::HashMap;
use std::sync::Arc;

use sparkle::style::{LayerKind, Resampling, StyleLayer};
use sparkle::tile::ColorRamp;
use sparkle::{Bucket, BucketGeometry};

use super::dem::{DemTexture, DemTile, DemTiles};
use super::{BucketData, DrawContext, LayerDraw, LayerRenderer};
use crate::gpu::{Pipeline, PipelineSpec, Texture, Topology};
use crate::render::{Gpu, LINEAR, NEAREST};

pub struct ColorReliefRenderer {
    pipeline: Pipeline,
    tiles: DemTiles,
    /// Per style layer.
    ramps: HashMap<usize, Ramp>,
}

struct Ramp {
    texture: Texture,
    min: f32,
    max: f32,
    size: u32,
}

struct GpuColorRelief {
    dem: Arc<DemTile>,
    ramp: Texture,
    min: f32,
    max: f32,
    ramp_size: u32,
}

impl ColorReliefRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let pipeline = gpu.pipeline(&PipelineSpec {
            program: "ColorRelief",
            vertex_buffers: vec![Gpu::quad_layout()],
            topology: Topology::TriangleStrip,
            ..Default::default()
        });
        Self {
            pipeline,
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

    fn upload(&mut self, gpu: &Gpu, layer: &StyleLayer, bucket: &Bucket) -> Option<BucketData> {
        let (BucketGeometry::ColorRelief(relief), LayerKind::ColorRelief(style)) =
            (&bucket.geometry, &layer.kind)
        else {
            return None;
        };
        let ramp = self.ramps.entry(bucket.style_layer).or_insert_with(|| {
            let ramp = ColorRamp::bake(&style.paint.color);
            let size = ramp.colors.len() as u32;
            Ramp {
                texture: gpu.rgba_texture(size, 1, bytemuck::cast_slice(&ramp.colors)),
                min: ramp.min,
                max: ramp.max,
                size,
            }
        });
        let dem = Arc::new(DemTile::new(
            gpu,
            relief.tile,
            (*relief.dem).clone(),
            DemTexture::Elevations,
            None,
        ));
        self.tiles.insert(gpu, bucket.style_layer, &dem);
        Some(Box::new(GpuColorRelief {
            dem,
            ramp: ramp.texture.clone(),
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
            cx.bind_uniforms(&u, Some(tile));
            // The shader interpolates elevations itself, reading pixel centers.
            cx.pass
                .set_texture("elevations", &bucket.dem.texture, NEAREST);
            cx.pass.set_texture("ramp", &bucket.ramp, LINEAR);
            cx.pass.set_stencil_reference(tile.stencil_ref);
            cx.pass.set_vertex_buffer(0, cx.gpu.quad.slice());
            cx.pass.draw(4, 1);
        }
    }
}
