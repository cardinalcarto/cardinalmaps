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
// `CARDINAL_SCREENSHOTS_DIR`:
//
// 1. `1-portrait-secondary.png`: the banner with a secondary row, if the route has one.
// 2. `2-portrait-turn.png`: the banner for a plain turn.
// 3. `3-landscape.png`: the same in landscape.
// 4. `4-roundabout.png`: a roundabout, if the route has one.
// 5. `5-recenter.png`: the recenter button, after the map is moved off the chevron.
// 6. `6-portrait-secondary-dark.png`: 1 in the dark theme.
//
// The router's secondary rows are rare and short-lived (the route's one is on a 25 m
// step), so 1 and 6 are each taken on a short drive of their own, from the step before
// it, rather than seven minutes into the long one.
//
// The destination is put in the app's store, as search puts a place it opens, under the
// ID [destinationId]; it's deleted by that ID at the end, which is all the drives add to
// the store.
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:cardinal/config.dart' show simulateNavigation;
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/main.dart' as app;
import 'package:cardinal/map/map.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_page.dart';
import 'package:cardinal/navigation/navigation_scope.dart';
import 'package:cardinal/routes.dart';
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

/// The fastest driving route from [from] to [destination].
Future<DirectionsRoute> _route(RouteSource routes, LngLat from) async =>
    (await routes.routes(
      mode: TravelMode.driving,
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
  NavigationScope.of(context).controller
      .start(route: route, mode: TravelMode.driving);
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
  final boundary = _rootBoundary(tester);
  final image = await boundary.toImage(pixelRatio: 2);
  final png = await image.toByteData(format: ui.ImageByteFormat.png);
  image.dispose();
  final file = File('$outDir/$name.png');
  await file.writeAsBytes(png!.buffer.asUint8List());
  final visual = controller.update?.visualInstruction;
  debugPrint(
    'nav-shots: $name ${image.width}x${image.height}: '
    '${visual?.primary.maneuverType?.name} ${visual?.primary.maneuverModifier?.name} '
    '"${visual?.primary.text}" / "${visual?.secondary?.text}" '
    'in ${controller.update?.progress?.distanceToNextManeuver.round()} m',
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
