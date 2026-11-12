// Walls of extruded polygons, a quad of `sparkle::tile::WallVertex`es per ring edge; see
// `fill_extrusion.wgsl`.

@vertex
fn vs_main(
    // `sparkle::tile::WallVertex`: position and the wall's normal, then top.
    @location(0) pos_normal: vec4<f32>,
    @location(1) top: f32,
    // `sparkle::tile::ExtrusionWalls::edge_distances`; zero without patterns, which are
    // all that read it.
    @location(4) edge: f32,
    @location(2) pattern_rect: vec4<f32>,
    @location(3) pattern_size: vec2<f32>,
    p: PaintIn,
) -> VertexOut {
    let base = max(paint_base(p), 0.0);
    let height = max(paint_height(p), 0.0);
    return extrude(pos_normal.xy, edge, vec3<f32>(pos_normal.zw, 0.0), top, base, height, pattern_rect, pattern_size, p);
}
