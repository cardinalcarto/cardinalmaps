//! `heatmap` layers, in two passes as in MapLibre. Before the main pass, every point's
//! kernel is added up in an offscreen density texture at a quarter of the viewport's
//! resolution; then, in the layer's place among the others, a full-screen quad looks each
//! pixel's density up in a `heatmap-color` ramp texture.
//!
//! The density pass isn't clipped to tiles (kernels are meant to spill over tile edges, and
//! the library drops points outside their tile), so each tile placement is drawn once even
//! when several clip areas share it.

use std::collections::HashMap;

use sparkle::style::{HeatmapPaint, LayerKind, StyleLayer};
use sparkle::{Bucket, BucketGeometry};
use sparkle_view::camera::Placement;

use super::{BucketData, DrawContext, LayerDraw, LayerRenderer, PrepareContext, bind_uniforms};
use crate::gpu::{
    Blend, Buffer, Load, PassDesc, Pipeline, PipelineSpec, Store, Texture, TextureFormat, Topology,
    VertexBufferLayout, VertexFormat,
};
use crate::render::paint::{GpuPaint, UniformsMemo};
use crate::render::{Gpu, LINEAR};

/// The density texture is this many times smaller than the viewport in each direction:
/// the kernels are smooth, so the lower resolution doesn't show and the pass is cheap.
const DENSITY_SCALE: u32 = 4;

pub struct HeatmapRenderer {
    /// Half float where it can be rendered to and blended; 8 bits otherwise.
    density_format: TextureFormat,
    /// By paint signature.
    density_pipelines: HashMap<u32, Pipeline>,
    color_pipeline: Pipeline,
    /// By style layer index.
    targets: HashMap<usize, LayerTarget>,
}

/// One layer's offscreen density and its color ramp.
struct LayerTarget {
    size: [u32; 2],
    density: Texture,
    ramp: Texture,
    /// Whether the density was drawn this frame, so the color pass has something to show.
    ready: bool,
}

struct GpuHeatmap {
    vertices: Buffer,
    indices: Buffer,
    index_count: u32,
    paint: GpuPaint,
}

impl HeatmapRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let color_pipeline = gpu.pipeline(&PipelineSpec {
            program: "HeatmapColor",
            vertex_buffers: vec![Gpu::quad_layout()],
            topology: Topology::TriangleStrip,
            clip: false,
            ..Default::default()
        });
        Self {
            density_format: density_format(gpu),
            density_pipelines: HashMap::new(),
            color_pipeline,
            targets: HashMap::new(),
        }
    }

    fn density_pipeline(&mut self, gpu: &Gpu, paint: &GpuPaint) -> Pipeline {
        *self
            .density_pipelines
            .entry(paint.signature())
            .or_insert_with(|| {
                let mut vertex_buffers = vec![VertexBufferLayout::packed(&[
                    ("pos", VertexFormat::Float32x2),
                    ("corner", VertexFormat::Float32x2),
                ])];
                vertex_buffers.extend(paint.vertex_buffers());
                gpu.pipeline(&PipelineSpec {
                    program: "Heatmap",
                    vertex_buffers,
                    clip: false,
                    blend: Blend::Add,
                    ..Default::default()
                })
            })
    }

    /// The layer's target, (re)made for a density texture of `size`.
    fn target(
        &mut self,
        gpu: &Gpu,
        index: usize,
        paint: &HeatmapPaint,
        size: [u32; 2],
    ) -> &mut LayerTarget {
        let stale = self.targets.get(&index).is_none_or(|t| t.size != size);
        if stale {
            // Keep the ramp of a target that only changes size.
            let ramp = match self.targets.remove(&index) {
                Some(t) => t.ramp,
                None => {
                    let ramp = paint.color_ramp();
                    gpu.rgba_texture(ramp.len() as u32, 1, bytemuck::cast_slice(&ramp))
                }
            };
            let density = gpu.target(size, self.density_format, 1);
            self.targets.insert(
                index,
                LayerTarget {
                    size,
                    density,
                    ramp,
                    ready: false,
                },
            );
        }
        self.targets.get_mut(&index).expect("inserted above")
    }
}

