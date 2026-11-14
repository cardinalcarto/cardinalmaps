import 'package:cardinal/directions/directions.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart';
import 'package:http/http.dart' as http;
import 'package:routing/routing.dart';

import '../fakes/fake_location.dart';
import '../fakes/fake_routes.dart';
import '../fakes/fake_transit.dart';

Place place(String id, double lon, double lat) =>
    Place(id: id, kind: 'poi', name: id, center: LngLat(lon, lat));

final a = RouteEndpoint.place(place('a', 1, 1));
final b = RouteEndpoint.place(place('b', 2, 2));
final c = RouteEndpoint.place(place('c', 3, 3));

final one = fakeRoute([
  [0, 0],
  [1, 1],
]);
final two = fakeRoute([
  [0, 0],
  [1, 0],
  [1, 1],
]);

const here = Fix(lng: 5, lat: 6, accuracy: 3);

void main() {
  late FakeRoutes routes;
  late FakeTransit transit;
  late FakeLocation location;
  late Directions directions;

  setUp(() {
    routes = FakeRoutes();
    transit = FakeTransit();
    location = FakeLocation(now: here);
    directions = Directions(
      routes: routes,
      transit: transit,
      location: location,
    );
    addTearDown(directions.dispose);
  });

  Future<void> answer(int i, List<DirectionsRoute> with_) async {
    routes.asked[i].answer.complete(with_);
    await pumpEventQueue();
  }

  void bothEnds() {
    directions
      ..setEndpoint(RouteField.from, a)
      ..setEndpoint(RouteField.to, b);
  }

  test('with an end missing there is nothing to ask', () {
    directions.setEndpoint(RouteField.to, b);
    expect(routes.asked, isEmpty);
    expect(directions.loading, isFalse);
    expect(directions.problem, isNull);
  });

  test('both ends set: loading, from one to the other', () {
    bothEnds();
    expect(directions.loading, isTrue);
    expect(routes.asked.single.from, a.point);
    expect(routes.asked.single.to, b.point);
    expect(routes.asked.single.mode, TravelMode.driving);
  });

  test('routes arrive: showing them, the best selected', () async {
    bothEnds();
    await answer(0, [one, two]);
    expect(directions.loading, isFalse);
    expect(directions.routes, [one, two]);
    expect(directions.selected, 0);
    expect(directions.selectedRoute, one);
  });

  test('no routes in the answer is a problem', () async {
    bothEnds();
    await answer(0, []);
    expect(directions.problem, RouteProblem.noRoute);
  });

  test("the router saying there's no way is a problem", () async {
    bothEnds();
    routes.asked[0].answer.completeError(
      const DirectionsError(kind: DirectionsErrorKind.noRoute, message: 'none'),
    );
    await pumpEventQueue();
    expect(directions.problem, RouteProblem.noRoute);
    expect(directions.loading, isFalse);
  });

  test('asking failing is a problem, and clears old routes', () async {
    bothEnds();
    await answer(0, [one]);
    directions.setEndpoint(RouteField.to, c);
    routes.asked[1].answer.completeError(http.ClientException('offline'));
    await pumpEventQueue();
    expect(directions.problem, RouteProblem.failed);
    expect(directions.routes, isEmpty);
  });

  test(
    'changing an end asks again, keeping the routes until the answer',
    () async {
      bothEnds();
      await answer(0, [one]);
      directions.setEndpoint(RouteField.to, c);
      expect(directions.loading, isTrue);
      expect(directions.routes, [one]);
      expect(routes.asked[1].to, c.point);
    },
  );

  test('changing the mode asks again', () async {
    bothEnds();
    directions.setMode(TravelMode.walking);
    expect(routes.asked[1].mode, TravelMode.walking);
    expect(directions.mode, TravelMode.walking);
  });

  test('walking by wheelchair asks for wheelchair routes', () {
    bothEnds();
    directions.setMode(TravelMode.walking);
    expect(routes.asked.last.wheelchair, isFalse);
    directions.setWheelchair(true);
    expect(directions.wheelchair, isTrue);
    expect(routes.asked, hasLength(3));
    expect(routes.asked.last.mode, TravelMode.walking);
    expect(routes.asked.last.wheelchair, isTrue);
    directions.setWheelchair(false);
    expect(routes.asked.last.wheelchair, isFalse);
  });

  test('walking by wheelchair changes nothing to ask for driving', () {
    var changes = 0;
    bothEnds();
    directions
      ..addListener(() => changes++)
      ..setWheelchair(true);
    expect(routes.asked, hasLength(1));
    // The walking chip changes, though.
    expect(changes, 1);
    directions.setWheelchair(true);
    expect(changes, 1);
  });

  test('the same mode again asks nothing', () {
    bothEnds();
    directions.setMode(TravelMode.driving);
    expect(routes.asked, hasLength(1));
  });

  group('transit', () {
    test('plans with how you get to stops, and gets ready first', () async {
      bothEnds();
      directions.setMode(TravelMode.transit);
      expect(transit.prepared, 1);
      expect(routes.asked, hasLength(1));
      expect(transit.asked.single.access, TransitAccess.walking);
      expect(transit.asked.single.from, a.point);
      transit.asked.single.answer.complete(fakePlan(journeys: 2));
      await pumpEventQueue();
      expect(directions.loading, isFalse);
      expect(directions.plan!.journeys, hasLength(2));
      expect(directions.journey, same(directions.plan!.journeys.first));
      expect(directions.problem, isNull);
    });

    test('changing how you get to stops asks again, only for transit', () {
      bothEnds();
      directions.setTransitAccess(TransitAccess.wheelchair);
      expect(transit.asked, isEmpty);
      directions.setMode(TravelMode.transit);
      expect(transit.asked.single.access, TransitAccess.wheelchair);
      directions.setTransitAccess(TransitAccess.cycling);
      expect(transit.asked.last.access, TransitAccess.cycling);
      expect(transit.asked, hasLength(2));
    });

    test('selecting a journey asks nothing', () async {
      bothEnds();
      directions.setMode(TravelMode.transit);
      transit.asked.single.answer.complete(fakePlan(journeys: 2));
      await pumpEventQueue();
      directions.selectJourney(1);
      expect(directions.journey, same(directions.plan!.journeys[1]));
      directions.selectJourney(5);
      expect(directions.selectedJourney, 1);
      expect(transit.asked, hasLength(1));
    });

    test('an end outside the offline area is a problem of its own', () async {
      bothEnds();
      routes.asked.single.answer.completeError(const OutsideOfflineArea());
      await pumpEventQueue();
      expect(directions.problem, RouteProblem.outsideArea);
      directions.setMode(TravelMode.transit);
      transit.asked.single.answer.completeError(const NoOfflineArea());
      await pumpEventQueue();
      expect(directions.problem, RouteProblem.noOfflineArea);
      expect(directions.plan, isNull);
    });

    test('a plan with no journeys is an answer, not a problem', () async {
      bothEnds();
      directions.setMode(TravelMode.transit);
      transit.asked.single.answer.complete(emptyPlan('no_service'));
      await pumpEventQueue();
      expect(directions.problem, isNull);
      expect(directions.journey, isNull);
      expect(directions.plan!.noJourneysReason, NoJourneysReason.noService);
    });

    test('going back to driving clears the plan once routes arrive', () async {
      bothEnds();
      directions.setMode(TravelMode.transit);
      transit.asked.single.answer.complete(fakePlan());
      await pumpEventQueue();
      directions.setMode(TravelMode.driving);
      routes.asked.last.answer.complete([one]);
      await pumpEventQueue();
      expect(directions.plan, isNull);
      expect(directions.routes, hasLength(1));
    });
  });

  test('swapping the ends asks the other way', () {
    bothEnds();
    directions.swap();
    expect(routes.asked[1].from, b.point);
    expect(routes.asked[1].to, a.point);
  });

  test('an end cleared: no routes, no problem, nothing loading', () async {
    bothEnds();
    await answer(0, [one]);
    directions.setEndpoint(RouteField.from, null);
    expect(directions.routes, isEmpty);
    expect(directions.problem, isNull);
    expect(directions.loading, isFalse);
  });

  test('selecting a route asks nothing', () async {
    bothEnds();
    await answer(0, [one, two]);
    directions.select(1);
    expect(directions.selectedRoute, two);
    expect(routes.asked, hasLength(1));
  });

  test('a route that is not there cannot be selected', () async {
    bothEnds();
    await answer(0, [one]);
    directions.select(4);
    expect(directions.selected, 0);
  });

  test('new routes select the best again', () async {
    bothEnds();
    await answer(0, [one, two]);
    directions.select(1);
    directions.setEndpoint(RouteField.to, c);
    await answer(1, [two, one]);
    expect(directions.selected, 0);
  });

  test('asking again aborts the last request and drops its answer', () async {
    bothEnds();
    var aborted = false;
    routes.asked[0].abort!.then((_) => aborted = true);
    directions.setEndpoint(RouteField.to, c);
    await pumpEventQueue();
    expect(aborted, isTrue);

    await answer(1, [two]);
    await answer(0, [one]);
    expect(directions.routes, [two]);
  });

  test('an aborted request leaves things as they are', () async {
    bothEnds();
    directions.setEndpoint(RouteField.to, c);
    routes.asked[0].answer.completeError(
      http.RequestAbortedException(Uri.parse('http://router/route')),
    );
    await pumpEventQueue();
    expect(directions.problem, isNull);
    expect(directions.loading, isTrue);
  });

  group('starting from where you are', () {
    test('allowed: the start is where you are, without asking', () async {
      await directions.startFromMyLocation();
      expect(directions.from!.isMyLocation, isTrue);
      expect(directions.from!.point, LngLat(here.lng, here.lat));
      expect(location.asked, 0);
    });

    test('not allowed: the start stays empty, without asking', () async {
      location.permission = LocationPermission.denied;
      await directions.startFromMyLocation();
      expect(directions.from, isNull);
      expect(location.asked, 0);
    });

    test('the location service off: the start stays empty', () async {
      location.enabled = false;
      await directions.startFromMyLocation();
      expect(directions.from, isNull);
    });

    test('a start chosen before the position arrives stays', () async {
      final starting = directions.startFromMyLocation();
      directions.setEndpoint(RouteField.from, a);
      await starting;
      expect(directions.from, a);
    });

    test('a start cleared before the position arrives stays clear', () async {
      final starting = directions.startFromMyLocation();
      directions.setEndpoint(RouteField.from, null);
      await starting;
      expect(directions.from, isNull);
    });

    test('a platform without a location service leaves it empty', () async {
      final none = Directions(
        routes: routes,
        transit: transit,
        location: ThrowingLocation(),
      );
      addTearDown(none.dispose);
      await none.startFromMyLocation();
      expect(none.from, isNull);
    });
  });

  group('choosing My Location', () {
    test('allowed: fills the field', () async {
      expect(
        await directions.useMyLocation(RouteField.to),
        LocateResult.located,
      );
      expect(directions.to!.isMyLocation, isTrue);
      expect(location.asked, 0);
    });

    test('not yet allowed: asks, and fills the field if granted', () async {
      location = FakeLocation(
        permission: LocationPermission.denied,
        granted: LocationPermission.whileInUse,
        now: here,
      );
      final asking = Directions(
        routes: routes,
        transit: transit,
        location: location,
      );
      addTearDown(asking.dispose);
      expect(await asking.useMyLocation(RouteField.from), LocateResult.located);
      expect(location.asked, 1);
      expect(asking.from!.isMyLocation, isTrue);
    });

    test('refused: says so and leaves the field', () async {
      location = FakeLocation(
        permission: LocationPermission.denied,
        granted: LocationPermission.denied,
        now: here,
      );
      final asking = Directions(
        routes: routes,
        transit: transit,
        location: location,
      );
      addTearDown(asking.dispose);
      expect(await asking.useMyLocation(RouteField.from), LocateResult.denied);
      expect(asking.from, isNull);
    });

    test('refused for good: says so without asking', () async {
      location.permission = LocationPermission.deniedForever;
      expect(
        await directions.useMyLocation(RouteField.from),
        LocateResult.deniedForever,
      );
      expect(location.asked, 0);
    });

    test('the location service off: says so', () async {
      location.enabled = false;
      expect(
        await directions.useMyLocation(RouteField.from),
        LocateResult.serviceOff,
      );
    });

    test('no position to be had: says so and leaves the field', () async {
      location = FakeLocation();
      final none = Directions(
        routes: routes,
        transit: transit,
        location: location,
      );
      addTearDown(none.dispose);
      expect(
        await none.useMyLocation(RouteField.from),
        LocateResult.unavailable,
      );
      expect(none.from, isNull);
    });
  });

  test('recalculating asks again from a fresh position', () async {
    await directions.startFromMyLocation();
    directions.setEndpoint(RouteField.to, b);
    final before = routes.asked.length;
    location.now = const Fix(lng: 7, lat: 8, accuracy: 3);
    await directions.recalculate();
    expect(routes.asked, hasLength(before + 1));
    expect(routes.asked.last.from, const LngLat(7, 8));
  });
}

/// A platform with no location service, where every call throws.
class ThrowingLocation extends FakeLocation {
  @override
  Future<LocationPermission> checkPermission() =>
      throw UnsupportedError('none');
}
