import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/navigation/trip_progress.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:cardinal/units.dart';
import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoButton;
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

/// 9.4 km and 18 minutes 40 seconds to go.
const midTrip = NavProgress(
  distanceToNextManeuver: 250,
  durationToNextManeuver: 20,
  distanceRemaining: 9400,
  durationRemaining: 18 * 60 + 40,
);

/// 12:23 in the afternoon, so mid-trip arrives at 12:41 PM.
final now = DateTime(2026, 11, 14, 12, 23);

void main() {
  late int ended;
  late DateTime clock;

  setUp(() {
    ended = 0;
    clock = now;
  });

  Widget pill({
    NavProgress? progress = midTrip,
    bool arrived = false,
    UnitSystem units = UnitSystem.metric,
    TargetPlatform platform = TargetPlatform.android,
    bool use24 = false,
  }) => MaterialApp(
    theme: appTheme(Brightness.light, platform: platform),
    localizationsDelegates: AppLocalizations.localizationsDelegates,
    supportedLocales: AppLocalizations.supportedLocales,
    builder: (context, child) => MediaQuery(
      data: MediaQuery.of(context).copyWith(alwaysUse24HourFormat: use24),
      child: child!,
    ),
    home: Scaffold(
      body: Align(
        alignment: Alignment.bottomCenter,
        child: TripProgressPill(
          progress: progress,
          arrived: arrived,
          units: units,
          onEnd: () => ended++,
          clock: () => clock,
        ),
      ),
    ),
  );

  Finder end(TargetPlatform platform) => platform == TargetPlatform.iOS
      ? find.descendant(
          of: find.bySemanticsLabel(TripProgressPill.endLabel),
          matching: find.byType(CupertinoButton),
        )
      : find.byTooltip(TripProgressPill.endLabel);

  for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
    group(platform.name, () {
      testWidgets(
        'shows arrival, time left and distance left, in that order, then End',
        (tester) async {
          await tester.pumpWidget(pill(platform: platform));
          final arrival = find.text('12:41 PM');
          final left = find.text('18 min');
          final distance = find.text('9.4 km');
          for (final value in [arrival, left, distance]) {
            expect(value, findsOneWidget);
          }
          expect(end(platform), findsOneWidget);
          final xs = [
            arrival,
            left,
            distance,
            end(platform),
          ].map((f) => tester.getCenter(f).dx).toList();
          expect(xs, orderedEquals([...xs]..sort()));
          // Values only, in Ferrostar's simplified style: no captions.
          for (final caption in ['Arrival', 'Duration', 'Distance']) {
            expect(find.text(caption), findsNothing);
          }
        },
      );

      testWidgets('End ends the trip', (tester) async {
        await tester.pumpWidget(pill(platform: platform));
        await tester.tap(end(platform));
        expect(ended, 1);
      });

      testWidgets('arrived: nothing left, at the time it arrived, and End', (
        tester,
      ) async {
        await tester.pumpWidget(pill(platform: platform));
        clock = DateTime(2026, 11, 14, 12, 42);
        await tester.pumpWidget(
          pill(progress: null, arrived: true, platform: platform),
        );
        expect(find.text('12:42 PM'), findsOneWidget);
        expect(find.text('<1 min'), findsOneWidget);
        expect(find.text('0 m'), findsOneWidget);
        // The arrival time stays put as time goes on.
        clock = DateTime(2026, 11, 14, 12, 50);
        await tester.pumpWidget(
          pill(progress: null, arrived: true, platform: platform),
        );
        expect(find.text('12:42 PM'), findsOneWidget);
        await tester.tap(end(platform));
        expect(ended, 1);
      });
    });
  }

  testWidgets('the arrival time is in the 24-hour clock if the device is', (
    tester,
  ) async {
    clock = DateTime(2026, 11, 14, 16, 55);
    await tester.pumpWidget(pill(use24: true));
    expect(find.text('17:13'), findsOneWidget);
    await tester.pumpWidget(pill());
    expect(find.text('5:13 PM'), findsOneWidget);
  });

  testWidgets('the arrival time moves as the time left does', (tester) async {
    await tester.pumpWidget(pill());
    expect(find.text('12:41 PM'), findsOneWidget);
    clock = now.add(const Duration(minutes: 5));
    await tester.pumpWidget(
      pill(
        progress: const NavProgress(
          distanceToNextManeuver: 250,
          durationToNextManeuver: 20,
          distanceRemaining: 9000,
          durationRemaining: 3600 + 5 * 60,
        ),
      ),
    );
    expect(find.text('1:33 PM'), findsOneWidget);
    expect(find.text('1 hr 5 min'), findsOneWidget);
    expect(find.text('9 km'), findsOneWidget);
  });

  testWidgets('imperial: the distance in miles, or feet near the end', (
    tester,
  ) async {
    await tester.pumpWidget(pill(units: UnitSystem.imperial));
    expect(find.text('5.8 mi'), findsOneWidget);
    await tester.pumpWidget(
      pill(
        units: UnitSystem.imperial,
        progress: const NavProgress(
          distanceToNextManeuver: 120,
          durationToNextManeuver: 15,
          distanceRemaining: 120,
          durationRemaining: 15,
        ),
      ),
    );
    expect(find.text('400 ft'), findsOneWidget);
    expect(find.text('<1 min'), findsOneWidget);
  });

  testWidgets('arrived in imperial is 0 ft', (tester) async {
    await tester.pumpWidget(
      pill(progress: null, arrived: true, units: UnitSystem.imperial),
    );
    expect(find.text('0 ft'), findsOneWidget);
  });

  testWidgets('before the first fix, only End', (tester) async {
    await tester.pumpWidget(pill(progress: null));
    expect(find.byType(Text), findsNothing);
    expect(end(TargetPlatform.android), findsOneWidget);
    await tester.tap(end(TargetPlatform.android));
    expect(ended, 1);
  });

  testWidgets('a long value shrinks to fit rather than overflowing', (
    tester,
  ) async {
    tester.view
      ..physicalSize = const Size(320, 640)
      ..devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      pill(
        progress: const NavProgress(
          distanceToNextManeuver: 250,
          durationToNextManeuver: 20,
          distanceRemaining: 4123000,
          durationRemaining: 100 * 3600 + 59 * 60,
        ),
      ),
    );
    expect(tester.takeException(), isNull);
    expect(find.text('100 hr 59 min'), findsOneWidget);
    expect(find.text('4,123 km'), findsOneWidget);
  });
}
