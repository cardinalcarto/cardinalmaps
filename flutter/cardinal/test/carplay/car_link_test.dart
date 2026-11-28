import 'dart:async';

import 'package:cardinal/carplay/car_link.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/map/viewport.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/store/store.dart';
import 'package:fake_async/fake_async.dart';
import 'package:flutter/material.dart' show Locale;
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_navigation.dart';
import '../fakes/fake_routes.dart';

/// Writes down what the car is sent, and says what the test has the car say.
class FakeCarChannel implements CarChannel {
  /// Whether a car was connected before the phone's engine asked.
  bool connected = false;
  final sent = <(String, Object?)>[];
  void Function(String method, Object? arguments)? call;

  @override
  Future<T?> invoke<T>(String method, [Object? arguments]) async {
    if (method == 'hello') return connected as T;
    sent.add((method, arguments));
    return null;
  }

  @override
  set onCall(void Function(String method, Object? arguments)? onCall) =>
      call = onCall;

  /// What was sent as [method], in order.
  List<Object?> of(String method) => [
    for (final (name, arguments) in sent)
      if (name == method) arguments,
  ];
}

final words = lookupAppLocalizations(const Locale('en'));

class Harness {
  Harness({bool connected = false}) {
    channel.connected = connected;
    link = CarPlayLink(
      controller: controller,
      settings: SettingsStore.inMemory(),
      tiles: () async =>
          (tileJson: 'https://tiles.example/planet.json', basemap: null),
      view: () async =>
          const MapViewport(lng: 1.5, lat: 42.5, zoom: 17, bearing: 30),
      channel: channel,
      words: () => words,
    )..start();
  }

  final channel = FakeCarChannel();
  final location = FakeNavigationLocation();
  final sessions = FakeSessions();
  late final NavigationController controller = fakeNavigationController(
    location: location,
    sessions: sessions,
  );
  late final CarPlayLink link;

  final route = fakeRoute([
    [1.52, 42.50],
    [1.53, 42.51],
  ]);

  void start(FakeAsync async) {
    controller.start(route: route, mode: TravelMode.driving, wheelchair: false);
    async.flushMicrotasks();
  }

  /// A fix that puts you [toManeuver] meters from a turn.
  void fix(FakeAsync async, double toManeuver) {
    final update = fakeUpdate(
      snapped: fakeFix(42.505, 1.525),
      visual: fakeVisual(fakeContent('Turn right', type: ManeuverType.turn)),
      toManeuver: toManeuver,
    );
    if (sessions.sessions.isEmpty) {
      sessions.start = update;
    } else {
      sessions.last.next = update;
    }
    location.send(fakeFix(42.5, 1.52));
    async.flushMicrotasks();
  }

  void car(FakeAsync async, String method, [Object? arguments]) {
    channel.call!(method, arguments);
    async.flushMicrotasks();
  }
}

void run(
  void Function(FakeAsync async, Harness h) body, {
  bool connected = false,
}) {
  FakeAsync().run((async) {
    final h = Harness(connected: connected);
    async.flushMicrotasks();
    body(async, h);
    unawaited(h.controller.end());
    h.link.dispose();
    async.flushMicrotasks();
    async.flushTimers();
  });
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('nothing is sent with no car connected', () {
    run((async, h) {
      h.start(async);
      h.fix(async, 240);
      expect(h.channel.sent, isEmpty);
    });
  });

  test('a car that connects is sent the map, the route and the trip', () {
    run((async, h) {
      h.start(async);
      h.fix(async, 240);
      h.car(async, 'connected');
      expect(h.channel.of('map').single, {
        'tileJson': 'https://tiles.example/planet.json',
        'basemap': null,
        // Where the phone's map looks, north up and no closer than a town.
        'view': const MapViewport(lng: 1.5, lat: 42.5, zoom: 14).arg,
      });
      final route = h.channel.of('route').single! as Map;
      expect(route['line'], [1.52, 42.50, 1.53, 42.51]);
      final trip = h.channel.of('trip').single! as Map;
      expect(trip['route'], route['id']);
      expect(trip['instruction'], 'Turn right');
    });
  });

  test('a car connected before the app started is sent them as it starts', () {
    run(connected: true, (async, h) {
      expect(h.channel.of('map'), hasLength(1));
      // No trip yet.
      expect(h.channel.of('trip'), [null]);
      expect(h.channel.of('route'), isEmpty);
    });
  });

  test('the trip is sent again only when what it says changes', () {
    run((async, h) {
      h.car(async, 'connected');
      h.start(async);
      h.fix(async, 240);
      final sent = h.channel.of('trip').length;
      // The same place and the same rounded distances.
      h.fix(async, 240);
      expect(h.channel.of('trip'), hasLength(sent));
      h.fix(async, 90);
      expect(h.channel.of('trip'), hasLength(sent + 1));
      // One route, sent once.
      expect(h.channel.of('route'), hasLength(1));
    });
  });

  test('the car is told the trip ended, and the next sends its route', () {
    run((async, h) {
      h.car(async, 'connected');
      h.start(async);
      h.fix(async, 240);
      unawaited(h.controller.end());
      async.flushMicrotasks();
      expect(h.channel.of('trip').last, isNull);
      h.start(async);
      expect(h.channel.of('route'), hasLength(2));
    });
  });

  test('nothing more is sent once the car is gone', () {
    run((async, h) {
      h.car(async, 'connected');
      h.car(async, 'disconnected');
      final sent = h.channel.sent.length;
      h.start(async);
      h.fix(async, 240);
      expect(h.channel.sent, hasLength(sent));
    });
  });

  test("the car's End ends the trip", () {
    run((async, h) {
      h.car(async, 'connected');
      h.start(async);
      h.car(async, 'end');
      expect(h.controller.state, NavigationState.idle);
    });
  });

  test("the car's mute button mutes the trip, and the car hears so", () {
    run((async, h) {
      h.car(async, 'connected');
      h.start(async);
      h.car(async, 'setMuted', true);
      expect(h.controller.muted, isTrue);
      expect((h.channel.of('trip').last! as Map)['muted'], isTrue);
    });
  });
}
