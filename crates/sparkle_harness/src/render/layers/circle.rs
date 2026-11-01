//! `circle` layers: a quad per circle, sized in the vertex shader and rounded in the
//! fragment shader.

use std::any::Any;
use std::collections::HashMap;

use sparkle::paint::PaintArrays;
use sparkle::style::{CirclePitchAlignment, CirclePitchScale, LayerKind, StyleLayer};
use sparkle::{Bucket, BucketGeometry};

use super::{DrawContext, LayerDraw, LayerRenderer, translate_units};
use crate::render::gpu::{Gpu, PipelineSpec};

pub struct CircleRenderer {
    /// By paint signature.
    pipelines: HashMap<u32, wgpu::RenderPipeline>,
}

struct GpuCircles {
    vertices: wgpu::Buffer,
    triangles: wgpu::Buffer,
    triangle_count: u32,
    paint_buffer: Option<wgpu::Buffer>,
    /// The layout of `paint_buffer`; its data isn't kept.
    paint: PaintArrays,
}

impl CircleRenderer {
    pub fn new(_gpu: &Gpu) -> Self {
        Self {
            pipelines: HashMap::new(),
        }
    }

    fn pipeline(&mut self, gpu: &Gpu, paint: &PaintArrays) -> &wgpu::RenderPipeline {
        self.pipelines.entry(paint.signature()).or_insert_with(|| {
            gpu.create_pipeline(&PipelineSpec {
                label: "circle",
                shader: include_str!("../../shaders/circle.wgsl"),
                vertex_buffers: &[wgpu::VertexBufferLayout {
                    array_stride: size_of::<sparkle::tile::CircleVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                }],
                paint: Some(paint),
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

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<Box<dyn Any>> {
        let BucketGeometry::Circle(circles) = &bucket.geometry else {
            return None;
        };
        let paint = circles.paint.layout();
        Some(Box::new(GpuCircles {
            vertices: gpu.buffer(
                "circle vertices",
                bytemuck::cast_slice(&circles.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            triangles: gpu.buffer(
                "circle triangles",
                bytemuck::cast_slice(&circles.triangles),
                wgpu::BufferUsages::INDEX,
            ),
            triangle_count: circles.triangles.len() as u32,
            paint_buffer: gpu.paint_buffer(&circles.paint),
            paint,
        }))
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::Circle(circle) = &layer.layer.kind else {
            return;
        };
        let paint = &circle.paint;
        let props = paint.attributes();
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
            u.paint = bucket.paint.uniforms(&props, zoom);
            u.extra[0] = [
                flag(scale_with_map),
                flag(pitch_with_map),
                camera_to_center,
                0.0,
            ];
            u.extra[1][..2].copy_from_slice(&translate_units(translate, anchor, tile, cx.view));

            cx.pass.set_pipeline(self.pipeline(cx.gpu, &bucket.paint));
            cx.bind_uniforms(&u);
            cx.pass.set_vertex_buffer(0, bucket.vertices.slice(..));
            if let Some(buffer) = &bucket.paint_buffer {
                cx.pass.set_vertex_buffer(1, buffer.slice(..));
            }
            cx.pass
                .set_index_buffer(bucket.triangles.slice(..), wgpu::IndexFormat::Uint32);
            cx.pass.draw_indexed(0..bucket.triangle_count, 0, 0..1);
        }
    }
}
