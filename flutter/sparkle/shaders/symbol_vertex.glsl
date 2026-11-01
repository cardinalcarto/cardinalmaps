// Text and icons (see `sparkle::tile::SymbolBucket`), the vertex stage. Included by
// `symbol_text.vert` and `symbol_icon.vert`, which define `IS_TEXT` for text.
//
// `extra[0]`: is halo pass, pitch with map, rotate with map (each 0 or 1), and the
// `sparkle::symbol::placement::DrawMode`: 0 point, 1 along a line on the map, 2 along a
// line on screen.
// `extra[1].xy`: `*-translate` in tile units (point labels); `.z`: the clip w of the
// view's center.
// `extra[2].xy`: the atlas size in pixels.

#define SPARKLE_VERTEX
#include <common.glsl>
#include <paint.glsl>

// `sparkle::tile::SymbolVertex`, repacked as floats by the renderer: the anchor (xy) and
// offset (zw), the pixel offset (xy) and texture position (zw), the section color, and
// the flags.
in vec4 anchor_offset;
in vec4 pixel_offset_tex;
in vec4 section_color;
in float flags;
// From placement: see `sparkle::symbol::placement::DrawMode`. `.w` is the opacity.
in vec4 placed;

PAINT_VEC4(color, 0)
PAINT_VEC4(halo_color, 1)
PAINT_FLOAT(halo_width, 2)
PAINT_FLOAT(halo_blur, 3)
PAINT_FLOAT(opacity, 4)
PAINT_FLOAT(size, 5)
#ifndef IS_TEXT
PAINT_FLOAT(text_size, 6)
#endif

// `SymbolVertex::flags`
const int SDF = 1;
const int SECTION_COLOR = 2;
const int TEXT_SIZED = 4;

// Where the outline is in a distance field, and how many of its units are one glyph
// pixel (see `sparkle::glyphs`).
const float EDGE = 0.75;
const float SDF_PX = 8.0;

// Normalized texture coordinates.
out vec2 v_tex;
// Premultiplied, opacity included.
out vec4 v_color;
// Whether to read a distance field, the distance at the edge to draw, and extra
// smoothing for halo blur, in distance field units.
out vec3 v_sdf;

#ifdef IS_TEXT
float layout_scale(int f) {
  return paint_size() / 24.0;
}
#else
// Icons fitted to their text are laid out in text units.
float layout_scale(int f) {
  if ((f & TEXT_SIZED) != 0) {
    return paint_text_size() / 24.0;
  }
  return paint_size();
}
#endif

vec2 rotate(vec2 v, float angle) {
  float s = sin(angle);
  float c = cos(angle);
  return vec2(c * v.x - s * v.y, s * v.x + c * v.y);
}

// Outside clip space, so the triangle is dropped.
const vec4 HIDDEN = vec4(2.0, 2.0, 2.0, 1.0);

void main() {
  vec2 anchor = anchor_offset.xy;
  vec2 offset = anchor_offset.zw;
  vec2 pixel_offset = pixel_offset_tex.xy;
  int f = int(flags + 0.5);
  v_tex = vec2(0.0);
  v_color = vec4(0.0);
  v_sdf = vec3(0.0);
  if (placed.w <= 0.0 || globe_hidden(anchor)) {
    gl_Position = HIDDEN;
    return;
  }
  bool is_halo = draw.extra[0].x > 0.5;
  bool pitch_with_map = draw.extra[0].y > 0.5;
  bool rotate_with_map = draw.extra[0].z > 0.5;
  int mode = int(draw.extra[0].w + 0.5);
  float center_w = draw.extra[1].z;

  vec2 pos = anchor + draw.extra[1].xy;
  vec4 projected = to_clip(pos);
  // Labels far away shrink only half as much as the map does, as in MapLibre: those
  // drawn in the map plane are enlarged and those facing the viewer shrunk.
  float distance_ratio = center_w / projected.w;
  if (pitch_with_map) {
    distance_ratio = projected.w / center_w;
  }
  float perspective = clamp(0.5 + 0.5 * distance_ratio, 0.0, 4.0);
  float scale = layout_scale(f);

  if (mode == 0) {
    // In logical pixels, y down, moved to the label's variable anchor.
    vec2 shift = (offset + placed.xy) * scale * perspective + pixel_offset;
    if (pitch_with_map) {
      if (!rotate_with_map) {
        // Turn against the map so the label stays upright on screen.
        shift = rotate(shift, draw.bearing);
      }
      gl_Position = to_clip(pos + shift * draw.units_per_pixel);
    } else {
      if (rotate_with_map) {
        // Along the map's x axis as it appears on screen.
        vec4 east = to_clip(pos + vec2(draw.units_per_pixel, 0.0));
        vec2 d = (east.xy / east.w - projected.xy / projected.w) * vec2(1.0, -1.0);
        shift = rotate(shift, atan(d.y * draw.viewport.y, d.x * draw.viewport.x));
      }
      vec2 ndc = shift * draw.pixel_ratio * 2.0 / draw.viewport * vec2(1.0, -1.0);
      gl_Position = projected + vec4(ndc * projected.w, 0.0, 0.0);
    }
  } else {
    // A glyph placed along its line: centered on `placed.xy`, turned by `placed.z`.
    vec2 shift = rotate(offset * scale * perspective + pixel_offset, placed.z);
    if (mode == 1) {
      gl_Position = to_clip(placed.xy + shift * draw.units_per_pixel);
    } else {
      vec2 screen = (placed.xy + shift) * draw.pixel_ratio / draw.viewport;
      float depth = clamp(projected.z / projected.w, 0.0, 1.0);
      gl_Position = vec4(screen.x * 2.0 - 1.0, 1.0 - screen.y * 2.0, depth, 1.0);
    }
  }
  v_tex = pixel_offset_tex.zw / draw.extra[2].xy;

  float opacity = paint_opacity() * placed.w;
  bool is_sdf = (f & SDF) != 0;
  // Halo widths and blur are in pixels, distance field units are glyph pixels.
  float font_scale = max(scale * perspective, 1e-3);
  if (is_halo) {
    v_color = paint_halo_color() * opacity;
    float width = paint_halo_width();
    float edge = (6.0 - width / font_scale) / SDF_PX;
    v_sdf = vec3(1.0, edge, paint_halo_blur() * 1.19 / SDF_PX / font_scale);
    if (!is_sdf || width <= 0.0) {
      gl_Position = HIDDEN;
    }
  } else {
    vec4 color = paint_color();
    if ((f & SECTION_COLOR) != 0) {
      color = section_color;
    }
    if (is_sdf) {
      v_color = color * opacity;
      v_sdf = vec3(1.0, EDGE, 0.0);
    } else {
      v_color = vec4(opacity);
      v_sdf = vec3(0.0);
    }
  }
}
