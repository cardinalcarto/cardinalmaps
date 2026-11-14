// Screenshots of the navigation screen on the Linux desktop, for reviewing its look.
// A development tool, not a test: run it with `tool/navigation_screenshots.sh`, which
// builds the app in profile mode for the desktop and drives it with `flutter drive`.
// It refuses to run anywhere else: phones only ever get profile or release builds, and
// they're built by their own scripts.
//
// It runs the app's real `main()`, in a 412 × 915 view at a device pixel ratio of 2 with
// a phone's system bars, and takes simulated drives (`CARDINAL_SIMULATE_NAVIGATION`)
// from downtown Seattle to the University District, routed offline in the desktop's
// downloaded Seattle region. Nothing is tapped or typed: it opens the screens through
// the router, starts and ends trips through the navigation controller, and moves the
// map through its controller, as the app's own code does. Each screenshot is the app's
// whole view, rendered from its root repaint boundary, the map included, into
// `CARDINAL_SCREENSHOTS_DIR`. Which ones is `CARDINAL_SCREENSHOTS_SET`'s choice:
//
// `banner` (the default), for the maneuver banner and the recenter button:
//
// 1. `1-portrait-secondary.png`: the banner with a secondary row, if the route has one.
// 2. `2-portrait-turn.png`: the banner for a plain turn.
// 3. `3-landscape.png`: the same in landscape.
// 4. `4-roundabout.png`: a roundabout, if the route has one.
// 5. `5-recenter.png`: the recenter button, after the map is moved off the chevron.
// 6. `6-portrait-secondary-dark.png`: 1 in the dark theme.
//
// `progress`, for the trip-progress pill, a kilometer into the long drive:
//
// 1. `1-portrait.png`: mid-trip, in portrait.
// 2. `2-landscape.png`: the same in landscape.
// 3. `3-portrait-dark.png`: 1 in the dark theme.
// 4. `4-portrait-24h.png`: 1 with the device's clock set to 24 hours.
//
// `buttons`, for the road-name pill and the camera and mute buttons, on the long drive
// once it's on a named road and 300 m along:
//
// 1. `1-portrait-following.png`: following, in portrait: the road name, Route overview
//    and Mute.
// 2. `2-portrait-overview.png`: the route overview, with Recenter and no road name.
// 3. `3-landscape-following.png`: 1 in landscape.
// 4. `4-portrait-muted.png`: 1, muted.
// 5. `5-portrait-dark.png`: 1 in the dark theme.
//
// `steps`, for the maneuver banner's list of steps, on the long drive once it's 300 m
// along with steps ahead:
//
// 1. `1-portrait-collapsed.png`: the banner closed, with its handle.
// 2. `2-portrait-expanded.png`: the banner open into the list.
// 3. `3-landscape-expanded.png`: 2 in landscape.
// 4. `4-portrait-expanded-dark.png`: 2 in the dark theme.
//
// The list is opened and closed through the banner's own `expanded` notifier, as a tap
// on its handle sets it, rather than by tapping.
//
// `status`, for the status cards in the banner's place, the GPS-lost indicator and the
// arrival, each made by steering the simulated trip (`SimulatedTripLocation`, as
// `main.dart`'s `simulatedTrip`): holding its fixes back, setting them off the route,
// or putting a location error on them.
//
// 1. `1-waiting-for-gps.png`: the long drive started with its fixes held back.
// 2. `2-gps-lost.png`: 300 m along a named road, fixes held back for longer than
//    `staleFixAge`.
// 3. `3-location-off.png`: then the location service's "off" error on them.
// 4. `4-off-route.png`: fixes set 150 m north of the route, before the hold is up.
// 5. `5-rerouting.png`: the new route being found, if a frame catches it: the offline
//    router answers in a fraction of a second, so it's tried on a few reroutes.
// 6. `6-reroute-failed.png`: fixes set 60 km north, outside the offline area, so the
//    new route fails and the controller waits to try again.
// 7. `7-arrival.png`: a short drive to the end of the long drive's route, arrived,
//    once the camera has eased to the destination.
// 8. `8-arrival-landscape.png`: 7 in landscape.
// 9. `9-arrival-dark.png`: 7 in the dark theme.
//
// The overview and Recenter are pressed by calling the buttons' own callbacks, as a tap
// would, rather than by tapping.
//
// The router's secondary rows are rare and short-lived (the route's one is on a 25 m
// step), so 1 and 6 are each taken on a short drive of their own, from the step before
// it, rather than seven minutes into the long one.
//
// The destination is put in the app's store, as search puts a place it opens, under the
// ID [destinationId]; it's deleted by that ID at the end. Muting stores the app's
// navigation-muted setting, so `buttons` puts back the stored row as it was, or deletes
// it if there was none. That's all the drives change in the store.
import 'dart:async';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:cardinal/config.dart' show simulateNavigation;
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/main.dart' as app;
import 'package:cardinal/map/map.dart';
import 'package:cardinal/map/map_control.dart';
import 'package:cardinal/navigation/maneuver_banner.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_location.dart';
import 'package:cardinal/navigation/navigation_page.dart';
import 'package:cardinal/navigation/navigation_scope.dart';
import 'package:cardinal/navigation/navigation_status.dart';
import 'package:cardinal/navigation/road_name.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/store/src/database.dart' show SettingRow;
import 'package:cardinal/store/store.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart' show BuildContext;
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:integration_test/integration_test.dart';
import 'package:routing/routing.dart';
import 'package:sparkle/sparkle.dart';

