// Included by every shader. The `Draw` block matches `DrawUniforms` in
// `crates/sparkle_flutter/src/render/view.rs` (std140), and `dem` is the DEM tile under the
// drawn tile, as `sparkle::dem::DemData::to_rgba` makes it, border included (a 1x1
// placeholder without terrain).

uniform Draw {
  // Tile coordinates (`0..extent`; z in logical pixels) to clip space.
  mat4 matrix;
  // Tile units per logical pixel.
  float units_per_pixel;
  float pixel_ratio;
  float zoom;
  float extent;
  // In physical pixels.
  vec2 viewport;
  // In radians.
  float bearing;
  float pitch;
  // Whatever a layer type needs besides its paint attributes.
  vec4 extra[8];
  // `sparkle::paint::PaintUniforms`: one per paint attribute, for uniform bindings
  // (scalars in `.x`, pairs in `.xy`), then composite factors, four to a vec4.
  vec4 paint_values[12];
  vec4 paint_factors[3];
  // Positions relative to the center of the view (logical pixels, x east, y south, z up)
  // to clip space, for the globe.
  mat4 view;
  // The tile `matrix` places: column, row, tiles per side, extent.
  vec4 tile;
  // From Mercator to the globe (0..1), the globe's radius in logical pixels, and the
  // center's longitude and latitude in radians.
  vec4 globe;
  // Logical pixels per meter of terrain (0 without), unused, and the DEM's pixels per
  // side without the border.
  vec4 terrain;
  // The tile's offset (xy) and size (z) in its DEM tile, as fractions of it.
  vec4 dem;
  // The clip w of the center and of the horizon, fog-ground-blend, fog opacity.
  vec4 fog;
  vec4 fog_color;
  // The camera in `view`'s frame.
  vec4 eye;
}
draw;

const float PI = 3.141592653589793;

#ifdef SPARKLE_VERTEX

uniform sampler2D dem;

float dem_texel(vec2 p) {
  float last = draw.terrain.z + 1.0;
  vec2 texel = clamp(p, vec2(0.0), vec2(last));
  vec4 c = floor(textureLod(dem, (texel + 0.5) / (last + 1.0), 0.0) * 255.0 + 0.5);
  return -10000.0 + (c.r * 65536.0 + c.g * 256.0 + c.b) * 0.1;
}

// The terrain's height under `pos` (tile units) in logical pixels, sampled bilinearly
// between pixel centers as `sparkle::dem::DemData::elevation_at` does. 0 without terrain.
float elevation(vec2 pos) {
  if (draw.terrain.x <= 0.0) {
    return 0.0;
  }
  vec2 uv = draw.dem.xy + pos / draw.tile.w * draw.dem.z;
  vec2 xy = uv * draw.terrain.z - 0.5;
  vec2 base = floor(xy);
  vec2 f = xy - base;
  // Pixel (0, 0) is at (1, 1) behind the border.
  vec2 p = base + 1.0;
  float top = mix(dem_texel(p), dem_texel(p + vec2(1.0, 0.0)), f.x);
  float bottom = mix(dem_texel(p + vec2(0.0, 1.0)), dem_texel(p + vec2(1.0, 1.0)), f.x);
  return mix(top, bottom, f.y) * draw.terrain.x;
}

#endif

// World coordinates (0..1, y down) of a point of the tile.
vec2 tile_world(vec2 pos) {
  return (draw.tile.xy + pos / draw.tile.w) / draw.tile.z;
}

// Where world point `world`, `height` logical pixels up, is on the globe relative to the
// center of the view, as `sparkle::projection::Globe::position`.
vec3 globe_position_of(vec2 world, float height) {
  float lng = world.x * 2.0 * PI - PI;
  float lat = 2.0 * atan(exp(PI - 2.0 * PI * world.y)) - PI / 2.0;
  float lat0 = draw.globe.w;
  float dl = lng - draw.globe.z;
  float east = cos(lat) * sin(dl);
  float north = sin(lat) * cos(lat0) - cos(lat) * sin(lat0) * cos(dl);
  float up = sin(lat) * sin(lat0) + cos(lat) * cos(lat0) * cos(dl);
  float r = draw.globe.y + height;
  return vec3(r * east, -r * north, r * up - draw.globe.y);
}

vec3 globe_position(vec2 pos, float height) {
  return globe_position_of(tile_world(pos), height);
}

// A tile point `height` logical pixels above sea level in clip space: on the Mercator
// plane through `matrix`, on the globe, or blended between them.
vec4 project(vec2 pos, float height) {
  vec4 flat_position = draw.matrix * vec4(pos, height, 1.0);
  float t = draw.globe.x;
  if (t <= 0.0) {
    return flat_position;
  }
  vec4 sphere = draw.view * vec4(globe_position(pos, height), 1.0);
  return mix(flat_position, sphere, t);
}

// Whether a tile point is on the far side of the globe, hidden behind it.
bool globe_hidden(vec2 pos) {
  if (draw.globe.x < 0.5) {
    return false;
  }
  vec3 p = globe_position(pos, 0.0);
  vec3 normal = p + vec3(0.0, 0.0, draw.globe.y);
  return dot(normal, draw.eye.xyz - p) < -1e-3 * draw.globe.y * draw.globe.y;
}

#ifdef SPARKLE_VERTEX

// A tile point on the ground (on the terrain, if any) in clip space.
vec4 to_clip(vec2 pos) {
  return project(pos, elevation(pos));
}

// A tile point `height` logical pixels above the ground in clip space.
vec4 to_clip_above(vec2 pos, float height) {
  return project(pos, height + elevation(pos));
}

#endif

// How far a clip position is into the fog, 0 (none) to 1.
float fog_amount(float w) {
  if (draw.fog.w <= 0.0) {
    return 0.0;
  }
  float depth = (w - draw.fog.x) / max(draw.fog.y - draw.fog.x, 1e-3);
  float start = draw.fog.z;
  float f = clamp((depth - start) / max(1.0 - start, 1e-3), 0.0, 1.0);
  return f * f * draw.fog.w;
}
