// On-device checks of turn-by-turn navigation, for a phone in profile or release mode.
// A development tool, not a test: build it with `tool/navigation_phone_checks.sh`, which
// makes an APK with this file as its entry point, install that with `adb install -r`
// (which keeps the app's data and offline regions), and launch it from the launcher or
// with `adb shell am start`. It never runs through `flutter drive`, which uninstalls the
// app when it ends, and it refuses to run in debug mode or off Android.
//
// It runs the app's real `main()` and then, by `CARDINAL_NAV_PHONE_CHECKS` (a comma-
// separated list), some of:
//
// - `latency`: how long `OfflineRouter.matchPosition` takes per fix, over the windows a
//   trip matches (each fix with up to 29 before it, a second apart, set 3 m to the left
//   and thrown up to 3 m more each way, as `match_latency` in the routing library makes
//   them), along routes the offline router finds in Seattle, driving, cycling and
//   walking. Each route is matched twice on a router of its own, opened fresh: first
//   cold, with no candidate grids, then again over the same windows, warm. Then how long
//   `NavigationSession.updateLocation` takes per fix along the route, with its matches.
// - `voice`: a simulated trip (the build needs `CARDINAL_SIMULATE_NAVIGATION=true`)
//   through the app's own navigation controller and speaker, on the navigation screen,
//   then a second simulated trip on a controller of its own, with a `PlatformSpeaker`,
//   whose fixes leave the route 150 m to one side while an announcement is being said,
//   so it reroutes. The speech engine's events, engines and voices are logged.
// - `screenoff`: a walking trip from the phone's own position on the real location
//   service (a build without simulation), on the navigation screen, for
//   `CARDINAL_NAV_PHONE_SECONDS` (default 300), logging each fix's arrival, so the
//   screen can be turned off meanwhile and the fixes checked to keep coming.
//
// Everything goes to the log as `navphone: ...` lines, and `navphone: done` ends it. No
// coordinates are logged. The trips' destination is put in the app's store under
// [destinationId], and deleted by that ID at the end; that's all it changes there.
//
// The map's element, which the checks look the app's scopes up from, stays mounted for
// as long as the app runs, so it's used across awaits.
// ignore_for_file: use_build_context_synchronously
import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:cardinal/config.dart' show simulateNavigation;
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/main.dart' as app;
import 'package:cardinal/map/map.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_location.dart';
import 'package:cardinal/navigation/navigation_page.dart';
import 'package:cardinal/navigation/navigation_scope.dart';
import 'package:cardinal/navigation/road_matcher.dart';
import 'package:cardinal/navigation/speech.dart';
import 'package:cardinal/offline/offline_area.dart';
import 'package:cardinal/offline/offline_scope.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' hide NavigationMode;
import 'package:flutter_tts/flutter_tts.dart';
import 'package:geolocator/geolocator.dart';
import 'package:go_router/go_router.dart';
import 'package:routing/routing.dart';

const checks = String.fromEnvironment(
  'CARDINAL_NAV_PHONE_CHECKS',
  defaultValue: 'latency,voice',
);
const screenOffSeconds = int.fromEnvironment(
  'CARDINAL_NAV_PHONE_SECONDS',
  defaultValue: 300,
);

/// The ID the trips' destination is stored under, and deleted by.
const destinationId = 'dev:navigation-phone-checks';

void log(String message) => debugPrint('navphone: $message');

/// The trips matched for latency, as `match_latency` in the routing library has them:
/// name, how, speed in meters per second, from, to.
const latencyTrips = [
  // Northgate to Sea-Tac: city streets, then I-5.
  (
    'auto north-south',
    TravelMode.driving,
    15.0,
    LngLat(-122.3255, 47.7062),
    LngLat(-122.3088, 47.4502),
  ),
  // Westlake to Cal Anderson Park: downtown's grid, where the most roads are.
  (
    'auto downtown',
    TravelMode.driving,
    10.0,
    LngLat(-122.3375, 47.6114),
    LngLat(-122.3191692, 47.6170378),
  ),
  (
    'bicycle',
    TravelMode.cycling,
    5.0,
    LngLat(-122.3500, 47.6510),
    LngLat(-122.3035, 47.6553),
  ),
  (
    'pedestrian',
    TravelMode.walking,
    1.4,
    LngLat(-122.3422, 47.6094),
    LngLat(-122.3493, 47.6205),
  ),
];

