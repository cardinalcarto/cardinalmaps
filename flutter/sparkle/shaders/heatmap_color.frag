#version 460 core

#include <common.glsl>

uniform sampler2D density;
uniform sampler2D ramp;

in vec2 v_uv;

out vec4 frag_color;

void main() {
  float t = texture(density, v_uv).r;
  // The ramp's first and last texel centers are densities 0 and 1.
  float n = draw.extra[0].y;
  float u = (clamp(t, 0.0, 1.0) * (n - 1.0) + 0.5) / n;
  frag_color = texture(ramp, vec2(u, 0.5)) * draw.extra[0].x;
}
