// Replays the GPU commands the Rust side records for a frame
// (`crates/sparkle_flutter/src/gpu.rs`) with Flutter GPU.

import 'dart:convert';
import 'dart:ffi';
import 'dart:math';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';
import 'package:flutter_gpu/gpu.dart' as gpu;
import 'package:vector_math/vector_math.dart' as vm;

/// Opcodes, as `Op` in `gpu.rs`.
abstract final class _Op {
  static const defineProgram = 1;
  static const defineLayout = 2;
  static const createBuffer = 3;
  static const freeBuffer = 4;
  static const createTexture = 5;
  static const writeTexture = 6;
  static const freeTexture = 7;
  static const beginPass = 8;
  static const endPass = 9;
  static const setPipeline = 10;
  static const setState = 11;
  static const setStencilReference = 12;
  static const setVertexBuffer = 13;
  static const setIndexBuffer = 14;
  static const setUniforms = 15;
  static const setTexture = 16;
  static const draw = 17;
  static const drawIndexed = 18;
  static const setUniformBlock = 19;
}

const _experiment = String.fromEnvironment('SPARKLE_EXPERIMENT'); // PERF-TEMP
const _skip = String.fromEnvironment('SPARKLE_SKIP'); // PERF-TEMP

/// Stands for "no resource" (or frame data) in operands.
const int _none = 0xffffffff;

/// A program's shaders, and the uniform slots of each that its draws bind. Flutter GPU
/// doesn't list a shader's slots, so they're found by binding, once per program.
class _Program {
  _Program(this.vertex, this.fragment, [this.name = '']);

  final String name; // PERF-TEMP

  final gpu.Shader vertex;
  final gpu.Shader fragment;

  /// The `Draw` block of each stage that has one, once found.
  List<gpu.UniformSlot>? draw;

  /// Per sampler name, the slots of the stages that have it.
  final Map<String, List<gpu.UniformSlot>> samplers = {};

  /// Per uniform block name other than `Draw`, the slots of the stages that have it.
  final Map<String, List<gpu.UniformSlot>> blocks = {};

  /// Per vertex layout.
  final Map<int, gpu.RenderPipeline> pipelines = {};
}

/// A buffer or texture, with the native memory behind it made known to Dart's garbage
/// collector.
///
/// Flutter GPU frees a resource's memory (on GLES, a copy of its contents in the native
/// heap as well as the GL object) only once Dart collects its object, and doesn't tell
/// the collector what that object holds: it looks a few bytes big. Resources the map
/// dropped then pile up uncollected, hundreds of megabytes of them after some panning,
/// for as long as Dart sees no reason to collect. Attaching their size as `externalSize`
/// lets it collect them as it would any memory it allocates itself.
final class _Resource<T extends Object> implements Finalizable {
  _Resource(this.value, int bytes) {
    // There's nothing to free besides `value`: freeing null does nothing.
    _finalizer.attach(this, nullptr, externalSize: max(bytes, 1));
  }

  static final _finalizer = NativeFinalizer(malloc.nativeFree);

  final T value;
}

/// One of the buffers frames write their uniform blocks to in turn.
class _UniformBuffer {
  gpu.DeviceBuffer? buffer;
  int capacity = 0;
}

/// What a draw binds, kept so that every draw can bind all of it: whether Flutter GPU
/// keeps bindings from one draw to the next isn't something to rely on.
///
/// The pipeline and its fixed-function state are the exception: they're the pass's, not
/// cleared with the bindings, and setting any of that state makes Flutter GPU look the
/// pipeline up again at the next draw. So they're only set when a command changed them.
class _Bindings {
  gpu.RenderPipeline? pipeline;
  _Program? program;
  List<int> state = const [];
  int stencilReference = 0;
  final Map<int, gpu.BufferView> vertexBuffers = {};

  /// Vertex buffer slots the bound pipeline's layout has; slots past it are left over
  /// from earlier draws, and binding them fails on GLES.
  int slotCount = 0;
  (gpu.BufferView, gpu.IndexType)? indexBuffer;

  /// The `Draw` block's offset and length in the frame's uniforms.
  (int, int)? uniforms;
  final Map<String, (gpu.Texture, gpu.SamplerOptions)> textures = {};

  /// Uniform blocks other than `Draw`, by name.
  final Map<String, gpu.BufferView> blocks = {};

