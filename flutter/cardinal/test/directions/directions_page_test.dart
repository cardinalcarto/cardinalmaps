import 'dart:math' as math;

import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoButton;
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/directions/transit_timeline.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/map/overlays.dart';
import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/map/selection.dart';
import 'package:cardinal/map/map_control.dart';
import 'package:cardinal/navigation/maneuver_banner.dart';
import 'package:cardinal/navigation/navigation_camera.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/road_name.dart';
import 'package:cardinal/navigation/trip_progress.dart';
import 'package:cardinal/routes.dart';
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

  testWidgets('routes from where you are to the place, with its steps', (
    tester,
  ) async {
    await open(tester);
    expect(find.text('My Location'), findsOneWidget);
    expect(find.text('Chu Minh'), findsOneWidget);
    expect(routes.asked.single.from, const LngLat(0.5, 2));
    expect(routes.asked.single.to, destination.center);
    expect(find.text('11 min'), findsOneWidget);
    expect(find.text('6.5 mi'), findsOneWidget);
    expect(find.text('Drive north.'), findsOneWidget);
    expect(find.text('Turn right.'), findsOneWidget);
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
      expect(find.text('10 km'), findsOneWidget);
      expect(find.text('6.5 mi'), findsNothing);
    });

    testWidgets('imperial in Germany', (tester) async {
      inLocale(tester, const Locale('de', 'DE'));
      await store.settings.set(unitSystem, UnitSystem.imperial);
      await open(tester);
      expect(find.text('6.5 mi'), findsOneWidget);
      expect(find.text('10 km'), findsNothing);
    });

    testWidgets("and the region's units until it's set", (tester) async {
      inLocale(tester, const Locale('de', 'DE'));
      await open(tester);
      expect(find.text('10 km'), findsOneWidget);
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
    expect(routes.asked, hasLength(1));
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
    expect(find.text('Start Navigation'), findsOneWidget);
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

  testWidgets('a chip routes that way and is remembered', (tester) async {
    await open(tester);
    await tester.tap(find.byIcon(Icons.directions_bike));
    await tester.pumpAndSettle();
    expect(routes.asked.last.mode, TravelMode.cycling);
    expect(await store.settings.get(lastTravelMode), TravelMode.cycling);
  });

  testWidgets('directions start with the mode last chosen', (tester) async {
    await store.settings.set(lastTravelMode, TravelMode.walking);
    await open(tester);
    expect(routes.asked.last.mode, TravelMode.walking);
  });

  group('navigation', () {
    List<Map<String, Object?>> lines() =>
        features(routeSource)
            .where((f) => (f['properties']! as Map)['role'] == 'line')
            .toList();

    Future<void> start(WidgetTester tester, {TargetPlatform? platform}) async {
      final router = await open(tester, platform: platform);
      await tester.tap(
        find.text(platform == TargetPlatform.iOS ? 'Go' : 'Start Navigation'),
      );
      await tester.pumpAndSettle();
      expect(
        router.state.uri.toString(),
        '${directionsLocation(destination.id)}/navigate',
      );
    }

    testWidgets('Start Navigation starts the trip and opens its screen', (
      tester,
    ) async {
      await start(tester);
      expect(navigation.state, NavigationState.acquiring);
      expect(navigation.route, same(best));
      expect(navigation.mode, TravelMode.driving);
      expect(find.byTooltip('End'), findsOneWidget);
      expect(find.text('Start Navigation'), findsNothing);
      // The trip's own drawing takes the place of the directions'.
      expect(lines(), isEmpty);
      expect(features(navigationSource), hasLength(1));
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
        expect(map.subjectsSet, fitted + 1);
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
        await tester.tap(find.text('Start Navigation'));
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
        await tester.tap(find.text('Start Navigation'));
        await tester.pumpAndSettle();
        expect(navigation.muted, isTrue);
        expect(find.byTooltip('Unmute'), findsOneWidget);
        expect(await store.settings.get(navigationMuted), isTrue);
      });
    });

    testWidgets('Go starts it on iOS, with Cupertino controls', (tester) async {
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
      expect(find.text('Start Navigation'), findsOneWidget);
      expect(find.byTooltip('End'), findsNothing);
      expect(lines(), hasLength(2));
      expect(features(navigationSource), isEmpty);
      expect(map.subject, isTrue);
    });

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
      expect(find.text('Start Navigation'), findsOneWidget);
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

    testWidgets('back ends the trip too', (tester) async {
      await start(tester);
      final back = tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      await back;
      expect(navigation.state, NavigationState.idle);
      expect(find.text('Start Navigation'), findsOneWidget);
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

    routeTaps.add(1);
    await tester.pumpAndSettle();
    expect(find.text('Go the long way.'), findsOneWidget);
    expect(find.text('Drive north.'), findsNothing);
    expect(routes.asked, hasLength(1));
  });

  testWidgets('a fling stops just below the summary, however hard', (
    tester,
  ) async {
    await open(tester);
    final sheet = SheetScaffold.controllerOf(
      tester.element(find.text('Start Navigation')),
    );
    sheet.animateTo(const SheetAnchor.full());
    await tester.pumpAndSettle();
    expect(sheet.restingAt, const SheetAnchor.full());

    // A short flick: a long drag carries the sheet past the anchor by hand.
    await tester.fling(find.text('Directions'), const Offset(0, 60), 8000);
    await tester.pumpAndSettle();
    expect(
      sheet.restingAt,
      const SheetAnchor.section(#directionsSummary, gravity: double.infinity),
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
    routes.asked.single.answer.completeError(StateError('offline'));
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
    expect(find.text('Start Navigation'), findsNothing);
    expect(features(selectionSource), isNotEmpty);
    expect(map.subject, isTrue);
    expect(features(routeSource), isEmpty);
    expect(features(routeCalloutSource), isEmpty);
  });

  group('transit', () {
    Future<void> openTransit(WidgetTester tester) async {
      await open(tester);
      await tester.tap(find.byIcon(Icons.directions_transit));
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
      expect(
        find.text('Live times from 1 of 2 real-time feeds'),
        findsOneWidget,
      );
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

    testWidgets('a journey opens in detail, and back returns to the list', (
      tester,
    ) async {
      await openTransit(tester);
      await tester.tap(find.textContaining('5:30 PM – 6:15 PM'));
      await tester.pumpAndSettle();
      expect(find.text('Trip details'), findsOneWidget);
      expect(
        find.text('Board at the temporary stop 50 feet north.'),
        findsOneWidget,
      );
      expect(find.text('Route 40 detoured'), findsOneWidget);
      await tester.tap(find.byTooltip('Back'));
      await tester.pumpAndSettle();
      expect(find.text('Trip details'), findsNothing);
      expect(find.byType(JourneyRow), findsWidgets);
    });

    group('the sheet', () {
      SheetController sheetOf(WidgetTester tester) =>
          SheetScaffold.controllerOf(tester.element(find.text('Directions')));
      const summary = SheetAnchor.section(
        #directionsSummary,
        gravity: double.infinity,
      );

      testWidgets('goes full for transit, and stays there', (tester) async {
        await open(tester);
        final sheet = sheetOf(tester);
        expect(sheet.restingAt, summary);
        expect(sheet.isDraggable, isTrue);

        await tester.tap(find.byIcon(Icons.directions_transit));
        await tester.pumpAndSettle();
        expect(sheet.restingAt, const SheetAnchor.full());
        expect(sheet.isDraggable, isFalse);
        expect(sheet.anchors.map((a) => a.anchor), [const SheetAnchor.full()]);

        // A drag down or a fling scrolls the list at most, and the sheet stays.
        await tester.fling(find.text('Directions'), const Offset(0, 300), 3000);
        await tester.pumpAndSettle();
        expect(sheet.restingAt, const SheetAnchor.full());
        expect(sheet.isAtMax, isTrue);
      });

      testWidgets('comes down to 40% for a journey, so the map shows it', (
        tester,
      ) async {
        await open(tester);
        await tester.tap(find.byIcon(Icons.directions_transit));
        await tester.pumpAndSettle();
        final sheet = sheetOf(tester);

        await tester.tap(find.textContaining('5:30 PM – 6:15 PM'));
        await tester.pumpAndSettle();
        expect(sheet.restingAt, const SheetAnchor.fraction(0.4));
        expect(sheet.isDraggable, isTrue);
        expect(map.subject, isTrue);

        // It can be pulled up to scroll through the journey.
        sheet.animateTo(const SheetAnchor.full());
        await tester.pumpAndSettle();
        expect(sheet.restingAt, const SheetAnchor.full());
      });

      testWidgets('back from a journey goes full again, held', (tester) async {
        await open(tester);
        await tester.tap(find.byIcon(Icons.directions_transit));
        await tester.pumpAndSettle();
        final sheet = sheetOf(tester);
        await tester.tap(find.textContaining('5:30 PM – 6:15 PM'));
        await tester.pumpAndSettle();

        await tester.tap(find.byTooltip('Back'));
        await tester.pumpAndSettle();
        expect(sheet.restingAt, const SheetAnchor.full());
        expect(sheet.isDraggable, isFalse);
      });

      testWidgets('another mode frees it and brings it down to the summary', (
        tester,
      ) async {
        await open(tester);
        await tester.tap(find.byIcon(Icons.directions_transit));
        await tester.pumpAndSettle();
        final sheet = sheetOf(tester);

        await tester.tap(find.byTooltip(TravelMode.walking.label));
        await tester.pumpAndSettle();
        expect(sheet.restingAt, summary);
        expect(sheet.isDraggable, isTrue);
        expect(sheet.anchors, hasLength(3));
      });
    });

    testWidgets('how you get to stops is asked with and remembered', (
      tester,
    ) async {
      await openTransit(tester);
      await tester.tap(find.text('Wheelchair'));
      await tester.pumpAndSettle();
      expect(transit.asked.last.access, TransitAccess.wheelchair);
      expect(
        await store.settings.get(lastTransitAccess),
        TransitAccess.wheelchair,
      );
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
      routes.asked.single.answer.completeError(const OutsideOfflineArea());
      await tester.pumpAndSettle();
      expect(find.textContaining('outside the offline area'), findsOneWidget);
    });
  });
}
