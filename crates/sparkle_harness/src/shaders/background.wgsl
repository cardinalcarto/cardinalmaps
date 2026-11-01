// `background` layers: a full-screen color, or a `background-pattern` drawn tile by tile
// so it stays fixed to the ground. Group 1 is the sprite atlas for patterns.
//
// `extra[0]`: the premultiplied color, opacity applied.
// `extra[1]`: x 1 to draw a pattern over the tile `matrix` places (`0..1`), y the
//     cross-fade from pattern a to b, z the opacity.
// `extra[2]`, `extra[3]`: patterns a and b, `[x, y, width, height]` in atlas pixels.
// `extra[4]`, `extra[5]`: where patterns a and b start in the tile (xy, in repeats) and
//     repeats per tile unit (zw); see `sparkle::pattern::pattern_placement`.
// `extra[7]`: xy the atlas size.

@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    // Pattern coordinates for a (xy) and b (zw), in repeats.
    @location(0) uv: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    var out: VertexOut;
    if draw.extra[1].x > 0.5 {
        // Two triangles over the tile.
        let corners = array<vec2<f32>, 6>(
            vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
            vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
        );
        let pos = corners[i];
        out.position = to_clip(pos);
        out.uv = vec4<f32>(
            draw.extra[4].xy + pos * draw.extra[4].zw,
            draw.extra[5].xy + pos * draw.extra[5].zw,
        );
    } else {
        out.position = fullscreen_triangle(i);
    }
    return out;
}

fn sample_pattern(rect: vec4<f32>, uv: vec2<f32>) -> vec4<f32> {
    let texel = clamp(fract(uv) * rect.zw, vec2<f32>(0.5), rect.zw - 0.5);
    return textureSampleLevel(atlas, atlas_sampler, (rect.xy + texel) / draw.extra[7].xy, 0.0);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if draw.extra[1].x > 0.5 {
        let a = sample_pattern(draw.extra[2], in.uv.xy);
        let b = sample_pattern(draw.extra[3], in.uv.zw);
        return mix(a, b, draw.extra[1].y) * draw.extra[1].z;
    }
    return draw.extra[0];
}
