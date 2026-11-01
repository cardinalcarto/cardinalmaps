//! Generated WGSL and vertex layouts for paint attributes (see [`sparkle::paint`]).
//!
//! For a bucket's [`PaintArrays`] this generates:
//!
//! ```wgsl
//! struct PaintIn {
//!     @builtin(vertex_index) vertex_index: u32,
//!     @location(4) a_width: vec2<f32>,     // one per non-uniform attribute
//! };
//! fn paint_color(p: PaintIn) -> vec4<f32> { return draw.paint.values[0]; }
//! fn paint_width(p: PaintIn) -> f32 { return mix(p.a_width.x, p.a_width.y, draw.paint.factors[0][1]); }
//! ```
//!
//! A layer's vertex shader takes `p: PaintIn` as an extra parameter and calls
//! `paint_<name>(p)` for each attribute, without knowing its binding. `PaintIn` always has
//! `vertex_index`, so shaders read it from there rather than declaring it themselves.

use std::fmt::Write;

use sparkle::paint::{Binding, PaintArrays, PaintAttribute};

/// The WGSL type of a value of `components` floats.
fn wgsl_type(components: u32) -> &'static str {
    match components {
        1 => "f32",
        2 => "vec2<f32>",
        _ => "vec4<f32>",
    }
}

/// The vertex attributes of one paint attribute: `(location offset, format, byte offset)`.
fn attribute_formats(a: &PaintAttribute) -> Vec<(wgpu::VertexFormat, u64)> {
    use wgpu::VertexFormat::*;
    let offset = u64::from(a.offset) * 4;
    match (a.binding, a.components) {
        (Binding::Uniform, _) => vec![],
        (Binding::Source, 1) => vec![(Float32, offset)],
        (Binding::Source, 2) | (Binding::Composite, 1) => vec![(Float32x2, offset)],
        (Binding::Source, _) | (Binding::Composite, 2) => vec![(Float32x4, offset)],
        (Binding::Composite, _) => vec![(Float32x4, offset), (Float32x4, offset + 16)],
    }
}

/// The vertex attributes of a bucket's paint vertex buffer, from `first_location` on.
pub fn vertex_attributes(paint: &PaintArrays, first_location: u32) -> Vec<wgpu::VertexAttribute> {
    let mut location = first_location;
    let mut out = Vec::new();
    for a in &paint.attributes {
        for (format, offset) in attribute_formats(a) {
            out.push(wgpu::VertexAttribute {
                format,
                offset,
                shader_location: location,
            });
            location += 1;
        }
    }
    out
}

/// `PaintIn` and the `paint_*` accessors for `paint`'s layout.
pub fn wgsl(paint: &PaintArrays, first_location: u32) -> String {
    let mut inputs =
        String::from("struct PaintIn {\n    @builtin(vertex_index) vertex_index: u32,\n");
    let mut functions = String::new();
    let mut location = first_location;
    for (i, a) in paint.attributes.iter().enumerate() {
        let ty = wgsl_type(a.components);
        let name = a.name;
        let swizzle = match a.components {
            1 => ".x",
            2 => ".xy",
            _ => "",
        };
        let factor = format!("draw.paint.factors[{}][{}]", i / 4, i % 4);
        let body = match (a.binding, a.components) {
            (Binding::Uniform, _) => format!("draw.paint.values[{i}]{swizzle}"),
            (Binding::Source, _) => format!("p.a_{name}"),
            (Binding::Composite, 1) => format!("mix(p.a_{name}.x, p.a_{name}.y, {factor})"),
            (Binding::Composite, 2) => format!("mix(p.a_{name}.xy, p.a_{name}.zw, {factor})"),
            (Binding::Composite, _) => format!("mix(p.a_{name}, p.a_{name}_b, {factor})"),
        };
        let formats = attribute_formats(a);
        for (k, (format, _)) in formats.iter().enumerate() {
            let field_ty = match format {
                wgpu::VertexFormat::Float32 => "f32",
                wgpu::VertexFormat::Float32x2 => "vec2<f32>",
                _ => "vec4<f32>",
            };
            let suffix = if k == 0 { "" } else { "_b" };
            let _ = writeln!(
                inputs,
                "    @location({location}) a_{name}{suffix}: {field_ty},"
            );
            location += 1;
        }
        let _ = writeln!(
            functions,
            "fn paint_{name}(p: PaintIn) -> {ty} {{\n    return {body};\n}}"
        );
    }
    inputs.push_str("};\n");
    inputs + &functions
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sparkle::paint::PaintProperty;
    use sparkle::style::{Color, PropertyValue};

    use super::*;

    #[test]
    fn generates_every_binding() {
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
        let fill = PropertyValue::<Color>::from_json(
            "fill-color",
            &json!([
                "interpolate",
                ["linear"],
                ["zoom"],
                1,
                ["get", "c"],
                2,
                "red"
            ]),
            Color::BLACK,
        )
        .unwrap();
        let props: [(&str, &dyn PaintProperty); 4] = [
            ("color", &color),
            ("opacity", &opacity),
            ("width", &width),
            ("fill", &fill),
        ];
        let paint = PaintArrays::new(&props, 1.0);
        let code = wgsl(&paint, 4);
        assert!(code.contains("@location(4) a_color: vec4<f32>"), "{code}");
        assert!(code.contains("return draw.paint.values[1].x;"), "{code}");
        assert!(code.contains("@location(5) a_width: vec2<f32>"), "{code}");
        assert!(code.contains("mix(p.a_width.x, p.a_width.y, draw.paint.factors[0][2])"));
        assert!(code.contains("@location(7) a_fill_b: vec4<f32>"), "{code}");
        let attributes = vertex_attributes(&paint, 4);
        assert_eq!(attributes.len(), 4);
        assert_eq!(attributes[3].offset, (4 + 2 + 4) * 4);
    }
}
