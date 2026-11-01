#version 460 core
// `extra[0]`: opacity, the weight of the image faded from, brightness min, brightness max.
// `extra[1]`: saturation factor, contrast factor.
// `extra[2]`: hue rotation weights.

#include <common.glsl>

uniform sampler2D image;
uniform sampler2D fade_from;

in vec2 v_uv;
in vec2 v_fade_uv;

out vec4 frag_color;

vec4 unpremultiply(vec4 c) {
  if (c.a > 0.0) {
    return vec4(c.rgb / c.a, c.a);
  }
  return c;
}

void main() {
  vec4 own = unpremultiply(texture(image, v_uv));
  vec4 faded = unpremultiply(texture(fade_from, v_fade_uv));
  vec4 color = mix(own, faded, draw.extra[0].y);
  color.a *= draw.extra[0].x;

  vec3 spin = draw.extra[2].xyz;
  vec3 rgb = vec3(dot(color.rgb, spin.xyz), dot(color.rgb, spin.zxy), dot(color.rgb, spin.yzx));
  float average = (color.r + color.g + color.b) / 3.0;
  rgb += (average - rgb) * draw.extra[1].x;
  rgb = (rgb - 0.5) * draw.extra[1].y + 0.5;
  vec3 low = vec3(draw.extra[0].z);
  vec3 high = vec3(draw.extra[0].w);
  frag_color = vec4(mix(low, high, rgb) * color.a, color.a);
}
