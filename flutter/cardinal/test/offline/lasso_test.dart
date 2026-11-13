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

    test('overshooting the start is trimmed off', () {
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

    test('a little curl along the way is trimmed off', () {
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

    test('a figure eight crosses itself', () {
      // Starting at the top of one loop, as a finger would.
      final eight = [
        for (var a = math.pi / 2; a <= 2.5 * math.pi; a += 0.02)
          Offset(
            200 + 140 * math.sin(a),
            300 + 140 * math.sin(a) * math.cos(a),
          ),
      ];
      expect(closeLasso(eight).problem, LassoProblem.crossesItself);
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
