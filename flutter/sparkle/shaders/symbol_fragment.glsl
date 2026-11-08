// Text and icons, the fragment stage. Included by `symbol_text.frag` and
// `symbol_icon.frag`, which define `IS_TEXT` for text.

uniform sampler2D atlas;

in vec2 v_tex;
in vec4 v_color;
in vec3 v_sdf;

out vec4 frag_color;

void main() {
  vec4 texel = texture(atlas, v_tex);
  // Glyph atlases have one channel; SDF icons keep the distance in alpha.
#ifdef IS_TEXT
  float dist = texel.r;
#else
  float dist = texel.a;
#endif
  float aa = fwidth(dist) * 0.7;
  if (v_sdf.x < 0.5) {
    frag_color = texel * v_color.a;
    return;
  }
  float gamma = aa + v_sdf.z;
  float alpha = smoothstep(v_sdf.y - gamma, v_sdf.y + gamma, dist);
  frag_color = v_color * alpha;
}
