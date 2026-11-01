#version 460 core
// Coloring by elevation: each pixel's elevation, read from the tile's DEM (with its
// one pixel border, mapbox-encoded), looked up in the layer's color ramp.
//
// `extra[0]`: DEM pixels per side (without the border), ramp min elevation, ramp max
// elevation, opacity.
// `extra[1]`: `extent`, 1 to sample the nearest pixel instead of interpolating, ramp size.

#include <common.glsl>

// The layer's DEM tile (not the terrain's `dem`), sampled with `NEAREST` at pixel centers.
uniform sampler2D elevations;
uniform sampler2D ramp;

in vec2 v_uv;

out vec4 frag_color;

// The elevation of a pixel of the bordered grid, in meters.
float dem_elevation(vec2 p) {
  float size = draw.extra[0].x + 2.0;
  vec2 texel = clamp(p, vec2(0.0), vec2(size - 1.0));
  vec3 rgb = floor(textureLod(elevations, (texel + 0.5) / size, 0.0).rgb * 255.0 + 0.5);
  return -10000.0 + (rgb.r * 65536.0 + rgb.g * 256.0 + rgb.b) * 0.1;
}

void main() {
  // Pixel centers of the tile are at `i + 0.5`; the border shifts them by one.
  vec2 p = v_uv * draw.extra[0].x - 0.5 + 1.0;
  float e;
  if (draw.extra[1].y > 0.5) {
    e = dem_elevation(floor(p + 0.5));
  } else {
    vec2 base = floor(p);
    vec2 f = p - base;
    float top = mix(dem_elevation(base), dem_elevation(base + vec2(1.0, 0.0)), f.x);
    float bottom =
        mix(dem_elevation(base + vec2(0.0, 1.0)), dem_elevation(base + vec2(1.0, 1.0)), f.x);
    e = mix(top, bottom, f.y);
  }
  float lo = draw.extra[0].y;
  float hi = draw.extra[0].z;
  float n = draw.extra[1].z;
  float t = clamp((e - lo) / max(hi - lo, 1e-6), 0.0, 1.0);
  float u = (t * (n - 1.0) + 0.5) / n;
  frag_color = textureLod(ramp, vec2(u, 0.5), 0.0) * draw.extra[0].w;
}
