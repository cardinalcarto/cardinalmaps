// The color pass: looks the accumulated density up in the `heatmap-color` ramp, over the
// whole viewport.
//
// `extra[0].x`: `heatmap-opacity`.

@group(1) @binding(0) var density: texture_2d<f32>;
@group(1) @binding(1) var ramp: texture_2d<f32>;
@group(1) @binding(2) var filtered: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_triangle(i);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let t = textureSample(density, filtered, frag.xy / draw.viewport).r;
    // The ramp's first and last texel centers are densities 0 and 1.
    let n = f32(textureDimensions(ramp).x);
    let u = (clamp(t, 0.0, 1.0) * (n - 1.0) + 0.5) / n;
    return textureSample(ramp, filtered, vec2<f32>(u, 0.5)) * draw.extra[0].x;
}