/// The most fixes a latency trip is matched over: 15 minutes of them.
const maxLatencyFixes = 900;

/// The simulated trips' route: Westlake to Cal Anderson Park, about 2 km downtown.
const voiceFrom = LngLat(-122.3375, 47.6114);
const voiceTo = LngLat(-122.3191692, 47.6170378);

Future<void> main() async {
  if (kDebugMode || !Platform.isAndroid) {
    stderr.writeln(
      'navigation_phone_checks: only on Android in profile or release mode; '
      'build it with tool/navigation_phone_checks.sh',
    );
    exit(64);
  }
  app.main();
  try {
    final context = await _mapContext();
    final wanted = checks.split(',').map((c) => c.trim()).toSet();
    log('checks $wanted, simulated ${simulateNavigation ? 'yes' : 'no'}');
    if (wanted.contains('latency')) await _latency(context);
    if (wanted.contains('voice')) {
      if (!simulateNavigation) {
        log('FAIL voice needs CARDINAL_SIMULATE_NAVIGATION=true');
      } else {
        await _voice(context);
      }
    }
    if (wanted.contains('screenoff')) {
      if (simulateNavigation) {
        log('FAIL screenoff needs a build without simulation');
      } else {
        await _screenOff(context);
      }
    }
  } on Object catch (e, stack) {
    log('FAIL $e\n$stack');
  } finally {
    await _deleteDestination();
    log('done');
  }
}

/// The map's element, under every scope the app puts up, once the app has drawn it.
Future<BuildContext> _mapContext() async {
  final deadline = DateTime.now().add(const Duration(seconds: 60));
  while (DateTime.now().isBefore(deadline)) {
    await Future<void>.delayed(const Duration(milliseconds: 500));
    Element? found;
    void visit(Element element) {
      if (found != null) return;
      if (element.widget is CardinalMap) {
        found = element;
        return;
      }
      element.visitChildElements(visit);
    }

    WidgetsBinding.instance.rootElement?.visitChildElements(visit);
    if (found != null) {
      // Let the map and the offline area settle.
      await Future<void>.delayed(const Duration(seconds: 3));
      return found!;
    }
  }
  throw StateError('the map never showed');
}

double _at(List<double> sorted, double q) =>
    sorted[((sorted.length - 1) * q).round()];

String _stats(List<double> times) {
  if (times.isEmpty) return 'n 0';
  final sorted = [...times]..sort();
  return 'n ${sorted.length} median ${_at(sorted, 0.5).toStringAsFixed(2)} '
      'p95 ${_at(sorted, 0.95).toStringAsFixed(2)} '
      'max ${sorted.last.toStringAsFixed(2)} ms';
}

double _ms(Stopwatch watch) => watch.elapsedMicroseconds / 1000;

// ---------------------------------------------------------------------------------------
// Latency

