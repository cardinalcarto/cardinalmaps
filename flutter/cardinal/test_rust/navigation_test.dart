// A simulated trip through the real navigation session: Ferrostar's controller in the
// routing library, over a route the offline router made from the Andorra fixture tiles,
// driven by the app's navigation controller and simulated fixes.
//
// It runs on the host, not on a device, with the host build of `routing_flutter`. Build
// it first, from `crates/`:
//
//   cargo build --release -p routing_flutter
//
// then, from `flutter/cardinal`:
//
//   timeout 45 flutter test --timeout 10s test_rust
//
// It's kept out of `test/`, whose tests never load the routing library, and out of
// `integration_test/`, which `flutter test` runs on a device.
import 'dart:io';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/navigation/road_matcher.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_location.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';
// The routing package's tests load the host build the same way; the app has no other
// reason to load it from a path.
// ignore: implementation_imports
import 'package:routing/src/rust/frb_generated.dart';

import '../test/fakes/fake_navigation.dart';
import '../test/fakes/fake_routes.dart';

/// The fixture tiles, built by mainline Valhalla 3.9.1 (`build_extract`).
const tiles = '../../valinor/valhalla-graphtile/fixtures/andorra-tiles.tar';

String library() {
  final name = Platform.isMacOS
      ? 'librouting_flutter.dylib'
      : Platform.isWindows
      ? 'routing_flutter.dll'
      : 'librouting_flutter.so';
  return '../../crates/target/release/$name';
}

/// [RoadMatcher] over [router] directly, as [OfflineRoadMatcher] is over the offline area's.
class RouterRoadMatcher implements RoadMatcher {
  RouterRoadMatcher(this.router);

  final OfflineRouter router;
  int matched = 0;

  @override
  Future<MatchedPosition?> match(
    NavigationMode mode,
    List<NavLocation> fixes,
  ) async {
    final position = await router.matchPosition(mode: mode, fixes: fixes);
    if (position != null) matched++;
    return position;
  }
}

void main() {
  setUpAll(() async {
    await RustLib.init(externalLibrary: ExternalLibrary.open(library()));
  });

  test('a simulated trip arrives on matched positions, saying each '
      'announcement once', () async {
    final router = await OfflineRouter.open(tileSets: [tiles]);
    final adapter = DirectionsAdapter.valhalla(
      endpointUrl: 'offline:route',
      costing: 'auto',
      alternates: 0,
      language: 'de-AT',
      units: DistanceUnits.kilometers,
    );
    final request = adapter.request(
      from: const DirectionsPoint(lat: 42.5078, lng: 1.5211),
      to: const DirectionsPoint(lat: 42.5355, lng: 1.583),
    );
    final response = await router.route(body: request.body);
    final route = (await adapter.parse(body: response.body)).first;
    final expected = [
      for (final step in route.navigable.steps())
        for (final spoken in step.spokenInstructions) spoken.text,
    ];
    expect(expected, isNotEmpty);

    final routes = FakeRoutes();
    final speaker = FakeSpeaker();
    final matcher = RouterRoadMatcher(router);
    var clock = DateTime.utc(2026, 11, 14);
    final controller = NavigationController(
      routes: routes,
      // Fixes a second's drive apart, as fast as the session takes them.
      location: SimulatedLocationSource(
        route.navigable,
        interval: const Duration(milliseconds: 3),
        // Stamped a second apart, as they'd come: the matcher won't join fixes that
        // would have taken longer to drive between than the time between them.
        now: () => clock = clock.add(const Duration(seconds: 1)),
      ),
      speaker: speaker,
      screen: FakeScreenWake(),
      settings: SettingsStore.inMemory(),
      matcher: matcher,
    );
    final states = <NavigationState>[];
    var updates = 0;
    var followedMatches = 0;
    controller.addListener(() {
      final update = controller.update;
      if (update != null && controller.state == NavigationState.onRoute) {
        updates++;
        if (update.matched) followedMatches++;
      }
      if (states.isEmpty || states.last != controller.state) {
        states.add(controller.state);
      }
    });

    controller.start(route: route, mode: TravelMode.driving);
    final watch = Stopwatch()..start();
    while (controller.state != NavigationState.arrived &&
        watch.elapsed < const Duration(seconds: 8)) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }

    expect(states, [
      NavigationState.acquiring,
      NavigationState.onRoute,
      NavigationState.arrived,
    ]);
    // Valhalla has no de-AT, so the route is in de-DE, and spoken in it.
    expect(speaker.languages, ['de-DE']);
    expect(speaker.said, expected);
    expect(routes.rerouted, isEmpty);
    // Fixes on the road's line all match it, and are followed.
    expect(matcher.matched, greaterThan(10));
    expect(followedMatches, greaterThan(updates * 0.9));
    expect(controller.update!.stepIndex, controller.steps.length);
    await controller.end();
    controller.dispose();
  });
}
