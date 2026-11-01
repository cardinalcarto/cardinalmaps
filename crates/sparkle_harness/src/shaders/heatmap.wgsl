// The density pass: a Gaussian kernel per point, summed with additive blending into an
// offscreen texture. Follows MapLibre's heatmap shaders, including their constants.
//
// `extra[0].x`: `heatmap-intensity`.

// Kernel values below this don't register in the density texture, so each point's quad
// only reaches as far as its kernel is larger. One step of an 8-bit target, divided again
// by 16 as MapLibre does to keep low weights visible.
const ZERO: f32 = 1.0 / 255.0 / 16.0;
// 1 / sqrt(2 pi): the normal distribution's coefficient.
const GAUSS_COEF: f32 = 0.3989422804014327;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    // The position in the kernel, in standard deviations (the radius is three of them).
    @location(0) extrude: vec2<f32>,
    @location(1) weight: f32,
};

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) corner: vec2<f32>, p: PaintIn) -> VertexOut {
    let weight = paint_weight(p);
    let radius = paint_radius(p);
    let intensity = draw.extra[0].x;
    var out: VertexOut;
    out.weight = weight;
    let peak = weight * intensity * GAUSS_COEF;
    if peak <= ZERO || radius <= 0.0 || globe_hidden(pos) {
        // Nothing to add: collapse the quad.
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        out.extrude = vec2<f32>(0.0);
        return out;
    }
    // How far out, relative to the radius, the kernel stays above ZERO.
    let s = sqrt(-2.0 * log(ZERO / peak)) / 3.0;
    out.extrude = s * corner * 3.0;
    let offset = s * corner * radius * draw.units_per_pixel;
    out.position = to_clip(pos + offset);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let d = -0.5 * dot(in.extrude, in.extrude);
    let value = in.weight * draw.extra[0].x * GAUSS_COEF * exp(d);
    return vec4<f32>(value, 1.0, 1.0, 1.0);
}
