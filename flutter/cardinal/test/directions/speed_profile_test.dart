import 'package:cardinal/directions/speed_profile.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_routes.dart';

typedef _Step = ({double left, double right, double top, bool motorway});

_Step _step(double left, double right, double top) =>
    (left: left, right: right, top: top, motorway: false);

void main() {
  group('the tones', () {
    for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
      test(
        'the freeway is the darker on a light sheet, on ${platform.name}',
        () {
          final scheme = appTheme(
            Brightness.light,
            platform: platform,
          ).colorScheme;
          final tones = profileTones(scheme);
          final motorway = HSLColor.fromColor(tones.motorway);
          final other = HSLColor.fromColor(tones.other);
          expect(other.lightness - motorway.lightness, greaterThan(0.25));
          expect(
            motorway.hue,
            moreOrLessEquals(
              HSLColor.fromColor(scheme.primary).hue,
              epsilon: 2,
            ),
          );
          expect(tones.motorway.a, 1);
          expect(tones.other.a, 1);
        },
      );

      test('and the lighter on a dark one, on ${platform.name}', () {
        final scheme = appTheme(
          Brightness.dark,
          platform: platform,
        ).colorScheme;
        final tones = profileTones(scheme);
        expect(
          HSLColor.fromColor(tones.motorway).lightness -
              HSLColor.fromColor(tones.other).lightness,
          greaterThan(0.25),
        );
      });
    }
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
    const tones = (motorway: Color(0xff102030), other: Color(0xff8090a0));

    test('paints again only for other runs, another scale or other tones', () {
      final painter = SpeedProfilePainter(
        runs: runs,
        topSpeed: 27,
        tones: tones,
      );
      SpeedProfilePainter other({
        List<DirectionsRun>? runs_,
        double topSpeed = 27,
        ProfileTones tones_ = tones,
      }) => SpeedProfilePainter(
        runs: runs_ ?? runs,
        topSpeed: topSpeed,
        tones: tones_,
      );
      expect(other().shouldRepaint(painter), isFalse);
      expect(other(runs_: [fakeRun(1, 1)]).shouldRepaint(painter), isTrue);
      expect(other(topSpeed: 30).shouldRepaint(painter), isTrue);
      expect(
        other(tones_: (motorway: tones.other, other: tones.motorway))
            .shouldRepaint(painter),
        isTrue,
      );
    });

    testWidgets('fills freeways in one tone and the rest in the other, each '
        'run as wide as its time and as tall as its speed', (tester) async {
      await tester.pumpWidget(
        Center(
          child: SizedBox(
            width: 180,
            child: SpeedProfile(runs: runs, topSpeed: 27, tones: tones),
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
          ..rect(rect: const Rect.fromLTRB(0, 0, 30, 32), color: tones.other)
          ..rect(
            rect: const Rect.fromLTRB(30, 0, 180, 32),
            color: tones.motorway,
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
            child: SpeedProfile(runs: [], topSpeed: 0, tones: tones),
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
          color: tones.other,
        ),
      );
    });
  });
}
