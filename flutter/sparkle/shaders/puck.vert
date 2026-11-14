#version 460 core
// The navigation puck: a square on the ground around it, which `puck.frag` cuts the
// chevron out of. `matrix` places the unit square, already turned to the puck's bearing,
// in clip space (see `crates/sparkle_flutter/src/render/puck.rs`), so it lies flat on the
// map and tilts with it.
//
// `extra[2].x`: half the square's side, in logical pixels.

#include <common.glsl>

// A corner of the unit square, drawn as a strip.
in vec2 position;

// Where in the square, in logical pixels from the puck, x right and y back from its tip.
out vec2 v_local;

void main() {
  gl_Position = draw.matrix * vec4(position, 0.0, 1.0);
  v_local = (position * 2.0 - 1.0) * draw.extra[2].x;
}
