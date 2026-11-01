#version 460 core

#include <common.glsl>

uniform sampler2D drape;

in vec2 v_uv;
in float v_fog;

out vec4 frag_color;

void main() {
  vec4 color = texture(drape, v_uv);
  frag_color = mix(color, draw.fog_color * color.a, v_fog);
}
