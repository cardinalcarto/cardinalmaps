#version 460 core
// Tile clipping masks: a quad over `0..extent` written into the stencil buffer.

#define SPARKLE_VERTEX
#include <common.glsl>

// A corner of the unit square.
in vec2 position;

void main() {
  gl_Position = to_clip(position * draw.extent);
}
