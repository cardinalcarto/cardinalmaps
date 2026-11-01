//! `heatmap` layers, in two passes as in MapLibre. Before the main pass, every point's
//! kernel is added up in an offscreen density texture at a quarter of the viewport's
//! resolution; then, in the layer's place among the others, a full-screen triangle looks
//! each pixel's density up in a `heatmap-color` ramp texture.
//!
//! The density pass isn't clipped to tiles (kernels are meant to spill over tile edges, and
//! the library drops points outside their tile), so each tile placement is drawn once even
//! when several clip areas share it.

use std::any::Any;
use std::collections::HashMap;

use sparkle::paint::PaintArrays;
use sparkle::style::{HeatmapPaint, LayerKind, StyleLayer};
use sparkle::{Bucket, BucketGeometry};

use super::{DrawContext, LayerDraw, LayerRenderer, PrepareContext};
use crate::camera::Placement;
use crate::render::gpu::{Gpu, PipelineSpec};

/// The density texture is this many times smaller than the viewport in each direction:
/// the kernels are smooth, so the lower resolution doesn't show and the pass is cheap.
const DENSITY_SCALE: u32 = 4;

pub struct HeatmapRenderer {
    /// Half float where it can be rendered to, blended and filtered; 8 bits otherwise.
    density_format: wgpu::TextureFormat,
    /// By paint signature.
    density_pipelines: HashMap<u32, wgpu::RenderPipeline>,
    color_pipeline: wgpu::RenderPipeline,
    color_layout: wgpu::BindGroupLayout,
    /// By style layer index.
    targets: HashMap<usize, LayerTarget>,
}

/// One layer's offscreen density and its color ramp.
struct LayerTarget {
    size: [u32; 2],
    density: wgpu::TextureView,
    ramp: wgpu::TextureView,
    group: wgpu::BindGroup,
    /// Whether the density was drawn this frame, so the color pass has something to show.
    ready: bool,
}

struct GpuHeatmap {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    paint_buffer: Option<wgpu::Buffer>,
    /// The layout of `paint_buffer`; its data isn't kept.
    paint: PaintArrays,
}

impl HeatmapRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let color_layout = gpu.texture_layout("heatmap color", 2);
        let color_pipeline = gpu.create_pipeline(&PipelineSpec {
            label: "heatmap color",
            shader: include_str!("../../shaders/heatmap_color.wgsl"),
            clip: false,
            bind_group_layouts: &[&color_layout],
            ..Default::default()
        });
        Self {
            density_format: density_format(gpu),
            density_pipelines: HashMap::new(),
            color_pipeline,
            color_layout,
            targets: HashMap::new(),
        }
    }

    fn density_pipeline(&mut self, gpu: &Gpu, paint: &PaintArrays) -> &wgpu::RenderPipeline {
        let format = self.density_format;
        self.density_pipelines
            .entry(paint.signature())
            .or_insert_with(|| {
                let add = wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                };
                gpu.create_pipeline(&PipelineSpec {
                    label: "heatmap density",
                    shader: include_str!("../../shaders/heatmap.wgsl"),
                    vertex_buffers: &[wgpu::VertexBufferLayout {
                        array_stride: size_of::<sparkle::tile::HeatmapVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    }],
                    paint: Some(paint),
                    clip: false,
                    blend: Some(wgpu::BlendState {
                        color: add,
                        alpha: add,
                    }),
                    target_format: Some(format),
                    sample_count: Some(1),
                    no_depth_stencil: true,
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
                    gpu.rgba_texture(
                        "heatmap ramp",
                        ramp.len() as u32,
                        1,
                        bytemuck::cast_slice(&ramp),
                    )
                    .create_view(&Default::default())
                }
            };
            let density = gpu
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("heatmap density"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.density_format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            let group =
                gpu.bind_textures(&self.color_layout, &[&density, &ramp], &gpu.linear_sampler);
            self.targets.insert(
                index,
                LayerTarget {
                    size,
                    density,
                    ramp,
                    group,
                    ready: false,
                },
            );
        }
        self.targets.get_mut(&index).expect("inserted above")
    }
}

/// The density texture's format: half float as in MapLibre, so dense areas don't saturate
/// and sparse ones don't round away; 8-bit where half float targets can't be blended or
/// filtered.
fn density_format(gpu: &Gpu) -> wgpu::TextureFormat {
    use wgpu::{TextureFormat, TextureFormatFeatureFlags as Flags, TextureUsages as Usages};
    let usable = |format: &TextureFormat| {
        let features = format.guaranteed_format_features(gpu.device.features());
        features
            .allowed_usages
            .contains(Usages::RENDER_ATTACHMENT | Usages::TEXTURE_BINDING)
            && features
                .flags
                .contains(Flags::BLENDABLE | Flags::FILTERABLE)
    };
    [TextureFormat::R16Float, TextureFormat::Rgba16Float]
        .into_iter()
        .find(usable)
        .unwrap_or(TextureFormat::Rgba8Unorm)
}

impl LayerRenderer for HeatmapRenderer {
    fn layer_type(&self) -> &'static str {
        "heatmap"
    }

    fn style_changed(&mut self) {
        self.targets.clear();
    }

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<Box<dyn Any>> {
        let BucketGeometry::Heatmap(heat) = &bucket.geometry else {
            return None;
        };
        let paint = heat.paint.layout();
        Some(Box::new(GpuHeatmap {
            vertices: gpu.buffer(
                "heatmap vertices",
                bytemuck::cast_slice(&heat.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            indices: gpu.buffer(
                "heatmap indices",
                bytemuck::cast_slice(&heat.indices),
                wgpu::BufferUsages::INDEX,
            ),
            index_count: heat.indices.len() as u32,
            paint_buffer: gpu.paint_buffer(&heat.paint),
            paint,
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

        let mut pass = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("heatmap density"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &density,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let props = paint.attributes();
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
            u.paint = bucket.paint.uniforms(&props, zoom);
            u.extra[0][0] = intensity;

            pass.set_pipeline(self.density_pipeline(cx.gpu, &bucket.paint));
            let (group, offset) = cx.uniforms.push(cx.gpu, &u);
            pass.set_bind_group(0, &group, &[offset]);
            pass.set_vertex_buffer(0, bucket.vertices.slice(..));
            if let Some(buffer) = &bucket.paint_buffer {
                pass.set_vertex_buffer(1, buffer.slice(..));
            }
            pass.set_index_buffer(bucket.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..bucket.index_count, 0, 0..1);
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
        cx.pass.set_pipeline(&self.color_pipeline);
        cx.bind_uniforms(&u);
        cx.pass.set_bind_group(1, &target.group, &[]);
        cx.pass.draw(0..3, 0..1);
    }
}
