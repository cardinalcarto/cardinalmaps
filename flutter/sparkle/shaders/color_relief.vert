#version 460 core
// `color-relief` layers: a DEM tile's quad; see `color_relief.frag`.
//
// `extra[1].x`: `extent`.

#define SPARKLE_VERTEX
#include <common.glsl>

// A corner of the unit square, drawn as a strip.
in vec2 position;

out vec2 v_uv;

void main() {
  gl_Position = to_clip(position * draw.extra[1].x);
  v_uv = position;
}