  /// Whether the pipeline, state or stencil reference changed since the last draw.
  bool pipelineChanged = false;
  bool stateChanged = false;
  bool stencilReferenceChanged = false;

  void reset() {
    pipeline = null;
    program = null;
    state = const [];
    stencilReference = 0;
    vertexBuffers.clear();
    slotCount = 0;
    indexBuffer = null;
    uniforms = null;
    textures.clear();
    blocks.clear();
    pipelineChanged = false;
    stateChanged = false;
    stencilReferenceChanged = false;
  }
}

/// The GPU resources of one map, and the replaying of its frames.
class GpuExecutor {
  GpuExecutor(this.context, this.library, this.screenFormat);

  final gpu.GpuContext context;
  final gpu.ShaderLibrary library;

  /// The format of the frames' color targets.
  final gpu.PixelFormat screenFormat;

  final Map<int, _Resource<gpu.DeviceBuffer>> _buffers = {};
  final Map<int, _Resource<gpu.Texture>> _textures = {};
  final Map<int, _Program> _programs = {};
  final Map<int, gpu.VertexLayout> _layouts = {};
  late final gpu.HostBuffer _host = context.createHostBuffer();

  /// Taken in turn by frames, as Flutter GPU's host buffers are: by the time a frame
  /// takes one again, the GPU is done with the frame that used it before.
  final List<_UniformBuffer> _uniformBuffers = List.generate(
    4,
    (_) => _UniformBuffer(),
  );
  int _frame = 0;
  late _UniformBuffer _uniformBuffer;

  late Uint32List _words;
  late ByteData _wordBytes;
  late Uint8List _uploads;
  late gpu.BufferView _frameData;
  int _at = 0;

  int draws = 0; // PERF-TEMP
  int lineIndices = 0; // PERF-TEMP
  bool _lineScissor = false; // PERF-TEMP

  int _next() => _words[_at++];

  double _nextFloat() => _wordBytes.getFloat32((_at++) * 4, Endian.little);

  String _nextString() {
    final length = _next();
    final bytes = _words.buffer.asUint8List(
      _words.offsetInBytes + _at * 4,
      length,
    );
    _at += (length + 3) ~/ 4;
    return ascii.decode(bytes);
  }

  /// Contents of a new buffer or texture.
  ByteData _bytes(int offset, int length) =>
      ByteData.sublistView(_uploads, offset, offset + length);

  gpu.BufferView _view(int buffer, int offset, int length) {
    if (buffer == _none) {
      return gpu.BufferView(
        _frameData.buffer,
        offsetInBytes: _frameData.offsetInBytes + offset,
        lengthInBytes: length,
      );
    }
    return gpu.BufferView(
      _buffers[buffer]!.value,
      offsetInBytes: offset,
      lengthInBytes: length,
    );
  }

  gpu.Texture _texture(int id, gpu.Texture screen) =>
      id == 0 ? screen : _textures[id]!.value;

  gpu.PixelFormat _format(int format) => switch (format) {
    0 => screenFormat,
    1 => gpu.PixelFormat.r8g8b8a8UNormInt,
    2 => gpu.PixelFormat.r8UNormInt,
    3 => gpu.PixelFormat.r16g16b16a16Float,
    _ => context.defaultDepthStencilFormat,
  };

  static int _bytesPerPixel(gpu.PixelFormat format) => switch (format) {
    gpu.PixelFormat.r8UNormInt => 1,
    gpu.PixelFormat.r16g16b16a16Float => 8,
    _ => 4,
  };

  static gpu.LoadAction _load(int load) => switch (load) {
    0 => gpu.LoadAction.clear,
    1 => gpu.LoadAction.load,
    _ => gpu.LoadAction.dontCare,
  };

  static gpu.StoreAction _store(int store) => switch (store) {
    0 => gpu.StoreAction.store,
    1 => gpu.StoreAction.dontCare,
    2 => gpu.StoreAction.multisampleResolve,
    _ => gpu.StoreAction.storeAndMultisampleResolve,
  };