Future<void> _latency(BuildContext context) async {
  final downloads = await OfflineScope.of(context).downloads;
  final area = await configuredOfflineArea(downloads);
  if (area == null) {
    log('FAIL latency: no offline area');
    return;
  }
  await initRouting();
  final tiles = ['${area.routing}/tiles'];
  log('latency: routing release ${area.routing.split('/').last}');
  final routeRouter = await OfflineRouter.open(
    tileSets: tiles,
    expectedBuildId: area.valhallaBuildId,
  );
  final byMode = <TravelMode, Map<String, List<double>>>{};
  try {
    for (final (name, mode, speed, from, to) in latencyTrips) {
      final route = await _routeWith(routeRouter, mode, from, to);
      if (route == null) {
        log('FAIL latency $name: no route');
        continue;
      }
      final fixes = _noisyFixes(route, speed);
      log(
        'latency $name: route ${route.distance.round()} m, '
        '${route.navigable.steps().length} steps, '
        '${route.geometry.length} points; ${fixes.length} fixes',
      );
      final navigation = navigationMode(mode)!;
      final times = byMode.putIfAbsent(mode, () => {});
      final router = await OfflineRouter.open(
        tileSets: tiles,
        expectedBuildId: area.valhallaBuildId,
      );
      try {
        final matches = <MatchedPosition?>[];
        for (final pass in ['cold', 'warm']) {
          final passTimes = <double>[];
          var unmatched = 0;
          for (var i = 0; i < fixes.length; i++) {
            final window = fixes.sublist(
              math.max(0, i - (maxMatchFixes - 1)),
              i + 1,
            );
            final watch = Stopwatch()..start();
            final matched = await router.matchPosition(
              mode: navigation,
              wheelchair: false,
              fixes: window,
            );
            passTimes.add(_ms(watch));
            if (matched == null) unmatched++;
            if (pass == 'warm') matches.add(matched);
          }
          if (pass == 'cold') {
            log(
              'latency $name: first match on a fresh router '
              '${passTimes.first.toStringAsFixed(2)} ms',
            );
            times.putIfAbsent('first', () => []).add(passTimes.first);
          }
          log(
            'latency $name $pass: ${_stats(passTimes)}, '
            '$unmatched unmatched',
          );
          times.putIfAbsent(pass, () => []).addAll(passTimes);
        }

        // The session, fed each fix with its match, as the controller does.
        final session = await NavigationSession.start(
          route: route.navigable,
          mode: navigation,
          location: fixes.first,
        );
        try {
          final updates = <double>[];
          var followed = 0, offRoute = 0;
          for (var i = 1; i < fixes.length; i++) {
            final watch = Stopwatch()..start();
            final update = await session.updateLocation(
              location: fixes[i],
              matched: matches[i],
            );
            updates.add(_ms(watch));
            if (update.matched) followed++;
            if (update.deviation == NavDeviation.offRoute) offRoute++;
            if (update.status == TripStatus.complete) break;
          }
          log(
            'latency $name updateLocation: ${_stats(updates)}; '
            'followed $followed matches, $offRoute off route',
          );
          times.putIfAbsent('update', () => []).addAll(updates);
        } finally {
          session.dispose();
        }
      } finally {
        router.dispose();
      }
    }
  } finally {
    routeRouter.dispose();
  }
  for (final MapEntry(key: mode, value: times) in byMode.entries) {
    for (final kind in ['first', 'cold', 'warm', 'update']) {
      log('latency summary ${mode.name} $kind: ${_stats(times[kind] ?? [])}');
    }
  }
}

/// The best route by [mode] from [from] to [to] on [router], in English and miles.
Future<DirectionsRoute?> _routeWith(
  OfflineRouter router,
  TravelMode mode,
  LngLat from,
  LngLat to,
) async {
  final adapter = DirectionsAdapter.valhalla(
    endpointUrl: 'offline:route',
    costing: mode.costing!,
    alternates: 0,
    language: 'en-US',
    units: DistanceUnits.miles,
    wheelchair: false,
  );
  final request = adapter.request(
    from: DirectionsPoint(lat: from.lat, lng: from.lon),
    to: DirectionsPoint(lat: to.lat, lng: to.lon),
  );
  final response = await router.route(body: request.body);
  final routes = await adapter.parse(body: response.body);
  return routes.firstOrNull;
}

/// Fixes a second apart along [route] at [speed], 3 m to its left and up to 3 m more
/// each way at random, claiming 5 m of accuracy, as `match_latency` makes them.
List<NavLocation> _noisyFixes(DirectionsRoute route, double speed) {
  final simulation = LocationSimulation(
    route: route.navigable,
    resampleDistance: speed,
    bias: SimulationBias.left,
    biasMeters: 3,
  );
  final random = math.Random(7);
  final start = DateTime.now().millisecondsSinceEpoch - 3600 * 1000;
  final fixes = <NavLocation>[];
  var fix = simulation.current();
  while (fixes.length < maxLatencyFixes) {
    final dLat = (random.nextDouble() * 2 - 1) * 3 / 111320;
    final dLng =
        (random.nextDouble() * 2 - 1) *
        3 /
        (111320 * math.cos(fix.lat * math.pi / 180));
    fixes.add(
      NavLocation(
        lat: fix.lat + dLat,
        lng: fix.lng + dLng,
        horizontalAccuracy: 5,
        course: fix.course,
        courseAccuracy: fix.course == null ? null : 10,
        speed: speed,
        speedAccuracy: 1,
        timestampMs: start + fixes.length * 1000,
      ),
    );
    if (simulation.isFinished()) break;
    fix = simulation.advance();
  }
  simulation.dispose();
  return fixes;
}

