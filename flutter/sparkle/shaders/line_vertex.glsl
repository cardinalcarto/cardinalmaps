// Lines widened from their centerline (see `sparkle::tile::LineBucket`), in one of four
// modes: a plain color, dashes from a signed distance atlas, a sprite pattern, or a
// `line-gradient` ramp. `image` is the mode's texture.
//
// `extra[0]`: x the mode (0 color, 1 dashes, 2 pattern, 3 gradient), y 1 when the dash
//     pattern or image is per vertex (`image_a`/`image_b`) rather than in `extra`, z the
//     cross-fade from the `a` to the `b` image, w how much the map has grown since the
//     integer zoom images are laid out at.
// `extra[1]`: xy `line-translate` in tile units, z `line-layer-opacity`.
// `extra[2]`, `extra[3]`: dash patterns a and b: atlas row at the center of the line, rows
//     from there to an edge, repeat length in line widths, cross-fade scale. Per-vertex
//     patterns only take the scales from here.
// `extra[4]`, `extra[5]`: pattern images a and b, `[x, y, width, height]` in atlas pixels.
// `extra[6]`: xy the size of pattern a in logical pixels, zw of pattern b.
// `extra[7]`: xy the texture's size in texels.
//
// With `LINE_MERGED` (`line_merged.vert`), for lines merged from several layers into
// one draw, each vertex takes its layer's paint and extras from the `Layers` block
// instead (see `line_layers.glsl`). Merged layers have uniform paint, a color or dashes
// from the layer-wide atlas.

#define SPARKLE_VERTEX
#include <common.glsl>

#ifdef LINE_MERGED
#include <line_layers.glsl>

// The style index of the vertex's layer.
in float layer;
// Where the vertex's layer's values start in `layers.data`.
int layer_base = 0;

vec4 layer_value(int i) {
  return layers.data[layer_base + i];
}

vec4 paint_color() {
  return layer_value(0);
}

float paint_opacity() {
  return 1.0;
}

float paint_width() {
  return layer_value(1).x;
}

float paint_blur() {
  return layer_value(1).y;
}

float paint_gap_width() {
  return layer_value(1).z;
}

float paint_offset() {
  return layer_value(1).w;
}

float paint_floorwidth() {
  return layer_value(2).x;
}

// The extras the code below reads, as `draw.extra` has them for one layer.
vec4 extra(int i) {
  if (i == 0) {
    return vec4(layer_value(2).y, 0.0, layer_value(2).zw);
  } else if (i == 1) {
    // The opacities are in the color.
    return vec4(layer_value(3).xy * draw.units_per_pixel, 1.0, layer_value(3).w);
  } else if (i == 2) {
    return layer_value(4);
  } else if (i == 3) {
    return layer_value(5);
  } else if (i == 7) {
    return layer_value(6);
  }
  return vec4(0.0);
}
#else
#include <paint.glsl>

PAINT_VEC4(color, 0)
PAINT_FLOAT(opacity, 1)
PAINT_FLOAT(width, 2)
PAINT_FLOAT(blur, 3)
PAINT_FLOAT(gap_width, 4)
PAINT_FLOAT(offset, 5)
PAINT_FLOAT(floorwidth, 6)

vec4 extra(int i) {
  return draw.extra[i];
}
#endif

const float MODE_DASH = 1.0;
const float MODE_PATTERN = 2.0;
const float MODE_GRADIENT = 3.0;

// `sparkle::tile::LineVertex`.
in vec2 pos;
in vec2 extrude;
in vec2 offset_dir;
// Distance, along, progress, round.
in vec4 metrics;
// The feature's `sparkle::pattern::FeatureImage`, for images per vertex.
in vec4 feature_a;
in vec4 feature_b;

// Premultiplied; just the opacity for patterns and gradients.
out vec4 v_color;
// The extrude scaled to at most 1: its length is the distance from the centerline as
// a fraction of `widths.x`, exactly, around round joins too. Round caps' corners keep
// theirs, so that the cap is cut to a half circle.
out vec2 v_normal;
// In logical pixels: the outer edge's distance from the centerline, the inner edge's
// (non-zero with `line-gap-width`), the blur including antialiasing, and the distance
// along the line.
out vec4 v_widths;
// x from -1 on the left edge to 1 on the right; y the line progress.
out vec2 v_side;
// Dashes: repeats along the line for a and b, and pixels per line width for a and b.
// Patterns: repeats along the line for a and b.
out vec4 v_tex;
flat out vec4 v_image_a;
flat out vec4 v_image_b;
// The cross-fade between a and b.
flat out float v_fade;
// What the fragment shader needs of the extras: the mode and the texture's size.
flat out vec3 v_mode_size;