import '../../test/offline/downloads/fake_bucket.dart';

/// Where the screenshots go: an absolute path.
const outDir = String.fromEnvironment('CARDINAL_SCREENSHOTS_DIR');

/// A release bucket on disk (`cardinal-publish --target dir:`), served at
/// `CARDINAL_RELEASES_URL` while the app runs, so it checks for updates against the
/// data it has rather than the live bucket.
const bucketDir = String.fromEnvironment('CARDINAL_SCREENSHOTS_BUCKET');
const releasesUrl = String.fromEnvironment('CARDINAL_RELEASES_URL');

/// Which screenshots to take: `banner`, `progress`, `buttons`, `steps` or `status` (see
/// above).
const shotSet = String.fromEnvironment(
  'CARDINAL_SCREENSHOTS_SET',
  defaultValue: 'banner',
);

/// The ID the destination is stored under, and deleted by.
const destinationId = 'dev:navigation-screenshots';

/// From Pike Street at 4th Avenue, downtown, to the University District: downtown's
/// streets, then I-5 and its exit to Northeast 45th Street, whose fork onto 7th Avenue
/// Northeast has a secondary row.
const start = LngLat(-122.3373, 47.6107);
const destination = LngLat(-122.3127, 47.6600);

const portrait = Size(412, 915);

