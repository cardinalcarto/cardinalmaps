// Lines widened from their centerline (see `sparkle::tile::LineBucket`), in one of four
// modes: a plain color, dashes from a signed distance atlas, a sprite pattern, or a
// `line-gradient` ramp. Group 1 is the mode's texture.
//
// `extra[0]`: x the mode (0 color, 1 dashes, 2 pattern, 3 gradient), y 1 when the dash
//     pattern or image is per vertex (`image_a`/`image_b`) rather than in `extra`, z the
//     cross-fade from the `a` to the `b` image, w how much the map has grown since the
//     integer zoom images are laid out at.
// `extra[1]`: xy `line-translate` in tile units, z `line-layer-opacity`.
// `extra[2]`, `extra[3]`: dash patterns a and b: atlas row at the center of the line, rows
//     from there to an edge, repeat length in line widths, cross-fade scale. Per-vertex
//     patterns only take the scales from here.
// `extra[4]`, `extra[5]`: pattern images a and b, `[x, y, width, height]` in atlas pixels.
// `extra[6]`: xy the size of pattern a in logical pixels, zw of pattern b.
// `extra[7]`: xy the texture's size in texels.

@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

const MODE_DASH: f32 = 1.0;
const MODE_PATTERN: f32 = 2.0;
const MODE_GRADIENT: f32 = 3.0;
// `sparkle::pattern::DASH_SDF_RANGE`.
const DASH_SDF_RANGE: f32 = 2.0;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    // Premultiplied; just the opacity for patterns and gradients.
    @location(0) color: vec4<f32>,
    // The extrude scaled to at most 1: its length is the distance from the centerline as
    // a fraction of `widths.x`, exactly, around round joins too. Round caps' corners keep
    // theirs, so that the cap is cut to a half circle.
    @location(1) normal: vec2<f32>,
    // In logical pixels: the outer edge's distance from the centerline, the inner edge's
    // (non-zero with `line-gap-width`), the blur including antialiasing, and the distance
    // along the line.
    @location(2) widths: vec4<f32>,
    // x from -1 on the left edge to 1 on the right; y the line progress.
    @location(3) side: vec2<f32>,
    // Dashes: repeats along the line for a and b, and pixels per line width for a and b.
    // Patterns: repeats along the line for a and b.
    @location(4) tex: vec4<f32>,
    @location(5) @interpolate(flat) image_a: vec4<f32>,
    @location(6) @interpolate(flat) image_b: vec4<f32>,
    // The cross-fade between a and b.
    @location(7) @interpolate(flat) fade: f32,
};

