import 'dart:math' as math;

import 'package:flutter/painting.dart';
import 'package:routing/routing.dart';
import 'package:sparkle/sparkle.dart';

import '../map/route_layer.dart';
import '../navigation/navigation_camera.dart';
import 'car_keys.dart';

/// The chevron on the car's screen: smaller than the phone's, since a car's screen is
/// often half as tall, with a disk that still covers the traveled line's end
/// ([chevronLag] stays near 12 px driving, under this radius less half the line).
const carPuckStyle = PuckStyle(size: 40, diskRadius: 28);

/// Draws a trip on the car's map from what the phone sends of it (`car_trip.dart`), as
/// `NavigationMap` draws the phone's from the controller: the chevron, the route ahead
/// with the part traveled over it, and the camera following the chevron at the mode's
/// zoom for your speed.
///
/// Nobody can move a car's map by hand (CarPlay keeps touches for its own buttons), so
/// the camera follows until [overview] shows the rest of the trip, and again from
/// [recenter]. Once the trip has arrived it shows the destination, without the
/// chevron, as the phone does.
///
/// The map takes a moment to set up after the car connects, and the camera can't be
/// moved before: [ready] says when it is, and the camera goes where it should then.
class CarMap {
  CarMap(this.map, {Duration Function()? elapsed})
    : _elapsed = elapsed ?? _stopwatch();

  final SparkleMapController map;

  /// Time on a steady clock, to tell how far apart messages came.
  final Duration Function() _elapsed;

  static Duration Function() _stopwatch() {
    final watch = Stopwatch()..start();
    return () => watch.elapsed;
  }

  bool _ready = false;
  List<DirectionsPoint> _line = const [];
  List<DirectionsPoint> _traveled = const [];
  NavigationMode? _mode;
  bool _arrived = false;

  /// Where the chevron was last put, when, and the line's point at or before it.
  ({double lng, double lat})? _at;
  Duration? _movedAt;
  double _bearing = 0;
  int _index = 0;
  double? _speed;

  /// Whether the rest of the trip shows, rather than the chevron being followed.
  bool _overview = false;

  /// The zoom the camera was last told to follow at; null while it isn't following.
  double? _followZoom;
  EdgeInsets _padding = EdgeInsets.zero;

  /// Starts drawing: the chevron's look is set, and it's hidden until a trip puts it
  /// somewhere.
  void start() => map.setPuckStyle(carPuckStyle);

  /// The map is set up: the camera goes where the trip has it.
  void ready() {
    if (_ready) return;
    _ready = true;
    _aim();
  }

  /// Where the camera puts the chevron when following: [padding] on top of the map's
  /// own (see [followPadding]).
  set padding(EdgeInsets padding) {
    if (padding == _padding) return;
    _padding = padding;
    if (!_overview) _aim();
  }

  /// The trip's route changed: [line] is drawn in the old one's place.
  void route(List<DirectionsPoint> line) {
    _line = line;
    // The traveled part is of the old route's line until the next message.
    _traveled = const [];
    _index = 0;
    _draw();
    if (_overview) _aim();
  }

  /// What the phone says of the trip now ([carTripOf]); null when it's over.
  void trip(Map<Object?, Object?>? trip) {
    if (trip == null) {
      _clear();
      return;
    }
    final mode = NavigationMode.values.asNameMap()[trip[CarKeys.mode]];
    final arrived = trip[CarKeys.state] == 'arrived';
    final newlyArrived = arrived && !_arrived;
    final first = _at == null;
    _mode = mode ?? _mode ?? NavigationMode.driving;
    _arrived = arrived;
    final moved = _move(trip);
    map.setPuckVisible(_at != null && !arrived);
    if (moved) _draw();
    if (newlyArrived) {
      _aim();
    } else if (moved && !arrived && (first || !_overview)) {
      _aim(moved: !first);
    }
  }

