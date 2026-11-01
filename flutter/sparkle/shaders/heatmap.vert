#version 460 core
// `heatmap` layers, the density pass: a Gaussian kernel per point, summed with additive
// blending into an offscreen texture. Follows MapLibre's heatmap shaders, including their
// constants.
//
// `extra[0].x`: `heatmap-intensity`.

#define SPARKLE_VERTEX
#include <common.glsl>
#include <paint.glsl>

// Kernel values below this don't register in the density texture, so each point's quad
// only reaches as far as its kernel is larger. One step of an 8-bit target, divided again
// by 16 as MapLibre does to keep low weights visible.
const float ZERO = 1.0 / 255.0 / 16.0;
// 1 / sqrt(2 pi): the normal distribution's coefficient.
const float GAUSS_COEF = 0.3989422804014327;

in vec2 pos;
in vec2 corner;

PAINT_FLOAT(weight, 0)
PAINT_FLOAT(radius, 1)

// The position in the kernel, in standard deviations (the radius is three of them).
out vec2 v_extrude;
out float v_weight;

void main() {
  float weight = paint_weight();
  float radius = paint_radius();
  float intensity = draw.extra[0].x;
  v_weight = weight;
  float peak = weight * intensity * GAUSS_COEF;
  if (peak <= ZERO || radius <= 0.0 || globe_hidden(pos)) {
    // Nothing to add: collapse the quad.
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    v_extrude = vec2(0.0);
    return;
  }
  // How far out, relative to the radius, the kernel stays above ZERO.
  float s = sqrt(-2.0 * log(ZERO / peak)) / 3.0;
  v_extrude = s * corner * 3.0;
  vec2 offset = s * corner * radius * draw.units_per_pixel;
  gl_Position = to_clip(pos + offset);
}