@vertex
fn vs_main(
    @location(0) pos: vec2<f32>,
    @location(1) extrude: vec2<f32>,
    @location(2) offset_dir: vec2<f32>,
    @location(3) metrics: vec4<f32>,
    @location(4) feature_a: vec4<f32>,
    @location(5) feature_b: vec4<f32>,
    p: PaintIn,
) -> VertexOut {
    let mode = draw.extra[0].x;
    var opacity = paint_opacity(p) * draw.extra[1].z;
    var width = max(paint_width(p), 0.0);
    let half_gap = max(paint_gap_width(p), 0.0) * 0.5;

    // Lines thinner than a physical pixel would flicker in and out of the samples, so
    // draw them a pixel wide and fade them out instead.
    let physical = width * draw.pixel_ratio;
    if physical < 1.0 {
        opacity *= physical;
        width = 1.0 / draw.pixel_ratio;
    }
    // Half a physical pixel on each side for the antialiased edge, as MapLibre does.
    let aa = 0.5 / draw.pixel_ratio;
    var outset = width * 0.5 + aa;
    var inset = 0.0;
    if half_gap > 0.0 {
        inset = half_gap + aa;
        outset = half_gap + width + aa;
    }
    // Positive offsets move the line to the right of its direction, which is where
    // `offset_dir` points.
    let dist = extrude * outset;
    let shift = dist + offset_dir * paint_offset(p);

    var out: VertexOut;
    out.position = to_clip(pos + shift * draw.units_per_pixel + draw.extra[1].xy);

    // In pitched views the far side's pixels are smaller; scale the blur so that it stays
    // about a pixel on screen.
    var gamma = 1.0;
    let dist_len = length(dist);
    if dist_len > 1e-3 {
        let projected = draw.matrix * vec4<f32>(dist * draw.units_per_pixel, 0.0, 0.0);
        let on_screen = length(projected.xy / out.position.w * draw.viewport * 0.5) / draw.pixel_ratio;
        gamma = dist_len / max(on_screen, 1e-6);
    }
    let blur = (max(paint_blur(p), 0.0) + 1.0 / draw.pixel_ratio) * gamma;

    let distance = metrics.x / draw.units_per_pixel + metrics.y * outset;
    out.normal = select(extrude / max(length(extrude), 1.0), extrude, metrics.w > 0.5);
    out.widths = vec4<f32>(outset, inset, blur, distance);
    let across = dot(extrude, offset_dir) / max(dot(offset_dir, offset_dir), 1e-6);
    out.side = vec2<f32>(across, metrics.z);
    out.color = paint_color(p) * opacity;
    out.fade = draw.extra[0].z;

    let per_vertex = draw.extra[0].y > 0.5;
    let floorwidth = max(paint_floorwidth(p), 1e-3);
    if mode == MODE_DASH {
        var a = draw.extra[2];
        var b = draw.extra[3];
        if per_vertex {
            // The feature's own pattern, keeping the cross-fade scales from `extra`.
            a = vec4<f32>(feature_a.xyz, a.w);
            b = vec4<f32>(feature_a.xyz, b.w);
        }
        // Dashes are laid out in widths of the integer zoom and grow with the map from there.
        let unit_a = floorwidth * a.w * draw.extra[0].w;
        let unit_b = floorwidth * b.w * draw.extra[0].w;
        out.tex = vec4<f32>(
            distance / max(a.z * unit_a, 1e-6),
            distance / max(b.z * unit_b, 1e-6),
            unit_a,
            unit_b,
        );
        out.image_a = a;
        out.image_b = b;
    } else if mode == MODE_PATTERN {
        var rect_a = draw.extra[4];
        var rect_b = draw.extra[5];
        var size_a = draw.extra[6].xy;
        var size_b = draw.extra[6].zw;
        if per_vertex {
            rect_a = feature_a;
            rect_b = feature_a;
            size_a = feature_a.zw / max(feature_b.x, 1e-6);
            size_b = size_a;
            out.fade = 1.0;
        }
        // As in MapLibre, the image is stretched across the line's width and keeps its
        // aspect ratio along it, at the integer zoom's width.
        out.tex = vec4<f32>(
            distance * size_a.y / max(size_a.x * floorwidth, 1e-6),
            distance * size_b.y / max(size_b.x * floorwidth, 1e-6),
            0.0,
            0.0,
        );
        out.image_a = rect_a;
        out.image_b = rect_b;
        out.color = vec4<f32>(opacity);
    } else if mode == MODE_GRADIENT {
        out.color = vec4<f32>(opacity);
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
    return textureSampleLevel(image, image_sampler, (rect.xy + texel) / draw.extra[7].xy, 0.0);
}

// A dash pattern's signed distance in pixels: positive inside dashes.
fn dash_distance(row: vec4<f32>, along: f32, across: f32, pixels_per_width: f32) -> f32 {
    if row.z <= 0.0 {
        // A solid line.
        return 1e3;
    }
    let y = (row.x + across * row.y) / draw.extra[7].y;
    let v = textureSampleLevel(image, image_sampler, vec2<f32>(along, y), 0.0).r;
    return (v - 0.5) * 2.0 * DASH_SDF_RANGE * pixels_per_width;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let outset = in.widths.x;
    let inset = in.widths.y;
    let blur = in.widths.z;
    let dist = length(in.normal) * outset;
    var alpha = clamp(min(dist - (inset - blur), outset - dist) / blur, 0.0, 1.0);

    let mode = draw.extra[0].x;
    var color = in.color;
    if mode == MODE_DASH {
        let across = clamp(in.side.x, -1.0, 1.0);
        let a = dash_distance(in.image_a, in.tex.x, across, in.tex.z);
        let b = dash_distance(in.image_b, in.tex.y, across, in.tex.w);
        alpha *= clamp(mix(a, b, in.fade) / blur + 0.5, 0.0, 1.0);
    } else if mode == MODE_PATTERN {
        let y = clamp(in.side.x, -1.0, 1.0) * 0.5 + 0.5;
        let a = sample_pattern(in.image_a, vec2<f32>(in.tex.x, y));
        let b = sample_pattern(in.image_b, vec2<f32>(in.tex.y, y));
        color = mix(a, b, in.fade) * in.color.a;
    } else if mode == MODE_GRADIENT {
        let ramp = textureSampleLevel(image, image_sampler, vec2<f32>(in.side.y, 0.5), 0.0);
        color = ramp * in.color.a;
    }
    return color * alpha;
}
