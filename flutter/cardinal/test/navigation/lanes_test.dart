import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/navigation/lanes.dart';
import 'package:cardinal/navigation/maneuver_banner.dart';
import 'package:cardinal/navigation/maneuver_glyphs.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_navigation.dart';

/// I-5's exit to Union Street, as the router gives it: five through lanes, and the
/// exit lane on the right.
final exitLanes = [
  for (var i = 0; i < 5; i++) fakeLane(['straight']),
  fakeLane(['slight right'], active: true, activeDirection: 'slight right'),
];

final offRamp = fakeContent(
  'Union Street',
  type: ManeuverType.offRamp,
  modifier: ManeuverModifier.slightRight,
  exitNumbers: ['165B'],
);

void main() {
  Future<void> show(
    WidgetTester tester,
    NavVisualInstruction instruction, {
    NavDrivingSide? drivingSide,
    List<NavStep> upcoming = const [],
  }) => tester.pumpWidget(
    MaterialApp(
      theme: appTheme(Brightness.light, platform: TargetPlatform.android),
      localizationsDelegates: AppLocalizations.localizationsDelegates,
      home: Scaffold(
        body: Align(
          alignment: Alignment.topCenter,
          child: ManeuverBanner(
            instruction: instruction,
            distance: 400,
            units: UnitSystem.imperial,
            drivingSide: drivingSide,
            upcoming: upcoming,
            expanded: ValueNotifier(upcoming.isNotEmpty),
          ),
        ),
      ),
    ),
  );

  IconData glyph(String name) => maneuverGlyphs[name]!;

  /// Each lane's icon, left to right.
  List<Icon> laneIcons(WidgetTester tester) => tester
      .widgetList<Icon>(
        find.descendant(
          of: find.byType(LaneStrip),
          matching: find.byType(Icon),
        ),
      )
      .toList();

  group('a lane is drawn with', () {
    for (final (lane, name) in [
      (fakeLane(['straight']), 'turn_straight'),
      (fakeLane(['none']), 'turn_straight'),
      (fakeLane([]), 'turn_straight'),
      (fakeLane(['left']), 'turn_left'),
      (fakeLane(['slight right']), 'turn_slight_right'),
      (fakeLane(['sharp left']), 'turn_sharp_left'),
      (fakeLane(['uturn']), 'turn_uturn'),
      (fakeLane(['merge to left']), 'turn_straight'),
      // An inactive lane's first arrow.
      (fakeLane(['left', 'straight']), 'turn_left'),
      // An active lane's arrow to take, whatever else it has.
      (
        fakeLane(
          ['straight', 'slight right'],
          active: true,
          activeDirection: 'slight right',
        ),
        'turn_slight_right',
      ),
      // Or its first, if it doesn't say which.
      (fakeLane(['right', 'straight'], active: true), 'turn_right'),
    ]) {
      test('$name for ${lane.directions} ${lane.active ? 'active' : ''}', () {
        final drawn = laneGlyph(lane, NavDrivingSide.right);
        expect(drawn.icon, glyph(name));
        expect(drawn.mirrored, isFalse);
      });
    }
  });

  test('only a U-turn lane is mirrored where traffic keeps left', () {
    final uturn = fakeLane(['uturn'], active: true, activeDirection: 'uturn');
    expect(laneGlyph(uturn, NavDrivingSide.left).mirrored, isTrue);
    expect(laneGlyph(uturn, NavDrivingSide.right).mirrored, isFalse);
    expect(laneGlyph(uturn, null).mirrored, isFalse);
    for (final direction in ['left', 'slight right', 'straight']) {
      final lane = fakeLane([direction], active: true);
      final drawn = laneGlyph(lane, NavDrivingSide.left);
      expect(drawn.mirrored, isFalse, reason: direction);
      expect(drawn.icon, laneGlyph(lane, NavDrivingSide.right).icon);
    }
  });

  testWidgets('the lanes show under the maneuver, the active ones emphasized', (
    tester,
  ) async {
    await show(tester, fakeVisual(offRamp, sub: fakeLanes(exitLanes)));
    final icons = laneIcons(tester);
    expect(icons.map((i) => i.icon), [
      for (var i = 0; i < 5; i++) glyph('turn_straight'),
      glyph('turn_slight_right'),
    ]);
    final ink = Theme.of(tester.element(find.byType(LaneStrip))).colorScheme;
    expect(icons.last.color, ink.onSurface);
    for (final icon in icons.take(5)) {
      expect(icon.color, ink.onSurface.withValues(alpha: 0.3));
    }
    // Left to right, under the instruction.
    final lanes = find.descendant(
      of: find.byType(LaneStrip),
      matching: find.byType(Icon),
    );
    expect(
      tester.getCenter(lanes.first).dx,
      lessThan(tester.getCenter(lanes.last).dx),
    );
    expect(
      tester.getTopLeft(lanes.first).dy,
      greaterThan(tester.getBottomLeft(find.textContaining('Union')).dy),
    );
    expect(
      find.bySemanticsLabel('1 lane of 6 for this maneuver'),
      findsOneWidget,
    );
  });

  testWidgets('the lanes go between the maneuver and the one after it', (
    tester,
  ) async {
    await show(
      tester,
      fakeVisual(
        offRamp,
        secondary: fakeContent('Union Street'),
        sub: fakeLanes(exitLanes),
      ),
    );
    final strip = tester.getRect(find.byType(LaneStrip));
    expect(
      strip.bottom,
      lessThan(tester.getTopLeft(find.text('Union Street')).dy),
    );
  });

  testWidgets('no lanes, no strip', (tester) async {
    await show(tester, fakeVisual(offRamp));
    expect(find.byType(LaneStrip), findsNothing);
    // Nor for a sub line without lanes.
    await show(tester, fakeVisual(offRamp, sub: fakeContent('then left')));
    expect(find.byType(LaneStrip), findsNothing);
  });

  testWidgets('a U-turn lane turns right where traffic keeps left', (
    tester,
  ) async {
    final lanes = fakeLanes([
      fakeLane(['uturn'], active: true, activeDirection: 'uturn'),
      fakeLane(['straight']),
    ]);
    final uturn = fakeContent(
      'Make a U-turn',
      type: ManeuverType.turn,
      modifier: ManeuverModifier.uTurn,
    );
    Iterable<Transform> flips() => tester.widgetList<Transform>(
      find.descendant(
        of: find.byType(LaneStrip),
        matching: find.byType(Transform),
      ),
    );

    await show(
      tester,
      fakeVisual(uturn, sub: lanes),
      drivingSide: NavDrivingSide.left,
    );
    final flipped = flips().where((t) => t.transform.storage[0] == -1);
    expect(flipped, hasLength(1));
    expect(
      find.descendant(
        of: find.byWidget(flipped.single),
        matching: find.byIcon(glyph('turn_uturn')),
      ),
      findsOneWidget,
    );

    await show(
      tester,
      fakeVisual(uturn, sub: lanes),
      drivingSide: NavDrivingSide.right,
    );
    expect(flips().where((t) => t.transform.storage[0] == -1), isEmpty);
  });

  testWidgets('a wide road\'s lanes shrink to fit the banner', (tester) async {
    tester.view.physicalSize = const Size(320, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await show(
      tester,
      fakeVisual(
        offRamp,
        sub: fakeLanes([
          for (var i = 0; i < 12; i++) fakeLane(['straight']),
        ]),
      ),
    );
    expect(tester.takeException(), isNull);
    expect(
      tester.getSize(find.byType(LaneStrip)).width,
      lessThanOrEqualTo(320),
    );
  });

  group('exit numbers', () {
    testWidgets('show as a badge before the instruction', (tester) async {
      await show(tester, fakeVisual(offRamp));
      expect(find.text('Exit 165B'), findsOneWidget);
      final badge = tester.getRect(find.byType(ExitBadge));
      final text = find.textContaining('Union Street');
      expect(badge.left, tester.getTopLeft(text).dx);
      // In the theme's primary color.
      final box =
          tester
                  .widget<DecoratedBox>(
                    find.descendant(
                      of: find.byType(ExitBadge),
                      matching: find.byType(DecoratedBox),
                    ),
                  )
                  .decoration
              as BoxDecoration;
      final colors = Theme.of(tester.element(text)).colorScheme;
      expect(box.color, colors.primary);
      expect(box.borderRadius, isNotNull);
      expect(
        tester.widget<Text>(find.text('Exit 165B')).style?.color,
        colors.onPrimary,
      );
    });

    testWidgets('several are joined', (tester) async {
      await show(
        tester,
        fakeVisual(
          fakeContent(
            'Mercer Street',
            type: ManeuverType.offRamp,
            exitNumbers: ['167A', '167B'],
          ),
        ),
      );
      expect(find.text('Exit 167A/167B'), findsOneWidget);
    });

    testWidgets('none, no badge', (tester) async {
      await show(tester, fakeVisual(fakeContent('Pike Street')));
      expect(find.byType(ExitBadge), findsNothing);
      expect(find.text('Pike Street'), findsOneWidget);
    });

    testWidgets('the step list shows exits, but not lanes', (tester) async {
      tester.view.physicalSize = const Size(800, 1200);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      await show(
        tester,
        fakeVisual(fakeContent('Keep left', type: ManeuverType.fork)),
        upcoming: [
          fakeManeuverStep(
            'Union Street',
            type: ManeuverType.offRamp,
            modifier: ManeuverModifier.slightRight,
            exitNumbers: ['165B'],
            lanes: exitLanes,
          ),
          fakeManeuverStep('Arrive', type: ManeuverType.arrive),
        ],
      );
      await tester.pumpAndSettle();
      expect(find.text('Exit 165B'), findsOneWidget);
      expect(find.textContaining('Union Street'), findsOneWidget);
      expect(find.byType(LaneStrip), findsNothing);
    });
  });
}
