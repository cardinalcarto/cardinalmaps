#version 460 core
// A fill-extrusion layer's offscreen image over the map, at `extra[0].x` opacity. The
// image is the size of the screen, so each fragment takes its own pixel of it.

#include <common.glsl>

uniform sampler2D layer_image;

in vec2 v_uv;

out vec4 frag_color;

void main() {
  frag_color = textureLod(layer_image, v_uv, 0.0) * draw.extra[0].x;
}
