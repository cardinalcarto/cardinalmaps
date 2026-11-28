import 'dart:async';

import 'package:cardinal/carplay/car_trip.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/navigation/maneuver_icon.dart';
import 'package:cardinal/units.dart';
import 'package:fake_async/fake_async.dart';
import 'package:flutter/material.dart' show Locale;
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_navigation.dart';
import '../fakes/fake_routes.dart';

final words = lookupAppLocalizations(const Locale('en'));

class Harness {
  final location = FakeNavigationLocation();
  final sessions = FakeSessions();
  late final controller = fakeNavigationController(
    location: location,
    sessions: sessions,
  );

  final route = fakeRoute(
    [
      [1.52, 42.50],
      [1.53, 42.51],
    ],
    navigationSteps: [
      fakeManeuverStep(
        'You have arrived',
        type: ManeuverType.arrive,
        modifier: ManeuverModifier.right,
      ),
    ],
  );

  void start(FakeAsync async, {String? destination}) {
    controller.start(
      route: route,
      mode: TravelMode.driving,
      wheelchair: false,
      destinationName: destination,
    );
    async.flushMicrotasks();
  }

  /// A fix the session answers with [update].
  void fix(FakeAsync async, NavUpdate update) {
    if (sessions.sessions.isEmpty) {
      sessions.start = update;
    } else {
      sessions.last.next = update;
    }
    location.send(fakeFix(42.5, 1.52));
    async.flushMicrotasks();
  }

  Map<String, Object?>? trip({UnitSystem units = UnitSystem.metric}) =>
      carTripOf(controller, words: words, units: units, route: 7);
}

void run(void Function(FakeAsync async, Harness h) body) {
  FakeAsync().run((async) {
    final h = Harness();
    async.flushMicrotasks();
    body(async, h);
    unawaited(h.controller.end());
    async.flushMicrotasks();
    async.flushTimers();
  });
}

/// On the route's line at [lat], [lng], with a right turn [meters] off.
NavUpdate turn(double meters, {double lat = 42.505, double lng = 1.525}) =>
    fakeUpdate(
      location: fakeFix(lat, lng, speed: 12),
      snapped: fakeFix(lat, lng, course: 40),
      geometryIndex: 0,
      stepIndex: 3,
      visual: fakeVisual(
        fakeContent(
          'Turn right onto Pine Street',
          type: ManeuverType.turn,
          modifier: ManeuverModifier.right,
        ),
      ),
      toManeuver: meters,
    );

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('there is nothing to say with no trip', () {
    run((async, h) => expect(h.trip(), isNull));
  });

  test('before the first good fix, it says what the status card does', () {
    run((async, h) {
      h.start(async, destination: 'Pine Street Market');
      final trip = h.trip()!;
      expect(trip['state'], 'acquiring');
      expect(trip['mode'], 'driving');
      expect(trip['route'], 7);
      expect(trip['status'], words.navigationWaitingForGps);
      expect(trip['destination'], 'Pine Street Market');
      expect(trip['end'], words.navigationEnd);
      expect(trip['to'], {'lat': 42.51, 'lng': 1.53});
      expect(trip.containsKey('instruction'), isFalse);
      expect(trip.containsKey('at'), isFalse);
    });
  });

  test('on the route, it says the maneuver, distances and where you are', () {
    run((async, h) {
      h.start(async);
      h.fix(async, turn(240));
      final trip = h.trip()!;
      expect(trip['state'], 'onRoute');
      expect(trip['instruction'], 'Turn right onto Pine Street');
      expect(trip['step'], 3);
      expect(
        trip['glyph'],
        maneuverIcon(ManeuverType.turn, ManeuverModifier.right)!.codePoint,
      );
      expect(trip.containsKey('status'), isFalse);
      // Rounded as the banner rounds them: 240 m to the nearest 100, 1.24 km to a
      // tenth.
      expect(trip['toManeuver'], {'value': 200.0, 'unit': 'meters'});
      expect(trip['toManeuverSeconds'], 24.0);
      expect(trip['remaining'], {'value': 1.2, 'unit': 'kilometers'});
      expect(trip['remainingSeconds'], 124.0);
      expect(trip['at'], {
        'lat': 42.505,
        'lng': 1.525,
        'bearing': 40.0,
        'speed': 12.0,
      });
      expect(trip['snapped'], {'lat': 42.505, 'lng': 1.525});
      expect(trip['index'], 0);
    });
  });

  test('distances are in feet and miles where the phone shows them so', () {
    run((async, h) {
      h.start(async);
      h.fix(async, turn(240));
      final trip = h.trip(units: UnitSystem.imperial)!;
      // 787 ft to the nearest 100, 0.77 mi to a tenth.
      expect(trip['toManeuver'], {'value': 800.0, 'unit': 'feet'});
      expect(trip['remaining'], {'value': 0.8, 'unit': 'miles'});
    });
  });

  test('off the route, it says so, with where navigation put you', () {
    run((async, h) {
      h.start(async);
      h.fix(
        async,
        fakeUpdate(
          deviation: NavDeviation.offRoute,
          location: fakeFix(42.6, 1.6),
          snapped: fakeFix(42.505, 1.525),
          effective: fakeFix(42.61, 1.61, course: 90),
        ),
      );
      final trip = h.trip()!;
      expect(trip['state'], 'offRoute');
      expect(trip['status'], words.navigationOffRoute);
      expect(trip.containsKey('instruction'), isFalse);
      expect((trip['at']! as Map)['lat'], 42.61);
      expect((trip['at']! as Map)['bearing'], 90.0);
      // The traveled line still ends on the route's line.
      expect(trip['snapped'], {'lat': 42.505, 'lng': 1.525});
    });
  });

  test('once arrived, it says so', () {
    run((async, h) {
      h.start(async);
      h.fix(async, turn(240));
      h.fix(async, fakeUpdate(status: TripStatus.complete));
      final trip = h.trip()!;
      expect(trip['state'], 'arrived');
      expect(trip['status'], words.navigationArrived);
      expect(trip.containsKey('instruction'), isFalse);
    });
  });

  test("a route's line comes back as it was sent", () {
    const line = [
      DirectionsPoint(lat: 42.5, lng: 1.52),
      DirectionsPoint(lat: 42.51, lng: 1.53),
      DirectionsPoint(lat: 42.52, lng: 1.55),
    ];
    final sent = carRouteOf(4, line);
    expect(sent['id'], 4);
    expect(carRouteLine(sent['line']), line);
    expect(carRouteLine(null), isEmpty);
  });
}
