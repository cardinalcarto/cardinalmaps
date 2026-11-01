#version 460 core
// Fills and their outlines, with a color or a `fill-pattern`. `atlas` is the sprite atlas
// for patterns.
//
// `extra[0]`: x 1 for the outline pass, y 1 when `fill-outline-color` is set, z the mode
//     (0 color, 1 the layer's pattern from `extra`, 2 a pattern per vertex), w the
//     cross-fade from pattern a to b.
// `extra[1]`: xy `fill-translate` in tile units, z `fill-layer-opacity`, w the cross-fade
//     scale of pattern a for patterns per vertex.
// `extra[2]`, `extra[3]`: patterns a and b, `[x, y, width, height]` in atlas pixels.
// `extra[4]`, `extra[5]`: where patterns a and b start in the tile (xy, in repeats) and
//     repeats per tile unit (zw); see `sparkle::pattern::pattern_placement`.
// `extra[6]`: for patterns per vertex, the tile's origin in pixels of the zoom patterns
//     are laid out at, split for precision: xy whole multiples of 65536, zw the rest.
// `extra[7]`: xy the atlas size, z pixels per tile unit at the zoom patterns are laid out
//     at, w the cross-fade scale of pattern b for patterns per vertex.

#define SPARKLE_VERTEX
#include <common.glsl>
#include <paint.glsl>

PAINT_VEC4(color, 0)
PAINT_FLOAT(opacity, 1)
PAINT_VEC4(outline_color, 2)

in vec2 pos;
// The feature's `sparkle::pattern::FeatureImage`, for patterns per vertex.
in vec4 feature_a;
in vec4 feature_b;

// Premultiplied; just the opacity when drawing a pattern.
out vec4 v_color;
// Pattern coordinates for a (xy) and b (zw), in repeats.
out vec4 v_uv;
flat out vec4 v_rect_a;
flat out vec4 v_rect_b;
flat out float v_fade;

vec2 modulo(vec2 a, vec2 b) {
  return a - b * floor(a / b);
}

// `offset` modulo `size`, for an offset given as `upper * 65536 + lower`, without the
// precision a float that large would lose.
vec2 wrap(vec2 upper, vec2 lower, vec2 size) {
  return modulo(modulo(modulo(upper, size) * 256.0, size) * 256.0 + lower, size);
}

void main() {
  bool outline = draw.extra[0].x > 0.5;
  float mode = draw.extra[0].z;
  vec4 color = paint_color();
  if (outline && draw.extra[0].y > 0.5) {
    color = paint_outline_color();
  }
  float opacity = paint_opacity() * draw.extra[1].z;

  gl_Position = to_clip(pos + draw.extra[1].xy);
  v_color = color * opacity;
  v_fade = draw.extra[0].w;
  v_uv = vec4(0.0);
  v_rect_a = vec4(0.0);
  v_rect_b = vec4(0.0);
  // An outline with its own color is drawn in that color, even over a pattern.
  if (mode > 0.5 && !(outline && draw.extra[0].y > 0.5)) {
    v_color = vec4(opacity);
    if (mode < 1.5) {
      v_rect_a = draw.extra[2];
      v_rect_b = draw.extra[3];
      v_uv = vec4(draw.extra[4].xy + pos * draw.extra[4].zw,
                  draw.extra[5].xy + pos * draw.extra[5].zw);
    } else {
      // The feature's own image, fading into itself at the next zoom's scale.
      vec2 size = max(feature_a.zw / max(feature_b.x, 1e-6), vec2(1e-3));
      vec2 size_a = size * draw.extra[1].w;
      vec2 size_b = size * draw.extra[7].w;
      vec2 pixels = pos * draw.extra[7].z;
      v_uv = vec4((wrap(draw.extra[6].xy, draw.extra[6].zw, size_a) + pixels) / size_a,
                  (wrap(draw.extra[6].xy, draw.extra[6].zw, size_b) + pixels) / size_b);
      v_rect_a = feature_a;
      v_rect_b = feature_a;
      if (feature_a.z <= 0.0) {
        // The feature's image is missing: draw nothing, as MapLibre does.
        v_color = vec4(0.0);
      }
    }
  }
}
