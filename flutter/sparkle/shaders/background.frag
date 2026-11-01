#version 460 core

#include <common.glsl>

uniform sampler2D atlas;

in vec4 v_uv;

out vec4 frag_color;

vec4 sample_pattern(vec4 rect, vec2 uv) {
  vec2 texel = clamp(fract(uv) * rect.zw, vec2(0.5), rect.zw - 0.5);
  return textureLod(atlas, (rect.xy + texel) / draw.extra[7].xy, 0.0);
}

void main() {
  if (draw.extra[1].x > 0.5) {
    vec4 a = sample_pattern(draw.extra[2], v_uv.xy);
    vec4 b = sample_pattern(draw.extra[3], v_uv.zw);
    frag_color = mix(a, b, draw.extra[1].y) * draw.extra[1].z;
  } else {
    frag_color = draw.extra[0];
  }
}
