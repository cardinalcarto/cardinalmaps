import 'dart:async';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_location.dart';
import 'package:cardinal/store/store.dart';
import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_navigation.dart';
import '../fakes/fake_routes.dart';

/// A controller over fakes, and the fakes, for one test.
class Harness {
  Harness() {
    controller = NavigationController(
      routes: routes,
      location: location,
      speaker: speaker,
      screen: screen,
      startSession: sessions.call,
    );
  }

  final routes = FakeRoutes();
  final location = FakeNavigationLocation();
  final speaker = FakeSpeaker();
  final screen = FakeScreenWake();
  final sessions = FakeSessions();
  late final NavigationController controller;

  final route = fakeRoute([
    [1.52, 42.50],
    [1.53, 42.51],
  ], language: 'pt-PT');

  NavigationState get state => controller.state;
  FakeTripSession get session => sessions.last;

  void start(FakeAsync async, {TravelMode mode = TravelMode.driving}) {
    controller.start(route: route, mode: mode);
    async.flushMicrotasks();
  }

  /// Sends [fix] (a good one by default) and lets the controller handle it.
  void fix(FakeAsync async, [NavLocation? fix]) {
    location.send(fix ?? fakeFix(42.5, 1.52));
    async.flushMicrotasks();
  }

  /// Starts and gets on the route.
  void onRoute(FakeAsync async) {
    start(async);
    fix(async);
    expect(state, NavigationState.onRoute);
  }

  /// Starts, gets on the route, and leaves it.
  void offRoute(FakeAsync async) {
    onRoute(async);
    session.next = fakeUpdate(deviation: NavDeviation.offRoute);
    fix(async);
    expect(state, NavigationState.offRoute);
  }

  /// Leaves the route and waits out the hold, so a new route is asked for.
  void rerouting(FakeAsync async) {
    offRoute(async);
    async.elapse(offRouteHold);
    expect(state, NavigationState.rerouting);
  }
}

void fakeAsync(void Function(FakeAsync async, Harness h) body) {
  FakeAsync().run((async) {
    final h = Harness();
    body(async, h);
    // Leaves nothing running between tests.
    unawaited(h.controller.end());
    async.flushMicrotasks();
    async.flushTimers();
  });
}

DirectionsRoute newRoute() => fakeRoute([
  [1.60, 42.60],
  [1.53, 42.51],
]);

