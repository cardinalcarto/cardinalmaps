//! `fill` layers: triangles, plus an outline pass with `fill-antialias`.

use std::any::Any;
use std::collections::HashMap;

use sparkle::pattern::{CrossFade, FeatureImage, pattern_placement};
use sparkle::style::{FillLayer, LayerKind, StyleLayer};
use sparkle::tile::FillVertex;
use sparkle::{Bucket, BucketGeometry};
use sparkle_view::camera::TILE_SIZE;

use super::pattern::crossfaded_pattern;
use super::{BucketData, DrawContext, LayerDraw, LayerRenderer, TileDraw, translate_units};
use crate::gpu::{
    Buffer, Pass, Pipeline, PipelineSpec, StepMode, Topology, VertexBufferLayout, VertexFormat,
};
use crate::render::indices::GpuIndices;
use crate::render::paint::{GpuPaint, UniformsMemo};
use crate::render::{Gpu, LINEAR};

pub struct FillRenderer {
    /// By paint signature, whether it draws outlines, and whether patterns are per vertex.
    pipelines: HashMap<(u32, bool, bool), Pipeline>,
    /// Stands in for per-vertex patterns in buckets without them: one zeroed instance.
    no_images: Buffer,
}

struct GpuFill {
    vertices: Buffer,
    triangles: GpuIndices,
    /// Ring edges as a line list.
    outlines: Option<GpuIndices>,
    paint: GpuPaint,
    /// Per-vertex [`FeatureImage`]s of a feature-dependent `fill-pattern`.
    images: Option<Buffer>,
}

/// How a layer's pattern is drawn this frame.
enum Pattern {
    None,
    /// The same for every feature: cross-faded images, placed per tile.
    Layer([super::pattern::PatternImage; 2]),
    PerVertex,
}

impl FillRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        Self {
            pipelines: HashMap::new(),
            no_images: gpu.buffer(&[0; size_of::<FeatureImage>()]),
        }
    }

    fn pipeline(
        &mut self,
        gpu: &Gpu,
        paint: &GpuPaint,
        outline: bool,
        per_vertex: bool,
    ) -> Pipeline {
        *self
            .pipelines
            .entry((paint.signature(), outline, per_vertex))
            .or_insert_with(|| {
                let mut vertex_buffers = vec![
                    VertexBufferLayout::packed(&[("pos", VertexFormat::Float32x2)]),
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
                    program: "Fill",

                    vertex_buffers,
                    topology: if outline {
                        Topology::LineList
                    } else {
                        Topology::TriangleList
                    },
                    ..Default::default()
                })
            })
    }

    /// The layer's pattern at the camera's zoom, or `None` if the layer can't be drawn:
    /// its images are missing.
    fn pattern(gpu: &Gpu, fill: &FillLayer, fade: &CrossFade) -> Option<Pattern> {
        let pattern = &fill.paint.pattern;
        if matches!(pattern.as_constant(), Some(None)) {
            return Some(Pattern::None);
        }
        let images = gpu.images.as_ref()?;
        if pattern.is_feature_dependent() {
            return Some(Pattern::PerVertex);
        }
        crossfaded_pattern(images, pattern, fade, true).map(Pattern::Layer)
    }
}

