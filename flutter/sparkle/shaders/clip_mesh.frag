#version 460 core
// Masks and depth over terrain: their pipelines write no color. A file of its own, as
// shaders of one file share a name in the bundle.

out vec4 frag_color;

void main() {
  frag_color = vec4(0.0);
}
