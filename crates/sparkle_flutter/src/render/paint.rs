//! Paint attributes (see [`sparkle::paint`]) for shaders compiled ahead of time.
//!
//! The harness generates each pipeline's accessors from a bucket's bindings. Here shaders
//! are compiled into a bundle at build time, so every paint property has fixed inputs,
//! sized for its composite form, whatever its binding:
//!
//! | floats | inputs                         | value                                        |
//! |--------|--------------------------------|----------------------------------------------|
//! | 1      | `vec2 a_<name>`                | `values[i].x + mix(a.x, a.y, factor)`        |
//! | 2      | `vec4 a_<name>`                | `values[i].xy + mix(a.xy, a.zw, factor)`     |
//! | 4      | `vec4 a_<name>`, `a_<name>_b`  | `values[i] + mix(a, a_b, factor)`            |
//!
//! (`shaders/paint.glsl` declares them.) Sparkle leaves `values[i]` zero unless the
//! property is a uniform, and `factor` zero unless it's composite. So per-vertex values are
//! repacked to the composite size (a source value written twice), and uniform properties
//! read zeros from [`PAINT_ZEROS`], a one-element instance buffer, since Flutter GPU wants
//! an attribute's format to match its input exactly and every input bound. Instanced draws
//! with per-instance paint ([`GpuPaint::instance_vertex_buffers`]) read the zeros per
//! vertex instead, as an instance buffer would be read past its end.

use sparkle::paint::{Binding, PaintArrays, PaintAttribute, PaintProperties, PaintUniforms};

use super::Gpu;
use crate::gpu::{Buffer, Pass, Slice, StepMode, VertexBufferLayout, VertexFormat, VertexLayout};

/// Bytes of the zeros uniform properties read: room for every property's inputs.
pub const PAINT_ZEROS: usize = sparkle::paint::MAX_PAINT_ATTRIBUTES * 32;

/// How many vertices of an instance may read [`PAINT_ZEROS`] each, with
/// [`GpuPaint::instance_vertex_buffers`]: the zeros buffer holds that many copies.
pub const PAINT_ZERO_VERTICES: usize = 6;

/// [`PaintArrays::uniforms`] for the tiles of one layer, evaluated once for all the tiles
/// laid out alike (with the same bindings and zoom range), as most of them are: the
/// expressions behind uniform properties don't depend on the tile.
#[derive(Default)]
pub struct UniformsMemo {
    last: Option<(Vec<Binding>, [f32; 2], f64, PaintUniforms)>,
}

impl UniformsMemo {
    /// The uniforms of `layout` with `properties` (always the same ones) at `zoom`.
    pub fn uniforms(
        &mut self,
        layout: &PaintArrays,
        properties: &PaintProperties<'_>,
        zoom: f64,
    ) -> PaintUniforms {
        if let Some((bindings, range, at, u)) = &self.last
            && *range == layout.zoom_range
            && *at == zoom
            && bindings
                .iter()
                .eq(layout.attributes.iter().map(|a| &a.binding))
        {
            return *u;
        }
        let u = layout.uniforms(properties, zoom);
        let bindings = layout.attributes.iter().map(|a| a.binding).collect();
        self.last = Some((bindings, layout.zoom_range, zoom, u));
        u
    }
}

/// Floats one vertex holds for `a` once repacked.
fn repacked_floats(a: &PaintAttribute) -> u32 {
    match (a.binding, a.components) {
        (Binding::Uniform, _) => 0,
        (_, 1) => 2,
        _ => 8.min(2 * a.components),
    }
}

/// The inputs of `a` at byte `offset`: `(name, format, offset)`.
fn inputs(a: &PaintAttribute, offset: u32) -> Vec<(String, VertexFormat, u32)> {
    let name = format!("a_{}", a.name);
    match a.components {
        1 => vec![(name, VertexFormat::Float32x2, offset)],
        2 => vec![(name, VertexFormat::Float32x4, offset)],
        _ => vec![
            (format!("{name}_b"), VertexFormat::Float32x4, offset + 16),
            (name, VertexFormat::Float32x4, offset),
        ],
    }
}

/// The vertex buffer slots of `paint`, to append after a layer's geometry: the repacked
/// per-vertex values (if any property has them), then the zeros (if any is a uniform).
pub fn vertex_buffers(paint: &PaintArrays) -> VertexLayout {
    let mut per_vertex = Vec::new();
    let mut zeros = Vec::new();
    let mut stride = 0;
    for (i, a) in paint.attributes.iter().enumerate() {
        if a.binding == Binding::Uniform {
            zeros.extend(inputs(a, i as u32 * 32));
        } else {
            per_vertex.extend(inputs(a, stride));
            stride += repacked_floats(a) * 4;
        }
    }
    let mut out = Vec::new();
    if !per_vertex.is_empty() {
        out.push(VertexBufferLayout {
            stride,
            step: StepMode::Vertex,
            attributes: per_vertex,
        });
    }
    if !zeros.is_empty() {
        out.push(VertexBufferLayout {
            stride: PAINT_ZEROS as u32,
            step: StepMode::Instance,
            attributes: zeros,
        });
    }
    out
}

