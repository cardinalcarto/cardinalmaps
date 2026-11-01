// `extra[0]`: opacity, the weight of the image faded from, brightness min, brightness max.
// `extra[1]`: saturation factor, contrast factor.
// `extra[2]`: hue rotation weights.
// `extra[3]`: scale and offset (x, y) from this image's texture coordinates to the one
// faded from.

@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var fade_from: texture_2d<f32>;
@group(1) @binding(2) var image_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fade_uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOut {
    var out: VertexOut;
    out.position = to_clip(position);
    out.uv = texcoord;
    out.fade_uv = texcoord * draw.extra[3].x + draw.extra[3].yz;
    return out;
}

fn unpremultiply(c: vec4<f32>) -> vec4<f32> {
    if c.a > 0.0 {
        return vec4<f32>(c.rgb / c.a, c.a);
    }
    return c;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let own = unpremultiply(textureSample(image, image_sampler, in.uv));
    let faded = unpremultiply(textureSample(fade_from, image_sampler, in.fade_uv));
    var color = mix(own, faded, draw.extra[0].y);
    color.a *= draw.extra[0].x;

    let spin = draw.extra[2].xyz;
    var rgb = vec3<f32>(dot(color.rgb, spin.xyz), dot(color.rgb, spin.zxy), dot(color.rgb, spin.yzx));
    let average = (color.r + color.g + color.b) / 3.0;
    rgb += (average - rgb) * draw.extra[1].x;
    rgb = (rgb - 0.5) * draw.extra[1].y + 0.5;
    let low = vec3<f32>(draw.extra[0].z);
    let high = vec3<f32>(draw.extra[0].w);
    return vec4<f32>(mix(low, high, rgb) * color.a, color.a);
}
