// Roofs and walls of extruded polygons, lit as MapLibre lights them (see
// `sparkle::light::ExtrusionLight::shade`), drawn into the layer's offscreen target: what
// `fill_extrusion_roof.wgsl` and `fill_extrusion_wall.wgsl` share. Each is appended to
// this file.
//
// `extra[0]`: the light's position (xyz, tile coordinates) and intensity (w).
// `extra[1].rgb`: the light's color.
// `extra[2]`: `fill-extrusion-translate` in tile units (xy), logical pixels per meter (z),
//             1 for `fill-extrusion-vertical-gradient` (w).
// `extra[3].x`: tile units per pattern pixel.

@group(1) @binding(0) var pattern_atlas: texture_2d<f32>;
@group(1) @binding(1) var pattern_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    // Opaque and premultiplied; with a pattern, the lighting to multiply it by.
    @location(0) color: vec4<f32>,
    // In pattern pixels: tile position on roofs, distance along and up the wall on walls.
    @location(1) pattern_pos: vec2<f32>,
    @location(2) @interpolate(flat) pattern_rect: vec4<f32>,
    @location(3) @interpolate(flat) pattern_size: vec2<f32>,
    // Where on the ground the fragment is, in (untranslated) tile coordinates.
    @location(4) tile_pos: vec2<f32>,
};

// A vertex at `pos` (tile coordinates), `edge` along its wall's ring, with unit `normal`
// (`[0, 0, 1]` on roofs), at the feature's `height` (`top` 1) or `base` (`top` 0), both
// in meters and at least 0. Roofs, at the height, don't read the base.
fn extrude(
    pos: vec2<f32>,
    edge: f32,
    normal: vec3<f32>,
    top: f32,
    base: f32,
    height: f32,
    pattern_rect: vec4<f32>,
    pattern_size: vec2<f32>,
    p: PaintIn,
) -> VertexOut {
    let z = select(base, height, top > 0.5) * draw.extra[2].z;

    var out: VertexOut;
    out.position = to_clip_above(pos + draw.extra[2].xy, z);

    let light_pos = draw.extra[0].xyz;
    let intensity = draw.extra[0].w;
    let light_color = draw.extra[1].rgb;
    let has_pattern = pattern_rect.z > 0.0;
    let color = paint_color(p);
    let luminance = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    // MapLibre's normals reach its dot product doubled (packing), hence the 2.
    var directional = clamp(2.0 * dot(normal, light_pos), 0.0, 1.0);
    let upper = select(max(1.0 - luminance + intensity, 1.0), max(0.5 + intensity, 1.0), has_pattern);
    directional = mix(1.0 - intensity, upper, directional);
    if normal.z == 0.0 && draw.extra[2].w > 0.5 {
        directional *= clamp((top + base) * sqrt(height / 150.0), mix(0.7, 0.98, 1.0 - intensity), 1.0);
    }
    let surface = select(color.rgb + vec3<f32>(0.03), vec3<f32>(1.0), has_pattern);
    let lit = clamp(
        surface * directional * light_color,
        mix(vec3<f32>(0.0), vec3<f32>(0.3), 1.0 - light_color),
        vec3<f32>(1.0),
    );
    out.color = vec4<f32>(lit, 1.0);

    let units_per_pattern_pixel = draw.extra[3].x;
    if normal.z == 0.0 {
        out.pattern_pos = vec2<f32>(edge, -z * draw.units_per_pixel) / units_per_pattern_pixel;
    } else {
        out.pattern_pos = pos / units_per_pattern_pixel;
    }
    out.pattern_rect = pattern_rect;
    out.pattern_size = pattern_size;
    out.tile_pos = pos;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Buildings that cross the tile's edge are in both tiles, clipped to their buffers.
    // Cut each copy at the edge, as a vertical plane, so the copies don't overlap: their
    // walls are slightly different (clipped vertices are rounded) and would z-fight.
    if any(in.tile_pos < vec2<f32>(0.0)) || any(in.tile_pos > vec2<f32>(draw.extent)) {
        discard;
    }
    if in.pattern_rect.z <= 0.0 {
        return in.color;
    }
    // Repeat the image, sampling only inside its own rectangle of the atlas.
    let rect = in.pattern_rect;
    let texel = rect.xy + fract(in.pattern_pos / in.pattern_size) * rect.zw;
    let clamped = clamp(texel, rect.xy + 0.5, rect.xy + rect.zw - 0.5);
    let uv = clamped / vec2<f32>(textureDimensions(pattern_atlas));
    return textureSampleLevel(pattern_atlas, pattern_sampler, uv, 0.0) * in.color;
}
