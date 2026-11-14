#version 460 core
// The navigation puck: a chevron, an arrowhead lying flat on the ground pointing along
// the bearing (toward -y in its frame), over a disk that faces the screen, a circle
// centered on it however the map is tilted. Each is filled and outlined and antialiased
// by its distance from its edge, measured where it lies: the chevron's on the ground, the
// disk's on screen, so `fwidth` puts both edges about a pixel wide on screen. The disk
// hides the route line just behind the chevron, where the traveled part, drawn only up to
// the last fix, lags it; flat, a tilt would squash it along the very direction of travel
// it has to cover.
//
// `extra[0]`: the chevron's fill, premultiplied.
// `extra[1]`: the chevron's outline, premultiplied.
// `extra[2]`: y the chevron's length, z its outline's width inside its edge, w the disk's
//     radius (0 for none), all in logical pixels.
// `extra[3]`: the disk's fill, premultiplied.
// `extra[4]`: the disk's outline, premultiplied.
// `extra[5]`: x the disk's outline's width inside its edge, in logical pixels.

#include <common.glsl>

in vec2 v_screen;
in vec3 v_ground;

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

// A shape `d` from its edge (negative inside), filled and outlined `width` in from that
// edge, its edges about a pixel wide on screen however it's tilted.
vec4 shade(float d, vec4 fill_color, vec4 outline_color, float width) {
  float aa = max(fwidth(d), 1e-4);
  float shape = clamp(0.5 - d / aa, 0.0, 1.0);
  float fill = clamp(0.5 - (d + width) / aa, 0.0, 1.0);
  return mix(outline_color, fill_color, fill) * shape;
}

void main() {
  // The point of the ground drawn here. The rectangle can reach above the horizon on a
  // steep tilt, where no ground is drawn (z <= 0), and so no chevron.
  vec2 ground = v_ground.xy / max(v_ground.z, 1e-6);
  float d = chevron(ground, draw.extra[2].y);
  vec4 arrow = shade(d, draw.extra[0], draw.extra[1], draw.extra[2].z);
  // Toward the horizon the ground stretches fast, so `fwidth` there would smear a
  // fragment far off the chevron into a faint edge; nothing a chevron's length away is
  // part of it.
  arrow *= float(v_ground.z > 0.0 && d < draw.extra[2].y);
  // Computed for every fragment even without a disk, as `fwidth` needs its neighbors.
  vec4 disk = shade(length(v_screen) - draw.extra[2].w, draw.extra[3], draw.extra[4],
                    draw.extra[5].x);
  // A radius of 0 would still leave a speck at the middle.
  disk *= step(1e-4, draw.extra[2].w);
  // The chevron over the disk, both premultiplied.
  frag_color = arrow + disk * (1.0 - arrow.a);
}
