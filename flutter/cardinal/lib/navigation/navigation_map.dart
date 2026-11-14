import 'dart:math' as math;

import 'package:flutter/foundation.dart';
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
/// On the route, the chevron is on the route's line ([NavUpdate.snappedLocation]). Off
/// it, and while a new route is coming, the line is the road you left, so the chevron is
/// where navigation put you instead ([NavUpdate.effectiveLocation]): on the road you're
/// on, where your fixes were matched to one, or at the fix itself.
///
/// Each update from the controller moves the chevron, gliding over the time since the
/// update before, and ends the traveled line where the update put you; the chevron's
/// disk covers the stretch between the two. A new route redraws the line.
///
/// The camera follows from the first update, at the mode's zoom for your speed
/// ([FollowCamera.zoomAt]). [overview] shows the rest of the trip instead, and moving
/// the map by hand stops it following too; either way it stays where it was put until
/// [recenter]. [cameraMode] says which it's doing.
///
/// Once the trip has arrived, [overview] shows the destination instead (see there),
/// which the navigation screen asks for as it arrives, and the chevron is hidden: the
/// trip is over, and its disk would sit on the destination, over its pin.
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

  /// Your speed at the last update, in meters per second, if it could be told.
  double? _speed;
  EdgeInsets _padding = EdgeInsets.zero;
  bool _started = false;

  /// Whether the chevron was last shown: once there's a place to put it, until the
  /// trip arrives.
  bool _puckShown = false;

  /// Starts drawing the trip, and keeps drawing it until [dispose].
  void start() {
    if (_started) return;
    _started = true;
    map.setPuckStyle(const PuckStyle());
    controller.addListener(_changed);
    map.following.addListener(_followingChanged);
    _changed();
  }

  /// What the camera is doing: null until it first follows the chevron, which is when
  /// there's a chevron to follow or a rest of the trip to show.
  ValueListenable<NavigationCameraMode?> get cameraMode => _cameraMode;
  final _cameraMode = ValueNotifier<NavigationCameraMode?>(null);

  /// Whether [overview] put the camera where it is, and nothing has moved it since but
  /// the map by hand, which [SparkleMapController.hasSubject] tells.
  bool _overview = false;

  /// What [overview] was last told to keep the trip clear of.
  EdgeInsets _overviewClear = EdgeInsets.zero;

  /// Whether [overview] last showed the destination, at the trip's arrival, rather
  /// than the rest of the trip.
  bool _overviewArrived = false;

  /// The zoom the destination is shown at once arrived: the camera's as it arrived.
  double? _arrivalZoom;

  void _followingChanged() {
    if (map.following.value) _overview = false;
    _cameraMode.value = _zoom == null
        ? null
        : map.following.value
        ? NavigationCameraMode.following
        : _overview
        ? NavigationCameraMode.overview
        : NavigationCameraMode.free;
  }

  /// Follows the chevron again, as from an update: at the zoom for the last speed and
  /// the mode's tilt, easing back over [zoomDuration]. Does nothing before the camera
  /// first follows.
  void recenter() {
    if (_zoom == null) return;
    _overview = false;
    _follow(_camera.zoomAt(_speed));
  }

  /// Stops following and shows the rest of the trip, from the chevron to the
  /// destination, flat and north up, as Ferrostar's route overview is (spec §2.5): it
  /// reads as a map then, rather than as the road ahead. It's kept clear of [clear],
  /// what the screen draws over the map, on top of the map's own padding.
  ///
  /// It's the rest of the trip, where Ferrostar shows the whole route: what's behind
  /// you is done with, and leaving it out shows more of what's ahead. A new route while
  /// in overview is shown in its place. Does nothing before the camera first follows,
  /// with no chevron to show the trip from.
  ///
  /// Told again what to keep clear while in overview, it fits the trip again only if
  /// that changed, so the screen can tell it on every move of what covers the map.
  ///
  /// Once the trip has arrived, it shows the destination instead: the route's end in
  /// the middle of what [clear] leaves, at the zoom the camera had as it arrived, flat
  /// and north up as the overview is. With nothing left to travel there's no rest of
  /// the trip to fit, and the zoom you were following at keeps the streets around it
  /// as they were, rather than jumping in or out.
  void overview(EdgeInsets clear) {
    if (_zoom == null) return;
    final arrived = controller.state == NavigationState.arrived;
    if (_overview &&
        clear == _overviewClear &&
        arrived == _overviewArrived &&
        map.hasSubject &&
        !map.following.value) {
      return;
    }
    _overview = true;
    _overviewClear = clear;
    _overviewArrived = arrived;
    if (arrived) {
      _fitDestination();
    } else {
      _fitRest();
    }
    _followingChanged();
  }

  /// Makes the destination the map's subject, as [overview] shows it once arrived.
  void _fitDestination() {
    final end = _route?.geometry.lastOrNull;
    if (end == null) return;
    final zoom = _arrivalZoom ??= map.camera?.zoom ?? _zoom!;
    map.setSubject(
      west: end.lng,
      south: end.lat,
      east: end.lng,
      north: end.lat,
      maxZoom: zoom,
      margin: overviewMargin,
      clear: _overviewClear,
      bearing: 0,
      pitch: 0,
    );
  }

  /// Makes the rest of the trip the map's subject, as [overview] shows it.
  void _fitRest() {
    final route = _route;
    final at = _at;
    if (route == null || at == null) return;
    var west = at.lng, east = at.lng, south = at.lat, north = at.lat;
    final from = (_update?.geometryIndex ?? 0) + 1;
    for (final p in route.geometry.skip(from)) {
      west = math.min(west, p.lng);
      east = math.max(east, p.lng);
      south = math.min(south, p.lat);
      north = math.max(north, p.lat);
    }
    map.setSubject(
      west: west,
      south: south,
      east: east,
      north: north,
      maxZoom: overviewMaxZoom,
      margin: overviewMargin,
      clear: _overviewClear,
      bearing: 0,
      pitch: 0,
    );
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
    if (controller.state != NavigationState.arrived) _arrivalZoom = null;
    if (route == null) return;
    if (newRoute) {
      // The traveled part is of the old route's line until the next update.
      _traveled = const [];
    }
    if (newUpdate && update != null) _move(route, update);
    final puckShown =
        _at != null && controller.state != NavigationState.arrived;
    if (puckShown != _puckShown) {
      _puckShown = puckShown;
      map.setPuckVisible(puckShown);
    }
    if (newRoute && _overview) {
      // Showing the old route's rest, unless the map was moved by hand since.
      if (map.hasSubject) {
        _fitRest();
      } else {
        _overview = false;
        _followingChanged();
      }
    }
    if (newRoute || newUpdate) {
      map.setGeoJson(
        navigationSource,
        navigationGeoJson(route.geometry, _traveled),
      );
    }
  }

  void _move(DirectionsRoute route, NavUpdate update) {
    final snapped = update.snappedLocation;
    if (snapped == null) return;
    final at = switch (controller.state) {
      NavigationState.offRoute ||
      NavigationState.rerouting => update.effectiveLocation ?? snapped,
      _ => snapped,
    };
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
    // The traveled part ends where the route's line has you, wherever the chevron is.
    _traveled = traveledLine(
      route.geometry,
      update.geometryIndex,
      DirectionsPoint(lat: snapped.lat, lng: snapped.lng),
    );
    final speed =
        update.location?.speed ?? at.speed ?? _motion(previous, at, since);
    _speed = speed;
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
    final first = _zoom == null;
    _zoom = zoom;
    map.follow(
      zoom: zoom,
      pitch: _camera.pitch,
      padding: _padding,
      duration: zoomDuration,
    );
    // Following says so itself, but only once it changes.
    if (first) _followingChanged();
  }

  /// Stops drawing the trip: the chevron is hidden, the line taken away, and the camera
  /// stays where it is.
  void dispose() {
    if (!_started) return;
    controller.removeListener(_changed);
    map.following.removeListener(_followingChanged);
    _cameraMode.dispose();
    map
      ..stopFollowing()
      ..setPuckVisible(false)
      ..setGeoJson(navigationSource, empty);
  }
}

/// What the navigation camera is doing.
enum NavigationCameraMode {
  /// Following the chevron.
  following,

  /// Showing the rest of the trip ([NavigationMap.overview]).
  overview,

  /// Where it was moved to by hand.
  free,
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
