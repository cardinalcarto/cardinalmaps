/// Turning a finger's lasso on the screen into a region's polygon: closing the stroke,
/// trimming where it overshoots its start, simplifying it, and checking it encloses a
/// usable area. Pure functions, in logical pixels, so they can be tested without a map.
library;

import 'dart:ui' show Offset;

import '../downloads/geometry.dart';

/// Why a lasso can't be used.
enum LassoProblem {
  /// It encloses too little to download.
  tooSmall,

  /// It crosses itself, beyond overshooting where it started.
  crossesItself,

  /// Part of it isn't over the ground (the sky of a tilted map, say).
  offMap,
}

/// Points of a stroke closer together than this are one point.
const lassoPointSpacing = 2.0;

/// How far, in logical pixels, the outline may stray from the stroke when simplified.
const lassoTolerance = 3.0;

/// The most corners an outline keeps: the tolerance grows until it's this few.
const lassoMaxVertices = 48;

/// The least area a lasso may enclose, in square logical pixels: a 56-pixel square,
/// about a fingertip and a half on a side.
const lassoMinArea = 56.0 * 56.0;

/// Where a stroke crosses itself, how much area the smaller part may enclose, as a
/// share of the larger's, to be dropped: an overshoot past the start, or a little curl.
/// More is a second loop (a figure eight).
const lassoMaxTrimmedShare = 0.2;

/// The most crossings a stroke may have trimmed.
const _maxCrossings = 6;

/// A lasso's outline, or why there's none.
typedef LassoResult = ({List<Offset>? ring, LassoProblem? problem});

/// Whether a stroke of [points] is just a tap or a twitch, to be ignored rather than
/// answered with a problem.
bool isLassoTap(List<Offset> points) {
  var length = 0.0;
  for (var i = 1; i < points.length; i++) {
    length += (points[i] - points[i - 1]).distance;
    if (length >= 24) return false;
  }
  return true;
}

/// Closes a stroke into an outline: a ring of at most [maxVertices] corners, without the
/// first point repeated.
///
/// Where the stroke crosses itself, the part enclosing far more is kept and the rest
/// trimmed: the usual overshoot past where the finger started, or a little curl. If
/// neither part does (a figure eight), or what's left still crosses itself, it's
/// [LassoProblem.crossesItself]; if the outline encloses less than [minArea], it's
/// [LassoProblem.tooSmall].
LassoResult closeLasso(
  List<Offset> stroke, {
  double tolerance = lassoTolerance,
  int maxVertices = lassoMaxVertices,
  double minArea = lassoMinArea,
}) {
  var ring = _spaced(stroke);
  if (ring.length < 3) return (ring: null, problem: LassoProblem.tooSmall);
  // Each crossing splits the stroke in two: keep the part that encloses far more, and
  // look again. Neither does: two loops, as a figure eight has.
  for (var i = 0; i < _maxCrossings; i++) {
    final split = _largestLoop(ring);
    if (split == null) break;
    final (:loop, :trimmed) = split;
    final (inside, outside) = (ringArea(loop), ringArea(trimmed));
    if (outside <= lassoMaxTrimmedShare * inside) {
      ring = loop;
    } else if (inside <= lassoMaxTrimmedShare * outside) {
      ring = trimmed;
    } else {
      return (ring: null, problem: LassoProblem.crossesItself);
    }
  }
  // Before the area, which a ring crossing itself can cancel out.
  if (ringSelfIntersects(ring)) {
    return (ring: null, problem: LassoProblem.crossesItself);
  }
  if (ring.length < 3 || ringArea(ring) < minArea) {
    return (ring: null, problem: LassoProblem.tooSmall);
  }
  final simplified = _simplifyWithin(ring, tolerance, maxVertices);
  if (simplified == null || ringArea(simplified) < minArea) {
    return (ring: null, problem: LassoProblem.tooSmall);
  }
  return (ring: simplified, problem: null);
}

/// [ring], screen points, as a polygon on the ground, through [toGeo] (the map's
/// `screenToLngLat`). Null if a point isn't over the ground, or the ring would cross the
/// antimeridian.
GeoPolygon? lassoPolygon(
  List<Offset> ring,
  ({double lng, double lat})? Function(Offset position) toGeo,
) {
  final points = <GeoPoint>[];
  for (final p in ring) {
    final geo = toGeo(p);
    if (geo == null) return null;
    if (points.isNotEmpty && (geo.lng - points.last.lng).abs() > 180) {
      return null;
    }
    points.add(GeoPoint(geo.lat, geo.lng));
  }
  if (points.length >= 3 && (points.first.lng - points.last.lng).abs() > 180) {
    return null;
  }
  try {
    return GeoPolygon(points);
  } on ArgumentError {
    return null;
  }
}

/// The area a ring encloses, in square logical pixels (the shoelace formula), however
/// it winds.
double ringArea(List<Offset> ring) {
  var twice = 0.0;
  for (var i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    twice += ring[j].dx * ring[i].dy - ring[i].dx * ring[j].dy;
  }
  return twice.abs() / 2;
}

/// Whether any two edges of the closed [ring] that don't share a corner cross.
bool ringSelfIntersects(List<Offset> ring) {
  final n = ring.length;
  if (n < 4) return false;
  for (var i = 0; i < n; i++) {
    final a = ring[i], b = ring[(i + 1) % n];
    for (var j = i + 2; j < n; j++) {
      // The last edge shares a corner with the first.
      if (i == 0 && j == n - 1) continue;
      if (_crossing(a, b, ring[j], ring[(j + 1) % n]) != null) return true;
    }
  }
  return false;
}

