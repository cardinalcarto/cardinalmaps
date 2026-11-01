#version 460 core
// See `fill_extrusion.vert`.

#include <common.glsl>

uniform sampler2D pattern_atlas;

in vec4 v_color;
in vec2 v_pattern_pos;
flat in vec4 v_pattern_rect;
flat in vec2 v_pattern_size;
in vec2 v_tile_pos;

out vec4 frag_color;

void main() {
  // Buildings that cross the tile's edge are in both tiles, clipped to their buffers.
  // Cut each copy at the edge, as a vertical plane, so the copies don't overlap: their
  // walls are slightly different (clipped vertices are rounded) and would z-fight.
  if (any(lessThan(v_tile_pos, vec2(0.0))) || any(greaterThan(v_tile_pos, vec2(draw.extent)))) {
    discard;
  }
  if (v_pattern_rect.z <= 0.0) {
    frag_color = v_color;
    return;
  }
  // Repeat the image, sampling only inside its own rectangle of the atlas.
  vec4 rect = v_pattern_rect;
  vec2 texel = rect.xy + fract(v_pattern_pos / v_pattern_size) * rect.zw;
  vec2 clamped = clamp(texel, rect.xy + 0.5, rect.xy + rect.zw - 0.5);
  vec2 uv = clamped / draw.extra[3].yz;
  frag_color = textureLod(pattern_atlas, uv, 0.0) * v_color;
}