impl LayerRenderer for FillRenderer {
    fn layer_type(&self) -> &'static str {
        "fill"
    }

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<BucketData> {
        let BucketGeometry::Fill(fill) = &bucket.geometry else {
            return None;
        };
        Some(Box::new(GpuFill {
            vertices: gpu.buffer(bytemuck::cast_slice(&fill.vertices)),
            triangles: GpuIndices::new(gpu, &fill.triangles, 3),
            outlines: (!fill.outlines.is_empty()).then(|| GpuIndices::new(gpu, &fill.outlines, 2)),
            paint: GpuPaint::new(gpu, &fill.paint),
            images: (!fill.images.is_empty())
                .then(|| gpu.buffer(bytemuck::cast_slice(&fill.images))),
        }))
    }

    fn update_paint(
        &mut self,
        gpu: &Gpu,
        data: &mut (dyn Any + Send + Sync),
        bucket: &Bucket,
    ) -> bool {
        match (data.downcast_mut::<GpuFill>(), &bucket.geometry) {
            (Some(gpu_fill), BucketGeometry::Fill(fill)) => gpu_fill.paint.update(gpu, &fill.paint),
            _ => false,
        }
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::Fill(fill) = &layer.layer.kind else {
            return;
        };
        let paint = &fill.paint;
        let props = paint.attributes();
        let mut memo = UniformsMemo::default();
        let zoom = cx.view.camera.zoom;
        let fade = CrossFade::at(zoom);
        let Some(pattern) = Self::pattern(cx.gpu, fill, &fade) else {
            return;
        };
        let per_vertex = matches!(pattern, Pattern::PerVertex);
        // Every sampler must be bound: a placeholder when there's no pattern.
        let texture = match (&pattern, &cx.gpu.images) {
            (Pattern::None, _) | (_, None) => cx.gpu.no_dem(),
            (_, Some(images)) => &images.texture,
        };
        let translate = paint.translate.evaluate_at_zoom(zoom);
        let anchor = paint.translate_anchor.evaluate_at_zoom(zoom);
        // As in MapLibre, the outline is drawn whenever `fill-antialias` is on, in
        // `fill-outline-color` or else the fill's own color (or pattern).
        let antialias = paint.antialias.evaluate_at_zoom(zoom);
        let outline_color_set = !matches!(paint.outline_color.as_constant(), Some(None));
        let layer_opacity = paint.layer_opacity.evaluate_at_zoom(zoom);

        for tile in &layer.tiles {
            let Some(bucket) = tile.bucket::<GpuFill>() else {
                continue;
            };
            let images = match (&bucket.images, per_vertex) {
                (Some(images), true) => images.clone(),
                (None, false) => self.no_images.clone(),
                // The bucket was built for another pattern binding.
                _ => continue,
            };
            let mut u = cx.tile_uniforms(tile);
            u.paint = memo.uniforms(&bucket.paint.layout, &props, zoom);
            u.extra[0] = [0.0, f32::from(u8::from(outline_color_set)), 0.0, fade.t];
            u.extra[1] = [0.0, 0.0, layer_opacity, 0.0];
            u.extra[1][..2].copy_from_slice(&translate_units(translate, anchor, tile, cx.view));
            pattern_uniforms(&mut u.extra, &pattern, tile, &fade, cx);

            let pipeline = self.pipeline(cx.gpu, &bucket.paint, false, per_vertex);
            cx.pass.set_pipeline(&pipeline);
            cx.bind_uniforms(&u, Some(tile));
            cx.pass.set_texture("atlas", texture, LINEAR);
            cx.pass.set_stencil_reference(tile.stencil_ref);
            let gpu = cx.gpu;
            let bind = |pass: &mut Pass<'_>, first| {
                pass.set_vertex_buffer(
                    0,
                    bucket.vertices.slice_from(first, size_of::<FillVertex>()),
                );
                let images = match per_vertex {
                    true => images.slice_from(first, size_of::<FeatureImage>()),
                    false => images.slice(),
                };
                pass.set_vertex_buffer(1, images);
                bucket.paint.bind_from(gpu, pass, 2, first);
            };
            bucket.triangles.draw(cx.pass, bind);

            if let (true, Some(outlines)) = (antialias, &bucket.outlines) {
                u.extra[0][0] = 1.0;
                let pipeline = self.pipeline(cx.gpu, &bucket.paint, true, per_vertex);
                cx.pass.set_pipeline(&pipeline);
                cx.bind_uniforms(&u, Some(tile));
                outlines.draw(cx.pass, bind);
            }
        }
    }
}

/// Fill in `extra[2..8]` of `fill.vert` for drawing `pattern` in `tile`.
fn pattern_uniforms(
    extra: &mut [[f32; 4]; 8],
    pattern: &Pattern,
    tile: &TileDraw<'_>,
    fade: &CrossFade,
    cx: &DrawContext<'_, '_>,
) {
    let Some(images) = &cx.gpu.images else {
        return;
    };
    extra[7] = [images.size[0] as f32, images.size[1] as f32, 0.0, 0.0];
    let p = tile.placement;
    match pattern {
        Pattern::None => {}
        Pattern::Layer([a, b]) => {
            extra[0][2] = 1.0;
            extra[2] = a.rect;
            extra[3] = b.rect;
            for (i, image) in [(4, a), (5, b)] {
                let (origin, per_unit) = pattern_placement(
                    (p.z, p.x, p.y),
                    tile.extent,
                    TILE_SIZE,
                    fade.layout_zoom,
                    image.size,
                );
                extra[i] = [origin[0], origin[1], per_unit[0], per_unit[1]];
            }
        }
        Pattern::PerVertex => {
            extra[0][2] = 2.0;
            // Each feature's pattern has its own size, so the shader reduces the tile's
            // origin itself.
            let tile_pixels = TILE_SIZE * (fade.layout_zoom - f64::from(p.z)).exp2();
            let origin = [p.x as f64 * tile_pixels, f64::from(p.y) * tile_pixels];
            let upper = origin.map(|o| (o / 65536.0).floor());
            let lower = [
                origin[0] - upper[0] * 65536.0,
                origin[1] - upper[1] * 65536.0,
            ];
            extra[6] = [upper[0], upper[1], lower[0], lower[1]].map(|v| v as f32);
            extra[7][2] = (tile_pixels / f64::from(tile.extent)) as f32;
            extra[1][3] = fade.from_scale;
            extra[7][3] = fade.to_scale;
        }
    }
}
