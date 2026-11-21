import 'package:cardinal/directions/route_tones.dart';
import 'package:cardinal/directions/speed_profile.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_routes.dart';

const _plain = Color(0xff000000);

ProfileStep _step(double left, double right, double top) =>
    (left: left, right: right, top: top, tone: _plain);

/// How far apart [a] and [b] are in contrast, as WCAG measures it.
double _contrast(Color a, Color b) {
  final (x, y) = (a.computeLuminance(), b.computeLuminance());
  return (x > y ? x + 0.05 : y + 0.05) / (x > y ? y + 0.05 : x + 0.05);
}

const _tones = ProfileTones(
  best: Color(0xff102030),
  middle: Color(0xff405060),
  worst: Color(0xff8090a0),
  ferry: Color(0x66808080),
  ground: Color(0x38808080),
);

void main() {
  group('the tones', () {
    for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
      double lightness(Color color) => HSLColor.fromColor(color).lightness;

      test('go dark to light from the best way to the worst on a light '
          'sheet, on ${platform.name}', () {
        final scheme = appTheme(
          Brightness.light,
          platform: platform,
        ).colorScheme;
        final tones = profileTones(scheme);
        expect(
          lightness(tones.middle) - lightness(tones.best),
          greaterThan(0.15),
        );
        expect(
          lightness(tones.worst) - lightness(tones.middle),
          greaterThan(0.15),
        );
        for (final tone in [tones.best, tones.middle, tones.worst]) {
          expect(
            HSLColor.fromColor(tone).hue,
            moreOrLessEquals(
              HSLColor.fromColor(scheme.primary).hue,
              epsilon: 2,
            ),
          );
          expect(tone.a, 1);
        }
      });

      test('and light to dark on a dark one, the best the lightest, on '
          '${platform.name}', () {
        final scheme = appTheme(
          Brightness.dark,
          platform: platform,
        ).colorScheme;
        final tones = profileTones(scheme);
        expect(
          lightness(tones.best) - lightness(tones.middle),
          greaterThan(0.15),
        );
        expect(
          lightness(tones.middle) - lightness(tones.worst),
          greaterThan(0.15),
        );
      });

      for (final brightness in Brightness.values) {
        test('"Toll" reads on each of them, ${brightness.name} on '
            '${platform.name}', () {
          final tones = profileTones(
            appTheme(brightness, platform: platform).colorScheme,
          );
          for (final tone in [tones.best, tones.middle, tones.worst]) {
            expect(
              _contrast(labelColorOn(tone), tone),
              greaterThanOrEqualTo(4.5),
              reason: '$tone',
            );
          }
        });
      }
    }

    test('a run takes its kind of way\'s tone, and a ferry the quiet one', () {
      Color of(DirectionsRunKind? kind, {bool ferry = false}) =>
          _tones.of(fakeRun(1, 1, kind: kind, ferry: ferry), detailed: true);
      expect(of(DirectionsRunKind.freeway), _tones.best);
      expect(of(DirectionsRunKind.mainRoad), _tones.middle);
      expect(of(DirectionsRunKind.street), _tones.worst);
      expect(of(DirectionsRunKind.trail), _tones.best);
      expect(of(DirectionsRunKind.bikeLane), _tones.middle);
      expect(of(DirectionsRunKind.sharedRoad), _tones.worst);
      expect(of(DirectionsRunKind.path), _tones.best);
      expect(of(DirectionsRunKind.sidewalk), _tones.middle);
      expect(of(DirectionsRunKind.stairs), _tones.worst);
      expect(of(null, ferry: true), _tones.ferry);
    });

    test(
      'without the kinds, a freeway or not, and a ferry is no different',
      () {
        Color of({bool motorway = false, bool ferry = false}) => _tones.of(
          fakeRun(1, 1, motorway: motorway, ferry: ferry),
          detailed: false,
        );
        expect(of(motorway: true), _tones.best);
        expect(of(), _tones.worst);
        expect(of(ferry: true), _tones.worst);
      },
    );

    test('the kinds of a route say its mode', () {
      DirectionsRoute route(List<DirectionsRun> runs) => fakeRoute(const [
        [0, 0],
        [1, 1],
      ], runs: runs);
      expect(
        WayKinds.of(
          route([
            fakeRun(1, 1, ferry: true),
            fakeRun(1, 1, kind: DirectionsRunKind.mainRoad),
          ]),
        ),
        WayKinds.driving,
      );
      expect(
        WayKinds.of(route([fakeRun(1, 1, kind: DirectionsRunKind.sharedRoad)])),
        WayKinds.cycling,
      );
      expect(
        WayKinds.of(route([fakeRun(1, 1, kind: DirectionsRunKind.stairs)])),
        WayKinds.walking,
      );
      expect(WayKinds.of(route([fakeRun(1, 1)])), isNull);
      expect(
        WayKinds.ofAll([
          route([]),
          route([fakeRun(1, 1)]),
        ]),
        isNull,
      );
    });
  });

  group('the outline', () {
    const size = Size(100, 32);

    test('is as wide as its box and as tall as its tallest step', () {
      final path = SpeedProfilePainter.outline([
        _step(0, 40, 20),
        _step(40, 100, 4),
      ], size);
      expect(path.getBounds(), const Rect.fromLTRB(0, 4, 100, 32));
    });

    test('rounds its outer corners at the top, and squares the bottom', () {
      final path = SpeedProfilePainter.outline([_step(0, 100, 10)], size);
      expect(path.contains(const Offset(0.3, 10.3)), isFalse);
      expect(path.contains(const Offset(99.7, 10.3)), isFalse);
      expect(path.contains(const Offset(3, 13)), isTrue);
      expect(path.contains(const Offset(0.3, 31.7)), isTrue);
      expect(path.contains(const Offset(99.7, 31.7)), isTrue);
    });

    test('fillets each step: rounded off above, filled in below', () {
      // Down 16 at x = 50.
      final path = SpeedProfilePainter.outline([
        _step(0, 50, 4),
        _step(50, 100, 20),
      ], size);
      // The corner of the taller step is cut round.
      expect(path.contains(const Offset(49.7, 4.3)), isFalse);
      expect(path.contains(const Offset(47, 7)), isTrue);
      // And the foot of the step is filled in, where a square one would be empty.
      expect(path.contains(const Offset(50.4, 19.6)), isTrue);
      expect(path.contains(const Offset(53, 17)), isFalse);
    });

    test('by less where a step is smaller than the fillet', () {
      // Down 2: a radius of 1 on each side.
      final path = SpeedProfilePainter.outline([
        _step(0, 50, 10),
        _step(50, 100, 12),
      ], size);
      // Within 3 pixels of the step, but outside its small fillets, it's square.
      expect(path.contains(const Offset(48, 10.2)), isTrue);
      expect(path.contains(const Offset(52, 11.8)), isFalse);
      expect(path.contains(const Offset(52, 12.2)), isTrue);
    });

    test('and where a run is narrower than it', () {
      // A run 2 wide between two others: its fillets are 1 at most.
      final path = SpeedProfilePainter.outline([
        _step(0, 49, 20),
        _step(49, 51, 4),
        _step(51, 100, 20),
      ], size);
      expect(path.getBounds(), const Rect.fromLTRB(0, 4, 100, 32));
      expect(path.contains(const Offset(50, 4.2)), isTrue);
      expect(path.contains(const Offset(47, 19.8)), isFalse);
    });
  });

  group('the painter', () {
    final runs = [fakeRun(3, 8), fakeRun(15, 27, motorway: true)];

    test('paints again only for other runs, another scale or other tones', () {
      final painter = SpeedProfilePainter(
        runs: runs,
        topSpeed: 27,
        tones: _tones,
      );
      SpeedProfilePainter other({
        List<DirectionsRun>? runs_,
        double topSpeed = 27,
        ProfileTones tones_ = _tones,
      }) => SpeedProfilePainter(
        runs: runs_ ?? runs,
        topSpeed: topSpeed,
        tones: tones_,
      );
      expect(other().shouldRepaint(painter), isFalse);
      expect(other(runs_: [fakeRun(1, 1)]).shouldRepaint(painter), isTrue);
      expect(other(topSpeed: 30).shouldRepaint(painter), isTrue);
      expect(
        other(
          tones_: ProfileTones(
            best: _tones.worst,
            middle: _tones.middle,
            worst: _tones.best,
            ferry: _tones.ferry,
            ground: _tones.ground,
          ),
        ).shouldRepaint(painter),
        isTrue,
      );
    });

    testWidgets('without kinds of road, fills freeways in one tone and the '
        'rest in another, each run as wide as its time and as tall as its '
        'speed', (tester) async {
      await tester.pumpWidget(
        Center(
          child: SizedBox(
            width: 180,
            child: SpeedProfile(runs: runs, topSpeed: 27, tones: _tones),
          ),
        ),
      );
      final box = find.byType(SpeedProfile);
      expect(tester.getSize(box), const Size(180, routeGraphHeight));
      expect(
        box,
        paints
          ..clipPath(
            pathMatcher: isPathThat(
              // 3 of 18 minutes at 8 of 27: 30 wide, about 9.5 tall. Then the
              // freeway, full height.
              includes: const [Offset(15, 28), Offset(100, 3), Offset(170, 16)],
              excludes: const [Offset(15, 18), Offset(15, 3)],
            ),
          )
          ..rect(rect: const Rect.fromLTRB(0, 0, 30, 32), color: _tones.worst)
          ..rect(rect: const Rect.fromLTRB(30, 0, 180, 32), color: _tones.best),
      );
    });

    testWidgets('with them, fills each run in its kind\'s tone of three', (
      tester,
    ) async {
      await tester.pumpWidget(
        Center(
          child: SizedBox(
            width: 220,
            child: SpeedProfile(
              runs: [
                fakeRun(3, 8, kind: DirectionsRunKind.street),
                fakeRun(
                  15,
                  27,
                  motorway: true,
                  kind: DirectionsRunKind.freeway,
                ),
                fakeRun(4, 12, kind: DirectionsRunKind.mainRoad),
              ],
              topSpeed: 27,
              tones: _tones,
            ),
          ),
        ),
      );
      expect(
        find.byType(SpeedProfile),
        paints
          ..clipPath()
          ..rect(rect: const Rect.fromLTRB(0, 0, 30, 32), color: _tones.worst)
          ..rect(rect: const Rect.fromLTRB(30, 0, 180, 32), color: _tones.best)
          ..rect(
            rect: const Rect.fromLTRB(180, 0, 220, 32),
            color: _tones.middle,
          ),
      );
    });

    testWidgets('a ferry, with them, in the quiet tone', (tester) async {
      await tester.pumpWidget(
        Center(
          child: SizedBox(
            width: 150,
            child: SpeedProfile(
              runs: [
                fakeRun(5, 10, kind: DirectionsRunKind.street),
                fakeRun(5, 6, ferry: true),
                fakeRun(5, 10, kind: DirectionsRunKind.street),
              ],
              topSpeed: 10,
              tones: _tones,
            ),
          ),
        ),
      );
      expect(
        find.byType(SpeedProfile),
        paints
          ..clipPath()
          ..rect(rect: const Rect.fromLTRB(0, 0, 50, 32), color: _tones.worst)
          ..rect(rect: const Rect.fromLTRB(50, 0, 100, 32), color: _tones.ferry)
          ..rect(
            rect: const Rect.fromLTRB(100, 0, 150, 32),
            color: _tones.worst,
          ),
      );
    });

    testWidgets('with no runs draws a plain bar, the width it\'s given', (
      tester,
    ) async {
      await tester.pumpWidget(
        const Center(
          child: SizedBox(
            width: 120,
            child: SpeedProfile(runs: [], topSpeed: 0, tones: _tones),
          ),
        ),
      );
      expect(
        find.byType(SpeedProfile),
        paints..rrect(
          rrect: RRect.fromLTRBR(
            0,
            routeGraphHeight - routeBarHeight,
            120,
            routeGraphHeight,
            const Radius.circular(routeBarHeight / 2),
          ),
          color: _tones.worst,
        ),
      );
    });
  });

  group('"Toll" on the profile', () {
    const label = Size(30, 18);
    const size = Size(250, routeGraphHeight);
    // 25 minutes in 250: 10 to the minute.
    List<DirectionsRun> tolled({
      bool kinds = true,
      double speed = 24,
      bool ferry = false,
    }) => [
      fakeRun(5, 10, kind: kinds ? DirectionsRunKind.mainRoad : null),
      fakeRun(
        10,
        speed,
        motorway: true,
        toll: true,
        ferry: ferry,
        kind: kinds && !ferry ? DirectionsRunKind.freeway : null,
      ),
      fakeRun(10, 11, kind: kinds ? DirectionsRunKind.street : null),
    ];
    List<TollLabel> labels(List<DirectionsRun> runs, {Size at = size}) =>
        tollLabels(runs, topSpeed: 27, size: at, label: label);

    test('goes on a tolled stretch that holds it, padded, below the '
        'stretch\'s top', () {
      final placed = labels(tolled()).single;
      // From 50 to 150, its top 24 of 27 up.
      const top = routeGraphHeight - routeGraphHeight * 24 / 27;
      expect(placed.rect.left, 50 + tollLabelPadding.left);
      expect(placed.rect.right, 150 - tollLabelPadding.right);
      expect(placed.rect.top, moreOrLessEquals(top + tollLabelPadding.top));
      expect(placed.rect.bottom, routeGraphHeight - tollLabelPadding.bottom);
      expect(placed.run.kind, DirectionsRunKind.freeway);
    });

    test('not on one too narrow for it with its padding', () {
      // 10 minutes in 37.6 less 8 of padding: 29.6.
      expect(labels(tolled(), at: const Size(94, routeGraphHeight)), isEmpty);
      // And 30.
      expect(
        labels(tolled(), at: const Size(95, routeGraphHeight)),
        hasLength(1),
      );
    });

    test('nor on one too low', () {
      // 18 of 27 stands about 21: less than 18 and 2 of padding.
      expect(labels(tolled(speed: 15)), isEmpty);
      expect(labels(tolled(speed: 18)), hasLength(1));
    });

    test('nor without the kinds of road, which the toll could be longer '
        'than', () {
      expect(labels(tolled(kinds: false)), isEmpty);
    });

    test('nor on a ferry', () {
      expect(labels(tolled(ferry: true)), isEmpty);
    });

    test('on each tolled stretch, and once on tolled runs one after another, '
        'below the lowest of them', () {
      final placed = labels([
        fakeRun(5, 24, toll: true, kind: DirectionsRunKind.freeway),
        fakeRun(5, 20, toll: true, kind: DirectionsRunKind.mainRoad),
        fakeRun(5, 10, kind: DirectionsRunKind.street),
        fakeRun(10, 24, toll: true, kind: DirectionsRunKind.freeway),
      ]);
      expect(placed, hasLength(2));
      expect(placed[0].rect.left, tollLabelPadding.left);
      expect(placed[0].rect.right, 100 - tollLabelPadding.right);
      expect(
        placed[0].rect.top,
        moreOrLessEquals(
          routeGraphHeight - routeGraphHeight * 20 / 27 + tollLabelPadding.top,
        ),
      );
      expect(placed[1].rect.left, 150 + tollLabelPadding.left);
    });

    testWidgets('is written where it goes, legibly on its tone, scaled no '
        'further than 1.3×', (tester) async {
      final tones = profileTones(appTheme(Brightness.light).colorScheme);
      final runs = tolled();
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await tester.pumpWidget(
        MaterialApp(
          theme: appTheme(Brightness.light),
          home: Center(
            child: SizedBox(
              width: size.width,
              child: SpeedProfile(
                runs: runs,
                topSpeed: 27,
                tones: tones,
                tollLabels: labels(runs),
              ),
            ),
          ),
        ),
      );
      final text = find.text(tollLabel);
      expect(text, findsOneWidget);
      final profile = tester.getRect(find.byType(SpeedProfile));
      final rect = tester.getRect(text);
      expect(rect.center.dx - profile.left, moreOrLessEquals(100));
      expect(rect.top - profile.top, greaterThan(4));
      expect(rect.bottom, lessThanOrEqualTo(profile.bottom));
      final written = tester.widget<Text>(text);
      expect(written.style!.color, labelColorOn(tones.best));
      expect(written.textScaler!.scale(10), 13);
      expect(tester.takeException(), isNull);
    });
  });
}
