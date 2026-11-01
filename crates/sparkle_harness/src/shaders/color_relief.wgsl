// Coloring by elevation: each pixel's elevation, read from the tile's DEM (with its
// one pixel border, mapbox-encoded), looked up in the layer's color ramp.
//
// `extra[0]`: DEM pixels per side (without the border), ramp min elevation, ramp max
// elevation, opacity.
// `extra[1]`: `extent`, 1 to sample the nearest pixel instead of interpolating, ramp size.

@group(1) @binding(0) var dem: texture_2d<f32>;
@group(1) @binding(1) var ramp: texture_2d<f32>;
@group(1) @binding(2) var ramp_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    let corner = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    var out: VertexOut;
    out.position = to_clip(corner * draw.extra[1].x);
    out.uv = corner;
    return out;
}

// The elevation of a pixel of the bordered grid, in meters.
fn dem_elevation(p: vec2<i32>) -> f32 {
    let size = i32(draw.extra[0].x) + 2;
    let rgb = round(textureLoad(dem, clamp(p, vec2<i32>(0), vec2<i32>(size - 1)), 0).rgb * 255.0);
    return -10000.0 + (rgb.r * 65536.0 + rgb.g * 256.0 + rgb.b) * 0.1;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Pixel centers of the tile are at `i + 0.5`; the border shifts them by one.
    let p = in.uv * draw.extra[0].x - 0.5 + 1.0;
    var e: f32;
    if draw.extra[1].y > 0.5 {
        e = dem_elevation(vec2<i32>(floor(p + 0.5)));
    } else {
        let base = floor(p);
        let f = p - base;
        let i = vec2<i32>(base);
        let top = mix(dem_elevation(i), dem_elevation(i + vec2<i32>(1, 0)), f.x);
        let bottom = mix(dem_elevation(i + vec2<i32>(0, 1)), dem_elevation(i + vec2<i32>(1, 1)), f.x);
        e = mix(top, bottom, f.y);
    }
    let lo = draw.extra[0].y;
    let hi = draw.extra[0].z;
    let n = draw.extra[1].z;
    let t = clamp((e - lo) / max(hi - lo, 1e-6), 0.0, 1.0);
    let u = (t * (n - 1.0) + 0.5) / n;
    return textureSampleLevel(ramp, ramp_sampler, vec2<f32>(u, 0.5), 0.0) * draw.extra[0].w;
}
