import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/directions/transit_sheet.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/map/overlays.dart';
import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/map/selection.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_location.dart';
import '../fakes/fake_map.dart';
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

  setUp(() async {
    store = fakeStore();
    await store.places.put(destination);
    routes = FakeRoutes(immediately: [best, other]);
    transit = FakeTransit(immediately: fakePlan(journeys: 2));
    map = FakeMap()..hasGround = true;
    overlays = MapOverlays();
  });

  Future<GoRouter> open(
    WidgetTester tester, {
    FakeLocation? location,
    bool settle = true,
  }) async {
    final router = await pumpFakeApp(
      tester,
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

  testWidgets('starting navigation is shown but does nothing', (tester) async {
    await open(tester);
    final start = tester.widget<FilledButton>(
      find.widgetWithText(FilledButton, 'Start Navigation'),
    );
    expect(start.onPressed, isNull);
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

    testWidgets('lists the journeys with their legs, and remembers the mode', (
      tester,
    ) async {
      await openTransit(tester);
      expect(transit.asked.single.access, TransitAccess.walking);
      expect(await store.settings.get(lastTravelMode), TravelMode.transit);
      expect(find.byType(JourneyCard), findsWidgets);
      expect(find.text('40 to Downtown Seattle'), findsWidgets);
      expect(find.text('1 Line to Angle Lake'), findsWidgets);
      expect(
        find.text('Live times from 1 of 2 real-time feeds'),
        findsOneWidget,
      );
      // The walk to a moved stop says so in the list too.
      expect(find.text('Fremont Ave N & N 34th St has moved'), findsWidgets);
      expect(
        find.textContaining('Stay on at 3rd Ave & Pike St as 7 to Rainier'),
        findsWidgets,
      );
      expect(find.textContaining('4 min late'), findsWidgets);
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
      await tester.tap(find.text('5:30 PM – 6:15 PM'));
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
      expect(find.byType(JourneyCard), findsWidgets);
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
