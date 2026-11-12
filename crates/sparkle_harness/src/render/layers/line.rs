//! `line` layers: triangles widened in the vertex shader (see `sparkle::tile::LineBucket`),
//! drawn with a color, dashes, a sprite pattern or a gradient.

use std::any::Any;
use std::collections::HashMap;

use sparkle::paint::PaintArrays;
use sparkle::pattern::{CrossFade, DashAtlas, DashPattern, FeatureImage};
use sparkle::style::{LayerKind, LineCap, LineLayer, LinePaint, StyleLayer};
use sparkle::{Bucket, BucketGeometry};

use super::pattern::{AtlasGroup, TextureGroup, crossfaded_pattern};
use super::{DrawContext, LayerDraw, LayerRenderer, translate_units};
use crate::render::gpu::{Gpu, PipelineSpec};
use crate::render::indices::GpuIndices;

/// Texels in a `line-gradient` ramp.
const GRADIENT_SIZE: usize = 256;

/// The modes of `line.wgsl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Color = 0,
    Dash = 1,
    Pattern = 2,
    Gradient = 3,
}

pub struct LineRenderer {
    /// By paint signature and whether images are per vertex.
    pipelines: HashMap<(u32, bool), wgpu::RenderPipeline>,
    textures: TextureGroup,
    atlas: AtlasGroup,
    /// Repeats along dash patterns.
    dash_sampler: wgpu::Sampler,
    /// The dash patterns of layer-wide `line-dasharray`s, and their texture once uploaded
    /// at the atlas's current height.
    dashes: DashAtlas,
    dash_group: Option<(u32, wgpu::BindGroup)>,
    /// `line-gradient` ramps by style layer index.
    gradients: HashMap<usize, wgpu::BindGroup>,
    /// Stands in for per-vertex images in buckets without them: one zeroed instance.
    no_images: wgpu::Buffer,
}

struct GpuLine {
    vertices: wgpu::Buffer,
    triangles: GpuIndices,
    paint_buffer: Option<wgpu::Buffer>,
    /// The layout of `paint_buffer`; its data isn't kept.
    paint: PaintArrays,
    /// Per-vertex [`FeatureImage`]s.
    images: Option<wgpu::Buffer>,
    /// The bucket's own dash patterns, which `images` refer to, and the texture's size.
    dashes: Option<(wgpu::BindGroup, [u32; 2])>,
}

/// What one layer draws with, the same for each of its tiles.
struct LayerMode {
    mode: Mode,
    per_vertex: bool,
    extra: [[f32; 4]; 8],
}

