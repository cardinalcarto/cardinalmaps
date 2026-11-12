#version 460 core
// Roofs of extruded polygons; see `fill_extrusion_vertex.glsl`.

#define SPARKLE_VERTEX
#include <common.glsl>
#include <paint.glsl>

// `sparkle::tile::ExtrusionRoofs::vertices`.
in vec2 pos;
// `sparkle::tile::FillExtrusionPattern`.
in vec4 pattern_rect;
in vec2 pattern_size;

// `sparkle::style::FillExtrusionPaint::roof_attributes`.
PAINT_VEC4(color, 0)
PAINT_FLOAT(height, 1)

#include <fill_extrusion_vertex.glsl>

void main() {
  float height = max(paint_height(), 0.0);
  extrude(pos, 0.0, vec3(0.0, 0.0, 1.0), 1.0, 0.0, height, pattern_rect, pattern_size);
}