  /// Replay a frame's `words` into `commandBuffer`, drawing into `screen` as texture 0.
  /// `uniforms` are its draws' uniform blocks, `data` the vertices they bind, and
  /// `uploads` the contents of the buffers and textures it creates; none of them are
  /// kept.
  ///
  /// Each render pass after the first goes in a command buffer of its own, the one
  /// before it submitted first, and the last is returned for the caller to present and
  /// submit. On Metal a Flutter GPU render pass opens its encoder when it's created and
  /// closes it only at submit, and a command buffer can't have two open at once: Metal
  /// validation fails with "encoding in progress", and the simulator crashes. A queue
  /// runs command buffers in the order they're submitted, so passes keep their order.
  gpu.CommandBuffer execute(
    Uint32List words,
    Uint8List uniforms,
    Uint8List data,
    Uint8List uploads,
    gpu.Texture screen,
    gpu.CommandBuffer commandBuffer,
  ) {
    var passes = 0;
    _words = words;
    _wordBytes = ByteData.sublistView(words);
    _uploads = uploads;
    _at = 0;
    _host.reset();
    _frameData = _uploadFrameData(data);
    _uniformBuffer = _writeUniforms(uniforms);
    gpu.RenderPass? pass;
    final bindings = _Bindings();
    while (_at < words.length) {
      final op = _next();
      switch (op) {
        case _Op.defineProgram:
          final id = _next();
          final vertex = _nextString();
          final fragment = _nextString();
          final vs = library[vertex];
          final fs = library[fragment];
          if (vs == null || fs == null) {
            throw StateError('the shader bundle has no $vertex or $fragment');
          }
          _programs[id] = _Program(vs, fs, vertex);
        case _Op.defineLayout:
          final id = _next();
          final count = _next();
          final buffers = <gpu.VertexBuffer>[];
          for (var b = 0; b < count; b++) {
            final stride = _next();
            final step = _next();
            final attributeCount = _next();
            final attributes = <gpu.VertexAttribute>[];
            for (var a = 0; a < attributeCount; a++) {
              final format = gpu.VertexFormat.values[_next()];
              final offset = _next();
              attributes.add(
                gpu.VertexAttribute(
                  name: _nextString(),
                  format: format,
                  offsetInBytes: offset,
                ),
              );
            }
            buffers.add(
              gpu.VertexBuffer(
                strideInBytes: stride,
                attributes: attributes,
                stepMode: step == 1
                    ? gpu.VertexStepMode.instance
                    : gpu.VertexStepMode.vertex,
              ),
            );
          }
          _layouts[id] = gpu.VertexLayout(buffers: buffers);
        case _Op.createBuffer:
          final id = _next();
          final offset = _next();
          final length = _next();
          // Flutter GPU has no empty buffers; nothing draws from these.
          _buffers[id] = _Resource(
            context.createDeviceBufferWithCopy(
              length == 0 ? ByteData(4) : _bytes(offset, length),
            ),
            length,
          );
        case _Op.freeBuffer:
          _buffers.remove(_next());
        case _Op.createTexture:
          final id = _next();
          final width = _next();
          final height = _next();
          final format = _format(_next());
          final samples = _next();
          final flags = _next();
          final offset = _next();
          final length = _next();
          final texture = context.createTexture(
            length > 0
                ? gpu.StorageMode.hostVisible
                : gpu.StorageMode.devicePrivate,
            width,
            height,
            format: format,
            sampleCount: samples,
            enableRenderTargetUsage: flags & 1 != 0,
            enableShaderReadUsage: flags & 2 != 0,
          );
          if (length > 0) {
            texture.overwrite(_bytes(offset, length));
          }
          _textures[id] = _Resource(
            texture,
            width * height * samples * _bytesPerPixel(format),
          );
        case _Op.writeTexture:
          final id = _next();
          final offset = _next();
          final length = _next();
          _textures[id]!.value.overwrite(_bytes(offset, length));
        case _Op.freeTexture:
          _textures.remove(_next());
        case _Op.beginPass:
          if (passes++ > 0) {
            commandBuffer.submit();
            commandBuffer = context.createCommandBuffer();
          }
          pass = _beginPass(commandBuffer, screen);
          bindings.reset();
          _lineScissor = false; // PERF-TEMP
          if (_experiment == 'scissor') {
            // PERF-TEMP
            pass.setScissor(gpu.Scissor(x: 0, y: 0, width: 8, height: 8));
          }
        case _Op.endPass:
          pass = null;
        case _Op.setPipeline:
          final program = _next();
          final layout = _next();
          final p = _programs[program]!;
          bindings.program = p;
          bindings.pipelineChanged = true;
          bindings.slotCount = _layouts[layout]!.buffers.length;
          bindings.pipeline = p.pipelines.putIfAbsent(
            layout,
            () => context.createRenderPipeline(
              p.vertex,
              p.fragment,
              vertexLayout: _layouts[layout],
            ),
          );
        case _Op.setState:
          bindings.state = List<int>.generate(8, (_) => _next());
          bindings.stateChanged = true;
        case _Op.setStencilReference:
          final reference = _next();
          // A pass starts with reference 0.
          bindings.stencilReferenceChanged |=
              reference != bindings.stencilReference;
          bindings.stencilReference = reference;
        case _Op.setVertexBuffer:
          final slot = _next();
          final buffer = _next();
          final offset = _next();
          final length = _next();
          bindings.vertexBuffers[slot] = _view(buffer, offset, length);
        case _Op.setIndexBuffer:
          final buffer = _next();
          final offset = _next();
          final length = _next();
          final type = _next() == 1 ? gpu.IndexType.int32 : gpu.IndexType.int16;
          bindings.indexBuffer = (_view(buffer, offset, length), type);
        case _Op.setUniforms:
          final offset = _next();
          final length = _next();
          bindings.uniforms = (offset, length);
        case _Op.setUniformBlock:
          final name = _nextString();
          final buffer = _next();
          final offset = _next();
          final length = _next();
          bindings.blocks[name] = _view(buffer, offset, length);
        case _Op.setTexture:
          final name = _nextString();
          final texture = _texture(_next(), screen);
          final linear = _next() == 1;
          final repeatU = _next() == 1;
          final repeatV = _next() == 1;
          final filter = linear
              ? gpu.MinMagFilter.linear
              : gpu.MinMagFilter.nearest;
          bindings.textures[name] = (
            texture,
            gpu.SamplerOptions(
              minFilter: filter,
              magFilter: filter,
              widthAddressMode: repeatU
                  ? gpu.SamplerAddressMode.repeat
                  : gpu.SamplerAddressMode.clampToEdge,
              heightAddressMode: repeatV
                  ? gpu.SamplerAddressMode.repeat
                  : gpu.SamplerAddressMode.clampToEdge,
            ),
          );
        case _Op.draw:
          final count = _next();
          final instances = _next();
          _bind(pass!, bindings);
          draws++; // PERF-TEMP
          if (_experiment == 'nodraw' ||
              (_skip.isNotEmpty && bindings.program!.name.startsWith(_skip)))
            continue; // PERF-TEMP
          pass.draw(count, instanceCount: instances);
        case _Op.drawIndexed:
          final count = _next();
          final instances = _next();
          _bind(pass!, bindings);
          draws++; // PERF-TEMP
          if (_experiment == 'nodraw' ||
              (_skip.isNotEmpty && bindings.program!.name.startsWith(_skip)))
            continue; // PERF-TEMP
          final isLine = bindings.program!.name.startsWith('Line'); // PERF-TEMP
          if (isLine) lineIndices += count * instances; // PERF-TEMP
          if (_experiment == 'linescissor' && isLine != _lineScissor) {
            // PERF-TEMP
            _lineScissor = isLine;
            pass.setScissor(
              isLine
                  ? gpu.Scissor(x: 0, y: 0, width: 8, height: 8)
                  : gpu.Scissor(
                      x: 0,
                      y: 0,
                      width: screen.width,
                      height: screen.height,
                    ),
            );
          }
          if (_experiment == 'split' && count >= 6) {
            // PERF-TEMP
            // The same triangles in two draws, each with its own bindings.
            final half = count ~/ 6 * 3;
            final (view, type) = bindings.indexBuffer!;
            final size = type == gpu.IndexType.int32 ? 4 : 2;
            pass.bindIndexBuffer(
              gpu.BufferView(
                view.buffer,
                offsetInBytes: view.offsetInBytes,
                lengthInBytes: half * size,
              ),
              type,
            );
            pass.drawIndexed(half, instanceCount: instances);
            _bind(pass, bindings);
            pass.bindIndexBuffer(
              gpu.BufferView(
                view.buffer,
                offsetInBytes: view.offsetInBytes + half * size,
                lengthInBytes: view.lengthInBytes - half * size,
              ),
              type,
            );
            pass.drawIndexed(count - half, instanceCount: instances);
            continue;
          }
          pass.drawIndexed(count, instanceCount: instances);
        default:
          throw StateError('unknown GPU command $op at word ${_at - 1}');
      }
    }
    return commandBuffer;
  }

