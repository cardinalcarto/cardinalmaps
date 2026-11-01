// `extra[0].x`: 1 when `circle-pitch-scale` is `map`. `extra[0].y`: 1 when
// `circle-pitch-alignment` is `map`. `extra[0].z`: the distance from the camera to the
// center of the view in logical pixels, which is clip `w` there.
// `extra[1].xy`: `circle-translate` in tile units.

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) stroke_color: vec4<f32>,
    // The quad corner, scaled so the circle's outer edge (with stroke) is at length 1.
    @location(2) extrude: vec2<f32>,
    // `circle-blur` as a fraction of the outer radius, and where the fill ends and the
    // stroke starts, also as a fraction of it (1 without a stroke).
    @location(3) shape: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) center: vec2<f32>,
    @location(1) extrude: vec2<f32>,
    p: PaintIn,
) -> VertexOut {
    let radius = max(paint_radius(p), 0.0);
    let stroke_width = max(paint_stroke_width(p), 0.0);
    let size = radius + stroke_width;
    let scale_with_map = draw.extra[0].x > 0.5;
    let pitch_with_map = draw.extra[0].y > 0.5;
    let camera_to_center = draw.extra[0].z;
    let pos = center + draw.extra[1].xy;

    var out: VertexOut;
    if globe_hidden(pos) {
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    if pitch_with_map {
        // Flat on the map: extrude in tile units, so the disc is foreshortened with it.
        var offset = extrude * size * draw.units_per_pixel;
        if !scale_with_map {
            // Far circles would shrink with the map; scale them back up to their size at
            // the center of the view.
            offset *= to_clip(pos).w / camera_to_center;
        }
        out.position = to_clip(pos + offset);
    } else {
        // Facing the viewer: extrude in clip space, where `w` pixels make one logical
        // pixel per unit after the perspective divide.
        out.position = to_clip(pos);
        let pixels_to_clip = 2.0 * draw.pixel_ratio / draw.viewport * vec2<f32>(1.0, -1.0);
        var w = out.position.w;
        if scale_with_map {
            // Constant in clip space, so the perspective divide shrinks far circles.
            w = camera_to_center;
        }
        out.position += vec4<f32>(extrude * size * pixels_to_clip * w, 0.0, 0.0);
    }

    out.color = paint_color(p) * paint_opacity(p);
    out.stroke_color = paint_stroke_color(p) * paint_stroke_opacity(p);
    out.extrude = extrude;
    var fill_end = 1.0;
    if stroke_width >= 0.01 && size > 0.0 {
        fill_end = radius / size;
    }
    out.shape = vec2<f32>(max(paint_blur(p), 0.0), fill_end);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let dist = length(in.extrude);
    // About one physical pixel in extrude units, wherever the circle is and however it is
    // pitched, so edges stay smooth without blurring large circles.
    let pixel = fwidth(dist);
    let blur = max(in.shape.x, pixel);
    let opacity = 1.0 - smoothstep(-blur, 0.0, dist - 1.0);
    var stroke = 0.0;
    if in.shape.y < 1.0 {
        stroke = smoothstep(-blur, 0.0, dist - in.shape.y);
    }
    return opacity * mix(in.color, in.stroke_color, stroke);
}
