/// Where a scrub along a row's graph is on the trip: how far along a route, or when in
/// a journey, what's said of it there, and where it is on the map.
///
/// A graph's width is time. A route's is the time its runs take, as its profile draws
/// them, so a share of the width is a time into the route, and the runs say how far it
/// has gone by then. A journey's is its timeline's, whose pieces are placed at their
/// times but kept wide enough to read, so a point on it is a time within the piece
/// drawn there.
library;

import 'dart:math' as math;

import 'package:routing/routing.dart';

import '../units.dart';
import 'callouts.dart';
import 'transit_plan.dart';
import 'transit_timeline.dart';

/// Meters between two points, near enough for the short distances along a line.
double _meters(DirectionsPoint a, DirectionsPoint b) {
  final scale = math.cos((a.lat + b.lat) / 2 * math.pi / 180);
  final dx = (b.lng - a.lng) * scale;
  final dy = b.lat - a.lat;
  return math.sqrt(dx * dx + dy * dy) * 111195;
}

/// A line on the map, measured along its length, to find the point a distance along
/// it and how far along it a point is.
class LineTrack {
  LineTrack(this.points) : _along = _measure(points);

  final List<DirectionsPoint> points;

  /// How far along the line each of [points] is, in meters.
  final List<double> _along;

  static List<double> _measure(List<DirectionsPoint> points) {
    final along = List<double>.filled(points.length, 0);
    for (var i = 1; i < points.length; i++) {
      along[i] = along[i - 1] + _meters(points[i - 1], points[i]);
    }
    return along;
  }

  /// How long the line is, in meters.
  double get length => _along.isEmpty ? 0 : _along.last;

  /// The point [meters] along the line, kept to its ends; null if it has no points.
  DirectionsPoint? pointAt(double meters) {
    if (points.isEmpty) return null;
    if (points.length == 1 || meters <= 0) return points.first;
    if (meters >= length) return points.last;
    // The last point at or before it.
    var low = 0, high = points.length - 1;
    while (high - low > 1) {
      final mid = (low + high) >> 1;
      if (_along[mid] <= meters) {
        low = mid;
      } else {
        high = mid;
      }
    }
    final span = _along[high] - _along[low];
    final t = span > 0 ? (meters - _along[low]) / span : 0.0;
    final a = points[low], b = points[high];
    return DirectionsPoint(
      lat: a.lat + (b.lat - a.lat) * t,
      lng: a.lng + (b.lng - a.lng) * t,
    );
  }

  /// The point [share] of the way along the line, from 0 at its start to 1 at its end.
  DirectionsPoint? pointAtShare(double share) =>
      pointAt(share.clamp(0.0, 1.0) * length);

  /// How far along the line, in meters, the point of it nearest [p] is, looking no
  /// further back than [from] meters: the stops of a ride snap onto its line in order,
  /// so one that the line passes twice is taken where the ride gets to it.
  double snap(DirectionsPoint p, {double from = 0}) {
    if (points.length < 2) return 0;
    final scale = math.cos(p.lat * math.pi / 180);
    var best = from.clamp(0.0, length);
    var bestDistance = double.infinity;
    for (var i = 1; i < points.length; i++) {
      if (_along[i] < from) continue;
      final a = points[i - 1], b = points[i];
      final ax = (a.lng - p.lng) * scale, ay = a.lat - p.lat;
      final bx = (b.lng - p.lng) * scale, by = b.lat - p.lat;
      final dx = bx - ax, dy = by - ay;
      final squared = dx * dx + dy * dy;
      var t = squared > 0 ? -(ax * dx + ay * dy) / squared : 0.0;
      t = t.clamp(0.0, 1.0);
      final at = _along[i - 1] + (_along[i] - _along[i - 1]) * t;
      if (at < from) continue;
      final x = ax + dx * t, y = ay + dy * t;
      final distance = x * x + y * y;
      if (distance < bestDistance) {
        bestDistance = distance;
        best = at;
      }
    }
    return best;
  }
}

/// Where along a street route a share of its graph's width is.
typedef StreetSpot = ({
  /// How far along the route, in meters, as its own distance counts them.
  double meters,

  /// The share of the route's distance gone, from 0 to 1, for its line on the map.
  double share,

  /// The run of its profile it's in, or null if the route has none.
  DirectionsRun? run,

  /// The road there, from the step it's in, if that has a name.
  String? road,
});

