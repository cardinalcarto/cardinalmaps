#version 460 core
// Terrain tiles: `sparkle::terrain::TerrainMesh` over the tile, lifted by the DEM (see
// `elevation`) and textured with the layers rendered for the tile, then fogged with
// distance.
//
// `extra[0].x`: how far skirts hang below the edge, in logical pixels.

#define SPARKLE_VERTEX
#include <common.glsl>

in vec2 uv;
in float skirt;

out vec2 v_uv;
out float v_fog;

void main() {
  vec2 pos = uv * draw.tile.w;
  float ground = elevation(pos);
  float height = ground - skirt * draw.extra[0].x;
  vec4 flat_position = draw.matrix * vec4(pos, height, 1.0);
  vec4 position = flat_position;
  float t = draw.globe.x;
  if (t > 0.0) {
    vec2 world = tile_world(pos);
    // The skirts along the top of the world's first row and the bottom of its last
    // reach to the poles instead, closing the globe where Mercator ends. On the plane
    // they fold up into the edge.
    float n = draw.tile.z;
    bool top = draw.tile.y == 0.0 && uv.y == 0.0;
    bool bottom = draw.tile.y == n - 1.0 && uv.y == 1.0;
    if (skirt > 0.5 && (top || bottom)) {
      world.y = top ? -1.0 : 2.0;
      height = 0.0;
      flat_position = draw.matrix * vec4(pos, ground, 1.0);
    }
    vec4 sphere = draw.view * vec4(globe_position_of(world, height), 1.0);
    position = mix(flat_position, sphere, t);
  }
  gl_Position = position;
  v_uv = uv;
  v_fog = fog_amount(position.w);
}
