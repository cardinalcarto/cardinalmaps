import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';
import 'package:sparkle/sparkle.dart';

import '../map/route_layer.dart';
import 'navigation_camera.dart';
import 'navigation_controller.dart';

/// Draws a trip on the map while the navigation screen shows: the chevron where the
/// controller puts you, the route ahead with the part traveled over it, and the camera
/// following the chevron.
///
/// Each update from the controller moves the chevron, gliding over the time since the
/// update before, and ends the traveled line where the update put you; the chevron's
/// disk covers the stretch between the two. A new route redraws the line.
///
/// The camera follows from the first update, at the mode's zoom for your speed
/// ([FollowCamera.zoomAt]). Moving the map by hand stops it following, and it stays
/// where it was put.
class NavigationMap {
  NavigationMap({
    required this.map,
    required this.controller,
    Duration Function()? elapsed,
  }) : _elapsed = elapsed ?? _stopwatch();

  final SparkleMapController map;
  final NavigationController controller;

  /// Time on a steady clock, to tell how far apart updates came.
  final Duration Function() _elapsed;

  static Duration Function() _stopwatch() {
    final watch = Stopwatch()..start();
    return () => watch.elapsed;
  }

  /// How long a change of zoom takes: as long as fixes are apart, so changes from one
  /// fix to the next run into each other rather than step.
  static const zoomDuration = Duration(seconds: 1);

  DirectionsRoute? _route;
  NavUpdate? _update;
  List<DirectionsPoint> _traveled = const [];

  /// When the last update moved the chevron, and where to.
  Duration? _movedAt;
  NavLocation? _at;
  double _bearing = 0;

  /// The zoom the camera was last told to follow at, null until it's told to.
  double? _zoom;
  EdgeInsets _padding = EdgeInsets.zero;
  bool _started = false;

  /// Starts drawing the trip, and keeps drawing it until [dispose].
  void start() {
    if (_started) return;
    _started = true;
    map.setPuckStyle(const PuckStyle());
    controller.addListener(_changed);
    _changed();
  }

  /// Where the camera puts the chevron: [padding] on top of the map's own (see
  /// [followPadding]).
  set padding(EdgeInsets padding) {
    if (padding == _padding) return;
    _padding = padding;
    final zoom = _zoom;
    if (zoom != null && map.following.value) _follow(zoom);
  }

  void _changed() {
    final route = controller.route;
    final update = controller.update;
    final newRoute = !identical(route, _route);
    final newUpdate = !identical(update, _update);
    _route = route;
    _update = update;
    if (route == null) return;
    if (newRoute) {
      // The traveled part is of the old route's line until the next update.
      _traveled = const [];
    }
    if (newUpdate && update != null) _move(route, update);
    if (newRoute || newUpdate) {
      map.setGeoJson(
        navigationSource,
        navigationGeoJson(route.geometry, _traveled),
      );
    }
  }

  void _move(DirectionsRoute route, NavUpdate update) {
    final at = update.snappedLocation;
    if (at == null) return;
    final now = _elapsed();
    final movedAt = _movedAt;
    final since = movedAt == null ? null : now - movedAt;
    final previous = _at;
    _movedAt = now;
    _at = at;
    _bearing = at.course ?? _bearing;
    map.setPuck(
      lng: at.lng,
      lat: at.lat,
      bearing: _bearing,
      duration: glideDuration(since),
    );
    if (previous == null) map.setPuckVisible(true);
    _traveled = traveledLine(
      route.geometry,
      update.geometryIndex,
      DirectionsPoint(lat: at.lat, lng: at.lng),
    );
    final speed =
        update.location?.speed ?? at.speed ?? _motion(previous, at, since);
    final zoom = _camera.zoomAt(speed);
    final last = _zoom;
    if (last == null) {
      _follow(zoom);
    } else if (map.following.value && (zoom - last).abs() > 0.01) {
      _follow(zoom);
    }
  }

  FollowCamera get _camera => followCamera(navigationMode(controller.mode!)!);

  void _follow(double zoom) {
    _zoom = zoom;
    map.follow(
      zoom: zoom,
      pitch: _camera.pitch,
      padding: _padding,
      duration: zoomDuration,
    );
  }

  /// Stops drawing the trip: the chevron is hidden, the line taken away, and the camera
  /// stays where it is.
  void dispose() {
    if (!_started) return;
    controller.removeListener(_changed);
    map
      ..stopFollowing()
      ..setPuckVisible(false)
      ..setGeoJson(navigationSource, empty);
  }
}

/// How fast you moved from [from] to [to] in [over], in meters per second, for a fix
/// without a speed of its own; null if it can't be told.
double? _motion(NavLocation? from, NavLocation to, Duration? over) {
  if (from == null || over == null || over <= Duration.zero) return null;
  return _meters(from, to) / (over.inMicroseconds / 1e6);
}

/// The distance between [a] and [b] along the ground, in meters.
double _meters(NavLocation a, NavLocation b) {
  const earthRadius = 6371008.8;
  double rad(double d) => d * math.pi / 180;
  final dLat = rad(b.lat - a.lat);
  final dLng = rad(b.lng - a.lng);
  final h =
      math.pow(math.sin(dLat / 2), 2) +
      math.cos(rad(a.lat)) *
          math.cos(rad(b.lat)) *
          math.pow(math.sin(dLng / 2), 2);
  return 2 * earthRadius * math.asin(math.sqrt(h));
}