/// Where [route] is [fraction] of the way across its graph, which is the time its runs
/// take: how far along by the runs, or by its distance alone if it has none.
StreetSpot streetSpotAt(DirectionsRoute route, double fraction) {
  final f = fraction.clamp(0.0, 1.0);
  final runs = route.runs;
  final time = runs.fold(0.0, (sum, run) => sum + run.duration);
  final length = runs.fold(0.0, (sum, run) => sum + run.distance);
  var share = f;
  DirectionsRun? at;
  if (time > 0 && length > 0) {
    final target = f * time;
    var elapsed = 0.0;
    var gone = 0.0;
    for (final run in runs) {
      if (run.duration <= 0) {
        gone += run.distance;
        continue;
      }
      at = run;
      if (elapsed + run.duration >= target) {
        gone += run.distance * ((target - elapsed) / run.duration);
        break;
      }
      elapsed += run.duration;
      gone += run.distance;
    }
    share = (gone / length).clamp(0.0, 1.0);
  }
  return (
    meters: share * route.distance,
    share: share,
    run: at,
    road: _roadAt(route.steps, share),
  );
}

/// The name of the road of the step [share] of the way along [steps] by distance.
String? _roadAt(List<DirectionsStep> steps, double share) {
  final total = steps.fold(0.0, (sum, step) => sum + step.distance);
  if (total <= 0) return null;
  final target = share * total;
  var gone = 0.0;
  DirectionsStep? within;
  for (final step in steps) {
    if (step.distance <= 0) continue;
    within = step;
    gone += step.distance;
    if (gone >= target) break;
  }
  final name = within?.roadName;
  return name == null || name.isEmpty ? null : name;
}

/// The height of [elevation] [meters] along a route [distance] long, between the
/// heights either side of it.
double elevationAt(
  DirectionsElevation elevation,
  double distance,
  double meters,
) {
  final heights = elevation.heights;
  final last = heights.length - 1;
  if (last <= 0) return heights.isEmpty ? 0 : heights.first;
  // Every interval along, and the last at the end, as the profile draws them.
  final end = distance.clamp(
    (last - 1) * elevation.interval,
    last * elevation.interval,
  );
  double at(int k) => k == last ? end : k * elevation.interval;
  final m = meters.clamp(0.0, end);
  final k = math.min((m / elevation.interval).floor(), last - 1);
  final span = at(k + 1) - at(k);
  final t = span > 0 ? ((m - at(k)) / span).clamp(0.0, 1.0) : 0.0;
  return heights[k] + (heights[k + 1] - heights[k]) * t;
}

/// How far either side of a point its grade is measured, in meters: several of the
/// heights, 30 m apart, so one height's error doesn't make a hill.
const double gradeWindow = 50;

/// Below this grade, either way, the ground is said to be level.
const double levelGrade = 0.01;

/// The grade [meters] along a route [distance] long, rise over run, over
/// [gradeWindow] either side, kept to the route; null if that's too short to tell.
double? gradeAt(DirectionsElevation elevation, double distance, double meters) {
  final from = math.max(0.0, meters - gradeWindow);
  final to = math.min(distance, meters + gradeWindow);
  if (to - from < 10) return null;
  return (elevationAt(elevation, distance, to) -
          elevationAt(elevation, distance, from)) /
      (to - from);
}

/// [meters] as a milepost or kilometer post: "Mile 6.2", "km 10.0"; whole from 100 on.
String formatMilepost(double meters, {required UnitSystem units}) {
  assert(units != UnitSystem.locale, 'resolve the units for a locale first');
  String tenths(double value) =>
      value >= 100 ? value.round().toString() : value.toStringAsFixed(1);
  return units == UnitSystem.imperial
      ? 'Mile ${tenths(meters / 1609.344)}'
      : 'km ${tenths(meters / 1000)}';
}

/// [metersPerSecond] as a speed: "58 mph", "93 km/h".
String formatSpeed(double metersPerSecond, {required UnitSystem units}) {
  assert(units != UnitSystem.locale, 'resolve the units for a locale first');
  return units == UnitSystem.imperial
      ? '${(metersPerSecond * 2.236936).round()} mph'
      : '${(metersPerSecond * 3.6).round()} km/h';
}

/// [meters] above sea level, to the nearest 10 ft or 5 m, as climbs are said.
String formatHeight(double meters, {required UnitSystem units}) {
  assert(units != UnitSystem.locale, 'resolve the units for a locale first');
  return units == UnitSystem.imperial
      ? '${(meters * 3.28084 / 10).round() * 10} ft'
      : '${(meters / 5).round() * 5} m';
}

/// [grade], rise over run, in words: "6% uphill", "3% downhill", "level".
String formatGrade(double grade) {
  if (grade.abs() < levelGrade) return 'level';
  final percent = (grade.abs() * 100).round();
  return '$percent% ${grade > 0 ? 'uphill' : 'downhill'}';
}