  /// Write the frame's uniform blocks to the next of [_uniformBuffers], growing it if
  /// they don't fit (which drops its binding sets).
  _UniformBuffer _writeUniforms(Uint8List uniforms) {
    final u = _uniformBuffers[_frame++ % _uniformBuffers.length];
    if (uniforms.isEmpty) {
      return u;
    }
    if (u.capacity < uniforms.length) {
      var capacity = max(u.capacity, 64 * 1024);
      while (capacity < uniforms.length) {
        capacity *= 2;
      }
      u.buffer = context.createDeviceBuffer(
        gpu.StorageMode.hostVisible,
        capacity,
      );
      u.capacity = capacity;
    }
    u.buffer!.overwrite(ByteData.sublistView(uniforms));
    u.buffer!.flush(lengthInBytes: uniforms.length);
    return u;
  }

  /// The frame's data in one buffer, at the offsets the Rust side aligned it to.
  gpu.BufferView _uploadFrameData(Uint8List data) {
    final bytes = data.isEmpty ? ByteData(16) : ByteData.sublistView(data);
    final view = _host.emplace(bytes);
    if (view.offsetInBytes % context.minimumUniformByteAlignment == 0) {
      return view;
    }
    final buffer = context.createDeviceBufferWithCopy(bytes);
    return gpu.BufferView(
      buffer,
      offsetInBytes: 0,
      lengthInBytes: bytes.lengthInBytes,
    );
  }