void main() {
  if (!Platform.isLinux || !kProfileMode) {
    stderr.writeln(
      'navigation_screenshots: only for the Linux desktop in profile mode; '
      'run tool/navigation_screenshots.sh',
    );
    exit(64);
  }
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
    'navigation screenshots',
    timeout: const Timeout(Duration(minutes: 10)),
    (tester) async {
      if (outDir.isEmpty || !simulateNavigation) {
        fail(
          'needs CARDINAL_SCREENSHOTS_DIR and CARDINAL_SIMULATE_NAVIGATION=true',
        );
      }
      if (!const {
        'banner',
        'progress',
        'buttons',
        'steps',
        'status',
      }.contains(shotSet)) {
        fail(
          'CARDINAL_SCREENSHOTS_SET is banner, progress, buttons, steps or '
          'status, not $shotSet',
        );
      }
      await Directory(outDir).create(recursive: true);
      final bucket = await _serveBucket();
      addTearDown(() => bucket?.close());

      _setView(tester, landscape: false);
      addTearDown(tester.view.reset);
      // Light whatever the desktop's theme, but for the dark screenshot.
      _brightness(binding, Brightness.light);
      addTearDown(binding.platformDispatcher.clearPlatformBrightnessTestValue);

      app.main();
      await _settle(tester, const Duration(seconds: 2));
      final context = tester.element(find.byType(CardinalMap));
      final map = MapScope.of(context);
      await map.firstComplete.timeout(
        const Duration(seconds: 30),
        onTimeout: () {},
      );

      final router = GoRouter.of(context);
      final controller = NavigationScope.of(context).controller;
      final routes = DirectionsScope.of(context).routes;
      final route = await _route(routes, start);
      _logSteps(route);
      await StoreScope.of(context).places.put(
        const Place(
          id: destinationId,
          kind: 'poi',
          name: 'Navigation screenshots',
          center: destination,
        ),
      );
      try {
        // As search, the place's card, and directions open them.
        router.go(placeLocation(destinationId));
        await _settle(tester, const Duration(seconds: 2));
        unawaited(router.push(directionsLocation(destinationId)));
        await _settle(tester, const Duration(seconds: 2));

        if (shotSet == 'progress') {
          await _navigate(tester, context, route);
          await _progress(tester, binding, controller);
          await _end(tester, context);
          return;
        }
        if (shotSet == 'steps') {
          await _navigate(tester, context, route);
          await _steps(tester, binding, controller);
          await _end(tester, context);
          return;
        }
        if (shotSet == 'status') {
          final trip = app.simulatedTrip!;
          try {
            await _status(tester, context, controller, trip, route);
            await _end(tester, context);
            final short = await _route(routes, _beforeEnd(route));
            _logSteps(short);
            await _navigate(tester, context, short);
            await _arrival(tester, binding, controller);
            await _end(tester, context);
          } finally {
            trip
              ..paused = false
              ..offsetNorth = 0;
          }
          return;
        }
        if (shotSet == 'buttons') {
          await _navigate(tester, context, route);
          await _buttons(tester, binding, controller);
          await _end(tester, context);
          return;
        }

        await _navigate(tester, context, route);
        await _turnAndRecenter(tester, controller, map, route);
        await _end(tester, context);

        final secondaryStart = _beforeSecondary(route);
        if (secondaryStart == null) {
          debugPrint('nav-shots: no secondary row on this route');
        } else {
          final short = await _route(routes, secondaryStart);
          _logSteps(short);
          for (final (name, brightness) in [
            ('1-portrait-secondary', Brightness.light),
            ('6-portrait-secondary-dark', Brightness.dark),
          ]) {
            _brightness(binding, brightness);
            await _settle(tester, const Duration(seconds: 2));
            await _navigate(tester, context, short);
            await _secondary(tester, controller, name);
            await _end(tester, context);
          }
          _brightness(binding, Brightness.light);
        }
      } finally {
        await controller.end();
        router.go('/');
        await _settle(tester, const Duration(seconds: 1));
        await _deleteDestination();
      }
    },
  );
}

void _brightness(
  IntegrationTestWidgetsFlutterBinding binding,
  Brightness brightness,
) => binding.platformDispatcher.platformBrightnessTestValue = brightness;

/// On the long drive, once it's [into] meters along, so the drive and the camera have
/// settled: the trip-progress pill in portrait and landscape, in the dark theme, and in
/// the 24-hour clock.
Future<void> _progress(
  WidgetTester tester,
  IntegrationTestWidgetsFlutterBinding binding,
  NavigationController controller, {
  double into = 1000,
}) async {
  double? first;
  final along = await _waitFor(tester, controller, (update) {
    final left = update.progress?.distanceRemaining;
    if (left == null) return false;
    first ??= left;
    return update.visualInstruction != null && left < first! - into;
  }, within: const Duration(minutes: 8));
  if (along == null) {
    debugPrint('nav-shots: the drive never got $into m along');
    return;
  }
  await _shoot(tester, '1-portrait', controller);
  _setView(tester, landscape: true);
  await _settle(tester, const Duration(seconds: 2));
  await _shoot(tester, '2-landscape', controller);
  _setView(tester, landscape: false);
  await _settle(tester, const Duration(seconds: 2));

  _brightness(binding, Brightness.dark);
  await _settle(tester, const Duration(seconds: 2));
  await _shoot(tester, '3-portrait-dark', controller);
  _brightness(binding, Brightness.light);

  // Setting the clock tells no one, so the brightness, set again, is what has the
  // app's media query read it.
  binding.platformDispatcher.alwaysUse24HourFormatTestValue = true;
  try {
    _brightness(binding, Brightness.light);
    await _settle(tester, const Duration(seconds: 2));
    await _shoot(tester, '4-portrait-24h', controller);
  } finally {
    binding.platformDispatcher.clearAlwaysUse24HourTestValue();
    _brightness(binding, Brightness.light);
  }
}

