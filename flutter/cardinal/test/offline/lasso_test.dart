import 'dart:math' as math;

import 'package:cardinal/offline/drawing/lasso.dart';
import 'package:flutter_test/flutter_test.dart';

/// A finger going [turns] times round a circle of [radius] about [center], a point
/// every [step] radians, with a little wobble.
List<Offset> circle({
  Offset center = const Offset(200, 300),
  double radius = 120,
  double turns = 1,
  double step = 0.02,
  double start = 0,
}) => [
  for (var a = start; a <= start + turns * 2 * math.pi; a += step)
    center + Offset(math.cos(a), math.sin(a)) * (radius + 2 * math.sin(a * 7)),
];

/// Whether [p] is inside [ring] (even-odd).
bool _inside(List<Offset> ring, Offset p) {
  var inside = false;
  for (var i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    final (a, b) = (ring[i], ring[j]);
    if ((a.dy > p.dy) != (b.dy > p.dy) &&
        p.dx < (b.dx - a.dx) * (p.dy - a.dy) / (b.dy - a.dy) + a.dx) {
      inside = !inside;
    }
  }
  return inside;
}

double _toRing(List<Offset> ring, Offset p) {
  var nearest = double.infinity;
  for (var i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    final (a, b) = (ring[j], ring[i]);
    final ab = b - a;
    final t = ab.distanceSquared == 0
        ? 0.0
        : (((p - a).dx * ab.dx + (p - a).dy * ab.dy) / ab.distanceSquared)
              .clamp(0.0, 1.0);
    nearest = math.min(nearest, (p - (a + ab * t)).distance);
  }
  return nearest;
}

/// Every point of [stroke] is inside [ring], or within a few pixels of its outline,
/// which simplifying may cut across.
void _expectAround(List<Offset> ring, List<Offset> stroke) {
  for (final p in stroke) {
    if (_inside(ring, p)) continue;
    expect(_toRing(ring, p), lessThan(8), reason: '$p is outside');
  }
}

