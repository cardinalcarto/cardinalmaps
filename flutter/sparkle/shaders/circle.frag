#version 460 core

in vec4 v_color;
in vec4 v_stroke_color;
in vec2 v_extrude;
in vec2 v_shape;

out vec4 frag_color;

void main() {
  float dist = length(v_extrude);
  // About one physical pixel in extrude units, wherever the circle is and however it is
  // pitched, so edges stay smooth without blurring large circles.
  float pixel = fwidth(dist);
  float blur = max(v_shape.x, pixel);
  float opacity = 1.0 - smoothstep(-blur, 0.0, dist - 1.0);
  float stroke = 0.0;
  if (v_shape.y < 1.0) {
    stroke = smoothstep(-blur, 0.0, dist - v_shape.y);
  }
  frag_color = opacity * mix(v_color, v_stroke_color, stroke);
}
