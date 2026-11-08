#version 460 core
// The sky behind the map: above a pitched plane's horizon, a gradient from the horizon
// color up to the sky color, with the haze of distant fog below it; around the globe, an
// atmosphere fading from the horizon color at its edge into space. Both blend by the
// globe factor.
//
// `matrix`: clip space to the frame of `view` (its inverse).
// `extra[0]`, `extra[1]`, `extra[2]`: the sky, horizon and haze colors, premultiplied.
// `extra[3]`: the horizon's y in normalized device coordinates on the plane,
//     `sky-horizon-blend`, `atmosphere-blend`, the globe factor.

// A corner of the unit square, drawn as a strip over the viewport.
in vec2 position;

out vec2 v_ndc;

void main() {
  v_ndc = position * 2.0 - 1.0;
  gl_Position = vec4(v_ndc, 0.0, 1.0);
}
