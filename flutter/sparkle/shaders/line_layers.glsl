// The paint values and extras of a window of style layers, for lines of several layers
// merged into one draw (see `crates/sparkle_flutter/src/render/layers/line.rs`): each
// vertex of such a draw carries its layer's style index.

// As many as fit beside the `Draw` block within what impellerc accepts per stage.
#define LINE_LAYERS 48
// Per layer, what `line_vertex.glsl` reads of its paint (all uniform) and extras, combined
// to take few reads per vertex:
//   0: the color, premultiplied by `line-opacity` and `line-layer-opacity`
//   1: width, blur, gap width, offset
//   2: floorwidth, the mode, the cross-fade, how much the map has grown since the
//      integer zoom (`extra[0].x`, `.z`, `.w`)
//   3: xy `line-translate` in pixels, z unused, w 1 if the layer is drawn this frame
//   4, 5: dash patterns a and b (`extra[2]`, `extra[3]`)
//   6: the dash atlas's size (`extra[7]`)
#define LINE_LAYER_VEC4S 7

uniform Layers {
  // x: the style index of the window's first layer.
  vec4 header;
  vec4 data[LINE_LAYERS * LINE_LAYER_VEC4S];
}
layers;
