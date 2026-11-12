// Walls of extruded polygons, a quad per instance from one `sparkle::tile::WallPoint` to
// the next; see `fill_extrusion.wgsl`.

@vertex
fn vs_main(
    // This instance's `WallPoint`: position and edge distance, then the wall's normal
    // (zero where no wall starts).
    @location(0) pos_edge: vec3<f32>,
    @location(1) normal_xy: vec2<f32>,
    // The next point's position and edge distance.
    @location(4) next_pos_edge: vec3<f32>,
    // Per vertex: 1 at the next point's end of the wall (x), 1 at the top (y).
    @location(5) corner: vec2<f32>,
    @location(2) pattern_rect: vec4<f32>,
    @location(3) pattern_size: vec2<f32>,
    p: PaintIn,
) -> VertexOut {
    // As in MapLibre, each end of the wall takes the other end's distance along the ring.
    let at_next = corner.x > 0.5;
    let pos = select(pos_edge.xy, next_pos_edge.xy, at_next);
    let edge = select(next_pos_edge.z, pos_edge.z, at_next);
    let base = max(paint_base(p), 0.0);
    let height = max(paint_height(p), 0.0);
    var out = extrude(pos, edge, vec3<f32>(normal_xy, 0.0), corner.y, base, height, pattern_rect, pattern_size, p);
    if all(normal_xy == vec2<f32>(0.0)) {
        // No wall: every corner at one point outside the view.
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    return out;
}