void main() {
  group('closing a lasso', () {
    test(
      'a loop becomes an outline of modest corners, close to the stroke',
      () {
        final stroke = circle();
        final lasso = closeLasso(stroke);
        expect(lasso.problem, isNull);
        final ring = lasso.ring!;
        expect(ring.length, inInclusiveRange(8, lassoMaxVertices));
        // About the circle's area.
        expect(
          ringArea(ring),
          closeTo(math.pi * 120 * 120, 0.06 * math.pi * 120 * 120),
        );
        expect(ringSelfIntersects(ring), isFalse);
      },
    );

    test('a loop that stops short of its start is closed straight across', () {
      final stroke = circle(turns: 0.85);
      final lasso = closeLasso(stroke);
      expect(lasso.problem, isNull);
      expect(ringSelfIntersects(lasso.ring!), isFalse);
    });

    test('overshooting the start adds nothing', () {
      // A loop and a sixth, the end crossing the start.
      final stroke = circle(turns: 1.17, start: -0.4);
      final lasso = closeLasso(stroke);
      expect(lasso.problem, isNull);
      final ring = lasso.ring!;
      expect(ringSelfIntersects(ring), isFalse);
      // The overlap isn't counted twice.
      expect(
        ringArea(ring),
        closeTo(math.pi * 120 * 120, 0.06 * math.pi * 120 * 120),
      );
    });

    test('a little curl along the way adds only what it encloses', () {
      // A loop, with a small loop-de-loop a quarter of the way round.
      final stroke = circle(turns: 1.1);
      final at = stroke.length ~/ 4;
      final curl = [
        for (var a = 0.0; a <= 2 * math.pi; a += 0.3)
          stroke[at] + Offset(math.cos(a) - 1, math.sin(a)) * 12,
      ];
      stroke.insertAll(at, curl);
      final lasso = closeLasso(stroke);
      expect(lasso.problem, isNull);
      expect(ringSelfIntersects(lasso.ring!), isFalse);
      expect(
        ringArea(lasso.ring!),
        closeTo(math.pi * 120 * 120, 0.06 * math.pi * 120 * 120),
      );
    });

    test('a long, wiggly stroke keeps no more than the most corners', () {
      final wiggly = [
        for (var a = 0.0; a <= 2 * math.pi; a += 0.002)
          const Offset(200, 300) +
              Offset(math.cos(a), math.sin(a)) * (150 + 20 * math.sin(a * 40)),
      ];
      final lasso = closeLasso(thinStroke(wiggly));
      expect(lasso.problem, isNull);
      expect(lasso.ring!.length, lessThanOrEqualTo(lassoMaxVertices));
      expect(ringSelfIntersects(lasso.ring!), isFalse);
    });

    test('an open C is closed across its gap', () {
      // Three quarters of the way round.
      final stroke = circle(turns: 0.75);
      final lasso = closeLasso(stroke);
      expect(lasso.problem, isNull);
      final ring = lasso.ring!;
      expect(ringSelfIntersects(ring), isFalse);
      // The arc and the triangle the chord cuts off with the middle.
      const r = 120.0;
      expect(
        ringArea(ring),
        closeTo(0.75 * math.pi * r * r + r * r / 2, 0.05 * math.pi * r * r),
      );
      _expectAround(ring, stroke);
    });

    test('overshooting inwards, the line back to the start cutting across, '
        'keeps all it encloses', () {
      // A turn and a third, closing in from 120 to 80 pixels round: the end is inside
      // the loop, and the line back to the start crosses the stroke.
      final stroke = [
        for (var a = 0.0; a <= 1.33 * 2 * math.pi; a += 0.02)
          const Offset(200, 300) +
              Offset(math.cos(a), math.sin(a)) *
                  (120 - 40 * a / (1.33 * 2 * math.pi)),
      ];
      final lasso = closeLasso(stroke);
      expect(lasso.problem, isNull);
      final ring = lasso.ring!;
      expect(ringSelfIntersects(ring), isFalse);
      expect(
        ringArea(ring),
        inInclusiveRange(math.pi * 90 * 90, math.pi * 120 * 120),
      );
      _expectAround(ring, stroke);
    });

    test('a spiral is the outside of its outer turn', () {
      // Three turns, from 150 pixels round to 40.
      final stroke = [
        for (var a = 0.0; a <= 3 * 2 * math.pi; a += 0.02)
          const Offset(200, 300) +
              Offset(math.cos(a), math.sin(a)) *
                  (150 - 110 * a / (3 * 2 * math.pi)),
      ];
      final lasso = closeLasso(thinStroke(stroke));
      expect(lasso.problem, isNull);
      final ring = lasso.ring!;
      expect(ring.length, lessThanOrEqualTo(lassoMaxVertices));
      expect(ringSelfIntersects(ring), isFalse);
      expect(
        ringArea(ring),
        inInclusiveRange(math.pi * 113 * 113, math.pi * 150 * 150),
      );
      _expectAround(ring, stroke);
    });

    test('a tangle is the outside of all of it', () {
      // A Lissajous scribble, crossing itself over and over.
      final stroke = [
        for (var t = 0.0; t <= 2 * math.pi; t += 0.01)
          Offset(
            200 + 120 * math.sin(3 * t + 0.3),
            300 + 120 * math.sin(2 * t),
          ),
      ];
      final lasso = closeLasso(stroke);
      expect(lasso.problem, isNull);
      final ring = lasso.ring!;
      expect(ring.length, lessThanOrEqualTo(lassoMaxVertices));
      expect(ringSelfIntersects(ring), isFalse);
      // Most of the square it scribbles over.
      expect(ringArea(ring), inInclusiveRange(0.5 * 240 * 240, 240 * 240));
      _expectAround(ring, stroke);
    });

    test('a figure eight keeps its larger loop', () {
      // Starting at the top of one loop, as a finger would; the second loop smaller.
      final eight = [
        for (var a = math.pi / 2; a <= 2.5 * math.pi; a += 0.02)
          () {
            final k = math.sin(a) >= 0 ? 1.0 : 0.6;
            return Offset(
              200 + 140 * k * math.sin(a),
              300 + 140 * k * math.sin(a) * math.cos(a),
            );
          }(),
      ];
      final lasso = closeLasso(eight);
      expect(lasso.problem, isNull);
      final ring = lasso.ring!;
      expect(ringSelfIntersects(ring), isFalse);
      // All on the larger loop's side of where they cross.
      expect(ring.every((p) => p.dx >= 199), isTrue);
      final loop = [
        for (final p in eight)
          if (p.dx >= 200) p,
      ];
      expect(ringArea(ring), closeTo(ringArea(loop), 0.06 * ringArea(loop)));
    });

    test('an even figure eight keeps one loop', () {
      final eight = [
        for (var a = math.pi / 2; a <= 2.5 * math.pi; a += 0.02)
          Offset(
            200 + 140 * math.sin(a),
            300 + 140 * math.sin(a) * math.cos(a),
          ),
      ];
      final lasso = closeLasso(eight);
      expect(lasso.problem, isNull);
      final ring = lasso.ring!;
      expect(ringSelfIntersects(ring), isFalse);
      expect(
        ring.every((p) => p.dx >= 199) || ring.every((p) => p.dx <= 201),
        isTrue,
      );
    });

    test('a small loop is too small', () {
      expect(closeLasso(circle(radius: 20)).problem, LassoProblem.tooSmall);
    });

    test('a line, out and back, is too small', () {
      final line = [
        for (var x = 0.0; x <= 200; x += 4) Offset(x, 100),
        for (var x = 200.0; x >= 0; x -= 4) Offset(x, 102),
      ];
      expect(closeLasso(line).problem, isNot(isNull));
    });

    test('a tap is just a tap', () {
      expect(isLassoTap(const [Offset(10, 10), Offset(12, 11)]), isTrue);
      expect(isLassoTap(circle()), isFalse);
    });
  });

  group('the outline on the ground', () {
    test('goes through the map, point by point', () {
      final ring = closeLasso(circle()).ring!;
      final polygon = lassoPolygon(
        ring,
        (p) => (lng: -122.3 + p.dx / 10000, lat: 47.6 - p.dy / 10000),
      )!;
      expect(polygon.points, hasLength(ring.length));
      expect(
        polygon.points.first.lng,
        closeTo(-122.3 + ring.first.dx / 10000, 1e-12),
      );
      expect(
        polygon.points.first.lat,
        closeTo(47.6 - ring.first.dy / 10000, 1e-12),
      );
      expect(polygon.bounds.west, closeTo(-122.3 + 80 / 10000, 3e-4));
    });

    test('is none if a point is off the ground', () {
      final ring = closeLasso(circle()).ring!;
      expect(
        lassoPolygon(ring, (p) => p.dy < 250 ? null : (lng: p.dx, lat: p.dy)),
        isNull,
      );
    });

    test('is none if it would cross the antimeridian', () {
      final ring = closeLasso(circle()).ring!;
      expect(
        lassoPolygon(ring, (p) => (lng: p.dx < 200 ? 179.9 : -179.9, lat: 0)),
        isNull,
      );
    });
  });

  test('a ring crossing itself is told from one that does not', () {
    const bowtie = [Offset(0, 0), Offset(10, 10), Offset(10, 0), Offset(0, 10)];
    const square = [Offset(0, 0), Offset(10, 0), Offset(10, 10), Offset(0, 10)];
    expect(ringSelfIntersects(bowtie), isTrue);
    expect(ringSelfIntersects(square), isFalse);
    expect(ringArea(square), 100);
  });
}