/// What the readout says [fraction] of the way across [route]'s graph, most important
/// first: how far along, then for a drive the road and the speed expected there, and
/// for a ride or a walk the height and the grade, each left out when the route
/// doesn't say.
List<String> streetReadout(
  DirectionsRoute route,
  double fraction, {
  required bool driving,
  required UnitSystem units,
}) {
  final spot = streetSpotAt(route, fraction);
  final elevation = route.elevation;
  final grade = elevation == null
      ? null
      : gradeAt(elevation, route.distance, spot.meters);
  return [
    formatMilepost(spot.meters, units: units),
    if (driving) ...[
      ?spot.road,
      if (spot.run case final run? when run.speed > 0)
        formatSpeed(run.speed, units: units),
    ] else if (elevation != null && elevation.heights.isNotEmpty) ...[
      formatHeight(
        elevationAt(elevation, route.distance, spot.meters),
        units: units,
      ),
      if (grade != null) formatGrade(grade),
    ],
  ];
}

/// Each route's line, measured, once.
final _routeLines = Expando<LineTrack>('route line');

/// Where [route] is on the map [fraction] of the way across its graph, along its line
/// by the share of its distance gone; null if it has no line.
DirectionsPoint? streetPointAt(DirectionsRoute route, double fraction) {
  final line = _routeLines[route] ??= LineTrack(route.geometry);
  return line.pointAtShare(streetSpotAt(route, fraction).share);
}

/// When in a journey a point [x] across its timeline is, the timeline's pieces placed
/// as [placed]: within the piece drawn there, by its share of that piece's time. Kept
/// to the drawn pieces, the first's start to the last's end.
DateTime timelineTimeAt(List<PlacedPiece> placed, double x) {
  assert(placed.isNotEmpty);
  final first = placed.first, last = placed.last;
  if (x <= first.left) return first.piece.start;
  if (x >= last.right) return last.piece.end;
  final p = placed.lastWhere((p) => p.left <= x, orElse: () => first);
  final share = p.width > 0 ? ((x - p.left) / p.width).clamp(0.0, 1.0) : 0.0;
  final span = p.piece.end.difference(p.piece.start).inMilliseconds;
  return p.piece.start.add(Duration(milliseconds: (span * share).round()));
}

/// What a rider of [journey] is doing at [time], as the readout says it: "on 1 Line",
/// "walking", "waiting at Westlake".
String journeyActivity(Journey journey, DateTime time) {
  final legs = journey.legs;
  for (final (i, leg) in legs.indexed) {
    if (time.isAfter(leg.arrival)) continue;
    if (time.isBefore(leg.departure)) return _waitingFor(leg);
    switch (leg) {
      case TransitLeg():
        return 'on ${routeBadgeText(_routeAt(leg, time))}';
      case StreetLeg(duration: final d) when d > 0:
        return switch (leg.mode) {
          'cycling' => 'cycling',
          'wheelchair' => 'by wheelchair',
          _ => 'walking',
        };
      case StreetLeg():
        // A change at one stop: the wait is for the next leg.
        if (i + 1 < legs.length) return _waitingFor(legs[i + 1]);
    }
  }
  return 'arriving';
}

/// "waiting at" the stop where [leg] starts, or just "waiting" if it doesn't start at
/// one.
String _waitingFor(JourneyLeg leg) {
  final stop = switch (leg) {
    TransitLeg() => leg.boarding.stop,
    StreetLeg() => leg.from,
  };
  return stop == null || stop.name.isEmpty
      ? 'waiting'
      : 'waiting at ${stop.name}';
}

/// The route [ride] is on at [time]: on a stay-seated ride, that of the trip it's on
/// then.
TransitRoute? _routeAt(TransitLeg ride, DateTime time) {
  final segments = ride.segments;
  if (segments.length < 2) return ride.route;
  final last = ride.stops.length - 1;
  for (final segment in segments) {
    final end = ride.stops[segment.lastStop.clamp(0, last)];
    if (!time.isAfter(end.arrival)) return segmentRoute(ride, segment);
  }
  return segmentRoute(ride, segments.last);
}

/// A leg of a journey measured for finding where on it a rider is at a time.
sealed class _LegTrack {
  DirectionsPoint? pointAt(DateTime time);
}

/// A walk or a ride on the streets: along its line by its share of the leg's time.
class _StreetTrack extends _LegTrack {
  _StreetTrack(StreetLeg leg)
    : _leg = leg,
      _line = LineTrack([
        if (leg.shape.isNotEmpty)
          ...leg.shape
        else ...[
          ?leg.from?.point,
          ?leg.to?.point,
        ],
      ]);

