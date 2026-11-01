// Text and icons (see `sparkle::tile::SymbolBucket`). The renderer prepends either the
// text or the icon definitions of `IS_TEXT` and `layout_scale(p, flags)`.
//
// `extra[0]`: is halo pass, pitch with map, rotate with map (each 0 or 1), and the
// `sparkle::symbol::placement::DrawMode`: 0 point, 1 along a line on the map, 2 along a
// line on screen.
// `extra[1].xy`: `*-translate` in tile units (point labels); `.z`: the clip w of the
// view's center.

@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

// `SymbolVertex::flags`
const SDF: u32 = 1u;
const SECTION_COLOR: u32 = 2u;

// Where the outline is in a distance field, and how many of its units are one glyph
// pixel (see `sparkle::glyphs`).
const EDGE: f32 = 0.75;
const SDF_PX: f32 = 8.0;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    // Normalized texture coordinates.
    @location(0) tex: vec2<f32>,
    // Premultiplied, opacity included.
    @location(1) color: vec4<f32>,
    // Whether to read a distance field, the distance at the edge to draw, and extra
    // smoothing for halo blur, in distance field units.
    @location(2) @interpolate(flat) sdf: vec3<f32>,
};

fn rotate(v: vec2<f32>, angle: f32) -> vec2<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

// Outside clip space, so the triangle is dropped.
const HIDDEN: vec4<f32> = vec4<f32>(2.0, 2.0, 2.0, 1.0);

@vertex
fn vs_main(
    @location(0) anchor: vec2<f32>,
    @location(1) offset: vec2<f32>,
    @location(2) pixel_offset: vec2<f32>,
    @location(3) tex: vec2<f32>,
    @location(4) section_color: vec4<f32>,
    @location(5) flags: u32,
    // From placement: see `sparkle::symbol::placement::DrawMode`. `.w` is the opacity.
    @location(6) placed: vec4<f32>,
    p: PaintIn,
) -> VertexOut {
    var out: VertexOut;
    if placed.w <= 0.0 || globe_hidden(anchor) {
        out.position = HIDDEN;
        return out;
    }
    let is_halo = draw.extra[0].x > 0.5;
    let pitch_with_map = draw.extra[0].y > 0.5;
    let rotate_with_map = draw.extra[0].z > 0.5;
    let mode = u32(draw.extra[0].w + 0.5);
    let center_w = draw.extra[1].z;

    let pos = anchor + draw.extra[1].xy;
    let projected = to_clip(pos);
    // Labels far away shrink only half as much as the map does, as in MapLibre: those
    // drawn in the map plane are enlarged and those facing the viewer shrunk.
    var distance_ratio = center_w / projected.w;
    if pitch_with_map {
        distance_ratio = projected.w / center_w;
    }
    let perspective = clamp(0.5 + 0.5 * distance_ratio, 0.0, 4.0);
    let scale = layout_scale(p, flags);

    if mode == 0u {
        // In logical pixels, y down, moved to the label's variable anchor.
        var shift = (offset + placed.xy) * scale * perspective + pixel_offset;
        if pitch_with_map {
            if !rotate_with_map {
                // Turn against the map so the label stays upright on screen.
                shift = rotate(shift, draw.bearing);
            }
            out.position = to_clip(pos + shift * draw.units_per_pixel);
        } else {
            if rotate_with_map {
                // Along the map's x axis as it appears on screen.
                let east = to_clip(pos + vec2<f32>(draw.units_per_pixel, 0.0));
                let d = (east.xy / east.w - projected.xy / projected.w) * vec2<f32>(1.0, -1.0);
                shift = rotate(shift, atan2(d.y * draw.viewport.y, d.x * draw.viewport.x));
            }
            let ndc = shift * draw.pixel_ratio * 2.0 / draw.viewport * vec2<f32>(1.0, -1.0);
            out.position = projected + vec4<f32>(ndc * projected.w, 0.0, 0.0);
        }
    } else {
        // A glyph placed along its line: centered on `placed.xy`, turned by `placed.z`.
        let shift = rotate(offset * scale * perspective + pixel_offset, placed.z);
        if mode == 1u {
            out.position = to_clip(placed.xy + shift * draw.units_per_pixel);
        } else {
            let screen = (placed.xy + shift) * draw.pixel_ratio / draw.viewport;
            let depth = clamp(projected.z / projected.w, 0.0, 1.0);
            out.position = vec4<f32>(screen.x * 2.0 - 1.0, 1.0 - screen.y * 2.0, depth, 1.0);
        }
    }
    out.tex = tex / vec2<f32>(textureDimensions(atlas));

    let opacity = paint_opacity(p) * placed.w;
    let is_sdf = (flags & SDF) != 0u;
    // Halo widths and blur are in pixels, distance field units are glyph pixels.
    let font_scale = max(scale * perspective, 1e-3);
    if is_halo {
        out.color = paint_halo_color(p) * opacity;
        let width = paint_halo_width(p);
        let edge = (6.0 - width / font_scale) / SDF_PX;
        out.sdf = vec3<f32>(1.0, edge, paint_halo_blur(p) * 1.19 / SDF_PX / font_scale);
        if !is_sdf || width <= 0.0 {
            out.position = HIDDEN;
        }
    } else {
        var color = paint_color(p);
        if (flags & SECTION_COLOR) != 0u {
            color = section_color;
        }
        if is_sdf {
            out.color = color * opacity;
            out.sdf = vec3<f32>(1.0, EDGE, 0.0);
        } else {
            out.color = vec4<f32>(opacity);
            out.sdf = vec3<f32>(0.0);
        }
    }
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(atlas, atlas_sampler, in.tex);
    // Glyph atlases have one channel; SDF icons keep the distance in alpha.
    var dist = texel.a;
    if IS_TEXT {
        dist = texel.r;
    }
    let aa = fwidth(dist) * 0.7;
    if in.sdf.x < 0.5 {
        return texel * in.color.a;
    }
    let gamma = aa + in.sdf.z;
    let alpha = smoothstep(in.sdf.y - gamma, in.sdf.y + gamma, dist);
    return in.color * alpha;
}
