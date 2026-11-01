// A fill-extrusion layer's offscreen image over the map, at `extra[0].x` opacity.

@group(1) @binding(0) var layer_image: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_triangle(i);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(layer_image, vec2<i32>(pos.xy), 0) * draw.extra[0].x;
}
