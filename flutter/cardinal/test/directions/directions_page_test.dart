import 'dart:async';
import 'dart:math' as math;

import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoActivityIndicator, CupertinoButton;
import 'package:cardinal/directions/connections.dart';
import 'package:cardinal/directions/journey_list.dart';
import 'package:cardinal/directions/directions_page.dart';
import 'package:cardinal/directions/mode_tabs.dart';
import 'package:cardinal/directions/option_row.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/directions/transit_details.dart';
import 'package:cardinal/directions/trip_details_page.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/map/overlays.dart';
import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/map/selection.dart';
import 'package:cardinal/map/map_control.dart';
import 'package:cardinal/navigation/maneuver_banner.dart';
import 'package:cardinal/navigation/maneuver_icon.dart';
import 'package:cardinal/navigation/navigation_camera.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_location.dart';
import 'package:cardinal/navigation/navigation_status.dart';
import 'package:cardinal/navigation/road_name.dart';
import 'package:cardinal/navigation/step_list.dart';
import 'package:cardinal/navigation/trip_progress.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/settings/accessibility.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';
import 'package:sparkle/sparkle.dart' show PuckStyle;

import '../fakes/fake_app.dart';
import '../fakes/fake_location.dart';
import '../fakes/fake_map.dart';
import '../fakes/fake_navigation.dart';
import '../fakes/fake_routes.dart';
import '../fakes/fake_transit.dart';

final destination = Place(
  id: 'poi:1',
  kind: 'poi',
  name: 'Chu Minh',
  center: const LngLat(1, 4),
);

final best = fakeRoute(
  [
    [0.5, 2],
    [1, 4],
  ],
  distance: 10460,
  duration: 672,
  steps: const [
    DirectionsStep(instruction: 'Drive north.', distance: 32, duration: 5),
    DirectionsStep(instruction: 'Turn right.', distance: 1000, duration: 60),
  ],
  // The road names as the router may give them, with space around, and none.
  navigationSteps: [fakeNavStep('  Pike Street '), fakeNavStep('   ')],
);
final other = fakeRoute(
  [
    [0.5, 2],
    [1.5, 3],
    [1, 4],
  ],
  distance: 12000,
  duration: 900,
  steps: const [
    DirectionsStep(
      instruction: 'Go the long way.',
      distance: 12000,
      duration: 900,
    ),
  ],
);

const here = Fix(lng: 0.5, lat: 2, accuracy: 3);

