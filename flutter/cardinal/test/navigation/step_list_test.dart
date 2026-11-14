import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/navigation/maneuver_banner.dart';
import 'package:cardinal/navigation/maneuver_glyphs.dart';
import 'package:cardinal/navigation/step_list.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:cardinal/units.dart';
import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoIcons;
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_navigation.dart';

/// A trip's steps, each banner for the maneuver at the step's end, as the router gives
/// them: the last, of no length, has none.
final steps = [
  fakeManeuverStep(
    'Turn left onto Pike Street',
    distance: 400,
    type: ManeuverType.turn,
    modifier: ManeuverModifier.left,
  ),
  fakeManeuverStep(
    'Turn right onto 4th Avenue',
    distance: 120,
    type: ManeuverType.turn,
    modifier: ManeuverModifier.right,
  ),
  fakeManeuverStep(
    'Keep left at the fork',
    distance: 2500,
    type: ManeuverType.fork,
    modifier: ManeuverModifier.slightLeft,
  ),
  fakeManeuverStep(
    'Your destination is on the right',
    distance: 30,
    type: ManeuverType.arrive,
    modifier: ManeuverModifier.right,
  ),
  fakeNavStep('Pine Street'),
];

const show = 'Show upcoming maneuvers';
const hide = 'Hide upcoming maneuvers';

