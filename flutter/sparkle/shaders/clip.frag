#version 460 core
// Tile clipping masks: their pipeline writes no color.

out vec4 frag_color;

void main() {
  frag_color = vec4(0.0);
}
