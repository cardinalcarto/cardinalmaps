import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/navigation/navigation_camera.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_map.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart' show EdgeInsets;
import 'package:routing/routing.dart';

import '../fakes/fake_map.dart';
import '../fakes/fake_navigation.dart';
import '../fakes/fake_routes.dart';

const line = [
  DirectionsPoint(lat: 0, lng: 0),
  DirectionsPoint(lat: 0, lng: 1),
  DirectionsPoint(lat: 0, lng: 2),
  DirectionsPoint(lat: 0, lng: 3),
];

List<List<double>> coordinates(FakeMap map, String role) {
  final features = ((map.geoJson[navigationSource]! as Map)['features'] as List)
      .cast<Map<String, Object?>>();
  final feature = features.where(
    (f) => (f['properties']! as Map)['role'] == role,
  );
  if (feature.isEmpty) return const [];
  return [
    for (final c in (feature.single['geometry']! as Map)['coordinates'] as List)
      [for (final v in c as List) (v as num).toDouble()],
  ];
}

void main() {
  group('the traveled line', () {
    test('runs to the point before where you are, then to you', () {
      const at = DirectionsPoint(lat: 0, lng: 1.5);
      expect(traveledLine(line, 1, at), [line[0], line[1], at]);
    });

    test('is just the start and you on the first segment', () {
      const at = DirectionsPoint(lat: 0, lng: 0.2);
      expect(traveledLine(line, 0, at), [line[0], at]);
    });

    test('is the whole line at its end', () {
      expect(traveledLine(line, 3, line[3]), [...line, line[3]]);
      // An index past the end, as once complete, is the end.
      expect(traveledLine(line, 9, line[3]), [...line, line[3]]);
    });

    test('is drawn over the whole line ahead', () {
      const at = DirectionsPoint(lat: 0, lng: 1.5);
      final json = navigationGeoJson(line, traveledLine(line, 1, at));
      final features = (json['features']! as List).cast<Map>();
      expect(features.map((f) => (f['properties'] as Map)['role']), [
        'ahead',
        'traveled',
      ]);
    });
  });

  group('the trip on the map', () {
    late FakeMap map;
    late FakeSessions sessions;
    late FakeNavigationLocation location;
    late NavigationController controller;
    late NavigationMap drawn;
    var now = Duration.zero;

    final route = fakeRoute([
      for (final p in line) [p.lng, p.lat],
    ]);

    // Made in each test's fake time, since the controller's work runs in the zone it's
    // made in.
    void make() {
      map = FakeMap();
      sessions = FakeSessions();
      location = FakeNavigationLocation();
      controller = fakeNavigationController(
        sessions: sessions,
        location: location,
      );
      now = Duration.zero;
      drawn = NavigationMap(
        map: map,
        controller: controller,
        elapsed: () => now,
      );
    }

    /// The session's answer to the next fix: you at [lng] on the line, past point
    /// [index], going [speed].
    NavUpdate at(double lng, int index, {double? speed}) => fakeUpdate(
      location: fakeFix(0, lng, speed: speed),
      snapped: fakeFix(0, lng, course: 90, speed: speed),
      geometryIndex: index,
    );

    void fix(FakeAsync async, NavUpdate update) {
      if (sessions.sessions.isEmpty) {
        sessions.start = update;
      } else {
        sessions.last.next = update;
      }
      location.send(fakeFix(0, 0));
      async.flushMicrotasks();
    }

    test('draws the route ahead, then you along it', () {
      fakeAsync((async) {
        make();
        controller.start(
          route: route,
          mode: TravelMode.driving,
          wheelchair: false,
        );
        drawn.start();
        expect(coordinates(map, 'ahead'), hasLength(4));
        expect(coordinates(map, 'traveled'), isEmpty);
        expect(map.puckVisible, isFalse);

        fix(async, at(0.5, 0));
        expect(map.puckVisible, isTrue);
        expect(map.pucks.single.duration, Duration.zero);
        expect(map.pucks.single.bearing, 90);
        expect(coordinates(map, 'traveled'), [
          [0, 0],
          [0.5, 0],
        ]);

        now += const Duration(milliseconds: 1100);
        fix(async, at(1.5, 1));
        expect(map.pucks.last.lng, 1.5);
        expect(map.pucks.last.duration, const Duration(milliseconds: 1100));
        expect(coordinates(map, 'traveled'), [
          [0, 0],
          [1, 0],
          [1.5, 0],
        ]);
      });
    });

    test('follows at the zoom for your speed', () {
      fakeAsync((async) {
        make();
        controller.start(
          route: route,
          mode: TravelMode.driving,
          wheelchair: false,
        );
        drawn.start();
        final camera = followCamera(NavigationMode.driving);
        fix(async, at(0.5, 0, speed: camera.referenceSpeed / 2));
        expect(map.follows.single.zoom, camera.zoom);
        expect(map.follows.single.pitch, camera.pitch);

        now += const Duration(seconds: 1);
        fix(async, at(0.6, 0, speed: camera.referenceSpeed * 2));
        expect(map.follows.last.zoom, closeTo(camera.zoom - 1, 1e-9));

        // Moved by hand: it stays where it was put.
        map.following.value = false;
        now += const Duration(seconds: 1);
        fix(async, at(0.7, 0, speed: 1));
        expect(map.follows, hasLength(2));
      });
    });

    test('has nothing to recenter on before it follows', () {
      fakeAsync((async) {
        make();
        controller.start(
          route: route,
          mode: TravelMode.driving,
          wheelchair: false,
        );
        drawn.start();
        expect(drawn.cameraMode.value, isNull);
        drawn.recenter();
        drawn.overview(EdgeInsets.zero);
        expect(map.follows, isEmpty);
        expect(map.subjectsSet, 0);
      });
    });

    test('offers to recenter once moved off you, and follows again', () {
      fakeAsync((async) {
        make();
        controller.start(
          route: route,
          mode: TravelMode.driving,
          wheelchair: false,
        );
        drawn.start();
        final camera = followCamera(NavigationMode.driving);
        fix(async, at(0.5, 0, speed: camera.referenceSpeed * 2));
        expect(drawn.cameraMode.value, NavigationCameraMode.following);

        // Moved by hand.
        map.following.value = false;
        expect(drawn.cameraMode.value, NavigationCameraMode.free);

        drawn.recenter();
        expect(map.follows, hasLength(2));
        // At the zoom for the last speed, and the mode's tilt.
        expect(map.follows.last.zoom, closeTo(camera.zoom - 1, 1e-9));
        expect(map.follows.last.pitch, camera.pitch);
        expect(map.following.value, isTrue);
        expect(drawn.cameraMode.value, NavigationCameraMode.following);
      });
    });

    test('overview shows the rest of the trip, flat and north up', () {
      fakeAsync((async) {
        make();
        controller.start(
          route: route,
          mode: TravelMode.driving,
          wheelchair: false,
        );
        drawn.start();
        fix(async, at(1.5, 1));
        const clear = EdgeInsets.only(top: 150, bottom: 90);
        drawn.overview(clear);
        expect(map.following.value, isFalse);
        expect(drawn.cameraMode.value, NavigationCameraMode.overview);
        // From the chevron to the end: what's behind it is left out.
        expect(map.subjectBounds, (west: 1.5, south: 0, east: 3, north: 0));
        expect((map.subjectBearing, map.subjectPitch), (0, 0));
        expect(map.subjectClear, clear);
        expect(map.subjectMargin, overviewMargin);
        expect(map.subjectMaxZoom, overviewMaxZoom);

        // It stays as you move along.
        now += const Duration(seconds: 1);
        fix(async, at(1.7, 1));
        expect(map.subjectsSet, 1);
        expect(map.follows, hasLength(1));

        // Told the same again, as the screen does on each move of the sheet, it
        // stays; told to keep clear of something new, it fits the rest again.
        drawn.overview(clear);
        expect(map.subjectsSet, 1);
        const wider = EdgeInsets.only(top: 150, bottom: 90, right: 70);
        drawn.overview(wider);
        expect(map.subjectsSet, 2);
        expect(map.subjectClear, wider);
        expect(map.subjectBounds, (west: 1.7, south: 0, east: 3, north: 0));

        // Recenter follows again.
        drawn.recenter();
        expect(map.following.value, isTrue);
        expect(drawn.cameraMode.value, NavigationCameraMode.following);
        expect(map.follows, hasLength(2));
      });
    });

    test('overview shows a new route in place of the old one', () {
      fakeAsync((async) {
        make();
        final routes = FakeRoutes();
        controller = fakeNavigationController(
          routes: routes,
          sessions: sessions,
          location: location,
        );
        drawn = NavigationMap(map: map, controller: controller);
        controller.start(
          route: route,
          mode: TravelMode.driving,
          wheelchair: false,
        );
        drawn.start();
        fix(async, at(0.5, 0));
        const clear = EdgeInsets.only(top: 150);
        drawn.overview(clear);
        fix(
          async,
          fakeUpdate(
            deviation: NavDeviation.offRoute,
            snapped: fakeFix(0, 0.5, course: 90),
            effective: fakeFix(0.5, 0.5, course: 0),
          ),
        );
        async.elapse(offRouteHold);
        expect(map.subjectsSet, 1);
        sessions.last.next = fakeUpdate(snapped: fakeFix(0.5, 0.5, course: 0));
        routes.rerouted.single.answer.complete(
          fakeRoute([
            [0.5, 0.5],
            [0.5, 2],
            [1, 2],
          ]),
        );
        async.flushMicrotasks();
        expect(map.subjectsSet, 2);
        expect(map.subjectBounds, (west: 0.5, south: 0.5, east: 1, north: 2));
        expect(map.subjectClear, clear);
        expect(drawn.cameraMode.value, NavigationCameraMode.overview);
      });
    });

    test('a new route leaves the map where it was moved by hand', () {
      fakeAsync((async) {
        make();
        final routes = FakeRoutes();
        controller = fakeNavigationController(
          routes: routes,
          sessions: sessions,
          location: location,
        );
        drawn = NavigationMap(map: map, controller: controller);
        controller.start(
          route: route,
          mode: TravelMode.driving,
          wheelchair: false,
        );
        drawn.start();
        fix(async, at(0.5, 0));
        drawn.overview(EdgeInsets.zero);
        // Moved by hand, which lets go of the overview.
        map.clearSubject();
        fix(async, fakeUpdate(deviation: NavDeviation.offRoute));
        async.elapse(offRouteHold);
        sessions.last.next = at(0.5, 0);
        routes.rerouted.single.answer.complete(
          fakeRoute([
            [0.5, 0],
            [0.5, 1],
          ]),
        );
        async.flushMicrotasks();
        expect(map.subjectsSet, 1);
        expect(drawn.cameraMode.value, NavigationCameraMode.free);
      });
    });

    test('puts the chevron where navigation put you off the route', () {
      fakeAsync((async) {
        make();
        controller.start(
          route: route,
          mode: TravelMode.driving,
          wheelchair: false,
        );
        drawn.start();
        fix(async, at(0.5, 0));
        expect(map.pucks.last.lng, 0.5);

        // On the line, even with a match beside it.
        final road = fakeFix(0.001, 0.6, course: 45);
        fix(
          async,
          fakeUpdate(
            location: fakeFix(0.0012, 0.6),
            snapped: fakeFix(0, 0.6, course: 90),
            effective: road,
            matched: true,
          ),
        );
        expect((map.pucks.last.lat, map.pucks.last.lng), (0, 0.6));
        expect(map.pucks.last.bearing, 90);

        // Off the route, on the road you're on; the traveled line still ends on the
        // route's.
        fix(
          async,
          fakeUpdate(
            deviation: NavDeviation.offRoute,
            location: fakeFix(0.0012, 0.7),
            snapped: fakeFix(0, 0.7, course: 90),
            effective: fakeFix(0.001, 0.7, course: 45),
            matched: true,
          ),
        );
        expect(controller.state, NavigationState.offRoute);
        expect((map.pucks.last.lat, map.pucks.last.lng), (0.001, 0.7));
        expect(map.pucks.last.bearing, 45);
        expect(coordinates(map, 'traveled').last, [0.7, 0]);

        // And while the new route is coming.
        async.elapse(offRouteHold);
        expect(controller.state, NavigationState.rerouting);
        fix(
          async,
          fakeUpdate(
            deviation: NavDeviation.offRoute,
            location: fakeFix(0.003, 0.8),
            snapped: fakeFix(0, 0.8, course: 90),
            effective: fakeFix(0.003, 0.8, course: 10),
          ),
        );
        // Unmatched, it's the fix itself.
        expect((map.pucks.last.lat, map.pucks.last.lng), (0.003, 0.8));
        expect(map.pucks.last.bearing, 10);
      });
    });

    test('redraws a new route', () {
      fakeAsync((async) {
        make();
        final routes = FakeRoutes();
        controller = fakeNavigationController(
          routes: routes,
          sessions: sessions,
          location: location,
        );
        drawn = NavigationMap(map: map, controller: controller);
        controller.start(
          route: route,
          mode: TravelMode.driving,
          wheelchair: false,
        );
        drawn.start();
        fix(async, at(0.5, 0));
        fix(async, fakeUpdate(deviation: NavDeviation.offRoute));
        async.elapse(offRouteHold);
        final rerouted = fakeRoute([
          [0.5, 0],
          [0.5, 1],
        ]);
        sessions.last.next = at(0.5, 0);
        routes.rerouted.single.answer.complete(rerouted);
        async.flushMicrotasks();
        expect(controller.route, same(rerouted));
        expect(coordinates(map, 'ahead'), [
          [0.5, 0],
          [0.5, 1],
        ]);
      });
    });

    test('takes the trip off the map when it stops', () {
      fakeAsync((async) {
        make();
        controller.start(
          route: route,
          mode: TravelMode.driving,
          wheelchair: false,
        );
        drawn.start();
        fix(async, at(0.5, 0));
        drawn.dispose();
        expect(map.puckVisible, isFalse);
        expect(map.following.value, isFalse);
        expect(coordinates(map, 'ahead'), isEmpty);
      });
    });
  });
}
