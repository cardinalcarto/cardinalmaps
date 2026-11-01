// Prepended to every layer shader. Matches `sparkle::paint::PaintUniforms`.
struct PaintUniforms {
    // One per paint attribute, for uniform bindings: scalars in `.x`, pairs in `.xy`.
    values: array<vec4<f32>, 12>,
    // Composite interpolation factors, four to a vec4, in attribute order.
    factors: array<vec4<f32>, 3>,
};

// Per-draw values, at a dynamic offset into a uniform buffer. Matches `DrawUniforms`.
struct Draw {
    // Tile coordinates (`0..extent`; z in logical pixels) to clip space.
    matrix: mat4x4<f32>,
    // Tile units per logical pixel.
    units_per_pixel: f32,
    pixel_ratio: f32,
    zoom: f32,
    extent: f32,
    // In physical pixels.
    viewport: vec2<f32>,
    // In radians.
    bearing: f32,
    pitch: f32,
    // Whatever a layer type needs besides its paint attributes.
    extra: array<vec4<f32>, 8>,
    paint: PaintUniforms,
    // Positions relative to the center of the view (logical pixels, x east, y south, z up)
    // to clip space, for the globe.
    view: mat4x4<f32>,
    // The tile `matrix` places: column, row, tiles per side, extent.
    tile: vec4<f32>,
    // From Mercator to the globe (0..1), the globe's radius in logical pixels, and the
    // center's longitude and latitude in radians.
    globe: vec4<f32>,
    // Logical pixels per meter of terrain (0 without), the DEM's layer in `dem_atlas`,
    // and its pixels per side without the border.
    terrain: vec4<f32>,
    // The tile's offset (xy) and size (z) in its DEM tile, as fractions of it.
    dem: vec4<f32>,
    // The clip w of the center and of the horizon, fog-ground-blend, fog opacity.
    fog: vec4<f32>,
    fog_color: vec4<f32>,
    // The camera in `view`'s frame.
    eye: vec4<f32>,
};

@group(0) @binding(0) var<uniform> draw: Draw;
// DEM tiles as `sparkle::dem::DemData::to_rgba` makes them, border included, one per layer.
@group(0) @binding(1) var dem_atlas: texture_2d_array<f32>;

const PI: f32 = 3.141592653589793;

fn dem_texel(p: vec2<i32>) -> f32 {
    let last = i32(draw.terrain.z) + 1;
    let c = round(textureLoad(dem_atlas, clamp(p, vec2<i32>(0), vec2<i32>(last)), i32(draw.terrain.y), 0) * 255.0);
    return -10000.0 + (c.r * 65536.0 + c.g * 256.0 + c.b) * 0.1;
}

// The terrain's height under `pos` (tile units) in logical pixels, sampled bilinearly
// between pixel centers as `sparkle::dem::DemData::elevation_at` does. 0 without terrain.
fn elevation(pos: vec2<f32>) -> f32 {
    if draw.terrain.x <= 0.0 {
        return 0.0;
    }
    let uv = draw.dem.xy + pos / draw.tile.w * draw.dem.z;
    let xy = uv * draw.terrain.z - 0.5;
    let base = floor(xy);
    let f = xy - base;
    // Pixel (0, 0) is at (1, 1) behind the border.
    let p = vec2<i32>(base) + vec2<i32>(1);
    let top = mix(dem_texel(p), dem_texel(p + vec2<i32>(1, 0)), f.x);
    let bottom = mix(dem_texel(p + vec2<i32>(0, 1)), dem_texel(p + vec2<i32>(1, 1)), f.x);
    return mix(top, bottom, f.y) * draw.terrain.x;
}

// World coordinates (0..1, y down) of a point of the tile.
fn tile_world(pos: vec2<f32>) -> vec2<f32> {
    return (draw.tile.xy + pos / draw.tile.w) / draw.tile.z;
}

// Where world point `world`, `height` logical pixels up, is on the globe relative to the
// center of the view, as `sparkle::projection::Globe::position`.
fn globe_position_of(world: vec2<f32>, height: f32) -> vec3<f32> {
    let lng = world.x * 2.0 * PI - PI;
    let lat = 2.0 * atan(exp(PI - 2.0 * PI * world.y)) - PI / 2.0;
    let lat0 = draw.globe.w;
    let dl = lng - draw.globe.z;
    let east = cos(lat) * sin(dl);
    let north = sin(lat) * cos(lat0) - cos(lat) * sin(lat0) * cos(dl);
    let up = sin(lat) * sin(lat0) + cos(lat) * cos(lat0) * cos(dl);
    let r = draw.globe.y + height;
    return vec3<f32>(r * east, -r * north, r * up - draw.globe.y);
}

fn globe_position(pos: vec2<f32>, height: f32) -> vec3<f32> {
    return globe_position_of(tile_world(pos), height);
}

// A tile point `height` logical pixels above sea level in clip space: on the Mercator
// plane through `matrix`, on the globe, or blended between them.
fn project(pos: vec2<f32>, height: f32) -> vec4<f32> {
    let flat = draw.matrix * vec4<f32>(pos, height, 1.0);
    let t = draw.globe.x;
    if t <= 0.0 {
        return flat;
    }
    let sphere = draw.view * vec4<f32>(globe_position(pos, height), 1.0);
    return mix(flat, sphere, t);
}

// Whether a tile point is on the far side of the globe, hidden behind it.
fn globe_hidden(pos: vec2<f32>) -> bool {
    if draw.globe.x < 0.5 {
        return false;
    }
    let p = globe_position(pos, 0.0);
    let normal = p + vec3<f32>(0.0, 0.0, draw.globe.y);
    return dot(normal, draw.eye.xyz - p) < -1e-3 * draw.globe.y * draw.globe.y;
}

// A tile point on the ground (on the terrain, if any) in clip space.
fn to_clip(pos: vec2<f32>) -> vec4<f32> {
    return project(pos, elevation(pos));
}

// A tile point `height` logical pixels above the ground in clip space.
fn to_clip_above(pos: vec2<f32>, height: f32) -> vec4<f32> {
    return project(pos, height + elevation(pos));
}

// How far a clip position is into the fog, 0 (none) to 1.
fn fog_amount(w: f32) -> f32 {
    if draw.fog.w <= 0.0 {
        return 0.0;
    }
    let depth = (w - draw.fog.x) / max(draw.fog.y - draw.fog.x, 1e-3);
    let start = draw.fog.z;
    let f = clamp((depth - start) / max(1.0 - start, 1e-3), 0.0, 1.0);
    return f * f * draw.fog.w;
}

// A triangle covering the viewport, from `vertex_index` 0..3.
fn fullscreen_triangle(i: u32) -> vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}