  gpu.RenderPass _beginPass(
    gpu.CommandBuffer commandBuffer,
    gpu.Texture screen,
  ) {
    final color = _next();
    final resolve = _next();
    final colorLoad = _next();
    final colorStore = _next();
    final clear = vm.Vector4(
      _nextFloat(),
      _nextFloat(),
      _nextFloat(),
      _nextFloat(),
    );
    final depthStencil = _next();
    final depthLoad = _next();
    final depthStore = _next();
    final clearDepth = _nextFloat();
    final stencilLoad = _next();
    final stencilStore = _next();
    final clearStencil = _next();
    return commandBuffer.createRenderPass(
      gpu.RenderTarget.singleColor(
        gpu.ColorAttachment(
          texture: _texture(color, screen),
          resolveTexture: resolve == _none ? null : _texture(resolve, screen),
          loadAction: _load(colorLoad),
          storeAction: _store(colorStore),
          clearValue: clear,
        ),
        depthStencilAttachment: depthStencil == _none
            ? null
            : gpu.DepthStencilAttachment(
                texture: _texture(depthStencil, screen),
                depthLoadAction: _load(depthLoad),
                depthStoreAction: _store(depthStore),
                depthClearValue: clearDepth,
                stencilLoadAction: _load(stencilLoad),
                stencilStoreAction: _store(stencilStore),
                stencilClearValue: clearStencil,
              ),
      ),
    );
  }

  /// The slots, among `shaders`' uniforms called `name`, that `bind` binds without
  /// failing: those of the shaders that have such a uniform.
  static List<gpu.UniformSlot> _find(
    List<gpu.Shader> shaders,
    String name,
    void Function(gpu.UniformSlot) bind,
  ) {
    final found = <gpu.UniformSlot>[];
    for (final shader in shaders) {
      final slot = shader.getUniformSlot(name);
      try {
        bind(slot);
        found.add(slot);
      } on Exception {
        // The shader has no such uniform.
      }
    }
    return found;
  }

