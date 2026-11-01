#version 460 core
// `raster` layers: an image on a quad; see `raster.frag`.
//
// `extra[3]`: scale and offset (x, y) from this image's texture coordinates to the one
// faded from.

#define SPARKLE_VERTEX
#include <common.glsl>

// `sparkle::tile::RasterVertex`, drawn as a strip.
in vec2 position;
in vec2 texcoord;

out vec2 v_uv;
out vec2 v_fade_uv;

void main() {
  gl_Position = to_clip(position);
  v_uv = texcoord;
  v_fade_uv = texcoord * draw.extra[3].x + draw.extra[3].yz;
}