/// On the long drive, once it's on a named road [into] meters along, so the drive and
/// the camera have settled: the road-name pill and the camera and mute buttons,
/// following in portrait, in the overview, in landscape, muted, and in the dark theme.
Future<void> _buttons(
  WidgetTester tester,
  IntegrationTestWidgetsFlutterBinding binding,
  NavigationController controller, {
  double into = 300,
}) async {
  double? first;
  final along = await _waitFor(tester, controller, (update) {
    final left = update.progress?.distanceRemaining;
    if (left == null) return false;
    first ??= left;
    return update.visualInstruction != null &&
        left < first! - into &&
        roadNameToShow(controller, following: true) != null;
  }, within: const Duration(minutes: 8));
  if (along == null) {
    debugPrint('nav-shots: the drive never got $into m along a named road');
    return;
  }
  await _shoot(tester, '1-portrait-following', controller);

  final wasMuted = controller.muted;
  final stored = await _storedMuted();
  controller.setMuted(true);
  await _settle(tester, const Duration(milliseconds: 500));
  await _shoot(tester, '4-portrait-muted', controller);
  controller.setMuted(wasMuted);
  await _settle(tester, const Duration(milliseconds: 500));
  await _restoreMuted(stored);

  _setView(tester, landscape: true);
  await _settle(tester, const Duration(seconds: 2));
  await _shoot(tester, '3-landscape-following', controller);
  _setView(tester, landscape: false);
  await _settle(tester, const Duration(seconds: 2));

  _brightness(binding, Brightness.dark);
  await _settle(tester, const Duration(seconds: 2));
  await _shoot(tester, '5-portrait-dark', controller);
  _brightness(binding, Brightness.light);
  await _settle(tester, const Duration(seconds: 2));

  _press(tester, 'Route overview');
  // Long enough for the flight there.
  await _settle(tester, const Duration(seconds: 3));
  await _shoot(tester, '2-portrait-overview', controller);
  _press(tester, 'Recenter');
  await _settle(tester, const Duration(seconds: 2));
}

/// On the long drive, once it's [into] meters along with steps ahead to list: the
/// banner closed with its handle, then open into the list in portrait, landscape and
/// the dark theme.
Future<void> _steps(
  WidgetTester tester,
  IntegrationTestWidgetsFlutterBinding binding,
  NavigationController controller, {
  double into = 300,
}) async {
  double? first;
  final along = await _waitFor(tester, controller, (update) {
    final left = update.progress?.distanceRemaining;
    if (left == null) return false;
    first ??= left;
    return update.visualInstruction != null &&
        left < first! - into &&
        update.stepIndex + 2 < controller.steps.length;
  }, within: const Duration(minutes: 8));
  if (along == null) {
    debugPrint('nav-shots: the drive never got $into m along with steps ahead');
    return;
  }
  final expanded = tester
      .widget<ManeuverBanner>(find.byType(ManeuverBanner))
      .expanded!;
  await _shoot(tester, '1-portrait-collapsed', controller);
  expanded.value = true;
  await _settle(tester, const Duration(seconds: 1));
  await _shoot(tester, '2-portrait-expanded', controller);

  _setView(tester, landscape: true);
  await _settle(tester, const Duration(seconds: 2));
  await _shoot(tester, '3-landscape-expanded', controller);
  _setView(tester, landscape: false);
  await _settle(tester, const Duration(seconds: 2));

  _brightness(binding, Brightness.dark);
  await _settle(tester, const Duration(seconds: 2));
  await _shoot(tester, '4-portrait-expanded-dark', controller);
  _brightness(binding, Brightness.light);
  expanded.value = false;
  await _settle(tester, const Duration(seconds: 1));
}

