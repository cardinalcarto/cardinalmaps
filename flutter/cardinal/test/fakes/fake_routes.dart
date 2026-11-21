import 'dart:async';
import 'dart:typed_data';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/store/store.dart';
import 'package:routing/routing.dart';

/// A question [FakeRoutes] was asked, waiting to be answered.
class RouteQuestion {
  RouteQuestion(this.mode, this.wheelchair, this.from, this.to, this.abort);

  final TravelMode mode;
  final bool wheelchair;
  final LngLat from;
  final LngLat to;
  final Future<void>? abort;
  final answer = Completer<List<DirectionsRoute>>();
}

/// A new route [FakeRoutes] was asked for, waiting to be answered.
class RerouteQuestion {
  RerouteQuestion(this.mode, this.wheelchair, this.from, this.to, this.request);

  final TravelMode mode;
  final bool wheelchair;
  final LngLat from;
  final LngLat to;

  /// What the navigation session would have asked the router.
  final DirectionsRequest request;
  final answer = Completer<DirectionsRoute>();
}

/// Routes answered by the test, or at once with [immediately] if it's set, or for a
/// way of traveling in [immediatelyBy] with its routes there.
class FakeRoutes implements RouteSource {
  FakeRoutes({this.immediately, Map<TravelMode, List<DirectionsRoute>>? by})
    : immediatelyBy = by ?? {};

  List<DirectionsRoute>? immediately;
  final Map<TravelMode, List<DirectionsRoute>> immediatelyBy;
  final asked = <RouteQuestion>[];
  final rerouted = <RerouteQuestion>[];

  @override
  Future<DirectionsRoute> reroute({
    required TravelMode mode,
    required bool wheelchair,
    required LngLat from,
    required LngLat to,
    required Future<DirectionsRequest> Function(DirectionsAdapter adapter)
    request,
  }) async {
    final made = await request(FakeDirectionsAdapter());
    final question = RerouteQuestion(mode, wheelchair, from, to, made);
    rerouted.add(question);
    return question.answer.future;
  }

  @override
  Future<List<DirectionsRoute>> routes({
    required TravelMode mode,
    required bool wheelchair,
    required LngLat from,
    required LngLat to,
    Future<void>? abort,
  }) {
    final question = RouteQuestion(mode, wheelchair, from, to, abort);
    asked.add(question);
    final now = immediatelyBy[mode] ?? immediately;
    if (now != null) question.answer.complete(now);
    return question.answer.future;
  }
}

/// A [NavigableRoute] without the routing library, for routes that are only shown, or
/// navigated by a fake session.
class FakeNavigableRoute implements NavigableRoute {
  FakeNavigableRoute({this.navigationSteps = const [], this.locale = 'en-US'});

  final List<NavStep> navigationSteps;
  final String locale;

  @override
  List<NavStep> steps() => navigationSteps;

  @override
  String language() => locale;

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

/// A [DirectionsAdapter] without the routing library, for a fake session to make
/// requests with.
class FakeDirectionsAdapter implements DirectionsAdapter {
  @override
  DirectionsRequest request({
    required DirectionsPoint from,
    required DirectionsPoint to,
  }) => throw UnimplementedError();

