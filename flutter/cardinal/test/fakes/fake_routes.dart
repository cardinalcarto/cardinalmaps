import 'dart:async';

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

/// Routes answered by the test, or at once with [immediately] if it's set.
class FakeRoutes implements RouteSource {
  FakeRoutes({this.immediately});

  List<DirectionsRoute>? immediately;
  final asked = <RouteQuestion>[];

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
    final now = immediately;
    if (now != null) question.answer.complete(now);
    return question.answer.future;
  }
}

/// A route along [points], given as `[lng, lat]`.
DirectionsRoute fakeRoute(
  List<List<double>> points, {
  double distance = 1000,
  double duration = 600,
  List<DirectionsStep> steps = const [],
}) => DirectionsRoute(
  geometry: [for (final p in points) DirectionsPoint(lat: p[1], lng: p[0])],
  distance: distance,
  duration: duration,
  steps: steps,
);
