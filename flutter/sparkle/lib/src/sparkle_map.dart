// A map widget: the Rust side decides what to draw (`crates/sparkle_flutter`), and the
// executor draws it with Flutter GPU into an image surface that this shows.

import 'dart:async';
import 'dart:ffi';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_gpu/gpu.dart' as gpu;

import 'executor.dart';
import 'rust/api/map.dart' as rust;
import 'rust/frb_generated.dart';

/// Where the build hook puts the shaders (see `hook/build.dart`).
const String _shaderBundle =
    'packages/sparkle/build/shaderbundles/sparkle.shaderbundle';

/// What the map is showing, after each frame.
class MapStatus {
  const MapStatus({
    required this.zoom,
    required this.bearing,
    required this.pitch,
    required this.tilesDrawn,
    required this.loading,
  });

  final double zoom;
  final double bearing;
  final double pitch;
  final int tilesDrawn;
  final int loading;
}

/// What two fingers on the map are doing.
enum _TwoFingers {
  /// They haven't moved far enough to tell.
  undecided,

  /// Panning, zooming and rotating.
  pinch,

  /// Dragging up or down side by side, to tilt.
  tilt,
}

/// Two fingers must turn this many degrees before a pinch starts rotating, so that
/// zooming doesn't also turn the map a little.
const double _rotateThreshold = 20;

/// The finger held down on the second tap of a double tap.
class _QuickZoom {
  _QuickZoom(this.pointer, this.anchor);

  final int pointer;

  /// Where the second tap went down: zooming is around it.
  final Offset anchor;

  /// Whether it has dragged, rather than lifting to zoom in a level.
  bool dragging = false;
}

/// Loads the native library; [SparkleMap] waits for it, so calling this is optional.
Future<void> initSparkle() => _init ??= RustLib.init();
Future<void>? _init;

/// A map of a tile server's or local tiles, set up from command line arguments as
/// `sparkle_harness` takes them (see `crates/sparkle_view/src/args.rs`).
///
/// Needs Flutter GPU, which is off by default: see the example app for how each platform
/// turns it on.
class SparkleMap extends StatefulWidget {
  const SparkleMap({
    super.key,
    required this.args,
    this.multisample = true,
    this.onStatus,
    this.onScreenshot,
    this.onError,
  });

  final List<String> args;

  /// Whether to multisample when the device can.
  final bool multisample;

  /// Called after every frame.
  final void Function(MapStatus status)? onStatus;

  /// Called once `--screenshot` is saved, with its path.
  final void Function(String path)? onScreenshot;

  /// Called when the map can't be set up or drawn.
  final void Function(Object error)? onError;

  @override
  State<SparkleMap> createState() => _SparkleMapState();
}

class _SparkleMapState extends State<SparkleMap> {
  final GlobalKey _boundary = GlobalKey();
  final FocusNode _focus = FocusNode(debugLabel: 'SparkleMap');

  gpu.ShaderLibrary? _library;
  rust.SparkleMap? _map;
  GpuExecutor? _executor;
  gpu.GpuImageSurface? _surface;
  ui.Image? _image;
  Object? _error;

  /// The view's size in physical pixels, and the pixel ratio.
  (int, int, double)? _size;
  bool _frameScheduled = false;
  StreamSubscription<int>? _wake;
  Timer? _stylePoll;
  String? _screenshot;
  bool _hovered = false;

  /// The primary pointer and its button.
  int? _pointer;
  int _button = 0;

  /// Fingers down, where they were last; the first two are the ones a two finger gesture
  /// follows.
  final Map<int, Offset> _touches = {};
  _TwoFingers? _twoFingers;

  /// How far each of the two fingers moved while [_TwoFingers.undecided].
  final Map<int, Offset> _travel = {};

  /// How far a pinch has turned, in degrees, until it passes [_rotateThreshold].
  double? _turned;

  /// When and where a lone finger went down, while it might be a tap.
  (Duration, Offset)? _tapDown;

