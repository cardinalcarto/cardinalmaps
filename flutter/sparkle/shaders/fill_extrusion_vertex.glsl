// Lighting and outputs shared by the roof and wall vertex shaders of extruded polygons
// (`fill_extrusion.vert`, `fill_extrusion_wall.vert`), lit as MapLibre lights them (see
// `sparkle::light::ExtrusionLight::shade`), drawn into the layer's offscreen target (or,
// writing no color, into the map's depth buffer).
//
// `extra[0]`: the light's position (xyz, tile coordinates) and intensity (w).
// `extra[1].rgb`: the light's color.
// `extra[2]`: `fill-extrusion-translate` in tile units (xy), logical pixels per meter (z),
//             1 for `fill-extrusion-vertical-gradient` (w).
// `extra[3]`: tile units per pattern pixel (x), the sprite atlas size in pixels (yz).
//
// Include after `common.glsl`, `paint.glsl` and the paint accessor `paint_color`.

// Opaque and premultiplied; with a pattern, the lighting to multiply it by.
out vec4 v_color;
// In pattern pixels: tile position on roofs, distance along and up the wall on walls.
out vec2 v_pattern_pos;
flat out vec4 v_pattern_rect;
flat out vec2 v_pattern_size;
// Where on the ground the fragment is, in (untranslated) tile coordinates.
out vec2 v_tile_pos;

// A vertex at `pos` (tile coordinates), `edge` along its wall's ring, with unit `normal`
// (`[0, 0, 1]` on roofs), at the feature's `height` (`top` 1) or `base` (`top` 0), both
// in meters and at least 0. Roofs, at the height, don't read the base.
void extrude(vec2 pos, float edge, vec3 normal, float top, float base, float height,
             vec4 pattern_rect, vec2 pattern_size) {
  float z = (top > 0.5 ? height : base) * draw.extra[2].z;

  gl_Position = to_clip_above(pos + draw.extra[2].xy, z);

  vec3 light_pos = draw.extra[0].xyz;
  float intensity = draw.extra[0].w;
  vec3 light_color = draw.extra[1].rgb;
  bool has_pattern = pattern_rect.z > 0.0;
  vec4 color = paint_color();
  float luminance = dot(color.rgb, vec3(0.2126, 0.7152, 0.0722));
  // MapLibre's normals reach its dot product doubled (packing), hence the 2.
  float directional = clamp(2.0 * dot(normal, light_pos), 0.0, 1.0);
  float upper = has_pattern ? max(0.5 + intensity, 1.0) : max(1.0 - luminance + intensity, 1.0);
  directional = mix(1.0 - intensity, upper, directional);
  if (normal.z == 0.0 && draw.extra[2].w > 0.5) {
    directional *=
        clamp((top + base) * sqrt(height / 150.0), mix(0.7, 0.98, 1.0 - intensity), 1.0);
  }
  vec3 surface = has_pattern ? vec3(1.0) : color.rgb + vec3(0.03);
  vec3 lit =
      clamp(surface * directional * light_color, mix(vec3(0.0), vec3(0.3), 1.0 - light_color),
            vec3(1.0));
  v_color = vec4(lit, 1.0);

  float units_per_pattern_pixel = draw.extra[3].x;
  if (normal.z == 0.0) {
    v_pattern_pos = vec2(edge, -z * draw.units_per_pixel) / units_per_pattern_pixel;
  } else {
    v_pattern_pos = pos / units_per_pattern_pixel;
  }
  v_pattern_rect = pattern_rect;
  v_pattern_size = pattern_size;
  v_tile_pos = pos;
}
