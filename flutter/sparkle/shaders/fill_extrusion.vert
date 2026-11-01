#version 460 core
// Roofs and walls of extruded polygons, lit as MapLibre lights them (see
// `sparkle::light::ExtrusionLight::shade`), drawn into the layer's offscreen target (or,
// writing no color, into the map's depth buffer).
//
// `extra[0]`: the light's position (xyz, tile coordinates) and intensity (w).
// `extra[1].rgb`: the light's color.
// `extra[2]`: `fill-extrusion-translate` in tile units (xy), logical pixels per meter (z),
//             1 for `fill-extrusion-vertical-gradient` (w).
// `extra[3]`: tile units per pattern pixel (x), the sprite atlas size in pixels (yz).

#define SPARKLE_VERTEX
#include <common.glsl>
#include <paint.glsl>

// `sparkle::tile::FillExtrusionVertex`: position and edge distance, then normal and top.
in vec3 pos_edge;
in vec4 normal_top;
// `sparkle::tile::FillExtrusionPattern`.
in vec4 pattern_rect;
in vec2 pattern_size;

PAINT_VEC4(color, 0)
PAINT_FLOAT(height, 1)
PAINT_FLOAT(base, 2)

// Opaque and premultiplied; with a pattern, the lighting to multiply it by.
out vec4 v_color;
// In pattern pixels: tile position on roofs, distance along and up the wall on walls.
out vec2 v_pattern_pos;
flat out vec4 v_pattern_rect;
flat out vec2 v_pattern_size;
// Where on the ground the fragment is, in (untranslated) tile coordinates.
out vec2 v_tile_pos;

void main() {
  float base = max(paint_base(), 0.0);
  float height = max(paint_height(), 0.0);
  vec3 normal = normal_top.xyz;
  float top = normal_top.w;
  float z = (top > 0.5 ? height : base) * draw.extra[2].z;

  gl_Position = to_clip_above(pos_edge.xy + draw.extra[2].xy, z);

  vec3 light_pos = draw.extra[0].xyz;
  float intensity = draw.extra[0].w;
  vec3 light_color = draw.extra[1].rgb;
  bool has_pattern = pattern_rect.z > 0.0;
  vec4 color = paint_color();
  float luminance = dot(color.rgb, vec3(0.2126, 0.7152, 0.0722));
  // MapLibre's normals reach its dot product doubled (packing), hence the 2.
  float directional = clamp(2.0 * dot(normal, light_pos), 0.0, 1.0);
  float upper = has_pattern ? max(0.5 + intensity, 1.0) : max(1.0 - luminance + intensity, 1.0);
  directional = mix(1.0 - intensity, upper, directional);
  if (normal.z == 0.0 && draw.extra[2].w > 0.5) {
    directional *=
        clamp((top + base) * sqrt(height / 150.0), mix(0.7, 0.98, 1.0 - intensity), 1.0);
  }
  vec3 surface = has_pattern ? vec3(1.0) : color.rgb + vec3(0.03);
  vec3 lit =
      clamp(surface * directional * light_color, mix(vec3(0.0), vec3(0.3), 1.0 - light_color),
            vec3(1.0));
  v_color = vec4(lit, 1.0);

  float units_per_pattern_pixel = draw.extra[3].x;
  if (normal.z == 0.0) {
    v_pattern_pos = vec2(pos_edge.z, -z * draw.units_per_pixel) / units_per_pattern_pixel;
  } else {
    v_pattern_pos = pos_edge.xy / units_per_pattern_pixel;
  }
  v_pattern_rect = pattern_rect;
  v_pattern_size = pattern_size;
  v_tile_pos = pos_edge.xy;
}