  /// When and where the last tap lifted, while a second could make it a double tap.
  (Duration, Offset)? _lastTap;

  /// Clicks where the last tap lifted once it can no longer be a double tap.
  Timer? _tapClick;
  _QuickZoom? _quickZoom;

  double _lastPanZoomScale = 1;
  double _lastPanZoomRotation = 0;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      await initSparkle();
      final library = await gpu.ShaderLibrary.fromAsset(_shaderBundle);
      if (library == null) {
        throw StateError('no shader bundle at $_shaderBundle');
      }
      if (!mounted) {
        return;
      }
      setState(() => _library = library);
    } on Object catch (e) {
      _fail(e);
    }
  }

  void _fail(Object error) {
    widget.onError?.call(error);
    if (mounted) {
      setState(() => _error = error);
    }
  }

  @override
  void dispose() {
    _wake?.cancel();
    _stylePoll?.cancel();
    _tapClick?.cancel();
    _focus.dispose();
    _map?.dispose();
    super.dispose();
  }

  /// Make the map (once the size is known) or tell it the size changed.
  void _layout(int width, int height, double pixelRatio) {
    if (_size == (width, height, pixelRatio) ||
        _error != null ||
        _library == null) {
      return;
    }
    _size = (width, height, pixelRatio);
    final context = gpu.gpuContext;
    _surface = context.createImageSurface(width, height);
    final map = _map;
    if (map == null) {
      try {
        final created = rust.SparkleMap(
          args: widget.args,
          device: rust.DeviceInfo(
            uniformAlignment: context.minimumUniformByteAlignment,
            sampleCount: widget.multisample && context.doesSupportOffscreenMSAA
                ? 4
                : 1,
            halfFloat: context.supportsTextureFormat(
              gpu.PixelFormat.r16g16b16a16Float,
              renderTarget: true,
            ),
          ),
          width: width,
          height: height,
          pixelRatio: pixelRatio,
        );
        _map = created;
        _executor = GpuExecutor(context, _library!, _surface!.format);
        _screenshot = created.screenshotPath();
        _wake = created.wakeStream().listen((_) => _requestFrame());
        _stylePoll = Timer.periodic(const Duration(milliseconds: 500), (_) {
          if (_map?.pollStyle() ?? false) {
            _requestFrame();
          }
        });
      } on Object catch (e) {
        // Not during build: report it after.
        scheduleMicrotask(() => _fail(e));
        return;
      }
    } else {
      map.resize(width: width, height: height, pixelRatio: pixelRatio);
    }
    _requestFrame();
  }

  void _requestFrame() {
    if (_frameScheduled || !mounted) {
      return;
    }
    _frameScheduled = true;
    SchedulerBinding.instance.scheduleFrameCallback((_) => _drawFrame());
    SchedulerBinding.instance.scheduleFrame();
  }

  void _drawFrame() {
    _frameScheduled = false;
    final map = _map;
    final surface = _surface;
    final executor = _executor;
    if (map == null || surface == null || executor == null || !mounted) {
      return;
    }
    final rust.FrameOutput out;
    try {
      final sw = Stopwatch()..start(); // PERF-TEMP
      final gap = _sinceFrame.elapsedMicroseconds; // PERF-TEMP
      _sinceFrame.reset(); // PERF-TEMP
      out = map.frame();
      final tRust = sw.elapsedMicroseconds; // PERF-TEMP
      final frame = surface.acquireNextFrame();
      final commandBuffer = gpu.gpuContext.createCommandBuffer();
      executor.execute(
        _words(out.commands),
        _bytes(out.uniforms),
        _bytes(out.data),
        _bytes(out.uploads),
        frame.colorTexture,
        commandBuffer,
      );
      final tExec = sw.elapsedMicroseconds - tRust; // PERF-TEMP
      frame.present(commandBuffer);
      commandBuffer.submit();
      if (tRust + tExec > 9000) { // PERF-TEMP
        debugPrint('PERF slow after ${gap ~/ 1000} ms idle: rust ${tRust ~/ 100 / 10} exec ${tExec ~/ 100 / 10} ms, '
            'uploads ${out.uploads.length ~/ 1024} KB, ${executor.draws} draws, ${out.commands.length} words');
      }
      _perf.add(tRust, tExec, sw.elapsedMicroseconds - tRust - tExec,
          out.commands.length, executor.draws); // PERF-TEMP
      executor.draws = 0; // PERF-TEMP
      _perf.lineIndices += executor.lineIndices; // PERF-TEMP
      executor.lineIndices = 0; // PERF-TEMP
    } on Object catch (e) {
      _fail(e);
      return;
    }
    setState(() => _image = surface.currentImage);
    widget.onStatus?.call(
      MapStatus(
        zoom: out.zoom,
        bearing: out.bearing,
        pitch: out.pitch,
        tilesDrawn: out.tilesDrawn,
        loading: out.loading,
      ),
    );
    if (out.complete && !_hovered) {
      _hovered = true;
      if (map.hoverArgument()) {
        _requestFrame();
        return;
      }
    }
    if (out.animating || const String.fromEnvironment('SPARKLE_EXPERIMENT') == 'spin') { // PERF-TEMP
      _requestFrame();
    } else if (out.complete && _screenshot != null) {
      final path = _screenshot!;
      _screenshot = null;
      SchedulerBinding.instance.addPostFrameCallback(
        (_) => _saveScreenshot(path),
      );
    }
  }

  /// Arrays in Rust's memory, read in place: valid until the next frame.
  static Uint32List _words(rust.Address a) => a.length == 0
      ? Uint32List(0)
      : Pointer<Uint32>.fromAddress(a.address).asTypedList(a.length);

  static Uint8List _bytes(rust.Address a) => a.length == 0
      ? Uint8List(0)
      : Pointer<Uint8>.fromAddress(a.address).asTypedList(a.length);

  Future<void> _saveScreenshot(String path) async {
    try {
      final boundary =
          _boundary.currentContext!.findRenderObject()!
              as RenderRepaintBoundary;
      final image = await boundary.toImage(pixelRatio: _size!.$3);
      final png = await image.toByteData(format: ui.ImageByteFormat.png);
      await File(path).writeAsBytes(png!.buffer.asUint8List());
      widget.onScreenshot?.call(path);
    } on Object catch (e) {
      _fail(e);
    }
  }

  // Input. Positions are logical pixels within the map.

  void _redrawIf(bool changed) {
    if (changed) {
      _requestFrame();
    }
  }

  void _onPointerDown(PointerDownEvent e) {
    _focus.requestFocus();
    final map = _map;
    if (map == null) {
      return;
    }
    if (e.kind == PointerDeviceKind.touch) {
      _touches[e.pointer] = e.localPosition;
      if (_touches.length == 1) {
        final last = _lastTap;
        _lastTap = null;
        if (last != null &&
            e.timeStamp - last.$1 < kDoubleTapTimeout &&
            (e.localPosition - last.$2).distance < kDoubleTapSlop) {
          // The second tap: zoom when it lifts, or as it drags. The first doesn't click.
          _tapClick?.cancel();
          _tapClick = null;
          _quickZoom = _QuickZoom(e.pointer, e.localPosition);
          return;
        }
        if (last != null) {
          _clickTapNow(last.$2);
        }
        _tapDown = (e.timeStamp, e.localPosition);
      } else {
        if (_touches.length == 2) {
          // Two fingers: stop dragging with the first.
          final first = _pointer;
          if (first != null) {
            final p = _touches[first]!;
            map.pointerUp(x: p.dx, y: p.dy, button: 2, click: false);
            _pointer = null;
          }
          _quickZoom = null;
          _tapDown = null;
          _twoFingers = _TwoFingers.undecided;
          _travel.clear();
          _turned = 0;
        }
        return;
      }
    }
    if (_pointer != null) {
      return;
    }
    _pointer = e.pointer;
    _button = e.buttons & kSecondaryMouseButton != 0
        ? 1
        : e.buttons & kPrimaryButton != 0
        ? 0
        : 2;
    _redrawIf(
      map.pointerDown(
        x: e.localPosition.dx,
        y: e.localPosition.dy,
        button: _button,
        rotate: HardwareKeyboard.instance.isControlPressed,
      ),
    );
  }

  /// The two fingers a two finger gesture follows.
  (Offset, Offset) _pair() {
    final [a, b] = _touches.values.take(2).toList();
    return (a, b);
  }

  void _onPointerMove(PointerMoveEvent e) {
    final map = _map;
    if (map == null) {
      return;
    }
    if (_quickZoom case final quick? when quick.pointer == e.pointer) {
      _touches[e.pointer] = e.localPosition;
      if (!quick.dragging &&
          (e.localPosition - quick.anchor).distance < kTouchSlop) {
        return;
      }
      quick.dragging = true;
      map.quickZoom(
        x: quick.anchor.dx,
        y: quick.anchor.dy,
        dy: e.localDelta.dy,
      );
      _requestFrame();
      return;
    }
    if (_touches.containsKey(e.pointer) && _touches.length >= 2) {
      if (_touches.keys.take(2).contains(e.pointer)) {
        final (a0, b0) = _pair();
        _touches[e.pointer] = e.localPosition;
        final (a1, b1) = _pair();
        _twoFingerMove(map, e.pointer, e.localDelta, (a0, b0), (a1, b1));
      } else {
        _touches[e.pointer] = e.localPosition;
      }
      return;
    }
    if (_touches.containsKey(e.pointer)) {
      _touches[e.pointer] = e.localPosition;
    }
    if (e.pointer == _pointer) {
      _redrawIf(map.pointerMove(x: e.localPosition.dx, y: e.localPosition.dy));
    }
  }

  /// One of two fingers moved by [delta], from [before] to [after].
  void _twoFingerMove(
    rust.SparkleMap map,
    int pointer,
    Offset delta,
    (Offset, Offset) before,
    (Offset, Offset) after,
  ) {
    final (a0, b0) = before;
    final (a1, b1) = after;
    if (_twoFingers == _TwoFingers.undecided) {
      _travel[pointer] = (_travel[pointer] ?? Offset.zero) + delta;
      final [ta, tb] = _touches.keys
          .take(2)
          .map((p) => _travel[p] ?? Offset.zero)
          .toList();
      if (ta.distance < kTouchSlop && tb.distance < kTouchSlop) {
        return;
      }
      // Tilting is both fingers, side by side, going up or down together.
      bool vertical(Offset t) => t.dy.abs() > t.dx.abs() * 2;
      final apart = a1 - b1;
      final tilt =
          apart.dy.abs() < apart.dx.abs() &&
          vertical(ta) &&
          vertical(tb) &&
          ta.dy.sign == tb.dy.sign;
      _twoFingers = tilt ? _TwoFingers.tilt : _TwoFingers.pinch;
      return;
    }
    final c0 = (a0 + b0) / 2;
    final moved = (a1 + b1) / 2 - c0;
    if (_twoFingers == _TwoFingers.tilt) {
      map.tilt(dy: moved.dy);
      _requestFrame();
      return;
    }
    final d0 = (a0 - b0).distance;
    final scale = d0 > 0 ? (a1 - b1).distance / d0 : 1.0;
    // Clockwise on screen, as y points down.
    var turn = ((b1 - a1).direction - (b0 - a0).direction) * 180 / pi;
    turn = (turn + 180) % 360 - 180;
    if (_turned case final turned?) {
      _turned = turned + turn;
      if (_turned!.abs() < _rotateThreshold) {
        turn = 0;
      } else {
        _turned = null;
      }
    }
    map.panZoom(
      x: c0.dx,
      y: c0.dy,
      dx: moved.dx,
      dy: moved.dy,
      scale: scale,
      rotation: turn,
    );
    _requestFrame();
  }

  void _onPointerUp(PointerEvent e) {
    _touches.remove(e.pointer);
    if (_touches.length < 2) {
      _twoFingers = null;
    }
    if (_quickZoom case final quick? when quick.pointer == e.pointer) {
      _quickZoom = null;
      if (!quick.dragging && e is PointerUpEvent) {
        _map?.doubleTapZoom(x: quick.anchor.dx, y: quick.anchor.dy);
        _requestFrame();
      }
      return;
    }
    var tap = false;
    if (_tapDown case (final time, final position)
        when e is PointerUpEvent && e.kind == PointerDeviceKind.touch) {
      tap =
          e.timeStamp - time < kLongPressTimeout &&
          (e.localPosition - position).distance < kTouchSlop;
    }
    _tapDown = null;
    if (tap) {
      // Click once it's clear this isn't the first of a double tap.
      final position = e.localPosition;
      _lastTap = (e.timeStamp, position);
      _tapClick = Timer(kDoubleTapTimeout, () {
        _tapClick = null;
        _lastTap = null;
        _redrawIf(_map?.click(x: position.dx, y: position.dy) ?? false);
      });
    }
    if (e.pointer != _pointer) {
      return;
    }
    _pointer = null;
    _redrawIf(
      _map?.pointerUp(
            x: e.localPosition.dx,
            y: e.localPosition.dy,
            button: _button,
            click: !tap,
          ) ??
          false,
    );
  }

  /// Click a tap still waiting to see whether it's a double tap.
  void _clickTapNow(Offset position) {
    final timer = _tapClick;
    if (timer == null) {
      return;
    }
    timer.cancel();
    _tapClick = null;
    _redrawIf(_map?.click(x: position.dx, y: position.dy) ?? false);
  }

  void _onPointerHover(PointerHoverEvent e) {
    _redrawIf(
      _map?.pointerMove(x: e.localPosition.dx, y: e.localPosition.dy) ?? false,
    );
  }

  void _onPointerSignal(PointerSignalEvent e) {
    if (e is! PointerScrollEvent) {
      return;
    }
    GestureBinding.instance.pointerSignalResolver.register(e, (event) {
      final scroll = event as PointerScrollEvent;
      _map?.scroll(
        x: scroll.localPosition.dx,
        y: scroll.localPosition.dy,
        delta: scroll.scrollDelta.dy,
      );
      _requestFrame();
    });
  }

  void _onPanZoomStart(PointerPanZoomStartEvent e) {
    _lastPanZoomScale = 1;
    _lastPanZoomRotation = 0;
  }

  void _onPanZoomUpdate(PointerPanZoomUpdateEvent e) {
    final scale = e.scale / _lastPanZoomScale;
    _lastPanZoomScale = e.scale;
    final rotation = e.rotation - _lastPanZoomRotation;
    _lastPanZoomRotation = e.rotation;
    _map?.panZoom(
      x: e.localPosition.dx,
      y: e.localPosition.dy,
      dx: e.localPanDelta.dx,
      dy: e.localPanDelta.dy,
      scale: scale,
      rotation: rotation * 180 / pi,
    );
    _requestFrame();
  }

  static final Map<LogicalKeyboardKey, String> _namedKeys = {
    LogicalKeyboardKey.arrowLeft: 'ArrowLeft',
    LogicalKeyboardKey.arrowRight: 'ArrowRight',
    LogicalKeyboardKey.arrowUp: 'ArrowUp',
    LogicalKeyboardKey.arrowDown: 'ArrowDown',
  };

  KeyEventResult _onKey(FocusNode node, KeyEvent e) {
    final map = _map;
    if (map == null || e is KeyUpEvent) {
      return KeyEventResult.ignored;
    }
    final key = _namedKeys[e.logicalKey] ?? e.character;
    if (key == null) {
      return KeyEventResult.ignored;
    }
    if (!map.key(key: key, shift: HardwareKeyboard.instance.isShiftPressed)) {
      return KeyEventResult.ignored;
    }
    _requestFrame();
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    if (_error case final error?) {
      return Center(child: Text('$error', textDirection: TextDirection.ltr));
    }
    return LayoutBuilder(
      builder: (context, constraints) {
        final ratio = MediaQuery.devicePixelRatioOf(context);
        final width = (constraints.maxWidth * ratio).round();
        final height = (constraints.maxHeight * ratio).round();
        if (width > 0 && height > 0) {
          _layout(width, height, ratio);
        }
        return Focus(
          focusNode: _focus,
          autofocus: true,
          onKeyEvent: _onKey,
          child: Listener(
            onPointerDown: _onPointerDown,
            onPointerMove: _onPointerMove,
            onPointerUp: _onPointerUp,
            onPointerCancel: _onPointerUp,
            onPointerHover: _onPointerHover,
            onPointerSignal: _onPointerSignal,
            onPointerPanZoomStart: _onPanZoomStart,
            onPointerPanZoomUpdate: _onPanZoomUpdate,
            child: RepaintBoundary(
              key: _boundary,
              child: RawImage(
                image: _image,
                width: constraints.maxWidth,
                height: constraints.maxHeight,
                fit: BoxFit.fill,
              ),
            ),
          ),
        );
      },
    );
  }
}

