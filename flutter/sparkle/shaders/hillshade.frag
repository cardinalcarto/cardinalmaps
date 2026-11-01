#version 460 core
// Hillshading from a tile's slopes (`sparkle::dem::DemData::hillshade_slopes`), with
// MapLibre's methods.
//
// `extra[0]`: exaggeration, method (0 standard, 1 combined, 2 igor, 3 multidirectional,
// 4 basic), light count, `extent`.
// `extra[1]`: the latitudes of the tile's top and bottom edges in radians, 1 to sample the
// nearest pixel instead of interpolating, pixels per side.
// `extra[2]`: the premultiplied accent color.
// `extra[3..5]`: light azimuths in radians, four per vec4.
// `extra[5..7]`: light altitudes in radians, four per vec4.
// `paint_values[0..6]`: premultiplied shadow colors, `paint_values[6..12]` highlight colors.

#include <common.glsl>

const int MAX_LIGHTS = 6;

// Sampled with `NEAREST` at pixel centers.
uniform sampler2D slopes;

in vec2 v_uv;

out vec4 frag_color;

float azimuth(int i) {
  return draw.extra[3 + i / 4][i - 4 * (i / 4)];
}

float altitude(int i) {
  return draw.extra[5 + i / 4][i - 4 * (i / 4)];
}

vec4 shadow(int i) {
  return draw.paint_values[i];
}

vec4 highlight(int i) {
  return draw.paint_values[MAX_LIGHTS + i];
}

float get_aspect(vec2 d) {
  if (d.x != 0.0) {
    return atan(d.y, -d.x);
  }
  return PI / 2.0 * (d.y > 0.0 ? 1.0 : -1.0);
}

vec4 standard(vec2 d) {
  float exaggeration = draw.extra[0].x;
  float az = azimuth(0) + PI;
  float slope = atan(0.625 * length(d));
  float aspect = get_aspect(d);
  // Steeper slopes shade more as the exaggeration grows, along an exponential curve.
  float base = max(1.875 - exaggeration * 1.75, 1e-4);
  float max_value = 0.5 * PI;
  float scaled = slope;
  if (abs(exaggeration - 0.5) > 1e-6) {
    scaled = ((pow(base, slope) - 1.0) / (pow(base, max_value) - 1.0)) * max_value;
  }
  float accent = cos(scaled);
  float k = clamp(exaggeration * 2.0, 0.0, 1.0);
  vec4 accent_color = (1.0 - accent) * draw.extra[2] * k;
  float shade = abs(mod((aspect + az) / PI + 0.5, 2.0) - 1.0);
  vec4 shade_color = mix(shadow(0), highlight(0), shade) * sin(scaled) * k;
  return accent_color * (1.0 - shade_color.a) + shade_color;
}

// How directly a light at `az`, `alt` shines on ground sloping by `d`: 1 facing it, 0
// edge on.
float incidence(vec2 d, float az, float alt) {
  float ca = cos(az);
  float sa = sin(az);
  float cl = cos(alt);
  float sl = sin(alt);
  return (sl - (d.y * ca * cl - d.x * sa * cl)) / sqrt(1.0 + dot(d, d));
}

vec4 combined(vec2 d0) {
  vec2 d = d0 * draw.extra[0].x * 2.0;
  float cang =
      clamp(acos(clamp(incidence(d, azimuth(0) + PI, altitude(0)), -1.0, 1.0)), 0.0, PI / 2.0);
  float w = atan(length(d)) * 4.0 / (PI * PI);
  return shadow(0) * (cang * w) + highlight(0) * ((PI / 2.0 - cang) * w);
}

vec4 igor(vec2 d0) {
  vec2 d = d0 * draw.extra[0].x * 2.0;
  float aspect = get_aspect(d);
  float az = azimuth(0) + PI;
  float slope_strength = atan(length(d)) * 2.0 / PI;
  float aspect_strength = 1.0 - abs(mod((aspect + az) / PI + 0.5, 2.0) - 1.0);
  float shadow_strength = slope_strength * aspect_strength;
  float highlight_strength = slope_strength * (1.0 - aspect_strength);
  return shadow(0) * shadow_strength + highlight(0) * highlight_strength;
}

vec4 directional(vec2 d, int i) {
  float s = clamp(incidence(d, azimuth(i) + PI, altitude(i)), 0.0, 1.0);
  if (s > 0.5) {
    return highlight(i) * (2.0 * s - 1.0);
  }
  return shadow(i) * (1.0 - 2.0 * s);
}

vec4 multidirectional(vec2 d0) {
  vec2 d = d0 * draw.extra[0].x * 2.0;
  int count = int(draw.extra[0].z);
  vec4 color = vec4(0.0);
  for (int i = 0; i < MAX_LIGHTS; i++) {
    if (i >= count) {
      break;
    }
    color += directional(d, i) / float(count);
  }
  return color;
}

// The east and south slopes of a pixel, 16 bits each (`sparkle::dem::decode_slopes`).
vec2 slope_at(vec2 p) {
  float size = draw.extra[1].w;
  vec2 texel = clamp(p, vec2(0.0), vec2(size - 1.0));
  vec4 c = floor(textureLod(slopes, (texel + 0.5) / size, 0.0) * 255.0 + 0.5);
  return (vec2(c.r * 256.0 + c.g, c.b * 256.0 + c.a) / 65535.0 - 0.5) * 8.0;
}

void main() {
  // Interpolated by hand: filtering the bytes of a 16-bit value apart would be wrong.
  vec2 p = v_uv * draw.extra[1].w - 0.5;
  vec2 slope;
  if (draw.extra[1].z > 0.5) {
    slope = slope_at(floor(p + 0.5));
  } else {
    vec2 base = floor(p);
    vec2 f = p - base;
    vec2 top = mix(slope_at(base), slope_at(base + vec2(1.0, 0.0)), f.x);
    vec2 bottom = mix(slope_at(base + vec2(0.0, 1.0)), slope_at(base + vec2(1.0, 1.0)), f.x);
    slope = mix(top, bottom, f.y);
  }
  // Slopes are computed with the meters per pixel at the equator; they're steeper by
  // 1 / cos(latitude) here.
  float lat = mix(draw.extra[1].x, draw.extra[1].y, v_uv.y);
  vec2 d = slope / cos(lat);
  int method = int(draw.extra[0].y + 0.5);
  if (method == 1) {
    frag_color = combined(d);
  } else if (method == 2) {
    frag_color = igor(d);
  } else if (method == 3) {
    frag_color = multidirectional(d);
  } else if (method == 4) {
    frag_color = directional(d * draw.extra[0].x * 2.0, 0);
  } else {
    frag_color = standard(d);
  }
}