/// The status cards and the GPS-lost indicator, on the long drive, made by steering
/// [trip] (see the set's description above).
Future<void> _status(
  WidgetTester tester,
  BuildContext context,
  NavigationController controller,
  SimulatedTripLocation trip,
  DirectionsRoute route,
) async {
  trip.paused = true;
  await _navigate(tester, context, route);
  await _shoot(tester, '1-waiting-for-gps', controller);
  trip.paused = false;

  double? first;
  const into = 300.0;
  final along = await _waitFor(tester, controller, (update) {
    final left = update.progress?.distanceRemaining;
    if (left == null) return false;
    first ??= left;
    return update.visualInstruction != null &&
        left < first! - into &&
        roadNameToShow(controller, following: true) != null;
  }, within: const Duration(minutes: 8));
  if (along == null) {
    debugPrint('nav-shots: the drive never got $into m along a named road');
    return;
  }

  trip.paused = true;
  await _settle(tester, staleFixAge + const Duration(seconds: 2));
  await _shoot(tester, '2-gps-lost', controller);
  trip.fail(const NavigationLocationError(LocationProblem.off));
  await _settle(tester, const Duration(seconds: 1));
  await _shoot(tester, '3-location-off', controller);
  // The next fix puts it right.
  trip.paused = false;
  await _settle(tester, const Duration(seconds: 3));

  trip.offsetNorth = 150;
  if (await _waitForState(
    tester,
    controller,
    (c) => c.state == NavigationState.offRoute && c.rerouteFailure == null,
    within: const Duration(seconds: 20),
  )) {
    // A moment for the chevron to get there.
    await _settle(tester, const Duration(seconds: 1));
    if (controller.state == NavigationState.offRoute) {
      await _shoot(tester, '4-off-route', controller);
    }
  } else {
    debugPrint('nav-shots: never off the route');
  }

  var caught = false;
  for (var attempt = 0; attempt < 4 && !caught; attempt++) {
    caught = await _catchRerouting(tester, controller);
  }
  if (!caught) debugPrint('nav-shots: no frame caught a reroute');

  trip.offsetNorth = 60000;
  if (await _waitForState(
    tester,
    controller,
    (c) => c.state == NavigationState.offRoute && c.rerouteFailure != null,
    within: const Duration(seconds: 60),
  )) {
    await _settle(tester, const Duration(seconds: 1));
    await _shoot(tester, '6-reroute-failed', controller);
    debugPrint(
      'nav-shots: reroute failed: ${controller.rerouteFailure} '
      '(${controller.rerouteError})',
    );
  } else {
    debugPrint('nav-shots: no new route failed');
  }
  trip.offsetNorth = 0;
}

/// Waits for a frame that shows the new route being found, and screenshots it as
/// `5-rerouting`; false if none came before the next reroute could have.
Future<bool> _catchRerouting(
  WidgetTester tester,
  NavigationController controller,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 20));
  while (DateTime.now().isBefore(deadline)) {
    await tester.pump(const Duration(milliseconds: 16));
    if (controller.state != NavigationState.rerouting) continue;
    // A frame built with it, as captured.
    await tester.pump();
    if (find.text('Finding a new route…').evaluate().isEmpty) continue;
    await _capture(tester, '5-rerouting', controller);
    return true;
  }
  return false;
}

/// Waits for [test] of [controller] to hold, for up to [within]; whether it did.
Future<bool> _waitForState(
  WidgetTester tester,
  NavigationController controller,
  bool Function(NavigationController controller) test, {
  required Duration within,
}) async {
  final deadline = DateTime.now().add(within);
  while (DateTime.now().isBefore(deadline)) {
    await tester.pump(const Duration(milliseconds: 100));
    if (test(controller)) return true;
  }
  return false;
}

