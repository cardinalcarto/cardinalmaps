import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/navigation/maneuver_banner.dart';
import 'package:cardinal/navigation/maneuver_icon.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_location.dart';
import 'package:cardinal/navigation/navigation_status.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:cardinal/units.dart';
import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoActivityIndicator, CupertinoButton;
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_navigation.dart';

void main() {
  Widget app(
    Widget child, {
    TargetPlatform platform = TargetPlatform.android,
  }) => MaterialApp(
    theme: appTheme(Brightness.light, platform: platform),
    localizationsDelegates: AppLocalizations.localizationsDelegates,
    supportedLocales: AppLocalizations.supportedLocales,
    home: Scaffold(
      body: Align(alignment: Alignment.topCenter, child: child),
    ),
  );

  List<String?> texts(WidgetTester tester) =>
      tester.widgetList<Text>(find.byType(Text)).map((t) => t.data).toList();

  group('the status card says', () {
    for (final (status, problem, failure, said) in [
      (NavigationStatus.acquiring, null, null, ['Waiting for GPS…']),
      (NavigationStatus.offRoute, null, null, ['Off route']),
      (NavigationStatus.rerouting, null, null, ['Finding a new route…']),
      (
        NavigationStatus.rerouteFailed,
        null,
        RerouteFailure.noRoute,
        ['No route from here', 'Will keep trying'],
      ),
      (
        NavigationStatus.rerouteFailed,
        null,
        RerouteFailure.outsideOfflineArea,
        ['Outside the offline area', 'Will keep trying'],
      ),
      (
        NavigationStatus.rerouteFailed,
        null,
        RerouteFailure.noOfflineArea,
        ['No offline area to route in', 'Will keep trying'],
      ),
      (
        NavigationStatus.rerouteFailed,
        null,
        RerouteFailure.failed,
        ["Couldn't find a new route", 'Will keep trying'],
      ),
      (
        NavigationStatus.locationProblem,
        LocationProblem.off,
        null,
        ['Location is off', 'Turn it on to keep navigating', 'Open settings'],
      ),
      (
        NavigationStatus.locationProblem,
        LocationProblem.denied,
        null,
        [
          'Location is turned off for Cardinal',
          'Allow it to keep navigating',
          'Open settings',
        ],
      ),
      (
        NavigationStatus.locationProblem,
        LocationProblem.failed,
        null,
        ['Your location stopped updating', 'Try again'],
      ),
    ]) {
      testWidgets('${status.name} ${problem?.name ?? failure?.name ?? ''}', (
        tester,
      ) async {
        await tester.pumpWidget(
          app(
            NavigationStatusContent(
              status: status,
              problem: problem,
              failure: failure,
              onAction: () {},
            ),
          ),
        );
        expect(texts(tester), said);
        // Only rerouting has a progress indicator, and only a location problem a
        // button.
        expect(
          find.byType(CircularProgressIndicator),
          status == NavigationStatus.rerouting ? findsOneWidget : findsNothing,
        );
        expect(
          find.byType(FilledButton),
          problem == null ? findsNothing : findsOneWidget,
        );
      });
    }
  });

  testWidgets('the location problem\'s button does its action', (tester) async {
    var pressed = 0;
    await tester.pumpWidget(
      app(
        NavigationStatusContent(
          status: NavigationStatus.locationProblem,
          problem: LocationProblem.failed,
          onAction: () => pressed++,
        ),
      ),
    );
    await tester.tap(find.text('Try again'));
    expect(pressed, 1);
  });

  testWidgets('on iOS, its button and progress are Cupertino', (tester) async {
    await tester.pumpWidget(
      app(
        const NavigationStatusContent(
          status: NavigationStatus.locationProblem,
          problem: LocationProblem.off,
        ),
        platform: TargetPlatform.iOS,
      ),
    );
    expect(find.byType(CupertinoButton), findsOneWidget);
    await tester.pumpWidget(
      app(
        const NavigationStatusContent(status: NavigationStatus.rerouting),
        platform: TargetPlatform.iOS,
      ),
    );
    expect(find.byType(CupertinoActivityIndicator), findsOneWidget);
    expect(find.byType(CircularProgressIndicator), findsNothing);
  });

  group('in the banner', () {
    final upcoming = [
      fakeManeuverStep('Turn right onto 4th Avenue'),
      fakeManeuverStep('Turn left onto Pine Street'),
    ];
    final instruction = fakeVisual(
      fakeContent(
        'Turn left onto Pike Street',
        type: ManeuverType.turn,
        modifier: ManeuverModifier.left,
      ),
    );

    testWidgets('takes the maneuver\'s place, with no handle', (tester) async {
      final height = ValueNotifier(0.0);
      final expanded = ValueNotifier(true);
      Widget banner(Widget? status) => app(
        ManeuverBanner(
          instruction: instruction,
          distance: 250,
          units: UnitSystem.metric,
          upcoming: upcoming,
          expanded: expanded,
          collapsedHeight: height,
          status: status,
        ),
      );
      await tester.pumpWidget(
        banner(
          const NavigationStatusContent(status: NavigationStatus.offRoute),
        ),
      );
      await tester.pump();
      expect(find.text('Off route'), findsOneWidget);
      expect(find.text('Turn left onto Pike Street'), findsNothing);
      expect(find.text('Turn right onto 4th Avenue'), findsNothing);
      expect(find.bySemanticsLabel('Hide upcoming maneuvers'), findsNothing);
      expect(find.bySemanticsLabel('Show upcoming maneuvers'), findsNothing);
      // As tall as the card, which the screen lays the buttons out under.
      expect(height.value, tester.getSize(find.byType(ManeuverBanner)).height);
      expect(height.value, lessThan(100));

      // Gone, the banner comes back as it was: open.
      await tester.pumpWidget(banner(null));
      await tester.pumpAndSettle();
      expect(expanded.value, isTrue);
      expect(find.text('Turn right onto 4th Avenue'), findsOneWidget);
      expect(find.bySemanticsLabel('Hide upcoming maneuvers'), findsOneWidget);
    });
  });

  group('the arrival', () {
    testWidgets('says you\'ve arrived, and where', (tester) async {
      await tester.pumpWidget(
        app(
          const ArrivalContent(
            destinationName: 'Chu Minh',
            side: ManeuverModifier.left,
          ),
        ),
      );
      expect(texts(tester), ["You've arrived", 'Chu Minh']);
      expect(
        tester.widget<Icon>(find.byType(Icon)).icon,
        maneuverIcon(ManeuverType.arrive, ManeuverModifier.left),
      );
    });

    testWidgets('without a name, says only that, with the plain icon', (
      tester,
    ) async {
      await tester.pumpWidget(app(const ArrivalContent(destinationName: null)));
      expect(texts(tester), ["You've arrived"]);
      expect(
        tester.widget<Icon>(find.byType(Icon)).icon,
        maneuverIcon(ManeuverType.arrive, null),
      );
    });

    test('takes its side from the route\'s arrival', () {
      expect(
        ArrivalContent.sideOf([
          fakeManeuverStep('Turn left', type: ManeuverType.turn),
          fakeManeuverStep(
            'Your destination is on the right',
            type: ManeuverType.arrive,
            modifier: ManeuverModifier.right,
          ),
          fakeNavStep(null),
        ]),
        ManeuverModifier.right,
      );
      expect(
        ArrivalContent.sideOf([
          fakeManeuverStep(
            'Turn left',
            type: ManeuverType.turn,
            modifier: ManeuverModifier.left,
          ),
        ]),
        isNull,
      );
    });
  });

  group('GPS signal lost', () {
    final at = DateTime(2026, 11, 14, 12);

    testWidgets('shows once no fix has come for $staleFixAge', (tester) async {
      await tester.pumpWidget(
        app(StaleFixIndicator(lastFixAt: at, applies: true)),
      );
      await tester.pump(staleFixAge - const Duration(milliseconds: 1));
      expect(find.text('GPS signal lost'), findsNothing);
      await tester.pump(const Duration(milliseconds: 1));
      expect(find.text('GPS signal lost'), findsOneWidget);

      // A fix clears it, and the wait starts over.
      final next = at.add(const Duration(seconds: 6));
      await tester.pumpWidget(
        app(StaleFixIndicator(lastFixAt: next, applies: true)),
      );
      expect(find.text('GPS signal lost'), findsNothing);
      await tester.pump(staleFixAge - const Duration(milliseconds: 1));
      expect(find.text('GPS signal lost'), findsNothing);
      await tester.pump(const Duration(milliseconds: 1));
      expect(find.text('GPS signal lost'), findsOneWidget);
    });

    testWidgets('not where it doesn\'t apply, nor before a first fix', (
      tester,
    ) async {
      await tester.pumpWidget(
        app(StaleFixIndicator(lastFixAt: at, applies: false)),
      );
      await tester.pump(staleFixAge * 2);
      expect(find.text('GPS signal lost'), findsNothing);
      await tester.pumpWidget(
        app(const StaleFixIndicator(lastFixAt: null, applies: true)),
      );
      await tester.pump(staleFixAge * 2);
      expect(find.text('GPS signal lost'), findsNothing);
      // Shown, it goes as soon as it no longer applies.
      await tester.pumpWidget(
        app(StaleFixIndicator(lastFixAt: at, applies: true)),
      );
      await tester.pump(staleFixAge);
      expect(find.text('GPS signal lost'), findsOneWidget);
      await tester.pumpWidget(
        app(StaleFixIndicator(lastFixAt: at, applies: false)),
      );
      expect(find.text('GPS signal lost'), findsNothing);
    });
  });
}
