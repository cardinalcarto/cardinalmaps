import 'dart:async';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/navigation/road_matcher.dart';
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
  Harness({String language = 'pt-PT', SettingsStore? settings})
    : settings = settings ?? SettingsStore.inMemory(),
      route = fakeRoute([
        [1.52, 42.50],
        [1.53, 42.51],
      ], language: language) {
    controller = NavigationController(
      routes: routes,
      location: location,
      speaker: speaker,
      screen: screen,
      matcher: matcher,
      settings: this.settings,
      startSession: sessions.call,
    );
  }

  final SettingsStore settings;

  final routes = FakeRoutes();
  final location = FakeNavigationLocation();
  final speaker = FakeSpeaker();
  final screen = FakeScreenWake();
  final sessions = FakeSessions();
  final matcher = FakeRoadMatcher();
  late final NavigationController controller;

  /// The route started with, its announcements in the harness's language.
  final DirectionsRoute route;

  NavigationState get state => controller.state;
  FakeTripSession get session => sessions.last;

  void start(
    FakeAsync async, {
    TravelMode mode = TravelMode.driving,
    bool wheelchair = false,
  }) {
    controller.start(route: route, mode: mode, wheelchair: wheelchair);
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

void fakeAsync(
  void Function(FakeAsync async, Harness h) body, {
  String language = 'pt-PT',
}) {
  FakeAsync().run((async) {
    final h = Harness(language: language);
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

    test(
      'a trip walked by wheelchair asks for its new route by wheelchair',
      () {
        fakeAsync((async, h) {
          h.start(async, mode: TravelMode.walking, wheelchair: true);
          expect(h.controller.wheelchair, isTrue);
          h.fix(async);
          h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
          h.fix(async);
          async.elapse(offRouteHold);
          final question = h.routes.rerouted.single;
          expect(question.mode, TravelMode.walking);
          expect(question.wheelchair, isTrue);
        });
      },
    );

    test('a trip walked on foot asks for its new route on foot', () {
      fakeAsync((async, h) {
        h.start(async, mode: TravelMode.walking);
        expect(h.controller.wheelchair, isFalse);
        h.fix(async);
        h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
        h.fix(async);
        async.elapse(offRouteHold);
        final question = h.routes.rerouted.single;
        expect(question.mode, TravelMode.walking);
        expect(question.wheelchair, isFalse);
      });
    });

    test('wheelchair means nothing to a trip that is not walked', () {
      fakeAsync((async, h) {
        h.start(async, wheelchair: true);
        expect(h.controller.wheelchair, isFalse);
        h.fix(async);
        h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
        h.fix(async);
        async.elapse(offRouteHold);
        expect(h.routes.rerouted.single.wheelchair, isFalse);
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

    test('leaving the route cuts off what is being said, once', () {
      fakeAsync((async, h) {
        h.sessions.start = fakeUpdate(spoken: fakeSpoken('a', 'Turn left'));
        h.onRoute(async);
        expect(h.speaker.log, ['Turn left']);
        h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
        h.fix(async);
        expect(h.speaker.log, ['Turn left', '(stop)']);

        // Staying off, rerouting, and a failed reroute that goes back to off route don't
        // cut anything off again: only the move off the route does.
        h.fix(async);
        async.elapse(offRouteHold);
        expect(h.state, NavigationState.rerouting);
        h.routes.rerouted.single.answer.completeError(const NoOfflineArea());
        async.flushMicrotasks();
        expect(h.state, NavigationState.offRoute);
        h.fix(async);
        expect(h.speaker.stopped, 1);
      });
    });

    test(
      'says "Rerouting" once each time you leave, right after the cut-off',
      () {
        fakeAsync((async, h) {
          h.sessions.start = fakeUpdate(spoken: fakeSpoken('a', 'Turn left'));
          h.onRoute(async);
          h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
          h.fix(async);
          expect(h.speaker.log, ['Turn left', '(stop)', 'Rerouting']);

          // Not again while off, rerouting, or trying again after a failed reroute.
          h.fix(async);
          async.elapse(offRouteHold);
          h.routes.rerouted.single.answer.completeError(const NoOfflineArea());
          async.flushMicrotasks();
          h.fix(async);
          async.elapse(rerouteBackoff);
          expect(h.routes.rerouted, hasLength(2));
          h.fix(async);
          expect(h.speaker.log, ['Turn left', '(stop)', 'Rerouting']);

          // Back on the line and off it again is a new departure, so it's said again.
          h.session.next = fakeUpdate();
          h.routes.rerouted.last.answer.completeError(const NoOfflineArea());
          async.flushMicrotasks();
          h.fix(async);
          expect(h.state, NavigationState.onRoute);
          h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
          h.fix(async);
          expect(h.speaker.log, [
            'Turn left',
            '(stop)',
            'Rerouting',
            '(stop)',
            'Rerouting',
          ]);
        }, language: 'en-GB');
      },
    );

    test('says "Rerouting" only in a language the app has it in', () {
      fakeAsync((async, h) {
        h.offRoute(async);
        expect(h.speaker.log, ['(stop)']);
      }, language: 'de-DE');
    });

    test("the old route's announcements aren't said off it, or late", () {
      fakeAsync((async, h) {
        h.offRoute(async);
        h.session.next = fakeUpdate(
          deviation: NavDeviation.offRoute,
          spoken: fakeSpoken('a', 'Turn left'),
        );
        h.fix(async);
        async.elapse(offRouteHold);
        expect(h.state, NavigationState.rerouting);
        h.session.next = fakeUpdate(
          deviation: NavDeviation.offRoute,
          spoken: fakeSpoken('b', 'Turn right'),
        );
        h.fix(async);
        expect(h.state, NavigationState.rerouting);

        // No route came; back on the line, the ones that came due while off stay unsaid.
        h.routes.rerouted.single.answer.completeError(const NoOfflineArea());
        async.flushMicrotasks();
        h.session.next = fakeUpdate(spoken: fakeSpoken('b', 'Turn right'));
        h.fix(async);
        expect(h.state, NavigationState.onRoute);
        h.session.next = fakeUpdate(spoken: fakeSpoken('a', 'Turn left'));
        h.fix(async);
        expect(h.speaker.said, isEmpty);
      });
    });

    test('speaking picks up back on the line', () {
      fakeAsync((async, h) {
        h.offRoute(async);
        h.session.next = fakeUpdate(spoken: fakeSpoken('a', 'Turn left'));
        h.fix(async);
        expect(h.state, NavigationState.onRoute);
        expect(h.speaker.said, ['Turn left']);
      });
    });

    test('speaking picks up with the new route', () {
      fakeAsync((async, h) {
        h.rerouting(async);
        h.session.next = fakeUpdate(spoken: fakeSpoken('n', 'Head south'));
        h.routes.rerouted.single.answer.complete(newRoute());
        async.flushMicrotasks();
        expect(h.state, NavigationState.onRoute);
        h.session.next = fakeUpdate(spoken: fakeSpoken('m', 'Turn right'));
        h.fix(async);
        expect(h.speaker.said, ['Head south', 'Turn right']);
      });
    });

    test('an arrival that comes with getting back on the line is said', () {
      fakeAsync((async, h) {
        h.offRoute(async);
        h.session.next = fakeUpdate(
          status: TripStatus.complete,
          spoken: fakeSpoken('z', 'You have arrived'),
        );
        h.fix(async);
        expect(h.state, NavigationState.arrived);
        expect(h.speaker.said, ['You have arrived']);
      });
    });
  });

  group('mute', () {
    /// The stored setting, read in [async]'s time.
    bool? stored(FakeAsync async, SettingsStore settings) {
      bool? value;
      unawaited(settings.get(navigationMuted).then((v) => value = v));
      async.flushMicrotasks();
      return value;
    }

    test('stops what is being said, and says nothing more', () {
      fakeAsync((async, h) {
        h.sessions.start = fakeUpdate(spoken: fakeSpoken('a', 'Turn left'));
        h.onRoute(async);
        var notified = 0;
        h.controller.addListener(() => notified++);
        h.controller.setMuted(true);
        expect(h.controller.muted, isTrue);
        expect(notified, 1);
        expect(h.speaker.log, ['Turn left', '(stop)']);

        h.session.next = fakeUpdate(spoken: fakeSpoken('b', 'Turn right'));
        h.fix(async);
        expect(h.speaker.said, ['Turn left']);
        // Muting again does nothing more.
        h.controller.setMuted(true);
        expect(h.speaker.stopped, 1);
      });
    });

    test('says nothing on leaving the route, "Rerouting" included', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        h.controller.setMuted(true);
        h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
        h.fix(async);
        expect(h.state, NavigationState.offRoute);
        expect(h.speaker.said, isEmpty);

        // Nor with the new route.
        async.elapse(offRouteHold);
        h.session.next = fakeUpdate(spoken: fakeSpoken('n', 'Head south'));
        h.routes.rerouted.single.answer.complete(newRoute());
        async.flushMicrotasks();
        expect(h.state, NavigationState.onRoute);
        expect(h.speaker.said, isEmpty);
      }, language: 'en-GB');
    });

    test('unmuted, the next announcement is said, and none passed over', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        h.controller.setMuted(true);
        h.session.next = fakeUpdate(spoken: fakeSpoken('a', 'Turn left'));
        h.fix(async);
        h.controller.setMuted(false);
        expect(h.controller.muted, isFalse);
        // Unmuting says nothing by itself, and what came due while muted isn't said
        // late.
        expect(h.speaker.said, isEmpty);
        h.fix(async);
        expect(h.speaker.said, isEmpty);
        h.session.next = fakeUpdate(spoken: fakeSpoken('b', 'Turn right'));
        h.fix(async);
        expect(h.speaker.said, ['Turn right']);

        // And "Rerouting" is said again.
        h.session.next = fakeUpdate(deviation: NavDeviation.offRoute);
        h.fix(async);
        expect(h.speaker.said, ['Turn right', 'Rerouting']);
      }, language: 'en-GB');
    });

    test('persists across trips, as the setting', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        h.controller.setMuted(true);
        unawaited(h.controller.end());
        async.flushMicrotasks();
        expect(h.controller.muted, isTrue);
        h.sessions.start = fakeUpdate(spoken: fakeSpoken('a', 'Turn left'));
        h.onRoute(async);
        expect(h.controller.muted, isTrue);
        expect(h.speaker.said, isEmpty);
        expect(stored(async, h.settings), isTrue);

        h.controller.setMuted(false);
        async.flushMicrotasks();
        expect(stored(async, h.settings), isFalse);
      });
    });

    test('a new controller starts muted with the setting on', () {
      FakeAsync().run((async) {
        final settings = SettingsStore.inMemory();
        unawaited(settings.set(navigationMuted, true));
        async.flushMicrotasks();
        final h = Harness(settings: settings);
        var notified = 0;
        h.controller.addListener(() => notified++);
        // Read before a trip's first announcement can come due.
        h.sessions.start = fakeUpdate(spoken: fakeSpoken('a', 'Turn left'));
        h.onRoute(async);
        expect(h.controller.muted, isTrue);
        expect(notified, greaterThan(0));
        expect(h.speaker.said, isEmpty);
        unawaited(h.controller.end());
        async.flushMicrotasks();
      });
    });

    test('muting set before the setting is read stands', () {
      FakeAsync().run((async) {
        final settings = SettingsStore.inMemory();
        unawaited(settings.set(navigationMuted, true));
        async.flushMicrotasks();
        final h = Harness(settings: settings);
        // Before the stored value is read.
        h.controller.setMuted(false);
        async.flushMicrotasks();
        expect(h.controller.muted, isFalse);
        expect(stored(async, settings), isFalse);
      });
    });
  });

  group('matching', () {
    const t0 = 1763000000000;
    NavLocation at(int second, {double lat = 42.5}) =>
        fakeFix(lat, 1.52, timestampMs: t0 + second * 1000);
    const road = MatchedPosition(
      lat: 42.5001,
      lng: 1.52,
      kind: MatchKind.matched,
      heading: 90,
    );

    test('matches a trip walked by wheelchair by wheelchair', () {
      fakeAsync((async, h) {
        h.start(async, mode: TravelMode.walking, wheelchair: true);
        h.fix(async, at(0));
        h.fix(async, at(1));
        final (mode, _, :wheelchair) = h.matcher.asked.last;
        expect(mode, NavigationMode.walking);
        expect(wheelchair, isTrue);
        // And not the next trip, on foot.
        unawaited(h.controller.end());
        async.flushMicrotasks();
        expect(h.controller.wheelchair, isFalse);
        h.start(async, mode: TravelMode.walking);
        h.fix(async, at(0));
        h.fix(async, at(1));
        expect(h.matcher.asked.last.wheelchair, isFalse);
      });
    });

    test('matches each fix with the window before it, for the session', () {
      fakeAsync((async, h) {
        h.start(async, mode: TravelMode.walking);
        h.fix(async, at(0));
        expect(h.state, NavigationState.onRoute);
        // The fix the session starts from isn't matched.
        expect(h.matcher.asked, isEmpty);

        h.matcher.answer = road;
        h.fix(async, at(1));
        h.fix(async, at(2));
        final (mode, fixes, :wheelchair) = h.matcher.asked.last;
        expect(mode, NavigationMode.walking);
        expect(wheelchair, isFalse);
        // The fix it started from too.
        expect(fixes, [at(0), at(1), at(2)]);
        expect(h.session.fixes.last, at(2));
        expect(h.session.matches.last, road);

        // No match goes to the session as none.
        h.matcher.answer = null;
        h.fix(async, at(3));
        expect(h.session.matches.last, isNull);
      });
    });

    test('the window keeps the last 30 s, starting over after a gap', () {
      final window = MatchWindow();
      for (var s = 0; s < 40; s++) {
        window.add(at(s));
      }
      expect(window.fixes.first, at(10));
      expect(window.fixes.last, at(39));
      expect(window.fixes, hasLength(30));

      // Fixes faster than a second apart: only the newest [maxMatchFixes].
      final fast = MatchWindow();
      for (var i = 0; i < 100; i++) {
        fast.add(fakeFix(42.5, 1.52, timestampMs: t0 + i * 200));
      }
      expect(fast.fixes, hasLength(maxMatchFixes));
      expect(fast.fixes.last.timestampMs.toInt(), t0 + 99 * 200);

      // Over 10 s without one, the window starts over.
      window.add(at(39 + 11));
      expect(window.fixes, [at(50)]);
      window.add(at(60));
      expect(window.fixes, [at(50), at(60)]);
    });

    test('a fix replaced before it was handled is still in the window', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        final session = h.session;
        session.hold = Completer();
        h.fix(async, at(1));
        h.fix(async, at(2));
        h.fix(async, at(3));
        session.hold!.complete();
        session.hold = null;
        async.flushMicrotasks();
        expect(h.matcher.asked.last.$2.last, at(3));
        expect(h.matcher.asked.last.$2, contains(at(2)));
      });
    });

    test('a new route starts from where navigation put you', () {
      fakeAsync((async, h) {
        h.onRoute(async);
        final raw = at(1, lat: 42.49);
        final matched = fakeFix(42.4901, 1.5201, course: 90);
        h.session.next = fakeUpdate(
          deviation: NavDeviation.offRoute,
          location: raw,
          effective: matched,
          matched: true,
        );
        h.fix(async, raw);
        async.elapse(offRouteHold);
        expect(h.state, NavigationState.rerouting);
        expect(h.routes.rerouted.single.from, const LngLat(1.5201, 42.4901));
        expect(h.session.rerouteFrom.single, matched);
      });
    });
  });

  test("transit can't be navigated, and a second start is refused", () {
    fakeAsync((async, h) {
      expect(
        () => h.controller.start(
          route: h.route,
          mode: TravelMode.transit,
          wheelchair: false,
        ),
        throwsArgumentError,
      );
      expect(h.state, NavigationState.idle);
      h.start(async);
      expect(
        () => h.controller.start(
          route: h.route,
          mode: TravelMode.driving,
          wheelchair: false,
        ),
        throwsStateError,
      );
    });
  });
}
