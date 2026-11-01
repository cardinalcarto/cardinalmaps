//! `line` layers: triangles widened in the vertex shader (see `sparkle::tile::LineBucket`),
//! drawn with a color, dashes, a sprite pattern or a gradient.
//!
//! A tile's buckets of consecutive layers that draw plainly (a color or the layer-wide
//! dash atlas, with no paint values per feature) are uploaded as one and drawn with one
//! draw per tile: each vertex carries its layer's style index, which picks its layer's
//! paint values and extras from a `Layers` block of a window of style layers
//! (`shaders/line_layers.glsl`). Styles tend to have long runs of such layers, such as
//! road casings and then roads.

use std::any::Any;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sparkle::paint::Binding;
use sparkle::pattern::{CrossFade, DashAtlas, DashPattern, FeatureImage};
use sparkle::style::{LayerKind, LineCap, LineLayer, LinePaint, StyleLayer};
use sparkle::{Bucket, BucketGeometry};

use super::pattern::crossfaded_pattern;
use super::{BucketData, DrawContext, LayerDraw, LayerRenderer, translate_pixels, translate_units};
use crate::gpu::{
    Buffer, Pipeline, PipelineSpec, Sampler, Slice, StepMode, Texture, TextureFormat,
    VertexBufferLayout, VertexFormat,
};
use crate::render::paint::{GpuPaint, UniformsMemo};
use crate::render::{Gpu, LINEAR};

/// Style layers per `Layers` block: `LINE_LAYERS` in `line_layers.glsl`.
const LINE_LAYERS: usize = 48;
/// Vec4s per layer in a `Layers` block: `LINE_LAYER_VEC4S` in `line_layers.glsl`.
const LAYER_VEC4S: usize = 7;

/// Texels in a `line-gradient` ramp.
const GRADIENT_SIZE: usize = 256;

/// Repeats along dash patterns.
const DASH_SAMPLER: Sampler = Sampler {
    linear: true,
    repeat_u: true,
    repeat_v: false,
};

/// The modes of `line.vert`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Color = 0,
    Dash = 1,
    Pattern = 2,
    Gradient = 3,
}

pub struct LineRenderer {
    /// By paint signature and whether images are per vertex.
    pipelines: HashMap<(u32, bool), Pipeline>,
    /// For merged buckets.
    merged: Pipeline,
    /// The dash patterns of layer-wide `line-dasharray`s, and their texture once uploaded
    /// at the atlas's current height.
    dashes: DashAtlas,
    dash_texture: Option<(u32, Texture)>,
    /// `line-gradient` ramps by style layer index.
    gradients: HashMap<usize, Texture>,
    /// Stands in for per-vertex images in buckets without them: one zeroed instance.
    no_images: Buffer,
}

struct GpuLine {
    vertices: Buffer,
    triangles: Buffer,
    triangle_count: u32,
    paint: GpuPaint,
    /// Per-vertex [`FeatureImage`]s.
    images: Option<Buffer>,
    /// The bucket's own dash patterns, which `images` refer to, and the texture's size.
    dashes: Option<(Texture, [u32; 2])>,
}

/// Line buckets of consecutive layers of a tile uploaded as one.
struct MergedLines {
    vertices: Buffer,
    /// The style index of each vertex's layer, as a float.
    layers: Buffer,
    triangles: Buffer,
    triangle_count: u32,
}

