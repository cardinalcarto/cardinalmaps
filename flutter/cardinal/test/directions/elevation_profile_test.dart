import 'package:cardinal/directions/elevation_profile.dart';
import 'package:cardinal/directions/route_tones.dart';
import 'package:cardinal/directions/speed_profile.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_routes.dart';

const _tones = ProfileTones(
  best: Color(0xff102030),
  middle: Color(0xff405060),
  worst: Color(0xff8090a0),
  ferry: Color(0x66808080),
  ground: Color(0x38808080),
);

const _line = [
  [0.0, 0.0],
  [1.0, 1.0],
];

void main() {
  group('the scale', () {
    test('is the lowest to the highest of every route of the list', () {
      final range = elevationRangeOf([
        fakeRoute(_line, elevation: fakeElevation([20, 140, 30])),
        fakeRoute(_line),
        fakeRoute(_line, elevation: fakeElevation([10, 60, 25])),
      ]);
      expect(range, (low: 10, high: 140));
    });

    test('spans at least the floor, so a flat route looks flat', () {
      final range = elevationRangeOf([
        fakeRoute(_line, elevation: fakeElevation([20, 24, 21])),
      ]);
      expect(range, (low: 20, high: 20 + elevationFloor));
    });

    test('is nothing with no elevation', () {
      expect(elevationRangeOf([fakeRoute(_line)]), isNull);
    });
  });

  group('along the time', () {
    test('a route goes as far as its runs at each time', () {
      // Half the time crawls a tenth of the way, the rest goes the other nine.
      final shares = ElevationProfilePainter.distanceShares([
        fakeRun(10, 1),
        fakeRun(10, 9),
      ], 4);
      expect(shares[0], 0);
      expect(shares[1], moreOrLessEquals(0.05));
      expect(shares[2], moreOrLessEquals(0.1));
      expect(shares[3], moreOrLessEquals(0.55));
      expect(shares[4], moreOrLessEquals(1));
    });

    test('or by distance if its runs don\'t say', () {
      expect(ElevationProfilePainter.distanceShares(const [], 4), [
        0,
        0.25,
        0.5,
        0.75,
        1,
      ]);
    });

    test('heights are read between their samples, the last at the route\'s '
        'end', () {
      // Every 100 m, the last 50 m past the one before.
      final elevation = fakeElevation([0, 100, 50], interval: 100);
      final samples = ElevationProfilePainter.resample(elevation, 150, [
        0,
        0.5,
        2 / 3,
        1,
      ]);
      expect(samples[0], 0);
      expect(samples[1], moreOrLessEquals(75, epsilon: 0.01));
      expect(samples[2], moreOrLessEquals(100, epsilon: 0.01));
      expect(samples[3], moreOrLessEquals(50, epsilon: 0.01));
    });

    test('each the highest of the samples halfway to its neighbors', () {
      final elevation = fakeElevation([0, 100, 50], interval: 100);
      // 90 m reaches from 45 m to 120 m, over the peak at 100 m.
      final samples = ElevationProfilePainter.resample(elevation, 150, [
        0,
        0.6,
        1,
      ]);
      expect(samples[1], 100);
    });

    test('a peak narrower than the points still shows', () {
      final heights = [for (var i = 0; i < 101; i++) i == 37 ? 80.0 : 0.0];
      final samples = ElevationProfilePainter.resample(
        fakeElevation(heights, interval: 10),
        1000,
        [for (var i = 0; i <= 10; i++) i / 10],
      );
      expect(samples.reduce((a, b) => a > b ? a : b), 80);
    });
  });

  group('the silhouette', () {
    const band = ElevationProfilePainter.bandHeight;
    const stand = ElevationProfilePainter.stand;

    test('stands its lowest a little off the bottom, and its highest at '
        'the top', () {
      final path = ElevationProfilePainter.silhouette(
        [0, 100, 50],
        (low: 0, high: 100),
        200,
      );
      expect(path.getBounds(), const Rect.fromLTRB(0, 0, 200, band));
      expect(path.contains(const Offset(1, band - stand / 2)), isTrue);
      expect(path.contains(const Offset(1, band - stand - 1)), isFalse);
      expect(path.contains(const Offset(100, 1)), isTrue);
    });

    test('to the list\'s scale', () {
      final path = ElevationProfilePainter.silhouette(
        [0, 0],
        (low: 0, high: 100),
        200,
      );
      // At the bottom of a scale another route tops.
      expect(path.getBounds().top, band - stand);
    });
  });

  group('the bar', () {
    test('is a stretch for each run of a kind of way, a seam between', () {
      final stretches = ElevationProfilePainter.barStretches(
        [
          fakeRun(3, 5, kind: DirectionsRunKind.trail),
          fakeRun(3, 5, kind: DirectionsRunKind.trail),
          fakeRun(2, 5, kind: DirectionsRunKind.bikeLane),
          fakeRun(2, 5, kind: DirectionsRunKind.sharedRoad),
        ],
        100,
        _tones,
      );
      expect(stretches, [
        (left: 0, right: 59.5, tone: _tones.best),
        (left: 60.5, right: 79.5, tone: _tones.middle),
        (left: 80.5, right: 100, tone: _tones.worst),
      ]);
    });

    test('and for a walk, stairs the third', () {
      final stretches = ElevationProfilePainter.barStretches(
        [
          fakeRun(1, 1, kind: DirectionsRunKind.path),
          fakeRun(1, 1, kind: DirectionsRunKind.sidewalk),
          fakeRun(1, 1, kind: DirectionsRunKind.stairs),
        ],
        90,
        _tones,
      );
      expect(
        [for (final s in stretches) s.tone],
        [_tones.best, _tones.middle, _tones.worst],
      );
    });

    test('is plain without the kinds', () {
      expect(
        ElevationProfilePainter.barStretches([fakeRun(1, 1)], 80, _tones),
        [(left: 0, right: 80, tone: _tones.worst)],
      );
      expect(ElevationProfilePainter.barStretches(const [], 80, _tones), [
        (left: 0, right: 80, tone: _tones.worst),
      ]);
    });
  });

  group('the painter', () {
    final runs = [
      fakeRun(5, 5, kind: DirectionsRunKind.trail),
      fakeRun(5, 5, kind: DirectionsRunKind.sharedRoad),
    ];
    final elevation = fakeElevation([10, 60, 10], interval: 1500);

    Widget profile({DirectionsElevation? elevation, List<DirectionsRun>? of}) =>
        Center(
          child: SizedBox(
            width: 200,
            child: ElevationProfile(
              runs: of ?? runs,
              elevation: elevation,
              distance: 3000,
              range: elevation == null ? null : (low: 10, high: 60),
              tones: _tones,
            ),
          ),
        );

    testWidgets('draws the ground over the bar, in the graph\'s height', (
      tester,
    ) async {
      await tester.pumpWidget(profile(elevation: elevation));
      final box = find.byType(ElevationProfile);
      expect(tester.getSize(box), const Size(200, routeGraphHeight));
      const bar = routeGraphHeight - routeBarHeight;
      expect(
        box,
        paints
          ..path(
            color: _tones.ground,
            includes: const [Offset(100, 1)],
            excludes: const [Offset(10, 10), Offset(100, bar - 1)],
          )
          ..clipRRect(
            rrect: RRect.fromLTRBR(
              0,
              bar,
              200,
              routeGraphHeight,
              const Radius.circular(routeBarHeight / 2),
            ),
          )
          ..rect(
            rect: const Rect.fromLTRB(0, bar, 99.5, routeGraphHeight),
            color: _tones.best,
          )
          ..rect(
            rect: const Rect.fromLTRB(100.5, bar, 200, routeGraphHeight),
            color: _tones.worst,
          ),
      );
    });

    testWidgets('with no elevation, leaves the space above the bar empty', (
      tester,
    ) async {
      await tester.pumpWidget(profile());
      expect(find.byType(ElevationProfile), isNot(paints..path()));
      expect(find.byType(ElevationProfile), paints..clipRRect());
    });

    testWidgets('with no kinds, draws the plain bar', (tester) async {
      await tester.pumpWidget(
        profile(elevation: elevation, of: [fakeRun(10, 5)]),
      );
      expect(
        find.byType(ElevationProfile),
        paints
          ..path(color: _tones.ground)
          ..clipRRect()
          ..rect(
            rect: const Rect.fromLTRB(
              0,
              routeGraphHeight - routeBarHeight,
              200,
              routeGraphHeight,
            ),
            color: _tones.worst,
          ),
      );
    });

    test('paints again only for another route, scale or tones', () {
      ElevationProfilePainter painter({
        DirectionsElevation? of,
        ElevationRange range = (low: 10, high: 60),
      }) => ElevationProfilePainter(
        runs: runs,
        elevation: of ?? elevation,
        distance: 3000,
        range: range,
        tones: _tones,
      );
      expect(painter().shouldRepaint(painter()), isFalse);
      expect(
        painter(of: fakeElevation([1, 2])).shouldRepaint(painter()),
        isTrue,
      );
      expect(
        painter(range: (low: 0, high: 60)).shouldRepaint(painter()),
        isTrue,
      );
    });
  });
}
