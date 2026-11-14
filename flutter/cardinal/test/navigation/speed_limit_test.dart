import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/speed_limit.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_navigation.dart';
import '../fakes/fake_routes.dart';

NavSpeedLimit kph(double kph) =>
    NavSpeedLimit(kind: SpeedLimitKind.limited, kph: kph);
const unlimited = NavSpeedLimit(kind: SpeedLimitKind.unlimited);
const unknown = NavSpeedLimit(kind: SpeedLimitKind.unknown);

const mutcd = SpeedLimitSign.mutcd;
const vienna = SpeedLimitSign.vienna;
const mph = SpeedLimitUnit.milesPerHour;
const kmh = SpeedLimitUnit.kilometersPerHour;

void main() {
  group('the sign shows', () {
    test('a limit in the step\'s unit, mph rounded from km/h', () {
      // Valhalla keeps `maxspeed=60 mph` as 97 km/h, and 25 mph as 40.
      expect(speedLimitToShow(kph(97), sign: mutcd, unit: mph), (
        sign: mutcd,
        unit: mph,
        speed: 60,
      ));
      expect(speedLimitToShow(kph(40), sign: mutcd, unit: mph)?.speed, 25);
      expect(speedLimitToShow(kph(32), sign: mutcd, unit: mph)?.speed, 20);
      // Canada's MUTCD signs are in km/h; the UK's Vienna signs in mph.
      expect(speedLimitToShow(kph(50), sign: mutcd, unit: kmh), (
        sign: mutcd,
        unit: kmh,
        speed: 50,
      ));
      expect(speedLimitToShow(kph(48), sign: vienna, unit: mph), (
        sign: vienna,
        unit: mph,
        speed: 30,
      ));
      expect(speedLimitToShow(kph(130), sign: vienna, unit: kmh)?.speed, 130);
    });

    test('in the sign\'s own unit, or on Vienna signs, where unsaid', () {
      expect(speedLimitToShow(kph(97), sign: mutcd)?.speed, 60);
      expect(speedLimitToShow(kph(50)), (sign: vienna, unit: kmh, speed: 50));
    });

    test('the end of limits on Vienna signs only, and nothing unknown', () {
      expect(speedLimitToShow(unlimited, sign: vienna, unit: kmh), (
        sign: vienna,
        unit: kmh,
        speed: null,
      ));
      expect(speedLimitToShow(unlimited, sign: mutcd, unit: mph), isNull);
      expect(speedLimitToShow(unknown, sign: mutcd, unit: mph), isNull);
      expect(speedLimitToShow(unknown, sign: vienna, unit: kmh), isNull);
    });
  });

  group('the trip shows a sign', () {
    late FakeNavigationLocation location;
    late FakeSessions sessions;
    late NavigationController controller;

    final steps = [
      fakeNavStep('Woodlawn Avenue North', sign: mutcd, unit: mph),
      fakeNavStep('North 45th Street', sign: mutcd, unit: mph),
    ];

    void start(TravelMode mode) {
      location = FakeNavigationLocation();
      sessions = FakeSessions();
      controller = fakeNavigationController(
        location: location,
        sessions: sessions,
      );
      controller.start(
        route: fakeRoute([
          [0, 0],
          [0.01, 0],
        ], navigationSteps: steps),
        mode: mode,
        wheelchair: false,
      );
    }

    void fix(FakeAsync async, NavUpdate update) {
      if (sessions.sessions.isEmpty) {
        sessions.start = update;
      } else {
        sessions.last.next = update;
      }
      location.send(fakeFix(0, 0));
      async.flushMicrotasks();
    }

    NavUpdate on({
      int step = 0,
      NavSpeedLimit? limit,
      NavDeviation deviation = NavDeviation.onRoute,
      TripStatus status = TripStatus.navigating,
    }) => fakeUpdate(
      status: status,
      location: fakeFix(0, 0),
      snapped: fakeFix(0, 0),
      stepIndex: step,
      deviation: deviation,
      toManeuver: 100,
      speedLimit: limit ?? kph(32),
    );

    test('driving on the route, in the step\'s sign and unit', () {
      fakeAsync((async) {
        start(TravelMode.driving);
        expect(speedLimitOf(controller), isNull);
        fix(async, on());
        expect(controller.state, NavigationState.onRoute);
        expect(speedLimitOf(controller), (sign: mutcd, unit: mph, speed: 20));
        fix(async, on(step: 1, limit: kph(40)));
        expect(speedLimitOf(controller)?.speed, 25);
        // A fix off the step but on the route keeps it.
        fix(
          async,
          on(step: 1, limit: kph(40), deviation: NavDeviation.offStep),
        );
        expect(speedLimitOf(controller)?.speed, 25);
        // An unknown limit shows none.
        fix(async, on(step: 1, limit: unknown));
        expect(speedLimitOf(controller), isNull);
      });
    });

    test('not off the route, rerouting or arrived', () {
      fakeAsync((async) {
        start(TravelMode.driving);
        fix(async, on());
        expect(speedLimitOf(controller), isNotNull);
        fix(async, on(deviation: NavDeviation.offRoute));
        expect(controller.state, NavigationState.offRoute);
        expect(speedLimitOf(controller), isNull);
        async.elapse(offRouteHold + const Duration(seconds: 1));
        fix(async, on(deviation: NavDeviation.offRoute));
        expect(controller.state, NavigationState.rerouting);
        expect(speedLimitOf(controller), isNull);
        async.elapse(const Duration(seconds: 1));
        async.flushMicrotasks();

        start(TravelMode.driving);
        fix(async, on());
        fix(async, on(status: TripStatus.complete));
        expect(controller.state, NavigationState.arrived);
        expect(speedLimitOf(controller), isNull);
      });
    });

    test('only driving', () {
      for (final mode in [TravelMode.cycling, TravelMode.walking]) {
        fakeAsync((async) {
          start(mode);
          fix(async, on());
          expect(controller.state, NavigationState.onRoute);
          expect(speedLimitOf(controller), isNull, reason: mode.name);
        });
      }
    });
  });

  group('the sign', () {
    Future<void> show(WidgetTester tester, SpeedLimitToShow limit) =>
        tester.pumpWidget(
          MaterialApp(
            // The dark theme: the sign is white whatever the theme.
            theme: appTheme(Brightness.dark, platform: TargetPlatform.android),
            localizationsDelegates: AppLocalizations.localizationsDelegates,
            home: Scaffold(
              body: Align(
                alignment: Alignment.topLeft,
                child: SpeedLimitSignView(limit: limit),
              ),
            ),
          ),
        );

    List<String?> texts(WidgetTester tester) =>
        tester.widgetList<Text>(find.byType(Text)).map((t) => t.data).toList();

    /// The colors of the sign's boxes, outermost first.
    List<Color?> fills(WidgetTester tester) => [
      for (final box in tester.widgetList<DecoratedBox>(
        find.descendant(
          of: find.byType(SpeedLimitSignView),
          matching: find.byType(DecoratedBox),
        ),
      ))
        if (box.decoration case BoxDecoration(:final color?)) color,
    ];

    testWidgets('MUTCD: "SPEED LIMIT" over the number', (tester) async {
      await show(tester, (sign: mutcd, unit: mph, speed: 25));
      expect(texts(tester), ['SPEED\nLIMIT', '25']);
      expect(
        tester.getSize(find.byType(SpeedLimitSignView)),
        const Size(60, 84),
      );
      expect(fills(tester).whereType<Color>(), [
        const Color(0xFFFFFFFF),
        const Color(0xFF000000),
        const Color(0xFFFFFFFF),
      ]);
      expect(find.bySemanticsLabel('Speed limit 25 mph'), findsOneWidget);
    });

    testWidgets('MUTCD in km/h says so under the number', (tester) async {
      await show(tester, (sign: mutcd, unit: kmh, speed: 50));
      expect(texts(tester), ['SPEED\nLIMIT', '50', 'km/h']);
      expect(find.bySemanticsLabel('Speed limit 50 km/h'), findsOneWidget);
    });

    testWidgets('Vienna: the number in a red ring', (tester) async {
      await show(tester, (sign: vienna, unit: kmh, speed: 130));
      expect(texts(tester), ['130']);
      expect(
        tester.getSize(find.byType(SpeedLimitSignView)),
        const Size(64, 64),
      );
      expect(fills(tester).whereType<Color>(), [
        const Color(0xFFD32F2F),
        const Color(0xFFFFFFFF),
      ]);
      // Three digits are smaller than two.
      expect(tester.widget<Text>(find.text('130')).style?.fontSize, 24);
      await show(tester, (sign: vienna, unit: kmh, speed: 50));
      expect(tester.widget<Text>(find.text('50')).style?.fontSize, 29);
      expect(find.bySemanticsLabel('Speed limit 50 km/h'), findsOneWidget);
    });

    testWidgets('Vienna in mph, as the UK posts them', (tester) async {
      await show(tester, (sign: vienna, unit: mph, speed: 30));
      expect(texts(tester), ['30']);
      expect(find.bySemanticsLabel('Speed limit 30 mph'), findsOneWidget);
    });

    testWidgets('Vienna with no limit: the end-of-restrictions sign', (
      tester,
    ) async {
      await show(tester, (sign: vienna, unit: kmh, speed: null));
      expect(texts(tester), isEmpty);
      expect(
        find.descendant(
          of: find.byType(SpeedLimitSignView),
          matching: find.byType(CustomPaint),
        ),
        findsOneWidget,
      );
      expect(find.bySemanticsLabel('No speed limit'), findsOneWidget);
    });
  });
}