impl LineRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        Self {
            pipelines: HashMap::new(),
            textures: TextureGroup::new(gpu),
            atlas: AtlasGroup::default(),
            dash_sampler: gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("dashes"),
                address_mode_u: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            dashes: DashAtlas::new(),
            dash_group: None,
            gradients: HashMap::new(),
            no_images: gpu.buffer(
                "no line images",
                &[0; size_of::<FeatureImage>()],
                wgpu::BufferUsages::VERTEX,
            ),
        }
    }

    fn pipeline(
        &mut self,
        gpu: &Gpu,
        paint: &PaintArrays,
        per_vertex: bool,
    ) -> &wgpu::RenderPipeline {
        let layout = &self.textures.layout;
        self.pipelines
            .entry((paint.signature(), per_vertex))
            .or_insert_with(|| {
                gpu.create_pipeline(&PipelineSpec {
                    label: "line",
                    shader: include_str!("../../shaders/line.wgsl"),
                    vertex_buffers: &[
                        wgpu::VertexBufferLayout {
                            array_stride: size_of::<sparkle::tile::LineVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![
                                0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4
                            ],
                        },
                        wgpu::VertexBufferLayout {
                            array_stride: size_of::<FeatureImage>() as u64,
                            step_mode: if per_vertex {
                                wgpu::VertexStepMode::Vertex
                            } else {
                                wgpu::VertexStepMode::Instance
                            },
                            attributes: &wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4],
                        },
                    ],
                    paint: Some(paint),
                    paint_location: 6,
                    bind_group_layouts: &[layout],
                    ..Default::default()
                })
            })
    }

    /// How `layer` draws at the camera's zoom, or `None` if it can't: its pattern image
    /// is missing.
    fn layer_mode(
        &mut self,
        gpu: &Gpu,
        index: usize,
        line: &LineLayer,
        zoom: f64,
        fade: &CrossFade,
    ) -> Option<LayerMode> {
        let paint = &line.paint;
        let mut extra = [[0.0; 4]; 8];
        extra[0] = [0.0, 0.0, fade.t, fade.zoom_scale(zoom)];
        let mut mode = LayerMode {
            mode: Mode::Color,
            per_vertex: false,
            extra,
        };
        // `line-pattern` wins over `line-dasharray`, which wins over `line-gradient`.
        if !matches!(paint.pattern.as_constant(), Some(None)) {
            let images = gpu.images.as_ref()?;
            mode.mode = Mode::Pattern;
            mode.extra[7] = [images.size[0] as f32, images.size[1] as f32, 0.0, 0.0];
            if paint.pattern.is_feature_dependent() {
                mode.per_vertex = true;
            } else {
                let [a, b] = crossfaded_pattern(images, &paint.pattern, fade, false)?;
                mode.extra[4] = a.rect;
                mode.extra[5] = b.rect;
                mode.extra[6] = [a.size[0], a.size[1], b.size[0], b.size[1]];
            }
        } else if !matches!(paint.dasharray.as_constant(), Some(None)) {
            mode.mode = Mode::Dash;
            if paint.dasharray.is_feature_dependent() {
                // Each feature's pattern fades into itself at the next zoom's scale.
                mode.per_vertex = true;
                mode.extra[2][3] = fade.from_scale;
                mode.extra[3][3] = fade.to_scale;
            } else {
                // As MapLibre, the layer's cap decides whether dashes are round.
                let round = line.layout.cap.evaluate_at_zoom(zoom) == LineCap::Round;
                let mut dash = |z: f64, scale: f32| {
                    let pattern = paint.dasharray.evaluate_at_zoom(z);
                    let p: Option<DashPattern> = pattern.and_then(|d| self.dashes.add(&d, round));
                    let mut u = p.map_or([0.0; 4], |p| p.uniform());
                    u[3] = scale;
                    u
                };
                mode.extra[2] = dash(fade.from_zoom, fade.from_scale);
                mode.extra[3] = dash(fade.to_zoom, fade.to_scale);
                if mode.extra[2][2] <= 0.0 && mode.extra[3][2] <= 0.0 {
                    mode.mode = Mode::Color;
                }
                mode.extra[7] = [
                    self.dashes.width() as f32,
                    self.dashes.height().max(1) as f32,
                    0.0,
                    0.0,
                ];
            }
        } else if self.gradient(gpu, index, paint).is_some() {
            mode.mode = Mode::Gradient;
        }
        mode.extra[0][0] = mode.mode as u8 as f32;
        mode.extra[0][1] = if mode.per_vertex { 1.0 } else { 0.0 };
        Some(mode)
    }

    /// The `line-gradient` ramp of the style layer `index` as a bind group, if it has one.
    fn gradient(&mut self, gpu: &Gpu, index: usize, paint: &LinePaint) -> Option<&wgpu::BindGroup> {
        if !self.gradients.contains_key(&index) {
            let ramp = paint.gradient_ramp(GRADIENT_SIZE)?;
            let texture = gpu.rgba_texture(
                "line gradient",
                GRADIENT_SIZE as u32,
                1,
                bytemuck::cast_slice(&ramp),
            );
            let view = texture.create_view(&Default::default());
            let group = self.textures.bind(gpu, &view, &gpu.linear_sampler);
            self.gradients.insert(index, group);
        }
        self.gradients.get(&index)
    }

    /// The layer-wide dash atlas as a bind group, uploaded again whenever it has grown.
    fn dash_group(&mut self, gpu: &Gpu) -> &wgpu::BindGroup {
        let height = self.dashes.height().max(1);
        if self.dash_group.as_ref().is_none_or(|(h, _)| *h != height) {
            let view = r8_texture(gpu, "line dashes", &self.dashes);
            let group = self.textures.bind(gpu, &view, &self.dash_sampler);
            self.dash_group = Some((height, group));
        }
        &self.dash_group.as_ref().expect("just made").1
    }
}

