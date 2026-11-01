#version 460 core

#include <common.glsl>

uniform sampler2D atlas;

in vec4 v_color;
in vec4 v_uv;
flat in vec4 v_rect_a;
flat in vec4 v_rect_b;
flat in float v_fade;

out vec4 frag_color;

// One repeat of an atlas image at `uv`, kept half a texel inside it so that filtering
// doesn't reach the neighbours.
vec4 sample_pattern(vec4 rect, vec2 uv) {
  if (rect.z <= 0.0 || rect.w <= 0.0) {
    return vec4(0.0);
  }
  vec2 texel = clamp(fract(uv) * rect.zw, vec2(0.5), rect.zw - 0.5);
  return textureLod(atlas, (rect.xy + texel) / draw.extra[7].xy, 0.0);
}

void main() {
  if (v_rect_a.z > 0.0 || v_rect_b.z > 0.0) {
    vec4 a = sample_pattern(v_rect_a, v_uv.xy);
    vec4 b = sample_pattern(v_rect_b, v_uv.zw);
    frag_color = mix(a, b, v_fade) * v_color.a;
  } else {
    frag_color = v_color;
  }
}