void main() {
  late Store store;
  late FakeRoutes routes;
  late FakeTransit transit;
  late FakeMap map;
  late MapOverlays overlays;
  late NavigationController navigation;
  late FakeNavigationLocation navigationLocation;
  late FakeSessions sessions;
  late FakeRoutes navigationRoutes;
  late FakeSpeaker speaker;

  setUp(() async {
    store = fakeStore();
    await store.places.put(destination);
    routes = FakeRoutes(immediately: [best, other]);
    transit = FakeTransit(immediately: fakePlan(journeys: 2));
    map = FakeMap()..hasGround = true;
    overlays = MapOverlays();
    navigationLocation = FakeNavigationLocation();
    sessions = FakeSessions();
    navigationRoutes = FakeRoutes();
    speaker = FakeSpeaker();
    navigation = fakeNavigationController(
      location: navigationLocation,
      sessions: sessions,
    );
  });

  Future<GoRouter> open(
    WidgetTester tester, {
    FakeLocation? location,
    bool settle = true,
    TargetPlatform? platform,
  }) async {
    final router = await pumpFakeApp(
      tester,
      platform: platform,
      navigation: navigation,
      store: store,
      routes: routes,
      transit: transit,
      map: map,
      overlays: overlays,
      locationSource: location ?? FakeLocation(now: here),
      location: placeLocation(destination.id),
    );
    await tester.tap(find.text('Directions'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));
    // While a request is out, a progress bar runs for as long as it takes.
    if (settle) await tester.pumpAndSettle();
    return router;
  }

  List<Map<String, Object?>> features(String source) =>
      ((map.geoJson[source]! as Map)['features'] as List)
          .cast<Map<String, Object?>>();

  /// The questions asked for [mode], in order.
  List<RouteQuestion> asked(TravelMode mode) =>
      routes.asked.where((question) => question.mode == mode).toList();

  /// The way of traveling whose tab is selected.
  TravelMode shownMode(WidgetTester tester) =>
      tester.widget<ModeTabs>(find.byType(ModeTabs)).selected;

  /// Whether each row is the selected one, in the list's order.
  List<bool> selectedRows(WidgetTester tester) => [
    for (final row in tester.widgetList<OptionRow>(find.byType(OptionRow)))
      row.selected,
  ];

  testWidgets('routes from where you are to the place, as rows', (
    tester,
  ) async {
    await open(tester);
    expect(find.text('My Location'), findsOneWidget);
    expect(find.text('Chu Minh'), findsOneWidget);
    expect(asked(TravelMode.driving).single.from, const LngLat(0.5, 2));
    expect(asked(TravelMode.driving).single.to, destination.center);
    expect(find.byType(OptionRow), findsNWidgets(2));
    expect(
      find.descendant(
        of: find.byType(OptionRow).first,
        matching: find.text('11 min'),
      ),
      findsOneWidget,
    );
    expect(find.text('6.5 mi · Fastest · Shortest'), findsOneWidget);
    expect(find.text('7.5 mi · 4 min slower'), findsOneWidget);
    // The steps are a page of their own.
    expect(find.text('Drive north.'), findsNothing);
  });

  group('distances follow the units setting over the region', () {
    void inLocale(WidgetTester tester, Locale locale) {
      tester.platformDispatcher.localeTestValue = locale;
      addTearDown(tester.platformDispatcher.clearLocaleTestValue);
    }

    testWidgets('metric in the US', (tester) async {
      inLocale(tester, const Locale('en', 'US'));
      await store.settings.set(unitSystem, UnitSystem.metric);
      await open(tester);
      expect(find.textContaining('10 km · '), findsOneWidget);
      expect(find.textContaining('6.5 mi'), findsNothing);
    });

    testWidgets('imperial in Germany', (tester) async {
      inLocale(tester, const Locale('de', 'DE'));
      await store.settings.set(unitSystem, UnitSystem.imperial);
      await open(tester);
      expect(find.textContaining('6.5 mi · '), findsOneWidget);
      expect(find.textContaining('10 km'), findsNothing);
    });

    testWidgets("and the region's units until it's set", (tester) async {
      inLocale(tester, const Locale('de', 'DE'));
      await open(tester);
      expect(find.textContaining('10 km · '), findsOneWidget);
    });
  });

  testWidgets('without permission the start is left empty and nothing asks', (
    tester,
  ) async {
    final location = FakeLocation(permission: LocationPermission.denied);
    await open(tester, location: location);
    expect(find.text('My Location'), findsNothing);
    expect(routes.asked, isEmpty);
    expect(location.asked, 0);
    expect(
      find.text('Enter a start and a destination to get directions'),
      findsOneWidget,
    );
  });

  testWidgets('My Location asks for permission, then fills the field', (
    tester,
  ) async {
    final location = FakeLocation(
      permission: LocationPermission.denied,
      granted: LocationPermission.whileInUse,
      now: here,
    );
    await open(tester, location: location);
    await tester.tap(find.widgetWithText(InkWell, 'From'));
    await tester.pumpAndSettle();
    expect(find.text('Use current location'), findsOneWidget);

    await tester.tap(find.text('Use current location'));
    await tester.pumpAndSettle();
    expect(location.asked, 1);
    expect(find.text('Use current location'), findsNothing);
    expect(find.text('My Location'), findsOneWidget);
    expect(routes.asked, hasLength(3), reason: 'every street way, once');
  });

  testWidgets('back while editing goes back to the trip, not the place', (
    tester,
  ) async {
    await open(tester);
    await tester.tap(find.widgetWithText(InkWell, 'To'));
    await tester.pumpAndSettle();
    expect(find.text('Use current location'), findsOneWidget);

    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(find.text('Use current location'), findsNothing);
    expect(find.text('Start'), findsOneWidget);
    expect(find.text('Chu Minh'), findsOneWidget);
  });

  testWidgets('saved places can be chosen', (tester) async {
    final saved = Place(
      id: 'poi:2',
      kind: 'poi',
      name: 'Library',
      center: const LngLat(2, 2),
    );
    await store.places.put(saved);
    await store.saved.savePlace(saved.id);
    await open(tester);
    await tester.tap(find.widgetWithText(InkWell, 'From'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Library'));
    await tester.pumpAndSettle();
    expect(routes.asked.last.from, saved.center);
    expect(find.text('Library'), findsOneWidget);
  });

  testWidgets('the start is marked, unless it is where you are', (
    tester,
  ) async {
    List<Map<String, Object?>> starts() =>
        features(routeSource)
            .where((f) => (f['properties']! as Map)['role'] == 'start')
            .toList();
    final saved = Place(
      id: 'poi:2',
      kind: 'poi',
      name: 'Library',
      center: const LngLat(2, 2),
    );
    await store.places.put(saved);
    await store.saved.savePlace(saved.id);
    await open(tester);
    // Where you are is marked by the map already.
    expect(starts(), isEmpty);
    await tester.tap(find.widgetWithText(InkWell, 'From'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Library'));
    await tester.pumpAndSettle();
    expect((starts().single['geometry']! as Map)['coordinates'], [2, 2]);
  });

  testWidgets('the place under directions adds no pin of its own', (
    tester,
  ) async {
    await open(tester);
    expect(features(selectionSource), isEmpty);
    // The card under the directions hears its place change.
    await store.places.put(
      Place(
        id: destination.id,
        kind: 'poi',
        name: 'Chu Minh Tofu',
        center: destination.center,
      ),
    );
    await tester.pumpAndSettle();
    expect(features(selectionSource), isEmpty);
    expect(
      features(routeSource)
          .where((f) => (f['properties']! as Map)['role'] == 'pin'),
      hasLength(1),
    );
  });

  testWidgets('a tab shows that way, asks nothing, and is remembered', (
    tester,
  ) async {
    await open(tester);
    final before = routes.asked.length;
    await tester.tap(find.byIcon(Icons.directions_bike));
    await tester.pumpAndSettle();
    expect(shownMode(tester), TravelMode.cycling);
    expect(routes.asked, hasLength(before));
    expect(await store.settings.get(lastTravelMode), TravelMode.cycling);
  });

  testWidgets('directions start with the mode last chosen', (tester) async {
    await store.settings.set(lastTravelMode, TravelMode.walking);
    await open(tester);
    expect(shownMode(tester), TravelMode.walking);
  });

  group('wheelchair by default', () {
    testWidgets('off: the walking tab is Walking, routing on foot', (
      tester,
    ) async {
      await open(tester);
      expect(find.byTooltip(RegExp('^Walking')), findsOneWidget);
      expect(find.byTooltip(RegExp('^Wheelchair')), findsNothing);
      expect(find.byIcon(Icons.accessible), findsNothing);
      await tester.tap(find.byIcon(Icons.directions_walk));
      await tester.pumpAndSettle();
      expect(shownMode(tester), TravelMode.walking);
      expect(asked(TravelMode.walking).single.wheelchair, isFalse);
    });

    testWidgets('on: the walking tab is Wheelchair, routing by wheelchair', (
      tester,
    ) async {
      await store.settings.set(wheelchairByDefault, true);
      await open(tester);
      expect(find.byTooltip(RegExp('^Wheelchair')), findsOneWidget);
      expect(find.byTooltip(RegExp('^Walking')), findsNothing);
      expect(find.byIcon(Icons.directions_walk), findsNothing);
      await tester.tap(find.byIcon(Icons.accessible));
      await tester.pumpAndSettle();
      expect(shownMode(tester), TravelMode.walking);
      expect(asked(TravelMode.walking).single.wheelchair, isTrue);
      // Remembered as walking, so turning the default off gives Walking back.
      expect(await store.settings.get(lastTravelMode), TravelMode.walking);
    });

    testWidgets('turned on while open, walking asks again by wheelchair', (
      tester,
    ) async {
      await store.settings.set(lastTravelMode, TravelMode.walking);
      await open(tester);
      expect(asked(TravelMode.walking).last.wheelchair, isFalse);
      final before = routes.asked.length;

      await store.settings.set(wheelchairByDefault, true);
      await tester.pumpAndSettle();
      expect(routes.asked, hasLength(before + 1));
      expect(routes.asked.last.mode, TravelMode.walking);
      expect(routes.asked.last.wheelchair, isTrue);
      expect(find.byTooltip(RegExp('^Wheelchair')), findsOneWidget);

      await store.settings.set(wheelchairByDefault, false);
      await tester.pumpAndSettle();
      expect(routes.asked.last.wheelchair, isFalse);
      expect(find.byTooltip(RegExp('^Walking')), findsOneWidget);
    });

    testWidgets('turned on while open, driving is left as it is', (
      tester,
    ) async {
      await open(tester);
      await tester.tap(find.byType(OptionRow).last);
      await tester.pumpAndSettle();
      final before = routes.asked.length;
      await store.settings.set(wheelchairByDefault, true);
      await tester.pumpAndSettle();
      // Only walking is asked again.
      expect(routes.asked, hasLength(before + 1));
      expect(routes.asked.last.mode, TravelMode.walking);
      expect(selectedRows(tester), [false, true]);
      expect(find.byTooltip(RegExp('^Wheelchair')), findsOneWidget);
    });
  });

  group('navigation', () {
    List<Map<String, Object?>> lines() =>
        features(routeSource)
            .where((f) => (f['properties']! as Map)['role'] == 'line')
            .toList();

    Future<void> start(
      WidgetTester tester, {
      TargetPlatform? platform,
      FakeLocation? location,
    }) async {
      final router = await open(tester, platform: platform, location: location);
      await tester.tap(find.text('Start'));
      await tester.pumpAndSettle();
      expect(
        router.state.uri.toString(),
        '${directionsLocation(destination.id)}/navigate',
      );
    }

    testWidgets('Start starts the trip and opens its screen', (tester) async {
      await start(tester);
      expect(navigation.state, NavigationState.acquiring);
      expect(navigation.route, same(best));
      expect(navigation.mode, TravelMode.driving);
      expect(navigation.wheelchair, isFalse);
      expect(find.byTooltip('End'), findsOneWidget);
      expect(find.text('Start'), findsNothing);
      // The trip's own drawing takes the place of the directions'.
      expect(lines(), isEmpty);
      expect(features(navigationSource), hasLength(1));
    });

    testWidgets('a trip walked by wheelchair is navigated by wheelchair', (
      tester,
    ) async {
      await store.settings.set(lastTravelMode, TravelMode.walking);
      await store.settings.set(wheelchairByDefault, true);
      await start(tester);
      expect(asked(TravelMode.walking).last.wheelchair, isTrue);
      expect(navigation.mode, TravelMode.walking);
      expect(navigation.wheelchair, isTrue);
    });

    testWidgets('a trip walked on foot is not', (tester) async {
      await store.settings.set(lastTravelMode, TravelMode.walking);
      await start(tester);
      expect(navigation.mode, TravelMode.walking);
      expect(navigation.wheelchair, isFalse);
    });

    /// Makes the controller again in the test's own fake time: its work runs in the
    /// zone it's made in, and the test pumps only its own.
    void navigateInTestTime() => navigation = fakeNavigationController(
      location: navigationLocation,
      sessions: sessions,
      routes: navigationRoutes,
      speaker: speaker,
      settings: store.settings,
    );

    /// Where the map buttons' column starts across the screen: the camera and mute
    /// buttons are as wide.
    double buttonsLeft(WidgetTester tester) => tester
        .getRect(
          find.byWidgetPredicate(
            (w) =>
                w is MapControl &&
                (w.tooltip == 'Mute' || w.tooltip == 'Unmute'),
          ),
        )
        .left;

    /// Navigation's answer to a fix: you on the route, with [visual] showing.
    Future<void> fix(
      WidgetTester tester, {
      NavVisualInstruction? visual,
      double toManeuver = 250,
      int stepIndex = 0,
      NavDeviation deviation = NavDeviation.onRoute,
      NavSpokenInstruction? spoken,
    }) async {
      final update = fakeUpdate(
        snapped: fakeFix(2.5, 0.6, course: 20, speed: 5),
        visual: visual,
        toManeuver: toManeuver,
        stepIndex: stepIndex,
        deviation: deviation,
        spoken: spoken,
      );
      if (sessions.sessions.isEmpty) {
        sessions.start = update;
      } else {
        sessions.last.next = update;
      }
      navigationLocation.send(fakeFix(2.5, 0.6));
      await tester.pumpAndSettle();
    }

    testWidgets('the destination keeps its pin', (tester) async {
      await start(tester);
      final pins = features(routeSource)
          .where((f) => (f['properties']! as Map)['role'] == 'pin');
      expect(pins, hasLength(1));
      expect((pins.single['geometry']! as Map)['coordinates'], [1, 4]);
      // And nothing else of the directions'.
      expect(features(routeSource), hasLength(1));
    });

    testWidgets('the banner shows the next maneuver, in the units set', (
      tester,
    ) async {
      navigateInTestTime();
      await start(tester);
      expect(find.text('Turn left onto Pike Street'), findsNothing);
      await fix(
        tester,
        visual: fakeVisual(
          fakeContent(
            'Turn left onto Pike Street',
            type: ManeuverType.turn,
            modifier: ManeuverModifier.left,
          ),
        ),
      );
      expect(find.text('Turn left onto Pike Street'), findsOneWidget);
      // The test's region is the US.
      expect(find.text('800 ft'), findsOneWidget);
      await store.settings.set(unitSystem, UnitSystem.metric);
      await tester.pumpAndSettle();
      // Rounded to the nearest 100 m, as Ferrostar does.
      expect(find.text('300 m'), findsOneWidget);
    });

    testWidgets('recenter shows once the map is moved off you, and follows', (
      tester,
    ) async {
      navigateInTestTime();
      await start(tester);
      await fix(tester);
      expect(map.following.value, isTrue);
      expect(find.byTooltip('Recenter'), findsNothing);
      expect(find.byTooltip('Route overview'), findsOneWidget);

      // Moved by hand.
      map.following.value = false;
      await tester.pump();
      expect(find.byTooltip('Recenter'), findsOneWidget);

      final follows = map.follows.length;
      await tester.tap(find.byTooltip('Recenter'));
      await tester.pump();
      expect(map.follows, hasLength(follows + 1));
      expect(map.following.value, isTrue);
      expect(find.byTooltip('Recenter'), findsNothing);
      expect(find.byTooltip('Route overview'), findsOneWidget);
    });

    group('the road name', () {
      testWidgets('shows the step\'s, trimmed, and none for a blank one', (
        tester,
      ) async {
        navigateInTestTime();
        await start(tester);
        // Before the first fix, there's no step.
        expect(find.text('Pike Street'), findsNothing);
        await fix(tester);
        expect(find.text('Pike Street'), findsOneWidget);
        expect(find.byType(RoadNamePill), findsOneWidget);
        await fix(tester, stepIndex: 1);
        expect(find.text('Pike Street'), findsNothing);
        expect(
          find.descendant(
            of: find.byType(RoadNamePill),
            matching: find.byType(Text),
          ),
          findsNothing,
        );
      });

      testWidgets('hides off the route and while a new one comes', (
        tester,
      ) async {
        navigateInTestTime();
        await start(tester);
        await fix(tester);
        expect(find.text('Pike Street'), findsOneWidget);
        // A shortcut onto a later step: the current step is one you've left.
        await fix(tester, deviation: NavDeviation.offStep);
        expect(find.text('Pike Street'), findsNothing);
        await fix(tester);
        expect(find.text('Pike Street'), findsOneWidget);
        await fix(tester, deviation: NavDeviation.offRoute);
        expect(navigation.state, NavigationState.offRoute);
        expect(find.text('Pike Street'), findsNothing);
        await tester.pump(offRouteHold);
        expect(navigation.state, NavigationState.rerouting);
        expect(find.text('Pike Street'), findsNothing);
        // The new route, followed.
        sessions.last.next = fakeUpdate(
          snapped: fakeFix(2.5, 0.6, course: 20, speed: 5),
        );
        navigationRoutes.rerouted.single.answer.complete(best);
        await tester.pumpAndSettle();
        expect(navigation.state, NavigationState.onRoute);
        expect(find.text('Pike Street'), findsOneWidget);
        // Until the time between new routes is up.
        await tester.pump(minRerouteInterval);
      });

      testWidgets('hides while a location problem shows', (tester) async {
        navigateInTestTime();
        await start(tester);
        await fix(tester);
        expect(find.text('Pike Street'), findsOneWidget);
        navigationLocation.fail(
          const NavigationLocationError(LocationProblem.failed),
        );
        await tester.pump();
        // Still on the route, as far as the trip knows.
        expect(navigation.state, NavigationState.onRoute);
        expect(find.text('Pike Street'), findsNothing);
        // Until a fix comes again.
        await fix(tester);
        expect(navigation.locationProblem, isNull);
        expect(find.text('Pike Street'), findsOneWidget);
      });

      testWidgets('hides when the camera is not following you', (tester) async {
        navigateInTestTime();
        await start(tester);
        await fix(tester);
        // Moved by hand.
        map.following.value = false;
        await tester.pump();
        expect(find.text('Pike Street'), findsNothing);
        await tester.tap(find.byTooltip('Recenter'));
        await tester.pump();
        expect(find.text('Pike Street'), findsOneWidget);
        // In the overview.
        await tester.tap(find.byTooltip('Route overview'));
        await tester.pump();
        expect(find.text('Pike Street'), findsNothing);
      });

      testWidgets('is in label large on Material, and as tall as it says', (
        tester,
      ) async {
        navigateInTestTime();
        await start(tester);
        await fix(tester);
        final context = tester.element(find.byType(RoadNamePill));
        expect(
          tester.widget<Text>(find.text('Pike Street')).style!.fontSize,
          Theme.of(context).textTheme.labelLarge!.fontSize,
        );
        expect(
          tester.getSize(find.byType(RoadNamePill)).height,
          closeTo(RoadNamePill.heightOf(context), 1e-9),
        );
      });

      testWidgets('shrinks a long name to fit, then cuts it off', (
        tester,
      ) async {
        Widget pill(String name) => MaterialApp(
          home: Center(
            child: SizedBox(
              width: 300,
              child: Center(child: RoadNamePill(name: name)),
            ),
          ),
        );
        double size(String name) =>
            tester.widget<Text>(find.text(name)).style!.fontSize!;
        await tester.pumpWidget(pill('Pike Street'));
        final full = size('Pike Street');
        const long = 'Martin Luther King Junior Way South';
        await tester.pumpWidget(pill(long));
        expect(size(long), lessThan(full));
        expect(tester.takeException(), isNull);
        const longer =
            'Martin Luther King Junior Way South and then some more of it';
        await tester.pumpWidget(pill(longer));
        expect(size(longer), full * RoadNamePill.minScale);
        expect(
          tester.widget<Text>(find.text(longer)).overflow,
          TextOverflow.ellipsis,
        );
        expect(tester.takeException(), isNull);
      });
    });

    group('the camera button', () {
      testWidgets('in landscape, fits the route clear of the buttons too', (
        tester,
      ) async {
        navigateInTestTime();
        await start(tester);
        tester.view.physicalSize = appScreen.flipped * 2;
        await tester.pumpAndSettle();
        await fix(tester);
        await tester.tap(find.byTooltip('Route overview'));
        await tester.pump();
        // The start half, and the end side in to the buttons' far side.
        expect(map.subjectClear!.left, appScreen.height / 2);
        expect(map.subjectClear!.right, appScreen.height - buttonsLeft(tester));
      });

      testWidgets('goes from following to overview and back', (tester) async {
        navigateInTestTime();
        await start(tester);
        // Nothing to show the trip from before the first fix.
        expect(find.byTooltip('Route overview'), findsNothing);
        expect(find.byTooltip('Recenter'), findsNothing);
        await fix(
          tester,
          visual: fakeVisual(fakeContent('Turn left onto Pike Street')),
        );
        final before = map.subjectsSet;
        await tester.tap(find.byTooltip('Route overview'));
        await tester.pump();
        expect(map.subjectsSet, before + 1);
        expect(map.following.value, isFalse);
        // Flat and north up, from the chevron to the end.
        expect((map.subjectBearing, map.subjectPitch), (0, 0));
        expect(map.subjectBounds, (west: 0.6, south: 2.5, east: 1, north: 4));
        // Clear of the banner, down to its bottom, and of trip progress, up to its
        // top, less what the map's own padding covers: the status bar (none in the
        // test) and what the sheet, down out of sight, still covers at the bottom.
        final banner = tester.getRect(find.byType(ManeuverBanner));
        final progress = tester.getRect(find.byType(TripProgressPill));
        final sheet = SheetScaffold.viewOf(
          tester.element(find.byType(TripProgressPill)),
        ).obscured;
        expect(map.subjectClear!.top, banner.bottom - sheet.top);
        expect(
          map.subjectClear!.bottom,
          math.max(0, appScreen.height - progress.top - sheet.bottom),
        );
        // And of the map buttons, down the end side.
        expect(map.subjectClear!.right, appScreen.width - buttonsLeft(tester));
        expect(map.subjectClear!.left, 0);
        expect(find.byTooltip('Route overview'), findsNothing);

        await tester.tap(find.byTooltip('Recenter'));
        await tester.pump();
        expect(map.following.value, isTrue);
        expect(find.byTooltip('Recenter'), findsNothing);
        expect(find.byTooltip('Route overview'), findsOneWidget);
      });

      testWidgets('shows a new route in the overview', (tester) async {
        navigateInTestTime();
        await start(tester);
        await fix(tester);
        await tester.tap(find.byTooltip('Route overview'));
        await tester.pump();
        final fitted = map.subjectsSet;
        await fix(tester, deviation: NavDeviation.offRoute);
        await tester.pump(offRouteHold);
        sessions.last.next = fakeUpdate(snapped: fakeFix(0, 0, course: 20));
        navigationRoutes.rerouted.single.answer.complete(
          fakeRoute([
            [0, 0],
            [3, 5],
          ]),
        );
        await tester.pumpAndSettle();
        // Fitted again for the new route, and as the status cards came and went in
        // the banner's place, each of them a height of its own.
        expect(map.subjectsSet, greaterThan(fitted));
        expect(map.subjectBounds, (west: 0, south: 0, east: 3, north: 5));
        expect(find.byTooltip('Recenter'), findsOneWidget);
        // Until the time between new routes is up.
        await tester.pump(minRerouteInterval);
      });
    });

    group('the follow camera', () {
      /// Where the chevron sits on screen, down from the top, with the camera
      /// following at [padding] on top of the map's own.
      double chevronY(WidgetTester tester, EdgeInsets padding) {
        final context = tester.element(find.byType(TripProgressPill));
        final size = MediaQuery.sizeOf(context);
        final mapPadding =
            EdgeInsets.only(top: MediaQuery.paddingOf(context).top) +
            SheetScaffold.viewOf(context).obscured;
        final all = mapPadding + padding;
        return all.top + (size.height - all.top - all.bottom) / 2;
      }

      /// What's under the chevron in landscape: the road-name pill, up from the
      /// bottom inset.
      double under(WidgetTester tester) {
        final context = tester.element(find.byType(TripProgressPill));
        return math.max(16.0, MediaQuery.paddingOf(context).bottom) +
            RoadNamePill.heightOf(context);
      }

      EdgeInsets expected(WidgetTester tester, {double under = 0}) {
        final context = tester.element(find.byType(TripProgressPill));
        return followPadding(
          size: MediaQuery.sizeOf(context),
          mapPadding:
              EdgeInsets.only(top: MediaQuery.paddingOf(context).top) +
              SheetScaffold.viewOf(context).obscured,
          systemPadding: MediaQuery.paddingOf(context),
          direction: TextDirection.ltr,
          under: under,
        );
      }

      testWidgets('keeps up with the sheet as it settles', (tester) async {
        navigateInTestTime();
        // In landscape, where the sheet is at the side, over the start half the chevron
        // is kept out of: padding read with the sheet still there pads by too little.
        await open(tester);
        tester.view.physicalSize = appScreen.flipped * 2;
        await tester.pumpAndSettle();
        await tester.tap(find.text('Start'));
        // The screen shows while the sheet is still on its way down.
        await tester.pump();
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 50));
        expect(find.byType(TripProgressPill), findsOneWidget);
        final early = SheetScaffold.viewOf(
          tester.element(find.byType(TripProgressPill)),
        ).obscured;
        await tester.pumpAndSettle();
        final settled = SheetScaffold.viewOf(
          tester.element(find.byType(TripProgressPill)),
        ).obscured;
        // Or the test shows nothing.
        expect(early, isNot(settled));
        await fix(tester);
        expect(
          map.follows.last.padding,
          expected(tester, under: under(tester)),
        );
      });

      testWidgets('in landscape, keeps the disk above the road name', (
        tester,
      ) async {
        navigateInTestTime();
        // Type large enough that the pill would reach the disk where it sits.
        tester.platformDispatcher.textScaleFactorTestValue = 1.5;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        await start(tester);
        tester.view.physicalSize = appScreen.flipped * 2;
        await tester.pumpAndSettle();
        await fix(tester);
        final pill = tester.getRect(find.byType(RoadNamePill));
        final padding = map.follows.last.padding;
        final disk = chevronY(tester, padding) + const PuckStyle().diskRadius;
        expect(pill.top - disk, closeTo(8, 1e-6));
        // Lifted off where it'd sit with nothing under it.
        expect(padding.bottom, greaterThan(expected(tester).bottom));
      });
    });

    group('the step list', () {
      final stepped = fakeRoute(
        [
          [0.5, 2],
          [1, 4],
        ],
        navigationSteps: [
          fakeManeuverStep('Turn left onto Pike Street', distance: 400),
          fakeManeuverStep('Turn right onto 4th Avenue', distance: 120),
          fakeManeuverStep('Your destination is on the right', distance: 30),
          fakeNavStep(null),
        ],
      );
      final rerouted = fakeRoute(
        [
          [0.6, 2.5],
          [1, 4],
        ],
        navigationSteps: [
          fakeManeuverStep('Turn right onto Union Street', distance: 80),
          fakeManeuverStep('Turn left onto 3rd Avenue', distance: 300),
          fakeManeuverStep('Your destination is on the left', distance: 20),
          fakeNavStep(null),
        ],
      );

      /// Navigation's answer to a fix: on step [index] of [route].
      NavUpdate onStep(int index, {DirectionsRoute? route}) => fakeUpdate(
        snapped: fakeFix(2.5, 0.6, course: 20, speed: 5),
        visual: (route ?? stepped).navigable
            .steps()[index]
            .visualInstructions
            .first,
        toManeuver: 250,
        stepIndex: index,
      );

      Future<void> fixOn(WidgetTester tester, int index) async {
        final update = onStep(index);
        if (sessions.sessions.isEmpty) {
          sessions.start = update;
        } else {
          sessions.last.next = update;
        }
        navigationLocation.send(fakeFix(2.5, 0.6));
        await tester.pumpAndSettle();
      }

      Finder listed(String text) =>
          find.descendant(of: find.byType(StepList), matching: find.text(text));

      Future<void> openList(WidgetTester tester) async {
        await tester.tap(find.bySemanticsLabel('Show upcoming maneuvers'));
        await tester.pumpAndSettle();
        expect(find.byType(StepList), findsOneWidget);
      }

      testWidgets('opens over the controls, which stay where they were', (
        tester,
      ) async {
        routes = FakeRoutes(immediately: [stepped, other]);
        navigateInTestTime();
        await start(tester);
        await fixOn(tester, 0);
        final closed = tester.getRect(find.byType(ManeuverBanner));
        final camera = tester.getRect(find.byTooltip('Route overview'));
        final mute = tester.getRect(find.byTooltip('Mute'));
        // Under the closed banner, handle and all.
        expect(camera.top, closed.bottom + 16);
        await openList(tester);
        expect(listed('Turn right onto 4th Avenue'), findsOneWidget);
        expect(listed('Your destination is on the right'), findsOneWidget);
        // Down to the bottom inset, over the buttons and trip progress, which haven't
        // moved.
        final open = tester.getRect(find.byType(ManeuverBanner));
        expect(open.top, closed.top);
        expect(open.bottom, appScreen.height - appNavigationBar);
        expect(tester.getRect(find.byTooltip('Mute')), mute);
        expect(open.contains(mute.center), isTrue);
        expect(
          open.contains(tester.getCenter(find.byType(TripProgressPill))),
          isTrue,
        );
        // A tap there is the list's.
        await tester.tapAt(mute.center);
        await tester.pump();
        expect(navigation.muted, isFalse);
        // The trip carries on under it, followed.
        expect(map.following.value, isTrue);
        expect(navigation.state, NavigationState.onRoute);
        // And a tap on the current step closes it.
        await tester.tap(find.text('Turn left onto Pike Street'));
        await tester.pumpAndSettle();
        expect(find.byType(StepList), findsNothing);
        expect(tester.getRect(find.byType(ManeuverBanner)), closed);
      });

      testWidgets('keeps up with each step, and with a new route', (
        tester,
      ) async {
        routes = FakeRoutes(immediately: [stepped, other]);
        navigateInTestTime();
        await start(tester);
        await fixOn(tester, 0);
        await openList(tester);
        await fixOn(tester, 1);
        expect(find.text('Turn right onto 4th Avenue'), findsOneWidget);
        expect(listed('Turn right onto 4th Avenue'), findsNothing);
        expect(listed('Your destination is on the right'), findsOneWidget);

        await fix(tester, deviation: NavDeviation.offRoute);
        await tester.pump(offRouteHold);
        expect(navigation.state, NavigationState.rerouting);
        sessions.last.next = onStep(0, route: rerouted);
        navigationRoutes.rerouted.single.answer.complete(rerouted);
        await tester.pumpAndSettle();
        expect(navigation.state, NavigationState.onRoute);
        // Still open, now with the new route's steps.
        expect(find.text('Turn right onto Union Street'), findsOneWidget);
        expect(listed('Turn left onto 3rd Avenue'), findsOneWidget);
        expect(listed('Your destination is on the left'), findsOneWidget);
        expect(find.text('Your destination is on the right'), findsNothing);
        // Until the time between new routes is up.
        await tester.pump(minRerouteInterval);
      });

      testWidgets('in landscape, stays in the start half', (tester) async {
        routes = FakeRoutes(immediately: [stepped, other]);
        navigateInTestTime();
        await start(tester);
        tester.view.physicalSize = appScreen.flipped * 2;
        await tester.pumpAndSettle();
        await fixOn(tester, 0);
        await openList(tester);
        final open = tester.getRect(find.byType(ManeuverBanner));
        const width = 800.0, height = 400.0;
        expect(open.left, 16);
        expect(open.right, 16 + (width - 32) / 2);
        expect(open.bottom, height - appNavigationBar);
        // The buttons, in the end half, are clear of it.
        expect(open.overlaps(tester.getRect(find.byTooltip('Mute'))), isFalse);
        expect(listed('Turn right onto 4th Avenue'), findsOneWidget);
      });
    });

    group('the mute button', () {
      testWidgets('stops speech and keeps quiet until unmuted', (tester) async {
        navigateInTestTime();
        await start(tester);
        await fix(tester, spoken: fakeSpoken('a', 'Turn left'));
        expect(speaker.said, ['Turn left']);
        await tester.tap(find.byTooltip('Mute'));
        await tester.pump();
        expect(navigation.muted, isTrue);
        expect(speaker.log.last, '(stop)');
        expect(find.byTooltip('Mute'), findsNothing);

        await fix(tester, spoken: fakeSpoken('b', 'Turn right'));
        await fix(tester, deviation: NavDeviation.offRoute);
        // Neither the announcement nor "Rerouting".
        expect(speaker.said, ['Turn left']);
        await fix(tester);
        expect(navigation.state, NavigationState.onRoute);

        await tester.tap(find.byTooltip('Unmute'));
        await tester.pump();
        expect(navigation.muted, isFalse);
        await fix(tester, spoken: fakeSpoken('c', 'Keep left'));
        expect(speaker.said, ['Turn left', 'Keep left']);
      });

      testWidgets('shows in the overview and once moved, as on iOS', (
        tester,
      ) async {
        navigateInTestTime();
        await start(tester);
        // From the start.
        expect(find.byTooltip('Mute'), findsOneWidget);
        await fix(tester);
        await tester.tap(find.byTooltip('Route overview'));
        await tester.pump();
        expect(find.byTooltip('Mute'), findsOneWidget);
        map.following.value = false;
        await tester.pump();
        expect(find.byTooltip('Mute'), findsOneWidget);
      });

      testWidgets('persists across trips', (tester) async {
        navigateInTestTime();
        await start(tester);
        await tester.tap(find.byTooltip('Mute'));
        await tester.pump();
        await tester.tap(find.byTooltip('End'));
        await tester.pumpAndSettle();
        await tester.tap(find.text('Start'));
        await tester.pumpAndSettle();
        expect(navigation.muted, isTrue);
        expect(find.byTooltip('Unmute'), findsOneWidget);
        expect(await store.settings.get(navigationMuted), isTrue);
      });
    });

    testWidgets('Start starts it on iOS, with Cupertino controls', (
      tester,
    ) async {
      await start(tester, platform: TargetPlatform.iOS);
      expect(navigation.state, NavigationState.acquiring);
      expect(
        find.descendant(
          of: find.bySemanticsLabel('End'),
          matching: find.byType(CupertinoButton),
        ),
        findsOneWidget,
      );
    });

    testWidgets('End goes back to directions, with the trip over', (
      tester,
    ) async {
      await start(tester);
      await tester.tap(find.byTooltip('End'));
      await tester.pumpAndSettle();
      expect(navigation.state, NavigationState.idle);
      expect(find.text('Start'), findsOneWidget);
      expect(find.byTooltip('End'), findsNothing);
      expect(lines(), hasLength(2));
      expect(features(navigationSource), isEmpty);
      expect(map.subject, isTrue);
    });

    testWidgets(
      'a trip ended from elsewhere, as by the notification, goes back too',
      (tester) async {
        await start(tester);
        // As the notification's End does, with no tap on the screen.
        unawaited(navigation.end());
        await tester.pumpAndSettle();
        expect(navigation.state, NavigationState.idle);
        expect(find.text('Start'), findsOneWidget);
        expect(find.byTooltip('End'), findsNothing);
        expect(features(navigationSource), isEmpty);
      },
    );

    testWidgets(
      'trip progress shows the time and distance left, in the units set',
      (tester) async {
        navigateInTestTime();
        await start(tester);
        await fix(tester, toManeuver: 250);
        // 1,250 m and 125 s left, the test's region being the US.
        expect(find.text('0.8 mi'), findsOneWidget);
        expect(find.text('2 min'), findsOneWidget);
        await store.settings.set(unitSystem, UnitSystem.metric);
        await tester.pumpAndSettle();
        expect(find.text('1.3 km'), findsOneWidget);
      },
    );

    testWidgets('arrived, trip progress stays, with nothing left, until End', (
      tester,
    ) async {
      navigateInTestTime();
      await start(tester);
      await fix(tester);
      sessions.last.next = fakeUpdate(status: TripStatus.complete);
      navigationLocation.send(fakeFix(4, 1));
      await tester.pumpAndSettle();
      expect(navigation.state, NavigationState.arrived);
      expect(find.text('<1 min'), findsOneWidget);
      expect(find.text('0 ft'), findsOneWidget);
      await tester.tap(find.byTooltip('End'));
      await tester.pumpAndSettle();
      expect(navigation.state, NavigationState.idle);
      expect(find.text('Start'), findsOneWidget);
    });

    testWidgets('ending it fits the routes again, flat and north up', (
      tester,
    ) async {
      await start(tester);
      // Directions' own fits keep whatever tilt and turn the map has.
      expect(map.subjectBearing, isNull);
      expect(map.subjectPitch, isNull);
      final before = map.subjectsSet;
      await tester.tap(find.byTooltip('End'));
      await tester.pumpAndSettle();
      expect(map.subjectsSet, greaterThan(before));
      expect(map.subjectBearing, 0);
      expect(map.subjectPitch, 0);
      final bounds = map.subjectBounds!;
      for (final p in [...best.geometry, ...other.geometry]) {
        expect(p.lng, inInclusiveRange(bounds.west, bounds.east));
        expect(p.lat, inInclusiveRange(bounds.south, bounds.north));
      }
    });

    group('the status card', () {
      Finder banner(String text) => find.descendant(
        of: find.byType(ManeuverBanner),
        matching: find.text(text),
      );
      const turn = 'Turn left onto Pike Street';
      final turnVisual = fakeVisual(
        fakeContent(
          turn,
          type: ManeuverType.turn,
          modifier: ManeuverModifier.left,
        ),
      );
      final stepped = fakeRoute(
        [
          [0.5, 2],
          [1, 4],
        ],
        navigationSteps: [
          fakeManeuverStep(turn, distance: 400),
          fakeManeuverStep('Turn right onto 4th Avenue', distance: 120),
          fakeNavStep(null),
        ],
      );

      testWidgets('says it waits for GPS, in the banner\'s place', (
        tester,
      ) async {
        navigateInTestTime();
        await start(tester);
        expect(navigation.state, NavigationState.acquiring);
        expect(banner('Waiting for GPS…'), findsOneWidget);
        // The buttons are laid out under it, as under the banner.
        final card = tester.getRect(find.byType(ManeuverBanner));
        expect(card.height, greaterThan(0));
        expect(
          tester.getRect(find.byTooltip('Mute')).top,
          greaterThan(card.bottom),
        );
        // A fix too poor to start from leaves it.
        navigationLocation.send(fakeFix(2.5, 0.6, accuracy: 500));
        await tester.pumpAndSettle();
        expect(banner('Waiting for GPS…'), findsOneWidget);
        await fix(tester, visual: turnVisual);
        expect(banner('Waiting for GPS…'), findsNothing);
        expect(banner(turn), findsOneWidget);
      });

      testWidgets(
        'off the route, then finding a new route, then the maneuver',
        (tester) async {
          routes = FakeRoutes(immediately: [stepped, other]);
          navigateInTestTime();
          await start(tester);
          await fix(tester, visual: turnVisual);
          expect(
            find.bySemanticsLabel('Show upcoming maneuvers'),
            findsOneWidget,
          );

          await fix(
            tester,
            visual: turnVisual,
            deviation: NavDeviation.offRoute,
          );
          expect(navigation.state, NavigationState.offRoute);
          expect(banner('Off route'), findsOneWidget);
          expect(banner(turn), findsNothing);
          // No handle, nor list, under a status card.
          expect(
            find.bySemanticsLabel('Show upcoming maneuvers'),
            findsNothing,
          );
          expect(find.byType(StepList), findsNothing);

          await tester.pump(offRouteHold);
          expect(navigation.state, NavigationState.rerouting);
          expect(banner('Finding a new route…'), findsOneWidget);
          expect(banner('Off route'), findsNothing);
          expect(
            find.descendant(
              of: find.byType(ManeuverBanner),
              matching: find.byType(CircularProgressIndicator),
            ),
            findsOneWidget,
          );
          expect(
            find.bySemanticsLabel('Show upcoming maneuvers'),
            findsNothing,
          );

          sessions.last.next = fakeUpdate(
            snapped: fakeFix(2.5, 0.6, course: 20, speed: 5),
            visual: turnVisual,
            toManeuver: 250,
          );
          navigationRoutes.rerouted.single.answer.complete(stepped);
          await tester.pumpAndSettle();
          expect(navigation.state, NavigationState.onRoute);
          expect(banner('Finding a new route…'), findsNothing);
          expect(banner(turn), findsOneWidget);
          expect(
            find.bySemanticsLabel('Show upcoming maneuvers'),
            findsOneWidget,
          );
          await tester.pump(minRerouteInterval);
        },
      );

      testWidgets('says why a new route failed, and that it keeps trying', (
        tester,
      ) async {
        navigateInTestTime();
        await start(tester);
        await fix(tester, visual: turnVisual);
        await fix(tester, visual: turnVisual, deviation: NavDeviation.offRoute);
        await tester.pump(offRouteHold);
        navigationRoutes.rerouted.single.answer.completeError(
          const DirectionsError(
            kind: DirectionsErrorKind.noRoute,
            message: 'no route',
          ),
        );
        await tester.pump();
        await tester.pump();
        expect(navigation.rerouteFailure, RerouteFailure.noRoute);
        expect(banner('No route from here'), findsOneWidget);
        expect(banner('Will keep trying'), findsOneWidget);
        expect(banner('Off route'), findsNothing);

        // Trying again, finding a new route comes first.
        await tester.pump(rerouteBackoff);
        expect(navigation.state, NavigationState.rerouting);
        expect(navigation.rerouteFailure, RerouteFailure.noRoute);
        expect(banner('Finding a new route…'), findsOneWidget);
        expect(banner('No route from here'), findsNothing);

        navigationRoutes.rerouted.last.answer.completeError(
          const OutsideOfflineArea(),
        );
        await tester.pump();
        await tester.pump();
        expect(banner('Outside the offline area'), findsOneWidget);
        expect(banner('Will keep trying'), findsOneWidget);

        // Back on the route, the maneuver again.
        await fix(tester, visual: turnVisual);
        expect(navigation.state, NavigationState.onRoute);
        expect(banner(turn), findsOneWidget);
        expect(banner('Outside the offline area'), findsNothing);
        await tester.pump(minRerouteInterval);
      });

      testWidgets(
        'location turned off comes first, and opens location settings',
        (tester) async {
          final location = FakeLocation(now: here);
          navigateInTestTime();
          await start(tester, location: location);
          await fix(tester, visual: turnVisual);
          await fix(
            tester,
            visual: turnVisual,
            deviation: NavDeviation.offRoute,
          );
          await tester.pump(offRouteHold);
          expect(navigation.state, NavigationState.rerouting);

          navigationLocation.fail(
            const NavigationLocationError(LocationProblem.off),
          );
          await tester.pump();
          expect(banner('Location is off'), findsOneWidget);
          expect(banner('Turn it on to keep navigating'), findsOneWidget);
          expect(banner('Finding a new route…'), findsNothing);
          await tester.tap(banner('Open settings'));
          await tester.pump();
          expect(location.locationSettingsOpened, 1);
          expect(location.appSettingsOpened, 0);

          // Until a fix comes again.
          navigationLocation.send(fakeFix(2.5, 0.6));
          await tester.pump();
          await tester.pump();
          expect(banner('Location is off'), findsNothing);
          expect(banner('Finding a new route…'), findsOneWidget);
          sessions.last.next = fakeUpdate(
            snapped: fakeFix(2.5, 0.6, course: 20, speed: 5),
            visual: turnVisual,
            toManeuver: 250,
          );
          navigationRoutes.rerouted.single.answer.complete(stepped);
          await tester.pumpAndSettle();
          expect(banner(turn), findsOneWidget);
          await tester.pump(minRerouteInterval);
        },
      );

      testWidgets('location not allowed opens the app\'s settings', (
        tester,
      ) async {
        final location = FakeLocation(now: here);
        navigateInTestTime();
        await start(tester, location: location);
        // Before the first fix too.
        navigationLocation.fail(
          const NavigationLocationError(LocationProblem.denied),
        );
        await tester.pumpAndSettle();
        expect(banner('Waiting for GPS…'), findsNothing);
        expect(banner('Location is turned off for Cardinal'), findsOneWidget);
        await tester.tap(banner('Open settings'));
        await tester.pump();
        expect(location.appSettingsOpened, 1);
        expect(location.locationSettingsOpened, 0);
      });

      testWidgets('location that failed tries again, listening again', (
        tester,
      ) async {
        navigateInTestTime();
        await start(tester);
        await fix(tester, visual: turnVisual);
        unawaited(navigationLocation.close());
        await tester.pumpAndSettle();
        expect(banner('Your location stopped updating'), findsOneWidget);
        expect(banner('Open settings'), findsNothing);
        final listened = navigationLocation.modes.length;
        await tester.tap(banner('Try again'));
        await tester.pumpAndSettle();
        expect(navigationLocation.modes, hasLength(listened + 1));
        expect(navigationLocation.listening, isTrue);
        // It stays until a fix comes.
        expect(banner('Your location stopped updating'), findsOneWidget);
        await fix(tester, visual: turnVisual);
        expect(banner('Your location stopped updating'), findsNothing);
        expect(banner(turn), findsOneWidget);
      });

      testWidgets('is Cupertino on iOS', (tester) async {
        navigateInTestTime();
        await start(tester, platform: TargetPlatform.iOS);
        await fix(tester, visual: turnVisual);
        await fix(tester, visual: turnVisual, deviation: NavDeviation.offRoute);
        await tester.pump(offRouteHold);
        expect(
          find.descendant(
            of: find.byType(ManeuverBanner),
            matching: find.byType(CupertinoActivityIndicator),
          ),
          findsOneWidget,
        );
        sessions.last.next = fakeUpdate(
          snapped: fakeFix(2.5, 0.6, course: 20, speed: 5),
          visual: turnVisual,
          toManeuver: 250,
        );
        navigationRoutes.rerouted.single.answer.complete(stepped);
        await tester.pumpAndSettle();
        await tester.pump(minRerouteInterval);
        unawaited(navigationLocation.close());
        await tester.pumpAndSettle();
        expect(
          find.descendant(
            of: find.byType(ManeuverBanner),
            matching: find.byType(CupertinoButton),
          ),
          findsOneWidget,
        );
      });
    });

    group('GPS signal lost', () {
      testWidgets('shows once no fix has come for a while, and goes with one', (
        tester,
      ) async {
        navigateInTestTime();
        await start(tester);
        // Not while waiting for the first: the card says so.
        await tester.pump(staleFixAge * 2);
        expect(find.text('GPS signal lost'), findsNothing);
        await fix(tester);
        // About then: settling the fix took some time of its own.
        await tester.pump(staleFixAge - const Duration(seconds: 1));
        expect(find.text('GPS signal lost'), findsNothing);
        await tester.pump(const Duration(seconds: 1));
        expect(find.text('GPS signal lost'), findsOneWidget);
        // Under the map buttons, at the end side, clear of the chevron's half.
        final chip = tester.getRect(find.text('GPS signal lost'));
        final mute = tester.getRect(find.byTooltip('Mute'));
        expect(chip.top, greaterThan(mute.bottom));
        expect(chip.bottom, lessThan(appScreen.height / 2));
        // A fix, however poor, and it's gone, until the next wait.
        navigationLocation.send(
          fakeFix(2.5, 0.6, accuracy: 500, timestampMs: 1763000009000),
        );
        await tester.pump();
        await tester.pump();
        expect(find.text('GPS signal lost'), findsNothing);
        await tester.pump(staleFixAge);
        expect(find.text('GPS signal lost'), findsOneWidget);
      });

      testWidgets('gives way to a location problem\'s card', (tester) async {
        navigateInTestTime();
        await start(tester);
        await fix(tester);
        await tester.pump(staleFixAge);
        expect(find.text('GPS signal lost'), findsOneWidget);
        navigationLocation.fail(
          const NavigationLocationError(LocationProblem.off),
        );
        await tester.pump();
        expect(find.text('GPS signal lost'), findsNothing);
        expect(find.text('Location is off'), findsOneWidget);
      });
    });

    group('arrival', () {
      Future<void> arrive(WidgetTester tester) async {
        sessions.last.next = fakeUpdate(
          status: TripStatus.complete,
          snapped: fakeFix(4, 1, course: 20),
        );
        navigationLocation.send(fakeFix(4, 1));
        await tester.pumpAndSettle();
        expect(navigation.state, NavigationState.arrived);
      }

      testWidgets('says you\'ve arrived, and where', (tester) async {
        final arriving = fakeRoute(
          [
            [0.5, 2],
            [1, 4],
          ],
          navigationSteps: [
            fakeManeuverStep('Turn left onto Pike Street'),
            fakeManeuverStep(
              'Your destination is on the right',
              type: ManeuverType.arrive,
              modifier: ManeuverModifier.right,
            ),
            fakeNavStep(null),
          ],
        );
        routes = FakeRoutes(immediately: [arriving, other]);
        navigateInTestTime();
        await start(tester);
        expect(navigation.destinationName, 'Chu Minh');
        await fix(tester, visual: fakeVisual(fakeContent('Turn left')));
        await arrive(tester);
        final inBanner = find.byType(ManeuverBanner);
        expect(
          find.descendant(of: inBanner, matching: find.text('You\'ve arrived')),
          findsOneWidget,
        );
        expect(
          find.descendant(of: inBanner, matching: find.text('Chu Minh')),
          findsOneWidget,
        );
        // The route's own arrival icon, on the right.
        final icon = tester.widget<Icon>(
          find.descendant(of: inBanner, matching: find.byType(Icon)),
        );
        expect(
          icon.icon,
          maneuverIcon(ManeuverType.arrive, ManeuverModifier.right),
        );
        expect(find.bySemanticsLabel('Show upcoming maneuvers'), findsNothing);
        // Trip progress keeps End; the road name goes.
        expect(find.byTooltip('End'), findsOneWidget);
        expect(find.byType(RoadNamePill), findsOneWidget);
        expect(
          tester.widget<RoadNamePill>(find.byType(RoadNamePill)).name,
          isNull,
        );
        // However long it's left there, the GPS is no news.
        await tester.pump(staleFixAge * 2);
        expect(find.text('GPS signal lost'), findsNothing);
      });

      testWidgets(
        'stops following and eases to the destination, then recenters',
        (tester) async {
          navigateInTestTime();
          await start(tester);
          await fix(tester);
          expect(map.following.value, isTrue);
          await arrive(tester);
          expect(map.following.value, isFalse);
          // The route's end, at the zoom it was last followed at, flat and north up.
          expect(map.subjectBounds, (west: 1, south: 4, east: 1, north: 4));
          expect(map.subjectMaxZoom, map.follows.last.zoom);
          expect(map.subjectBearing, 0);
          expect(map.subjectPitch, 0);
          // Clear of the arrival card, as the overview is of the banner, and of trip
          // progress, less what the map's own padding covers.
          final card = tester.getRect(find.byType(ManeuverBanner));
          final progress = tester.getRect(find.byType(TripProgressPill));
          final sheet = SheetScaffold.viewOf(
            tester.element(find.byType(TripProgressPill)),
          ).obscured;
          expect(map.subjectClear!.top, card.bottom - sheet.top);
          expect(
            map.subjectClear!.bottom,
            math.max(0, appScreen.height - progress.top - sheet.bottom),
          );
          expect(find.byTooltip('Recenter'), findsOneWidget);

          await tester.tap(find.byTooltip('Recenter'));
          await tester.pump();
          expect(map.following.value, isTrue);
          expect(navigation.state, NavigationState.arrived);
          expect(find.byTooltip('Route overview'), findsOneWidget);
          // The overview, from there, shows the destination again.
          await tester.tap(find.byTooltip('Route overview'));
          await tester.pump();
          expect(map.subjectBounds, (west: 1, south: 4, east: 1, north: 4));
        },
      );
    });

    testWidgets('back ends the trip too', (tester) async {
      await start(tester);
      final back = tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      await back;
      expect(navigation.state, NavigationState.idle);
      expect(find.text('Start'), findsOneWidget);
    });
  });

  testWidgets(
    'the map shows the routes and the destination, the best selected',
    (tester) async {
      await open(tester);
      final drawn = features(routeSource);
      final lines = drawn.where(
        (f) => (f['properties']! as Map)['role'] == 'line',
      );
      expect(lines.map((f) => (f['properties']! as Map)['selected']), [
        true,
        false,
      ]);
      expect(
        drawn.where((f) => (f['properties']! as Map)['role'] == 'pin'),
        hasLength(1),
      );
      expect(map.subject, isTrue);
    },
  );

  testWidgets('each route has a callout, and tapping one selects it', (
    tester,
  ) async {
    await open(tester);
    await tester.pump(const Duration(milliseconds: 300));
    final callouts = features(routeCalloutSource);
    expect(callouts.map((f) => (f['properties']! as Map)['duration']), [
      '11 min',
      '15 min',
    ]);

    // The map doesn't report taps on features without IDs.
    expect(callouts.map((f) => f['id']), [1, 2]);
    expect(selectedRows(tester), [true, false]);
    final before = routes.asked.length;

    routeTaps.add(1);
    await tester.pumpAndSettle();
    expect(selectedRows(tester), [false, true]);
    expect(
      features(routeSource)
          .where((f) => (f['properties']! as Map)['role'] == 'line')
          .map((f) => (f['properties']! as Map)['selected']),
      [false, true],
    );
    expect(routes.asked, hasLength(before));
  });

  group('a fling of the sheet', () {
    const footer = SheetAnchor.footer();
    const header = DirectionsPage.headerAnchor;

    SheetController sheetOf(WidgetTester tester) =>
        SheetScaffold.controllerOf(tester.element(find.text('From')));

    double extentOf(SheetController sheet, SheetAnchor anchor) =>
        sheet.anchors.firstWhere((a) => a.anchor == anchor).extent;

    ScrollPosition listOf(WidgetTester tester) => tester
        .state<ScrollableState>(
          find
              .descendant(
                of: find.byType(DirectionsPage),
                matching: find.byType(Scrollable),
              )
              .first,
        )
        .position;

    /// Flings from [from] by [dy] at [speed], and says where the sheet was let go.
    Future<double> fling(
      WidgetTester tester,
      Offset from,
      double dy,
      double speed,
    ) async {
      await tester.flingFrom(from, Offset(0, dy), speed);
      final released = sheetOf(tester).extent;
      await tester.pumpAndSettle();
      return released;
    }

    /// The handle, and the row held under it, with the sheet where it is.
    Offset handle(WidgetTester tester) => Offset(
      appScreen.width / 2,
      appScreen.height - sheetOf(tester).extent + 8,
    );
    Offset heldRow(WidgetTester tester) => handle(tester) + const Offset(0, 48);

    testWidgets('the header anchor holds as hard as the page says', (
      tester,
    ) async {
      await open(tester);
      final sheet = sheetOf(tester);
      expect(sheet.anchors.map((a) => a.anchor), [footer, header]);
      expect(sheet.anchors.last.gravity, DirectionsPage.headerGravity);
      expect(DirectionsPage.headerGravity, 2400);
      expect(sheet.anchors.first.gravity, sheet.physics.gravity);
      expect(sheet.ranges.single.range.from, header);
    });

    testWidgets('down from the list at 6000 goes to the lowest anchor', (
      tester,
    ) async {
      await open(tester);
      final sheet = sheetOf(tester);
      expect(sheet.restingIn, isNotNull);
      final released = await fling(
        tester,
        tester.getCenter(find.text('From')),
        60,
        6000,
      );
      // Still faster than the header holds when it gets there.
      expect(
        sheet.physics.speedAt(released, -6000, extentOf(sheet, header)),
        greaterThan(2 * DirectionsPage.headerGravity),
      );
      expect(sheet.restingAt, footer);
      expect(sheet.extent, extentOf(sheet, footer));
    });

    testWidgets(
      'down from as high as the sheet goes at 6000 goes to the lowest '
      'anchor',
      (tester) async {
        await open(tester);
        final sheet = sheetOf(tester);
        sheet.animateTo(const SheetAnchor.full());
        await tester.pumpAndSettle();
        expect(sheet.isAtMax, isTrue);
        final released = await fling(
          tester,
          tester.getCenter(find.text('From')),
          60,
          6000,
        );
        expect(
          sheet.physics.speedAt(released, -6000, extentOf(sheet, header)),
          greaterThan(DirectionsPage.headerGravity + 500),
        );
        expect(sheet.restingAt, footer);
      },
    );

    testWidgets('down from the list at 1000 stops at the header anchor', (
      tester,
    ) async {
      await open(tester);
      final sheet = sheetOf(tester);
      final released = await fling(
        tester,
        tester.getCenter(find.text('From')),
        60,
        1000,
      );
      // It would have carried past the header, and gets there slower than it holds:
      // as any fling let go slower than that does.
      expect(
        sheet.physics.landing(released, -1000),
        lessThan(extentOf(sheet, header) - 50),
      );
      expect(
        sheet.physics.speedAt(released, -1000, extentOf(sheet, header)),
        lessThan(DirectionsPage.headerGravity),
      );
      expect(sheet.restingAt, header);
      expect(sheet.extent, extentOf(sheet, header));
    });

    testWidgets('up from the lowest anchor at 6000 goes into the list, as far '
        'up as the sheet goes', (tester) async {
      await open(tester);
      final sheet = sheetOf(tester);
      sheet.animateTo(footer);
      await tester.pumpAndSettle();
      expect(sheet.restingAt, footer);

      // By the handle, which moves only the sheet.
      final released = await fling(tester, handle(tester), -60, 6000);
      expect(
        sheet.physics.speedAt(released, 6000, extentOf(sheet, header)),
        greaterThan(2 * DirectionsPage.headerGravity),
      );
      expect(sheet.restingAt, isNull);
      expect(sheet.restingIn, sheet.ranges.single.range);
      expect(sheet.extent, appScreen.height);
      expect(listOf(tester).pixels, 0);

      // By the held row, which is the content's: the same, and what speed is left at
      // the top is the list's to scroll with.
      sheet.animateTo(footer);
      await tester.pumpAndSettle();
      await fling(tester, heldRow(tester), -60, 6000);
      expect(sheet.restingIn, sheet.ranges.single.range);
      expect(sheet.extent, appScreen.height);
      expect(listOf(tester).pixels, listOf(tester).maxScrollExtent);
    });

    testWidgets('up from the lowest anchor at 3000 passes the header without '
        'stopping, only slowing', (tester) async {
      await open(tester);
      final sheet = sheetOf(tester);
      sheet.animateTo(footer);
      await tester.pumpAndSettle();

      await tester.flingFrom(handle(tester), const Offset(0, -60), 3000);
      final released = sheet.extent;
      expect(
        sheet.physics.speedAt(released, 3000, extentOf(sheet, header)),
        greaterThan(DirectionsPage.headerGravity + 200),
      );
      // The frame its motion starts on, and then every frame until it rests.
      await tester.pump();
      final extents = [released];
      for (var i = 0; i < 600 && !sheet.isResting; i++) {
        await tester.pump(const Duration(milliseconds: 16));
        extents.add(sheet.extent);
      }
      expect(sheet.restingIn, sheet.ranges.single.range);
      expect(sheet.extent, appScreen.height);
      // Over the header anchor and on up without stopping, only slowing, until the
      // top stops it.
      final moving = [
        for (final extent in extents)
          if (extent < appScreen.height - 1) extent,
      ];
      expect(moving.where((e) => e < extentOf(sheet, header)), isNotEmpty);
      expect(moving.where((e) => e > extentOf(sheet, header)), isNotEmpty);
      var last = double.infinity;
      for (var i = 1; i < moving.length; i++) {
        final step = moving[i] - moving[i - 1];
        expect(step, greaterThan(0), reason: 'frame $i stands still');
        expect(step, lessThanOrEqualTo(last + 0.01), reason: 'frame $i');
        last = step;
      }
    });

    testWidgets(
      'up from the lowest anchor at 1000 stops at the header anchor',
      (tester) async {
        await open(tester);
        final sheet = sheetOf(tester);
        sheet.animateTo(footer);
        await tester.pumpAndSettle();
        final released = await fling(tester, heldRow(tester), -60, 1000);
        expect(
          sheet.physics.landing(released, 1000),
          greaterThan(extentOf(sheet, header) + 50),
        );
        expect(
          sheet.physics.speedAt(released, 1000, extentOf(sheet, header)),
          lessThan(DirectionsPage.headerGravity),
        );
        expect(sheet.restingAt, header);
        expect(sheet.extent, extentOf(sheet, header));
      },
    );
  });

  testWidgets('directions hide the map overlays until they close', (
    tester,
  ) async {
    await open(tester);
    expect(overlays.shown, isFalse);

    await tester.tap(find.byTooltip('Close'));
    await tester.pumpAndSettle();
    expect(overlays.shown, isTrue);
  });

  testWidgets("couldn't get directions says so", (tester) async {
    routes.immediately = null;
    await open(tester, settle: false);
    asked(TravelMode.driving).single.answer
        .completeError(StateError('offline'));
    await tester.pumpAndSettle();
    expect(find.text("Couldn't get directions. Try again."), findsOneWidget);
  });

  testWidgets('closing goes back to the place, which the map shows again', (
    tester,
  ) async {
    await open(tester);
    expect(features(selectionSource), isEmpty);

    await tester.tap(find.byTooltip('Close'));
    await tester.pumpAndSettle();
    expect(find.text('Start'), findsNothing);
    expect(features(selectionSource), isNotEmpty);
    expect(map.subject, isTrue);
    expect(features(routeSource), isEmpty);
    expect(features(routeCalloutSource), isEmpty);
  });

  group('transit', () {
    /// Opens the selected journey's Trip details.
    Future<void> openDetails(WidgetTester tester) async {
      await tester.tap(find.widgetWithText(FilledButton, 'Trip details'));
      await tester.pumpAndSettle();
    }

    /// Chooses [way] from the menu of how stops are reached.
    Future<void> chooseAccess(WidgetTester tester, String way) async {
      await tester.tap(find.text('Walking to stops'));
      await tester.pumpAndSettle();
      await tester.tap(find.text(way).last);
      await tester.pumpAndSettle();
    }

    /// Shows transit's journeys, with the sheet pulled up to show them all.
    Future<void> openTransit(WidgetTester tester) async {
      await open(tester);
      await tester.tap(find.byIcon(Icons.directions_transit));
      await tester.pumpAndSettle();
      SheetScaffold.controllerOf(tester.element(find.text('From')))
          .animateTo(const SheetAnchor.full());
      await tester.pumpAndSettle();
    }

    testWidgets('lists the journeys as timelines, and remembers the mode', (
      tester,
    ) async {
      await openTransit(tester);
      expect(transit.asked.single.access, TransitAccess.walking);
      expect(await store.settings.get(lastTravelMode), TravelMode.transit);
      expect(find.byType(JourneyRow), findsNWidgets(2));
      expect(
        find.byWidgetPredicate(
          (w) =>
              w is Semantics && w.properties.label == '40, then 7, then 1 Line',
        ),
        findsNWidgets(2),
      );
      expect(find.textContaining('5:30 PM – 6:15 PM'), findsOneWidget);
      expect(find.text('45 min'), findsNWidgets(2));
      // Feeds, not messages: one of the two feeds is live.
      expect(find.text('Live times from 1 of 2 feeds'), findsOneWidget);
      // The walk to a moved stop is flagged in the overview.
      expect(find.byIcon(Icons.wrong_location), findsNWidgets(2));
    });

    testWidgets('draws the selected journey on the map', (tester) async {
      await openTransit(tester);
      final drawn = features(transitSource);
      final rides = drawn.where(
        (f) => (f['properties']! as Map)['role'] == 'ride',
      );
      expect(rides.map((f) => (f['properties']! as Map)['color']), [
        '#FDB71A',
        '#28813F',
      ]);
      expect(
        drawn.where((f) => (f['properties']! as Map)['role'] == 'street'),
        hasLength(1),
      );
      expect(
        features(routeSource)
            .where((f) => (f['properties']! as Map)['role'] == 'line'),
        isEmpty,
      );
    });

    group('a departure chosen instead of the plan\'s', () {
      /// The lines the map draws for the journey's rides, as `[lng, lat]` points.
      List<List<Object?>> rideLines() => [
        for (final f in features(transitSource))
          if ((f['properties']! as Map)['role'] == 'ride')
            (f['geometry']! as Map)['coordinates'] as List<Object?>,
      ];
      const planned = [
        [0.6, 2.1],
        [0.8, 3.0],
        [0.9, 3.2],
      ];
      const later = [
        [0.6, 2.1],
        [0.7, 2.6],
        [0.9, 3.2],
      ];
      const last = [
        [0.6, 2.1],
        [0.65, 2.9],
        [0.9, 3.2],
      ];

      Future<void> openJourney(WidgetTester tester) async {
        await openTransit(tester);
        await openDetails(tester);
      }

      Future<void> choose(WidgetTester tester, String trip) async {
        final tile = find.byWidgetPredicate(
          (w) => w is ConnectionTile && w.departure.tripId == trip,
        );
        await tester.ensureVisible(tile);
        await tester.pumpAndSettle();
        await tester.tap(tile);
        await tester.pump();
      }

      testWidgets('is drawn along its own line once the planner has it', (
        tester,
      ) async {
        await openJourney(tester);
        expect(rideLines().first, planned);
        expect(transit.rides, isEmpty, reason: 'the plan has its own legs');

        await choose(tester, 't40c');
        final question = transit.rides.single;
        expect(question.query, (
          feed: 'us-wa-Metro-Transit',
          boardStopId: 'Fremont Ave N & N 34th St',
          alightStopId: 'Westlake Station',
          tripId: 't40c',
          startDate: '20261109',
          departure: DateTime.fromMillisecondsSinceEpoch(
            (fakeStart + 1320) * 1000,
          ),
        ));
        expect(question.access, TransitAccess.walking);
        // Until then, along the plan's line between the same two stops.
        expect(rideLines().first, planned);

        question.answer.complete(
          fakeRideLeg('t40c', later, departure: fakeStart + 1320),
        );
        await tester.pump();
        expect(rideLines().first, later);
        // The other ride and the walks are as they were.
        expect(rideLines(), hasLength(2));

        // Back to the plan's own, the plan's line; and again, without asking again.
        await choose(tester, 't40');
        expect(rideLines().first, planned);
        await choose(tester, 't40c');
        expect(rideLines().first, later);
        expect(transit.rides, hasLength(1));
      });

      testWidgets('lists its stops once the planner has it', (tester) async {
        await openJourney(tester);
        await choose(tester, 't40c');
        await tester.pumpAndSettle();
        // The plan's trip went on as the 7; this one's ends are all that's known.
        expect(find.text('Stay on board'), findsNothing);
        transit.rides.single.answer.complete(
          fakeRideLeg(
            't40c',
            later,
            departure: fakeStart + 1320,
            from: 'Fremont Ave N & N 34th St',
            to: 'Westlake Station',
            between: const ['Aurora Bridge', 'Dexter Ave N'],
          ),
        );
        await tester.pumpAndSettle();
        await tester.ensureVisible(find.textContaining('2 stops · '));
        await tester.tap(find.textContaining('2 stops · '));
        await tester.pumpAndSettle();
        expect(find.text('Aurora Bridge'), findsOneWidget);
        expect(find.text('Dexter Ave N'), findsOneWidget);
      });

      testWidgets('ignores an answer for one chosen before it', (tester) async {
        await openJourney(tester);
        await choose(tester, 't40c');
        await choose(tester, 't40e');
        final [first, second] = transit.rides;
        expect(first.query.tripId, 't40c');
        expect(first.aborted, isTrue, reason: 'no longer wanted');
        expect(second.query.tripId, 't40e');
        expect(second.aborted, isFalse);

        first.answer.complete(
          fakeRideLeg('t40c', later, departure: fakeStart + 1320),
        );
        await tester.pump();
        expect(
          rideLines().first,
          planned,
          reason: 'not the 17:52, which is gone',
        );

        second.answer.complete(
          fakeRideLeg(
            't40e',
            last,
            departure: fakeStart + 2040,
            route: {
              'route_id': '100062',
              'short_name': '62',
              'color': '00857C',
            },
          ),
        );
        await tester.pump();
        expect(rideLines().first, last);
        final ride = features(transitSource)
            .firstWhere((f) => (f['properties']! as Map)['role'] == 'ride');
        expect((ride['properties']! as Map)['color'], '#00857C');
      });

      /// Chooses the 17:52, and has its leg arrive.
      Future<void> chooseLater(WidgetTester tester) async {
        await choose(tester, 't40c');
        transit.rides.last.answer.complete(
          fakeRideLeg('t40c', later, departure: fakeStart + 1320),
        );
        await tester.pumpAndSettle();
        expect(rideLines().first, later);
      }

      /// The departures the open journey takes.
      ConnectionChain shownChain(WidgetTester tester) => tester
          .widget<JourneyHeader>(
            find.byType(JourneyHeader, skipOffstage: false),
          )
          .chain;

      testWidgets('re-times the rest of the journey', (tester) async {
        await openJourney(tester);
        expect(shownChain(tester).suggested, isTrue);
        final leaving = shownChain(tester).departure;
        await choose(tester, 't40c');
        await tester.pumpAndSettle();
        final chain = shownChain(tester);
        expect(chain.rides.first.departure?.tripId, 't40c');
        expect(chain.suggested, isFalse);
        // Later, so it's time to leave later.
        expect(chain.departure.isAfter(leaving), isTrue);
      });

      testWidgets('is drawn no more once back at the journeys', (tester) async {
        await openJourney(tester);
        await chooseLater(tester);
        // The header is scrolled out of view.
        await tester.binding.handlePopRoute();
        await tester.pumpAndSettle();
        expect(rideLines().first, planned);
      });

      testWidgets('is the plan\'s again when the journey opens again, and its '
          'leg isn\'t asked for again', (tester) async {
        await openJourney(tester);
        await chooseLater(tester);
        await tester.binding.handlePopRoute();
        await tester.pumpAndSettle();

        await openDetails(tester);
        expect(shownChain(tester).suggested, isTrue);
        expect(rideLines().first, planned);
        await choose(tester, 't40c');
        expect(rideLines().first, later);
        expect(transit.rides, hasLength(1));
      });

      testWidgets('a new plan returns to the journeys, where they were left', (
        tester,
      ) async {
        await openTransit(tester);
        final sheet = SheetScaffold.controllerOf(
          tester.element(find.text('From')),
        );
        final extent = sheet.extent;
        await openDetails(tester);
        await chooseLater(tester);

        // Getting to stops follows the default, so transit is asked again.
        await store.settings.set(wheelchairByDefault, true);
        await tester.pumpAndSettle();
        expect(transit.asked.last.access, TransitAccess.wheelchair);
        expect(find.byType(JourneyHeader), findsNothing);
        expect(find.byType(JourneyRow), findsNWidgets(2));
        expect(sheet.extent, moreOrLessEquals(extent));
        // The new plan's journey, as the plan has it.
        expect(rideLines().first, planned);
      });

      testWidgets('with no journey in the new plan, returns to the journeys, '
          'which say why', (tester) async {
        await openJourney(tester);
        transit.immediately = emptyPlan('no_egress');
        await store.settings.set(wheelchairByDefault, true);
        await tester.pumpAndSettle();
        expect(find.byType(JourneyHeader), findsNothing);
        expect(find.byType(JourneyRow), findsNothing);
        expect(find.textContaining('No stops within'), findsOneWidget);
        expect(rideLines(), isEmpty);
      });
    });

    group('Trip details', () {
      SheetController sheetOf(WidgetTester tester) =>
          SheetScaffold.controllerOf(
            tester.element(find.byType(ModeTabs, skipOffstage: false)),
          );

      /// How far [page]'s content is scrolled.
      double scrolled(WidgetTester tester, Type page) => tester
          .state<ScrollableState>(
            find
                .descendant(
                  of: find.byType(page),
                  matching: find.byType(Scrollable),
                )
                .first,
          )
          .position
          .pixels;

      /// Shows ten journeys, full, the list scrolled some way down.
      Future<GoRouter> openScrolled(WidgetTester tester) async {
        transit.immediately = fakePlan(journeys: 10);
        final router = await open(tester);
        await tester.tap(find.byIcon(Icons.directions_transit));
        await tester.pumpAndSettle();
        sheetOf(tester).animateTo(const SheetAnchor.full());
        await tester.pumpAndSettle();
        await tester.drag(find.byType(JourneyRow).at(2), const Offset(0, -150));
        await tester.pumpAndSettle();
        expect(scrolled(tester, DirectionsPage), greaterThan(0));
        return router;
      }

      /// Selects the fifth journey, and taps Trip details.
      Future<void> openJourney(WidgetTester tester) async {
        await tester.tap(find.byType(JourneyRow).at(4).hitTestable());
        await tester.pumpAndSettle();
        await tester.tap(find.widgetWithText(FilledButton, 'Trip details'));
      }

      testWidgets('opens from the journey\'s top, with it from its first '
          'frame, at 40%', (tester) async {
        final router = await openScrolled(tester);
        await openJourney(tester);
        // The page is built a frame or so after the tap, and has the journey from
        // the first frame it's built in.
        for (var frames = 0; frames < 3; frames++) {
          await tester.pump();
          if (find.byType(TripDetailsPage).evaluate().isNotEmpty) break;
        }
        expect(find.byType(TripDetailsPage), findsOneWidget);
        expect(find.byType(JourneyHeader), findsOneWidget);
        expect(find.byType(JourneyItinerary), findsOneWidget);
        await tester.pumpAndSettle();
        expect(
          router.state.uri.toString(),
          tripDetailsLocation(destination.id),
        );
        expect(sheetOf(tester).restingAt, const SheetAnchor.fraction(0.4));
        expect(scrolled(tester, TripDetailsPage), 0);
        // From its top: its back button, and when to go.
        expect(
          find.descendant(
            of: find.byType(JourneyHeader),
            matching: find.byTooltip('Back'),
          ),
          findsOneWidget,
        );
        expect(
          find.descendant(
            of: find.byType(JourneyHeader),
            matching: find.textContaining('Go '),
          ),
          findsWidgets,
        );
      });

      for (final how in ['its back button', 'the system\'s back']) {
        testWidgets('back by $how returns to the journeys, the sheet and the '
            'list where they were left', (tester) async {
          final router = await openScrolled(tester);
          final sheet = sheetOf(tester);
          // Lowered into the free range, the list still scrolled.
          await tester.drag(find.text('From'), const Offset(0, 100));
          await tester.pumpAndSettle();
          expect(sheet.restingIn, isNotNull);
          // The fifth journey selected, its row where it's held from here on.
          await tester.tap(find.byType(JourneyRow).at(4).hitTestable());
          await tester.pumpAndSettle();
          final extent = sheet.extent;
          final offset = scrolled(tester, DirectionsPage);
          expect(offset, greaterThan(0));
          List<Rect> rows() => [
            for (var i = 0; i < find.byType(JourneyRow).evaluate().length; i++)
              tester.getRect(find.byType(JourneyRow).at(i)),
          ];
          final before = rows();

          await openJourney(tester);
          await tester.pumpAndSettle();
          if (how == 'its back button') {
            await tester.tap(find.byTooltip('Back'));
          } else {
            await tester.binding.handlePopRoute();
          }
          await tester.pumpAndSettle();
          expect(
            router.state.uri.toString(),
            directionsLocation(destination.id),
          );
          expect(sheet.extent, moreOrLessEquals(extent));
          expect(scrolled(tester, DirectionsPage), offset);
          expect(rows(), before);
        });
      }

      testWidgets('restored without its journey is directions', (tester) async {
        final router = await open(tester);
        router.go(tripDetailsLocation(destination.id));
        await tester.pumpAndSettle();
        expect(router.state.uri.toString(), directionsLocation(destination.id));
      });
    });

    testWidgets('a journey opens in detail, and back returns to the list', (
      tester,
    ) async {
      await openTransit(tester);
      await openDetails(tester);
      expect(find.byType(TripDetailsPage), findsOneWidget);
      expect(
        find.text('Board at the temporary stop 50 feet north.'),
        findsOneWidget,
      );
      expect(find.text('Route 40 detoured'), findsOneWidget);
      await tester.tap(find.byTooltip('Back'));
      await tester.pumpAndSettle();
      expect(find.byType(TripDetailsPage), findsNothing);
      expect(find.byType(JourneyRow), findsWidgets);
    });

    group('the sheet', () {
      SheetController sheetOf(WidgetTester tester) =>
          SheetScaffold.controllerOf(tester.element(find.text('From')));
      const header = DirectionsPage.headerAnchor;

      testWidgets('rests as for the other ways, and is free to move', (
        tester,
      ) async {
        await store.settings.set(lastTravelMode, TravelMode.transit);
        await open(tester);
        final sheet = sheetOf(tester);
        expect(sheet.isDraggable, isTrue);
        expect(sheet.anchors.map((a) => a.anchor), [
          const SheetAnchor.footer(),
          header,
        ]);
        expect(sheet.ranges, hasLength(1), reason: 'free from the header up');
        // Its own button, and none of a street way's.
        expect(find.text('Trip details'), findsOneWidget);
        expect(find.text('Start'), findsNothing);
        expect(find.text('Steps'), findsNothing);
        await tester.fling(find.text('From'), const Offset(0, 300), 3000);
        await tester.pumpAndSettle();
        expect(sheet.restingAt, isNotNull);
        expect(sheet.extent, lessThan(appScreen.height * 0.5));
      });

      testWidgets('comes down to 40% for a journey, so the map shows it', (
        tester,
      ) async {
        await openTransit(tester);
        final sheet = sheetOf(tester);

        await openDetails(tester);
        expect(sheet.restingAt, const SheetAnchor.fraction(0.4));
        expect(sheet.isDraggable, isTrue);
        expect(map.subject, isTrue);

        // It can be pulled up to scroll through the journey, resting wherever it's
        // let go on the way: full is the top of its free range, not an anchor.
        expect(sheet.anchors.map((a) => a.anchor), [
          const SheetAnchor.fraction(0.4),
        ]);
        sheet.animateTo(const SheetAnchor.full());
        await tester.pumpAndSettle();
        expect(sheet.restingIn, isNotNull);
        expect(sheet.isAtMax, isTrue);
      });

      testWidgets('back from a journey rests where it was left, and is free to '
          'move', (tester) async {
        await openTransit(tester);
        final sheet = sheetOf(tester);
        // Somewhere in the free range, below full.
        await tester.drag(find.text('From'), const Offset(0, 120));
        await tester.pumpAndSettle();
        expect(sheet.restingIn, isNotNull);
        expect(sheet.isAtMax, isFalse);
        final extent = sheet.extent;
        await openDetails(tester);

        await tester.tap(find.byTooltip('Back'));
        await tester.pumpAndSettle();
        expect(sheet.extent, moreOrLessEquals(extent));
        expect(sheet.restingIn, isNotNull);
        expect(sheet.isDraggable, isTrue);
      });

      testWidgets('another way has its row held again, and its buttons', (
        tester,
      ) async {
        await openTransit(tester);
        final sheet = sheetOf(tester);

        await tester.tap(find.byTooltip(RegExp('^Walking')));
        await tester.pumpAndSettle();
        expect(sheet.isDraggable, isTrue);
        expect(sheet.anchors, hasLength(2));
        expect(sheet.ranges, hasLength(1));
        expect(find.text('Start'), findsOneWidget);
        expect(find.byType(OptionRow), findsNWidgets(2));
      });
    });

    testWidgets('how you get to stops is asked with and remembered', (
      tester,
    ) async {
      await openTransit(tester);
      await chooseAccess(tester, 'Wheelchair');
      expect(transit.asked.last.access, TransitAccess.wheelchair);
      expect(
        await store.settings.get(lastTransitAccess),
        TransitAccess.wheelchair,
      );
    });

    group('getting to stops', () {
      for (final wheelchair in [false, true]) {
        final flag = wheelchair ? 'on' : 'off';

        testWidgets('as remembered, with wheelchair by default $flag', (
          tester,
        ) async {
          await store.settings.set(wheelchairByDefault, wheelchair);
          await store.settings.set(lastTransitAccess, TransitAccess.cycling);
          await openTransit(tester);
          expect(transit.asked.single.access, TransitAccess.cycling);
        });

        testWidgets('unremembered, with wheelchair by default $flag', (
          tester,
        ) async {
          await store.settings.set(wheelchairByDefault, wheelchair);
          await openTransit(tester);
          expect(
            transit.asked.single.access,
            wheelchair ? TransitAccess.wheelchair : TransitAccess.walking,
          );
          // The default isn't remembered as a choice.
          expect(await store.settings.get(lastTransitAccess), isNull);
        });
      }

      testWidgets('unremembered, follow the default as it changes', (
        tester,
      ) async {
        await openTransit(tester);
        expect(transit.asked.single.access, TransitAccess.walking);
        await store.settings.set(wheelchairByDefault, true);
        await tester.pumpAndSettle();
        expect(transit.asked.last.access, TransitAccess.wheelchair);
        await store.settings.set(wheelchairByDefault, false);
        await tester.pumpAndSettle();
        expect(transit.asked.last.access, TransitAccess.walking);
      });

      testWidgets('once chosen, stay as chosen when the default changes', (
        tester,
      ) async {
        await openTransit(tester);
        await chooseAccess(tester, 'Bike');
        final asked = transit.asked.length;
        await store.settings.set(wheelchairByDefault, true);
        await tester.pumpAndSettle();
        expect(transit.asked, hasLength(asked));
        expect(transit.asked.last.access, TransitAccess.cycling);
      });
    });

    testWidgets('says why there are no journeys', (tester) async {
      transit.immediately = emptyPlan('no_egress');
      await openTransit(tester);
      expect(
        find.text('No stops within walking range of the destination'),
        findsOneWidget,
      );
    });

    testWidgets('says when the trip is outside the offline area', (
      tester,
    ) async {
      routes.immediately = null;
      await open(tester, settle: false);
      asked(TravelMode.driving).single.answer
          .completeError(const OutsideOfflineArea());
      await tester.pumpAndSettle();
      expect(find.textContaining('outside the offline area'), findsOneWidget);
    });
  });
}
