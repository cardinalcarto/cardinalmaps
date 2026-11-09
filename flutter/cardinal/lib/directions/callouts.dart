import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

/// A route's label on the map.
@immutable
class Callout {
  const Callout({required this.route, required this.lng, required this.lat});

  /// The route's index.
  final int route;
  final double lng;
  final double lat;

  @override
  bool operator ==(Object other) =>
      other is Callout &&
      other.route == route &&
      other.lng == lng &&
      other.lat == lat;

  @override
  int get hashCode => Object.hash(route, lng, lat);

  @override
  String toString() => 'Callout($route at $lng, $lat)';
}

/// A spot along a route where its callout could go.
class _Spot {
  const _Spot(this.lng, this.lat, this.separation, this.fromMiddle);

  final double lng;
  final double lat;

  /// How far it is from the nearest other route, in meters.
  final double separation;

  /// How far it is along the route from the route's middle, as a fraction of its length.
  final double fromMiddle;
}

/// Where each route's callout goes: the visible point on it that's farthest from every
/// other route, which leaves no doubt which line the callout belongs to. Points about as
/// far from the others as each other, as all of a lone route's are, go to the one nearest
/// the middle of the route.
///
/// Separation doesn't depend on the view, so it's worked out once for a set of routes,
/// which is the slow part; [place] can then be called as the map moves.
class CalloutPlacer {
  CalloutPlacer(List<DirectionsRoute> routes) : _spots = _spotsFor(routes);

  final List<List<_Spot>> _spots;

  /// How many points along a route are considered.
  static const _samples = 128;

  /// Separations within this many meters count as the same.
  static const _tolerance = 1.0;

  /// A callout for each route with a point that's [visible].
  List<Callout> place(bool Function(double lng, double lat) visible) => [
    for (final (route, spots) in _spots.indexed)
      if (_best(spots.where((s) => visible(s.lng, s.lat))) case final spot?)
        Callout(route: route, lng: spot.lng, lat: spot.lat),
  ];

  /// The spot farthest from the other routes, or of those about as far as that, the one
  /// nearest the middle of the route.
  static _Spot? _best(Iterable<_Spot> spots) {
    final candidates = spots.toList();
    if (candidates.isEmpty) return null;
    final farthest = candidates.map((s) => s.separation).reduce(math.max);
    return candidates
        .where((s) => s.separation >= farthest - _tolerance)
        .reduce((a, b) => b.fromMiddle < a.fromMiddle ? b : a);
  }

  static List<List<_Spot>> _spotsFor(List<DirectionsRoute> routes) {
    final firstPoint = routes.expand((route) => route.geometry).firstOrNull;
    if (firstPoint == null) return [for (final _ in routes) const []];
    final projection = _Projection(firstPoint.lat);
    final lines = [
      for (final route in routes)
        [for (final p in route.geometry) projection.project(p)],
    ];
    return [
      for (final (index, route) in routes.indexed)
        _spotsAlong(route.geometry, lines, index),
    ];
  }

  static List<_Spot> _spotsAlong(
    List<DirectionsPoint> geometry,
    List<List<math.Point<double>>> lines,
    int index,
  ) {
    final line = lines[index];
    if (line.isEmpty) return const [];
    final others = [
      for (final (i, other) in lines.indexed)
        if (i != index) other,
    ];
    double separation(math.Point<double> p) => others.isEmpty
        ? 0
        : others.map((other) => _distanceToLine(p, other)).reduce(math.min);

    var length = 0.0;
    for (var i = 1; i < line.length; i++) {
      length += line[i - 1].distanceTo(line[i]);
    }
    if (length == 0) {
      final point = geometry.first;
      return [_Spot(point.lng, point.lat, separation(line.first), 0)];
    }

    final spots = <_Spot>[];
    var segment = 1;
    var before = 0.0;
    for (var i = 0; i < _samples; i++) {
      final fraction = (i + 0.5) / _samples;
      final target = fraction * length;
      while (segment < line.length - 1 &&
          before + line[segment - 1].distanceTo(line[segment]) < target) {
        before += line[segment - 1].distanceTo(line[segment]);
        segment++;
      }
      final span = line[segment - 1].distanceTo(line[segment]);
      final t = span == 0 ? 0.0 : ((target - before) / span).clamp(0.0, 1.0);
      final a = geometry[segment - 1], b = geometry[segment];
      final p = line[segment - 1] + (line[segment] - line[segment - 1]) * t;
      spots.add(
        _Spot(
          a.lng + (b.lng - a.lng) * t,
          a.lat + (b.lat - a.lat) * t,
          separation(p),
          (fraction - 0.5).abs(),
        ),
      );
    }
    return spots;
  }

  static double _distanceToLine(
    math.Point<double> p,
    List<math.Point<double>> line,
  ) {
    if (line.isEmpty) return double.infinity;
    if (line.length == 1) return p.distanceTo(line.first);
    var nearest = double.infinity;
    for (var i = 1; i < line.length; i++) {
      nearest = math.min(nearest, _squaredToSegment(p, line[i - 1], line[i]));
    }
    return math.sqrt(nearest);
  }

  static double _squaredToSegment(
    math.Point<double> p,
    math.Point<double> a,
    math.Point<double> b,
  ) {
    final abx = b.x - a.x, aby = b.y - a.y;
    final squared = abx * abx + aby * aby;
    final t = squared == 0
        ? 0.0
        : (((p.x - a.x) * abx + (p.y - a.y) * aby) / squared).clamp(0.0, 1.0);
    final dx = a.x + t * abx - p.x, dy = a.y + t * aby - p.y;
    return dx * dx + dy * dy;
  }
}

/// Degrees to meters, on a plane that's true near one latitude: enough across a trip to
/// say which of two nearby lines is closer.
class _Projection {
  _Projection(double latitude) : _cos = math.cos(latitude * math.pi / 180);

  final double _cos;
  static const _metersPerDegree = 111320.0;

  math.Point<double> project(DirectionsPoint p) =>
      math.Point(p.lng * _cos * _metersPerDegree, p.lat * _metersPerDegree);
}

/// Whether a longitude and latitude are inside a convex outline of the ground, its
/// corners in order around it.
bool insideOutline(
  List<({double lng, double lat})> corners,
  double lng,
  double lat,
) {
  var sign = 0;
  for (var i = 0; i < corners.length; i++) {
    final a = corners[i], b = corners[(i + 1) % corners.length];
    final cross =
        (b.lng - a.lng) * (lat - a.lat) - (b.lat - a.lat) * (lng - a.lng);
    if (cross == 0) continue;
    final side = cross.sign.toInt();
    if (sign != 0 && side != sign) return false;
    sign = side;
  }
  return true;
}