void main() {
  group('transitions', () {
    test('idle to acquiring, on start: listens and keeps the screen on', () {
      fakeAsync((async, h) {
        expect(h.state, NavigationState.idle);
        h.start(async, mode: TravelMode.cycling);
        expect(h.state, NavigationState.acquiring);
        expect(h.controller.mode, TravelMode.cycling);
        expect(h.controller.route, same(h.route));
        expect(h.location.modes, [NavigationMode.cycling]);
        expect(h.location.listening, isTrue);
        expect(h.screen.on, isTrue);
        expect(h.speaker.languages, ['pt-PT']);
        expect(h.controller.language, 'pt-PT');
        expect(h.sessions.started, isEmpty);
      });
    });

    test('acquiring to on route, on the first fix good enough to start', () {
      fakeAsync((async, h) {
        h.start(async);
        h.fix(async, fakeFix(42.5, 1.52, accuracy: acquiringAccuracy + 1));
        expect(h.state, NavigationState.acquiring);
        expect(h.sessions.started, isEmpty);
        expect(h.controller.lastFixAt, isNotNull);

        final good = fakeFix(42.5, 1.52, accuracy: acquiringAccuracy);
        h.fix(async, good);
        expect(h.state, NavigationState.onRoute);
        final (route, mode, from) = h.sessions.started.single;
        expect(route, same(h.route.navigable));
        expect(mode, NavigationMode.driving);
        expect(from, good);
        expect(h.controller.update, isNotNull);
      });
    });

    test('acquiring to idle, on end', () {
      fakeAsync((async, h) {
        h.start(async);
        unawaited(h.controller.end());
        async.flushMicrotasks();
        expect(h.state, NavigationState.idle);
        expect(h.location.listening, isFalse);
        expect(h.screen.on, isFalse);
        expect(h.speaker.stopped, 1);
      });
    });

    test('on route to off route, when the session says so', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        // Off the step but still on the route isn't off it.
        h.session.next = fakeUpdate(deviation: NavDeviation.offStep);
        h.fix(async);
        expect(h.state, NavigationState.onRoute);
        h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
        h.fix(async);
        expect(h.state, NavigationState.offRoute);
      });
    });

    test('on route to arrived, when the trip completes, and it stays', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        h.session.next = fakeUpdate(status: TripStatus.complete);
        h.fix(async);
        expect(h.state, NavigationState.arrived);
        // Never ends by itself: still arrived, still listening, long after.
        async.elapse(const Duration(hours: 1));
        h.fix(async);
        expect(h.state, NavigationState.arrived);
        expect(h.location.listening, isTrue);
        expect(h.screen.on, isTrue);
      });
    });

    test('on route to idle, on end: lets go of everything', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        final session = h.session;
        unawaited(h.controller.end());
        async.flushMicrotasks();
        expect(h.state, NavigationState.idle);
        expect(session.disposed, isTrue);
        expect(h.location.listening, isFalse);
        expect(h.screen.on, isFalse);
        expect(h.speaker.stopped, 1);
        expect(h.controller.route, isNull);
        expect(h.controller.update, isNull);
        expect(h.controller.lastFixAt, isNull);
      });
    });

    test('off route to on route, back on the line before the hold ends', () {
      fakeAsync((async, h) {
        h.offRoute(async);
        async.elapse(offRouteHold ~/ 2);
        h.session.next = fakeUpdate();
        h.fix(async);
        expect(h.state, NavigationState.onRoute);
        async.elapse(offRouteHold * 2);
        expect(h.state, NavigationState.onRoute);
        expect(h.routes.rerouted, isEmpty);
      });
    });

    test('off route to rerouting, still off after the hold', () {
      fakeAsync((async, h) {
        h.offRoute(async);
        final from = fakeFix(42.49, 1.50);
        h.fix(async, from);
        async.elapse(offRouteHold - const Duration(milliseconds: 1));
        expect(h.state, NavigationState.offRoute);
        async.elapse(const Duration(milliseconds: 1));
        expect(h.state, NavigationState.rerouting);
        final question = h.routes.rerouted.single;
        expect(question.mode, TravelMode.driving);
        expect(question.from, const LngLat(1.50, 42.49));
        expect(question.to, const LngLat(1.53, 42.51));
        expect(h.session.rerouteFrom.single, from);
      });
    });

    test('off route to idle, on end, and the hold does nothing after', () {
      fakeAsync((async, h) {
        h.offRoute(async);
        unawaited(h.controller.end());
        async.flushMicrotasks();
        expect(h.state, NavigationState.idle);
        async.elapse(offRouteHold * 2);
        expect(h.state, NavigationState.idle);
        expect(h.routes.rerouted, isEmpty);
      });
    });

    test('rerouting to on route, along the new route from the latest fix', () {
      fakeAsync((async, h) {
        h.rerouting(async);
        // The old route keeps following you while the request runs.
        h.session.next = fakeUpdate(
          deviation: NavDeviation.offRoute,
          stepIndex: 1,
        );
        final latest = fakeFix(42.48, 1.49);
        h.fix(async, latest);
        expect(h.state, NavigationState.rerouting);
        expect(h.controller.update!.stepIndex, 1);

        final route = newRoute();
        h.session.next = fakeUpdate();
        h.routes.rerouted.single.answer.complete(route);
        async.flushMicrotasks();
        expect(h.state, NavigationState.onRoute);
        expect(h.controller.route, same(route));
        final (navigable, from) = h.session.replaced.single;
        expect(navigable, same(route.navigable));
        expect(from, latest);
        expect(h.controller.rerouteFailure, isNull);
      });
    });

    test('rerouting to off route on failure, trying again after a backoff', () {
      fakeAsync((async, h) {
        h.rerouting(async);
        h.routes.rerouted.single.answer.completeError(
          const DirectionsError(
            kind: DirectionsErrorKind.failed,
            message: 'broken',
          ),
        );
        async.flushMicrotasks();
        expect(h.state, NavigationState.offRoute);
        expect(h.controller.rerouteFailure, RerouteFailure.failed);

        async.elapse(rerouteBackoff - const Duration(milliseconds: 1));
        expect(h.routes.rerouted, hasLength(1));
        async.elapse(const Duration(milliseconds: 1));
        expect(h.state, NavigationState.rerouting);
        expect(h.routes.rerouted, hasLength(2));

        // Each failure waits twice as long, up to the most.
        h.routes.rerouted.last.answer.completeError(const OutsideOfflineArea());
        async.flushMicrotasks();
        expect(h.controller.rerouteFailure, RerouteFailure.outsideOfflineArea);
        async.elapse(rerouteBackoff * 2 - const Duration(milliseconds: 1));
        expect(h.routes.rerouted, hasLength(2));
        async.elapse(const Duration(milliseconds: 1));
        expect(h.routes.rerouted, hasLength(3));
        for (var i = 0; i < 4; i++) {
          h.routes.rerouted.last.answer.completeError(const NoOfflineArea());
          async.flushMicrotasks();
          async.elapse(maxRerouteBackoff);
        }
        expect(h.routes.rerouted, hasLength(7));
        expect(h.controller.rerouteFailure, RerouteFailure.noOfflineArea);
      });
    });

    test('rerouting to off route with no route keeps the reason', () {
      fakeAsync((async, h) {
        h.rerouting(async);
        const none = DirectionsError(
          kind: DirectionsErrorKind.noRoute,
          message: 'no path',
        );
        h.routes.rerouted.single.answer.completeError(none);
        async.flushMicrotasks();
        expect(h.state, NavigationState.offRoute);
        expect(h.controller.rerouteFailure, RerouteFailure.noRoute);
        expect(h.controller.rerouteError, none);
        // Kept while still off the route, until you're back on it.
        h.fix(async);
        expect(h.controller.rerouteFailure, RerouteFailure.noRoute);
        h.session.next = fakeUpdate();
        h.fix(async);
        expect(h.state, NavigationState.onRoute);
        expect(h.controller.rerouteFailure, isNull);
      });
    });

    test('rerouting to idle, on end, dropping the late answer', () {
      fakeAsync((async, h) {
        h.rerouting(async);
        final session = h.session;
        unawaited(h.controller.end());
        async.flushMicrotasks();
        expect(h.state, NavigationState.idle);
        h.routes.rerouted.single.answer.complete(newRoute());
        async.flushMicrotasks();
        expect(h.state, NavigationState.idle);
        expect(session.replaced, isEmpty);
      });
    });

    test('arrived to idle, on end', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        h.session.next = fakeUpdate(status: TripStatus.complete);
        h.fix(async);
        expect(h.state, NavigationState.arrived);
        unawaited(h.controller.end());
        async.flushMicrotasks();
        expect(h.state, NavigationState.idle);
        expect(h.location.listening, isFalse);
        expect(h.screen.on, isFalse);
      });
    });
  });

  group('rerouting', () {
    test('waits the least time between reroutes', () {
      fakeAsync((async, h) {
        h.rerouting(async);
        h.session.next = fakeUpdate();
        h.routes.rerouted.single.answer.complete(newRoute());
        async.flushMicrotasks();
        expect(h.state, NavigationState.onRoute);

        // Off the new route at once: the hold ends before the interval does.
        h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
        h.fix(async);
        async.elapse(offRouteHold);
        expect(h.state, NavigationState.offRoute);
        expect(h.routes.rerouted, hasLength(1));
        async.elapse(minRerouteInterval - offRouteHold * 2);
        expect(h.routes.rerouted, hasLength(1));
        async.elapse(offRouteHold);
        expect(h.state, NavigationState.rerouting);
        expect(h.routes.rerouted, hasLength(2));
      });
    });

    test(
      'an update that completes the trip off route goes on route, then arrived',
      () {
        fakeAsync((async, h) {
          h.offRoute(async);
          h.session.next = fakeUpdate(status: TripStatus.complete);
          h.fix(async);
          expect(h.state, NavigationState.arrived);
        });
      },
    );

    test('maps errors to failures', () {
      expect(
        RerouteFailure.of(
          const DirectionsError(kind: DirectionsErrorKind.noRoute, message: ''),
        ),
        RerouteFailure.noRoute,
      );
      expect(
        RerouteFailure.of(
          const DirectionsError(kind: DirectionsErrorKind.failed, message: ''),
        ),
        RerouteFailure.failed,
      );
      expect(
        RerouteFailure.of(const OutsideOfflineArea()),
        RerouteFailure.outsideOfflineArea,
      );
      expect(
        RerouteFailure.of(const NoOfflineArea()),
        RerouteFailure.noOfflineArea,
      );
      expect(RerouteFailure.of(StateError('x')), RerouteFailure.failed);
    });
  });

  group('fixes', () {
    test('are handled one at a time, keeping only the newest waiting', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        final session = h.session;
        session.hold = Completer();
        h.fix(async, fakeFix(1, 1));
        h.fix(async, fakeFix(2, 2));
        h.fix(async, fakeFix(3, 3));
        expect(session.inFlight, 1);
        session.hold!.complete();
        session.hold = null;
        async.flushMicrotasks();
        expect(session.mostInFlight, 1);
        expect([for (final f in session.fixes) f.lat], [1, 3]);
      });
    });

    test('keep the time of the last one, even one too poor to use', () {
      fakeAsync((async, h) {
        h.start(async);
        h.fix(
          async,
          fakeFix(42.5, 1.52, accuracy: 500, timestampMs: 1763000001234),
        );
        expect(
          h.controller.lastFixAt,
          DateTime.fromMillisecondsSinceEpoch(1763000001234),
        );
      });
    });

    test('stopping is a problem on the trip, not a state, until one comes', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        h.location.fail(const NavigationLocationError(LocationProblem.denied));
        async.flushMicrotasks();
        expect(h.state, NavigationState.onRoute);
        expect(h.controller.locationProblem, LocationProblem.denied);

        unawaited(h.location.close());
        async.flushMicrotasks();
        expect(h.controller.locationProblem, LocationProblem.denied);
        unawaited(h.controller.resumeLocation());
        async.flushMicrotasks();
        expect(h.location.listening, isTrue);
        h.fix(async);
        expect(h.controller.locationProblem, isNull);

        h.location.fail(StateError('broken'));
        async.flushMicrotasks();
        expect(h.controller.locationProblem, LocationProblem.failed);
      });
    });

    test('a stream that ends by itself is a failure', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        unawaited(h.location.close());
        async.flushMicrotasks();
        expect(h.controller.locationProblem, LocationProblem.failed);
        expect(h.state, NavigationState.onRoute);
      });
    });
  });

  group('speech', () {
    test('says each announcement once, across fixes and a new route', () {
      fakeAsync((async, h) {
        h.sessions.start = fakeUpdate(spoken: fakeSpoken('a', 'Head north'));
        h.onRoute(async);
        h.fix(async);
        h.session.next = fakeUpdate(spoken: fakeSpoken('b', 'Turn left'));
        h.fix(async);
        h.fix(async);
        h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
        h.fix(async);
        async.elapse(offRouteHold);
        h.session.next = fakeUpdate(spoken: fakeSpoken('c', 'Head south'));
        h.routes.rerouted.single.answer.complete(newRoute());
        async.flushMicrotasks();
        h.fix(async);
        expect(h.speaker.said, ['Head north', 'Turn left', 'Head south']);
      });
    });
  });

  test("transit can't be navigated, and a second start is refused", () {
    fakeAsync((async, h) {
      expect(
        () => h.controller.start(route: h.route, mode: TravelMode.transit),
        throwsArgumentError,
      );
      expect(h.state, NavigationState.idle);
      h.start(async);
      expect(
        () => h.controller.start(route: h.route, mode: TravelMode.driving),
        throwsStateError,
      );
    });
  });
}
