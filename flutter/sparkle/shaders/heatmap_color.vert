#version 460 core
// `heatmap` layers, the color pass: looks the accumulated density up in the
// `heatmap-color` ramp, over the whole viewport.
//
// `extra[0].x`: `heatmap-opacity`; `.y`: the ramp's width in texels.

#define SPARKLE_VERTEX
#include <common.glsl>

// A corner of the unit square, drawn as a strip.
in vec2 position;

// Where the pixel is in the density texture.
out vec2 v_uv;

void main() {
  gl_Position = vec4(position * 2.0 - 1.0, 0.0, 1.0);
  // Textures have their first row at the top of clip space.
  v_uv = vec2(position.x, 1.0 - position.y);
}