  void _bind(gpu.RenderPass pass, _Bindings b) {
    final p = b.program!;
    // Flutter GPU keeps uniforms bound per shader until cleared, and binds them all with
    // the next draw: on GLES every program's `Draw` block lands on binding point 0, so
    // an earlier program's would shadow this one's.
    pass.clearBindings();
    if (b.pipelineChanged) {
      pass.bindPipeline(b.pipeline!);
      b.pipelineChanged = false;
    }
    if (b.stateChanged) {
      _applyState(pass, b.state);
      b.stateChanged = false;
    }
    if (b.stencilReferenceChanged) {
      pass.setStencilReference(b.stencilReference);
      b.stencilReferenceChanged = false;
    }
    for (var slot = 0; slot < b.slotCount; slot++) {
      if (b.vertexBuffers[slot] case final view?) {
        pass.bindVertexBuffer(view, slot: slot);
      }
    }
    if (b.indexBuffer case (final view, final type)) {
      pass.bindIndexBuffer(view, type);
    }
    if (b.uniforms case (final offset, final length)) {
      final slots = p.draw ??= [
        for (final shader in [p.vertex, p.fragment])
          if (shader.getUniformSlot('Draw').sizeInBytes != null)
            shader.getUniformSlot('Draw'),
      ];
      final view = gpu.BufferView(
        _uniformBuffer.buffer!,
        offsetInBytes: offset,
        lengthInBytes: length,
      );
      for (final slot in slots) {
        pass.bindUniform(slot, view);
      }
    }
    for (final MapEntry(key: name, value: view) in b.blocks.entries) {
      final slots = p.blocks[name] ??= [
        for (final shader in [p.vertex, p.fragment])
          if (shader.getUniformSlot(name).sizeInBytes != null)
            shader.getUniformSlot(name),
      ];
      for (final slot in slots) {
        pass.bindUniform(slot, view);
      }
    }
    for (final MapEntry(key: name, value: (texture, sampler))
        in b.textures.entries) {
      if (p.samplers[name] case final slots?) {
        for (final slot in slots) {
          pass.bindTexture(slot, texture, sampler: sampler);
        }
      } else {
        p.samplers[name] = _find(
          [p.vertex, p.fragment],
          name,
          (slot) => pass.bindTexture(slot, texture, sampler: sampler),
        );
      }
    }
  }

  static const _compare = gpu.CompareFunction.values;

  static final _premultiplied = gpu.ColorBlendEquation(
    sourceColorBlendFactor: gpu.BlendFactor.one,
    destinationColorBlendFactor: gpu.BlendFactor.oneMinusSourceAlpha,
    sourceAlphaBlendFactor: gpu.BlendFactor.one,
    destinationAlphaBlendFactor: gpu.BlendFactor.oneMinusSourceAlpha,
  );
  static final _add = gpu.ColorBlendEquation(
    sourceColorBlendFactor: gpu.BlendFactor.one,
    destinationColorBlendFactor: gpu.BlendFactor.one,
    sourceAlphaBlendFactor: gpu.BlendFactor.one,
    destinationAlphaBlendFactor: gpu.BlendFactor.one,
  );
  static final _noColor = gpu.ColorBlendEquation(
    sourceColorBlendFactor: gpu.BlendFactor.zero,
    destinationColorBlendFactor: gpu.BlendFactor.one,
    sourceAlphaBlendFactor: gpu.BlendFactor.zero,
    destinationAlphaBlendFactor: gpu.BlendFactor.one,
  );

  void _applyState(gpu.RenderPass pass, List<int> state) {
    if (state.isEmpty) {
      return;
    }
    final [
      blend,
      depthCompare,
      depthWrite,
      stencilCompare,
      stencilPass,
      read,
      write,
      primitive,
    ] = state;
    switch (blend) {
      case 0:
        pass.setColorBlendEnable(false);
      case 1:
        pass.setColorBlendEnable(true);
        pass.setColorBlendEquation(_premultiplied);
      case 2:
        pass.setColorBlendEnable(true);
        pass.setColorBlendEquation(_add);
      default:
        // Leave the color as it is.
        pass.setColorBlendEnable(true);
        pass.setColorBlendEquation(_noColor);
    }
    pass.setDepthCompareOperation(_compare[depthCompare]);
    pass.setDepthWriteEnable(depthWrite == 1);
    pass.setStencilConfig(
      gpu.StencilConfig(
        compareFunction: _compare[stencilCompare],
        depthStencilPassOperation: switch (stencilPass) {
          1 => gpu.StencilOperation.zero,
          2 => gpu.StencilOperation.setToReferenceValue,
          _ => gpu.StencilOperation.keep,
        },
        readMask: read,
        writeMask: write,
      ),
    );
    pass.setPrimitiveType(switch (primitive) {
      1 => gpu.PrimitiveType.triangleStrip,
      2 => gpu.PrimitiveType.line,
      _ => gpu.PrimitiveType.triangle,
    });
  }
}