/// Simplifies the closed [ring] with Douglas-Peucker at [tolerance] pixels: the corners
/// left are within [tolerance] of every point dropped.
List<Offset> simplifyRing(List<Offset> ring, double tolerance) {
  if (ring.length <= 3) return List.of(ring);
  // Split the ring at its first point and the point furthest from it, and simplify
  // each half as a line.
  var far = 1;
  var farthest = 0.0;
  for (var i = 1; i < ring.length; i++) {
    final d = (ring[i] - ring[0]).distanceSquared;
    if (d > farthest) (far, farthest) = (i, d);
  }
  final keep = List.filled(ring.length, false)
    ..[0] = true
    ..[far] = true;
  final closed = [...ring, ring[0]];
  _douglasPeucker(closed, 0, far, tolerance, keep);
  _douglasPeucker(closed, far, ring.length, tolerance, keep);
  return [
    for (var i = 0; i < ring.length; i++)
      if (keep[i]) ring[i],
  ];
}

/// Simplifies [ring] as little as [maxVertices] allows without making it cross itself,
/// or null if that can't be done.
List<Offset>? _simplifyWithin(
  List<Offset> ring,
  double tolerance,
  int maxVertices,
) {
  var t = tolerance;
  var simplified = simplifyRing(ring, t);
  while (simplified.length > maxVertices) {
    t *= 1.5;
    simplified = simplifyRing(ring, t);
  }
  // Simplifying a narrow neck can make it cross; back off while there are corners to
  // spare.
  while (ringSelfIntersects(simplified)) {
    if (simplified.length >= maxVertices || t <= 0.5) return null;
    t /= 1.5;
    final finer = simplifyRing(ring, t);
    if (finer.length > maxVertices) return null;
    simplified = finer;
  }
  return simplified.length < 3 ? null : simplified;
}

void _douglasPeucker(
  List<Offset> points,
  int first,
  int last,
  double tolerance,
  List<bool> keep,
) {
  if (last - first < 2) return;
  var index = -1;
  var furthest = tolerance;
  for (var i = first + 1; i < last; i++) {
    final d = _distanceToSegment(points[i], points[first], points[last]);
    if (d > furthest) (index, furthest) = (i, d);
  }
  if (index < 0) return;
  keep[index % keep.length] = true;
  _douglasPeucker(points, first, index, tolerance, keep);
  _douglasPeucker(points, index, last, tolerance, keep);
}

double _distanceToSegment(Offset p, Offset a, Offset b) {
  final ab = b - a;
  final length = ab.distanceSquared;
  if (length == 0) return (p - a).distance;
  final t = (((p - a).dx * ab.dx + (p - a).dy * ab.dy) / length).clamp(
    0.0,
    1.0,
  );
  return (p - (a + ab * t)).distance;
}

/// [stroke] without points closer than [lassoPointSpacing] to the one before, or to
/// the first at the end.
List<Offset> _spaced(List<Offset> stroke) {
  final points = <Offset>[];
  for (final p in stroke) {
    if (points.isEmpty || (p - points.last).distance >= lassoPointSpacing) {
      points.add(p);
    }
  }
  while (points.length > 1 &&
      (points.last - points.first).distance < lassoPointSpacing) {
    points.removeLast();
  }
  return points;
}

/// Where the open stroke [points] crosses itself, the loop between a crossing that
/// encloses the most, and what's left outside it (closed through the crossing); null
/// if it doesn't cross.
({List<Offset> loop, List<Offset> trimmed})? _largestLoop(List<Offset> points) {
  ({List<Offset> loop, List<Offset> trimmed})? best;
  var bestArea = -1.0;
  for (var i = 0; i < points.length - 1; i++) {
    for (var j = i + 2; j < points.length - 1; j++) {
      final x = _crossing(points[i], points[i + 1], points[j], points[j + 1]);
      if (x == null) continue;
      final loop = [x, ...points.sublist(i + 1, j + 1)];
      final area = ringArea(loop);
      if (area > bestArea) {
        bestArea = area;
        best = (
          loop: loop,
          trimmed: [...points.sublist(0, i + 1), x, ...points.sublist(j + 1)],
        );
      }
    }
  }
  return best;
}

/// Where segments [a]–[b] and [c]–[d] cross, if they do (touching at an end doesn't
/// count).
Offset? _crossing(Offset a, Offset b, Offset c, Offset d) {
  final r = b - a, s = d - c;
  final denominator = r.dx * s.dy - r.dy * s.dx;
  if (denominator.abs() < 1e-9) return null;
  final ac = c - a;
  final t = (ac.dx * s.dy - ac.dy * s.dx) / denominator;
  final u = (ac.dx * r.dy - ac.dy * r.dx) / denominator;
  const e = 1e-9;
  if (t <= e || t >= 1 - e || u <= e || u >= 1 - e) return null;
  return a + r * t;
}

/// The longest a lasso's stroke is kept, in points: a finger dragged for minutes
/// shouldn't slow the crossing search, which is quadratic.
const lassoMaxStrokePoints = 1200;

/// [points] thinned to at most [lassoMaxStrokePoints], keeping its ends.
List<Offset> thinStroke(List<Offset> points) {
  if (points.length <= lassoMaxStrokePoints) return points;
  final step = points.length / lassoMaxStrokePoints;
  return [
    for (var i = 0; i < lassoMaxStrokePoints - 1; i++)
      points[(i * step).floor()],
    points.last,
  ];
}
