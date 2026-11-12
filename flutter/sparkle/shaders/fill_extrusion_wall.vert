#version 460 core
// Walls of extruded polygons, a quad per instance from one `sparkle::tile::WallPoint` to
// the next; see `fill_extrusion_vertex.glsl`.

#define SPARKLE_VERTEX
#include <common.glsl>
#include <paint.glsl>

// This instance's `WallPoint`: position, then the wall's normal (zero where no wall
// starts).
in vec4 pos_normal;
// The next point's position.
in vec2 next_pos;
// Per vertex: 1 at the next point's end of the wall (x), 1 at the top (y).
in vec2 corner;
// This point's and the next one's `ExtrusionWalls::edge_distances`; zeros without
// patterns, which are all that read them.
in float edge;
in float next_edge;
// `sparkle::tile::FillExtrusionPattern`.
in vec4 pattern_rect;
in vec2 pattern_size;

PAINT_VEC4(color, 0)
PAINT_FLOAT(height, 1)
PAINT_FLOAT(base, 2)

#include <fill_extrusion_vertex.glsl>

void main() {
  // As in MapLibre, each end of the wall takes the other end's distance along the ring.
  bool at_next = corner.x > 0.5;
  vec2 pos = at_next ? next_pos : pos_normal.xy;
  vec2 normal_xy = pos_normal.zw;
  float base = max(paint_base(), 0.0);
  float height = max(paint_height(), 0.0);
  extrude(pos, at_next ? edge : next_edge, vec3(normal_xy, 0.0), corner.y, base, height,
          pattern_rect, pattern_size);
  if (normal_xy == vec2(0.0)) {
    // No wall: every corner at one point outside the view.
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
  }
}
