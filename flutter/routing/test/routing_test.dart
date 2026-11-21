// Routes over the Andorra fixture tiles with the host build of `routing_flutter`.
//
// Build the library first, from `crates/`:
//
//   cargo build --release -p routing_flutter
//
// then run `timeout 45 flutter test --timeout 10s` here.

import 'dart:convert';
import 'dart:io';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';
import 'package:routing/src/rust/frb_generated.dart';

/// The fixture tiles, built by mainline Valhalla 3.9.1 (`build_extract`).
const tiles = '../../valinor/valhalla-graphtile/fixtures/andorra-tiles.tar';

/// The body Ferrostar's `ValhallaHttpRequestGenerator` makes, as in the oracle's
/// `ferrostar_auto` case.
final ferrostarBody = jsonEncode({
  'format': 'osrm',
  'filters': {
    'action': 'include',
    'attributes': [
      'shape_attributes.speed',
      'shape_attributes.speed_limit',
      'shape_attributes.time',
      'shape_attributes.length',
    ],
  },
  'banner_instructions': true,
  'voice_instructions': true,
  'costing': 'auto',
  'locations': [
    {'lat': 42.5078, 'lon': 1.5211, 'street_side_tolerance': 5, 'heading': 90},
    {'lat': 42.5355, 'lon': 1.583, 'type': 'break'},
  ],
});

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

  test('routes a Ferrostar request as the server would', () async {
    final router = await OfflineRouter.open(tileSets: [tiles]);
    final response = await router.route(body: utf8.encode(ferrostarBody));
    expect(response.status, 200);
    final json = jsonDecode(utf8.decode(response.body)) as Map<String, dynamic>;
    expect(json['code'], 'Ok');
    final route = (json['routes'] as List).first as Map<String, dynamic>;
    // Mainline's distance for this route (the oracle capture).
    expect(route['distance'], closeTo(8072.066, 1e-3));
    final step =
        ((route['legs'] as List).first['steps'] as List).first
            as Map<String, dynamic>;
    expect(step['bannerInstructions'], isNotEmpty);
    expect(step['voiceInstructions'], isNotEmpty);
    expect((route['legs'] as List).first['annotation'], contains('distance'));
  });

  test("Ferrostar's request through the router reads back as routes", () async {
    final router = await OfflineRouter.open(tileSets: [tiles]);
    final adapter = DirectionsAdapter.valhalla(
      endpointUrl: 'http://unused/route',
      costing: 'auto',
      alternates: 2,
      language: 'en-US',
      units: DistanceUnits.kilometers,
      wheelchair: false,
    );
    final request = adapter.request(
      from: const DirectionsPoint(lat: 42.5078, lng: 1.5211),
      to: const DirectionsPoint(lat: 42.5355, lng: 1.583),
    );
    final response = await router.route(body: request.body);
    expect(response.status, 200);
    final routes = await adapter.parse(body: response.body);
    expect(routes, isNotEmpty);
    final best = routes.first;
    expect(best.distance, closeTo(8072.066, 1));
    expect(best.duration, greaterThan(0));
    expect(best.geometry.length, greaterThan(2));
    expect(best.steps.first.instruction, isNotEmpty);
    // What the answer says besides: the roads, and the speed profile.
    expect(best.steps.first.roadName, 'Carrer Doctor Nequí');
    expect(best.roadNames, ['CG-2']);
    expect(
      (best.hasToll, best.hasFerry, best.hasMotorway),
      (false, false, false),
    );
    expect(best.runs.length, inInclusiveRange(10, 50));
    final ran = best.runs.fold(0.0, (sum, run) => sum + run.duration);
    expect(ran, closeTo(best.duration, best.duration / 200));
    expect(best.runs.first.speed, greaterThan(0));
  });

  test('navigation follows the route the directions show', () async {
    final router = await OfflineRouter.open(tileSets: [tiles]);
    final adapter = DirectionsAdapter.valhalla(
      endpointUrl: 'http://unused/route',
      costing: 'auto',
      alternates: 0,
      language: 'de-AT',
      units: DistanceUnits.kilometers,
      wheelchair: false,
    );
    final request = adapter.request(
      from: const DirectionsPoint(lat: 42.5078, lng: 1.5211),
      to: const DirectionsPoint(lat: 42.5355, lng: 1.583),
    );
    final response = await router.route(body: request.body);
    final route = (await adapter.parse(body: response.body)).first;
    final steps = route.navigable.steps();
    expect(steps.length, route.steps.length);
    expect(steps.first.spokenInstructions, isNotEmpty);

    final simulation = LocationSimulation(
      route: route.navigable,
      resampleDistance: 10,
      bias: SimulationBias.none,
      biasMeters: 0,
    );
    final session = await NavigationSession.start(
      route: route.navigable,
      mode: NavigationMode.driving,
      location: simulation.current(),
    );
    expect(session.current().status, TripStatus.navigating);
    var update = session.current();
    while (!simulation.isFinished()) {
      update = await session.updateLocation(location: simulation.advance());
    }
    expect(update.status, TripStatus.complete);
    expect(update.stepIndex, steps.length);
  });

  test('routes concurrently', () async {
    final router = await OfflineRouter.open(tileSets: [tiles]);
    final responses = await Future.wait(
      List.generate(6, (_) => router.route(body: utf8.encode(ferrostarBody))),
    );
    for (final response in responses) {
      expect(response.status, 200);
      expect(response.body, responses.first.body);
    }
  });

  test('routing errors are OSRM-format responses', () async {
    final router = await OfflineRouter.open(tileSets: [tiles]);
    final response = await router.route(
      body: utf8.encode(
        jsonEncode({
          'format': 'osrm',
          'costing': 'auto',
          'locations': [
            {'lat': 42.5078, 'lon': 1.5211},
          ],
        }),
      ),
    );
    expect(response.status, 400);
    final json = jsonDecode(utf8.decode(response.body)) as Map<String, dynamic>;
    expect(json['code'], isA<String>());
    expect(json['message'], isA<String>());
  });

  test('no way there is a kind of error of its own', () async {
    final adapter = DirectionsAdapter.valhalla(
      endpointUrl: 'http://unused/route',
      costing: 'auto',
      alternates: 0,
      language: 'en-US',
      units: DistanceUnits.kilometers,
      wheelchair: false,
    );
    await expectLater(
      adapter.parse(
        body: utf8.encode('{"code":"NoRoute","routes":[],"waypoints":[]}'),
      ),
      throwsA(
        isA<DirectionsError>().having(
          (e) => e.kind,
          'kind',
          DirectionsErrorKind.noRoute,
        ),
      ),
    );
  });

  test('opening reports why it failed', () async {
    await expectLater(
      OfflineRouter.open(tileSets: []),
      throwsA(
        isA<OpenError>().having(
          (e) => e.kind,
          'kind',
          OpenErrorKind.noTileSets,
        ),
      ),
    );
    await expectLater(
      OfflineRouter.open(tileSets: ['/nonexistent/region.tar']),
      throwsA(
        isA<OpenError>()
            .having((e) => e.kind, 'kind', OpenErrorKind.unreadable)
            .having((e) => e.message, 'message', contains('region.tar')),
      ),
    );
  });

  test('tiles that arrive are routed after refreshTiles', () async {
    final dir = await Directory.systemTemp.createTemp('routing-arrive');
    addTearDown(() => dir.delete(recursive: true));
    // The fixture's build ID, as mainline 3.9.1 stamped it.
    const build = 53174;
    final router = await OfflineRouter.open(
      tileSets: [dir.path],
      expectedBuildId: build,
    );
    expect((await router.route(body: utf8.encode(ferrostarBody))).status, 400);

    const fixture = '../../valinor/valhalla-graphtile/fixtures/andorra-tiles';
    for (final tile in const [
      '0/003/015.gph',
      '1/047/701.gph',
      '2/000/762/485.gph',
      '2/000/762/486.gph',
      '2/000/763/925.gph',
      '2/000/763/926.gph',
      '2/000/763/927.gph',
    ]) {
      final to = File('${dir.path}/$tile');
      await to.parent.create(recursive: true);
      // Whole files only: written aside, then renamed into place.
      final part = await File('$fixture/$tile').copy('${to.path}.part');
      await part.rename(to.path);
    }
    await router.refreshTiles();
    expect((await router.route(body: utf8.encode(ferrostarBody))).status, 200);
    await expectLater(
      OfflineRouter.open(tileSets: [dir.path], expectedBuildId: build + 1),
      throwsA(
        isA<OpenError>().having(
          (e) => e.kind,
          'kind',
          OpenErrorKind.wrongBuild,
        ),
      ),
    );
  });
}
