#version 460 core
// `hillshade` layers: a DEM tile's quad; see `hillshade.frag`.
//
// `extra[0].w`: `extent`.

#define SPARKLE_VERTEX
#include <common.glsl>

// A corner of the unit square, drawn as a strip.
in vec2 position;

out vec2 v_uv;

void main() {
  gl_Position = to_clip(position * draw.extra[0].w);
  v_uv = position;
}
