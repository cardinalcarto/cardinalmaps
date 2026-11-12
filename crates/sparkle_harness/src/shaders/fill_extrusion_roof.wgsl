// Roofs of extruded polygons; see `fill_extrusion.wgsl`.

@vertex
fn vs_main(
    // `sparkle::tile::ExtrusionRoofs::vertices`.
    @location(0) pos: vec2<f32>,
    @location(2) pattern_rect: vec4<f32>,
    @location(3) pattern_size: vec2<f32>,
    p: PaintIn,
) -> VertexOut {
    // `sparkle::style::FillExtrusionPaint::roof_attributes`: no base.
    let height = max(paint_height(p), 0.0);
    return extrude(pos, 0.0, vec3<f32>(0.0, 0.0, 1.0), 1.0, 0.0, height, pattern_rect, pattern_size, p);
}
