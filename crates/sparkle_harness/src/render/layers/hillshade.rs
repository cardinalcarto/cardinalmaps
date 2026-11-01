//! `hillshade` layers: each DEM tile's slopes, shaded per pixel with the layer's lights.
//! Slopes come precomputed from the library; borders are backfilled from neighbouring
//! tiles as they arrive (see [`super::dem`]).

use std::any::Any;
use std::rc::Rc;

use sparkle::style::{HillshadeMethod, LayerKind, Resampling, StyleLayer};
use sparkle::tile::{HillshadeLight, latitude_range};
use sparkle::{Bucket, BucketGeometry};

use super::dem::{DemTexture, DemTile, DemTiles};
use super::{DrawContext, LayerDraw, LayerRenderer};
use crate::render::gpu::{Gpu, PipelineSpec};

/// Lights beyond this many are ignored; the shader has room for this many.
const MAX_LIGHTS: usize = 6;

pub struct HillshadeRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    tiles: DemTiles,
}

struct GpuHillshade {
    dem: Rc<DemTile>,
    group: wgpu::BindGroup,
}

impl HillshadeRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let layout = gpu.texture_layout("hillshade", 1);
        let pipeline = gpu.create_pipeline(&PipelineSpec {
            label: "hillshade",
            shader: include_str!("../../shaders/hillshade.wgsl"),
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            bind_group_layouts: &[&layout],
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            tiles: DemTiles::default(),
        }
    }
}

fn method_index(method: HillshadeMethod) -> f32 {
    match method {
        HillshadeMethod::Standard => 0.0,
        HillshadeMethod::Combined => 1.0,
        HillshadeMethod::Igor => 2.0,
        HillshadeMethod::Multidirectional => 3.0,
        HillshadeMethod::Basic => 4.0,
    }
}

impl LayerRenderer for HillshadeRenderer {
    fn layer_type(&self) -> &'static str {
        "hillshade"
    }

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<Box<dyn Any>> {
        let BucketGeometry::Hillshade(hillshade) = &bucket.geometry else {
            return None;
        };
        let dem = Rc::new(DemTile::new(
            gpu,
            hillshade.tile,
            (*hillshade.dem).clone(),
            DemTexture::Slopes,
            Some(&hillshade.slopes.pixels),
        ));
        self.tiles.insert(gpu, bucket.style_layer, &dem);
        let view = dem.texture.create_view(&Default::default());
        // The shader interpolates itself, so the sampler is unused.
        let group = gpu.bind_textures(&self.layout, &[&view], &gpu.nearest_sampler);
        Some(Box::new(GpuHillshade { dem, group }))
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::Hillshade(hillshade) = &layer.layer.kind else {
            return;
        };
        let paint = &hillshade.paint;
        let camera = &cx.view.camera;
        let zoom = camera.zoom;
        let light = HillshadeLight::evaluate(paint, zoom, camera.bearing);
        let nearest = paint.resampling.evaluate_at_zoom(zoom) == Resampling::Nearest;
        let sources = &light.sources[..light.sources.len().min(MAX_LIGHTS)];

        let mut template = cx.view.uniforms();
        template.extra[0] = [
            light.exaggeration,
            method_index(light.method),
            sources.len() as f32,
            0.0,
        ];
        template.extra[2] = light.accent.0;
        for (i, s) in sources.iter().enumerate() {
            template.extra[3 + i / 4][i % 4] = s.azimuth;
            template.extra[5 + i / 4][i % 4] = s.altitude;
            template.paint.values[i] = s.shadow.0;
            template.paint.values[MAX_LIGHTS + i] = s.highlight.0;
        }

        for tile in &layer.tiles {
            let Some(bucket) = tile.bucket::<GpuHillshade>() else {
                continue;
            };
            let mut u = cx.tile_uniforms(tile);
            u.extra = template.extra;
            u.paint = template.paint;
            u.extra[0][3] = tile.extent as f32;
            let [top, bottom] = latitude_range(bucket.dem.tile);
            u.extra[1] = [
                top.to_radians() as f32,
                bottom.to_radians() as f32,
                if nearest { 1.0 } else { 0.0 },
                bucket.dem.dim() as f32,
            ];
            cx.pass.set_pipeline(&self.pipeline);
            cx.bind_uniforms(&u);
            cx.pass.set_bind_group(1, &bucket.group, &[]);
            cx.pass.set_stencil_reference(tile.stencil_ref);
            cx.pass.draw(0..4, 0..1);
        }
    }
}
