/// The marker on the map at where a row's graph is scrubbed to.
library;

import 'dart:async';

import 'package:flutter/scheduler.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';
import 'package:sparkle/sparkle.dart';

import '../map/route_layer.dart';

/// Puts the scrub marker on [map] (in [scrubSource]) where it's [move]d to, at most once
/// a frame however often it's moved, and tells [onPlaced] each time it's put somewhere
/// new, for the camera to keep it in view. Once [release]d it stays for [linger], then
/// fades out over [fade]; moving it again brings it back at once.
class ScrubMarker {
  ScrubMarker({
    required this.map,
    required TickerProvider vsync,
    required this.onPlaced,
  }) : _shown = AnimationController(vsync: vsync, duration: fade, value: 0) {
    _shown.addListener(_schedule);
  }

  final SparkleMapController map;
  final void Function(DirectionsPoint point) onPlaced;

  /// How long the marker stays after it's released, before it fades.
  static const linger = Duration(seconds: 1);

  /// How long it takes to fade out.
  static const fade = Duration(milliseconds: 300);

  /// How much of it shows.
  final AnimationController _shown;

  /// Where it is, and whether it's been moved since it was last put there.
  DirectionsPoint? _point;
  bool _moved = false;

  /// Whether it's to be put on the map in the next frame.
  bool _scheduled = false;

  /// Whether it's on the map, as far as was last put there.
  bool _drawn = false;
  Timer? _linger;

  /// Whether it shows, or is fading.
  bool get shown => _shown.value > 0;

  /// Puts it at [point], or takes it off the map for now if that's null, showing it
  /// whole.
  void move(DirectionsPoint? point) {
    _linger?.cancel();
    _linger = null;
    _shown
      ..stop()
      ..value = 1;
    _point = point;
    _moved = true;
    _schedule();
  }

  /// It's no longer being moved: it stays [linger], then fades.
  void release() {
    _linger?.cancel();
    _linger = Timer(linger, () {
      _linger = null;
      _shown.reverse(from: 1);
    });
  }

  /// Takes it off the map at once.
  void hide() {
    _linger?.cancel();
    _linger = null;
    _shown
      ..stop()
      ..value = 0;
    _point = null;
    _schedule();
  }

  bool _disposed = false;

  void _schedule() {
    if (_scheduled || _disposed) return;
    _scheduled = true;
    SchedulerBinding.instance.scheduleFrameCallback((_) {
      _scheduled = false;
      if (!_disposed) _draw();
    });
  }

  void _draw() {
    final point = _point;
    final opacity = _shown.value;
    final draws = point != null && opacity > 0;
    if (draws || _drawn) {
      map.setGeoJson(
        scrubSource,
        scrubGeoJson(draws ? point : null, opacity: opacity),
      );
    }
    _drawn = draws;
    if (draws && _moved) onPlaced(point);
    _moved = false;
  }

  void dispose() {
    _disposed = true;
    _linger?.cancel();
    _shown.dispose();
  }
}