  final StreetLeg _leg;
  final LineTrack _line;

  @override
  DirectionsPoint? pointAt(DateTime time) {
    final span = _leg.arrival.difference(_leg.departure).inMilliseconds;
    final share = span > 0
        ? time.difference(_leg.departure).inMilliseconds / span
        : 1.0;
    return _line.pointAtShare(share);
  }
}

/// A ride: from stop to stop by their times, along its line between where each stop
/// is on it, and standing at a stop while it's there.
class _RideTrack extends _LegTrack {
  _RideTrack(TransitLeg ride) : _line = LineTrack(ride.shape) {
    var from = 0.0;
    for (final visit in ride.stops) {
      final point = visit.stop.point;
      if (point == null) continue;
      from = _line.snap(point, from: from);
      _stops.add((visit: visit, along: from));
    }
  }

  final LineTrack _line;

  /// The stops that say where they are, and how far along the line each is.
  final _stops = <({StopVisit visit, double along})>[];

  @override
  DirectionsPoint? pointAt(DateTime time) {
    if (_stops.isEmpty) return _line.pointAtShare(0);
    if (!time.isAfter(_stops.first.visit.departure)) {
      return _line.pointAt(_stops.first.along);
    }
    for (var i = 0; i + 1 < _stops.length; i++) {
      final a = _stops[i], b = _stops[i + 1];
      if (!time.isAfter(a.visit.departure)) return _line.pointAt(a.along);
      if (time.isAfter(b.visit.arrival)) continue;
      final span = b.visit.arrival.difference(a.visit.departure).inMilliseconds;
      final share = span > 0
          ? time.difference(a.visit.departure).inMilliseconds / span
          : 1.0;
      return _line.pointAt(a.along + (b.along - a.along) * share);
    }
    return _line.pointAt(_stops.last.along);
  }
}

/// Each journey's legs, measured, once.
final _journeyTracks = Expando<List<_LegTrack>>('journey tracks');

/// Where a rider of [journey] is at [time]: along a ride between its stops by their
/// times, along a walk by its time, and at the stop where the next leg starts while
/// waiting for it. Null if the legs don't say where they go.
DirectionsPoint? journeyPointAt(Journey journey, DateTime time) {
  final tracks = _journeyTracks[journey] ??= [
    for (final leg in journey.legs)
      switch (leg) {
        TransitLeg() => _RideTrack(leg),
        StreetLeg() => _StreetTrack(leg),
      },
  ];
  final legs = journey.legs;
  for (var i = 0; i < legs.length; i++) {
    final leg = legs[i];
    if (time.isAfter(leg.arrival)) continue;
    // Waiting for it to start, at where it starts.
    final at = time.isBefore(leg.departure) ? leg.departure : time;
    final point = tracks[i].pointAt(at);
    if (point != null) return point;
  }
  for (var i = legs.length - 1; i >= 0; i--) {
    if (tracks[i].pointAt(legs[i].arrival) case final point?) return point;
  }
  return null;
}

/// How far, in degrees, the map has to move for the point at [lng], [lat] to be inside
/// [corners] (a convex outline of the ground, as [insideOutline] takes it): none if it
/// is, and otherwise to the nearest point of the outline's edge, so it moves no more
/// than it must.
({double lng, double lat})? panInto(
  List<({double lng, double lat})> corners,
  double lng,
  double lat,
) {
  if (corners.length < 3 || insideOutline(corners, lng, lat)) return null;
  // On the ground near the point, east and north alike in meters.
  final scale = math.cos(lat * math.pi / 180);
  var best = (lng: lng, lat: lat);
  var bestDistance = double.infinity;
  for (var i = 0; i < corners.length; i++) {
    final a = corners[i], b = corners[(i + 1) % corners.length];
    final ax = (a.lng - lng) * scale, ay = a.lat - lat;
    final dx = (b.lng - a.lng) * scale, dy = b.lat - a.lat;
    final squared = dx * dx + dy * dy;
    final t = squared > 0
        ? (-(ax * dx + ay * dy) / squared).clamp(0.0, 1.0)
        : 0.0;
    final x = ax + dx * t, y = ay + dy * t;
    final distance = x * x + y * y;
    if (distance < bestDistance) {
      bestDistance = distance;
      best = (
        lng: a.lng + (b.lng - a.lng) * t,
        lat: a.lat + (b.lat - a.lat) * t,
      );
    }
  }
  return (lng: lng - best.lng, lat: lat - best.lat);
}