// ---------------------------------------------------------------------------------------
// Voice

Future<void> _voice(BuildContext context) async {
  // flutter_tts has one channel, whose callbacks go to the instance made last, so the
  // speech engine's events are watched through what the speakers hear
  // ([PlatformSpeaker.debugEngineEvents]), each of which makes its instance as its trip
  // starts, after the one here for the engine's voices.
  final watch = Stopwatch()..start();
  String at() => 't=${(watch.elapsedMilliseconds / 1000).toStringAsFixed(1)}s';
  var speaking = false;
  PlatformSpeaker.debugEngineEvents = (event) {
    speaking = event == 'start';
    log('tts $event ${at()}');
  };
  final tts = FlutterTts();
  try {
    log('tts engines ${await tts.getEngines}');
    log('tts default engine ${await tts.getDefaultEngine}');
    log('tts default voice ${await tts.getDefaultVoice}');
    final voices = (await tts.getVoices as List?) ?? const [];
    final english = [
      for (final v in voices)
        if ('${(v as Map)['locale']}'.startsWith('en')) v['name'],
    ];
    log('tts ${voices.length} voices, ${english.length} English: $english');
    for (final language in [
      'en-US',
      'en-GB',
      'es-ES',
      'es-US',
      'fr-FR',
      'de-DE',
      'pt-PT',
      'pt-BR',
      'it-IT',
      'ja-JP',
      'zh-CN',
      'ar',
      'hi-IN',
    ]) {
      final available = await tts.isLanguageAvailable(language);
      final installed = await tts.isLanguageInstalled(language);
      log('tts $language available $available installed $installed');
    }
  } on Object catch (e) {
    log('tts query failed: $e');
  }

  final routes = DirectionsScope.of(context).routes;
  final route = (await routes.routes(
    mode: TravelMode.driving,
    wheelchair: false,
    from: voiceFrom,
    to: voiceTo,
  )).first;
  final controller = NavigationScope.of(context).controller;
  final router = GoRouter.of(context);
  log(
    'voice: route language ${route.navigable.language()}, '
    '${route.distance.round()} m, muted ${controller.muted}',
  );
  await StoreScope.of(context).places.put(
    const Place(
      id: destinationId,
      kind: 'poi',
      name: 'Navigation phone checks',
      center: voiceTo,
    ),
  );

  // The app's own trip: its controller, simulated fixes and speaker, on its screen.
  router.go(placeLocation(destinationId));
  await Future<void>.delayed(const Duration(seconds: 2));
  unawaited(router.push(directionsLocation(destinationId)));
  await Future<void>.delayed(const Duration(seconds: 2));
  final spokenLog = _logTrip('app trip', controller, at);
  controller.start(route: route, mode: TravelMode.driving, wheelchair: false);
  unawaited(router.push(navigationLocation(destinationId)));
  log('voice: app trip started ${at()}, language ${controller.language}');
  final arrived = await _until(
    () => controller.state == NavigationState.arrived,
    const Duration(minutes: 4),
  );
  log('voice: app trip ${arrived ? 'arrived' : 'timed out'} ${at()}');
  // Long enough for the arrival to be said.
  await Future<void>.delayed(const Duration(seconds: 8));
  endNavigation(context);
  controller.removeListener(spokenLog);
  await Future<void>.delayed(const Duration(seconds: 2));
  router.go('/');

  // A trip that leaves the route, on a controller of its own.
  final deviating = _DeviatingLocation(route.navigable);
  final offline = await configuredOfflineArea(
    await OfflineScope.of(context).downloads,
  );
  final second = NavigationController(
    routes: routes,
    location: deviating,
    speaker: PlatformSpeaker(),
    screen: _NoScreen(),
    matcher: OfflineRoadMatcher(() async => offline),
    settings: StoreScope.of(context).settings,
  );
  final secondLog = _logTrip('reroute trip', second, at);
  // Off the route as a long announcement on it comes due, so it's still being said when
  // the trip finds it's off: the speech should be cut off, and "Rerouting" said.
  String? lastDue;
  void leaveDuringAnnouncement() {
    final spoken = second.update?.spokenInstruction;
    if (spoken == null || spoken.utteranceId == lastDue) return;
    lastDue = spoken.utteranceId;
    if (second.state == NavigationState.onRoute &&
        deviating.sent > 15 &&
        spoken.text.length >= 40 &&
        !deviating.deviating) {
      deviating.deviating = true;
      log('voice: leaving the route as a long announcement comes due ${at()}');
    }
  }

  second.addListener(leaveDuringAnnouncement);
  second.start(route: route, mode: TravelMode.driving, wheelchair: false);
  log('voice: reroute trip started ${at()}');
  // At least 40 s off the route, ending back on a new route rather than in the middle
  // of leaving it.
  await _until(() {
    return second.state == NavigationState.arrived ||
        (deviating.deviatedAt != null &&
            DateTime.now().difference(deviating.deviatedAt!) >
                const Duration(seconds: 40) &&
            second.state == NavigationState.onRoute &&
            !speaking);
  }, const Duration(minutes: 4));
  second.removeListener(leaveDuringAnnouncement);
  if (!deviating.deviating) {
    log('voice: the reroute trip never left the route');
  }
  log(
    'voice: speaking at end $speaking, route rerouted ${second.route != route}',
  );
  await second.end();
  second.removeListener(secondLog);
  second.dispose();
  PlatformSpeaker.debugEngineEvents = null;
}

