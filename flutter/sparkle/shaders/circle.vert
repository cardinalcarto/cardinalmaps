#version 460 core
// `circle` layers: a quad per circle, sized here and rounded in the fragment shader.
//
// `extra[0].x`: 1 when `circle-pitch-scale` is `map`. `extra[0].y`: 1 when
// `circle-pitch-alignment` is `map`. `extra[0].z`: the distance from the camera to the
// center of the view in logical pixels, which is clip `w` there.
// `extra[1].xy`: `circle-translate` in tile units.

#define SPARKLE_VERTEX
#include <common.glsl>
#include <paint.glsl>

in vec2 center;
in vec2 extrude;

PAINT_VEC4(color, 0)
PAINT_FLOAT(radius, 1)
PAINT_FLOAT(blur, 2)
PAINT_FLOAT(opacity, 3)
PAINT_VEC4(stroke_color, 4)
PAINT_FLOAT(stroke_width, 5)
PAINT_FLOAT(stroke_opacity, 6)

out vec4 v_color;
out vec4 v_stroke_color;
// The quad corner, scaled so the circle's outer edge (with stroke) is at length 1.
out vec2 v_extrude;
// `circle-blur` as a fraction of the outer radius, and where the fill ends and the
// stroke starts, also as a fraction of it (1 without a stroke).
out vec2 v_shape;

void main() {
  float radius = max(paint_radius(), 0.0);
  float stroke_width = max(paint_stroke_width(), 0.0);
  float size = radius + stroke_width;
  bool scale_with_map = draw.extra[0].x > 0.5;
  bool pitch_with_map = draw.extra[0].y > 0.5;
  float camera_to_center = draw.extra[0].z;
  vec2 pos = center + draw.extra[1].xy;

  v_color = vec4(0.0);
  v_stroke_color = vec4(0.0);
  v_extrude = vec2(0.0);
  v_shape = vec2(0.0);
  if (globe_hidden(pos)) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
  if (pitch_with_map) {
    // Flat on the map: extrude in tile units, so the disc is foreshortened with it.
    vec2 offset = extrude * size * draw.units_per_pixel;
    if (!scale_with_map) {
      // Far circles would shrink with the map; scale them back up to their size at
      // the center of the view.
      offset *= to_clip(pos).w / camera_to_center;
    }
    gl_Position = to_clip(pos + offset);
  } else {
    // Facing the viewer: extrude in clip space, where `w` pixels make one logical
    // pixel per unit after the perspective divide.
    vec4 position = to_clip(pos);
    vec2 pixels_to_clip = 2.0 * draw.pixel_ratio / draw.viewport * vec2(1.0, -1.0);
    float w = position.w;
    if (scale_with_map) {
      // Constant in clip space, so the perspective divide shrinks far circles.
      w = camera_to_center;
    }
    gl_Position = position + vec4(extrude * size * pixels_to_clip * w, 0.0, 0.0);
  }

  v_color = paint_color() * paint_opacity();
  v_stroke_color = paint_stroke_color() * paint_stroke_opacity();
  v_extrude = extrude;
  float fill_end = 1.0;
  if (stroke_width >= 0.01 && size > 0.0) {
    fill_end = radius / size;
  }
  v_shape = vec2(max(paint_blur(), 0.0), fill_end);
}