/// What each of the merged buckets is.
struct MergedLine(Arc<MergedLines>);

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
            merged: gpu.pipeline(&PipelineSpec {
                program: "LineMerged",
                vertex_buffers: vec![
                    line_vertex_layout(),
                    VertexBufferLayout::packed(&[("layer", VertexFormat::Float32)]),
                ],
                ..Default::default()
            }),
            dashes: DashAtlas::new(),
            dash_texture: None,
            gradients: HashMap::new(),
            no_images: gpu.buffer(&[0; size_of::<FeatureImage>()]),
        }
    }

    fn pipeline(&mut self, gpu: &Gpu, paint: &GpuPaint, per_vertex: bool) -> Pipeline {
        *self
            .pipelines
            .entry((paint.signature(), per_vertex))
            .or_insert_with(|| {
                let mut vertex_buffers = vec![
                    line_vertex_layout(),
                    VertexBufferLayout {
                        step: if per_vertex {
                            StepMode::Vertex
                        } else {
                            StepMode::Instance
                        },
                        ..VertexBufferLayout::packed(&[
                            ("feature_a", VertexFormat::Float32x4),
                            ("feature_b", VertexFormat::Float32x4),
                        ])
                    },
                ];
                vertex_buffers.extend(paint.vertex_buffers());
                gpu.pipeline(&PipelineSpec {
                    program: "Line",
                    vertex_buffers,
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

    /// The `line-gradient` ramp of the style layer `index` as a texture, if it has one.
    fn gradient(&mut self, gpu: &Gpu, index: usize, paint: &LinePaint) -> Option<&Texture> {
        match self.gradients.entry(index) {
            Entry::Occupied(e) => Some(e.into_mut()),
            Entry::Vacant(e) => {
                let ramp = paint.gradient_ramp(GRADIENT_SIZE)?;
                Some(e.insert(gpu.rgba_texture(
                    GRADIENT_SIZE as u32,
                    1,
                    bytemuck::cast_slice(&ramp),
                )))
            }
        }
    }

    /// The layer-wide dash atlas as a texture, uploaded again whenever it has grown.
    fn dash_texture(&mut self, gpu: &Gpu) -> &Texture {
        let height = self.dashes.height().max(1);
        if self.dash_texture.as_ref().is_none_or(|(h, _)| *h != height) {
            self.dash_texture = Some((height, r8_texture(gpu, &self.dashes)));
        }
        &self.dash_texture.as_ref().expect("just made").1
    }
}

impl LayerRenderer for LineRenderer {
    fn layer_type(&self) -> &'static str {
        "line"
    }

    fn style_changed(&mut self) {
        self.gradients.clear();
    }

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<BucketData> {
        let BucketGeometry::Line(line) = &bucket.geometry else {
            return None;
        };
        let dashes = line.dashes.as_ref().map(|atlas| {
            (
                r8_texture(gpu, atlas),
                [atlas.width(), atlas.height().max(1)],
            )
        });
        Some(Box::new(GpuLine {
            vertices: gpu.buffer(bytemuck::cast_slice(&line.vertices)),
            triangles: gpu.buffer(bytemuck::cast_slice(&line.triangles)),
            triangle_count: line.triangles.len() as u32,
            paint: GpuPaint::new(gpu, &line.paint),
            images: (!line.images.is_empty())
                .then(|| gpu.buffer(bytemuck::cast_slice(&line.images))),
            dashes,
        }))
    }

    fn merge_key(&self, layer: &StyleLayer, bucket: &Bucket) -> Option<usize> {
        let (LayerKind::Line(l), BucketGeometry::Line(line)) = (&layer.kind, &bucket.geometry)
        else {
            return None;
        };
        let p = &l.paint;
        let plain = matches!(p.pattern.as_constant(), Some(None))
            && matches!(p.gradient.as_constant(), Some(None))
            && !p.dasharray.is_feature_dependent();
        let uniform = line
            .paint
            .attributes
            .iter()
            .all(|a| a.binding == Binding::Uniform);
        let off = option_env!("SPARKLE_NO_MERGE").is_some(); // PERF-TEMP
        (!off && plain && uniform && line.images.is_empty() && line.dashes.is_none())
            .then_some(bucket.style_layer / LINE_LAYERS)
    }

    fn upload_merged(&mut self, gpu: &Gpu, buckets: &[(&StyleLayer, &Bucket)]) -> Vec<BucketData> {
        let lines: Vec<(&Bucket, &sparkle::tile::LineBucket)> = buckets
            .iter()
            .filter_map(|&(_, b)| match &b.geometry {
                BucketGeometry::Line(line) => Some((b, line)),
                _ => None,
            })
            .collect();
        let count: usize = lines.iter().map(|(_, l)| l.vertices.len()).sum();
        let mut vertices = Vec::with_capacity(count);
        let mut layers = Vec::with_capacity(count);
        let mut triangles = Vec::new();
        for (bucket, line) in &lines {
            let base = vertices.len() as u32;
            vertices.extend_from_slice(&line.vertices);
            layers.extend(std::iter::repeat_n(
                bucket.style_layer as f32,
                line.vertices.len(),
            ));
            triangles.extend(line.triangles.iter().map(|i| i + base));
        }
        let merged = Arc::new(MergedLines {
            vertices: gpu.buffer(bytemuck::cast_slice(&vertices)),
            layers: gpu.buffer(bytemuck::cast_slice(&layers)),
            triangles: gpu.buffer(bytemuck::cast_slice(&triangles)),
            triangle_count: triangles.len() as u32,
        });
        buckets
            .iter()
            .map(|_| Box::new(MergedLine(merged.clone())) as BucketData)
            .collect()
    }

    fn update_paint(
        &mut self,
        gpu: &Gpu,
        data: &mut (dyn Any + Send + Sync),
        bucket: &Bucket,
    ) -> bool {
        match (data.downcast_mut::<GpuLine>(), &bucket.geometry) {
            (Some(gpu_line), BucketGeometry::Line(line)) => gpu_line.paint.update(gpu, &line.paint),
            _ => false,
        }
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        self.draw_group(cx, &[layer]);
    }

    fn draw_group(&mut self, cx: &mut DrawContext<'_, '_>, layers: &[&LayerDraw<'_>]) {
        let zoom = cx.view.camera.zoom;
        let fade = CrossFade::at(zoom);
        // Every layer's mode first: they add to the dash atlas, which then has its size.
        let mut modes = Vec::with_capacity(layers.len());
        for layer in layers {
            let LayerKind::Line(line) = &layer.layer.kind else {
                continue;
            };
            if let Some(mode) = self.layer_mode(cx.gpu, layer.index, line, zoom, &fade) {
                // For buckets drawn on their own: at the camera's zoom and the layout's.
                let memos = [UniformsMemo::default(), UniformsMemo::default()];
                modes.push((*layer, line, mode, memos));
            }
        }
        let dash_size = [
            self.dashes.width() as f32,
            self.dashes.height().max(1) as f32,
            0.0,
            0.0,
        ];
        // The `Layers` blocks of the windows these layers are in.
        let mut blocks: HashMap<usize, Vec<[f32; 4]>> = HashMap::new();
        for (layer, line, mode, _) in &mut modes {
            if mode.mode == Mode::Dash && !mode.per_vertex {
                mode.extra[7] = dash_size;
            }
            let paint = &line.paint;
            mode.extra[1][2] = paint.layer_opacity.evaluate_at_zoom(zoom);
            let window = layer.index / LINE_LAYERS;
            let block = blocks.entry(window).or_insert_with(|| {
                let mut block = vec![[0.0; 4]; 1 + LINE_LAYERS * LAYER_VEC4S];
                block[0][0] = (window * LINE_LAYERS) as f32;
                block
            });
            // As `line_layers.glsl` lays them out.
            let at = 1 + (layer.index % LINE_LAYERS) * LAYER_VEC4S;
            let value =
                |property: &dyn sparkle::paint::PaintProperty, zoom| property.uniform_value(zoom);
            let opacity = value(&paint.opacity, zoom)[0] * mode.extra[1][2];
            let width = value(&paint.width, zoom)[0];
            let translate = paint.translate.evaluate_at_zoom(zoom);
            let anchor = paint.translate_anchor.evaluate_at_zoom(zoom);
            let [x, y] = translate_pixels(translate, anchor, cx.view);
            let [mode_index, _, fade_t, zoom_scale] = mode.extra[0];
            block[at..at + LAYER_VEC4S].copy_from_slice(&[
                value(&paint.color, zoom).map(|c| c * opacity),
                [
                    width,
                    value(&paint.blur, zoom)[0],
                    value(&paint.gap_width, zoom)[0],
                    value(&paint.offset, zoom)[0],
                ],
                [
                    value(&paint.width, fade.layout_zoom)[0],
                    mode_index,
                    fade_t,
                    zoom_scale,
                ],
                // Drawn this frame.
                [x, y, 0.0, 1.0],
                mode.extra[2],
                mode.extra[3],
                mode.extra[7],
            ]);
        }
        let blocks: HashMap<usize, Slice<'static>> = blocks
            .into_iter()
            .map(|(window, block)| (window, cx.pass.frame_data(bytemuck::cast_slice(&block))))
            .collect();

        // Merged buckets are drawn once per tile, with the first of their layers here.
        let mut drawn = HashSet::new();
        for (layer, line, mode, memos) in &mut modes {
            let block = blocks[&(layer.index / LINE_LAYERS)];
            for tile in &layer.tiles {
                if let Some(MergedLine(merged)) = tile.bucket::<MergedLine>() {
                    if drawn.insert((Arc::as_ptr(merged), tile.placement)) {
                        self.draw_merged(cx, tile, merged, block);
                    }
                    continue;
                }
                if let Some(bucket) = tile.bucket::<GpuLine>() {
                    self.draw_tile(cx, layer, line, mode, memos, tile, bucket);
                }
            }
        }
    }
}

impl LineRenderer {
    /// Draw a tile's merged bucket, with its layers' values in `block`.
    fn draw_merged(
        &mut self,
        cx: &mut DrawContext<'_, '_>,
        tile: &super::TileDraw<'_>,
        merged: &MergedLines,
        block: Slice<'_>,
    ) {
        cx.pass.set_pipeline(&self.merged);
        // Paint values and extras are in the block.
        let u = cx.tile_uniforms(tile);
        cx.bind_uniforms(&u, Some(tile));
        cx.pass.set_uniform_block("Layers", block);
        // Plain colors don't sample it.
        let texture = self.dash_texture(cx.gpu).clone();
        cx.pass.set_texture("image", &texture, DASH_SAMPLER);
        cx.pass.set_stencil_reference(tile.stencil_ref);
        cx.pass.set_vertex_buffer(0, merged.vertices.slice());
        cx.pass.set_vertex_buffer(1, merged.layers.slice());
        cx.pass.set_index_buffer(merged.triangles.slice(), false);
        cx.pass.draw_indexed(merged.triangle_count, 1);
    }

    /// Draw one tile's bucket of `layer` on its own.
    #[allow(clippy::too_many_arguments)]
    fn draw_tile(
        &mut self,
        cx: &mut DrawContext<'_, '_>,
        layer: &LayerDraw<'_>,
        line: &LineLayer,
        mode: &LayerMode,
        [memo, floor_memo]: &mut [UniformsMemo; 2],
        tile: &super::TileDraw<'_>,
        bucket: &GpuLine,
    ) {
        let paint = &line.paint;
        let props = paint.attributes();
        let zoom = cx.view.camera.zoom;
        let fade = CrossFade::at(zoom);
        let mut u = cx.tile_uniforms(tile);
        u.paint = memo.uniforms(&bucket.paint.layout, &props, zoom);
        let at_floor = floor_memo.uniforms(&bucket.paint.layout, &props, fade.layout_zoom);
        floorwidth_uniforms(&mut u.paint, &at_floor);
        u.extra = mode.extra;
        let translate = paint.translate.evaluate_at_zoom(zoom);
        let anchor = paint.translate_anchor.evaluate_at_zoom(zoom);
        u.extra[1][..2].copy_from_slice(&translate_units(translate, anchor, tile, cx.view));

        let images = match (&bucket.images, mode.per_vertex) {
            (Some(images), true) => images.clone(),
            (None, false) => self.no_images.clone(),
            // The bucket was built for another style, or another image binding.
            _ => return,
        };
        // Every sampler must be bound: a placeholder for plain colors.
        let (texture, sampler) = match mode.mode {
            Mode::Pattern => match &cx.gpu.images {
                Some(images) => (images.texture.clone(), LINEAR),
                None => return,
            },
            Mode::Dash if mode.per_vertex => {
                let Some((texture, size)) = &bucket.dashes else {
                    return;
                };
                u.extra[7] = [size[0] as f32, size[1] as f32, 0.0, 0.0];
                (texture.clone(), DASH_SAMPLER)
            }
            Mode::Dash => (self.dash_texture(cx.gpu).clone(), DASH_SAMPLER),
            Mode::Gradient => (self.gradients[&layer.index].clone(), LINEAR),
            Mode::Color => (cx.gpu.no_dem().clone(), LINEAR),
        };

        let pipeline = self.pipeline(cx.gpu, &bucket.paint, mode.per_vertex);
        cx.pass.set_pipeline(&pipeline);
        cx.bind_uniforms(&u, Some(tile));
        cx.pass.set_texture("image", &texture, sampler);
        cx.pass.set_stencil_reference(tile.stencil_ref);
        cx.pass.set_vertex_buffer(0, bucket.vertices.slice());
        cx.pass.set_vertex_buffer(1, images.slice());
        bucket.paint.bind(cx.gpu, cx.pass, 2);
        cx.pass.set_index_buffer(bucket.triangles.slice(), false);
        cx.pass.draw_indexed(bucket.triangle_count, 1);
    }
}

/// Make `paint_floorwidth` the width in `at_floor`, the uniforms at the integer zoom the
/// layout is at, rather than the camera's.
fn floorwidth_uniforms(
    u: &mut sparkle::paint::PaintUniforms,
    at_floor: &sparkle::paint::PaintUniforms,
) {
    let i = LinePaint::FLOORWIDTH_ATTRIBUTE;
    u.values[i] = at_floor.values[i];
    u.factors[i / 4][i % 4] = at_floor.factors[i / 4][i % 4];
}

/// `sparkle::tile::LineVertex`.
fn line_vertex_layout() -> VertexBufferLayout {
    VertexBufferLayout::packed(&[
        ("pos", VertexFormat::Float32x2),
        ("extrude", VertexFormat::Float32x2),
        ("offset_dir", VertexFormat::Float32x2),
        ("metrics", VertexFormat::Float32x4),
    ])
}

/// A dash atlas as an R8 texture.
fn r8_texture(gpu: &Gpu, atlas: &DashAtlas) -> Texture {
    let height = atlas.height().max(1);
    let mut data = atlas.data.clone();
    data.resize((atlas.width() * height) as usize, 0);
    gpu.texture(
        [atlas.width(), height],
        TextureFormat::R8,
        1,
        false,
        Some(&data),
    )
}
