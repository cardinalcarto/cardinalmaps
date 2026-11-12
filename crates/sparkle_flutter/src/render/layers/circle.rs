//! `circle` layers: a quad per circle, sized in the vertex shader and rounded in the
//! fragment shader.

use std::collections::HashMap;

use sparkle::style::{CirclePitchAlignment, CirclePitchScale, LayerKind, StyleLayer};
use sparkle::tile::CircleVertex;
use sparkle::{Bucket, BucketGeometry};

use super::{BucketData, DrawContext, LayerDraw, LayerRenderer, translate_units};
use crate::gpu::{Buffer, Pass, Pipeline, PipelineSpec, VertexBufferLayout, VertexFormat};
use crate::render::Gpu;
use crate::render::indices::GpuIndices;
use crate::render::paint::{GpuPaint, UniformsMemo};

pub struct CircleRenderer {
    /// By paint signature.
    pipelines: HashMap<u32, Pipeline>,
}

struct GpuCircles {
    vertices: Buffer,
    triangles: GpuIndices,
    paint: GpuPaint,
}

impl CircleRenderer {
    pub fn new(_gpu: &Gpu) -> Self {
        Self {
            pipelines: HashMap::new(),
        }
    }

    fn pipeline(&mut self, gpu: &Gpu, paint: &GpuPaint) -> Pipeline {
        *self.pipelines.entry(paint.signature()).or_insert_with(|| {
            let mut vertex_buffers = vec![VertexBufferLayout::packed(&[
                ("center", VertexFormat::Float32x2),
                ("extrude", VertexFormat::Float32x2),
            ])];
            vertex_buffers.extend(paint.vertex_buffers());
            gpu.pipeline(&PipelineSpec {
                program: "Circle",
                vertex_buffers,
                // Each tile only has the circles centered inside it, so circles overlapping
                // the tile's edge must not be cut off there.
                clip: false,
                ..Default::default()
            })
        })
    }
}

impl LayerRenderer for CircleRenderer {
    fn layer_type(&self) -> &'static str {
        "circle"
    }

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<BucketData> {
        let BucketGeometry::Circle(circles) = &bucket.geometry else {
            return None;
        };
        if circles.triangles.is_empty() {
            return None;
        }
        Some(Box::new(GpuCircles {
            vertices: gpu.buffer(bytemuck::cast_slice(&circles.vertices)),
            triangles: GpuIndices::new(gpu, &circles.triangles, 3),
            paint: GpuPaint::new(gpu, &circles.paint),
        }))
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::Circle(circle) = &layer.layer.kind else {
            return;
        };
        let paint = &circle.paint;
        let props = paint.attributes();
        let mut memo = UniformsMemo::default();
        let zoom = cx.view.camera.zoom;
        let translate = paint.translate.evaluate_at_zoom(zoom);
        let anchor = paint.translate_anchor.evaluate_at_zoom(zoom);
        let scale_with_map = paint.pitch_scale.evaluate_at_zoom(zoom) == CirclePitchScale::Map;
        let pitch_with_map =
            paint.pitch_alignment.evaluate_at_zoom(zoom) == CirclePitchAlignment::Map;
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        let camera_to_center = cx.view.camera.camera_to_center() as f32;
        for tile in &layer.tiles {
            let Some(bucket) = tile.bucket::<GpuCircles>() else {
                continue;
            };
            let mut u = cx.tile_uniforms(tile);
            u.paint = memo.uniforms(&bucket.paint.layout, &props, zoom);
            u.extra[0] = [
                flag(scale_with_map),
                flag(pitch_with_map),
                camera_to_center,
                0.0,
            ];
            u.extra[1][..2].copy_from_slice(&translate_units(translate, anchor, tile, cx.view));

            let pipeline = self.pipeline(cx.gpu, &bucket.paint);
            cx.pass.set_pipeline(&pipeline);
            cx.bind_uniforms(&u, Some(tile));
            let gpu = cx.gpu;
            bucket
                .triangles
                .draw(cx.pass, |pass: &mut Pass<'_>, first| {
                    let stride = size_of::<CircleVertex>();
                    pass.set_vertex_buffer(0, bucket.vertices.slice_from(first, stride));
                    bucket.paint.bind_from(gpu, pass, 1, first);
                });
        }
    }
}
