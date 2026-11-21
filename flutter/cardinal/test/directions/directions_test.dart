import 'dart:async';

import 'package:cardinal/directions/directions.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/offline/offline_area.dart' show NoOfflineTransit;
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

  /// The questions asked for [mode], in order.
  List<RouteQuestion> asked(TravelMode mode) =>
      routes.asked.where((question) => question.mode == mode).toList();

  /// Answers the latest question for [mode].
  Future<void> answer(
    List<DirectionsRoute> with_, {
    TravelMode mode = TravelMode.driving,
  }) async {
    asked(mode).last.answer.complete(with_);
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
    expect(transit.asked, isEmpty);
    expect(directions.loading, isFalse);
    expect(directions.problem, isNull);
  });

  test('both ends set: every way is asked at once, and loading', () {
    bothEnds();
    expect(routes.asked.map((question) => question.mode), [
      TravelMode.driving,
      TravelMode.walking,
      TravelMode.cycling,
    ]);
    expect(transit.asked, hasLength(1));
    for (final question in routes.asked) {
      expect(question.from, a.point);
      expect(question.to, b.point);
    }
    expect(transit.asked.single.from, a.point);
    expect(transit.asked.single.to, b.point);
    for (final mode in TravelMode.values) {
      expect(directions.options(mode).loading, isTrue, reason: '$mode');
    }
    expect(directions.loading, isTrue);
  });

  test('routes arrive: showing them, the first selected', () async {
    bothEnds();
    await answer([one, two]);
    expect(directions.loading, isFalse);
    expect(directions.routes, [one, two]);
    expect(directions.selected, 0);
    expect(directions.selectedRoute, one);
  });

  test('each way gets its own answer, whenever it comes', () async {
    bothEnds();
    await answer([two], mode: TravelMode.cycling);
    expect(directions.options(TravelMode.cycling).routes, [two]);
    expect(directions.options(TravelMode.cycling).loading, isFalse);
    expect(directions.options(TravelMode.driving).loading, isTrue);
    expect(directions.options(TravelMode.walking).loading, isTrue);
    expect(directions.options(TravelMode.transit).loading, isTrue);
    // The way that's showing is still waiting.
    expect(directions.loading, isTrue);
    expect(directions.routes, isEmpty);

    await answer([one, two]);
    expect(directions.routes, [one, two]);
    asked(TravelMode.walking).last.answer
        .completeError(http.ClientException('offline'));
    transit.asked.single.answer.complete(fakePlan());
    await pumpEventQueue();
    expect(directions.options(TravelMode.walking).problem, RouteProblem.failed);
    expect(directions.options(TravelMode.transit).plan, isNotNull);
    expect(directions.options(TravelMode.cycling).routes, [two]);
    expect(directions.routes, [one, two]);
  });

  group('the best time of a way', () {
    final now = DateTime.fromMillisecondsSinceEpoch(fakeStart * 1000);

    test('is its fastest route\'s', () {
      final slow = fakeRoute([
        [0, 0],
        [1, 1],
      ], duration: 900);
      final fast = fakeRoute([
        [0, 0],
        [1, 1],
      ], duration: 620);
      expect(
        ModeOptions(routes: [slow, fast]).best(now),
        const Duration(seconds: 620),
      );
    });

    test('for transit is from now until the earliest arrival', () {
      // Its journeys arrive 45 and 55 minutes after the plan was made.
      final options = ModeOptions(plan: fakePlan(journeys: 2));
      expect(options.best(now), const Duration(minutes: 45));
      expect(
        options.best(now.add(const Duration(minutes: 5))),
        const Duration(minutes: 40),
      );
      expect(
        options.best(now.add(const Duration(hours: 2))),
        Duration.zero,
        reason: 'never less than nothing',
      );
    });

    test('is nothing with no options', () {
      expect(const ModeOptions().best(now), isNull);
      expect(const ModeOptions(loading: true).best(now), isNull);
      expect(
        const ModeOptions(problem: RouteProblem.noRoute).best(now),
        isNull,
      );
      expect(ModeOptions(plan: emptyPlan('no_service')).best(now), isNull);
    });
  });

  test('no routes in the answer is a problem', () async {
    bothEnds();
    await answer([]);
    expect(directions.problem, RouteProblem.noRoute);
  });

  test("the router saying there's no way is a problem", () async {
    bothEnds();
    asked(TravelMode.driving).last.answer.completeError(
      const DirectionsError(kind: DirectionsErrorKind.noRoute, message: 'none'),
    );
    await pumpEventQueue();
    expect(directions.problem, RouteProblem.noRoute);
    expect(directions.loading, isFalse);
  });

  test('asking failing is a problem', () async {
    bothEnds();
    await answer([one]);
    directions.setEndpoint(RouteField.to, c);
    asked(TravelMode.driving).last.answer
        .completeError(http.ClientException('offline'));
    await pumpEventQueue();
    expect(directions.problem, RouteProblem.failed);
    expect(directions.routes, isEmpty);
  });

  test(
    'changing an end asks every way again, with nothing to show meanwhile',
    () async {
      bothEnds();
      await answer([one]);
      await answer([two], mode: TravelMode.cycling);
      final before = routes.asked.length;
      directions.setEndpoint(RouteField.to, c);
      expect(routes.asked, hasLength(before + 3));
      expect(transit.asked, hasLength(2));
      expect(routes.asked.skip(before).map((question) => question.to), [
        c.point,
        c.point,
        c.point,
      ]);
      for (final mode in TravelMode.values) {
        final options = directions.options(mode);
        expect(options.loading, isTrue, reason: '$mode');
        expect(options.routes, isEmpty, reason: '$mode');
        expect(options.plan, isNull, reason: '$mode');
      }
    },
  );

  test('choosing another way shows what it has, and asks nothing', () async {
    bothEnds();
    await answer([one]);
    await answer([two, one], mode: TravelMode.walking);
    final before = routes.asked.length;
    var changes = 0;
    directions.addListener(() => changes++);
    directions.setMode(TravelMode.walking);
    expect(directions.mode, TravelMode.walking);
    expect(directions.routes, [two, one]);
    expect(routes.asked, hasLength(before));
    expect(changes, 1);
    directions.setMode(TravelMode.cycling);
    expect(directions.loading, isTrue, reason: 'its answer is still to come');
    expect(routes.asked, hasLength(before));
  });

  test('each way keeps its selection while the trip is the same', () async {
    bothEnds();
    await answer([one, two]);
    await answer([two, one], mode: TravelMode.cycling);
    directions.select(1);
    directions.setMode(TravelMode.cycling);
    expect(directions.selected, 0);
    directions.select(1);
    directions.setMode(TravelMode.driving);
    expect(directions.selected, 1);
    expect(directions.selectedRoute, two);
    directions.setMode(TravelMode.cycling);
    expect(directions.selectedRoute, one);
    // An answer for another way changes neither.
    await answer([one], mode: TravelMode.walking);
    expect(directions.selected, 1);
    expect(directions.options(TravelMode.driving).selected, 1);
  });

  test('a changed trip selects the first of every way again', () async {
    bothEnds();
    await answer([one, two]);
    await answer([two, one], mode: TravelMode.cycling);
    directions.select(1);
    directions.setMode(TravelMode.cycling);
    directions.select(1);
    directions.swap();
    await answer([two, one]);
    await answer([one, two], mode: TravelMode.cycling);
    expect(directions.selected, 0);
    expect(directions.options(TravelMode.driving).selected, 0);
  });

  test('walking by wheelchair asks for walking again, and only walking', () {
    bothEnds();
    expect(asked(TravelMode.walking).single.wheelchair, isFalse);
    final before = routes.asked.length;
    directions.setWheelchair(true);
    expect(directions.wheelchair, isTrue);
    expect(routes.asked, hasLength(before + 1));
    expect(transit.asked, hasLength(1));
    expect(routes.asked.last.mode, TravelMode.walking);
    expect(routes.asked.last.wheelchair, isTrue);
    directions.setWheelchair(false);
    expect(routes.asked.last.wheelchair, isFalse);
  });

  test('walking by wheelchair leaves the other ways as they are', () async {
    var changes = 0;
    bothEnds();
    await answer([one, two]);
    directions.select(1);
    directions
      ..addListener(() => changes++)
      ..setWheelchair(true);
    expect(directions.routes, [one, two]);
    expect(directions.selected, 1);
    expect(directions.options(TravelMode.walking).loading, isTrue);
    // The walking tab changes, though.
    expect(changes, 1);
    directions.setWheelchair(true);
    expect(changes, 1);
  });

  test('the same mode again changes nothing', () {
    bothEnds();
    var changes = 0;
    directions.addListener(() => changes++);
    directions.setMode(TravelMode.driving);
    expect(changes, 0);
  });

  group('transit', () {
    test('plans with how you get to stops', () async {
      bothEnds();
      expect(transit.asked.single.access, TransitAccess.walking);
      expect(transit.asked.single.from, a.point);
      directions.setMode(TravelMode.transit);
      expect(transit.prepared, 1);
      expect(transit.asked, hasLength(1));
      transit.asked.single.answer.complete(fakePlan(journeys: 2));
      await pumpEventQueue();
      expect(directions.loading, isFalse);
      expect(directions.plan!.journeys, hasLength(2));
      expect(directions.journey, same(directions.plan!.journeys.first));
      expect(directions.problem, isNull);
    });

    test('changing how you get to stops asks again, only for transit', () {
      directions.setTransitAccess(TransitAccess.wheelchair);
      expect(transit.asked, isEmpty, reason: 'there is no trip yet');
      bothEnds();
      expect(transit.asked.single.access, TransitAccess.wheelchair);
      final before = routes.asked.length;
      directions.setTransitAccess(TransitAccess.cycling);
      expect(transit.asked.last.access, TransitAccess.cycling);
      expect(transit.asked, hasLength(2));
      expect(routes.asked, hasLength(before));
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
      asked(TravelMode.driving).single.answer
          .completeError(const OutsideOfflineArea());
      await pumpEventQueue();
      expect(directions.problem, RouteProblem.outsideArea);
      directions.setMode(TravelMode.transit);
      transit.asked.single.answer.completeError(const NoOfflineArea());
      await pumpEventQueue();
      expect(directions.problem, RouteProblem.noOfflineArea);
      expect(directions.plan, isNull);
    });

    test('an offline area without transit finds no journeys', () async {
      bothEnds();
      directions.setMode(TravelMode.transit);
      transit.asked.single.answer.completeError(const NoOfflineTransit());
      await pumpEventQueue();
      expect(directions.problem, RouteProblem.noRoute);
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

    test('its plan is its own, and driving has none', () async {
      bothEnds();
      transit.asked.single.answer.complete(fakePlan());
      await answer([one]);
      expect(directions.plan, isNull);
      expect(directions.routes, hasLength(1));
      directions.setMode(TravelMode.transit);
      expect(directions.plan, isNotNull);
      expect(directions.routes, isEmpty);
    });
  });

  test('swapping the ends asks the other way', () {
    bothEnds();
    directions.swap();
    expect(routes.asked.last.from, b.point);
    expect(routes.asked.last.to, a.point);
    expect(transit.asked.last.from, b.point);
  });

  test('an end cleared: no routes, no problem, nothing loading', () async {
    bothEnds();
    await answer([one]);
    directions.setEndpoint(RouteField.from, null);
    for (final mode in TravelMode.values) {
      final options = directions.options(mode);
      expect(options.routes, isEmpty);
      expect(options.plan, isNull);
      expect(options.problem, isNull);
      expect(options.loading, isFalse);
    }
  });

  test('selecting a route asks nothing', () async {
    bothEnds();
    await answer([one, two]);
    final before = routes.asked.length;
    directions.select(1);
    expect(directions.selectedRoute, two);
    expect(routes.asked, hasLength(before));
  });

  test('a route that is not there cannot be selected', () async {
    bothEnds();
    await answer([one]);
    directions.select(4);
    expect(directions.selected, 0);
  });

  test(
    'asking again aborts the last requests and drops their answers',
    () async {
      bothEnds();
      final first = asked(TravelMode.driving).single;
      final firstPlan = transit.asked.single;
      var aborted = 0;
      for (final question in routes.asked) {
        unawaited(question.abort!.then((_) => aborted++));
      }
      unawaited(firstPlan.abort!.then((_) => aborted++));
      directions.setEndpoint(RouteField.to, c);
      await pumpEventQueue();
      expect(aborted, 4);

      await answer([two]);
      first.answer.complete([one]);
      firstPlan.answer.complete(fakePlan());
      await pumpEventQueue();
      expect(directions.routes, [two]);
      expect(directions.options(TravelMode.transit).plan, isNull);
      expect(directions.options(TravelMode.transit).loading, isTrue);
    },
  );

  test(
    'an answer for an older trip is dropped, whatever way it is for',
    () async {
      bothEnds();
      final old = asked(TravelMode.cycling).single;
      directions.swap();
      old.answer.complete([one]);
      await pumpEventQueue();
      expect(directions.options(TravelMode.cycling).routes, isEmpty);
      expect(directions.options(TravelMode.cycling).loading, isTrue);
    },
  );

  test('an aborted request leaves things as they are', () async {
    bothEnds();
    final first = asked(TravelMode.driving).single;
    directions.setEndpoint(RouteField.to, c);
    first.answer.completeError(
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

  group('transit planned again', () {
    TransitPlan planOf(List<String> tags) =>
        fakePlan(journeys: tags.length, tags: tags);
    ModeOptions transitOptions() => directions.options(TravelMode.transit);

    /// Transit showing [plan], for a trip with both ends.
    Future<void> showing(TransitPlan plan) async {
      bothEnds();
      directions.setMode(TravelMode.transit);
      transit.asked.single.answer.complete(plan);
      await pumpEventQueue();
    }

    test('keeps its plan while it asks, and has the answer wait', () async {
      final plan = planOf(['-x', '-y']);
      await showing(plan);
      final question = directions.transitQuestion;
      var told = 0;
      directions.addListener(() => told++);
      final refresh = directions.refreshTransit();
      expect(directions.refreshing, isTrue);
      expect(transit.asked, hasLength(2));
      expect(transit.asked.last.from, a.point);
      expect(transit.asked.last.access, TransitAccess.walking);
      expect(directions.plan, same(plan));
      expect(directions.loading, isFalse);
      expect(told, 0, reason: 'nothing to show for it yet');
      // Asked once at a time.
      unawaited(directions.refreshTransit());
      expect(transit.asked, hasLength(2));

      final fresh = planOf(['-x', '-y']);
      transit.asked.last.answer.complete(fresh);
      await refresh;
      expect(directions.refreshing, isFalse);
      expect(directions.refreshed, same(fresh));
      expect(directions.plan, same(plan));
      expect(told, 1);
      expect(asked(TravelMode.driving), hasLength(1), reason: 'transit alone');

      directions.showRefreshed();
      expect(directions.plan, same(fresh));
      expect(directions.refreshed, isNull);
      expect(told, 2);
      // The trip is the one that was asked about.
      expect(directions.transitQuestion, question);
    });

    test(
      'the selected journey stays selected where the new plan has it',
      () async {
        await showing(planOf(['-x', '-y', '-z']));
        directions.selectJourney(1);
        final refresh = directions.refreshTransit();
        transit.asked.last.answer.complete(planOf(['-y', '-z', '-w']));
        await refresh;
        directions.showRefreshed();
        expect(directions.selectedJourney, 0);
        expect(directions.journey!.rides.first.tripId, 't40-y');
      },
    );

    test('with the selected journey gone, the first is selected', () async {
      await showing(planOf(['-x', '-y', '-z']));
      directions.selectJourney(1);
      final refresh = directions.refreshTransit();
      transit.asked.last.answer.complete(planOf(['-x', '-z']));
      await refresh;
      directions.showRefreshed();
      expect(directions.selectedJourney, 0);
      expect(directions.journey!.rides.first.tripId, 't40-x');
    });

    test('a plan that fails to come changes nothing', () async {
      final plan = planOf(['-x']);
      await showing(plan);
      var told = 0;
      directions.addListener(() => told++);
      final refresh = directions.refreshTransit();
      transit.asked.last.answer.completeError(
        const TransitPlanError('timed_out', 'slow'),
      );
      await refresh;
      expect(directions.refreshing, isFalse);
      expect(directions.refreshed, isNull);
      expect(directions.plan, same(plan));
      expect(directions.problem, isNull);
      expect(told, 0);
    });

    test('a plan with no journeys is a plan, and is shown', () async {
      await showing(planOf(['-x']));
      final refresh = directions.refreshTransit();
      transit.asked.last.answer.complete(emptyPlan('no_service'));
      await refresh;
      directions.showRefreshed();
      expect(directions.plan!.journeys, isEmpty);
      expect(directions.journey, isNull);
    });

    test('after a problem, a plan that comes takes its place', () async {
      bothEnds();
      directions.setMode(TravelMode.transit);
      transit.asked.single.answer.completeError(
        const TransitPlanError('timed_out', 'slow'),
      );
      await pumpEventQueue();
      expect(directions.problem, RouteProblem.failed);
      final refresh = directions.refreshTransit();
      transit.asked.last.answer.complete(planOf(['-x']));
      await refresh;
      directions.showRefreshed();
      expect(directions.problem, isNull);
      expect(directions.plan!.journeys, hasLength(1));
    });

    test('asked again for a changed trip, drops the refresh under way and '
        'the plan that waits', () async {
      await showing(planOf(['-x']));
      final first = directions.refreshTransit();
      transit.asked.last.answer.complete(planOf(['-x']));
      await first;
      expect(directions.refreshed, isNotNull);
      final second = directions.refreshTransit();
      final stale = transit.asked.last;
      final question = directions.transitQuestion;

      directions.setTransitAccess(TransitAccess.cycling);
      expect(directions.transitQuestion, question + 1);
      expect(directions.refreshed, isNull);
      expect(directions.refreshing, isFalse);
      expect(transitOptions().loading, isTrue);
      stale.answer.complete(planOf(['-y']));
      await second;
      expect(directions.refreshed, isNull);
      directions.showRefreshed();
      expect(transitOptions().loading, isTrue);
    });

    test('asks nothing while transit loads, or without both ends', () async {
      await directions.refreshTransit();
      expect(transit.asked, isEmpty);
      bothEnds();
      await directions.refreshTransit();
      expect(transit.asked, hasLength(1), reason: 'only the first question');
    });
  });
}

/// A platform with no location service, where every call throws.
class ThrowingLocation extends FakeLocation {
  @override
  Future<LocationPermission> checkPermission() =>
      throw UnsupportedError('none');
}
