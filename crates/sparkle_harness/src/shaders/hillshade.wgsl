// Hillshading from a tile's slopes (`sparkle::dem::DemData::hillshade_slopes`), with
// MapLibre's methods.
//
// `extra[0]`: exaggeration, method (0 standard, 1 combined, 2 igor, 3 multidirectional,
// 4 basic), light count, `extent`.
// `extra[1]`: the latitudes of the tile's top and bottom edges in radians, 1 to sample the
// nearest pixel instead of interpolating, pixels per side.
// `extra[2]`: the premultiplied accent color.
// `extra[3..5]`: light azimuths in radians, four per vec4.
// `extra[5..7]`: light altitudes in radians, four per vec4.
// `paint.values[0..6]`: premultiplied shadow colors, `paint.values[6..12]` highlight colors.

const MAX_LIGHTS: u32 = 6u;

@group(1) @binding(0) var slopes: texture_2d<f32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    let corner = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    var out: VertexOut;
    out.position = to_clip(corner * draw.extra[0].w);
    out.uv = corner;
    return out;
}

fn azimuth(i: u32) -> f32 {
    return draw.extra[3u + i / 4u][i % 4u];
}

fn altitude(i: u32) -> f32 {
    return draw.extra[5u + i / 4u][i % 4u];
}

fn shadow(i: u32) -> vec4<f32> {
    return draw.paint.values[i];
}

fn highlight(i: u32) -> vec4<f32> {
    return draw.paint.values[MAX_LIGHTS + i];
}

// GLSL's `mod`, which floors.
fn fmod(x: f32, y: f32) -> f32 {
    return x - y * floor(x / y);
}

fn get_aspect(d: vec2<f32>) -> f32 {
    if d.x != 0.0 {
        return atan2(d.y, -d.x);
    }
    return PI / 2.0 * select(-1.0, 1.0, d.y > 0.0);
}

fn standard(d: vec2<f32>) -> vec4<f32> {
    let exaggeration = draw.extra[0].x;
    let az = azimuth(0u) + PI;
    let slope = atan(0.625 * length(d));
    let aspect = get_aspect(d);
    // Steeper slopes shade more as the exaggeration grows, along an exponential curve.
    let base = max(1.875 - exaggeration * 1.75, 1e-4);
    let max_value = 0.5 * PI;
    var scaled = slope;
    if abs(exaggeration - 0.5) > 1e-6 {
        scaled = ((pow(base, slope) - 1.0) / (pow(base, max_value) - 1.0)) * max_value;
    }
    let accent = cos(scaled);
    let k = clamp(exaggeration * 2.0, 0.0, 1.0);
    let accent_color = (1.0 - accent) * draw.extra[2] * k;
    let shade = abs(fmod((aspect + az) / PI + 0.5, 2.0) - 1.0);
    let shade_color = mix(shadow(0u), highlight(0u), shade) * sin(scaled) * k;
    return accent_color * (1.0 - shade_color.a) + shade_color;
}

// How directly a light at `az`, `alt` shines on ground sloping by `d`: 1 facing it, 0
// edge on.
fn incidence(d: vec2<f32>, az: f32, alt: f32) -> f32 {
    let ca = cos(az);
    let sa = sin(az);
    let cl = cos(alt);
    let sl = sin(alt);
    return (sl - (d.y * ca * cl - d.x * sa * cl)) / sqrt(1.0 + dot(d, d));
}

fn combined(d0: vec2<f32>) -> vec4<f32> {
    let d = d0 * draw.extra[0].x * 2.0;
    let cang = clamp(acos(clamp(incidence(d, azimuth(0u) + PI, altitude(0u)), -1.0, 1.0)), 0.0, PI / 2.0);
    let w = atan(length(d)) * 4.0 / (PI * PI);
    return shadow(0u) * (cang * w) + highlight(0u) * ((PI / 2.0 - cang) * w);
}

fn igor(d0: vec2<f32>) -> vec4<f32> {
    let d = d0 * draw.extra[0].x * 2.0;
    let aspect = get_aspect(d);
    let az = azimuth(0u) + PI;
    let slope_strength = atan(length(d)) * 2.0 / PI;
    let aspect_strength = 1.0 - abs(fmod((aspect + az) / PI + 0.5, 2.0) - 1.0);
    let shadow_strength = slope_strength * aspect_strength;
    let highlight_strength = slope_strength * (1.0 - aspect_strength);
    return shadow(0u) * shadow_strength + highlight(0u) * highlight_strength;
}

fn directional(d: vec2<f32>, i: u32) -> vec4<f32> {
    let s = clamp(incidence(d, azimuth(i) + PI, altitude(i)), 0.0, 1.0);
    if s > 0.5 {
        return highlight(i) * (2.0 * s - 1.0);
    }
    return shadow(i) * (1.0 - 2.0 * s);
}

fn multidirectional(d0: vec2<f32>) -> vec4<f32> {
    let d = d0 * draw.extra[0].x * 2.0;
    let count = u32(draw.extra[0].z);
    var color = vec4<f32>(0.0);
    for (var i = 0u; i < count; i++) {
        color += directional(d, i) / f32(count);
    }
    return color;
}

// The east and south slopes of a pixel, 16 bits each (`sparkle::dem::decode_slopes`).
fn slope_at(p: vec2<i32>) -> vec2<f32> {
    let size = i32(draw.extra[1].w);
    let c = round(textureLoad(slopes, clamp(p, vec2<i32>(0), vec2<i32>(size - 1)), 0) * 255.0);
    return (vec2<f32>(c.r * 256.0 + c.g, c.b * 256.0 + c.a) / 65535.0 - 0.5) * 8.0;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Interpolated by hand: filtering the bytes of a 16-bit value apart would be wrong.
    let p = in.uv * draw.extra[1].w - 0.5;
    var slope: vec2<f32>;
    if draw.extra[1].z > 0.5 {
        slope = slope_at(vec2<i32>(floor(p + 0.5)));
    } else {
        let base = floor(p);
        let f = p - base;
        let i = vec2<i32>(base);
        let top = mix(slope_at(i), slope_at(i + vec2<i32>(1, 0)), f.x);
        let bottom = mix(slope_at(i + vec2<i32>(0, 1)), slope_at(i + vec2<i32>(1, 1)), f.x);
        slope = mix(top, bottom, f.y);
    }
    // Slopes are computed with the meters per pixel at the equator; they're steeper by
    // 1 / cos(latitude) here.
    let lat = mix(draw.extra[1].x, draw.extra[1].y, in.uv.y);
    let d = slope / cos(lat);
    switch u32(draw.extra[0].y) {
        case 1u: { return combined(d); }
        case 2u: { return igor(d); }
        case 3u: { return multidirectional(d); }
        case 4u: { return directional(d * draw.extra[0].x * 2.0, 0u); }
        default: { return standard(d); }
    }
}