  @override
  Future<List<DirectionsRoute>> parse({required List<int> body}) =>
      throw UnimplementedError();

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

/// A route along [points], given as `[lng, lat]`, navigated in [navigationSteps],
/// called by [roadNames], with what it has along it, its speed profile ([runs]) and,
/// as Valinor says them with its road details, its [elevation] and shares of ways.
DirectionsRoute fakeRoute(
  List<List<double>> points, {
  double distance = 1000,
  double duration = 600,
  List<DirectionsStep> steps = const [],
  List<NavStep> navigationSteps = const [],
  String language = 'en-US',
  List<String> roadNames = const [],
  bool toll = false,
  bool ferry = false,
  bool motorway = false,
  List<DirectionsRun> runs = const [],
  DirectionsElevation? elevation,
  double trailShare = 0,
  double bikeLaneShare = 0,
  bool stairs = false,
}) => DirectionsRoute(
  geometry: [for (final p in points) DirectionsPoint(lat: p[1], lng: p[0])],
  distance: distance,
  duration: duration,
  steps: steps,
  roadNames: roadNames,
  hasToll: toll,
  hasFerry: ferry,
  hasMotorway: motorway,
  runs: runs,
  elevation: elevation,
  trailShare: trailShare,
  bikeLaneShare: bikeLaneShare,
  hasStairs: stairs,
  navigable: FakeNavigableRoute(
    navigationSteps: navigationSteps,
    locale: language,
  ),
);

/// A run of a route's speed profile, [minutes] long at [speed] meters a second, on
/// [kind] of way if the router said.
DirectionsRun fakeRun(
  double minutes,
  double speed, {
  bool motorway = false,
  bool toll = false,
  bool ferry = false,
  DirectionsRunKind? kind,
}) => DirectionsRun(
  duration: minutes * 60,
  distance: minutes * 60 * speed,
  speed: speed,
  motorway: motorway,
  toll: toll,
  ferry: ferry,
  kind: kind,
);

/// A route's elevation: [heights] every [interval] meters, and the climb and descent
/// they make unless given.
DirectionsElevation fakeElevation(
  List<double> heights, {
  double interval = 30,
  double? climb,
  double? descent,
}) {
  var up = 0.0;
  var down = 0.0;
  for (var i = 1; i < heights.length; i++) {
    final change = heights[i] - heights[i - 1];
    if (change > 0) {
      up += change;
    } else {
      down -= change;
    }
  }
  return DirectionsElevation(
    interval: interval,
    heights: Float32List.fromList(heights),
    climb: climb ?? up,
    descent: descent ?? down,
  );
}

/// A step that says what its maneuver is.
DirectionsStep fakeStep(
  String instruction,
  ManeuverType? type, {
  ManeuverModifier? modifier,
  int? exit,
  double distance = 400,
}) => DirectionsStep(
  instruction: instruction,
  distance: distance,
  duration: distance / 10,
  maneuverType: type,
  maneuverModifier: modifier,
  roundaboutExit: exit,
);

/// Where the detailed routes below go: made-up coordinates.
const _detailedLine = [
  [0.5, 2.0],
  [1.0, 4.0],
];

/// Three ways to drive, with Valinor's road details: by freeway, with the steps'
/// maneuvers; by a tolled freeway whose toll is a long stretch in the middle; and by
/// streets and main roads over a short tolled bridge and a ferry.
final detailedDrivingRoutes = [
  fakeRoute(
    _detailedLine,
    distance: 22850,
    duration: 1320,
    roadNames: const ['I-5 South'],
    motorway: true,
    runs: [
      fakeRun(3, 8, kind: DirectionsRunKind.street),
      fakeRun(15, 27, motorway: true, kind: DirectionsRunKind.freeway),
      fakeRun(4, 12, kind: DirectionsRunKind.mainRoad),
    ],
    steps: detailedSteps,
  ),
  fakeRoute(
    _detailedLine,
    distance: 22200,
    duration: 1500,
    roadNames: const ['SR 99', 'SR 509'],
    toll: true,
    motorway: true,
    runs: [
      fakeRun(5, 10, kind: DirectionsRunKind.mainRoad),
      fakeRun(
        10,
        24,
        motorway: true,
        toll: true,
        kind: DirectionsRunKind.freeway,
      ),
      fakeRun(10, 11, kind: DirectionsRunKind.street),
    ],
  ),
  fakeRoute(
    _detailedLine,
    distance: 30000,
    duration: 2700,
    roadNames: const ['East Marginal Way South'],
    toll: true,
    ferry: true,
    runs: [
      fakeRun(20, 11, kind: DirectionsRunKind.street),
      fakeRun(0.2, 15, toll: true, kind: DirectionsRunKind.mainRoad),
      fakeRun(15, 6, ferry: true, toll: true),
      fakeRun(9.8, 12, kind: DirectionsRunKind.mainRoad),
    ],
  ),
];

/// A drive's steps, a maneuver of each family and one that doesn't say.
const detailedSteps = [
  DirectionsStep(
    instruction: 'Drive south on 5th Avenue North.',
    distance: 300,
    duration: 40,
    maneuverType: ManeuverType.depart,
  ),
  DirectionsStep(
    instruction: 'Turn left onto Denny Way.',
    distance: 600,
    duration: 80,
    maneuverType: ManeuverType.turn,
    maneuverModifier: ManeuverModifier.left,
  ),
  DirectionsStep(
    instruction: 'Take the ramp on the right to I-5 South.',
    distance: 400,
    duration: 30,
    maneuverType: ManeuverType.onRamp,
    maneuverModifier: ManeuverModifier.slightRight,
  ),
  DirectionsStep(
    instruction: 'Enter the roundabout and take the 2nd exit.',
    distance: 200,
    duration: 30,
    maneuverType: ManeuverType.roundabout,
    maneuverModifier: ManeuverModifier.right,
    roundaboutExit: 2,
  ),
  DirectionsStep(instruction: 'Keep going.', distance: 21000, duration: 1100),
  DirectionsStep(
    instruction: 'Arrive at Museum of Flight.',
    distance: 0,
    duration: 0,
    maneuverType: ManeuverType.arrive,
  ),
];

/// Three rides with Valinor's road details, each over a hill: the fastest; one mostly
/// on trails, with the least climbing; and the shortest, with the most.
final detailedCyclingRoutes = [
  fakeRoute(
    _detailedLine,
    distance: 25100,
    duration: 4680,
    roadNames: const ['Elliott Bay Trail', 'East Marginal Way South'],
    runs: [
      fakeRun(28, 5.4, kind: DirectionsRunKind.trail),
      fakeRun(30, 5.4, kind: DirectionsRunKind.bikeLane),
      fakeRun(20, 5.4, kind: DirectionsRunKind.sharedRoad),
    ],
    elevation: fakeElevation([
      for (var i = 0; i <= 836; i++) 10 + 150 * _hill(i / 836),
    ]),
    trailShare: 0.36,
    bikeLaneShare: 0.38,
  ),
  fakeRoute(
    _detailedLine,
    distance: 26200,
    duration: 5040,
    roadNames: const ['Green River Trail'],
    runs: [
      fakeRun(54, 5.2, kind: DirectionsRunKind.trail),
      fakeRun(17, 5.2, kind: DirectionsRunKind.bikeLane),
      fakeRun(13, 5.2, kind: DirectionsRunKind.sharedRoad),
    ],
    elevation: fakeElevation([
      for (var i = 0; i <= 873; i++) 10 + 40 * _hill(i / 873),
    ]),
    trailShare: 0.64,
    bikeLaneShare: 0.2,
  ),
  fakeRoute(
    _detailedLine,
    distance: 24300,
    duration: 5460,
    roadNames: const ['Beacon Avenue South'],
    runs: [
      fakeRun(7, 4.4, kind: DirectionsRunKind.trail),
      fakeRun(47, 4.4, kind: DirectionsRunKind.bikeLane),
      fakeRun(37, 4.4, kind: DirectionsRunKind.sharedRoad),
    ],
    elevation: fakeElevation([
      for (var i = 0; i <= 810; i++) 10 + 280 * _hill(i / 810),
    ]),
    bikeLaneShare: 0.52,
  ),
];

/// Two walks with Valinor's road details: one up a flight of stairs, and one mostly
/// on paths, flat.
final detailedWalkingRoutes = [
  fakeRoute(
    _detailedLine,
    distance: 2100,
    duration: 1680,
    roadNames: const ['Airport Way South'],
    runs: [
      fakeRun(10, 1.25, kind: DirectionsRunKind.sidewalk),
      fakeRun(2, 0.5, kind: DirectionsRunKind.stairs),
      fakeRun(16, 1.25, kind: DirectionsRunKind.path),
    ],
    elevation: fakeElevation([for (var i = 0; i <= 70; i++) i < 25 ? 20 : 45]),
    trailShare: 0.7,
    stairs: true,
  ),
  fakeRoute(
    _detailedLine,
    distance: 2300,
    duration: 1840,
    roadNames: const ['4th Avenue South'],
    runs: [
      fakeRun(20, 1.25, kind: DirectionsRunKind.path),
      fakeRun(10.7, 1.25, kind: DirectionsRunKind.sidewalk),
    ],
    // Ripples the climb doesn't count.
    elevation: fakeElevation([
      for (var i = 0; i <= 77; i++) 20.0 + i % 2,
    ], climb: 0),
    trailShare: 0.65,
  ),
];

/// A rise and fall from 0 at [t] = 0 to 1 at the middle and 0 again at 1.
double _hill(double t) => t < 0.5 ? 2 * t : 2 * (1 - t);