impl LayerRenderer for LineRenderer {
    fn layer_type(&self) -> &'static str {
        "line"
    }

    fn style_changed(&mut self) {
        self.gradients.clear();
    }

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<Box<dyn Any>> {
        let BucketGeometry::Line(line) = &bucket.geometry else {
            return None;
        };
        let paint = line.paint.layout();
        let dashes = line.dashes.as_ref().map(|atlas| {
            let view = r8_texture(gpu, "line dashes", atlas);
            let group = self.textures.bind(gpu, &view, &self.dash_sampler);
            (group, [atlas.width(), atlas.height().max(1)])
        });
        Some(Box::new(GpuLine {
            vertices: gpu.buffer(
                "line vertices",
                bytemuck::cast_slice(&line.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            triangles: GpuIndices::new(gpu, "line triangles", &line.triangles, 3),
            paint_buffer: gpu.paint_buffer(&line.paint),
            paint,
            images: (!line.images.is_empty()).then(|| {
                gpu.buffer(
                    "line images",
                    bytemuck::cast_slice(&line.images),
                    wgpu::BufferUsages::VERTEX,
                )
            }),
            dashes,
        }))
    }

    fn update_paint(&mut self, gpu: &Gpu, data: &mut dyn Any, bucket: &Bucket) -> bool {
        match (data.downcast_mut::<GpuLine>(), &bucket.geometry) {
            (Some(gpu_line), BucketGeometry::Line(line)) => gpu.update_paint_buffer(
                gpu_line.paint_buffer.as_ref(),
                &gpu_line.paint,
                &line.paint,
            ),
            _ => false,
        }
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::Line(line) = &layer.layer.kind else {
            return;
        };
        let paint = &line.paint;
        let props = paint.attributes();
        let zoom = cx.view.camera.zoom;
        let fade = CrossFade::at(zoom);
        let Some(mut mode) = self.layer_mode(cx.gpu, layer.index, line, zoom, &fade) else {
            return;
        };
        let translate = paint.translate.evaluate_at_zoom(zoom);
        let anchor = paint.translate_anchor.evaluate_at_zoom(zoom);
        mode.extra[1][2] = paint.layer_opacity.evaluate_at_zoom(zoom);

        for tile in &layer.tiles {
            let Some(bucket) = tile.bucket::<GpuLine>() else {
                continue;
            };
            let mut u = cx.tile_uniforms(tile);
            u.paint = bucket.paint.uniforms(&props, zoom);
            floorwidth_uniforms(&mut u.paint, &bucket.paint, &props, fade.layout_zoom);
            u.extra = mode.extra;
            u.extra[1][..2].copy_from_slice(&translate_units(translate, anchor, tile, cx.view));

            let images = match (&bucket.images, mode.per_vertex) {
                (Some(images), true) => Some(images),
                (None, false) => None,
                // The bucket was built for another style, or another image binding.
                _ => continue,
            };
            let group = match mode.mode {
                Mode::Pattern => match self.atlas.get(cx.gpu, &self.textures) {
                    Some(group) => group.clone(),
                    None => continue,
                },
                Mode::Dash if mode.per_vertex => {
                    let Some((group, size)) = &bucket.dashes else {
                        continue;
                    };
                    u.extra[7] = [size[0] as f32, size[1] as f32, 0.0, 0.0];
                    group.clone()
                }
                Mode::Dash => self.dash_group(cx.gpu).clone(),
                Mode::Gradient => self.gradients[&layer.index].clone(),
                Mode::Color => self.textures.empty.clone(),
            };

            cx.pass
                .set_pipeline(self.pipeline(cx.gpu, &bucket.paint, mode.per_vertex));
            cx.bind_uniforms(&u);
            cx.pass.set_bind_group(1, &group, &[]);
            cx.pass.set_stencil_reference(tile.stencil_ref);
            cx.pass.set_vertex_buffer(0, bucket.vertices.slice(..));
            cx.pass
                .set_vertex_buffer(1, images.unwrap_or(&self.no_images).slice(..));
            if let Some(buffer) = &bucket.paint_buffer {
                cx.pass.set_vertex_buffer(2, buffer.slice(..));
            }
            bucket.triangles.draw(cx.pass);
        }
    }
}

/// Make `paint_floorwidth` the width at the integer zoom `floor`, rather than the camera's.
fn floorwidth_uniforms(
    u: &mut sparkle::paint::PaintUniforms,
    paint: &PaintArrays,
    props: &sparkle::paint::PaintProperties<'_>,
    floor: f64,
) {
    let i = LinePaint::FLOORWIDTH_ATTRIBUTE;
    let at_floor = paint.uniforms(props, floor);
    u.values[i] = at_floor.values[i];
    u.factors[i / 4][i % 4] = at_floor.factors[i / 4][i % 4];
}

/// A dash atlas as an R8 texture view.
fn r8_texture(gpu: &Gpu, label: &str, atlas: &DashAtlas) -> wgpu::TextureView {
    let height = atlas.height().max(1);
    let mut data = atlas.data.clone();
    data.resize((atlas.width() * height) as usize, 0);
    gpu.texture_with_data(
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: atlas.width(),
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        &data,
    )
    .create_view(&Default::default())
}
