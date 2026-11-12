#version 460 core
// Walls of extruded polygons, a quad of `sparkle::tile::WallVertex`es per ring edge; see
// `fill_extrusion_vertex.glsl`.

#define SPARKLE_VERTEX
#include <common.glsl>
#include <paint.glsl>

// `sparkle::tile::WallVertex`: position and the wall's normal, then top.
in vec4 pos_normal;
in float top;
// `sparkle::tile::ExtrusionWalls::edge_distances`; zero without patterns, which are all
// that read it.
in float edge;
// `sparkle::tile::FillExtrusionPattern`.
in vec4 pattern_rect;
in vec2 pattern_size;

PAINT_VEC4(color, 0)
PAINT_FLOAT(height, 1)
PAINT_FLOAT(base, 2)

#include <fill_extrusion_vertex.glsl>

void main() {
  float base = max(paint_base(), 0.0);
  float height = max(paint_height(), 0.0);
  extrude(pos_normal.xy, edge, vec3(pos_normal.zw, 0.0), top, base, height, pattern_rect,
          pattern_size);
}
