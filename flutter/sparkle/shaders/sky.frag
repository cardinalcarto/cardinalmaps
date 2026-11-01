#version 460 core

#include <common.glsl>

in vec2 v_ndc;

out vec4 frag_color;

// The page under the map, as the renderer clears it.
const vec4 PAGE = vec4(1.0);
// Beyond the atmosphere.
const vec4 SPACE = vec4(0.02, 0.03, 0.07, 1.0);

void main() {
  vec4 sky = draw.extra[0];
  vec4 horizon = draw.extra[1];
  vec4 haze = draw.extra[2];
  float h = draw.extra[3].x;
  float sky_horizon_blend = draw.extra[3].y;
  float atmosphere_blend = draw.extra[3].z;
  float t = draw.extra[3].w;

  vec4 plane = haze;
  if (v_ndc.y > h) {
    // 0 at the horizon, 1 at the top of the view.
    float up = (v_ndc.y - h) / max(1.0 - h, 1e-3);
    float blend = 1.0;
    if (sky_horizon_blend > 0.0) {
      blend = clamp(up / sky_horizon_blend, 0.0, 1.0);
    }
    plane = mix(horizon, sky, blend);
  }
  if (t <= 0.0) {
    frag_color = plane;
    return;
  }

  // How close the ray under this pixel passes to the globe's center, in radii.
  vec4 near = draw.matrix * vec4(v_ndc, 0.0, 1.0);
  vec4 far = draw.matrix * vec4(v_ndc, 1.0, 1.0);
  vec3 a = near.xyz / near.w;
  vec3 d = normalize(far.xyz / far.w - a);
  float radius = draw.globe.y;
  vec3 to_center = vec3(0.0, 0.0, -radius) - a;
  float r = length(cross(to_center, d)) / radius;
  // A thin band of the horizon color at the globe's edge, turning into the sky color
  // and then into space.
  vec4 edge = mix(sky, horizon, smoothstep(1.08, 1.0, r));
  float glow = pow(smoothstep(1.35, 1.0, r), 1.5);
  vec4 globe = mix(PAGE, mix(SPACE, edge, glow), atmosphere_blend);
  frag_color = mix(plane, globe, t);
}
