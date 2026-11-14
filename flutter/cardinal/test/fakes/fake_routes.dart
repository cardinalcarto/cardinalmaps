import 'dart:async';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/store/store.dart';
import 'package:routing/routing.dart';

/// A question [FakeRoutes] was asked, waiting to be answered.
class RouteQuestion {
  RouteQuestion(this.mode, this.from, this.to, this.abort);

  final TravelMode mode;
  final LngLat from;
  final LngLat to;
  final Future<void>? abort;
  final answer = Completer<List<DirectionsRoute>>();
}

/// A new route [FakeRoutes] was asked for, waiting to be answered.
class RerouteQuestion {
  RerouteQuestion(this.mode, this.from, this.to, this.request);

  final TravelMode mode;
  final LngLat from;
  final LngLat to;

  /// What the navigation session would have asked the router.
  final DirectionsRequest request;
  final answer = Completer<DirectionsRoute>();
}

/// Routes answered by the test, or at once with [immediately] if it's set.
class FakeRoutes implements RouteSource {
  FakeRoutes({this.immediately});

  List<DirectionsRoute>? immediately;
  final asked = <RouteQuestion>[];
  final rerouted = <RerouteQuestion>[];

  @override
  Future<DirectionsRoute> reroute({
    required TravelMode mode,
    required LngLat from,
    required LngLat to,
    required Future<DirectionsRequest> Function(DirectionsAdapter adapter)
    request,
  }) async {
    final made = await request(FakeDirectionsAdapter());
    final question = RerouteQuestion(mode, from, to, made);
    rerouted.add(question);
    return question.answer.future;
  }

  @override
  Future<List<DirectionsRoute>> routes({
    required TravelMode mode,
    required LngLat from,
    required LngLat to,
    Future<void>? abort,
  }) {
    final question = RouteQuestion(mode, from, to, abort);
    asked.add(question);
    final now = immediately;
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

/// A route along [points], given as `[lng, lat]`, navigated in [navigationSteps].
DirectionsRoute fakeRoute(
  List<List<double>> points, {
  double distance = 1000,
  double duration = 600,
  List<DirectionsStep> steps = const [],
  List<NavStep> navigationSteps = const [],
  String language = 'en-US',
}) => DirectionsRoute(
  geometry: [for (final p in points) DirectionsPoint(lat: p[1], lng: p[0])],
  distance: distance,
  duration: duration,
  steps: steps,
  navigable: FakeNavigableRoute(
    navigationSteps: navigationSteps,
    locale: language,
  ),
);