/// Where to start a short drive to the end of [route]: about 500 m before it, along
/// its line.
LngLat _beforeEnd(DirectionsRoute route) {
  final line = route.geometry;
  var left = 500.0;
  for (var i = line.length - 1; i > 0; i--) {
    final a = line[i], b = line[i - 1];
    final dLat = (b.lat - a.lat) * 111320;
    final dLng = (b.lng - a.lng) * 111320 * math.cos(a.lat * math.pi / 180);
    left -= math.sqrt(dLat * dLat + dLng * dLng);
    if (left <= 0) return LngLat(b.lng, b.lat);
  }
  return LngLat(line.first.lng, line.first.lat);
}

/// Waits for the short drive to arrive, then screenshots the arrival once the camera
/// has eased to the destination, in portrait, landscape and the dark theme.
Future<void> _arrival(
  WidgetTester tester,
  IntegrationTestWidgetsFlutterBinding binding,
  NavigationController controller,
) async {
  if (!await _waitForState(
    tester,
    controller,
    (c) => c.state == NavigationState.arrived,
    within: const Duration(minutes: 3),
  )) {
    debugPrint('nav-shots: the short drive never arrived');
    return;
  }
  await _settle(tester, const Duration(seconds: 3));
  await _shoot(tester, '7-arrival', controller);
  _setView(tester, landscape: true);
  await _settle(tester, const Duration(seconds: 3));
  await _shoot(tester, '8-arrival-landscape', controller);
  _setView(tester, landscape: false);
  await _settle(tester, const Duration(seconds: 2));
  _brightness(binding, Brightness.dark);
  await _settle(tester, const Duration(seconds: 2));
  await _shoot(tester, '9-arrival-dark', controller);
  _brightness(binding, Brightness.light);
  await _settle(tester, const Duration(seconds: 1));
}

/// Presses the navigation screen's map button labeled [tooltip] by calling its own
/// callback, as a tap on it would.
void _press(WidgetTester tester, String tooltip) {
  final buttons = tester.widgetList<MapControl>(find.byType(MapControl));
  buttons.singleWhere((b) => b.tooltip == tooltip).onPressed();
}

/// The fastest driving route from [from] to [destination].
Future<DirectionsRoute> _route(RouteSource routes, LngLat from) async =>
    (await routes.routes(
      mode: TravelMode.driving,
      wheelchair: false,
      from: from,
      to: destination,
    )).first;

/// Starts navigating [route] and opens the navigation screen, as directions' Start
/// Navigation does.
Future<void> _navigate(
  WidgetTester tester,
  BuildContext context,
  DirectionsRoute route,
) async {
  // Named as directions name the stored place.
  NavigationScope.of(context).controller.start(
    route: route,
    mode: TravelMode.driving,
    wheelchair: false,
    destinationName: 'Navigation screenshots',
  );
  unawaited(GoRouter.of(context).push(navigationLocation(destinationId)));
  await _settle(tester, const Duration(seconds: 2));
}

/// Ends the trip and goes back to directions, as End does.
Future<void> _end(WidgetTester tester, BuildContext context) async {
  endNavigation(context);
  await _settle(tester, const Duration(seconds: 2));
}

/// Waits for the controller's next update that matches [test], for up to [within].
Future<NavUpdate?> _waitFor(
  WidgetTester tester,
  NavigationController controller,
  bool Function(NavUpdate update) test, {
  Duration within = const Duration(minutes: 2),
}) async {
  final deadline = DateTime.now().add(within);
  while (DateTime.now().isBefore(deadline)) {
    await tester.pump(const Duration(milliseconds: 100));
    final update = controller.update;
    if (update != null && test(update)) return update;
    if (controller.state == NavigationState.arrived) return null;
  }
  return null;
}