void main() {
  late ValueNotifier<bool> expanded;
  late ValueNotifier<double> collapsed;

  setUp(() {
    expanded = ValueNotifier(false);
    collapsed = ValueNotifier(-1);
  });
  tearDown(() {
    expanded.dispose();
    collapsed.dispose();
  });

  /// The banner on step [index] of [route], [distance] off its maneuver, at the top of
  /// the test's 800 × 600 screen.
  Future<void> pump(
    WidgetTester tester, {
    int index = 0,
    List<NavStep>? route,
    double distance = 180,
    TargetPlatform platform = TargetPlatform.android,
  }) async {
    final all = route ?? steps;
    await tester.pumpWidget(
      MaterialApp(
        theme: appTheme(Brightness.light, platform: platform),
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        home: Scaffold(
          body: Padding(
            padding: const EdgeInsets.all(16),
            child: Align(
              alignment: Alignment.topCenter,
              child: ManeuverBanner(
                instruction: all[index].visualInstructions.firstOrNull,
                distance: distance,
                units: UnitSystem.metric,
                upcoming: upcomingSteps(all, index),
                expanded: expanded,
                collapsedHeight: collapsed,
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  Finder handle(String label) => find.bySemanticsLabel(label);
  Finder list() => find.byType(StepList);
  double top(WidgetTester tester, String text) =>
      tester.getTopLeft(find.text(text)).dy;

  group('upcoming steps', () {
    test('are those after the current one', () {
      expect(upcomingSteps(steps, 0), steps.sublist(1));
      expect(upcomingSteps(steps, 3), [steps[4]]);
    });

    test('are none at the last step and past it, as once complete', () {
      expect(upcomingSteps(steps, 4), isEmpty);
      expect(upcomingSteps(steps, 5), isEmpty);
    });
  });

  for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
    group(platform.name, () {
      testWidgets('a tap on the handle opens the list, and another closes it', (
        tester,
      ) async {
        await pump(tester, platform: platform);
        expect(list(), findsNothing);
        await tester.tap(handle(show));
        await tester.pumpAndSettle();
        expect(expanded.value, isTrue);
        expect(list(), findsOneWidget);
        expect(find.text('Keep left at the fork'), findsOneWidget);
        expect(
          find.byIcon(
            platform == TargetPlatform.iOS
                ? CupertinoIcons.chevron_up
                : Icons.keyboard_arrow_up,
          ),
          findsOneWidget,
        );
        await tester.tap(handle(hide));
        await tester.pumpAndSettle();
        expect(expanded.value, isFalse);
        expect(list(), findsNothing);
        expect(handle(show), findsOneWidget);
      });

      testWidgets('a tap on the current step opens and closes it too', (
        tester,
      ) async {
        await pump(tester, platform: platform);
        await tester.tap(find.text('Turn left onto Pike Street'));
        await tester.pumpAndSettle();
        expect(list(), findsOneWidget);
        await tester.tap(find.text('Turn left onto Pike Street'));
        await tester.pumpAndSettle();
        expect(list(), findsNothing);
      });

      testWidgets('open, it fills the height it has', (tester) async {
        await pump(tester, platform: platform);
        final closed = tester.getRect(find.byType(ManeuverBanner));
        expect(collapsed.value, closed.height);
        expanded.value = true;
        await tester.pumpAndSettle();
        final open = tester.getRect(find.byType(ManeuverBanner));
        expect(open.top, closed.top);
        expect(open.bottom, 600 - 16);
        // What the controls are laid out under stays the closed banner's.
        expect(collapsed.value, closed.height);
      });
    });
  }

  group('dragging the handle', () {
    testWidgets('down far enough opens the list, and up closes it', (
      tester,
    ) async {
      await pump(tester);
      await tester.timedDrag(
        handle(show),
        const Offset(0, 260),
        const Duration(seconds: 1),
      );
      await tester.pumpAndSettle();
      expect(expanded.value, isTrue);
      expect(list(), findsOneWidget);
      await tester.timedDrag(
        handle(hide),
        const Offset(0, -260),
        const Duration(seconds: 1),
      );
      await tester.pumpAndSettle();
      expect(expanded.value, isFalse);
      expect(list(), findsNothing);
    });

    testWidgets('pulls the list out after the finger', (tester) async {
      await pump(tester);
      final closed = tester.getRect(find.byType(ManeuverBanner)).height;
      final gesture = await tester.startGesture(tester.getCenter(handle(show)));
      // Past the slop, then 100 more.
      await gesture.moveBy(const Offset(0, 30));
      await gesture.moveBy(const Offset(0, 100));
      await tester.pump();
      expect(list(), findsOneWidget);
      expect(
        tester.getRect(find.byType(ManeuverBanner)).height,
        closeTo(closed + 100, 1),
      );
      await gesture.up();
      await tester.pumpAndSettle();
    });

    testWidgets('a short slow drag springs back', (tester) async {
      await pump(tester);
      await tester.timedDrag(
        handle(show),
        const Offset(0, 100),
        const Duration(seconds: 1),
      );
      await tester.pumpAndSettle();
      expect(expanded.value, isFalse);
      expect(list(), findsNothing);

      expanded.value = true;
      await tester.pumpAndSettle();
      await tester.timedDrag(
        handle(hide),
        const Offset(0, -100),
        const Duration(seconds: 1),
      );
      await tester.pumpAndSettle();
      expect(expanded.value, isTrue);
      expect(tester.getRect(find.byType(ManeuverBanner)).bottom, 600 - 16);
    });

    testWidgets('a short fling opens it, as its projected end is far enough', (
      tester,
    ) async {
      await pump(tester);
      await tester.fling(handle(show), const Offset(0, 60), 1000);
      await tester.pumpAndSettle();
      expect(expanded.value, isTrue);
    });
  });

  testWidgets('lists the steps after the current one, in order, with arriving '
      'last', (tester) async {
    expanded.value = true;
    await pump(tester);
    // The current step stays at the top, as the banner shows it.
    expect(find.text('Turn left onto Pike Street'), findsOneWidget);
    final listed = [
      'Turn right onto 4th Avenue',
      'Keep left at the fork',
      'Your destination is on the right',
    ];
    for (final text in listed) {
      expect(
        find.descendant(of: list(), matching: find.text(text)),
        findsOneWidget,
      );
    }
    final tops = [
      top(tester, 'Turn left onto Pike Street'),
      for (final text in listed) top(tester, text),
    ];
    expect(tops, orderedEquals([...tops]..sort()));
    // The last step, with no banner, isn't a row.
    expect(
      find.descendant(of: list(), matching: find.byType(ManeuverRow)),
      findsNWidgets(3),
    );
    // Each with its icon.
    for (final icon in ['turn_right', 'fork_slight_left', 'arrive_right']) {
      expect(
        find.descendant(
          of: list(),
          matching: find.byIcon(maneuverGlyphs[icon]!),
        ),
        findsOneWidget,
      );
    }
  });

  testWidgets("each row's distance is its step's length", (tester) async {
    expanded.value = true;
    await pump(tester);
    Finder row(String text) =>
        find.ancestor(of: find.text(text), matching: find.byType(ManeuverRow));
    String distanceOf(String text) => tester
        .widgetList<Text>(
          find.descendant(of: row(text), matching: find.byType(Text)),
        )
        .first
        .data!;
    expect(distanceOf('Turn right onto 4th Avenue'), '100 m');
    expect(distanceOf('Keep left at the fork'), '2.5 km');
    expect(distanceOf('Your destination is on the right'), '30 m');
  });

  testWidgets('the current step counts down as you go, open', (tester) async {
    expanded.value = true;
    await pump(tester, distance: 180);
    expect(find.text('200 m'), findsOneWidget);
    await pump(tester, distance: 42);
    expect(find.text('40 m'), findsOneWidget);
    expect(find.text('200 m'), findsNothing);
    expect(list(), findsOneWidget);
  });

  testWidgets('moves on with each step, and closes at the last', (
    tester,
  ) async {
    expanded.value = true;
    await pump(tester);
    await pump(tester, index: 1);
    expect(find.text('Turn right onto 4th Avenue'), findsOneWidget);
    expect(
      find.descendant(
        of: list(),
        matching: find.text('Turn right onto 4th Avenue'),
      ),
      findsNothing,
    );
    expect(
      find.descendant(of: list(), matching: find.byType(ManeuverRow)),
      findsNWidgets(2),
    );
    // On the last step with a banner, nothing's left to list.
    await pump(tester, index: 3);
    expect(list(), findsNothing);
    expect(handle(show), findsNothing);
    expect(handle(hide), findsNothing);
    expect(collapsed.value, tester.getSize(find.byType(ManeuverBanner)).height);
  });

  testWidgets('lists a new route in place of the old', (tester) async {
    expanded.value = true;
    await pump(tester);
    final rerouted = [
      fakeManeuverStep('Turn right onto Union Street', distance: 80),
      fakeManeuverStep('Turn left onto 3rd Avenue', distance: 300),
      fakeManeuverStep('Your destination is on the left', distance: 20),
      fakeNavStep(null),
    ];
    await pump(tester, route: rerouted);
    expect(find.text('Turn right onto Union Street'), findsOneWidget);
    expect(find.text('Keep left at the fork'), findsNothing);
    expect(
      find.descendant(
        of: list(),
        matching: find.text('Turn left onto 3rd Avenue'),
      ),
      findsOneWidget,
    );
    expect(expanded.value, isTrue);
  });

  testWidgets('shows nothing without an instruction, and says so', (
    tester,
  ) async {
    await pump(tester);
    expect(collapsed.value, greaterThan(0));
    await tester.pumpWidget(
      MaterialApp(
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        home: ManeuverBanner(
          instruction: null,
          distance: null,
          units: UnitSystem.metric,
          upcoming: steps,
          collapsedHeight: collapsed,
        ),
      ),
    );
    await tester.pump();
    expect(collapsed.value, 0);
    expect(find.byType(Text), findsNothing);
  });
}
