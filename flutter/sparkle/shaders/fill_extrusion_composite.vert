#version 460 core
// A fill-extrusion layer's offscreen image over the map; see
// `fill_extrusion_composite.frag`.

// A corner of the unit square, drawn as a strip over the whole screen.
in vec2 position;

// Where in the image the fragment is: rows run down from the top, as the image was drawn.
out vec2 v_uv;

void main() {
  gl_Position = vec4(position * 2.0 - 1.0, 0.0, 1.0);
  v_uv = vec2(position.x, 1.0 - position.y);
}
