
uniform sampler2D image;

const float MODE_DASH = 1.0;
const float MODE_PATTERN = 2.0;
const float MODE_GRADIENT = 3.0;
// `sparkle::pattern::DASH_SDF_RANGE`.
const float DASH_SDF_RANGE = 2.0;

in vec4 v_color;
in vec2 v_normal;
in vec4 v_widths;
in vec2 v_side;
in vec4 v_tex;
flat in vec4 v_image_a;
flat in vec4 v_image_b;
flat in float v_fade;
flat in vec3 v_mode_size;

out vec4 frag_color;

// One repeat of an atlas image at `uv`, kept half a texel inside it so that filtering
// doesn't reach the neighbours.
vec4 sample_pattern(vec4 rect, vec2 uv) {
  if (rect.z <= 0.0 || rect.w <= 0.0) {
    return vec4(0.0);
  }
  vec2 texel = clamp(fract(uv) * rect.zw, vec2(0.5), rect.zw - 0.5);
  return textureLod(image, (rect.xy + texel) / v_mode_size.yz, 0.0);
}

// A dash pattern's signed distance in pixels: positive inside dashes.
float dash_distance(vec4 row, float along, float across, float pixels_per_width) {
  if (row.z <= 0.0) {
    // A solid line.
    return 1e3;
  }
  float y = (row.x + across * row.y) / v_mode_size.z;
  float v = textureLod(image, vec2(along, y), 0.0).r;
  return (v - 0.5) * 2.0 * DASH_SDF_RANGE * pixels_per_width;
}

void main() {
  float outset = v_widths.x;
  float inset = v_widths.y;
  float blur = v_widths.z;
  float dist = length(v_normal) * outset;
  float alpha = clamp(min(dist - (inset - blur), outset - dist) / blur, 0.0, 1.0);

  float mode = v_mode_size.x;
  vec4 color = v_color;
  if (mode == MODE_DASH) {
    float across = clamp(v_side.x, -1.0, 1.0);
    float a = dash_distance(v_image_a, v_tex.x, across, v_tex.z);
    float b = dash_distance(v_image_b, v_tex.y, across, v_tex.w);
    alpha *= clamp(mix(a, b, v_fade) / blur + 0.5, 0.0, 1.0);
  } else if (mode == MODE_PATTERN) {
    float y = clamp(v_side.x, -1.0, 1.0) * 0.5 + 0.5;
    vec4 a = sample_pattern(v_image_a, vec2(v_tex.x, y));
    vec4 b = sample_pattern(v_image_b, vec2(v_tex.y, y));
    color = mix(a, b, v_fade) * v_color.a;
  } else if (mode == MODE_GRADIENT) {
    vec4 ramp = textureLod(image, vec2(v_side.y, 0.5), 0.0);
    color = ramp * v_color.a;
  }
  frag_color = color * alpha;
}