/// The density texture's format: half float as in MapLibre, so dense areas don't saturate
/// and sparse ones don't round away; 8-bit where half float targets can't be blended.
/// (Flutter GPU has no single channel half float format.)
fn density_format(gpu: &Gpu) -> TextureFormat {
    if gpu.info.half_float {
        TextureFormat::Rgba16Float
    } else {
        TextureFormat::Rgba8
    }
}

impl LayerRenderer for HeatmapRenderer {
    fn layer_type(&self) -> &'static str {
        "heatmap"
    }

    fn style_changed(&mut self) {
        self.targets.clear();
    }

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<BucketData> {
        let BucketGeometry::Heatmap(heat) = &bucket.geometry else {
            return None;
        };
        if heat.indices.is_empty() {
            return None;
        }
        Some(Box::new(GpuHeatmap {
            vertices: gpu.buffer(bytemuck::cast_slice(&heat.vertices)),
            indices: gpu.buffer(bytemuck::cast_slice(&heat.indices)),
            index_count: heat.indices.len() as u32,
            paint: GpuPaint::new(gpu, &heat.paint),
        }))
    }

    fn prepare(&mut self, cx: &mut PrepareContext<'_>, layer: &LayerDraw<'_>) {
        let LayerKind::Heatmap(heat) = &layer.layer.kind else {
            return;
        };
        let paint = &heat.paint;
        let zoom = cx.view.camera.zoom;
        if let Some(target) = self.targets.get_mut(&layer.index) {
            target.ready = false;
        }
        if paint.opacity.evaluate_at_zoom(zoom) <= 0.0 {
            return;
        }
        let [width, height] = cx.view.camera.viewport;
        let size = [width, height].map(|v| (v as u32).div_ceil(DENSITY_SCALE).max(1));
        let target = self.target(cx.gpu, layer.index, paint, size);
        target.ready = true;
        let density = target.density.clone();

        let gpu = cx.gpu;
        let mut pass = gpu.begin_pass(&PassDesc {
            color: &density,
            resolve: None,
            color_load: Load::Clear([0.0; 4]),
            color_store: Store::Store,
            depth_stencil: None,
        });
        let props = paint.attributes();
        let mut memo = UniformsMemo::default();
        let intensity = paint.intensity.evaluate_at_zoom(zoom);
        let mut drawn: Vec<Placement> = Vec::new();
        for tile in &layer.tiles {
            let Some(bucket) = tile.bucket::<GpuHeatmap>() else {
                continue;
            };
            if drawn.contains(&tile.placement) {
                continue;
            }
            drawn.push(tile.placement);
            let mut u = cx.view.tile_uniforms(tile.matrix, &tile.geo);
            u.units_per_pixel = tile.units_per_pixel;
            u.extent = tile.extent as f32;
            u.paint = memo.uniforms(&bucket.paint.layout, &props, zoom);
            u.extra[0][0] = intensity;

            let pipeline = self.density_pipeline(gpu, &bucket.paint);
            pass.set_pipeline(&pipeline);
            bind_uniforms(gpu, &mut pass, &u, tile.geo.dem_texture.as_ref());
            pass.set_vertex_buffer(0, bucket.vertices.slice());
            bucket.paint.bind(gpu, &mut pass, 1);
            pass.set_index_buffer(bucket.indices.slice(), false);
            pass.draw_indexed(bucket.index_count, 1);
        }
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::Heatmap(heat) = &layer.layer.kind else {
            return;
        };
        let Some(target) = self.targets.get(&layer.index).filter(|t| t.ready) else {
            return;
        };
        let mut u = cx.view.uniforms();
        u.extra[0][0] = heat.paint.opacity.evaluate_at_zoom(cx.view.camera.zoom);
        u.extra[0][1] = target.ramp.width as f32;
        cx.pass.set_pipeline(&self.color_pipeline);
        cx.bind_uniforms(&u, None);
        cx.pass.set_vertex_buffer(0, cx.gpu.quad.slice());
        cx.pass.set_texture("density", &target.density, LINEAR);
        cx.pass.set_texture("ramp", &target.ramp, LINEAR);
        cx.pass.draw(4, 1);
    }
}
