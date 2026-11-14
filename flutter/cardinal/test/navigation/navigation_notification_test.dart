import 'dart:async';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/navigation/maneuver_icon.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_location.dart';
import 'package:cardinal/navigation/navigation_notification.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/units.dart';
import 'package:fake_async/fake_async.dart';
import 'package:flutter/foundation.dart' show VoidCallback;
import 'package:flutter/material.dart' show Locale;
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_navigation.dart';
import '../fakes/fake_routes.dart';

/// Writes down what it's told to show, and when it's asked for permission.
class FakeNotifier implements NavigationNotifier {
  final shown = <NavigationNotice>[];
  int hidden = 0;
  int asked = 0;
  VoidCallback? end;

  @override
  Future<void> show(NavigationNotice notice) async => shown.add(notice);

  @override
  Future<void> hide() async => hidden++;

  @override
  Future<bool> requestPermission() async {
    asked++;
    return false;
  }

  @override
  set onEnd(VoidCallback? onEnd) => end = onEnd;
}

final words = lookupAppLocalizations(const Locale('en'));

class Harness {
  Harness({SettingsStore? settings})
    : settings = settings ?? SettingsStore.inMemory() {
    controller = fakeNavigationController(
      location: location,
      sessions: sessions,
      routes: routes,
      settings: this.settings,
    );
    notification = TripNotification(
      controller: controller,
      notifier: notifier,
      settings: this.settings,
      words: () => words,
    )..start();
  }

  final SettingsStore settings;
  final location = FakeNavigationLocation();
  final sessions = FakeSessions();
  final routes = FakeRoutes();
  final notifier = FakeNotifier();
  late final NavigationController controller;
  late final TripNotification notification;

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

  void fix(FakeAsync async, [NavUpdate? next]) {
    if (next != null && sessions.sessions.isNotEmpty) sessions.last.next = next;
    location.send(fakeFix(42.5, 1.52));
    async.flushMicrotasks();
  }

  NavigationNotice? get last => notifier.shown.lastOrNull;
}

void run(void Function(FakeAsync async, Harness h) body) {
  FakeAsync().run((async) {
    final h = Harness();
    async.flushMicrotasks();
    body(async, h);
    unawaited(h.controller.end());
    h.notification.dispose();
    async.flushMicrotasks();
    async.flushTimers();
  });
}

NavUpdate turn(double meters, {String text = 'Turn right onto Pine Street'}) =>
    fakeUpdate(
      visual: fakeVisual(
        fakeContent(
          text,
          type: ManeuverType.turn,
          modifier: ManeuverModifier.right,
        ),
      ),
      toManeuver: meters,
    );

