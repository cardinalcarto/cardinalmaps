// Tile clipping masks: a quad over `0..extent` written into the stencil buffer.

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32(i & 1u), f32(i >> 1u)) * draw.extent;
    return to_clip(corner);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
