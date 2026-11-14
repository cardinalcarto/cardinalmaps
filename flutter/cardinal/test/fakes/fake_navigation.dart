import 'dart:async';
import 'dart:typed_data';

import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_location.dart';
import 'package:cardinal/navigation/speech.dart';
import 'package:cardinal/navigation/trip_session.dart';
import 'package:routing/routing.dart';

import 'fake_routes.dart';

/// A fix at [lat], [lng], as accurate as [accuracy] says.
NavLocation fakeFix(
  double lat,
  double lng, {
  double accuracy = 5,
  double? course,
  double? speed,
  int timestampMs = 1763000000000,
}) => NavLocation(
  lat: lat,
  lng: lng,
  horizontalAccuracy: accuracy,
  course: course,
  speed: speed,
  timestampMs: timestampMs,
);

/// An update while navigating, off the route or not, with [spoken] due, showing
/// [visual] with the maneuver [toManeuver] meters off.
NavUpdate fakeUpdate({
  TripStatus status = TripStatus.navigating,
  NavDeviation deviation = NavDeviation.onRoute,
  NavSpokenInstruction? spoken,
  NavLocation? location,
  NavLocation? snapped,
  int stepIndex = 0,
  int geometryIndex = 0,
  NavVisualInstruction? visual,
  double? toManeuver,
}) => NavUpdate(
  status: status,
  location: location,
  snappedLocation: snapped,
  stepIndex: stepIndex,
  geometryIndex: geometryIndex,
  deviation: deviation,
  deviationMeters: deviation == NavDeviation.offRoute ? 100 : 0,
  spokenInstruction: spoken,
  visualInstruction: visual,
  progress: toManeuver == null
      ? null
      : NavProgress(
          distanceToNextManeuver: toManeuver,
          durationToNextManeuver: toManeuver / 10,
          distanceRemaining: toManeuver + 1000,
          durationRemaining: toManeuver / 10 + 100,
        ),
);

/// A banner line saying [text], for a maneuver of [type] and [modifier].
NavInstructionContent fakeContent(
  String text, {
  ManeuverType? type,
  ManeuverModifier? modifier,
}) => NavInstructionContent(
  text: text,
  maneuverType: type,
  maneuverModifier: modifier,
  lanes: const [],
  exitNumbers: const [],
);

/// A banner with [primary], and [secondary] under it if given.
NavVisualInstruction fakeVisual(
  NavInstructionContent primary, {
  NavInstructionContent? secondary,
}) => NavVisualInstruction(
  primary: primary,
  secondary: secondary,
  triggerDistanceBeforeManeuver: 400,
);

/// An announcement with [id], saying [text].
NavSpokenInstruction fakeSpoken(String id, [String? text]) =>
    NavSpokenInstruction(
      text: text ?? 'say $id',
      utteranceId: id,
      triggerDistanceBeforeManeuver: 100,
    );

/// A session that answers each fix as told, and records what it's asked.
///
/// [next] is what the next fix (or a new route) gets; it stays until changed. A call is
/// held while [hold] is set, until the test completes it, so tests can see fixes queue.
class FakeTripSession implements TripSession {
  FakeTripSession({NavUpdate? start}) : _current = start ?? fakeUpdate();

  NavUpdate _current;
  NavUpdate? next;
  Completer<void>? hold;
  final fixes = <NavLocation>[];
  final replaced = <(NavigableRoute, NavLocation)>[];
  final rerouteFrom = <NavLocation>[];
  bool disposed = false;

  /// How many calls are running now, which must never be more than one.
  int inFlight = 0;
  int mostInFlight = 0;

  Future<NavUpdate> _answer() async {
    inFlight++;
    if (inFlight > mostInFlight) mostInFlight = inFlight;
    try {
      await hold?.future;
      return _current = next ?? _current;
    } finally {
      inFlight--;
    }
  }

  @override
  NavUpdate current() => _current;

  @override
  Future<NavUpdate> updateLocation(NavLocation location) {
    fixes.add(location);
    return _answer();
  }

  @override
  DirectionsRequest rerouteRequest(
    DirectionsAdapter adapter,
    NavLocation location,
  ) {
    rerouteFrom.add(location);
    return DirectionsRequest(
      url: 'offline:route',
      headers: const {},
      body: Uint8List(0),
    );
  }

  @override
  Future<NavUpdate> replaceRoute(NavigableRoute route, NavLocation location) {
    replaced.add((route, location));
    return _answer();
  }

  @override
  void dispose() => disposed = true;
}

/// Starts [FakeTripSession]s, keeping each for the test.
class FakeSessions {
  final started = <(NavigableRoute, NavigationMode, NavLocation)>[];
  final sessions = <FakeTripSession>[];

  /// What the next session starts at.
  NavUpdate? start;

  Future<TripSession> call(
    NavigableRoute route,
    NavigationMode mode,
    NavLocation location,
  ) async {
    started.add((route, mode, location));
    final session = FakeTripSession(start: start);
    sessions.add(session);
    return session;
  }

  FakeTripSession get last => sessions.last;
}

/// Fixes sent by the test.
class FakeNavigationLocation implements NavigationLocationSource {
  StreamController<NavLocation>? _controller;
  final modes = <NavigationMode>[];
  int cancelled = 0;

  bool get listening => _controller != null;

  @override
  Stream<NavLocation> fixes(NavigationMode mode) {
    modes.add(mode);
    final controller = StreamController<NavLocation>(sync: true);
    controller.onCancel = () {
      cancelled++;
      if (_controller == controller) _controller = null;
    };
    _controller = controller;
    return controller.stream;
  }

  void send(NavLocation fix) => _controller!.add(fix);
  void fail(Object error) => _controller!.addError(error);

  /// Ends the stream, as a platform's may after an error.
  Future<void> close() async {
    final controller = _controller!;
    _controller = null;
    await controller.close();
  }
}

/// A speaker that writes down what it's told.
class FakeSpeaker implements Speaker {
  final languages = <String>[];
  final said = <String>[];
  int stopped = 0;

  /// What's said and each stop, in order, so tests can tell what a stop cut off.
  final log = <String>[];

  @override
  Future<void> prepare(String language) async => languages.add(language);

  @override
  Future<void> speak(String text) async {
    said.add(text);
    log.add(text);
  }

  @override
  Future<void> stop() async {
    stopped++;
    log.add('(stop)');
  }
}

/// Whether the screen would be kept on.
class FakeScreenWake implements ScreenWake {
  bool on = false;

  @override
  Future<void> keepOn() async => on = true;

  @override
  Future<void> release() async => on = false;
}

/// A controller over fakes: its own, or those given.
NavigationController fakeNavigationController({
  FakeRoutes? routes,
  FakeNavigationLocation? location,
  FakeSessions? sessions,
}) => NavigationController(
  routes: routes ?? FakeRoutes(),
  location: location ?? FakeNavigationLocation(),
  speaker: FakeSpeaker(),
  screen: FakeScreenWake(),
  startSession: (sessions ?? FakeSessions()).call,
);
