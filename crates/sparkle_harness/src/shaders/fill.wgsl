// Fills and their outlines, with a color or a `fill-pattern`. Group 1 is the sprite atlas
// for patterns.
//
// `extra[0]`: x 1 for the outline pass, y 1 when `fill-outline-color` is set, z the mode
//     (0 color, 1 the layer's pattern from `extra`, 2 a pattern per vertex), w the
//     cross-fade from pattern a to b.
// `extra[1]`: xy `fill-translate` in tile units, z `fill-layer-opacity`, w the cross-fade
//     scale of pattern a for patterns per vertex.
// `extra[2]`, `extra[3]`: patterns a and b, `[x, y, width, height]` in atlas pixels.
// `extra[4]`, `extra[5]`: where patterns a and b start in the tile (xy, in repeats) and
//     repeats per tile unit (zw); see `sparkle::pattern::pattern_placement`.
// `extra[6]`: for patterns per vertex, the tile's origin in pixels of the zoom patterns
//     are laid out at, split for precision: xy whole multiples of 65536, zw the rest.
// `extra[7]`: xy the atlas size, z pixels per tile unit at the zoom patterns are laid out
//     at, w the cross-fade scale of pattern b for patterns per vertex.

@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    // Premultiplied; just the opacity when drawing a pattern.
    @location(0) color: vec4<f32>,
    // Pattern coordinates for a (xy) and b (zw), in repeats.
    @location(1) uv: vec4<f32>,
    @location(2) @interpolate(flat) rect_a: vec4<f32>,
    @location(3) @interpolate(flat) rect_b: vec4<f32>,
    @location(4) @interpolate(flat) fade: f32,
};

fn modulo(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return a - b * floor(a / b);
}

// `offset` modulo `size`, for an offset given as `upper * 65536 + lower`, without the
// precision a float that large would lose.
fn wrap(upper: vec2<f32>, lower: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
    return modulo(modulo(modulo(upper, size) * 256.0, size) * 256.0 + lower, size);
}

@vertex
fn vs_main(
    @location(0) pos: vec2<f32>,
    @location(1) feature_a: vec4<f32>,
    @location(2) feature_b: vec4<f32>,
    p: PaintIn,
) -> VertexOut {
    let outline = draw.extra[0].x > 0.5;
    let mode = draw.extra[0].z;
    var color = paint_color(p);
    if outline && draw.extra[0].y > 0.5 {
        color = paint_outline_color(p);
    }
    let opacity = paint_opacity(p) * draw.extra[1].z;

    var out: VertexOut;
    out.position = to_clip(pos + draw.extra[1].xy);
    out.color = color * opacity;
    out.fade = draw.extra[0].w;
    // An outline with its own color is drawn in that color, even over a pattern.
    if mode > 0.5 && !(outline && draw.extra[0].y > 0.5) {
        out.color = vec4<f32>(opacity);
        if mode < 1.5 {
            out.rect_a = draw.extra[2];
            out.rect_b = draw.extra[3];
            out.uv = vec4<f32>(
                draw.extra[4].xy + pos * draw.extra[4].zw,
                draw.extra[5].xy + pos * draw.extra[5].zw,
            );
        } else {
            // The feature's own image, fading into itself at the next zoom's scale.
            let size = max(feature_a.zw / max(feature_b.x, 1e-6), vec2<f32>(1e-3));
            let size_a = size * draw.extra[1].w;
            let size_b = size * draw.extra[7].w;
            let pixels = pos * draw.extra[7].z;
            out.uv = vec4<f32>(
                (wrap(draw.extra[6].xy, draw.extra[6].zw, size_a) + pixels) / size_a,
                (wrap(draw.extra[6].xy, draw.extra[6].zw, size_b) + pixels) / size_b,
            );
            out.rect_a = feature_a;
            out.rect_b = feature_a;
            if feature_a.z <= 0.0 {
                // The feature's image is missing: draw nothing, as MapLibre does.
                out.color = vec4<f32>(0.0);
            }
        }
    }
    return out;
}

// One repeat of an atlas image at `uv`, kept half a texel inside it so that filtering
// doesn't reach the neighbours.
fn sample_pattern(rect: vec4<f32>, uv: vec2<f32>) -> vec4<f32> {
    if rect.z <= 0.0 || rect.w <= 0.0 {
        return vec4<f32>(0.0);
    }
    let texel = clamp(fract(uv) * rect.zw, vec2<f32>(0.5), rect.zw - 0.5);
    return textureSampleLevel(atlas, atlas_sampler, (rect.xy + texel) / draw.extra[7].xy, 0.0);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if in.rect_a.z > 0.0 || in.rect_b.z > 0.0 {
        let a = sample_pattern(in.rect_a, in.uv.xy);
        let b = sample_pattern(in.rect_b, in.uv.zw);
        return mix(a, b, in.fade) * in.color.a;
    }
    return in.color;
}