  /// Shows the rest of the trip, flat and north up, until [recenter].
  void overview() {
    _overview = true;
    _aim();
  }

  /// Follows the chevron again.
  void recenter() {
    _overview = false;
    _aim();
  }

  /// Moves the chevron and the traveled line's end to where [trip] has them; whether
  /// it had anywhere to put them.
  bool _move(Map<Object?, Object?> trip) {
    final at = trip[CarKeys.at];
    final snapped = trip[CarKeys.snapped];
    if (at is! Map || snapped is! Map) return false;
    final lng = _number(at[CarKeys.lng]);
    final lat = _number(at[CarKeys.lat]);
    final endLng = _number(snapped[CarKeys.lng]);
    final endLat = _number(snapped[CarKeys.lat]);
    if (lng == null || lat == null || endLng == null || endLat == null) {
      return false;
    }
    final now = _elapsed();
    final movedAt = _movedAt;
    _movedAt = now;
    _at = (lng: lng, lat: lat);
    _bearing = _number(at[CarKeys.bearing]) ?? _bearing;
    _speed = _number(at[CarKeys.speed]);
    _index = (_number(trip[CarKeys.index]) ?? 0).toInt();
    map.setPuck(
      lng: lng,
      lat: lat,
      bearing: _bearing,
      duration: glideDuration(movedAt == null ? null : now - movedAt),
    );
    _traveled = traveledLine(
      _line,
      _index,
      DirectionsPoint(lat: endLat, lng: endLng),
    );
    return true;
  }

  void _draw() =>
      map.setGeoJson(navigationSource, navigationGeoJson(_line, _traveled));

  /// Puts the camera where the trip has it: on the destination once arrived, on the
  /// rest of the trip in overview, and following the chevron otherwise.
  ///
  /// When only the chevron [moved], a camera that's already following is told again
  /// only if the speed changed its zoom, as on the phone: it follows the chevron by
  /// itself.
  void _aim({bool moved = false}) {
    if (!_ready) return;
    if (_arrived) {
      // The session gives no position once the trip is complete, so this doesn't
      // wait for one: a car that connects after the arrival still shows it.
      final end = _line.lastOrNull;
      if (end != null) _show(end.lng, end.lat, end.lng, end.lat);
      return;
    }
    final at = _at;
    if (at == null) return;
    if (_overview) {
      var west = at.lng, east = at.lng, south = at.lat, north = at.lat;
      for (final p in _line.skip(_index + 1)) {
        west = math.min(west, p.lng);
        east = math.max(east, p.lng);
        south = math.min(south, p.lat);
        north = math.max(north, p.lat);
      }
      _show(west, south, east, north);
      return;
    }
    final camera = followCamera(_mode ?? NavigationMode.driving);
    final zoom = camera.zoomAt(_speed);
    final followed = _followZoom;
    if (moved && followed != null && (zoom - followed).abs() <= 0.01) return;
    _followZoom = zoom;
    map.follow(
      zoom: zoom,
      pitch: camera.pitch,
      padding: _padding,
      duration: const Duration(seconds: 1),
    );
  }

  void _show(double west, double south, double east, double north) {
    _followZoom = null;
    map.setSubject(
      west: west,
      south: south,
      east: east,
      north: north,
      maxZoom: overviewMaxZoom,
      margin: 16 + carPuckStyle.diskRadius,
      bearing: 0,
      pitch: 0,
    );
  }

  /// The trip is over: the chevron and the line go, and the camera stays where it is.
  void _clear() {
    _line = const [];
    _traveled = const [];
    _mode = null;
    _arrived = false;
    _at = null;
    _movedAt = null;
    _index = 0;
    _speed = null;
    _overview = false;
    _followZoom = null;
    map
      ..stopFollowing()
      ..setPuckVisible(false)
      ..setGeoJson(navigationSource, empty);
  }

  static double? _number(Object? value) =>
      value is num ? value.toDouble() : null;
}
