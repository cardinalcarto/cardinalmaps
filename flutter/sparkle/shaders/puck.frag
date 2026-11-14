#version 460 core
// The navigation puck's chevron: an arrowhead pointing up the square (toward -y), filled
// and outlined, antialiased by its distance from the edge.
//
// `extra[0]`: the fill, premultiplied.
// `extra[1]`: the outline, premultiplied.
// `extra[2]`: y the chevron's length, z the outline's width inside its edge, both in
//     logical pixels.

#include <common.glsl>

in vec2 v_local;

out vec4 frag_color;

// The edge from `a` to `b` of a polygon: the squared distance from `p` to it in `d`, and
// whether a ray from `p` to the right crosses it, which flips `inside`.
void edge(vec2 p, vec2 a, vec2 b, inout float d, inout bool inside) {
  vec2 e = b - a;
  vec2 w = p - a;
  vec2 q = w - e * clamp(dot(w, e) / dot(e, e), 0.0, 1.0);
  d = min(d, dot(q, q));
  if ((a.y > p.y) != (b.y > p.y) && p.x < a.x + e.x * (p.y - a.y) / e.y) {
    inside = !inside;
  }
}

// The signed distance from `p` to a chevron `s` long, negative inside: a tip at the top,
// two tails at the bottom and a notch between them.
float chevron(vec2 p, float s) {
  vec2 tip = vec2(0.0, -0.5) * s;
  vec2 right = vec2(0.42, 0.5) * s;
  vec2 notch = vec2(0.0, 0.22) * s;
  vec2 left = vec2(-0.42, 0.5) * s;
  float d = 1e20;
  bool inside = false;
  edge(p, tip, right, d, inside);
  edge(p, right, notch, d, inside);
  edge(p, notch, left, d, inside);
  edge(p, left, tip, d, inside);
  return (inside ? -1.0 : 1.0) * sqrt(d);
}

void main() {
  float d = chevron(v_local, draw.extra[2].y);
  // About a pixel on screen, however the square is tilted.
  float aa = max(fwidth(d), 1e-4);
  float shape = clamp(0.5 - d / aa, 0.0, 1.0);
  float fill = clamp(0.5 - (d + draw.extra[2].z) / aa, 0.0, 1.0);
  frag_color = mix(draw.extra[1], draw.extra[0], fill) * shape;
}