/// Distances in feet and miles, whatever the test machine's locale.
Future<void> imperial(SettingsStore settings) =>
    settings.set(unitSystem, UnitSystem.imperial);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('what it says', () {
    test('nothing shows with no trip', () {
      run((async, h) {
        expect(h.notifier.shown, isEmpty);
        expect(h.notifier.hidden, 0);
      });
    });

    test('waiting for GPS as the trip starts', () {
      run((async, h) {
        h.start(async);
        expect(h.last?.title, words.navigationWaitingForGps);
        expect(h.last?.text, isNull);
        expect(h.last?.glyph, isNull);
        expect(h.last?.channel, words.navigationNotificationChannel);
        expect(h.last?.end, words.navigationNotificationEnd);
      });
    });

    test('the next maneuver: its distance, instruction and icon', () {
      run((async, h) {
        unawaited(imperial(h.settings));
        async.flushMicrotasks();
        h.sessions.start = turn(150);
        h.start(async);
        h.fix(async);
        expect(h.controller.state, NavigationState.onRoute);
        expect(h.last?.title, '500 ft');
        expect(h.last?.text, 'Turn right onto Pine Street');
        expect(
          h.last?.glyph,
          maneuverIcon(ManeuverType.turn, ManeuverModifier.right),
        );
        expect(h.last?.glyph, isNotNull);
      });
    });

    test('a maneuver with no distance yet shows its instruction alone', () {
      run((async, h) {
        h.sessions.start = fakeUpdate(
          visual: fakeVisual(fakeContent('Head north')),
        );
        h.start(async);
        h.fix(async);
        expect(h.last?.title, 'Head north');
        expect(h.last?.text, isNull);
      });
    });

    test('on the route with no maneuver yet says Navigating', () {
      run((async, h) {
        h.start(async);
        h.fix(async);
        expect(h.controller.state, NavigationState.onRoute);
        expect(h.last?.title, words.navigationNotificationText);
      });
    });

    test('off the route, then finding a new route, then a failure', () {
      run((async, h) {
        h.sessions.start = turn(150);
        h.start(async);
        h.fix(async);
        h.fix(async, fakeUpdate(deviation: NavDeviation.offRoute));
        expect(h.last?.title, words.navigationOffRoute);
        async.elapse(offRouteHold);
        expect(h.controller.state, NavigationState.rerouting);
        expect(h.last?.title, words.navigationRerouting);
        h.routes.rerouted.last.answer.completeError(Exception('down'));
        async.flushMicrotasks();
        expect(h.last?.title, words.navigationRerouteFailed);
        expect(h.last?.text, words.navigationRerouteRetrying);
      });
    });

    test('a location problem says what to do', () {
      run((async, h) {
        h.start(async);
        h.location.fail(const NavigationLocationError(LocationProblem.off));
        async.flushMicrotasks();
        expect(h.last?.title, words.navigationLocationOff);
        expect(h.last?.text, words.navigationLocationOffDetail);
      });
    });

    test("arrived: you've arrived, the destination, and the arrival icon", () {
      run((async, h) {
        h.start(async, destination: 'Pike Place Market');
        h.fix(async);
        h.fix(async, fakeUpdate(status: TripStatus.complete));
        expect(h.controller.state, NavigationState.arrived);
        expect(h.last?.title, words.navigationArrived);
        expect(h.last?.text, 'Pike Place Market');
        expect(
          h.last?.glyph,
          maneuverIcon(ManeuverType.arrive, ManeuverModifier.right),
        );
      });
    });
  });

  group('updates', () {
    test('only when the rounded distance or the maneuver changes', () {
      run((async, h) {
        unawaited(h.settings.set(unitSystem, UnitSystem.metric));
        async.flushMicrotasks();
        h.sessions.start = turn(500);
        h.start(async);
        h.fix(async);
        expect(h.last?.title, '500 m');
        final before = h.notifier.shown.length;
        // 520 and 480 m round to 500 m, as shown: nothing new.
        h.fix(async, turn(520));
        h.fix(async, turn(480));
        expect(h.notifier.shown.length, before);
        // A new rounded distance.
        h.fix(async, turn(300));
        expect(h.notifier.shown.length, before + 1);
        expect(h.last?.title, '300 m');
        // A new maneuver at the same distance.
        h.fix(async, turn(300, text: 'Turn right onto Pike Street'));
        expect(h.notifier.shown.length, before + 2);
        expect(h.last?.text, 'Turn right onto Pike Street');
        // The same again.
        h.fix(async, turn(300, text: 'Turn right onto Pike Street'));
        expect(h.notifier.shown.length, before + 2);
      });
    });

    test('a change of units shows the distance in them', () {
      run((async, h) {
        unawaited(h.settings.set(unitSystem, UnitSystem.metric));
        async.flushMicrotasks();
        h.sessions.start = turn(50);
        h.start(async);
        h.fix(async);
        expect(h.last?.title, '50 m');
        unawaited(imperial(h.settings));
        async.flushMicrotasks();
        expect(h.last?.title, '150 ft');
      });
    });

    test('ending the trip hides it', () {
      run((async, h) {
        h.start(async);
        unawaited(h.controller.end());
        async.flushMicrotasks();
        expect(h.notifier.hidden, 1);
        // A second trip shows it again.
        h.start(async);
        expect(h.last?.title, words.navigationWaitingForGps);
      });
    });
  });

  group('End', () {
    test("the notification's End ends the trip", () {
      run((async, h) {
        h.start(async);
        h.fix(async);
        expect(h.notifier.end, isNotNull);
        h.notifier.end!();
        async.flushMicrotasks();
        expect(h.controller.state, NavigationState.idle);
        expect(h.location.listening, isFalse);
        expect(h.notifier.hidden, 1);
      });
    });

    test('works once arrived', () {
      run((async, h) {
        h.start(async);
        h.fix(async);
        h.fix(async, fakeUpdate(status: TripStatus.complete));
        h.notifier.end!();
        async.flushMicrotasks();
        expect(h.controller.state, NavigationState.idle);
      });
    });
  });

  group('permission', () {
    test('asked for as the first trip starts, and never again', () {
      run((async, h) {
        expect(h.notifier.asked, 0);
        h.start(async);
        expect(h.notifier.asked, 1);
        h.fix(async);
        unawaited(h.controller.end());
        async.flushMicrotasks();
        h.start(async);
        expect(h.notifier.asked, 1);
      });
    });

    test('not asked again after the app starts over, denied or not', () {
      FakeAsync().run((async) {
        final settings = SettingsStore.inMemory();
        final first = Harness(settings: settings);
        first.start(async);
        expect(first.notifier.asked, 1);
        unawaited(first.controller.end());
        first.notification.dispose();
        async.flushMicrotasks();
        final second = Harness(settings: settings);
        second.start(async);
        expect(second.notifier.asked, 0);
        // The trip goes on, and the notification is still shown to the system.
        expect(second.last?.title, words.navigationWaitingForGps);
        unawaited(second.controller.end());
        second.notification.dispose();
        async.flushMicrotasks();
      });
    });
  });
}