/// Logs [controller]'s state changes and the announcements it comes to, as they come.
VoidCallback _logTrip(
  String name,
  NavigationController controller,
  String Function() at,
) {
  NavigationState? state;
  String? utterance;
  void listener() {
    if (controller.state != state) {
      state = controller.state;
      log('$name: state ${state!.name} ${at()}');
    }
    final spoken = controller.update?.spokenInstruction;
    if (spoken != null && spoken.utteranceId != utterance) {
      utterance = spoken.utteranceId;
      log('$name: announcement due ${at()} "${spoken.text}"');
    }
  }

  controller.addListener(listener);
  return listener;
}

Future<bool> _until(bool Function() test, Duration within) async {
  final deadline = DateTime.now().add(within);
  while (DateTime.now().isBefore(deadline)) {
    if (test()) return true;
    await Future<void>.delayed(const Duration(milliseconds: 200));
  }
  return test();
}

/// Simulated fixes along a route that, once [deviating] is set, carry on 150 m to the
/// right of it, as a driver who missed a turn onto a parallel street would be.
class _DeviatingLocation implements NavigationLocationSource {
  _DeviatingLocation(this.route);

  final NavigableRoute route;
  bool deviating = false;
  DateTime? deviatedAt;

  /// How many fixes have been given.
  int sent = 0;

  /// The offset, in degrees, fixed when the fixes leave the route.
  double? _dLat, _dLng;

  @override
  Stream<NavLocation> fixes(NavigationMode mode) {
    final speed = SimulatedLocationSource.speed(mode);
    Timer? timer;
    late final StreamController<NavLocation> controller;
    controller = StreamController<NavLocation>(
      onListen: () {
        final simulation = LocationSimulation(
          route: route,
          resampleDistance: speed,
          bias: SimulationBias.none,
          biasMeters: 0,
        );
        timer = Timer.periodic(const Duration(seconds: 1), (_) {
          final fix = simulation.isFinished()
              ? simulation.current()
              : simulation.advance();
          sent++;
          // If no long announcement came due to leave during, leave at the 120th fix.
          if (sent >= 120 && !deviating) {
            deviating = true;
            log('voice: leaving the route at fix $sent, between announcements');
          }
          if (deviating && _dLat == null) {
            deviatedAt = DateTime.now();
            final right = ((fix.course ?? 0) + 90) * math.pi / 180;
            _dLat = 150 * math.cos(right) / 111320;
            _dLng =
                150 *
                math.sin(right) /
                (111320 * math.cos(fix.lat * math.pi / 180));
          }
          controller.add(
            NavLocation(
              lat: fix.lat + (_dLat ?? 0),
              lng: fix.lng + (_dLng ?? 0),
              horizontalAccuracy: 5,
              course: fix.course,
              courseAccuracy: fix.course == null ? null : 10,
              speed: speed,
              speedAccuracy: 1,
              timestampMs: DateTime.now().millisecondsSinceEpoch,
            ),
          );
        });
      },
      onCancel: () => timer?.cancel(),
    );
    return controller.stream;
  }
}

class _NoScreen implements ScreenWake {
  @override
  Future<void> keepOn() async {}

