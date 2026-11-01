// Paint attribute inputs and accessors for vertex shaders; see
// `crates/sparkle_flutter/src/render/paint.rs`. Each property has inputs sized for its
// composite form, and its value is the uniform value (zero unless the property is a
// uniform) plus its two per-vertex values mixed by its composite factor (zero unless the
// property is composite). Use as, for paint attribute 2 named `width`:
//
//   PAINT_FLOAT(width, 2)
//
// which declares the inputs and `float paint_width()`.

#define PAINT_FACTOR(i) draw.paint_factors[(i) / 4][(i) - 4 * ((i) / 4)]

#define PAINT_FLOAT(name, i)                                                        \
  in vec2 a_##name;                                                                 \
  float paint_##name() {                                                            \
    return draw.paint_values[i].x + mix(a_##name.x, a_##name.y, PAINT_FACTOR(i));   \
  }

#define PAINT_VEC2(name, i)                                                         \
  in vec4 a_##name;                                                                 \
  vec2 paint_##name() {                                                             \
    return draw.paint_values[i].xy + mix(a_##name.xy, a_##name.zw, PAINT_FACTOR(i)); \
  }

#define PAINT_VEC4(name, i)                                                         \
  in vec4 a_##name;                                                                 \
  in vec4 a_##name##_b;                                                             \
  vec4 paint_##name() {                                                             \
    return draw.paint_values[i] + mix(a_##name, a_##name##_b, PAINT_FACTOR(i));     \
  }
