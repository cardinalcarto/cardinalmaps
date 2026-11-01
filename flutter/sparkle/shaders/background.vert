#version 460 core
// `background` layers: a full-screen color, or a `background-pattern` drawn tile by tile
// so it stays fixed to the ground.
//
// `extra[0]`: the premultiplied color, opacity applied.
// `extra[1]`: x 1 to draw a pattern over the tile `matrix` places (`0..1`), y the
//     cross-fade from pattern a to b, z the opacity.
// `extra[2]`, `extra[3]`: patterns a and b, `[x, y, width, height]` in atlas pixels.
// `extra[4]`, `extra[5]`: where patterns a and b start in the tile (xy, in repeats) and
//     repeats per tile unit (zw); see `sparkle::pattern::pattern_placement`.
// `extra[7]`: xy the atlas size.

#define SPARKLE_VERTEX
#include <common.glsl>

// A corner of the unit square, drawn as a strip.
in vec2 position;

// Pattern coordinates for a (xy) and b (zw), in repeats.
out vec4 v_uv;

void main() {
  if (draw.extra[1].x > 0.5) {
    gl_Position = to_clip(position);
    v_uv = vec4(draw.extra[4].xy + position * draw.extra[4].zw,
                draw.extra[5].xy + position * draw.extra[5].zw);
  } else {
    gl_Position = vec4(position * 2.0 - 1.0, 0.0, 1.0);
    v_uv = vec4(0.0);
  }
}