/// On the long drive: the first plain turn, in portrait and landscape; a roundabout,
/// if the route has one; then the map moved off the chevron, for the recenter button.
Future<void> _turnAndRecenter(
  WidgetTester tester,
  NavigationController controller,
  SparkleMapController map,
  DirectionsRoute route,
) async {
  final turn = await _waitFor(tester, controller, (update) {
    final visual = update.visualInstruction;
    return visual != null &&
        visual.secondary == null &&
        visual.primary.maneuverType == ManeuverType.turn &&
        (update.progress?.distanceToNextManeuver ?? 1e9) < 400;
  });
  if (turn == null) {
    debugPrint('nav-shots: no plain turn on this route');
  } else {
    await _shoot(tester, '2-portrait-turn', controller);
    _setView(tester, landscape: true);
    await _settle(tester, const Duration(seconds: 2));
    await _shoot(tester, '3-landscape', controller);
    _setView(tester, landscape: false);
    await _settle(tester, const Duration(seconds: 2));
  }

  bool roundabout(ManeuverType? type) =>
      type == ManeuverType.roundabout || type == ManeuverType.rotary;
  if (route.navigable.steps().any(
    (step) =>
        step.visualInstructions.any((v) => roundabout(v.primary.maneuverType)),
  )) {
    final at = await _waitFor(
      tester,
      controller,
      (update) => roundabout(update.visualInstruction?.primary.maneuverType),
      within: const Duration(minutes: 8),
    );
    if (at != null) await _shoot(tester, '4-roundabout', controller);
  } else {
    debugPrint('nav-shots: no roundabout on this route');
  }

  await _recenterShot(tester, controller, map);
}

/// Where to start a short drive that soon shows [route]'s first secondary row: partway
/// along the step before it, so the camera has settled when the row comes up. Null if
/// the route has none.
LngLat? _beforeSecondary(DirectionsRoute route) {
  final steps = route.navigable.steps();
  final index = steps.indexWhere(
    (step) => step.visualInstructions.any((v) => v.secondary != null),
  );
  if (index < 0) return null;
  final line = steps[index > 0 ? index - 1 : index].geometry;
  final point = line[line.length ~/ 2];
  return LngLat(point.lng, point.lat);
}

/// Waits for a secondary row on a short drive, and screenshots it as [name].
Future<void> _secondary(
  WidgetTester tester,
  NavigationController controller,
  String name,
) async {
  final shown = await _waitFor(
    tester,
    controller,
    (update) => update.visualInstruction?.secondary != null,
    within: const Duration(seconds: 60),
  );
  if (shown == null) {
    debugPrint('nav-shots: no secondary row on the short drive for $name');
    return;
  }
  await _shoot(tester, name, controller);
}

/// Moves the map off the chevron through its controller, as a pan would, and shows
/// the recenter button that brings it back.
Future<void> _recenterShot(
  WidgetTester tester,
  NavigationController controller,
  SparkleMapController map,
) async {
  final camera = map.camera;
  if (camera == null) return;
  // A few hundred meters north-west, at the same zoom.
  map.easeTo(
    lng: camera.lng - 0.004,
    lat: camera.lat + 0.003,
    zoom: camera.zoom,
    duration: const Duration(milliseconds: 400),
  );
  await _settle(tester, const Duration(seconds: 2));
  await _shoot(tester, '5-recenter', controller);
}

/// Sets the view to a 412 × 915 phone at a device pixel ratio of 2, upright or on its
/// side, with a status bar and a gesture bar.
void _setView(WidgetTester tester, {required bool landscape}) {
  final size = landscape ? portrait.flipped : portrait;
  tester.view
    ..devicePixelRatio = 2
    ..physicalSize = size * 2
    ..padding = const FakeViewPadding(top: 24 * 2, bottom: 24 * 2)
    ..viewPadding = const FakeViewPadding(top: 24 * 2, bottom: 24 * 2);
}

/// Lets the app run for [time], drawing frames as it goes.
Future<void> _settle(WidgetTester tester, Duration time) async {
  final end = DateTime.now().add(time);
  while (DateTime.now().isBefore(end)) {
    await tester.pump(const Duration(milliseconds: 100));
  }
}

/// Saves the app's view as `<name>.png`, and logs what the banner shows.
Future<void> _shoot(
  WidgetTester tester,
  String name,
  NavigationController controller,
) async {
  // A frame with whatever just changed.
  await tester.pump(const Duration(milliseconds: 100));
  await _capture(tester, name, controller);
}