void main() {
#ifdef LINE_MERGED
  layer_base = (int(layer + 0.5) - int(layers.header.x + 0.5)) * LINE_LAYER_VEC4S;
  if (extra(1).w <= 0.0) {
    // A layer of the merged draw that isn't drawn this frame.
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
#endif
  vec2 translate = extra(1).xy;
  v_mode_size = vec3(extra(0).x, extra(7).xy);
  float mode = extra(0).x;
  float opacity = paint_opacity() * extra(1).z;
  float width = max(paint_width(), 0.0);
  float half_gap = max(paint_gap_width(), 0.0) * 0.5;

  // Lines thinner than a physical pixel would flicker in and out of the samples, so
  // draw them a pixel wide and fade them out instead.
  float physical = width * draw.pixel_ratio;
  if (physical < 1.0) {
    opacity *= physical;
    width = 1.0 / draw.pixel_ratio;
  }
  // Half a physical pixel on each side for the antialiased edge, as MapLibre does.
  float aa = 0.5 / draw.pixel_ratio;
  float outset = width * 0.5 + aa;
  float inset = 0.0;
  if (half_gap > 0.0) {
    inset = half_gap + aa;
    outset = half_gap + width + aa;
  }
  // Positive offsets move the line to the right of its direction, which is where
  // `offset_dir` points.
  vec2 dist = extrude * outset;
  vec2 shift = dist + offset_dir * paint_offset();

  vec4 position = to_clip(pos + shift * draw.units_per_pixel + translate);
  gl_Position = position;

  // In pitched views the far side's pixels are smaller; scale the blur so that it stays
  // about a pixel on screen.
  float gamma = 1.0;
  float dist_len = length(dist);
  if (dist_len > 1e-3) {
    vec4 projected = draw.matrix * vec4(dist * draw.units_per_pixel, 0.0, 0.0);
    float on_screen =
        length(projected.xy / position.w * draw.viewport * 0.5) / draw.pixel_ratio;
    gamma = dist_len / max(on_screen, 1e-6);
  }
  float blur = (max(paint_blur(), 0.0) + 1.0 / draw.pixel_ratio) * gamma;

  float distance = metrics.x / draw.units_per_pixel + metrics.y * outset;
  v_normal = metrics.w > 0.5 ? extrude : extrude / max(length(extrude), 1.0);
  v_widths = vec4(outset, inset, blur, distance);
  float across = dot(extrude, offset_dir) / max(dot(offset_dir, offset_dir), 1e-6);
  v_side = vec2(across, metrics.z);
  v_color = paint_color() * opacity;
  v_fade = extra(0).z;
  v_tex = vec4(0.0);
  v_image_a = vec4(0.0);
  v_image_b = vec4(0.0);

  bool per_vertex = extra(0).y > 0.5;
  float floorwidth = max(paint_floorwidth(), 1e-3);
  if (mode == MODE_DASH) {
    vec4 a = extra(2);
    vec4 b = extra(3);
    if (per_vertex) {
      // The feature's own pattern, keeping the cross-fade scales from `extra`.
      a = vec4(feature_a.xyz, a.w);
      b = vec4(feature_a.xyz, b.w);
    }
    // Dashes are laid out in widths of the integer zoom and grow with the map from there.
    float unit_a = floorwidth * a.w * extra(0).w;
    float unit_b = floorwidth * b.w * extra(0).w;
    v_tex = vec4(distance / max(a.z * unit_a, 1e-6), distance / max(b.z * unit_b, 1e-6),
                 unit_a, unit_b);
    v_image_a = a;
    v_image_b = b;
  } else if (mode == MODE_PATTERN) {
    vec4 rect_a = extra(4);
    vec4 rect_b = extra(5);
    vec2 size_a = extra(6).xy;
    vec2 size_b = extra(6).zw;
    if (per_vertex) {
      rect_a = feature_a;
      rect_b = feature_a;
      size_a = feature_a.zw / max(feature_b.x, 1e-6);
      size_b = size_a;
      v_fade = 1.0;
    }
    // As in MapLibre, the image is stretched across the line's width and keeps its
    // aspect ratio along it, at the integer zoom's width.
    v_tex = vec4(distance * size_a.y / max(size_a.x * floorwidth, 1e-6),
                 distance * size_b.y / max(size_b.x * floorwidth, 1e-6), 0.0, 0.0);
    v_image_a = rect_a;
    v_image_b = rect_b;
    v_color = vec4(opacity);
  } else if (mode == MODE_GRADIENT) {
    v_color = vec4(opacity);
  }
}