  @override
  Future<void> release() async {}
}

// ---------------------------------------------------------------------------------------
// Screen off

Future<void> _screenOff(BuildContext context) async {
  final watch = Stopwatch()..start();
  String at() => 't=${(watch.elapsedMilliseconds / 1000).toStringAsFixed(1)}s';
  final lifecycle = AppLifecycleListener(
    onStateChange: (state) => log('lifecycle ${state.name} ${at()}'),
  );
  try {
    Position? position;
    try {
      position = await Geolocator.getCurrentPosition(
        locationSettings: AndroidSettings(
          accuracy: LocationAccuracy.high,
          forceLocationManager: true,
          timeLimit: const Duration(seconds: 90),
        ),
      );
      log('screenoff: current fix, accuracy ${position.accuracy.round()} m');
    } on Object catch (e) {
      log('screenoff: no current fix ($e); trying the last known');
      position = await Geolocator.getLastKnownPosition(
        forceAndroidLocationManager: true,
      );
      if (position != null) {
        log(
          'screenoff: last known fix, accuracy ${position.accuracy.round()} m, '
          '${DateTime.now().difference(position.timestamp).inMinutes} min old',
        );
      }
    }
    if (position == null) {
      log('FAIL screenoff: no position');
      return;
    }
    final from = LngLat(position.longitude, position.latitude);
    // About 600 m north-east.
    final to = LngLat(
      position.longitude +
          420 / (111320 * math.cos(position.latitude * math.pi / 180)),
      position.latitude + 420 / 111320,
    );
    final routes = DirectionsScope.of(context).routes;
    final route = (await routes.routes(
      mode: TravelMode.walking,
      wheelchair: false,
      from: from,
      to: to,
    )).first;
    log('screenoff: walking route ${route.distance.round()} m');
    await StoreScope.of(context).places.put(
      Place(
        id: destinationId,
        kind: 'poi',
        name: 'Navigation phone checks',
        center: to,
      ),
    );
    final controller = NavigationScope.of(context).controller;
    final router = GoRouter.of(context);
    router.go(placeLocation(destinationId));
    await Future<void>.delayed(const Duration(seconds: 2));
    unawaited(router.push(directionsLocation(destinationId)));
    await Future<void>.delayed(const Duration(seconds: 2));

    DateTime? lastFix;
    var count = 0;
    NavigationState? state;
    void listener() {
      if (controller.state != state) {
        state = controller.state;
        log('screenoff: state ${state!.name} ${at()}');
      }
      final fixAt = controller.lastFixAt;
      if (fixAt != null && fixAt != lastFix) {
        count++;
        final gap = lastFix == null
            ? '-'
            : '${fixAt.difference(lastFix!).inMilliseconds} ms';
        final accuracy = controller.update?.location?.horizontalAccuracy;
        log(
          'screenoff: fix $count ${at()} gap $gap '
          'accuracy ${accuracy?.round() ?? '?'} state ${controller.state.name} '
          'problem ${controller.locationProblem?.name}',
        );
        lastFix = fixAt;
      }
    }

    controller.addListener(listener);
    controller.start(route: route, mode: TravelMode.walking, wheelchair: false);
    unawaited(router.push(navigationLocation(destinationId)));
    log('screenoff: trip started ${at()}, running ${screenOffSeconds}s');
    final end = DateTime.now().add(const Duration(seconds: screenOffSeconds));
    while (DateTime.now().isBefore(end)) {
      await Future<void>.delayed(const Duration(seconds: 10));
      log(
        'screenoff: tick ${at()} state ${controller.state.name}, $count fixes, '
        'last ${lastFix == null ? '-' : '${DateTime.now().difference(lastFix!).inMilliseconds} ms ago'}, '
        'problem ${controller.locationProblem?.name}',
      );
    }
    log(
      'screenoff: ending ${at()}: state ${controller.state.name}, $count fixes',
    );
    final ctx = context.mounted ? context : null;
    if (ctx != null) endNavigation(ctx);
    await controller.end();
    controller.removeListener(listener);
    await Future<void>.delayed(const Duration(seconds: 2));
    router.go('/');
  } finally {
    lifecycle.dispose();
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
    log('deleted $deleted stored place $destinationId');
  } finally {
    await db.close();
  }
}