/// `paint`'s per-vertex values repacked for [`vertex_buffers`]'s layout.
pub fn repack(paint: &PaintArrays) -> Vec<f32> {
    let stride = paint.stride as usize;
    if stride == 0 {
        return Vec::new();
    }
    let floats: usize = paint
        .attributes
        .iter()
        .map(|a| repacked_floats(a) as usize)
        .sum();
    let vertices = paint.data.len() / stride;
    let mut out = Vec::with_capacity(vertices * floats);
    for vertex in paint.data.chunks_exact(stride) {
        for a in &paint.attributes {
            let at = a.offset as usize;
            let n = a.components as usize;
            match a.binding {
                Binding::Uniform => {}
                Binding::Source => {
                    out.extend_from_slice(&vertex[at..at + n]);
                    out.extend_from_slice(&vertex[at..at + n]);
                }
                Binding::Composite => out.extend_from_slice(&vertex[at..at + 2 * n]),
            }
        }
    }
    out
}

/// A bucket's paint values on the GPU.
pub struct GpuPaint {
    buffer: Option<Buffer>,
    /// The layout `buffer` was made with; its data isn't kept.
    pub layout: PaintArrays,
}

impl GpuPaint {
    pub fn new(gpu: &Gpu, paint: &PaintArrays) -> Self {
        let values = repack(paint);
        Self {
            buffer: (!values.is_empty()).then(|| gpu.buffer(bytemuck::cast_slice(&values))),
            layout: paint.layout(),
        }
    }

    /// Upload new values of the same layout, after feature state changed them. Returns
    /// false if `paint` doesn't have this layout.
    pub fn update(&mut self, gpu: &Gpu, paint: &PaintArrays) -> bool {
        if self.layout.signature() != paint.signature() || self.layout.stride != paint.stride {
            return false;
        }
        let values = repack(paint);
        self.buffer = (!values.is_empty()).then(|| gpu.buffer(bytemuck::cast_slice(&values)));
        true
    }

    /// The vertex buffer slots this binds, for the pipeline.
    pub fn vertex_buffers(&self) -> VertexLayout {
        vertex_buffers(&self.layout)
    }

    /// [`Self::vertex_buffers`] for instanced draws of up to [`PAINT_ZERO_VERTICES`]
    /// vertices with a value per instance: the values step per instance, and the zeros
    /// per vertex, since stepping them per instance would read past their end.
    pub fn instance_vertex_buffers(&self) -> VertexLayout {
        let mut layout = self.vertex_buffers();
        for b in &mut layout {
            b.step = match b.step {
                StepMode::Vertex => StepMode::Instance,
                StepMode::Instance => StepMode::Vertex,
            };
        }
        layout
    }

    /// Bind the paint slots from `first_slot` on.
    pub fn bind(&self, gpu: &Gpu, pass: &mut Pass<'_>, first_slot: u32) {
        self.bind_from(gpu, pass, first_slot, 0);
    }

    /// [`Self::bind`], with the per-vertex values from vertex `first_vertex` on: for an
    /// index batch.
    pub fn bind_from(&self, gpu: &Gpu, pass: &mut Pass<'_>, first_slot: u32, first_vertex: u32) {
        let mut slot = first_slot;
        if let Some(buffer) = &self.buffer {
            let floats: u32 = self.layout.attributes.iter().map(repacked_floats).sum();
            pass.set_vertex_buffer(slot, buffer.slice_from(first_vertex, floats as usize * 4));
            slot += 1;
        }
        if self
            .layout
            .attributes
            .iter()
            .any(|a| a.binding == Binding::Uniform)
        {
            pass.set_vertex_buffer(slot, gpu.paint_zeros.slice());
        }
    }

    pub fn signature(&self) -> u32 {
        self.layout.signature()
    }
}

/// For [`Slice`] users that only have bytes.
pub fn bytes<T: bytemuck::Pod>(values: &[T]) -> Slice<'_> {
    Slice::Bytes(bytemuck::cast_slice(values))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sparkle::paint::PaintProperty;
    use sparkle::style::{Color, PropertyValue};

    use super::*;

    #[test]
    fn repacks_every_binding() {
        let color =
            PropertyValue::<Color>::from_json("line-color", &json!(["get", "c"]), Color::BLACK)
                .unwrap();
        let opacity = PropertyValue::<f32>::from_json("line-opacity", &json!(0.5), 1.0).unwrap();
        let width = PropertyValue::<f32>::from_json(
            "line-width",
            &json!(["interpolate", ["linear"], ["zoom"], 1, ["get", "w"], 2, 3]),
            1.0,
        )
        .unwrap();
        let props: [(&str, &dyn PaintProperty); 3] =
            [("color", &color), ("opacity", &opacity), ("width", &width)];
        let mut paint = PaintArrays::new(&props, 1.0);
        let mut ctx = maplibre_expr::EvaluationContext::new().with_zoom(1.0);
        ctx.feature
            .properties
            .insert("c".into(), maplibre_expr::Value::String("#ff0000".into()));
        ctx.feature
            .properties
            .insert("w".into(), maplibre_expr::Value::Number(5.0));
        paint.populate(&props, &mut ctx, None, 1);

        let layout = vertex_buffers(&paint);
        assert_eq!(layout.len(), 2);
        // Color as source (two vec4s), width as composite (one vec2).
        assert_eq!(layout[0].stride, (8 + 2) * 4);
        let names: Vec<_> = layout[0].attributes.iter().map(|a| a.0.as_str()).collect();
        assert_eq!(names, ["a_color_b", "a_color", "a_width"]);
        assert_eq!(layout[1].step, StepMode::Instance);
        assert_eq!(
            layout[1].attributes[0],
            ("a_opacity".into(), VertexFormat::Float32x2, 32)
        );

        let values = repack(&paint);
        assert_eq!(&values[..8], [1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0]);
        assert_eq!(values[8], 5.0);
        assert_eq!(values.len(), 10);
    }
}
