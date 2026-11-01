#version 460 core

#include <common.glsl>

// 1 / sqrt(2 pi), as in `heatmap.vert`.
const float GAUSS_COEF = 0.3989422804014327;

in vec2 v_extrude;
in float v_weight;

out vec4 frag_color;

void main() {
  float d = -0.5 * dot(v_extrude, v_extrude);
  float value = v_weight * draw.extra[0].x * GAUSS_COEF * exp(d);
  frag_color = vec4(value, 1.0, 1.0, 1.0);
}
