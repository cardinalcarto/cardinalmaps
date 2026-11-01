#version 460 core
// `extra[0].x`: how far to pull depths toward the camera, in normalized depth.
//
// Tile clipping masks over terrain or the globe: the terrain mesh of the clip tile written
// into the stencil buffer, depth tested against the terrain so that tiles behind hills or
// on the far side of the globe don't claim the pixels in front of them. Also the terrain's
// depth alone, for 3D layers drawn offscreen.

#define SPARKLE_VERTEX
#include <common.glsl>

in vec2 uv;

void main() {
  vec4 p = to_clip(uv * draw.tile.w);
  // Masks are pulled this far in front of the terrain drawn at the same place, which a
  // neighbouring or parent tile may have drawn at slightly different depths. Depth isn't
  // linear in distance: keep it tiny, or masks claim pixels of hills in front of them.
  p.z = max(p.z - draw.extra[0].x * p.w, 0.0);
  gl_Position = p;
}
