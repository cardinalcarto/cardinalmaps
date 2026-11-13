/// Turning a finger's lasso on the screen into a region's polygon: closing the stroke,
/// taking the outer boundary of what it encloses, simplifying it, and checking it encloses a
/// usable area. Pure functions, in logical pixels, so they can be tested without a map.
library;

import 'dart:math' as math;
import 'dart:ui' show Offset;

import '../downloads/geometry.dart';

/// Why a lasso can't be used.
enum LassoProblem {
  /// It encloses too little to download.
  tooSmall,

  /// Its outline still crosses itself: only if the stroke met itself exactly, as a
  /// finger all but never does.
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
/// The end of the stroke is joined back to its start, and the outline is the outer
/// boundary of everything the closed stroke encloses: an open C is closed across its
/// gap, and an overshoot, a curl or a tangle add only what they enclose beyond the rest.
/// Where that leaves pieces touching only at a point (a figure eight), the largest is
/// kept. If the outline encloses less than [minArea], it's [LassoProblem.tooSmall].
LassoResult closeLasso(
  List<Offset> stroke, {
  double tolerance = lassoTolerance,
  int maxVertices = lassoMaxVertices,
  double minArea = lassoMinArea,
}) {
  final spaced = _spaced(stroke);
  if (spaced.length < 3) return (ring: null, problem: LassoProblem.tooSmall);
  final ring = _outerBoundary(spaced);
  if (ring == null || ring.length < 3 || ringArea(ring) < minArea) {
    return (ring: null, problem: LassoProblem.tooSmall);
  }
  // The boundary is simple unless the stroke met itself exactly (three edges through
  // one point, or edges lying along each other), which a finger all but never does.
  if (ringSelfIntersects(ring)) {
    return (ring: null, problem: LassoProblem.crossesItself);
  }
  final simplified = _simplifyWithin(ring, tolerance, maxVertices);
  if (simplified == null || ringArea(simplified) < minArea) {
    return (ring: null, problem: LassoProblem.tooSmall);
  }
  return (ring: simplified, problem: null);
}

/// The outer boundary of what the closed [ring] encloses, however it crosses itself:
/// the edge of the area outside it all, traced round. Where that goes through a point
/// twice, the pieces either side meet only there, and the one enclosing the most is
/// kept. Null if it encloses nothing.
List<Offset>? _outerBoundary(List<Offset> ring) {
  final n = ring.length;
  // The corners, with any the stroke went through twice as one, then the crossings.
  final points = <Offset>[];
  final ids = <Offset, int>{};
  final corner = [
    for (final p in ring)
      ids.putIfAbsent(p, () {
        points.add(p);
        return points.length - 1;
      }),
  ];
  // Where each edge (from corner i to i + 1) is crossed, how far along it.
  final crossings = List.generate(n, (_) => <(double, int)>[]);
  for (var i = 0; i < n; i++) {
    final a = ring[i], b = ring[(i + 1) % n];
    final (left, right) = (math.min(a.dx, b.dx), math.max(a.dx, b.dx));
    final (top, bottom) = (math.min(a.dy, b.dy), math.max(a.dy, b.dy));
    for (var j = i + 2; j < n; j++) {
      // The last edge shares a corner with the first.
      if (i == 0 && j == n - 1) continue;
      final c = ring[j], d = ring[(j + 1) % n];
      if (math.max(c.dx, d.dx) < left ||
          math.min(c.dx, d.dx) > right ||
          math.max(c.dy, d.dy) < top ||
          math.min(c.dy, d.dy) > bottom) {
        continue;
      }
      final x = _crossingAlong(a, b, c, d);
      if (x == null) continue;
      final id = points.length;
      points.add(a + (b - a) * x.$1);
      crossings[i].add((x.$1, id));
      crossings[j].add((x.$2, id));
    }
  }
  // The edges, split where they're crossed.
  final next = List.generate(points.length, (_) => <int>{});
  for (var i = 0; i < n; i++) {
    final along = crossings[i]..sort((x, y) => x.$1.compareTo(y.$1));
    final chain = [
      corner[i],
      for (final (_, id) in along) id,
      corner[(i + 1) % n],
    ];
    for (var k = 1; k < chain.length; k++) {
      final (p, q) = (chain[k - 1], chain[k]);
      if (p == q) continue;
      next[p].add(q);
      next[q].add(p);
    }
  }
  // From the leftmost point, which is on the outer boundary, follow it round with the
  // outside on one hand, taking the sharpest turn that way at each point.
  var start = 0;
  for (var i = 1; i < points.length; i++) {
    final (p, s) = (points[i], points[start]);
    if (p.dx < s.dx || (p.dx == s.dx && p.dy < s.dy)) start = i;
  }
  if (next[start].isEmpty) return null;
  int turn(int from, int at, Offset heading) {
    var best = -1;
    var sharpest = double.infinity;
    for (final to in next[at]) {
      final out = points[to] - points[at];
      final angle = to == from
          ? math.pi
          : math.atan2(
              heading.dx * out.dy - heading.dy * out.dx,
              heading.dx * out.dx + heading.dy * out.dy,
            );
      if (angle < sharpest) (best, sharpest) = (to, angle);
    }
    return best;
  }

  final edges = next.fold(0, (sum, to) => sum + to.length);
  final walk = <int>[];
  final first = turn(-1, start, const Offset(1, 0));
  var (from, at) = (start, first);
  do {
    walk.add(from);
    if (walk.length > edges + 1) return null;
    final to = turn(from, at, points[at] - points[from]);
    (from, at) = (at, to);
  } while (from != start || at != first);
  // Where the walk comes back to a point, what it went round in between is a piece.
  final pieces = <List<int>>[];
  final stack = <int>[];
  final onStack = <int, int>{};
  for (final p in walk) {
    final at = onStack[p];
    if (at == null) {
      onStack[p] = stack.length;
      stack.add(p);
      continue;
    }
    pieces.add(stack.sublist(at));
    for (final q in stack.skip(at + 1)) {
      onStack.remove(q);
    }
    stack.length = at + 1;
  }
  pieces.add(stack);
  List<Offset>? largest;
  var most = 0.0;
  for (final piece in pieces) {
    final outline = [for (final p in piece) points[p]];
    final area = outline.length < 3 ? 0.0 : ringArea(outline);
    if (area > most) (largest, most) = (outline, area);
  }
  return largest;
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

/// Where segments [a]–[b] and [c]–[d] cross, if they do (touching at an end doesn't
/// count).
Offset? _crossing(Offset a, Offset b, Offset c, Offset d) {
  final along = _crossingAlong(a, b, c, d);
  return along == null ? null : a + (b - a) * along.$1;
}

/// How far along [a]–[b] and along [c]–[d] they cross, if they do (touching at an end
/// doesn't count).
(double, double)? _crossingAlong(Offset a, Offset b, Offset c, Offset d) {
  final r = b - a, s = d - c;
  final denominator = r.dx * s.dy - r.dy * s.dx;
  if (denominator.abs() < 1e-9) return null;
  final ac = c - a;
  final t = (ac.dx * s.dy - ac.dy * s.dx) / denominator;
  final u = (ac.dx * r.dy - ac.dy * r.dx) / denominator;
  const e = 1e-9;
  if (t <= e || t >= 1 - e || u <= e || u >= 1 - e) return null;
  return (t, u);
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