// PERF-TEMP: frame timing averages, logged every 60 map frames.
final _sinceFrame = Stopwatch()..start();
final _perf = _PerfLog();

class _PerfLog {
  _PerfLog() {
    SchedulerBinding.instance.addTimingsCallback((timings) {
      for (final t in timings) {
        _build += t.buildDuration.inMicroseconds;
        _raster += t.rasterDuration.inMicroseconds;
        _total += t.totalSpan.inMicroseconds;
        _frames++;
        if (t.buildDuration.inMicroseconds > 11100) _slowBuild++;
        if (t.rasterDuration.inMicroseconds > 11100) _slowRaster++;
        if (t.rasterDuration.inMicroseconds > _rasterMax) {
          _rasterMax = t.rasterDuration.inMicroseconds;
        }
      }
    });
  }

  int _n = 0, _rust = 0, _exec = 0, _submit = 0, _words = 0, _rustMax = 0;
  int _frames = 0, _build = 0, _raster = 0, _total = 0, _rasterMax = 0;
  int _slowBuild = 0, _slowRaster = 0, _mapMax = 0, _slowMap = 0;

  int _draws = 0;
  int lineIndices = 0;

  void add(int rust, int exec, int submit, int words, int draws) {
    _draws += draws;
    _n++;
    _rust += rust;
    _exec += exec;
    _submit += submit;
    _words += words;
    if (rust > _rustMax) _rustMax = rust;
    if (rust + exec > _mapMax) _mapMax = rust + exec;
    if (rust + exec > 9000) _slowMap++;
    if (_n < 60) return;
    final f = _frames == 0 ? 1 : _frames;
    debugPrint('PERF rust ${(_rust / _n / 1000).toStringAsFixed(2)} '
        '(max ${(_rustMax / 1000).toStringAsFixed(1)}) '
        'exec ${(_exec / _n / 1000).toStringAsFixed(2)} '
        'submit ${(_submit / _n / 1000).toStringAsFixed(2)} ms, '
        '${_words ~/ _n} words ${_draws ~/ _n} draws ${lineIndices ~/ _n} line idx | flutter frames $_frames '
        'build ${(_build / f / 1000).toStringAsFixed(2)} '
        'raster ${(_raster / f / 1000).toStringAsFixed(2)} '
        '(max ${(_rasterMax / 1000).toStringAsFixed(1)}) '
        'total ${(_total / f / 1000).toStringAsFixed(2)} ms, '
        'over 11.1: build $_slowBuild raster $_slowRaster | rust+exec max '
        '${(_mapMax / 1000).toStringAsFixed(1)}, over 9: $_slowMap');
    _n = _rust = _exec = _submit = _words = _rustMax = _draws = lineIndices = 0;
    _frames = _build = _raster = _total = _rasterMax = _slowBuild = _slowRaster = _mapMax = _slowMap = 0;
  }
}
