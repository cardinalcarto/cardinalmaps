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
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_location.dart';
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

void main() {
  setUpAll(() async {
    await RustLib.init(externalLibrary: ExternalLibrary.open(library()));
  });

  test('a simulated trip arrives, saying each announcement once', () async {
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
    final controller = NavigationController(
      routes: routes,
      // Fixes a second's drive apart, as fast as the session takes them.
      location: SimulatedLocationSource(
        route.navigable,
        interval: const Duration(milliseconds: 3),
      ),
      speaker: speaker,
      screen: FakeScreenWake(),
    );
    final states = <NavigationState>[];
    controller.addListener(() {
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
    expect(controller.update!.stepIndex, controller.steps.length);
    await controller.end();
    controller.dispose();
  });
}
