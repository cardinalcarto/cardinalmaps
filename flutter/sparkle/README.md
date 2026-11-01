# sparkle for Flutter

MapLibre-style vector maps from tile servers or local tiles: processed in Rust by
[`sparkle`](../../crates/sparkle) and drawn with [Flutter GPU](https://docs.flutter.dev/)
(Impeller only). A port of [`sparkle_harness`](../../crates/sparkle_harness), the wgpu
test renderer; `example/` is the harness as a Flutter app.

## How it fits together

```
Dart (this package)                       Rust (crates/sparkle_flutter, via flutter_rust_bridge)
──────────────────                        ────────────────────────────────────────────────────
SparkleMap widget ── input, frame ticks ─▶ app.rs: MapView — camera, motion, style reload,
                                            hover/select (as the harness's window)
                                          sparkle_view: args, setup, tile loading and
                                            fetching, which tiles to draw (shared with
                                            the harness)
                                          render/: the harness's renderer, recording
                                            GPU commands instead of calling wgpu
GpuExecutor ◀── commands + data ───────── gpu.rs: the command stream
   │
   └─ flutter_gpu: buffers, textures, passes, draws ─▶ GpuImageSurface ─▶ RawImage
```

Everything the harness decides on the CPU (tile selection, symbol placement, paint
evaluation, terrain) stays in Rust; the Dart side only replays one command stream per
frame (`lib/src/executor.dart`). Shaders are GLSL in `shaders/`, compiled into a shader
bundle by `hook/build.dart` (with `flutter_gpu_shaders`) from `sparkle.shaderbundle.json`.

Where Flutter GPU differs from wgpu, the port works around it:

- Shaders are compiled ahead of time, so paint properties have fixed inputs sized for
  their zoom-and-feature form, and uniform-bound properties read zeros from a
  one-element instance buffer (see `crates/sparkle_flutter/src/render/paint.rs` and
  `shaders/paint.glsl`).
- Vertex attributes are 32-bit floats whose format matches the shader input exactly, so
  vertices with integer or 8-bit fields are repacked at upload.
- There are no texture arrays: each DEM tile under the terrain is a texture of its own.
- There's no `vertex_index` or `textureDimensions`; quads come from a shared buffer and
  texture sizes from uniforms.
- A render pass keeps bindings across draws (on GLES, an earlier program's uniform block
  wins), so the executor clears them and binds everything again for every draw, and only
  the vertex slots the bound layout has.
- Each program in `sparkle.shaderbundle.json` needs shader files of its own: two entries
  naming the same file break one of the programs.
- The GLES shaders are GLSL ES 3.00 (`glesLanguageVersion` in `hook/build.dart`), and
  inputs a shader doesn't read are compiled away, so a vertex layout mustn't name them.

Frames stay smooth while tiles load: each frame's commands are read by Dart in place in
Rust's memory, new tiles' contents travel apart from what draws bind, and a frame takes
in at most 2 MB of new tiles (the rest follow in the next frames). On Linux with AMD's
GL driver, the example sets `AMD_DEBUG=nooptvariant` in its runner: otherwise the driver
compiles optimized shader variants mid-frame when new kinds of labels first appear,
stalling that frame by 15–30 ms.

## Running the harness

Flutter GPU has to be turned on per platform; the example does it in its Linux runner
(`fl_dart_project_set_enable_flutter_gpu`), `AndroidManifest.xml`
(`io.flutter.embedding.android.EnableFlutterGPU`) and iOS/macOS `Info.plist`
(`FLTEnableFlutterGPU`).

The example takes the harness's command line (see `crates/sparkle_view/src/args.rs`),
except `--headless` and `--next-style`:

```sh
cd example
flutter build linux --release
cd ../../../crates
../flutter/sparkle/example/build/linux/x64/release/bundle/sparkle_example \
    https://tiles.example.org/seattle --view -122.335,47.608,14
```

Tiles come from a tile server's TileJSON URL (such as a [Martin](https://maplibre.org/martin/)
tileset's `https://host/name`) or a local `.mbtiles` file. Fetched tiles are cached on disk
(`--cache`, `--cache-size`): a cached tile is drawn at once and checked with the server in
the background once it's a day old, and tiles that can't be fetched are retried while
they're in view. Without arguments, as on Android, the example shows the tileset it was
built with:

```sh
flutter run --dart-define=SPARKLE_TILES_URL=https://tiles.example.org/seattle \
    [--dart-define=SPARKLE_VIEW=-122.335,47.608,13]
```

`--screenshot out.png` saves the first frame in which every visible tile has loaded, then
exits. The window is `--size` logical pixels (1024x768 by default), so on a 2x display
compare against `sparkle_harness --headless --size 2048x1536 --pixel-ratio 2`.

Drag to pan; right-drag or ctrl+drag to rotate and pitch; scroll, pinch or +/- to zoom;
arrows pan, shift+arrows rotate and pitch; N resets bearing and pitch; Esc quits. The
feature under the pointer gets feature state `hover=true`, and clicking toggles
`selected=true`. A `--style` file is reloaded when it changes, with paint transitions.

## Changing the Rust API

After changing `crates/sparkle_flutter/src/api/`, regenerate the bindings from this
directory with `flutter_rust_bridge_codegen generate` (version 2.13.0).
