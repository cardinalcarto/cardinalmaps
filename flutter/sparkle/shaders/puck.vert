#version 460 core
// The navigation puck: a rectangle on screen around the chevron and its disk, which
// `puck.frag` cuts them out of (see `crates/sparkle_flutter/src/render/puck.rs`). The
// disk faces the screen, so it's measured on screen; the chevron lies flat on the map, so
// it's measured on the ground under each fragment. Drawn upright with clip w 1, every
// output is interpolated evenly across the screen, so the ground's homogeneous
// coordinates can be divided per fragment for the exact point of the ground.
//
// `matrix`: clip space to the ground around the puck in logical pixels from it, up to
//     scale (its `w`), x right and y back from the chevron's tip.
// `extra[6]`: the rectangle in clip space: left, top, right, bottom.
// `extra[7]`: the same in logical pixels from the puck on screen, y down.

#include <common.glsl>

// A corner of the unit square, drawn as a strip: x left to right, y top to bottom.
in vec2 position;

// Logical pixels from the puck's middle on screen, y down.
out vec2 v_screen;
// The ground under the fragment, in logical pixels from the puck, times `z`.
out vec3 v_ground;

void main() {
  vec2 clip = mix(draw.extra[6].xy, draw.extra[6].zw, position);
  gl_Position = vec4(clip, 0.0, 1.0);
  v_screen = mix(draw.extra[7].xy, draw.extra[7].zw, position);
  v_ground = (draw.matrix * vec4(clip, 0.0, 1.0)).xyw;
}