/// Saves the app's view as `<name>.png` as last drawn, and logs what the banner shows.
Future<void> _capture(
  WidgetTester tester,
  String name,
  NavigationController controller,
) async {
  final boundary = _rootBoundary(tester);
  final image = await boundary.toImage(pixelRatio: 2);
  final png = await image.toByteData(format: ui.ImageByteFormat.png);
  image.dispose();
  final file = File('$outDir/$name.png');
  await file.writeAsBytes(png!.buffer.asUint8List());
  final visual = controller.update?.visualInstruction;
  debugPrint(
    'nav-shots: $name ${image.width}x${image.height}: '
    'road "${roadNameToShow(controller, following: true)}", '
    '${controller.muted ? 'muted' : 'not muted'}; '
    '${visual?.primary.maneuverType?.name} ${visual?.primary.maneuverModifier?.name} '
    '"${visual?.primary.text}" / "${visual?.secondary?.text}" '
    'in ${controller.update?.progress?.distanceToNextManeuver.round()} m; '
    '${controller.update?.progress?.distanceRemaining.round()} m and '
    '${controller.update?.progress?.durationRemaining.round()} s left; '
    '${controller.state.name}, ${controller.locationProblem?.name}, '
    '${controller.rerouteFailure?.name}',
  );
}

/// The outermost repaint boundary: the whole of the app's view.
RenderRepaintBoundary _rootBoundary(WidgetTester tester) {
  RenderRepaintBoundary? found;
  void visit(RenderObject object) {
    if (found != null) return;
    if (object is RenderRepaintBoundary) {
      found = object;
      return;
    }
    object.visitChildren(visit);
  }

  tester.binding.renderViews.first.visitChildren(visit);
  return found!;
}

void _logSteps(DirectionsRoute route) {
  final steps = route.navigable.steps();
  for (final (i, step) in steps.indexed) {
    final banner = step.visualInstructions.firstOrNull;
    debugPrint(
      'nav-shots: step $i ${banner?.primary.maneuverType?.name} '
      '${banner?.primary.maneuverModifier?.name} "${banner?.primary.text}"'
      '${banner?.secondary == null ? '' : ' / "${banner!.secondary!.text}"'} '
      '${step.distance.round()} m',
    );
  }
}

Future<FakeBucket?> _serveBucket() async {
  if (bucketDir.isEmpty) return null;
  final port = Uri.parse(releasesUrl).port;
  return FakeBucket.serve(bucketDir, port: port);
}

/// The stored row of the navigation-muted setting, null if there's none.
Future<String?> _storedMuted() async {
  final db = CardinalDatabase.open();
  try {
    final row = await (db.select(
      db.settings,
    )..where((s) => s.key.equals(navigationMuted.key))).getSingleOrNull();
    return row?.value;
  } finally {
    await db.close();
  }
}

/// Puts the navigation-muted setting's row back to [value], or deletes it if there was
/// none, through a connection of its own, as [_deleteDestination] does.
Future<void> _restoreMuted(String? value) async {
  final db = CardinalDatabase.open();
  try {
    final rows = db.settings;
    if (value == null) {
      await (db.delete(
        rows,
      )..where((s) => s.key.equals(navigationMuted.key))).go();
    } else {
      await db
          .into(rows)
          .insertOnConflictUpdate(
            SettingRow(key: navigationMuted.key, value: value),
          );
    }
    debugPrint('nav-shots: navigation-muted setting restored to $value');
  } finally {
    await db.close();
  }
}

/// Deletes the destination from the app's store by its ID, through a connection of its
/// own: the store has no deleting of places, which the app never needs.
Future<void> _deleteDestination() async {
  final db = CardinalDatabase.open();
  try {
    final deleted = await (db.delete(
      db.places,
    )..where((p) => p.id.equals(destinationId))).go();
    debugPrint('nav-shots: deleted $deleted stored place $destinationId');
  } finally {
    await db.close();
  }
}
