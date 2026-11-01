// The sky behind the map: above a pitched plane's horizon, a gradient from the horizon
// color up to the sky color, with the haze of distant fog below it; around the globe, an
// atmosphere fading from the horizon color at its edge into space. Both blend by the
// globe factor.
//
// `matrix`: clip space to the frame of `view` (its inverse).
// `extra[0]`, `extra[1]`, `extra[2]`: the sky, horizon and haze colors, premultiplied.
// `extra[3]`: the horizon's y in normalized device coordinates on the plane,
//     `sky-horizon-blend`, `atmosphere-blend`, the globe factor.

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    var out: VertexOut;
    out.position = fullscreen_triangle(i);
    out.ndc = out.position.xy;
    return out;
}

// The page under the map, as the renderer clears it.
const PAGE: vec4<f32> = vec4<f32>(1.0);
// Beyond the atmosphere.
const SPACE: vec4<f32> = vec4<f32>(0.02, 0.03, 0.07, 1.0);

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let sky = draw.extra[0];
    let horizon = draw.extra[1];
    let haze = draw.extra[2];
    let h = draw.extra[3].x;
    let sky_horizon_blend = draw.extra[3].y;
    let atmosphere_blend = draw.extra[3].z;
    let t = draw.extra[3].w;

    var plane = haze;
    if in.ndc.y > h {
        // 0 at the horizon, 1 at the top of the view.
        let up = (in.ndc.y - h) / max(1.0 - h, 1e-3);
        var blend = 1.0;
        if sky_horizon_blend > 0.0 {
            blend = clamp(up / sky_horizon_blend, 0.0, 1.0);
        }
        plane = mix(horizon, sky, blend);
    }
    if t <= 0.0 {
        return plane;
    }

    // How close the ray under this pixel passes to the globe's center, in radii.
    let near = draw.matrix * vec4<f32>(in.ndc, 0.0, 1.0);
    let far = draw.matrix * vec4<f32>(in.ndc, 1.0, 1.0);
    let a = near.xyz / near.w;
    let d = normalize(far.xyz / far.w - a);
    let radius = draw.globe.y;
    let to_center = vec3<f32>(0.0, 0.0, -radius) - a;
    let r = length(cross(to_center, d)) / radius;
    // A thin band of the horizon color at the globe's edge, turning into the sky color
    // and then into space.
    let edge = mix(sky, horizon, smoothstep(1.08, 1.0, r));
    let glow = pow(smoothstep(1.35, 1.0, r), 1.5);
    let globe = mix(PAGE, mix(SPACE, edge, glow), atmosphere_blend);
    return mix(plane, globe, t);
}
